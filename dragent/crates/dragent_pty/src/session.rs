use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use async_channel::Receiver as AsyncReceiver;
use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};

use crate::event_loop::{spawn_event_loop, FeedCallback, SyncBufferLenCallback, WakeupCallback};
use crate::messages::{PtyCommand, PtyEvent};
#[cfg(unix)]
use crate::mio_channel::{self, Sender as CmdSender};
use crate::shell_env::{apply_login_shell_arg, configure_shell_command, resolve_shell};
#[cfg(windows)]
use crate::windows::poll_channel::{self, Sender as CmdSender};

/// Configuration for a long-lived interactive PTY session.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            shell: None,
            cwd: std::env::current_dir().ok(),
            rows: 24,
            cols: 80,
        }
    }
}

/// Result of a one-shot command execution.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub output: Vec<String>,
    pub exit_code: i32,
}

/// On-demand probe for whether a foreground job (a running command) owns the
/// terminal, versus the shell sitting at an idle prompt. Holds a dup of the
/// master fd so the UI thread can `tcgetpgrp` it directly without touching the
/// event-loop thread. Unix only.
#[cfg(unix)]
struct ForegroundProbe {
    /// Dup of the PTY master fd (owned; closed on drop).
    master_fd: std::os::unix::io::RawFd,
    /// The interactive shell's pgid (== its pid). A different foreground group
    /// means a command is running.
    shell_pgid: i32,
}

#[cfg(unix)]
impl ForegroundProbe {
    fn is_running(&self) -> bool {
        let fg = unsafe { libc::tcgetpgrp(self.master_fd) };
        fg > 0 && fg != self.shell_pgid
    }
}

#[cfg(unix)]
impl Drop for ForegroundProbe {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.master_fd);
        }
    }
}

/// Handle for sending commands to a running interactive PTY session.
#[derive(Clone)]
pub struct PtySessionHandle {
    cmd_tx: CmdSender<PtyCommand>,
    /// Flow control, read by the event loop on every pass.
    read_paused: Arc<std::sync::atomic::AtomicBool>,
    /// Foreground-job probe (Unix only; `None` when unavailable).
    #[cfg(unix)]
    foreground: Option<Arc<ForegroundProbe>>,
}

impl PtySessionHandle {
    pub fn write(&self, data: &[u8]) -> Result<()> {
        self.cmd_tx
            .send(PtyCommand::Write(data.to_vec()))
            .context("PTY session command channel closed")
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.cmd_tx
            .send(PtyCommand::Resize { rows, cols })
            .context("PTY session command channel closed")
    }

    /// Send SIGINT to the child's process group (Ctrl+C).
    pub fn interrupt(&self) -> Result<()> {
        self.cmd_tx
            .send(PtyCommand::Interrupt)
            .context("PTY session command channel closed")
    }

    /// Firmly stop the running foreground job and its process group (SIGINT then
    /// SIGTERM), without killing the shell itself. Stronger than [`interrupt`]
    /// for dev servers that trap or ignore Ctrl+C.
    ///
    /// [`interrupt`]: Self::interrupt
    pub fn terminate(&self) -> Result<()> {
        self.cmd_tx
            .send(PtyCommand::Terminate)
            .context("PTY session command channel closed")
    }

    /// What a successor process needs to adopt this session: a PTY master fd
    /// (owned by this handle; the caller must clear `FD_CLOEXEC` in the
    /// child only) and the shell's pid. Unix only.
    #[cfg(unix)]
    pub fn handoff_parts(&self) -> Option<(std::os::fd::RawFd, u32)> {
        self.foreground
            .as_ref()
            .map(|probe| (probe.master_fd, probe.shell_pgid as u32))
    }

    /// Flow control: pause or resume reading this session's output. Takes
    /// effect on the loop's next pass, even mid-flood.
    pub fn set_read_paused(&self, paused: bool) -> Result<()> {
        self.read_paused
            .store(paused, std::sync::atomic::Ordering::Relaxed);
        self.cmd_tx
            .send(PtyCommand::PauseRead(paused))
            .context("PTY session command channel closed")
    }

    pub fn shutdown(&self) -> Result<()> {
        self.cmd_tx
            .send(PtyCommand::Shutdown)
            .context("PTY session command channel closed")
    }

