//! Live tests: real apps on this desktop, read and changed through the
//! platform's accessibility layer, then read back. They open apps and act
//! in them, so they run only when asked:
//!
//!     cargo test -p dragent_computer --test live -- --live [name filter]
//!
//! (`--live --permissions` only says what this desktop allows.)
//!
//! They need a desktop session: macOS with Accessibility allowed for the
//! process running them, Windows, or Linux with an X server and the
//! accessibility bus (as the computer-use workflow sets up). Typing and key
//! presses take over the keyboard, so those steps run only with
//! `DRAGENT_COMPUTER_LIVE_INPUT=1`. Every app they use is an instance of
//! their own, ended afterwards; apps already open are left alone.
//!
//! Requests go through the helper process, as they do in Dragent: this
//! binary started again with the helper flag, serving on its main thread.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use dragent_computer::{run, Access, Outcome, Request};
use serde_json::{json, Value};

const TESTS: &[(&str, fn())] = &[
    (
        "an_editor_is_read_written_and_read_back",
        an_editor_is_read_written_and_read_back,
    ),
    ("a_web_page_reads_compactly", a_web_page_reads_compactly),
    (
        "menus_and_window_geometry_work_in_an_editor",
        menus_and_window_geometry_work_in_an_editor,
    ),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == dragent_computer::SERVICE_ARG) {
        return dragent_computer::serve();
    }
    if !args.iter().any(|a| a == "--live") {
        println!("Live computer-use tests skipped: pass `-- --live` to run them on this desktop.");
        return;
    }
    let filter = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    dragent_computer::use_helper(vec![
        std::env::current_exe().unwrap().into_os_string(),
        dragent_computer::SERVICE_ARG.into(),
    ]);
    // What this desktop allows, before anything opens.
    println!(
        "{}",
        serde_json::to_string(&dragent_computer::permissions()).unwrap()
    );
    if args.iter().any(|a| a == "--permissions") {
        return;
    }
    let permissions = dragent_computer::permissions();
    if permissions.accessibility != Access::Granted {
        println!(
            "Live computer-use tests skipped: accessibility is unavailable for this runner: {:?}",
            permissions.notes
        );
        return;
    }
    let mut failed = Vec::new();
    for (name, test) in TESTS.iter().filter(|(name, _)| name.contains(&filter)) {
        println!("live {name} ...");
        match std::panic::catch_unwind(test) {
            Ok(()) => println!("live {name} ... ok"),
            Err(_) => {
                println!("live {name} ... FAILED");
                failed.push(*name);
            }
        }
    }
    dragent_computer::stop_helper();
    if !failed.is_empty() {
        eprintln!("failed: {}", failed.join(", "));
        std::process::exit(1);
    }
}

fn start() {
    let permissions = dragent_computer::permissions();
    assert_eq!(
        permissions.accessibility,
        Access::Granted,
        "Accessibility isn't allowed for this process: {:?}",
        permissions.notes
    );
}

fn call(tool: &str, arguments: Value) -> Result<Outcome, String> {
    run(Request::from_tool(tool, &arguments)?)
}

fn input_allowed() -> bool {
    std::env::var("DRAGENT_COMPUTER_LIVE_INPUT").is_ok_and(|v| v == "1")
}

fn eventually<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "{what} didn't happen within 45 s"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn snapshot(app: &str) -> Result<Outcome, String> {
    call("snapshot", json!({ "app": app, "screenshot": false }))
}

/// The number and line of the first element whose line matches.
fn find(outcome: &Outcome, matches: impl Fn(&str) -> bool) -> Option<(usize, String)> {
    outcome.data["elements"]
        .as_array()?
        .iter()
        .find(|element| matches(element["line"].as_str().unwrap_or_default()))
        .map(|element| {
            (
                element["index"].as_u64().unwrap_or_default() as usize,
                element["line"].as_str().unwrap_or_default().to_string(),
            )
        })
}

fn editable(line: &str) -> bool {
    ["] text area", "] document", "] text field", "] edit"]
        .iter()
        .any(|role| line.contains(role))
}

fn pids_named(names: &[&str]) -> Vec<u32> {
    let Ok(outcome) = call("apps", json!({})) else {
        return Vec::new();
    };
    outcome.data["apps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|app| {
            let name = app["name"].as_str().unwrap_or_default().to_lowercase();
            let id = app["id"].as_str().unwrap_or_default().to_lowercase();
            names.iter().any(|n| name.contains(n) || id.contains(n))
        })
        .filter_map(|app| app["pid"].as_u64().map(|pid| pid as u32))
        .collect()
}

/// An app instance a test started; ended when dropped.
struct Opened {
    pid: u32,
    launcher: Child,
}

impl Opened {
    fn app(&self) -> String {
        format!("pid:{}", self.pid)
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let pid = self.pid.to_string();
        if cfg!(windows) {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid])
                .status();
        } else {
            let _ = Command::new("kill").args(["-9", &pid]).status();
        }
        let _ = self.launcher.kill();
        let _ = self.launcher.wait();
    }
}

