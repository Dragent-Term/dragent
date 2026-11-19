//! `dragent browser`: drive the browser pane beside this terminal, the way a
//! person would — open pages, read them, click, type — while they watch.

use std::time::Duration;

use dragent_host::outcome::{Failure, Status};
use serde_json::{json, Value};

use super::requests;

const HELP: &str = "\
Drive the browser beside this terminal in Dragent. The person sees every step.

Usage: dragent browser <command> [--pane ID]

  list                        Browser panes: id, address, title, whether shown; the
                              one commands use by default is marked
  open URL [--new]            Open an address (localhost:3000 works) in the browser
                              beside you, or a fresh pane with --new
  status                      Address, title, loading, last download or new tab
  snapshot                    The page as text with numbered elements: [e3] button
                              \"Sign in\", plus open dialogs, alerts and frames
  click REF                   Click an element from a snapshot (e.g. e3)
  hover REF                   Move the pointer over it (menus, tooltips)
  type REF TEXT [--submit]    Replace a field's text (or choose a select option);
                              --submit sends its form
  press KEY                   A key in the focused element: Enter, Escape, Tab,
                              ArrowDown, Control+a…
  scroll [down|up|top|bottom|REF]
                              Scroll the page, or bring an element into view
  wait [--for TEXT] [--gone TEXT] [--selector CSS] [--url PART] [--ref-gone REF]
       [--idle] [--timeout SECONDS]
                              Until every condition holds (default 10 s, at most 60);
                              --idle: loaded, no requests running, nothing changing
  dialog accept [TEXT] | dismiss
                              How to answer the next alert, confirm or prompt
                              (unanswered ones are dismissed)
  eval SCRIPT [--await]       Run JavaScript in the page; --await waits for a promise
  console [--errors] [--clear]
                              What the page logged: errors, failed requests, dialogs
  screenshot [FILE]           Save a PNG of the page (prints where)
  back | forward | reload     Navigate

References come from the latest snapshot and never repeat: one that's gone fails as
stale_ref instead of hitting another element. Wait for what you expect instead of
sleeping. Failures are JSON with status, code and next.";

/// Pages can take a while to load; the app answers once they have.
const WAIT: Duration = Duration::from_secs(45);

pub fn run(args: &[String]) -> Result<String, String> {
    let Some((tool, arguments)) = parse(args)? else {
        return Ok(HELP.to_string());
    };
    call_tool(tool, arguments)
}

/// Run one browser command (`open`, `snapshot`, `click`…) in the open app,
/// for the browser beside the terminal this process runs in.
pub(crate) fn call_tool(tool: &str, mut arguments: Value) -> Result<String, String> {
    // The app saves screenshots; it doesn't share this process's directory.
    let resolved = arguments["path"]
        .as_str()
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty() && path.is_relative())
        .and_then(|path| std::env::current_dir().ok().map(|cwd| cwd.join(path)));
    if let Some(path) = resolved {
        arguments["path"] = json!(path);
    }
    // A wait answers when it's done or out of time: give it that long.
    let wait = match tool {
        "wait" => Duration::from_secs_f64(arguments["timeout"].as_f64().unwrap_or(10.0)) + Duration::from_secs(15),
        _ => WAIT,
    };
    let request = requests::Request::BrowserTool {
        pane: crate::integrations::policy::helper_pane(),
        terminal: std::env::var("DRAGENT_HOST_TERMINAL_ID")
            .ok()
            .filter(|id| !id.is_empty()),
        tool: tool.to_string(),
        arguments,
    };
    // No answer in time: a timeout with the request's id, so the caller can
    // check whether it went through before trying again.
    let response = super::ask_app_waiting(request, wait)?;
    Ok(response
        .data
        .as_ref()
        .and_then(|data| data["output"].as_str())
        .unwrap_or_default()
        .to_string())
}

fn invalid(code: &str, message: impl Into<String>) -> String {
    Failure::new(Status::InvalidInput, code, message)
        .next("`dragent browser help` shows the commands.")
        .encode()
}

