//! Child-process spawn helpers.

/// `CREATE_NO_WINDOW`: the child gets no console window (and no console at all
/// unless it allocates one).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// dragent.exe is a GUI-subsystem binary (see the crate attribute in
/// `main.rs`), so it owns no console for children to inherit. Any *console*
/// child — git, rg, node, python, cmd.exe, … — therefore creates its own
/// visible console window for its lifetime, which flashes on screen even when
/// all stdio is piped. Every `Command` the app runs in the background must go
/// through this. No-op on non-Windows platforms.
pub trait NoWindow {
    /// Suppress the child's console window on Windows. No-op elsewhere.
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

/// `gh`, found the way the machine page finds it. A bare `Command::new("gh")`
/// fails in an app opened from the Finder, whose PATH holds only system folders.
pub(crate) fn gh_command() -> std::process::Command {
    #[cfg(not(target_family = "wasm"))]
    {
        dragent_host::host_tools::command("gh")
    }
    #[cfg(target_family = "wasm")]
    {
        std::process::Command::new("gh")
    }
}

/// Capture a small executable-discovery probe without waiting indefinitely
/// on shell startup, package-manager hooks, or inherited output pipes.
pub(crate) fn probe_output(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::{Output, Stdio};
    use std::time::Instant;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .no_window()
        .spawn()?;
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped probe stdout");
    let (send, receive) = std::sync::mpsc::channel();
    crate::bg::detach(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .by_ref()
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = send.send(result);
    });
    let start = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break receive
                    .recv_timeout(timeout.saturating_sub(start.elapsed()))
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "Executable probe output timed out",
                        )
                    })
                    .and_then(|output| output)
                    .map(|stdout| Output {
                        status,
                        stdout,
                        stderr: Vec::new(),
                    });
            }
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Ok(None) => {
                break Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Executable probe timed out",
                ))
            }
            Err(error) => break Err(error),
        }
    };
    // Probes own a separate group so shell/npm descendants cannot outlive a
    // timeout and keep their capture pipes open.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

// `tokio` is native-only (no async process runtime on wasm).
#[cfg(not(target_family = "wasm"))]
impl NoWindow for tokio::process::Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }

    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}
