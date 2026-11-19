//! A burst of output larger than one read pass must be delivered in full
//! without any further input waking the event loop.
#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_pty::{spawn_interactive, PtyConfig, PtyFeedStats};

#[test]
fn output_burst_is_drained_without_further_input() {
    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = captured.clone();
    let feed: dragent_pty::FeedCallback = Box::new(move |bytes: &[u8]| {
        sink.lock().unwrap().extend_from_slice(bytes);
        PtyFeedStats { bytes_processed: bytes.len() }
    });
    let config = PtyConfig {
        shell: Some(r"C:\Windows\System32\cmd.exe".to_string()),
        ..PtyConfig::default()
    };
    let (handle, _events) =
        spawn_interactive(config, feed, Box::new(|| None), Box::new(|| {})).expect("spawn cmd");

    // ~100KB (far more than one read pass), then a value the typed line itself
    // does not contain. Nothing is written after this.
    handle
        .write(b"for /L %i in (1,1,1500) do @echo 0123456789012345678901234567890123456789012345678901234567890123 & set /a 6*7*1000+1\r")
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(90);
    let delivered = loop {
        if String::from_utf8_lossy(&captured.lock().unwrap()).contains("42001") {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let received = captured.lock().unwrap().len();
    handle.shutdown().ok();
    assert!(delivered, "output stalled after {received} bytes");
}
