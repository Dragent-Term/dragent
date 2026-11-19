//! Running an interactive session on a PTY this process did not create.
//!
//! During a daemon upgrade the predecessor passes each session's PTY master
//! fd to its successor (fd inheritance), and the shell keeps running on the
//! slave side. The successor wraps the inherited fd and the shell's pid in
//! the same `portable_pty` traits a freshly spawned session uses, so the
//! whole event loop, flow control and close path work unchanged.
//!
//! The shell is not this process's child, so no SIGCHLD arrives and its exit
//! status cannot be collected. Its end is observed when reads from the
//! master fail (every slave fd closed); the exit code is then unknown.

use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, RawFd};

use anyhow::{bail, Result};
use portable_pty::{Child, ChildKiller, ExitStatus, MasterPty, PtySize};

/// The master side of an inherited PTY. Owns `fd` (closed on drop).
#[derive(Debug)]
pub struct AdoptedMaster {
    fd: RawFd,
}

impl AdoptedMaster {
    /// # Safety
    /// `fd` must be an open PTY master that nothing else in this process
    /// owns; it is closed when this value drops.
    pub unsafe fn from_raw_fd(fd: RawFd) -> Self {
        AdoptedMaster { fd }
    }

    fn dup_file(&self) -> Result<File> {
        let dup = unsafe { libc::fcntl(self.fd, libc::F_DUPFD_CLOEXEC, 0) };
        if dup < 0 {
            bail!("dup pty master: {}", io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(dup) })
    }
}

impl Drop for AdoptedMaster {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

impl MasterPty for AdoptedMaster {
    fn resize(&self, size: PtySize) -> Result<()> {
        let winsize = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        if unsafe { libc::ioctl(self.fd, libc::TIOCSWINSZ, &winsize as *const _) } != 0 {
            bail!("resize pty: {}", io::Error::last_os_error());
        }
        Ok(())
    }

    fn get_size(&self) -> Result<PtySize> {
        let mut winsize: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(self.fd, libc::TIOCGWINSZ, &mut winsize as *mut _) } != 0 {
            bail!("pty size: {}", io::Error::last_os_error());
        }
        Ok(PtySize {
            rows: winsize.ws_row,
            cols: winsize.ws_col,
            pixel_width: winsize.ws_xpixel,
            pixel_height: winsize.ws_ypixel,
        })
    }

    fn try_clone_reader(&self) -> Result<Box<dyn io::Read + Send>> {
        Ok(Box::new(self.dup_file()?))
    }

    fn take_writer(&self) -> Result<Box<dyn io::Write + Send>> {
        Ok(Box::new(self.dup_file()?))
    }

    fn process_group_leader(&self) -> Option<libc::pid_t> {
        match unsafe { libc::tcgetpgrp(self.fd) } {
            pid if pid > 0 => Some(pid),
            _ => None,
        }
    }

    fn as_raw_fd(&self) -> Option<RawFd> {
        Some(self.fd)
    }
}

/// A shell running on an adopted PTY. Not our child: status comes from
/// signal-0 probes, never `waitpid`.
#[derive(Debug, Clone)]
pub struct AdoptedChild {
    pid: u32,
}

impl AdoptedChild {
    pub fn new(pid: u32) -> Self {
        AdoptedChild { pid }
    }

    fn alive(&self) -> bool {
        unsafe { libc::kill(self.pid as libc::pid_t, 0) == 0 }
    }
}

impl ChildKiller for AdoptedChild {
    fn kill(&mut self) -> io::Result<()> {
        if unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

impl Child for AdoptedChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // The exit code of a process we did not spawn is unknowable.
        Ok((!self.alive()).then(|| ExitStatus::with_exit_code(0)))
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        while self.alive() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Ok(ExitStatus::with_exit_code(0))
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.pid)
    }
}
