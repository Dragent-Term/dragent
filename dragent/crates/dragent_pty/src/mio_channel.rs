//! mpsc channel that wakes a [`mio::Poll`] when messages are sent.
//!
//! Standard pattern for integrating blocking command queues with an I/O event loop:
//! the receiver implements [`mio::event::Source`] via [`mio::Waker`].

use std::io;
pub use std::sync::mpsc::SendError;
use std::sync::{mpsc, Arc, Mutex};

use mio::{event, Token, Waker};

/// Create a [`Sender`] and [`Receiver`] pair for use with a mio event loop.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();

    let state = Arc::new(Mutex::new(State {
        waker: None,
        needs_wake_on_register: false,
    }));

    (
        Sender {
            state: state.clone(),
            tx,
        },
        Receiver { state, rx },
    )
}

/// `mpsc::Receiver` wrapper registered with [`mio::Poll`] via a [`Waker`].
pub struct Receiver<T> {
    state: Arc<Mutex<State>>,
    rx: mpsc::Receiver<T>,
}

impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

impl<T> event::Source for Receiver<T> {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: Token,
        _: mio::Interest,
    ) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();

        if state.waker.is_none() {
            let waker = Waker::new(registry, token)?;
            if state.needs_wake_on_register {
                waker.wake()?;
                state.needs_wake_on_register = false;
            }
            state.waker = Some(waker);
        }

        Ok(())
    }

    fn reregister(
        &mut self,
        _registry: &mio::Registry,
        _token: Token,
        _: mio::Interest,
    ) -> io::Result<()> {
        Ok(())
    }

    fn deregister(&mut self, _: &mio::Registry) -> io::Result<()> {
        Ok(())
    }
}

/// `mpsc::Sender` wrapper that wakes the event loop after each successful send.
pub struct Sender<T> {
    state: Arc<Mutex<State>>,
    tx: mpsc::Sender<T>,
}

impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        self.tx.send(t)?;

        let mut state = self.state.lock().unwrap();
        if let Some(waker) = &mut state.waker {
            let _ = waker.wake();
        } else {
            state.needs_wake_on_register = true;
        }

        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            tx: self.tx.clone(),
        }
    }
}

struct State {
    waker: Option<Waker>,
    needs_wake_on_register: bool,
}
