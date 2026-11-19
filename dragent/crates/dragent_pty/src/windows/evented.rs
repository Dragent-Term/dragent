use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use polling::{Event, PollMode, Poller};
use portable_pty::{Child, MasterPty, PtySize};

use super::blocking::{UnblockedReader, UnblockedWriter};
use super::child_exit::ChildExitWatcher;
use crate::traits::{ChildExit, CHILD_TOKEN, PTY_IO_TOKEN};

pub struct EventedPty {
    reader: UnblockedReader<Box<dyn Read + Send>>,
    writer: UnblockedWriter<Box<dyn Write + Send>>,
    child_watcher: ChildExitWatcher,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
}

impl EventedPty {
    pub fn new(
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    ) -> std::io::Result<Self> {
        let reader = {
            let master_guard = master.lock().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "master mutex poisoned")
            })?;
            master_guard
                .try_clone_reader()
                .map_err(std::io::Error::other)?
        };
        let writer = {
            let master_guard = master.lock().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "master mutex poisoned")
            })?;
            master_guard.take_writer().map_err(std::io::Error::other)?
        };

        let child_handle = {
            let child_guard = child.lock().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "child mutex poisoned")
            })?;
            child_guard.as_raw_handle().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::Other, "child has no raw handle")
            })? as windows_sys::Win32::Foundation::HANDLE
        };

        let child_watcher = ChildExitWatcher::new(child_handle)?;
        let reader = UnblockedReader::new(reader, 64 * 1024);
        let writer = UnblockedWriter::new(writer, 64 * 1024);

        Ok(Self {
            reader,
            writer,
            child_watcher,
            master,
            child,
        })
    }

    pub fn register(
        &mut self,
        poller: &Arc<Poller>,
        interest: Event,
        mode: PollMode,
    ) -> std::io::Result<()> {
        // polling 3.x `Event` has a private field, so it must be built via its
        // constructor rather than a struct literal.
        let pty_event = Event::new(PTY_IO_TOKEN, interest.readable, interest.writable);
        self.reader.register(poller, pty_event, mode);
        self.writer.register(poller, pty_event, mode);
        self.child_watcher
            .register(poller, Event::readable(CHILD_TOKEN));
        Ok(())
    }

    pub fn deregister(&self) {
        self.reader.deregister();
        self.writer.deregister();
        self.child_watcher.deregister();
    }

    pub fn reader(&mut self) -> &mut UnblockedReader<Box<dyn Read + Send>> {
        &mut self.reader
    }

    pub fn writer(&mut self) -> &mut UnblockedWriter<Box<dyn Write + Send>> {
        &mut self.writer
    }

    pub fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let master = self
            .master
            .lock()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "master mutex poisoned"))?;
        master.resize(size).map_err(std::io::Error::other)
    }

    pub fn kill_child(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    /// Explicit close: end the shell and everything it started (see
    /// [`super::process_tree`]).
    pub fn hangup_child(&self) {
        if let Ok(mut child) = self.child.lock() {
            // Captured while the shell is alive: its children are found
            // through it.
            let tree = child
                .process_id()
                .map(super::process_tree::capture)
                .unwrap_or_default();
            let _ = child.kill();
            super::process_tree::terminate(&tree);
        }
    }

    /// No-op: on Windows the interrupt travels as the 0x03 byte written to the
    /// ConPTY input, not as an out-of-band signal.
    ///
    /// ConPTY already gives us process-group semantics for free — conhost turns
    /// a 0x03 in the input stream into a CTRL_C_EVENT delivered to *every*
    /// process attached to the pseudoconsole, which is exactly the foreground
    /// job and its descendants. That is the Unix `kill(-pgid, SIGINT)` behaviour,
    /// so nothing extra is needed here.
    ///
    /// This used to shell out to `taskkill /T /PID <shell>`. That was dead code:
    /// without `/F`, taskkill only posts WM_CLOSE to top-level windows, which a
    /// console process like bash does not have — verified against Git Bash, where
    /// it left `sleep 30` running untouched. Adding `/F` would be worse than
    /// useless: it force-kills the whole tree *including the shell itself*, so
    /// Ctrl+C would destroy the session instead of interrupting one command.
    pub fn interrupt_child(&self) {}

    pub fn try_child_exit(&mut self) -> Option<ChildExit> {
        match self.child_watcher.try_recv() {
            Some(ChildExit::Exited(code)) => Some(ChildExit::Exited(code)),
            None => None,
        }
    }
}

impl crate::traits::EventedPty for EventedPty {
    fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        EventedPty::resize(self, rows, cols)
    }

    fn interrupt_child(&self) {
        EventedPty::interrupt_child(self);
    }

    fn hangup_child(&self) {
        EventedPty::hangup_child(self);
    }

    fn kill_child(&self) {
        EventedPty::kill_child(self);
    }

    fn try_child_exit(&mut self) -> Option<ChildExit> {
        EventedPty::try_child_exit(self)
    }
}
