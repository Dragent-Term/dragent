// Isolation probe: does dragent's OWN ConPTY stack (dragent_pty, not portable-pty)
// deliver the OSC 7979 hook markers when hooks are installed and a command is run
// exactly the way TerminalManager does it?
//
// Mirrors app/src/terminal/manager/terminal_manager.rs::install_shell_hooks
// (write `. 'hooks.ps1'` + \r immediately after spawn) and
// terminal_view.rs::write_command (write `ls` + \r).
//
// Run: cargo run -p dragent_pty --example dragent_pty_probe

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_pty::{spawn_interactive, PtyConfig, PtyFeedStats};
use dragent_shell_hooks::ShellKind;

fn count(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || hay.len() < needle.len() {
        return 0;
    }
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

fn main() {
    let shell = std::env::var("PROBE_SHELL").unwrap_or_else(|_| "powershell.exe".to_string());
    let user_cmd = std::env::var("PROBE_CMD").unwrap_or_else(|_| "ls".to_string());

    // Accumulate every byte the event loop feeds us — this is the exact stream the
    // real app parses into HookEvents.
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let feed_buf = buf.clone();
    let feed = Box::new(move |bytes: &[u8]| -> PtyFeedStats {
        feed_buf.lock().unwrap().extend_from_slice(bytes);
        PtyFeedStats {
            bytes_processed: bytes.len(),
        }
    });
    let sync_buffer_len = Box::new(|| None);
    let on_wakeup = Box::new(|| {});

    let config = PtyConfig {
        shell: Some(shell.clone()),
        cwd: std::env::current_dir().ok(),
        rows: 24,
        cols: 80,
    };

    let (handle, _rx) = spawn_interactive(config, feed, sync_buffer_len, on_wakeup)
        .expect("spawn_interactive failed");

    // --- Replicate install_shell_hooks (immediately after spawn) ---
    let kind = ShellKind::from_program(&shell);
    let path = dragent_shell_hooks::install_bootstrap(kind)
        .expect("install_bootstrap")
        .expect("no bootstrap for shell");
    let source_cmd = dragent_shell_hooks::source_command(kind, &path);
    eprintln!(">>> install hooks: {source_cmd:?}");
    handle.write(source_cmd.as_bytes()).unwrap();
    handle.write(b"\r").unwrap();

    // Give the shell time to source hooks and settle at a prompt.
    std::thread::sleep(Duration::from_millis(2500));
    let after_source = count(&buf.lock().unwrap(), b"7979;");

    // --- Replicate write_command (user runs `ls`) ---
    eprintln!(">>> run command: {user_cmd:?}");
    handle.write(user_cmd.as_bytes()).unwrap();
    handle.write(b"\r").unwrap();

    // Wait for the command to run and the next prompt to render.
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(3000) {
        std::thread::sleep(Duration::from_millis(100));
    }

    let all = buf.lock().unwrap().clone();

    println!("\n================ dragent_pty SEQUENCE SCAN ================");
    println!("  total bytes fed        : {}", all.len());
    println!("  OSC 7979 (any)         : {}", count(&all, b"]7979;"));
    println!("  7979; (loose)          : {}", count(&all, b"7979;"));
    println!("  ...after source only   : {after_source}");
    println!(
        "  preexec                : {}",
        count(&all, b"7979;preexec")
    );
    println!("  precmd                 : {}", count(&all, b"7979;precmd"));
    println!("  clear                  : {}", count(&all, b"7979;clear"));
    println!("  ?1049h (enter alt)     : {}", count(&all, b"[?1049h"));

    // Show a small window around the first marker, escapes visible, for sanity.
    if let Some(pos) = all.windows(5).position(|w| w == b"7979;") {
        let lo = pos.saturating_sub(6);
        let hi = (pos + 40).min(all.len());
        let mut vis = String::new();
        for &b in &all[lo..hi] {
            match b {
                0x1b => vis.push_str("\\e"),
                0x07 => vis.push_str("\\a"),
                0x20..=0x7e => vis.push(b as char),
                _ => vis.push_str(&format!("\\x{b:02x}")),
            }
        }
        println!("  first marker context   : {vis}");
    } else {
        println!("  first marker context   : <NO 7979 MARKER FOUND>");
    }

    // Full raw dump (escapes visible) so we can tell write-vs-read failure:
    // if we see the echo of ". 'hooks.ps1'" and "ls", writes reached the shell.
    let mut vis = String::new();
    for &b in &all {
        match b {
            0x1b => vis.push_str("\\e"),
            b'\n' => vis.push_str("\\n\n"),
            b'\r' => vis.push_str("\\r"),
            0x07 => vis.push_str("\\a"),
            0x20..=0x7e => vis.push(b as char),
            _ => vis.push_str(&format!("\\x{b:02x}")),
        }
    }
    println!(
        "\n================ RAW ({} bytes) ================\n{vis}",
        all.len()
    );
}
