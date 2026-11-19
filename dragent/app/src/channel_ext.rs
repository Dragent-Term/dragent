//! `send_blocking` compatibility for wasm.
//!
//! `async_channel::Sender::send_blocking` parks the current thread until the
//! channel has capacity — impossible on wasm's single cooperative thread, so
//! async-channel doesn't compile it there. Our UI channels are unbounded, where
//! a send only ever waits on a *full* channel (never), so a non-blocking
//! `try_send` is behaviorally identical.
//!
//! This trait provides an inherent-signature-compatible `send_blocking` **on
//! wasm only**. On native the real inherent method is used (this trait isn't
//! even compiled), so call sites read the same on both targets. Import it under
//! `#[cfg(target_family = "wasm")]` in files that call `send_blocking`.

#[cfg(target_family = "wasm")]
pub trait SendBlockingCompat<T> {
    fn send_blocking(&self, msg: T) -> Result<(), async_channel::SendError<T>>;
}

#[cfg(target_family = "wasm")]
impl<T> SendBlockingCompat<T> for async_channel::Sender<T> {
    fn send_blocking(&self, msg: T) -> Result<(), async_channel::SendError<T>> {
        // Unbounded channel: the only failure is a closed receiver, which
        // `SendError` also represents. Bounded-full would drop here, but the UI
        // channels this covers are unbounded.
        self.try_send(msg)
            .map_err(|e| async_channel::SendError(e.into_inner()))
    }
}
