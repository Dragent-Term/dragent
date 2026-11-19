// Adapted from alacritty_terminal/src/event_loop.rs under the Apache License 2.0.
// copyright: check crates/LICENSE-ALACRITTY.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::thread;

use crate::messages::{PtyCommand, PtyEvent};
use crate::traits::ChildExit;
#[cfg(unix)]
use crate::traits::CHANNEL_TOKEN;
#[cfg(windows)]
use crate::traits::{CHILD_TOKEN, PTY_IO_TOKEN};

/// Size of the buffer used when reading from the PTY (256 KiB).
pub const READ_BUFFER_SIZE: usize = 0x10_0000;

/// Max bytes to feed the terminal per locked `feed` call before yielding.
///
/// This bounds how long the PTY reader thread holds the shared session lock in
/// one go. The UI thread needs that same lock for every render and input event,
/// so a large value makes the app unresponsive during high-throughput output
/// (e.g. `yes | head -1000000`): each keystroke waits for a full chunk to parse.
/// 8 KiB keeps each locked parse to ~1 ms so input/render interleave smoothly;
/// the extra per-chunk overhead is negligible.
pub const MAX_LOCKED_READ: usize = 0x2000;

/// Duty-cycle throttle for sustained high-throughput output.
///
/// The reader thread parses PTY output while holding the shared session lock the
/// UI thread also needs. A firehose producer (e.g. `yes | head -1000000`) keeps
/// the PTY readable continuously, so without a governor the reader holds the lock
/// ~100% of wall-clock time: the UI can't render or drain its work and the app
/// freezes, then gets killed as backlog builds. After a read pass that hit the
/// per-pass byte cap (the signal for sustained output) and spent at least
/// `THROTTLE_MIN_FEED` parsing, the reader sleeps for `feed_time * THROTTLE_RATIO`
/// so it yields a proportional slice of wall-time back to the UI. Interactive
/// output never reaches the cap / min-feed threshold, so echo latency is
/// unaffected. The proportional (not fixed) sleep keeps the duty cycle stable
/// across debug and release builds.
#[cfg(unix)]
const THROTTLE_RATIO: f32 = 0.5;
#[cfg(unix)]
const THROTTLE_MIN_FEED: std::time::Duration = std::time::Duration::from_millis(3);

/// Statistics returned after feeding PTY bytes into the terminal model.
#[derive(Debug, Clone, Copy, Default)]
pub struct PtyFeedStats {
    pub bytes_processed: usize,
}

pub type FeedCallback = Box<dyn FnMut(&[u8]) -> PtyFeedStats + Send>;
pub type SyncBufferLenCallback = Box<dyn Fn() -> Option<usize> + Send + Sync>;
pub type WakeupCallback = Box<dyn Fn() + Send + Sync>;

/// Spawn the cross-platform PTY event loop (mio on Unix, polling on Windows).
pub fn spawn_event_loop(
    #[cfg(unix)] pty: crate::unix::EventedPty,
    #[cfg(windows)] pty: crate::windows::EventedPty,
    #[cfg(unix)] cmd_rx: crate::mio_channel::Receiver<PtyCommand>,
    #[cfg(windows)] cmd_rx: crate::windows::poll_channel::Receiver<PtyCommand>,
    #[cfg(windows)] poller: std::sync::Arc<polling::Poller>,
    event_tx: async_channel::Sender<PtyEvent>,
    feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
    read_paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("dragent-pty-event-loop".into())
        .spawn(move || {
            #[cfg(unix)]
            unix_mio_event_loop(
                pty,
                cmd_rx,
                event_tx,
                feed,
                sync_buffer_len,
                on_wakeup,
                read_paused,
            );
            #[cfg(windows)]
            windows_poll_event_loop(
                pty,
                cmd_rx,
                poller,
                event_tx,
                feed,
                sync_buffer_len,
                on_wakeup,
                read_paused,
            );
        })
        .expect("failed to spawn PTY event loop")
}

