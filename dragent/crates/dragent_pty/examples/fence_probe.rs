// Probe: does the PowerShell "fence" sentinel cell (SGR bg 48;2;1;2;3) survive
// ConPTY's repaint byte-for-byte, and how long after the precmd OSC report does
// it actually arrive at the reader? terminal_session_core's fence scanner looks
// for the literal byte substring "48;2;1;2;3" in the raw PTY stream; if ConPTY
// reformats/downsamples that SGR when repainting, the scanner never matches and
// every block falls back to the 160ms drain timer — a visible flash of raw
// terminal content before the block UI reasserts itself.
//
// Run: cargo run -p dragent_pty --example fence_probe
// (DRAGENT_CONPTY=1 is set on the child, exactly as the real app's shell_env.rs does.)

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
    let hooks_path = std::env::temp_dir().join("dragent_hooks_fence.ps1");
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
    // This is the bit the older conpty_probe.rs never set — without it the
    // hooks' `if ($env:DRAGENT_CONPTY)` guard is false and NEITHER the preexec
    // screen-clear NOR the precmd fence write ever happens at all.
    cmd.env("DRAGENT_CONPTY", "1");
    let _child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    let (tx, rx) = mpsc::channel::<(u64, Vec<u8>)>();
    let start0 = Instant::now();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let ms = start0.elapsed().as_millis() as u64;
                    if tx.send((ms, buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let user_cmd = std::env::var("PROBE_CMD").unwrap_or_else(|_| "Get-ChildItem".to_string());
    let src_ms: u64 = 200;
    let cmd_ms = 1600;

    let script: Vec<(u64, Vec<u8>)> = vec![
        (
            src_ms,
            format!(". '{}'\r", hooks_path.display()).into_bytes(),
        ),
        (cmd_ms, format!("{}\r", user_cmd).into_bytes()),
        (cmd_ms + 2500, b"exit\r".to_vec()),
    ];

    let start = Instant::now();
    let mut sent = 0usize;
    let mut all = Vec::new();
    let mut chunks: Vec<(u64, Vec<u8>)> = Vec::new();
    let deadline = Duration::from_millis(8000);

    while start.elapsed() < deadline {
        let ms = start.elapsed().as_millis() as u64;
        while sent < script.len() && script[sent].0 <= ms {
            let _ = writer.write_all(&script[sent].1);
            let _ = writer.flush();
            sent += 1;
        }
        if let Ok((recv_ms, chunk)) = rx.recv_timeout(Duration::from_millis(30)) {
            all.extend_from_slice(&chunk);
            chunks.push((recv_ms, chunk));
        }
    }

    println!(
        "================ RAW STREAM ({} bytes) ================",
        all.len()
    );
    println!("{}", visible(&all));

    println!("\n================ FENCE ANALYSIS ================");
    const FENCE: &[u8] = b"48;2;1;2;3";
    let fence_count = all.windows(FENCE.len()).filter(|w| *w == FENCE).count();
    println!("literal fence \"48;2;1;2;3\" occurrences: {fence_count}");

    // Any OTHER background-color SGR near where we'd expect the fence — evidence
    // ConPTY reformatted/downsampled the true-color code instead of dropping it.
    let precmd_positions: Vec<usize> = all
        .windows(b"7979;precmd".len())
        .enumerate()
        .filter(|(_, w)| *w == b"7979;precmd")
        .map(|(i, _)| i)
        .collect();
    println!("precmd OSC report occurrences: {}", precmd_positions.len());

    for &p in &precmd_positions {
        let window_end = (p + 200).min(all.len());
        let after = &all[p..window_end];
        println!(
            "\n--- 200 bytes after a precmd report ---\n{}",
            visible(after)
        );
    }

    println!("\n================ CHUNK TIMELINE (ms since spawn) ================");
    for (ms, chunk) in &chunks {
        let has_precmd = chunk.windows(11).any(|w| w == b"7979;precmd");
        let has_fence = chunk.windows(FENCE.len()).any(|w| w == FENCE);
        if has_precmd || has_fence || chunk.len() > 2 {
            println!(
                "@{:>5}ms  {:>4}B  precmd={:<5} fence={:<5}  {}",
                ms,
                chunk.len(),
                has_precmd,
                has_fence,
                visible(&chunk[..chunk.len().min(120)])
            );
        }
    }
}
