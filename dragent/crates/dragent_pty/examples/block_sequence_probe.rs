// Sequence probe: does dragent's OWN ConPTY stack deliver a CLEAN, ordered
// preexec -> precmd per command, or does it emit stray `precmd`s (e.g. on a
// resize/repaint) that would prematurely finalise an optimistic block?
//
// Unlike dragent_pty_probe (totals only), this records every OSC 7979 marker in
// arrival order WITH an elapsed timestamp, and injects a resize between two
// commands to reproduce the repaint that PowerShell's `prompt`-wrapper turns
// into a stray precmd. zsh/bash should show exactly: preexec;<cmd> ... precmd;..
// once per command and NOTHING on resize.
//
// Run (force Windows PowerShell 5.1):
//   $env:PROBE_SHELL="C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
//   $env:SHELL=""; cargo run -p dragent_pty --example block_sequence_probe

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_pty::{spawn_interactive, PtyConfig, PtyFeedStats};
use dragent_shell_hooks::ShellKind;

/// A parsed OSC 7979 marker with the elapsed time (ms since spawn) it arrived.
#[derive(Clone)]
struct Marker {
    at_ms: u128,
    payload: String,
}

/// Shared state scanned incrementally in the feed callback so each marker is
/// stamped with its TRUE arrival time. `cursor` is how far into `buf` we've
/// already extracted complete markers from.
#[derive(Default)]
struct Capture {
    buf: Vec<u8>,
    cursor: usize,
    markers: Vec<Marker>,
}

/// Append a freshly-arrived chunk and extract any newly-completed markers.
fn ingest(cap: &mut Capture, chunk: &[u8], at_ms: u128) {
    cap.buf.extend_from_slice(chunk);
    let needle = b"7979;";
    let buf = &cap.buf;
    let mut i = cap.cursor;
    while i + needle.len() <= buf.len() {
        if &buf[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            // payload runs until ESC (ST/String terminator) or BEL.
            while j < buf.len() && buf[j] != 0x1b && buf[j] != 0x07 {
                j += 1;
            }
            if j < buf.len() {
                let payload = String::from_utf8_lossy(&buf[i + needle.len()..j]).to_string();
                cap.markers.push(Marker { at_ms, payload });
                cap.cursor = j;
                i = j;
                continue;
            } else {
                // marker not yet terminated — leave cursor here for next chunk.
                break;
            }
        }
        i += 1;
    }
}

fn main() {
    let shell = std::env::var("PROBE_SHELL").unwrap_or_else(|_| "powershell.exe".to_string());

    let start = Instant::now();
    let cap = Arc::new(Mutex::new(Capture::default()));
    let feed_cap = cap.clone();
    let feed = Box::new(move |bytes: &[u8]| -> PtyFeedStats {
        let at_ms = start.elapsed().as_millis();
        ingest(&mut feed_cap.lock().unwrap(), bytes, at_ms);
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

    // A timeline of (label, action). Actions are closures run at the given ms.
    // We narrate each write so the marker log can be read against what we did.
    let kind = ShellKind::from_program(&shell);
    let path = dragent_shell_hooks::install_bootstrap(kind)
        .expect("install_bootstrap")
        .expect("no bootstrap for shell");
    let source_cmd = dragent_shell_hooks::source_command(kind, &path);

    struct Step {
        at_ms: u128,
        note: &'static str,
        run: Box<dyn Fn(&dragent_pty::PtySessionHandle)>,
    }
    let src = source_cmd.clone();
    let steps: Vec<Step> = vec![
        Step {
            at_ms: 100,
            note: "SOURCE HOOKS",
            run: Box::new(move |h| {
                h.write(src.as_bytes()).unwrap();
                h.write(b"\r").unwrap();
            }),
        },
        Step {
            at_ms: 3000,
            note: "RUN `ls`",
            run: Box::new(|h| {
                h.write(b"ls\r").unwrap();
            }),
        },
        Step {
            at_ms: 6000,
            note: "RESIZE 24x100 (repaint)",
            run: Box::new(|h| {
                h.resize(24, 100).unwrap();
            }),
        },
        Step {
            at_ms: 8000,
            note: "RUN `echo hello-block`",
            run: Box::new(|h| {
                h.write(b"echo hello-block\r").unwrap();
            }),
        },
    ];

    let deadline = Duration::from_millis(12000);
    let mut done = 0usize;
    let mut timeline: Vec<(u128, &'static str)> = Vec::new();
    while start.elapsed() < deadline {
        let ms = start.elapsed().as_millis();
        while done < steps.len() && steps[done].at_ms <= ms {
            (steps[done].run)(&handle);
            timeline.push((ms, steps[done].note));
            eprintln!(">>> @{ms}ms  {}", steps[done].note);
            done += 1;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let markers = cap.lock().unwrap().markers.clone();

    println!("\n================ MARKER TIMELINE (in arrival order) ================");
    println!("shell: {shell}");
    println!("(actions are what WE wrote; markers are what the shell EMITTED)\n");

    // Merge actions + markers into one ordered log.
    let mut events: Vec<(u128, String)> = Vec::new();
    for (ms, note) in &timeline {
        events.push((*ms, format!("  ACTION  ▸ {note}")));
    }
    for m in &markers {
        let short: String = m.payload.chars().take(60).collect();
        events.push((m.at_ms, format!("  marker  · 7979;{short}")));
    }
    events.sort_by_key(|e| e.0);
    for (ms, line) in &events {
        println!("  @{:>5}ms {}", ms, line);
    }

    let preexec = markers
        .iter()
        .filter(|m| m.payload.starts_with("preexec"))
        .count();
    let precmd = markers
        .iter()
        .filter(|m| m.payload.starts_with("precmd"))
        .count();
    let clear = markers
        .iter()
        .filter(|m| m.payload.starts_with("clear"))
        .count();
    println!("\n================ COUNTS ================");
    println!("  preexec : {preexec}");
    println!("  precmd  : {precmd}   (expect ~ 1 startup + 1 per command = 3; extras on RESIZE = the bug)");
    println!("  clear   : {clear}");
    println!("  2 commands run (ls, echo) => healthy = 2 preexec, and NO precmd adjacent to the RESIZE action");
}