/// Start an app with `launch` and wait for its new process to show up.
fn open(names: &[&str], launch: &mut Command) -> Opened {
    let before = pids_named(names);
    let launcher = launch.spawn().expect("the app didn't start");
    let pid = eventually("the app to open a window", || {
        pids_named(names)
            .into_iter()
            .find(|pid| !before.contains(pid))
    });
    Opened { pid, launcher }
}

fn open_editor(file: &Path) -> Opened {
    if cfg!(target_os = "macos") {
        // A fresh instance of its own, in the background, not restoring or
        // saving windows.
        open(
            &["textedit"],
            Command::new("open")
                .args(["-g", "-n", "-F", "-a", "TextEdit"])
                .arg(file)
                .args(["--args", "-ApplePersistenceIgnoreState", "YES"]),
        )
    } else if cfg!(windows) {
        open(&["notepad"], Command::new("notepad.exe").arg(file))
    } else {
        open(
            &["gedit"],
            Command::new("gedit").arg("--standalone").arg(file),
        )
    }
}

fn open_browser(page: &Path, profile: &Path) -> Opened {
    let url = format!("file://{}", page.display()).replace('\\', "/");
    let mut flags = vec![
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--new-window".into(),
    ];
    if cfg!(target_os = "linux") {
        // Chromium otherwise exposes no accessibility tree in the headless
        // desktop session used by the live workflow.
        flags.push("--force-renderer-accessibility".into());
    }
    flags.push(url);
    if cfg!(target_os = "macos") {
        open(
            &["google chrome", "com.google.chrome"],
            Command::new("open")
                .args(["-g", "-n", "-a", "Google Chrome", "--args"])
                .args(&flags),
        )
    } else if cfg!(windows) {
        let chrome = [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ]
        .into_iter()
        .find(|path| Path::new(path).exists())
        .expect("these tests use Google Chrome");
        open(&["chrome"], Command::new(chrome).args(&flags))
    } else {
        // Chrome shows its pages to assistive apps on Linux only when asked
        // to at startup, as desktop accessibility settings do.
        open(
            &["chrome"],
            Command::new("google-chrome")
                .args(&flags)
                .env("ACCESSIBILITY_ENABLED", "1"),
        )
    }
}

fn an_editor_is_read_written_and_read_back() {
    start();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dragent-live.txt");
    std::fs::write(&file, "First line from the file\n").unwrap();
    let editor = open_editor(&file);
    let app = editor.app();

    let (area, line) = eventually("the document to load", || {
        find(&snapshot(&app).ok()?, |line| {
            editable(line) && line.contains("First line from the file")
        })
    });
    println!("editor: {line}");

    let set = call(
        "set_value",
        json!({ "app": app, "element": area, "value": "Written by Dragent", "screenshot": false }),
    )
    .unwrap();
    // The action answers with a fresh look at the window.
    assert!(set.text.contains("Written by Dragent"), "{}", set.text);
    let (_, line) = find(&snapshot(&app).unwrap(), editable).expect("the editor is still there");
    assert!(line.contains("Written by Dragent"), "{line}");

    if input_allowed() {
        let (area, _) = find(&snapshot(&app).unwrap(), editable).unwrap();
        call(
            "click",
            json!({ "app": app, "element": area, "restore_window": true, "screenshot": false }),
        )
        .unwrap();
        let pressed = call(
            "key",
            json!({ "app": app, "key": "cmdorctrl+a", "restore_window": true, "screenshot": false }),
        )
        .unwrap();
        println!("key: {}", pressed.text.lines().next().unwrap_or_default());
        let typed = call(
            "type",
            json!({ "app": app, "text": "Typed by Dragent", "restore_window": true, "screenshot": false }),
        )
        .unwrap();
        println!("type: {}", typed.text.lines().next().unwrap_or_default());
        let mut seen = String::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        let line = loop {
            if let Some((_, line)) = snapshot(&app).ok().and_then(|shot| find(&shot, editable)) {
                if line.contains("Typed by Dragent") {
                    break line;
                }
                seen = line;
            }
            assert!(
                Instant::now() < deadline,
                "the typed text never arrived; the editor shows: {seen}"
            );
            std::thread::sleep(Duration::from_millis(500));
        };
        assert!(!line.contains("Written by Dragent"), "{line}");
    }
}

