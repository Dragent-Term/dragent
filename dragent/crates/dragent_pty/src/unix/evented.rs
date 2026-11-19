use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};

use libc::{fcntl, F_GETFL, F_SETFL, O_NONBLOCK};
use mio::unix::SourceFd;
use mio::{Interest, Poll, Token};
use portable_pty::{Child, MasterPty, PtySize};
use signal_hook::consts::SIGCHLD;
use signal_hook_mio::v1_0::SignalsInfo;

use crate::traits::{ChildExit, CHILD_TOKEN, PTY_IO_TOKEN};

pub const PTY_TOKEN: Token = Token(PTY_IO_TOKEN);
pub const CHILD_SIGNAL_TOKEN: Token = Token(CHILD_TOKEN);

pub struct EventedPty {
    fd: File,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    signals: SignalsInfo,
    /// The shell is not our child (inherited during a daemon upgrade): no
    /// SIGCHLD will come, so the PTY closing is its exit signal.
    adopted: bool,
}

impl EventedPty {
    pub fn new(
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    ) -> io::Result<Self> {
        let raw_fd = {
            let master = master
                .lock()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "master mutex poisoned"))?;
            master
                .as_raw_fd()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "PTY has no raw fd"))?
        };

        let dup_fd = dup_fd(raw_fd)?;
        set_nonblocking(dup_fd)?;
        let fd = unsafe { File::from_raw_fd(dup_fd) };
        let signals = SignalsInfo::new([SIGCHLD]).map_err(io::Error::other)?;

        Ok(Self {
            fd,
            master,
            child,
            signals,
            adopted: false,
        })
    }

    /// Mark this PTY's shell as adopted (see the `adopted` field).
    pub fn into_adopted(mut self) -> Self {
        self.adopted = true;
        self
    }

    pub fn is_adopted(&self) -> bool {
        self.adopted
    }

    pub fn register(&mut self, poll: &Poll, interest: Interest) -> io::Result<()> {
        poll.registry()
            .register(&mut self.signals, CHILD_SIGNAL_TOKEN, Interest::READABLE)?;
        poll.registry()
            .register(&mut SourceFd(&self.fd.as_raw_fd()), PTY_TOKEN, interest)
    }

    pub fn reregister(&mut self, poll: &Poll, interest: Interest) -> io::Result<()> {
        poll.registry()
            .reregister(&mut self.signals, CHILD_SIGNAL_TOKEN, Interest::READABLE)?;
        poll.registry()
            .reregister(&mut SourceFd(&self.fd.as_raw_fd()), PTY_TOKEN, interest)
    }

    pub fn deregister(&mut self, poll: &Poll) -> io::Result<()> {
        poll.registry().deregister(&mut self.signals)?;
        poll.registry()
            .deregister(&mut SourceFd(&self.fd.as_raw_fd()))
    }

    pub fn read_token(&self) -> Token {
        PTY_TOKEN
    }

    pub fn write_token(&self) -> Token {
        PTY_TOKEN
    }

    pub fn child_token(&self) -> Token {
        CHILD_SIGNAL_TOKEN
    }

    pub fn reader(&mut self) -> &mut File {
        &mut self.fd
    }

    pub fn writer(&mut self) -> &mut File {
        &mut self.fd
    }

    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let master = self
            .master
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "master mutex poisoned"))?;
        master.resize(size).map_err(io::Error::other)
    }

    pub fn kill_child(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    /// Explicit close: hang up the shell's whole process tree now and
    /// SIGKILL whatever ignores it (see [`super::process_tree`]).
    pub fn hangup_child(&self) {
        if let Ok(child) = self.child.lock() {
            if let Some(pid) = child.process_id() {
                super::process_tree::hangup_tree(pid as i32);
            }
        }
    }

    /// SIGINT the TTY's *foreground* process group — the running command.
    ///
    /// NOT the shell's group: an interactive shell runs job control, so every
    /// job it launches gets its OWN process group and becomes the terminal's
    /// foreground group. The old `kill(-shell_pid)` signalled the shell's
    /// group instead — which contains only the (SIGINT-immune) interactive
    /// shell — so the running command never received anything. It went
    /// unnoticed because the 0x03 byte written alongside does the job via the
    /// kernel (ISIG) for cooked-mode programs; raw-mode programs (gh, npm
    /// prompts, anything using survey/inquirer) turn ISIG off, which is
    /// exactly when this signal is the only interrupt left.
    ///
    /// `tcgetpgrp` on the pty fd answers "who owns the terminal right now":
    /// the foreground job while one runs, or the shell at an idle prompt
    /// (where SIGINT is harmless — it just redraws the prompt).
    pub fn interrupt_child(&self) {
        let fg = unsafe { libc::tcgetpgrp(self.fd.as_raw_fd()) };
        if fg > 0 {
            unsafe {
                libc::kill(-fg, libc::SIGINT);
            }
            return;
        }
        // Fallback (pty fd can't answer): the shell's own group.
        if let Ok(child) = self.child.lock() {
            if let Some(pid) = child.process_id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGINT);
                }
            }
        }
    }

    /// Firmly stop the foreground job and its whole process group. Sends SIGINT
    /// (cooperative — lets a cooked program clean up) immediately followed by
    /// SIGTERM (for dev servers like `yarn dev`/`next dev`/node that trap or
    /// ignore SIGINT). Signalling the GROUP (`-pgrp`) reaches the wrapper and
    /// every child it spawned. The interactive shell is in its own group, so it
    /// — and its cd/env — is untouched.
    pub fn terminate_child(&self) {
        let target = {
            let fg = unsafe { libc::tcgetpgrp(self.fd.as_raw_fd()) };
            if fg > 0 {
                Some(fg)
            } else {
                // Fallback (pty fd can't answer): the shell's own group.
                self.child
                    .lock()
                    .ok()
                    .and_then(|c| c.process_id())
                    .map(|pid| pid as i32)
            }
        };
        if let Some(pgrp) = target {
            unsafe {
                libc::kill(-pgrp, libc::SIGINT);
                libc::kill(-pgrp, libc::SIGTERM);
            }
        }
    }

    pub fn drain_child_signals(&mut self) {
        while self.signals.pending().next().is_some() {}
    }

    pub fn try_child_exit(&mut self) -> Option<ChildExit> {
        self.drain_child_signals();
        let Ok(mut child) = self.child.lock() else {
            return None;
        };
        match child.try_wait() {
            Ok(Some(status)) => Some(ChildExit::Exited(status.exit_code() as i32)),
            Ok(None) => None,
            Err(_) => Some(ChildExit::Exited(1)),
        }
    }
}

impl crate::traits::EventedPty for EventedPty {
    fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        EventedPty::resize(self, rows, cols)
    }

    fn interrupt_child(&self) {
        EventedPty::interrupt_child(self);
    }

    fn terminate_child(&self) {
        EventedPty::terminate_child(self);
    }

    fn kill_child(&self) {
        EventedPty::kill_child(self);
    }

    fn hangup_child(&self) {
        EventedPty::hangup_child(self);
    }

    fn try_child_exit(&mut self) -> Option<ChildExit> {
        EventedPty::try_child_exit(self)
    }
}

/// Close-on-exec: a daemon upgrade starts its successor with `exec`, and a
/// plain `dup` went with it. The successor kept that copy forever and made
/// its own, so a terminal's handle count grew with every upgrade until the
/// daemon hit the open-file limit and could no longer accept a connection.
fn dup_fd(fd: RawFd) -> io::Result<RawFd> {
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(dup)
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { fcntl(fd, F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let res = unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