#[cfg(unix)]
fn unix_mio_event_loop(
    mut pty: crate::unix::EventedPty,
    mut cmd_rx: crate::mio_channel::Receiver<PtyCommand>,
    event_tx: async_channel::Sender<PtyEvent>,
    mut feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
    read_paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use mio::{Events, Interest, Poll, Token};
    use std::sync::atomic::Ordering;

    let mut state = LoopState::default();
    let mut buf = [0u8; READ_BUFFER_SIZE];
    let mut can_read = false;
    let mut can_write = false;

    let mut poll = Poll::new().expect("create mio Poll");
    let mut interest = Interest::READABLE;
    let channel_token = Token(CHANNEL_TOKEN);

    poll.registry()
        .register(&mut cmd_rx, channel_token, Interest::READABLE)
        .expect("register command channel");
    pty.register(&poll, interest).expect("register PTY");

    let mut events = Events::with_capacity(1024);

    'event_loop: loop {
        events.clear();
        // A bounded read pass may leave an edge-triggered descriptor readable.
        // Poll without blocking in that case, so command/resize/exit events are
        // still serviced between output batches without losing the read edge.
        let timeout =
            (can_read && !read_paused.load(Ordering::Relaxed)).then_some(std::time::Duration::ZERO);
        if let Err(err) = poll.poll(&mut events, timeout) {
            match err.kind() {
                ErrorKind::Interrupted => continue,
                _ => {
                    let _ = event_tx.send_blocking(PtyEvent::Error(format!("poll error: {err}")));
                    break;
                }
            }
        }

        for event in events.iter() {
            match event.token() {
                token if token == channel_token => {
                    if !drain_commands(&mut cmd_rx, &mut state, &pty, &event_tx) {
                        break 'event_loop;
                    }
                }
                token if token == pty.child_token() => {
                    if let Some(ChildExit::Exited(code)) = pty.try_child_exit() {
                        let _ = event_tx.send_blocking(PtyEvent::ProcessExited(code));
                        break 'event_loop;
                    }
                }
                token if token == pty.read_token() || token == pty.write_token() => {
                    if event.is_read_closed() || event.is_write_closed() {
                        // Our own child reports its exit (with its code) via
                        // SIGCHLD. An adopted shell never will: every slave
                        // fd closing is the only sign it has ended.
                        if pty.is_adopted() {
                            let code = match pty.try_child_exit() {
                                Some(ChildExit::Exited(code)) => code,
                                None => -1,
                            };
                            let _ = event_tx.send_blocking(PtyEvent::ProcessExited(code));
                            break 'event_loop;
                        }
                        continue;
                    }
                    if event.is_readable() {
                        can_read = true;
                    }
                    if event.is_writable() {
                        can_write = true;
                    }
                }
                _ => (),
            }
        }

        // Edge-triggered: while paused `can_read` stays set, so resuming
        // drains what accumulated without waiting for a new readable edge.
        if (can_read && !read_paused.load(Ordering::Relaxed)) || (state.needs_write() && can_write)
        {
            if can_read && !read_paused.load(Ordering::Relaxed) {
                match pty_read_unix(
                    &mut pty,
                    &mut buf,
                    &mut can_read,
                    &mut feed,
                    &sync_buffer_len,
                    &on_wakeup,
                ) {
                    Ok(()) => {}
                    Err(err) => {
                        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                        if err.kind() == ErrorKind::Other {
                            continue;
                        }
                        let _ =
                            event_tx.send_blocking(PtyEvent::Error(format!("read error: {err}")));
                        break 'event_loop;
                    }
                }
            }

            if state.needs_write() && can_write {
                if let Err(err) = pty_write_unix(&mut pty, &mut state, &mut can_write) {
                    let _ = event_tx.send_blocking(PtyEvent::Error(format!("write error: {err}")));
                    break 'event_loop;
                }
            }
        }

        let needs_write = state.needs_write();
        let new_interest = if needs_write {
            Interest::READABLE | Interest::WRITABLE
        } else {
            Interest::READABLE
        };
        if new_interest != interest {
            interest = new_interest;
            let _ = pty.reregister(&poll, interest);
        }
    }

    let _ = poll.registry().deregister(&mut cmd_rx);
    let _ = pty.deregister(&poll);
}