    /// The longest line the terminal can take right now; `None` when it has no
    /// limit. A program that reads whole lines (a shell that isn't at its
    /// prompt yet, unlike one editing a line) leaves them to the terminal,
    /// which drops what it can't hold.
    pub fn line_limit(&self) -> Option<usize> {
        #[cfg(unix)]
        {
            let probe = self.foreground.as_ref()?;
            // SAFETY: the probe owns the descriptor, and `state` is only read
            // once filled.
            let state = unsafe {
                let mut state: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(probe.master_fd, &mut state) != 0 {
                    return None;
                }
                state
            };
            // Both one less than the buffer, for the line's end.
            let held = if cfg!(target_os = "linux") {
                4095
            } else {
                1023
            };
            (state.c_lflag & libc::ICANON != 0).then_some(held)
        }
        // The console host gives a program its lines whole.
        #[cfg(windows)]
        {
            None
        }
    }

    /// True when a foreground job (a running command) currently owns the
    /// terminal — i.e. the shell is NOT at an idle prompt. This reflects a
    /// SILENT running process too (unlike output-activity heuristics). Unix
    /// only; always `false` on Windows (ConPTY has no process-group concept),
    /// where callers should fall back to an activity heuristic.
    pub fn foreground_running(&self) -> bool {
        #[cfg(unix)]
        {
            self.foreground
                .as_ref()
                .map(|p| p.is_running())
                .unwrap_or(false)
        }
        #[cfg(windows)]
        {
            false
        }
    }
}

/// Log (once) which ConPTY backend this process will use. portable-pty
/// prefers a `conpty.dll` sideloaded next to the executable — shipped by
/// `scripts/provision-conpty.ps1` alongside its `OpenConsole.exe` host — and
/// falls back to the OS in-box console, whose behavior varies by Windows
/// build. Knowing which one is active is the first question in any
/// escape-sequence bug report. No-op on Unix (PTYs are kernel primitives).
fn log_conpty_backend() {
    #[cfg(windows)]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let bundled = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|d| d.join("conpty.dll")))
                .filter(|p| p.is_file());
            match bundled {
                Some(dll) => log::info!(
                    "[pty] using bundled ConPTY: {} (pinned; see scripts/provision-conpty.ps1)",
                    dll.display()
                ),
                None => log::info!(
                    "[pty] using OS in-box ConPTY (conhost) — behavior varies by Windows build; \
                     run scripts/provision-conpty.ps1 to pin"
                ),
            }
        });
    }
}

/// Spawns an interactive shell session with a single multiplexed PTY event loop.
pub fn spawn_interactive(
    config: PtyConfig,
    feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
) -> Result<(PtySessionHandle, AsyncReceiver<PtyEvent>)> {
    spawn_interactive_with_args(config, &[], &[], feed, sync_buffer_len, on_wakeup)
}

