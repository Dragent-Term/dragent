// Reproduce the user's exact scenario under Git Bash and dump the RAW ConPTY
// stream with escapes visible, segmented by our OSC-7979 marks. Goal: see what
// bytes actually land between `preexec;ls` and the following `precmd` — i.e.
// whether ConPTY re-emits the previous prompts (a full-screen repaint) into the
// window the `ls` block's emulator captures, which would explain a block body
// showing the whole session scrollback instead of just `ls`'s output.
//
// Run:
//   $env:PROBE_SHELL="C:\Program Files\Git\bin\bash.exe"; $env:SHELL=""
//   $env:PROBE_RESIZE="1"   # optional: resize during `ls` to test repaint
//   cargo run -p dragent_pty --example multi_command_dump

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
    let do_resize = std::env::var("PROBE_RESIZE").as_deref() == Ok("1");

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
        cwd: dirs_home(),
        rows: 24,
        cols: 80,
    };
    let (handle, _rx) =
        spawn_interactive(config, feed, Box::new(|| None), Box::new(|| {})).expect("spawn");

    let kind = ShellKind::from_program(&shell);
    if let Some(path) = dragent_shell_hooks::install_bootstrap(kind).expect("install") {
        let src = dragent_shell_hooks::source_command(kind, &path);
        handle.write(src.as_bytes()).unwrap();
        handle.write(b"\r").unwrap();
    }
    std::thread::sleep(Duration::from_millis(2500));

    let mark = |label: &str, b: &Arc<Mutex<Vec<u8>>>| {
        b.lock().unwrap().len().max(0).to_string() + "|" + label
    };
    let _ = mark; // silence if unused

    // cd dev/project
    let len_before_cd = buf.lock().unwrap().len();
    handle.write(b"cd dev/project\r").unwrap();
    std::thread::sleep(Duration::from_millis(1500));

    // ls (optionally with a resize mid-flight to test repaint)
    let len_before_ls = buf.lock().unwrap().len();
    handle.write(b"ls\r").unwrap();
    if do_resize {
        std::thread::sleep(Duration::from_millis(120));
        handle.resize(24, 100).unwrap();
    }
    std::thread::sleep(Duration::from_millis(2000));

    let all = buf.lock().unwrap().clone();

    let seg = |from: usize, to: usize| -> String {
        visible(&all[from.min(all.len())..to.min(all.len())])
    };

    println!("\n================ SEGMENT: after `cd dev` submitted ================");
    println!("{}", seg(len_before_cd, len_before_ls));
    println!("\n================ SEGMENT: after `ls` submitted (this is what the ls block captures){} ================",
        if do_resize { " [with resize]" } else { "" });
    println!("{}", seg(len_before_ls, all.len()));

    // Does the ls segment re-contain the earlier prompt/command text? That would
    // mean ConPTY repainted prior screen content into the ls block's window.
    let ls_seg = &all[len_before_ls.min(all.len())..];
    let contains =
        |needle: &str| -> bool { ls_seg.windows(needle.len()).any(|w| w == needle.as_bytes()) };
    println!("\n================ REPAINT CHECK (in the `ls` segment) ================");
    println!(
        "  contains 'cd dev'                 : {}",
        contains("cd dev")
    );
    println!(
        "  contains 'builtin source'         : {}",
        contains("builtin source")
    );
    println!(
        "  contains 'dragent_hooks'          : {}",
        contains("dragent_hooks")
    );
    println!(
        "  => if any are true, ConPTY re-emitted earlier screen lines into the ls block window"
    );
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .map(std::path::PathBuf::from)
}
