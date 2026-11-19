//! Stream throttle for coalescing high-frequency UI wakeups (e.g. PTY output bursts).

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_lite::{ready, Stream};
use warpui::r#async::Timer;

pub struct Throttle<S> {
    period: Duration,
    stream: S,
    timer: Option<Timer>,
}

pub fn throttle<S: Stream>(period: Duration, stream: S) -> Throttle<S> {
    Throttle {
        period,
        stream,
        timer: None,
    }
}

impl<S: Stream> Stream for Throttle<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let period = self.period;
        if let Some(timer) = self.as_mut().timer_pin() {
            ready!(timer.poll_next(ctx));

            let mut stream = self.as_mut().stream_pin();
            let mut last_poll = Poll::Pending;
            while let Poll::Ready(item) = stream.as_mut().poll_next(ctx) {
                if item.is_none() {
                    self.set_timer(None);
                    return Poll::Ready(None);
                }
                last_poll = Poll::Ready(item);
            }

            if last_poll.is_ready() {
                self.set_timer(Some(Timer::after(period)));
            } else {
                self.set_timer(None);
            }
            last_poll
        } else {
            let item = ready!(self.as_mut().stream_pin().poll_next(ctx));
            self.set_timer(Some(Timer::after(period)));
            Poll::Ready(item)
        }
    }
}

impl<S> Throttle<S> {
    fn timer_pin(self: Pin<&mut Self>) -> Option<Pin<&mut Timer>> {
        if self.timer.is_some() {
            Some(unsafe { self.map_unchecked_mut(|s| s.timer.as_mut().unwrap()) })
        } else {
            None
        }
    }

    fn stream_pin(self: Pin<&mut Self>) -> Pin<&mut S> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stream) }
    }

    fn set_timer(self: Pin<&mut Self>, timer: Option<Timer>) {
        unsafe {
            self.get_unchecked_mut().timer = timer;
        }
    }
}
