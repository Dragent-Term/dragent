// Probe: does the app's Ctrl+C path actually interrupt a long-running command
// under Git Bash?
//
// Exercises the REAL stack the app uses — spawn_interactive + the event loop +
// UnblockedWriter + interrupt_child — not raw portable-pty. Starts `sleep 30`,
// then applies one interrupt mechanism and watches whether the shell comes back
// to a prompt. A prompt within a couple of seconds means the interrupt landed.
//
// PROBE_MODE selects the mechanism, so the two halves of the app's Ctrl+C can be
// judged separately:
//   write     — write(&[0x03]) only  (the tty interrupt char)
//   interrupt — interrupt() only     (the Windows taskkill path)
//   both      — what the app actually does today
//
// Run:
//   $env:PROBE_MODE="write"; cargo run -p dragent_pty --example ctrl_c_probe

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_pty::{
    spawn_interactive, FeedCallback, PtyConfig, PtyFeedStats, SyncBufferLenCallback, WakeupCallback,
};

fn main() {
    let shell = std::env::var("PROBE_SHELL")
        .unwrap_or_else(|_| r"C:\Program Files\Git\bin\bash.exe".to_string());
    let mode = std::env::var("PROBE_MODE").unwrap_or_else(|_| "both".to_string());
    println!("shell: {shell}\nmode:  {mode}\n");

    let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let feed: FeedCallback = Box::new(move |bytes| {
        sink.lock().unwrap().extend_from_slice(bytes);
        PtyFeedStats {
            bytes_processed: bytes.len(),
        }
    });
    let sync_len: SyncBufferLenCallback = Box::new(|| None);
    let on_wakeup: WakeupCallback = Box::new(|| {});

    let config = PtyConfig {
        shell: Some(shell),
        ..PtyConfig::default()
    };
    let (handle, _rx) = spawn_interactive(config, feed, sync_len, on_wakeup).expect("spawn shell");

    let take = |captured: &Arc<Mutex<Vec<u8>>>| -> String {
        let mut guard = captured.lock().unwrap();
        let text = String::from_utf8_lossy(&guard).to_string();
        guard.clear();
        text
    };

    // Let the shell start up and print its first prompt.
    std::thread::sleep(Duration::from_secs(3));
    let _ = take(&captured);

    println!("--- starting `sleep 30` ---");
    handle.write(b"sleep 30\n").unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let _ = take(&captured);

    println!("--- applying interrupt (mode={mode}) ---");
    let started = Instant::now();
    match mode.as_str() {
        "write" => handle.write(&[0x03]).unwrap(),
        "interrupt" => handle.interrupt().unwrap(),
        _ => {
            handle.write(&[0x03]).unwrap();
            handle.interrupt().unwrap();
        }
    }

    // `sleep 30` still has ~25s to run. If the interrupt landed, bash prints a
    // prompt almost immediately; if not, we sit here in silence.
    std::thread::sleep(Duration::from_secs(6));
    let after = take(&captured);
    println!(
        "output in the 6s after interrupt ({}ms):\n{after:?}\n",
        started.elapsed().as_millis()
    );

    if after.trim().is_empty() {
        println!(">>> RESULT: SILENCE — `sleep 30` survived. Interrupt NOT delivered.");
    } else {
        println!(">>> RESULT: shell responded — interrupt DELIVERED.");
    }

    handle.shutdown().ok();
}