#[cfg(windows)]
fn windows_poll_event_loop(
    mut pty: crate::windows::EventedPty,
    mut cmd_rx: crate::windows::poll_channel::Receiver<PtyCommand>,
    poller: std::sync::Arc<polling::Poller>,
    event_tx: async_channel::Sender<PtyEvent>,
    mut feed: FeedCallback,
    sync_buffer_len: SyncBufferLenCallback,
    on_wakeup: WakeupCallback,
    read_paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use polling::{Event, Events, PollMode};
    use std::sync::atomic::Ordering;

    let mut state = LoopState::default();
    let mut buf = [0u8; READ_BUFFER_SIZE];
    let mut can_read = false;
    let mut can_write = false;

    let interest = Event::new(PTY_IO_TOKEN, true, true);
    let mut pty_writable_interest = true;
    let mut pty_readable_interest = true;
    pty.register(&poller, interest, PollMode::Level)
        .expect("register PTY");

    let mut events = Events::new();

    'event_loop: loop {
        if !drain_commands(&mut cmd_rx, &mut state, &pty, &event_tx) {
            break;
        }

        // Re-arm writable interest based on the post-drain backlog BEFORE blocking
        // in wait(). drain_commands() (above) queues writes, but an idle shell
        // produces no readable event — so if a write is pending while writable
        // interest is off, wait() would block forever and the write would never
        // flush (only the first write, covered by the initial writable=true
        // registration, ever lands). This is the Windows analogue of the Unix
        // loop's per-iteration reregister. Without it, submitted commands stall
        // until some unrelated event (resize/repaint) happens to wake the loop.
        // Level-triggered: a paused loop must drop read interest or the
        // pending output would wake wait() continuously.
        let needs_write = state.needs_write();
        let wants_read = !read_paused.load(Ordering::Relaxed);
        if needs_write != pty_writable_interest || wants_read != pty_readable_interest {
            pty_writable_interest = needs_write;
            pty_readable_interest = wants_read;
            pty.register(
                &poller,
                Event::new(PTY_IO_TOKEN, wants_read, needs_write),
                PollMode::Level,
            )
            .ok();
        }

        events.clear();
        // Each pass reads one chunk, and a wake-up is posted only once a read
        // finds the pipe empty. With output still buffered, block-forever would
        // strand it: the pipe fills, the console host blocks writing, and the
        // shell freezes mid-startup until some unrelated write or resize wakes
        // the loop. Poll without waiting until the reader drains, as on Unix.
        let timeout =
            (can_read && !read_paused.load(Ordering::Relaxed)).then_some(std::time::Duration::ZERO);
        if let Err(err) = poller.wait(&mut events, timeout) {
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            let _ = event_tx.send_blocking(PtyEvent::Error(format!("poll error: {err}")));
            break;
        }

        for event in events.iter() {
            match event.key {
                PTY_IO_TOKEN => {
                    if event.readable {
                        can_read = true;
                    }
                    if event.writable {
                        can_write = true;
                    }
                }
                CHILD_TOKEN => {
                    if let Some(ChildExit::Exited(code)) = pty.try_child_exit() {
                        let _ = event_tx.send_blocking(PtyEvent::ProcessExited(code));
                        break 'event_loop;
                    }
                }
                _ => {}
            }
        }

        if (can_read && !read_paused.load(Ordering::Relaxed)) || (state.needs_write() && can_write)
        {
            if can_read && !read_paused.load(Ordering::Relaxed) {
                match pty_read_windows(
                    &mut pty,
                    &mut buf,
                    &mut can_read,
                    &mut feed,
                    &sync_buffer_len,
                    &on_wakeup,
                ) {
                    Ok(()) => {}
                    Err(err) => {
                        let _ =
                            event_tx.send_blocking(PtyEvent::Error(format!("read error: {err}")));
                        break 'event_loop;
                    }
                }
            }

            if state.needs_write() && can_write {
                if let Err(err) = pty_write_windows(&mut pty, &mut state, &mut can_write) {
                    let _ = event_tx.send_blocking(PtyEvent::Error(format!("write error: {err}")));
                    break 'event_loop;
                }
            }
        }

        let needs_write = state.needs_write();
        let wants_read = !read_paused.load(Ordering::Relaxed);
        if needs_write != pty_writable_interest || wants_read != pty_readable_interest {
            pty_writable_interest = needs_write;
            pty_readable_interest = wants_read;
            pty.register(
                &poller,
                Event::new(PTY_IO_TOKEN, wants_read, needs_write),
                PollMode::Level,
            )
            .ok();
        }
    }

    pty.deregister();
}