/// [`spawn_interactive`] with extra shell arguments after the login flag,
/// e.g. a startup command that loads shell integration without typing it, and
/// extra variables in the shell's environment.
pub fn spawn_interactive_with_args(
    config: PtyConfig,
    args: &[String],
    env: &[(&str, String)],
    feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
) -> Result<(PtySessionHandle, AsyncReceiver<PtyEvent>)> {
    log_conpty_backend();
    let pair = open_pty(config.rows, config.cols)?;
    let shell = resolve_shell(config.shell);

    let mut cmd = CommandBuilder::new(&shell);
    apply_login_shell_arg(&mut cmd, &shell);
    cmd.args(args);
    configure_shell_command(&mut cmd, &shell);
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(cwd) = config.cwd {
        cmd.cwd(cwd);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .context("failed to spawn shell in PTY")?;

    let master = Arc::new(Mutex::new(pair.master));
    let child = Arc::new(Mutex::new(child));

    #[cfg(unix)]
    {
        let pty = crate::unix::EventedPty::new(master.clone(), child.clone())
            .context("failed to create evented PTY")?;
        start_unix_session(pty, master, child, feed, sync_buffer_len, on_wakeup, false)
    }

    #[cfg(windows)]
    {
        let (event_tx, event_rx) = async_channel::bounded(512);
        // Flow control flag shared with the event loop (see `set_read_paused`).
        let read_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let poller =
            Arc::new(polling::Poller::new().context("failed to create Windows PTY poller")?);
        let (cmd_tx, cmd_rx) = poll_channel::channel(poller.clone());
        let pty = crate::windows::EventedPty::new(master.clone(), child.clone())
            .context("failed to create evented PTY")?;
        spawn_event_loop(
            pty,
            cmd_rx,
            poller,
            event_tx,
            feed,
            sync_buffer_len,
            on_wakeup,
            read_paused.clone(),
        );
        return Ok((
            PtySessionHandle {
                cmd_tx,
                read_paused,
            },
            event_rx,
        ));
    }
}

/// Executes a single command in a fresh PTY and waits for completion.
pub fn execute_command(command: &str, cwd: Option<PathBuf>) -> CommandResult {
    match execute_command_inner(command, cwd) {
        Ok(result) => result,
        Err(err) => CommandResult {
            output: vec![format!("pty error: {err:#}")],
            exit_code: 1,
        },
    }
}

fn execute_command_inner(command: &str, cwd: Option<PathBuf>) -> Result<CommandResult> {
    let pair = open_pty(24, 80)?;
    let shell = resolve_shell(None);

    let wrapped = format!(
        "{command}; __dragent_ec=$?; printf '\\n__DRAGENT_EXIT_%s\\n' \"$__dragent_ec\"; exit $__dragent_ec"
    );

    let mut cmd = CommandBuilder::new(&shell);
    apply_login_shell_arg(&mut cmd, &shell);
    cmd.arg("-c");
    cmd.arg(&wrapped);
    configure_shell_command(&mut cmd, &shell);
    if let Some(cwd) = cwd {
        cmd.cwd(cwd);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .context("failed to spawn command process")?;
    // The child has its own copy of the terminal's slave end. Ours must go,
    // or reading the master never ends: Linux reports end of output only
    // once every slave descriptor is closed (macOS ends it when the child
    // exits, which hid this).
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;

    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let exit_code = child.wait().context("failed to wait on child")?.exit_code() as i32;
    let text = String::from_utf8_lossy(&raw);
    let (output, parsed_exit) = parse_command_output(&text, exit_code);

    Ok(CommandResult {
        output,
        exit_code: parsed_exit,
    })
}

/// Start the event loop for a Unix PTY (spawned or adopted) and hand back
/// its command handle plus lifecycle events.
#[cfg(unix)]
fn start_unix_session(
    pty: crate::unix::EventedPty,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
    initially_paused: bool,
) -> Result<(PtySessionHandle, AsyncReceiver<PtyEvent>)> {
    {
        let (cmd_tx, cmd_rx) = mio_channel::channel();
        let (event_tx, event_rx) = async_channel::bounded(512);
        // Flow control flag shared with the event loop (see `set_read_paused`).
        let read_paused = Arc::new(std::sync::atomic::AtomicBool::new(initially_paused));
        // Foreground probe: a dup of the master fd + the shell's pgid, so the UI
        // can ask "is a command running?" via tcgetpgrp at any time.
        let foreground = {
            let raw = master.lock().ok().and_then(|m| m.as_raw_fd());
            let pid = child
                .lock()
                .ok()
                .and_then(|c| c.process_id())
                .map(|p| p as i32);
            match (raw, pid) {
                (Some(fd), Some(pid)) => {
                    // Close-on-exec: a plain dup leaked this session's master
                    // into every shell spawned later, keeping the PTY open
                    // (and its shell un-hung-up) after the session closed.
                    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
                    if dup >= 0 {
                        Some(Arc::new(ForegroundProbe {
                            master_fd: dup,
                            shell_pgid: pid,
                        }))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
        spawn_event_loop(
            pty,
            cmd_rx,
            event_tx,
            feed,
            sync_buffer_len,
            on_wakeup,
            read_paused.clone(),
        );
        Ok((
            PtySessionHandle {
                cmd_tx,
                foreground,
                read_paused,
            },
            event_rx,
        ))
    }
}

/// Resume an interactive session on a PTY inherited from a predecessor
/// process (daemon upgrade): `master_fd` is the PTY master, `shell_pid` the
/// shell still running on it. Unix only — a Windows ConPTY cannot be handed
/// between processes. Reading starts paused; the receiver must resume only
/// after its complete adoption transaction succeeds.
///
/// # Safety
/// `master_fd` must be an open PTY master this process now exclusively owns.
#[cfg(unix)]
pub unsafe fn adopt_interactive_paused(
    master_fd: std::os::fd::RawFd,
    shell_pid: u32,
    feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
) -> Result<(PtySessionHandle, AsyncReceiver<PtyEvent>)> {
    use crate::unix::adopt::{AdoptedChild, AdoptedMaster};
    // The inherited fd must not leak into shells spawned later.
    unsafe {
        libc::fcntl(master_fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let master: Box<dyn portable_pty::MasterPty + Send> =
        Box::new(unsafe { AdoptedMaster::from_raw_fd(master_fd) });
    let child: Box<dyn portable_pty::Child + Send + Sync> = Box::new(AdoptedChild::new(shell_pid));
    let master = Arc::new(Mutex::new(master));
    let child = Arc::new(Mutex::new(child));
    let pty = crate::unix::EventedPty::new(master.clone(), child.clone())
        .context("failed to create evented PTY")?
        .into_adopted();
    start_unix_session(pty, master, child, feed, sync_buffer_len, on_wakeup, true)
}

/// Adopt and immediately resume one inherited PTY.
///
/// # Safety
/// `master_fd` must be an open, exclusively owned PTY master.
#[cfg(unix)]
pub unsafe fn adopt_interactive(
    master_fd: std::os::fd::RawFd,
    shell_pid: u32,
    feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
) -> Result<(PtySessionHandle, AsyncReceiver<PtyEvent>)> {
    let (handle, events) = unsafe {
        adopt_interactive_paused(master_fd, shell_pid, feed, sync_buffer_len, on_wakeup)
    }?;
    handle.set_read_paused(false)?;
    Ok((handle, events))
}

fn open_pty(rows: u16, cols: u16) -> Result<PtyPair> {
    let pty_system = native_pty_system();
    pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to open PTY")
}

fn parse_command_output(text: &str, fallback_exit: i32) -> (Vec<String>, i32) {
    let marker_prefix = "__DRAGENT_EXIT_";
    let mut exit_code = fallback_exit;
    let mut lines: Vec<String> = Vec::new();

    for line in text.lines() {
        if let Some(code_str) = line.strip_prefix(marker_prefix) {
            if let Ok(code) = code_str.trim().parse::<i32>() {
                exit_code = code;
            }
            continue;
        }
        let clean = crate::ansi::strip_for_display(line);
        if !clean.is_empty() {
            lines.push(clean.into_owned());
        }
    }

    if lines.is_empty() && exit_code == 0 {
        lines.push("(no output)".to_string());
    }

    (lines, exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::PtyFeedStats;
    use parking_lot::FairMutex;
    use std::time::Duration;

    #[test]
    fn execute_echo_command() {
        let result = execute_command("echo hello", None);
        assert_eq!(result.exit_code, 0);
        assert!(result.output.iter().any(|l| l.contains("hello")));
    }

    #[test]
    fn execute_failing_command() {
        let result = execute_command("exit 42", None);
        assert_eq!(result.exit_code, 42);
    }

    #[test]
    fn spawn_interactive_session() {
        let captured = Arc::new(FairMutex::new(Vec::new()));
        let captured_feed = captured.clone();
        let feed: FeedCallback = Box::new(move |bytes| {
            captured_feed.lock().extend_from_slice(bytes);
            PtyFeedStats {
                bytes_processed: bytes.len(),
            }
        });
        let sync_len: SyncBufferLenCallback = Box::new(|| None);
        let on_wakeup: WakeupCallback = Box::new(|| {});

        let (handle, rx) =
            spawn_interactive(PtyConfig::default(), feed, sync_len, on_wakeup).expect("spawn");
        handle
            .write(b"echo dragent_pty_ok\n")
            .expect("write command");
        std::thread::sleep(Duration::from_millis(500));
        let saw_output = captured.lock().iter().copied().collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&saw_output);
        handle.shutdown().ok();
        assert!(
            text.contains("dragent_pty_ok"),
            "expected shell echo output"
        );
        while rx.try_recv().is_ok() {}
    }

    #[test]
    fn spawn_interactive_accepts_write_after_idle() {
        let captured = Arc::new(FairMutex::new(Vec::new()));
        let captured_feed = captured.clone();
        let feed: FeedCallback = Box::new(move |bytes| {
            captured_feed.lock().extend_from_slice(bytes);
            PtyFeedStats {
                bytes_processed: bytes.len(),
            }
        });
        let sync_len: SyncBufferLenCallback = Box::new(|| None);
        let on_wakeup: WakeupCallback = Box::new(|| {});

        let (handle, rx) =
            spawn_interactive(PtyConfig::default(), feed, sync_len, on_wakeup).expect("spawn");
        std::thread::sleep(Duration::from_millis(800));
        handle
            .write(b"echo dragent_idle_ok\n")
            .expect("write command");
        std::thread::sleep(Duration::from_millis(500));
        let saw = captured.lock();
        let text = String::from_utf8_lossy(&saw);
        handle.shutdown().ok();
        assert!(
            text.contains("dragent_idle_ok"),
            "expected output after idle wait, got: {text}"
        );
        while rx.try_recv().is_ok() {}
    }
}
