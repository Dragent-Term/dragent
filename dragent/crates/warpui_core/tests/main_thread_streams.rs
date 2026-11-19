//! A stream polled on the main thread keeps delivering while every
//! background thread is busy.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use warpui_core::elements::Empty;
use warpui_core::platform::WindowStyle;
use warpui_core::{App, AppContext, Element, Entity, TypedActionView, View};

#[derive(Default)]
struct Pane {
    items: Vec<usize>,
}

impl Entity for Pane {
    type Event = ();
}

impl View for Pane {
    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        Empty::new().finish()
    }

    fn ui_name() -> &'static str {
        "Pane"
    }
}

impl TypedActionView for Pane {
    type Action = ();
}

/// Blocking work held every background thread (slow git, in the app) and
/// background-polled streams stalled with it: no terminal output reached
/// its pane until a thread freed up.
#[test]
fn main_thread_streams_deliver_while_background_threads_are_blocked() {
    App::test((), |mut app| async move {
        let (_, pane) = app.add_window(WindowStyle::NotStealFocus, |_| Pane::default());

        // Occupy more blocking tasks than there are background threads. They
        // let go after a deadline so a regression fails instead of hanging.
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let threads = std::thread::available_parallelism().map_or(8, |n| n.get()) * 2;
        let background = app.background_executor();
        for _ in 0..threads {
            let release = release.clone();
            background
                .spawn(async move {
                    let (lock, ready) = &*release;
                    let guard = lock.lock().unwrap();
                    let _ = ready
                        .wait_timeout_while(guard, Duration::from_secs(5), |released| !*released)
                        .unwrap();
                })
                .detach();
        }
        std::thread::sleep(Duration::from_millis(100));

        let (tx, rx) = async_channel::unbounded();
        for item in [1, 2, 3] {
            tx.send_blocking(item).unwrap();
        }
        drop(tx);

        let (done_tx, done_rx) = futures::channel::oneshot::channel();
        let mut done_tx = Some(done_tx);
        let started = Instant::now();
        pane.update(&mut app, |_, ctx| {
            ctx.spawn_stream_on_main_thread(
                rx,
                |pane, item, _| pane.items.push(item),
                move |_, _| {
                    if let Some(done) = done_tx.take() {
                        let _ = done.send(());
                    }
                },
            );
        });
        done_rx.await.unwrap();
        let waited = started.elapsed();

        let (lock, ready) = &*release;
        *lock.lock().unwrap() = true;
        ready.notify_all();

        pane.read(&app, |pane, _| assert_eq!(pane.items, [1, 2, 3]));
        assert!(
            waited < Duration::from_secs(2),
            "items waited {waited:?} for a background thread"
        );
    });
}