/// The tool and its arguments a command asks for; `None` for help.
fn parse(args: &[String]) -> Result<Option<(&'static str, Value)>, String> {
    const SWITCHES: [&str; 9] = [
        "--submit", "--clear", "--new", "--await", "--errors", "--idle", "--json", "--text", "--quiet",
    ];
    const VALUES: [&str; 7] = ["--pane", "--timeout", "--for", "--gone", "--selector", "--url", "--ref-gone"];
    let mut words: Vec<&str> = Vec::new();
    let mut switches: Vec<&str> = Vec::new();
    let mut values: Vec<(&str, String)> = Vec::new();
    let mut rest = args.iter().map(String::as_str);
    while let Some(arg) = rest.next() {
        if SWITCHES.contains(&arg) {
            switches.push(arg);
        } else if VALUES.contains(&arg) {
            let value = rest
                .next()
                .ok_or_else(|| invalid("missing_value", format!("{arg} needs a value")))?;
            values.push((arg, value.to_string()));
        } else {
            words.push(arg);
        }
    }
    let switch = |name: &str| switches.contains(&name);
    let value = |name: &str| {
        values
            .iter()
            .rev()
            .find(|(flag, _)| *flag == name)
            .map(|(_, value)| value.clone())
    };
    let Some((command, rest)) = words.split_first() else {
        return Ok(None);
    };
    let joined = || rest.join(" ");
    let required = |what: &str| -> Result<String, String> {
        let text = joined();
        if text.trim().is_empty() {
            Err(invalid("missing_argument", format!("`dragent browser {command}` needs {what}")))
        } else {
            Ok(text)
        }
    };
    let (tool, mut arguments) = match *command {
        "help" | "--help" | "-h" => return Ok(None),
        "list" | "ls" => ("list", json!({})),
        "open" | "goto" | "navigate" => (
            "open",
            json!({ "url": required("an address")?, "new": switch("--new") }),
        ),
        "status" => ("status", json!({})),
        "snapshot" | "read" => ("snapshot", json!({})),
        "click" => ("click", json!({ "ref": reference(rest.first().copied())? })),
        "hover" => ("hover", json!({ "ref": reference(rest.first().copied())? })),
        "type" | "fill" => {
            let reference = reference(rest.first().copied())?;
            (
                "type",
                json!({ "ref": reference, "text": rest.get(1..).unwrap_or_default().join(" "), "submit": switch("--submit") }),
            )
        }
        "press" => ("press", json!({ "key": required("a key, such as Enter")? })),
        "scroll" => {
            let to = rest.first().copied().unwrap_or("down");
            let to = match to {
                "down" | "up" | "top" | "bottom" => to.to_string(),
                other => reference(Some(other)).map_err(|_| {
                    invalid("invalid_scroll", "scroll takes down, up, top, bottom or an element (e3)")
                })?,
            };
            ("scroll", json!({ "to": to }))
        }
        "wait" => {
            let mut arguments = json!({ "idle": switch("--idle") });
            for (flag, key) in [("--for", "text"), ("--gone", "gone"), ("--selector", "selector"), ("--url", "url")] {
                if let Some(value) = value(flag) {
                    arguments[key] = json!(value);
                }
            }
            if let Some(gone) = value("--ref-gone") {
                arguments["ref_gone"] = json!(reference(Some(&gone))?);
            }
            if let Some(seconds) = value("--timeout") {
                let seconds: f64 = seconds
                    .parse()
                    .map_err(|_| invalid("invalid_timeout", format!("--timeout takes seconds, not “{seconds}”")))?;
                arguments["timeout"] = json!(seconds);
            }
            ("wait", arguments)
        }
        "dialog" => match rest.first().copied() {
            Some("accept") => (
                "dialog",
                json!({ "answer": "accept", "text": (rest.len() > 1).then(|| rest[1..].join(" ")) }),
            ),
            Some("dismiss") => ("dialog", json!({ "answer": "dismiss" })),
            _ => return Err(invalid("missing_argument", "dialog takes accept [TEXT] or dismiss")),
        },
        "eval" | "js" => (
            "eval",
            json!({ "script": required("a script")?, "await": switch("--await") }),
        ),
        "console" | "logs" => (
            "console",
            json!({ "clear": switch("--clear"), "errors": switch("--errors") }),
        ),
        "screenshot" => {
            // The app saves the file; it doesn't share this terminal's directory.
            let path = rest.first().map(|path| {
                let path = std::path::Path::new(path);
                std::env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .unwrap_or_else(|_| path.to_path_buf())
            });
            ("screenshot", json!({ "path": path }))
        }
        "back" => ("back", json!({})),
        "forward" => ("forward", json!({})),
        "reload" => ("reload", json!({})),
        other => {
            return Err(invalid(
                "unknown_browser_command",
                format!("unknown browser command “{other}”. Run `dragent browser help`."),
            ))
        }
    };
    if let Some(pane) = value("--pane") {
        let pane: u64 = pane
            .parse()
            .map_err(|_| invalid("invalid_pane", format!("--pane takes a browser's id from `dragent browser list`, not “{pane}”")))?;
        arguments["pane"] = json!(pane);
    }
    Ok(Some((tool, arguments)))
}

