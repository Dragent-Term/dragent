//! Reproduction harness: spawn interactive PowerShell in a ConPTY exactly like
//! the app does, inject the shell-hooks bootstrap the same way TerminalManager
//! does, then submit `ls` and `cd ..` and dump the raw PTY output.
//!
//! Run with: cargo test -p dragent_pty --test ps_repro -- --nocapture --ignored

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dragent_pty::{spawn_interactive, PtyConfig, PtyFeedStats};

#[test]
#[ignore = "manual reproduction harness; needs a real ConPTY + powershell"]
fn powershell_interactive_ls_repro() {
    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured_feed = captured.clone();
    let feed: dragent_pty::FeedCallback = Box::new(move |bytes: &[u8]| {
        captured_feed.lock().unwrap().extend_from_slice(bytes);
        PtyFeedStats {
            bytes_processed: bytes.len(),
        }
    });
    let sync_len: dragent_pty::SyncBufferLenCallback = Box::new(|| None);
    let on_wakeup: dragent_pty::WakeupCallback = Box::new(|| {});

    let config = PtyConfig {
        shell: Some("C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_string()),
        ..PtyConfig::default()
    };
    let (handle, _rx) = spawn_interactive(config, feed, sync_len, on_wakeup).expect("spawn");

    // Give the shell time to start, then inject the bootstrap like
    // TerminalManager::install_shell_hooks does.
    std::thread::sleep(Duration::from_millis(3000));
    let kind = dragent_shell_hooks::ShellKind::PowerShell;
    let path = dragent_shell_hooks::install_bootstrap(kind)
        .expect("install bootstrap")
        .expect("powershell has a bootstrap");
    let source = dragent_shell_hooks::source_command(kind, &path);
    handle.write(source.as_bytes()).unwrap();
    handle.write(b"\r").unwrap();

    std::thread::sleep(Duration::from_millis(3000));

    // Submit `ls` the way write_command does on Windows: Ctrl-U then command + CR.
    handle.write(b"\x15ls\r").unwrap();
    std::thread::sleep(Duration::from_millis(4000));
    handle.write(b"\x15cd ..\r").unwrap();
    std::thread::sleep(Duration::from_millis(3000));

    let raw = captured.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&raw);
    println!("==== RAW ({} bytes) ====", raw.len());
    // Make escapes visible.
    let vis = text
        .replace('\u{1b}', "\\e")
        .replace('\r', "\\r")
        .replace('\n', "\\n\n");
    println!("{vis}");
    println!("==== END ====");

    handle.shutdown().ok();
}
