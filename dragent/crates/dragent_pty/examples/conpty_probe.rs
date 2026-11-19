// Temporary diagnostic probe: spawn PowerShell under ConPTY, optionally source
// the dragent hooks, run a normal command, and dump the raw byte stream with
// escape sequences made visible. Flags alt-screen and OSC-7979 hook sequences.
//
// Run: cargo run -p dragent_pty --example conpty_probe

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn visible(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            0x1b => s.push_str("\\e"),
            b'\n' => s.push_str("\\n\n"),
            b'\r' => s.push_str("\\r"),
            0x07 => s.push_str("\\a"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{:02x}", b)),
        }
    }
    s
}

fn main() {
    let hooks_path = std::env::temp_dir().join("dragent_hooks.ps1");
    std::fs::write(
        &hooks_path,
        dragent_shell_hooks::bootstrap_script(dragent_shell_hooks::ShellKind::PowerShell).unwrap(),
    )
    .unwrap();

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new("powershell.exe");
    cmd.arg("-NoLogo");
    cmd.arg("-NoExit");
    let _child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    // Reader thread -> channel.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let source_hooks = std::env::var("PROBE_HOOKS").as_deref() != Ok("0");
    let user_cmd = std::env::var("PROBE_CMD").unwrap_or_else(|_| "echo hello-block".to_string());

    // When (ms after spawn) to source the hooks. dragent sources immediately at
    // spawn (~0ms), BEFORE PowerShell shows its first prompt; the standalone probe
    // historically waited 800ms. Configurable via PROBE_SRC_MS to compare.
    let src_ms: u64 = std::env::var("PROBE_SRC_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(800);
    let cmd_ms = src_ms.max(800) + 1200;

    // Timeline of writes.
    let script: Vec<(u64, Vec<u8>)> = {
        let mut v = Vec::new();
        if source_hooks {
            v.push((
                src_ms,
                format!(". '{}'\r", hooks_path.display()).into_bytes(),
            ));
        }
        v.push((cmd_ms, format!("{}\r", user_cmd).into_bytes()));
        v.push((cmd_ms + 1800, b"exit\r".to_vec()));
        v
    };

    let start = Instant::now();
    let mut sent = 0usize;
    let mut all = Vec::new();
    let deadline = Duration::from_millis(9000);
    let mut last_beat = 0u64;

    while start.elapsed() < deadline {
        let ms = start.elapsed().as_millis() as u64;
        while sent < script.len() && script[sent].0 <= ms {
            let _ = writer.write_all(&script[sent].1);
            let _ = writer.flush();
            eprintln!("\n>>> WROTE @{}ms: {}", ms, visible(&script[sent].1));
            sent += 1;
        }
        if ms / 1000 != last_beat {
            last_beat = ms / 1000;
            eprintln!("... @{}ms received {} bytes so far", ms, all.len());
        }
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(50)) {
            all.extend_from_slice(&chunk);
        }
    }

    println!(
        "\n================ RAW STREAM ({} bytes) ================\n",
        all.len()
    );
    println!("{}", visible(&all));

    println!("\n================ SEQUENCE SCAN ================");
    let hay = &all;
    let count =
        |needle: &[u8]| -> usize { hay.windows(needle.len()).filter(|w| *w == needle).count() };
    println!("  ?1049h (enter alt): {}", count(b"[?1049h"));
    println!("  ?1049l (exit  alt): {}", count(b"[?1049l"));
    println!("  ?1047h            : {}", count(b"[?1047h"));
    println!("  ?47h              : {}", count(b"[?47h"));
    println!("  OSC 7979 (hooks)  : {}", count(b"]7979;"));
    println!("  preexec           : {}", count(b"7979;preexec"));
    println!("  precmd            : {}", count(b"7979;precmd"));
}