fn a_web_page_reads_compactly() {
    start();
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.html");
    std::fs::write(
        &page,
        r##"<!doctype html><html><head><title>Dragent live page</title></head><body>
<h1>Live heading</h1>
<p>Read the <a href="#far">reference docs</a> for more.</p>
<table><tr><td>Inbox</td><td>3 unread</td></tr></table>
<button onclick="document.title = 'Pressed by Dragent'">Press me</button>
<div style="height: 6000px"></div>
<p id="far">Far below</p><button>Far button</button>
</body></html>"##,
    )
    .unwrap();
    let browser = open_browser(&page, &dir.path().join("profile"));
    let app = browser.app();

    let shot = eventually("the page to be readable", || {
        let shot = snapshot(&app).ok()?;
        shot.text.contains("Press me").then_some(shot)
    });
    println!("{}", shot.text);
    // Where each element sits, against the window, for reading a failure.
    println!("window {}", shot.data["window"]["frame"]);
    for element in shot.data["elements"].as_array().into_iter().flatten() {
        println!("[{}] {}", element["index"], element["frame"]);
    }
    for expected in [
        "heading \"Live heading\"",
        "link \"reference docs\"",
        "Inbox",
        "button \"Press me\"",
    ] {
        assert!(
            shot.text.contains(expected),
            "missing {expected}:\n{}",
            shot.text
        );
    }
    assert!(!shot.text.contains("Far button"), "{}", shot.text);
    assert!(shot.text.contains("outside the window"), "{}", shot.text);
    assert_eq!(shot.data["offscreen"].as_u64().map(|n| n > 0), Some(true));

    let (button, _) = find(&shot, |line| line.contains("button \"Press me\"")).unwrap();
    call(
        "click",
        json!({ "app": app, "element": button, "screenshot": false }),
    )
    .unwrap();
    eventually("the page to react to the press", || {
        let windows = call("windows", json!({ "app": app })).ok()?;
        windows.text.contains("Pressed by Dragent").then_some(())
    });
}

fn menus_and_window_geometry_work_in_an_editor() {
    if !cfg!(target_os = "macos") {
        // Elsewhere both answer unsupported_capability, as capabilities say.
        start();
        let refused = call("menus", json!({ "app": "pid:1" })).unwrap_err();
        println!("menus elsewhere: {refused}");
        return;
    }
    start();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dragent-menus.txt");
    std::fs::write(&file, "Menus and windows\n").unwrap();
    let editor = open_editor(&file);
    let app = editor.app();
    eventually("the document to load", || {
        find(&snapshot(&app).ok()?, |line| {
            line.contains("Menus and windows")
        })
    });

    let bar = call("menus", json!({ "app": app })).unwrap();
    println!(
        "{}",
        bar.text.lines().take(12).collect::<Vec<_>>().join("\n")
    );
    let items = bar.data["items"].as_array().unwrap();
    let file_menu = items
        .iter()
        .find(|menu| menu["title"] == "File")
        .expect("a File menu");
    let save = file_menu["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["title"]
                .as_str()
                .is_some_and(|t| t.starts_with("Save"))
        })
        .expect("File › Save");
    assert_eq!(save["shortcut"], "⌘S", "{save}");
    let font = call("menus", json!({ "app": app, "path": "Format > Font" })).unwrap();
    assert!(font.text.contains("Show Fonts"), "{}", font.text);
    let missing = call("menu", json!({ "app": app, "path": "Format > Teleport" })).unwrap_err();
    assert!(missing.starts_with("element_not_found"), "{missing}");

    if input_allowed() {
        let format = call("menus", json!({ "app": app, "path": "Format" })).unwrap();
        let (to, back) = if format.text.contains("Make Rich Text") {
            ("Make Rich Text", "Make Plain Text")
        } else {
            ("Make Plain Text", "Make Rich Text")
        };
        let chose = call(
            "menu",
            json!({ "app": app, "path": format!("Format > {to}"), "screenshot": false }),
        )
        .unwrap();
        println!("menu: {}", chose.text.lines().next().unwrap_or_default());
        eventually("the menu choice to take effect", || {
            let format = call("menus", json!({ "app": app, "path": "Format" })).ok()?;
            format.text.contains(back).then_some(())
        });
    }

    let moved = call(
        "window",
        json!({ "app": app, "x": 120, "y": 140, "width": 640, "height": 420, "screenshot": false }),
    )
    .unwrap();
    println!("window: {}", moved.text.lines().next().unwrap_or_default());
    assert_eq!(
        moved.data["action"]["verification"]["state"], "verified",
        "{}",
        moved.text
    );
    let frame = &moved.data["action"]["frame"];
    assert_eq!(
        (frame["x"].as_f64(), frame["width"].as_f64()),
        (Some(120.0), Some(640.0))
    );
    let listed = call("windows", json!({ "app": app })).unwrap();
    assert!(listed.text.contains("640"), "{}", listed.text);

    call(
        "window",
        json!({ "app": app, "state": "minimize", "screenshot": false }),
    )
    .unwrap();
    call(
        "window",
        json!({ "app": app, "state": "restore", "screenshot": false }),
    )
    .unwrap();
    let back = call(
        "window",
        json!({ "app": app, "x": 160, "y": 140, "screenshot": false }),
    )
    .unwrap();
    assert_eq!(
        back.data["action"]["frame"]["x"].as_f64(),
        Some(160.0),
        "{}",
        back.text
    );
}
