// Reproduce the "first character dropped" bug (ls -> s) seen with Git Bash.
//
// The app's write_command (terminal_view.rs) sends a command as TWO separate PTY
// writes: the command bytes, then the submit byte (`\r`) as a distinct write.
// The earlier sequence probe wrote "ls\r" as ONE chunk and worked. This probe
// replicates the app's split-write pattern, and also a char-by-char mode, to see
// which one loses the leading byte.
//
// Run against Git Bash:
//   $env:PROBE_SHELL="C:\Program Files\Git\bin\bash.exe"; $env:SHELL=""
//   $env:PROBE_MODE="split"   # or "single" or "chars"
//   cargo run -p dragent_pty --example write_split_probe

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_pty::{spawn_interactive, PtyConfig, PtyFeedStats};
use dragent_shell_hooks::ShellKind;

fn visible(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            0x1b => s.push_str("\\e"),
            b'\n' => s.push_str("\\n\n"),
            b'\r' => s.push_str("\\r"),
            0x07 => s.push_str("\\a"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s
}

fn main() {
    let shell = std::env::var("PROBE_SHELL")
        .unwrap_or_else(|_| "C:\\Program Files\\Git\\bin\\bash.exe".to_string());
    let mode = std::env::var("PROBE_MODE").unwrap_or_else(|_| "split".to_string());
    let cmd = std::env::var("PROBE_CMD").unwrap_or_else(|_| "ls".to_string());

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let feed_buf = buf.clone();
    let feed = Box::new(move |bytes: &[u8]| -> PtyFeedStats {
        feed_buf.lock().unwrap().extend_from_slice(bytes);
        PtyFeedStats {
            bytes_processed: bytes.len(),
        }
    });

    let config = PtyConfig {
        shell: Some(shell.clone()),
        cwd: std::env::current_dir().ok(),
        rows: 24,
        cols: 80,
    };

    let (handle, _rx) =
        spawn_interactive(config, feed, Box::new(|| None), Box::new(|| {})).expect("spawn failed");

    // Install hooks exactly like the app.
    let kind = ShellKind::from_program(&shell);
    if let Some(path) = dragent_shell_hooks::install_bootstrap(kind).expect("install") {
        let src = dragent_shell_hooks::source_command(kind, &path);
        handle.write(src.as_bytes()).unwrap();
        handle.write(b"\r").unwrap();
    }
    std::thread::sleep(Duration::from_millis(2500));
    let before = buf.lock().unwrap().len();

    eprintln!(">>> shell={shell}");
    eprintln!(">>> mode={mode}  cmd={cmd:?}");
    match mode.as_str() {
        // App's actual pattern: command bytes, then CR as a separate write.
        "split" => {
            handle.write(cmd.as_bytes()).unwrap();
            handle.write(b"\r").unwrap();
        }
        // Control: everything in one write (this worked in the sequence probe).
        "single" => {
            let mut v = cmd.into_bytes();
            v.push(b'\r');
            handle.write(&v).unwrap();
        }
        // Extreme: one write per character, then CR.
        "chars" => {
            for ch in cmd.as_bytes() {
                handle.write(&[*ch]).unwrap();
            }
            handle.write(b"\r").unwrap();
        }
        other => panic!("unknown PROBE_MODE {other}"),
    }

    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(3000) {
        std::thread::sleep(Duration::from_millis(50));
    }

    let all = buf.lock().unwrap().clone();
    let after = &all[before.min(all.len())..];

    // Extract the preexec payload the shell actually saw.
    let preexec = {
        let needle = b"7979;preexec;";
        all.windows(needle.len())
            .position(|w| w == needle)
            .map(|p| {
                let start = p + needle.len();
                let mut end = start;
                while end < all.len() && all[end] != 0x1b && all[end] != 0x07 {
                    end += 1;
                }
                String::from_utf8_lossy(&all[start..end]).to_string()
            })
    };

    println!("\n================ RESULT (mode={mode}) ================");
    println!(
        "  wrote command : {cmd:?}",
        cmd = std::env::var("PROBE_CMD").unwrap_or_else(|_| "ls".into())
    );
    println!("  preexec saw   : {preexec:?}   <-- if this drops the first char, the shell received the wrong bytes");
    println!(
        "\n---- bytes received AFTER command was sent ----\n{}",
        visible(after)
    );
}
