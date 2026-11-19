//! Reproduction harness for the "npm install never completes" bug: spawn
//! interactive PowerShell in a ConPTY like the app does, inject the shell-hooks
//! bootstrap, run a real `npm install` in a tiny project, and dump the raw PTY
//! bytes so we can see whether the precmd OSC + fence arrive and in what shape.
//!
//! Run with:
//!   DRAGENT_NPM_REPRO_DIR=<project dir> cargo test -p dragent_pty --test npm_repro -- --nocapture --ignored

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_pty::{spawn_interactive, PtyConfig, PtyFeedStats};

#[test]
#[ignore = "manual reproduction harness; needs a real ConPTY + powershell + npm"]
fn powershell_npm_install_repro() {
    let proj = std::env::var("DRAGENT_NPM_REPRO_DIR").expect("set DRAGENT_NPM_REPRO_DIR");

    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    // Every PTY read chunk, in order, so the session-core replay test can feed
    // them with the exact boundaries the app saw (the fence scanner is
    // chunk-boundary sensitive).
    let chunks = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let captured_feed = captured.clone();
    let chunks_feed = chunks.clone();
    let feed: dragent_pty::FeedCallback = Box::new(move |bytes: &[u8]| {
        captured_feed.lock().unwrap().extend_from_slice(bytes);
        chunks_feed.lock().unwrap().push(bytes.to_vec());
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

    std::thread::sleep(Duration::from_millis(3000));
    let kind = dragent_shell_hooks::ShellKind::PowerShell;
    let path = dragent_shell_hooks::install_bootstrap(kind)
        .expect("install bootstrap")
        .expect("powershell has a bootstrap");
    let source = dragent_shell_hooks::source_command(kind, &path);
    handle.write(source.as_bytes()).unwrap();
    handle.write(b"\r").unwrap();
    std::thread::sleep(Duration::from_millis(3000));

    handle
        .write(format!("\x15cd \"{proj}\"\r").as_bytes())
        .unwrap();
    std::thread::sleep(Duration::from_millis(2000));

    // Clean slate so npm actually does work.
    handle
        .write(b"\x15if (Test-Path node_modules) { Remove-Item -Recurse -Force node_modules }\r")
        .unwrap();
    std::thread::sleep(Duration::from_millis(3000));

    let mark = captured.lock().unwrap().len();
    handle
        .write(b"\x15npm install --no-audit --no-fund\r")
        .unwrap();

    // Wait until we see a precmd OSC after the npm output, or time out.
    let timeout_secs: u64 = std::env::var("DRAGENT_NPM_REPRO_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut saw_precmd = false;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let buf = captured.lock().unwrap();
        if twoway_find(&buf[mark..], b"]7979;precmd").is_some() {
            saw_precmd = true;
            break;
        }
    }
    // Grab a little extra after precmd (prompt repaint, fence).
    std::thread::sleep(Duration::from_millis(2000));

    let raw = captured.lock().unwrap().clone();
    let npm_part = &raw[mark..];
    println!(
        "==== npm chunk: {} bytes, saw_precmd={} ====",
        npm_part.len(),
        saw_precmd
    );

    let precmd_count = count_occurrences(npm_part, b"]7979;precmd");
    let preexec_count = count_occurrences(npm_part, b"]7979;preexec");
    let fence_count = count_occurrences(npm_part, b"48;2;1;2;3");
    println!(
        "precmd={} preexec={} fence-sgr={}",
        precmd_count, preexec_count, fence_count
    );

    // Dump the FIRST 3000 bytes and the LAST 6000 bytes with escapes visible.
    let head_len = npm_part.len().min(3000);
    println!("---- first {head_len} bytes ----");
    println!("{}", visible(&npm_part[..head_len]));
    let tail_start = npm_part.len().saturating_sub(6000);
    println!("---- last {} bytes ----", npm_part.len() - tail_start);
    println!("{}", visible(&npm_part[tail_start..]));
    println!("==== END ====");

    // Persist the npm-command chunks (u32-LE length prefix + bytes each) for
    // the session-core replay test. Only chunks at/after the mark.
    if let Ok(out) = std::env::var("DRAGENT_NPM_CAPTURE_OUT") {
        let chunks = chunks.lock().unwrap();
        let mut offset = 0usize;
        let mut file = Vec::new();
        for chunk in chunks.iter() {
            if offset >= mark {
                file.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                file.extend_from_slice(chunk);
            }
            offset += chunk.len();
        }
        std::fs::write(&out, file).expect("write capture");
        println!("wrote chunked capture to {out}");
    }

    handle.shutdown().ok();
}

fn visible(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.replace('\u{1b}', "\\e")
        .replace('\u{7}', "\\a")
        .replace('\r', "\\r")
        .replace('\n', "\\n\n")
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    let mut count = 0;
    let mut i = 0;
    while let Some(pos) = twoway_find(&haystack[i..], needle) {
        count += 1;
        i += pos + needle.len();
    }
    count
}

fn twoway_find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
