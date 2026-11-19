// Proves the fresh_windows_path() fix: this process's OWN inherited PATH does
// NOT contain node (same as dragent.exe launched before Node was installed).
// Spawns a real interactive shell through dragent_pty's actual production
// path (spawn_interactive -> configure_shell_command -> apply_shell_environment)
// and checks whether `node -v` resolves INSIDE that child shell.
//
// Run: cargo run -p dragent_pty --example node_path_probe

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dragent_pty::{
    spawn_interactive, FeedCallback, PtyConfig, PtyFeedStats, SyncBufferLenCallback, WakeupCallback,
};

fn main() {
    println!(
        "this process's own inherited PATH has node? {}",
        std::env::var("PATH")
            .unwrap_or_default()
            .to_lowercase()
            .contains("nodejs")
    );

    let shell = std::env::var("PROBE_SHELL")
        .unwrap_or_else(|_| r"C:\Program Files\Git\bin\bash.exe".to_string());
    println!("shell: {shell}\n");

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

    std::thread::sleep(Duration::from_secs(2));
    captured.lock().unwrap().clear();

    handle.write(b"echo PATH_SEEN_BY_CHILD=[$PATH]\n").unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let path_out = String::from_utf8_lossy(&captured.lock().unwrap()).to_string();
    captured.lock().unwrap().clear();
    println!("--- $PATH as seen inside the spawned shell ---\n{path_out}\n");

    handle.write(b"node -v; echo NODE_EXIT=$?\n").unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let out1 = String::from_utf8_lossy(&captured.lock().unwrap()).to_string();
    captured.lock().unwrap().clear();

    handle.write(b"npm -v; echo NPM_EXIT=$?\n").unwrap();
    std::thread::sleep(Duration::from_secs(3));
    let out2 = String::from_utf8_lossy(&captured.lock().unwrap()).to_string();

    println!("--- node -v output ---\n{out1}");
    println!("--- npm -v output ---\n{out2}");

    let node_ok = out1.contains("NODE_EXIT=0");
    let npm_ok = out2.contains("NPM_EXIT=0");
    println!(
        "\n>>> RESULT: node -v {}  |  npm -v {}",
        if node_ok {
            "WORKS"
        } else {
            "FAILED (command not found)"
        },
        if npm_ok {
            "WORKS"
        } else {
            "FAILED (command not found)"
        },
    );

    handle.shutdown().ok();
}
