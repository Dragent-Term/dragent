// Adapted from alacritty_terminal/src/tty/windows/blocking.rs under Apache License 2.0.
// See crates/LICENSE-ALACRITTY.

use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use piper::{pipe, Reader, Writer};
use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, PollMode, Poller};

struct Registration {
    interest: Mutex<Option<Interest>>,
    end: PipeEnd,
}

#[derive(Copy, Clone)]
enum PipeEnd {
    Reader,
    Writer,
}

struct Interest {
    event: Event,
    poller: Arc<Poller>,
    mode: PollMode,
}

pub struct UnblockedReader<R> {
    interest: Arc<Registration>,
    pipe: Reader,
    first_register: bool,
    _reader: PhantomData<R>,
}

impl<R: Read + Send + 'static> UnblockedReader<R> {
    pub fn new(mut source: R, pipe_capacity: usize) -> Self {
        let (reader, mut writer) = pipe(pipe_capacity);
        let interest = Arc::new(Registration {
            interest: Mutex::new(None),
            end: PipeEnd::Reader,
        });

        thread::Builder::new()
            .name("dragent-pty-unblocked-reader".into())
            .spawn(move || {
                let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
                let mut context = Context::from_waker(&waker);
                loop {
                    match writer.poll_fill(&mut context, &mut source) {
                        Poll::Ready(Ok(0)) => return,
                        Poll::Ready(Ok(_)) => continue,
                        Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Poll::Ready(Err(e)) => {
                            log::error!("unblocked reader error: {e}");
                            return;
                        }
                        Poll::Pending => thread::park(),
                    }
                }
            })
            .expect("spawn unblocked reader");

        Self {
            interest,
            pipe: reader,
            first_register: true,
            _reader: PhantomData,
        }
    }

    pub fn register(&mut self, poller: &Arc<Poller>, event: Event, mode: PollMode) {
        let mut interest = self.interest.interest.lock().unwrap();
        *interest = Some(Interest {
            event,
            poller: poller.clone(),
            mode,
        });
        if (!self.pipe.is_empty() && event.readable) || self.first_register {
            self.first_register = false;
            poller.post(CompletionPacket::new(event)).ok();
        }
    }

    pub fn deregister(&self) {
        *self.interest.interest.lock().unwrap() = None;
    }

    pub fn try_read(&mut self, buf: &mut [u8]) -> usize {
        let waker = Waker::from(self.interest.clone());
        match self
            .pipe
            .poll_drain_bytes(&mut Context::from_waker(&waker), buf)
        {
            Poll::Pending => 0,
            Poll::Ready(n) => n,
        }
    }
}

impl<R: Read + Send + 'static> Read for UnblockedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(self.try_read(buf))
    }
}

pub struct UnblockedWriter<W> {
    interest: Arc<Registration>,
    pipe: Writer,
    _writer: PhantomData<W>,
}

impl<W: Write + Send + 'static> UnblockedWriter<W> {
    pub fn new(mut sink: W, pipe_capacity: usize) -> Self {
        let (mut reader, writer) = pipe(pipe_capacity);
        let interest = Arc::new(Registration {
            interest: Mutex::new(None),
            end: PipeEnd::Writer,
        });

        thread::Builder::new()
            .name("dragent-pty-unblocked-writer".into())
            .spawn(move || {
                let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
                let mut context = Context::from_waker(&waker);
                loop {
                    match reader.poll_drain(&mut context, &mut sink) {
                        Poll::Ready(Ok(0)) => return,
                        Poll::Ready(Ok(_)) => continue,
                        Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Poll::Ready(Err(e)) => {
                            log::error!("unblocked writer error: {e}");
                            return;
                        }
                        Poll::Pending => thread::park(),
                    }
                }
            })
            .expect("spawn unblocked writer");

        Self {
            interest,
            pipe: writer,
            _writer: PhantomData,
        }
    }

    pub fn register(&self, poller: &Arc<Poller>, event: Event, mode: PollMode) {
        let mut interest = self.interest.interest.lock().unwrap();
        *interest = Some(Interest {
            event,
            poller: poller.clone(),
            mode,
        });
        if !self.pipe.is_full() && event.writable {
            poller.post(CompletionPacket::new(event)).ok();
        }
    }

    pub fn deregister(&self) {
        *self.interest.interest.lock().unwrap() = None;
    }

    pub fn try_write(&mut self, buf: &[u8]) -> usize {
        let waker = Waker::from(self.interest.clone());
        match self
            .pipe
            .poll_fill_bytes(&mut Context::from_waker(&waker), buf)
        {
            Poll::Pending => 0,
            Poll::Ready(n) => n,
        }
    }
}

impl<W: Write + Send + 'static> Write for UnblockedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(self.try_write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

impl Wake for Registration {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let mut interest_lock = self.interest.lock().unwrap();
        if let Some(interest) = interest_lock.as_ref() {
            let send_event = match self.end {
                PipeEnd::Reader => interest.event.readable,
                PipeEnd::Writer => interest.event.writable,
            };
            if send_event {
                interest
                    .poller
                    .post(CompletionPacket::new(interest.event))
                    .ok();
                if matches!(interest.mode, PollMode::Oneshot | PollMode::EdgeOneshot) {
                    *interest_lock = None;
                }
            }
        }
    }
}
