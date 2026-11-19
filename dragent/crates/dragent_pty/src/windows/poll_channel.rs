//! Command channel integrated with [`polling::Poller`] (Windows).

use std::sync::{mpsc, Arc};

use polling::Poller;

pub use std::sync::mpsc::SendError;

pub struct Receiver<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

pub struct Sender<T> {
    poller: Arc<Poller>,
    tx: mpsc::Sender<T>,
}

impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        self.tx.send(t)?;
        self.poller.notify().ok();
        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            poller: self.poller.clone(),
            tx: self.tx.clone(),
        }
    }
}

pub fn channel<T>(poller: Arc<Poller>) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Sender { poller, tx }, Receiver { rx })
}