/// `e3`, `[e3]` or `3`, as printed by `snapshot`.
fn reference(word: Option<&str>) -> Result<String, String> {
    let word = word
        .map(|word| word.trim_matches(|c| c == '[' || c == ']'))
        .filter(|word| !word.is_empty())
        .ok_or_else(|| invalid("missing_ref", "name an element from `dragent browser snapshot`, such as e3"))?;
    let number = word.strip_prefix('e').unwrap_or(word);
    if number.chars().all(|c| c.is_ascii_digit()) {
        Ok(format!("e{number}"))
    } else {
        Err(invalid(
            "invalid_ref",
            format!("“{word}” isn't an element reference; use one like e3 from `snapshot`"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn commands_become_tools() {
        assert_eq!(parse(&args(&[])).unwrap(), None);
        assert_eq!(
            parse(&args(&["open", "localhost:3000"])).unwrap(),
            Some(("open", json!({ "url": "localhost:3000", "new": false })))
        );
        assert_eq!(
            parse(&args(&["open", "--new", "localhost:3000", "--pane", "7"])).unwrap(),
            Some(("open", json!({ "url": "localhost:3000", "new": true, "pane": 7 })))
        );
        assert_eq!(
            parse(&args(&["wait", "--for", "Welcome back", "--idle", "--timeout", "5"])).unwrap(),
            Some(("wait", json!({ "text": "Welcome back", "idle": true, "timeout": 5.0 })))
        );
        assert_eq!(
            parse(&args(&["wait", "--ref-gone", "[e4]"])).unwrap(),
            Some(("wait", json!({ "idle": false, "ref_gone": "e4" })))
        );
        assert_eq!(
            parse(&args(&["dialog", "accept", "Ada", "Lovelace"])).unwrap(),
            Some(("dialog", json!({ "answer": "accept", "text": "Ada Lovelace" })))
        );
        assert_eq!(
            parse(&args(&["scroll", "e9"])).unwrap(),
            Some(("scroll", json!({ "to": "e9" })))
        );
        assert_eq!(parse(&args(&["list"])).unwrap(), Some(("list", json!({}))));
        assert_eq!(
            parse(&args(&["click", "[e12]"])).unwrap(),
            Some(("click", json!({ "ref": "e12" })))
        );
        assert_eq!(
            parse(&args(&["type", "e4", "hello", "world", "--submit"])).unwrap(),
            Some((
                "type",
                json!({ "ref": "e4", "text": "hello world", "submit": true })
            ))
        );
        assert_eq!(
            parse(&args(&["eval", "fetch('/health').then(r => r.status)", "--await"])).unwrap(),
            Some(("eval", json!({ "script": "fetch('/health').then(r => r.status)", "await": true })))
        );
        for bad in [
            &["click", "button"][..],
            &["open"][..],
            &["scroll", "sideways"][..],
            &["frobnicate"][..],
            &["wait", "--timeout", "soon"][..],
            &["dialog"][..],
            &["status", "--pane", "x"][..],
        ] {
            let failure = Failure::from_error(&parse(&args(bad)).unwrap_err());
            assert_eq!(failure.status, Status::InvalidInput, "{bad:?}");
        }
    }
}