fn drain_commands(
    cmd_rx: &mut impl CommandReceiver,
    state: &mut LoopState,
    pty: &impl crate::traits::EventedPty,
    event_tx: &async_channel::Sender<PtyEvent>,
) -> bool {
    while let Ok(cmd) = cmd_rx.try_recv_command() {
        match cmd {
            PtyCommand::Write(data) => state.write_list.push_back(Cow::Owned(data)),
            PtyCommand::Resize { rows, cols } => {
                if let Err(err) = pty.resize(rows, cols) {
                    let _ = event_tx.send_blocking(PtyEvent::Error(format!("resize error: {err}")));
                }
            }
            PtyCommand::Interrupt => {
                pty.interrupt_child();
            }
            PtyCommand::Terminate => {
                pty.terminate_child();
            }
            // The flag itself is shared memory, read by the loop on every
            // pass; the command only wakes a loop sleeping in poll so a
            // resume drains pending output promptly.
            PtyCommand::PauseRead(_) => {}
            PtyCommand::Shutdown => {
                pty.hangup_child();
                return false;
            }
        }
    }
    true
}

trait CommandReceiver {
    fn try_recv_command(&mut self) -> Result<PtyCommand, std::sync::mpsc::TryRecvError>;
}

#[cfg(unix)]
impl CommandReceiver for crate::mio_channel::Receiver<PtyCommand> {
    fn try_recv_command(&mut self) -> Result<PtyCommand, std::sync::mpsc::TryRecvError> {
        self.try_recv()
    }
}

#[cfg(windows)]
impl CommandReceiver for crate::windows::poll_channel::Receiver<PtyCommand> {
    fn try_recv_command(&mut self) -> Result<PtyCommand, std::sync::mpsc::TryRecvError> {
        self.try_recv()
    }
}

#[cfg(unix)]
fn pty_read_unix(
    pty: &mut crate::unix::EventedPty,
    buf: &mut [u8],
    can_read: &mut bool,
    feed: &mut FeedCallback,
    sync_buffer_len: &SyncBufferLenCallback,
    on_wakeup: &WakeupCallback,
) -> std::io::Result<()> {
    let mut bytes_in_buffer = 0;
    let mut bytes_processed = 0;
    let mut feed_time = std::time::Duration::ZERO;

    loop {
        let read_limit = buf.len().min(MAX_LOCKED_READ);
        match pty.reader().read(&mut buf[bytes_in_buffer..read_limit]) {
            Ok(0) if bytes_in_buffer == 0 => {
                *can_read = false;
                break;
            }
            Ok(got) => bytes_in_buffer += got,
            Err(err) => match err.kind() {
                ErrorKind::Interrupted => {
                    if bytes_in_buffer == 0 {
                        break;
                    }
                }
                ErrorKind::WouldBlock => {
                    *can_read = false;
                    if bytes_in_buffer == 0 {
                        break;
                    }
                }
                _ => return Err(err),
            },
        }

        let mut offset = 0;
        while offset < bytes_in_buffer {
            let chunk_end = (offset + MAX_LOCKED_READ).min(bytes_in_buffer);
            let t0 = std::time::Instant::now();
            let stats = feed(&buf[offset..chunk_end]);
            feed_time += t0.elapsed();
            offset = chunk_end;
            maybe_wakeup(&stats, sync_buffer_len, on_wakeup);
        }

        bytes_processed += bytes_in_buffer;
        bytes_in_buffer = 0;

        if bytes_processed >= MAX_LOCKED_READ {
            break;
        }
    }

    // Duty-cycle governor (see THROTTLE_RATIO). Only sustained output reaches the
    // per-pass cap AND spends real time parsing; yield a proportional slice of
    // wall-time so the UI thread can take the shared lock and stay responsive.
    if bytes_processed >= MAX_LOCKED_READ && feed_time >= THROTTLE_MIN_FEED {
        thread::sleep(feed_time.mul_f32(THROTTLE_RATIO));
    }

    Ok(())
}

