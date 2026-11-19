// Adapted from alacritty_terminal/src/tty/windows/child.rs under Apache License 2.0.
// See crates/LICENSE-ALACRITTY.

use std::ffi::c_void;
use std::io;
use std::os::windows::process::ExitStatusExt;
use std::process::ExitStatus;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, Poller};
use windows_sys::Win32::Foundation::{BOOLEAN, FALSE, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, RegisterWaitForSingleObject, UnregisterWait, INFINITE,
    WT_EXECUTEINWAITTHREAD, WT_EXECUTEONLYONCE,
};

use crate::traits::ChildExit;

struct Interest {
    poller: Arc<Poller>,
    event: Event,
}

struct ChildExitSender {
    sender: mpsc::Sender<ChildExit>,
    interest: Arc<Mutex<Option<Interest>>>,
    child_handle: AtomicPtr<c_void>,
}

extern "system" fn child_exit_callback(ctx: *mut c_void, timed_out: BOOLEAN) {
    if timed_out != 0 {
        return;
    }
    let event_tx = unsafe { Box::from_raw(ctx as *mut ChildExitSender) };
    let mut exit_code = 0_u32;
    let child_handle = event_tx.child_handle.load(Ordering::Relaxed) as HANDLE;
    let status = unsafe { GetExitCodeProcess(child_handle, &mut exit_code) };
    let _exit_status = if status == FALSE {
        None
    } else {
        Some(ExitStatus::from_raw(exit_code))
    };
    event_tx
        .sender
        .send(ChildExit::Exited(exit_code as i32))
        .ok();
    let interest = event_tx.interest.lock().unwrap();
    if let Some(interest) = interest.as_ref() {
        interest
            .poller
            .post(CompletionPacket::new(interest.event))
            .ok();
    }
}

pub struct ChildExitWatcher {
    wait_handle: AtomicPtr<c_void>,
    event_rx: mpsc::Receiver<ChildExit>,
    interest: Arc<Mutex<Option<Interest>>>,
}

impl ChildExitWatcher {
    pub fn new(child_handle: HANDLE) -> io::Result<Self> {
        let (event_tx, event_rx) = mpsc::channel();
        let mut wait_handle: HANDLE = ptr::null_mut();
        let interest = Arc::new(Mutex::new(None));
        let sender_ref = Box::new(ChildExitSender {
            sender: event_tx,
            interest: interest.clone(),
            child_handle: AtomicPtr::from(child_handle),
        });

        let success = unsafe {
            RegisterWaitForSingleObject(
                &mut wait_handle,
                child_handle,
                Some(child_exit_callback),
                Box::into_raw(sender_ref).cast(),
                INFINITE,
                WT_EXECUTEINWAITTHREAD | WT_EXECUTEONLYONCE,
            )
        };

        if success == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self {
                event_rx,
                interest,
                wait_handle: AtomicPtr::from(wait_handle),
            })
        }
    }

    pub fn register(&self, poller: &Arc<Poller>, event: Event) {
        *self.interest.lock().unwrap() = Some(Interest {
            poller: poller.clone(),
            event,
        });
    }

    pub fn deregister(&self) {
        *self.interest.lock().unwrap() = None;
    }

    pub fn try_recv(&self) -> Option<ChildExit> {
        match self.event_rx.try_recv() {
            Ok(ev) => Some(ev),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(ChildExit::Exited(1)),
        }
    }
}

impl Drop for ChildExitWatcher {
    fn drop(&mut self) {
        unsafe {
            UnregisterWait(self.wait_handle.load(Ordering::Relaxed) as HANDLE);
        }
    }
}
