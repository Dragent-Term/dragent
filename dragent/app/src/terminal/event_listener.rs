use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_channel::{Receiver, Sender};

/// Dedicated wakeup channel between the PTY event loop and the UI thread.
#[derive(Clone)]
pub struct TerminalEventListener {
    wakeup_tx: Sender<()>,
    wakeup_rx: Receiver<()>,
    pending: Arc<AtomicBool>,
}

impl TerminalEventListener {
    pub fn new() -> Self {
        let (wakeup_tx, wakeup_rx) = async_channel::unbounded();
        Self {
            wakeup_tx,
            wakeup_rx,
            pending: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Queue a single coalesced wakeup (additional calls while pending are dropped).
    pub fn send_wakeup(&self) {
        if self
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            let _ = self.wakeup_tx.try_send(());
        }
    }

    pub fn wakeup_receiver(&self) -> Receiver<()> {
        self.wakeup_rx.clone()
    }

    pub fn clear_pending(&self) {
        self.pending.store(false, Ordering::Release);
    }
}