#[cfg(unix)]
fn pty_write_unix(
    pty: &mut crate::unix::EventedPty,
    state: &mut LoopState,
    can_write: &mut bool,
) -> std::io::Result<()> {
    pty_write_impl(pty.writer(), state, can_write)
}

#[cfg(windows)]
fn pty_read_windows(
    pty: &mut crate::windows::EventedPty,
    buf: &mut [u8],
    can_read: &mut bool,
    feed: &mut FeedCallback,
    sync_buffer_len: &SyncBufferLenCallback,
    on_wakeup: &WakeupCallback,
) -> std::io::Result<()> {
    let read_limit = buf.len().min(MAX_LOCKED_READ);
    let n = pty.reader().read(&mut buf[..read_limit])?;
    if n == 0 {
        *can_read = false;
        return Ok(());
    }

    let mut offset = 0;
    while offset < n {
        let chunk_end = (offset + MAX_LOCKED_READ).min(n);
        let stats = feed(&buf[offset..chunk_end]);
        offset = chunk_end;
        maybe_wakeup(&stats, sync_buffer_len, on_wakeup);
    }
    Ok(())
}

#[cfg(windows)]
fn pty_write_windows(
    pty: &mut crate::windows::EventedPty,
    state: &mut LoopState,
    can_write: &mut bool,
) -> std::io::Result<()> {
    pty_write_impl(pty.writer(), state, can_write)
}

fn pty_write_impl<W: Write>(
    writer: &mut W,
    state: &mut LoopState,
    can_write: &mut bool,
) -> std::io::Result<()> {
    state.ensure_next();

    'write_many: while let Some(mut current) = state.take_current() {
        'write_one: loop {
            match writer.write(current.remaining_bytes()) {
                Ok(0) => {
                    state.set_current(Some(current));
                    *can_write = false;
                    break 'write_many;
                }
                Ok(n) => {
                    current.advance(n);
                    if current.finished() {
                        state.goto_next();
                        break 'write_one;
                    }
                }
                Err(err) => {
                    state.set_current(Some(current));
                    match err.kind() {
                        ErrorKind::Interrupted | ErrorKind::WouldBlock => {
                            if err.kind() == ErrorKind::WouldBlock {
                                *can_write = false;
                            }
                            break 'write_many;
                        }
                        _ => return Err(err),
                    }
                }
            }
        }
    }

    Ok(())
}

fn maybe_wakeup(
    stats: &PtyFeedStats,
    sync_buffer_len: &SyncBufferLenCallback,
    on_wakeup: &WakeupCallback,
) {
    let buffered = sync_buffer_len().unwrap_or(0);
    if stats.bytes_processed > buffered {
        on_wakeup();
    }
}

#[derive(Default)]
struct LoopState {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
}

impl LoopState {
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }

    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(Writing::new);
    }

    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }

    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }

    fn set_current(&mut self, new: Option<Writing>) {
        self.writing = new;
    }
}

struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

impl Writing {
    fn new(c: Cow<'static, [u8]>) -> Self {
        Self {
            source: c,
            written: 0,
        }
    }

    fn advance(&mut self, n: usize) {
        self.written += n;
    }

    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }

    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}
