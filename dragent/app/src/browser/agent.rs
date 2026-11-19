//! Agent control of a browser pane: `dragent browser` commands, run in the
//! page the person is looking at. The page side lives in `agent.js`, which
//! every page gets at its start and every command brings along in case it's
//! missing; it answers `{"ok": value}` or `{"error": text, "code": code}`.
//! `snapshot` numbers the elements (`data-dragent-ref`) that `click`, `type`
//! and `hover` use; numbers keep counting for the whole tab, so an old one
//! fails as stale instead of landing on a different element.

use std::path::PathBuf;
use std::time::Duration;

use dragent_host::outcome::{Failure, Status};
use dragent_workspace_model::BrowserUrl;
use serde_json::{json, Value};

/// Answers one command, from any thread.
pub type AgentReply = Box<dyn FnOnce(Result<String, String>) + Send>;

/// The page side, installed with every page.
pub const PAGE_AGENT: &str = include_str!("agent.js");

/// The longest `wait` an agent may ask for.
pub const MAX_WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub enum AgentCommand {
    Open(BrowserUrl),
    Status,
    Snapshot,
    Click(String),
    Hover(String),
    Type {
        reference: String,
        text: String,
        submit: bool,
    },
    Press(String),
    Scroll(String),
    /// With `settle`, a promise the script returns is waited for. An
    /// `inline` script is one expression built into the page call itself
    /// instead of handed to the page's `eval`, so it still runs on pages
    /// whose Content-Security-Policy forbids `eval` (sign-in pages often
    /// do). The desktop's own scripts (the phone's browser view) use it.
    Eval {
        script: String,
        settle: bool,
        inline: bool,
    },
    Console {
        clear: bool,
        errors_only: bool,
    },
    /// The answer for the next dialog the page opens.
    Dialog {
        accept: bool,
        text: Option<String>,
    },
    /// Until every condition holds (checked a few times a second), at most
    /// `timeout`.
    Wait {
        conditions: Value,
        timeout: Duration,
    },
    Screenshot(Option<PathBuf>),
    /// A reduced capture for a phone's live view, saved to this path.
    Picture(PathBuf),
    Back,
    Forward,
    Reload,
}

/// A failure an agent can act on: what happened, its stable code, and what
/// to do next.
pub fn failure(status: Status, code: &str, message: impl Into<String>, next: &str) -> String {
    Failure::new(status, code, message).next(next).encode()
}

impl AgentCommand {
    pub fn parse(tool: &str, arguments: &Value) -> Result<Self, String> {
        let text = |key: &str| arguments[key].as_str().unwrap_or_default().to_string();
        let invalid = |message: &str| {
            failure(
                Status::InvalidInput,
                "invalid_input",
                message,
                "`dragent browser help` shows the commands.",
            )
        };
        let reference = |key: &str| {
            let reference = text(key);
            let valid = reference.len() > 1
                && reference.starts_with('e')
                && reference[1..].chars().all(|c| c.is_ascii_digit());
            if valid {
                Ok(reference)
            } else {
                Err(invalid("Name an element from `dragent browser snapshot`, such as e3."))
            }
        };
        Ok(match tool {
            "open" => Self::Open(BrowserUrl::from_address_or_search(&text("url"))?),
            "status" => Self::Status,
            "snapshot" => Self::Snapshot,
            "click" => Self::Click(reference("ref")?),
            "hover" => Self::Hover(reference("ref")?),
            "type" => Self::Type {
                reference: reference("ref")?,
                text: text("text"),
                submit: arguments["submit"].as_bool().unwrap_or(false),
            },
            "press" => Self::Press(text("key")),
            "scroll" => Self::Scroll(text("to")),
            "eval" => Self::Eval {
                script: text("script"),
                settle: arguments["await"].as_bool().unwrap_or(false),
                inline: arguments["inline"].as_bool().unwrap_or(false),
            },
            "console" => Self::Console {
                clear: arguments["clear"].as_bool().unwrap_or(false),
                errors_only: arguments["errors"].as_bool().unwrap_or(false),
            },
            "dialog" => Self::Dialog {
                accept: match text("answer").as_str() {
                    "accept" => true,
                    "dismiss" => false,
                    _ => return Err(invalid("Say accept or dismiss for the next dialog.")),
                },
                text: arguments["text"].as_str().map(str::to_string),
            },
            "wait" => {
                let mut conditions = json!({});
                for key in ["text", "gone", "selector", "url"] {
                    if let Some(value) = arguments[key].as_str().filter(|value| !value.is_empty()) {
                        conditions[key] = json!(value);
                    }
                }
                if arguments["ref_gone"].is_string() {
                    conditions["ref_gone"] = json!(reference("ref_gone")?);
                }
                if arguments["idle"].as_bool() == Some(true) {
                    conditions["idle"] = json!(true);
                }
                if conditions.as_object().is_none_or(|map| map.is_empty()) {
                    return Err(invalid(
                        "Say what to wait for: --text, --gone, --selector, --url, --ref-gone or --idle.",
                    ));
                }
                let seconds = arguments["timeout"].as_f64().unwrap_or(10.0);
                if !(0.5..=MAX_WAIT.as_secs_f64()).contains(&seconds) {
                    return Err(invalid("--timeout is 0.5 to 60 seconds."));
                }
                Self::Wait {
                    conditions,
                    timeout: Duration::from_secs_f64(seconds),
                }
            }
            "screenshot" => Self::Screenshot(
                arguments["path"]
                    .as_str()
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from),
            ),
            "picture" => Self::Picture(PathBuf::from(
                arguments["path"].as_str().filter(|path| !path.is_empty()).ok_or_else(|| invalid("Say where to save the picture."))?,
            )),
            "back" => Self::Back,
            "forward" => Self::Forward,
            "reload" => Self::Reload,
            other => {
                return Err(failure(
                    Status::InvalidInput,
                    "unknown_command",
                    format!("Unknown browser command “{other}”."),
                    "`dragent browser help` shows the commands.",
                ))
            }
        })
    }

    /// The page script for commands that run in the page. A `wait`'s script
    /// checks its conditions once; an awaited `eval`'s starts the script.
    pub fn script(&self) -> Option<String> {
        let quote = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
        let call = match self {
            Self::Snapshot => "snapshot()".to_string(),
            Self::Click(reference) => format!("click({})", quote(reference)),
            Self::Hover(reference) => format!("hover({})", quote(reference)),
            Self::Type {
                reference,
                text,
                submit,
            } => format!("type({}, {}, {submit})", quote(reference), quote(text)),
            Self::Press(key) => format!("press({})", quote(key)),
            Self::Scroll(to) => format!("scroll({})", quote(to)),
            Self::Eval {
                script,
                settle,
                inline: true,
            } => format!("evaluate(null, {settle}, () => ({script}\n))"),
            Self::Eval { script, settle, .. } => format!("evaluate({}, {settle})", quote(script)),
            Self::Console { clear, errors_only } => format!(
                "readConsole({clear}, {})",
                if *errors_only { r#"["error", "network"]"# } else { "null" }
            ),
            Self::Dialog { accept, text } => format!(
                "dialog({accept}, {})",
                text.as_deref().map(quote).unwrap_or_else(|| "null".into())
            ),
            Self::Wait { conditions, .. } => format!("check({conditions})"),
            _ => return None,
        };
        Some(in_page(&call))
    }
}

/// `call` on the page side, installing it first when the page lacks it.
pub fn in_page(call: &str) -> String {
    format!("{PAGE_AGENT}\n;window.__dragentAgent.{call}")
}

/// The page's answer: its value, or the failure it reported.
fn page_value(raw: &str) -> Result<Value, String> {
    let unreadable = || {
        failure(
            Status::Unavailable,
            "page_not_answering",
            "The page didn't answer. It may be navigating.",
            "`dragent browser status`, then try again.",
        )
    };
    let inner: String = serde_json::from_str(raw).map_err(|_| unreadable())?;
    let value: Value = serde_json::from_str(&inner).map_err(|_| unreadable())?;
    if let Some(error) = value["error"].as_str() {
        let code = value["code"].as_str().unwrap_or("page_error");
        let (status, next) = match code {
            "stale_ref" => (Status::InvalidTarget, "`dragent browser snapshot`, then use its references."),
            "element_disabled" => (Status::InvalidTarget, "Snapshot again to see what the page wants first."),
            "no_such_option" | "invalid_input" => (Status::InvalidInput, "Use one of the values the answer lists."),
            "unsupported" => (Status::Unsupported, "Ask the person to do this step in the browser pane."),
            _ => (Status::Failed, "Read `dragent browser console` for what the page logged."),
        };
        return Err(failure(status, code, error, next));
    }
    Ok(value["ok"].clone())
}

/// What a page script returned, as the command's answer.
pub fn answer(command: &AgentCommand, raw: &str) -> Result<String, String> {
    let value = page_value(raw)?;
    if *command == AgentCommand::Snapshot {
        return Ok(snapshot_text(&value));
    }
    Ok(match value {
        Value::String(text) => text,
        other => other.to_string(),
    })
}

/// Where a repeated check stands.
#[derive(Debug, PartialEq)]
pub enum Progress {
    /// Done: what to answer.
    Met(String),
    /// Not yet: what's still missing.
    Waiting(String),
    /// An awaited script is running; check `settled(id)` from now on.
    Started(u64),
}

/// A `check`, `settled` or awaited `evaluate` answer.
pub fn progress(raw: &str) -> Result<Progress, String> {
    let value = page_value(raw)?;
    if let Some(id) = value["pending"].as_u64() {
        return Ok(Progress::Started(id));
    }
    match value["met"].as_bool() {
        Some(true) => Ok(Progress::Met(value["detail"].as_str().unwrap_or_default().to_string())),
        Some(false) => Ok(Progress::Waiting(value["detail"].as_str().unwrap_or_default().to_string())),
        // An `evaluate` that didn't return a promise: its value at once.
        None => Ok(Progress::Met(match value {
            Value::String(text) => text,
            other => other.to_string(),
        })),
    }
}

fn snapshot_text(value: &Value) -> String {
    let list = |key: &str| {
        value[key]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|item| format!("  {item}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    let mut out = format!(
        "Page: {}\nURL: {}\n",
        value["title"].as_str().unwrap_or_default(),
        value["url"].as_str().unwrap_or_default()
    );
    for (key, heading) in [
        ("dialogs", "Open dialogs"),
        ("alerts", "Alerts and status messages"),
        ("headings", "Headings"),
    ] {
        let items = list(key);
        if !items.is_empty() {
            out.push_str(&format!("\n{heading}:\n{items}\n"));
        }
    }
    let elements = list("elements");
    out.push_str(&if elements.is_empty() {
        "\nNo interactive elements are visible.\n".to_string()
    } else {
        format!("\nElements (pass the reference to click, type or hover):\n{elements}\n")
    });
    if value["truncated"].as_bool() == Some(true) {
        out.push_str("  … more elements below; scroll and snapshot again to see them.\n");
    }
    for (key, heading) in [
        ("frames", "Frames from other sites"),
        ("drawn", "Drawn content"),
    ] {
        let items = list(key);
        if !items.is_empty() {
            out.push_str(&format!("\n{heading}:\n{items}\n"));
        }
    }
    let text = value["text"].as_str().unwrap_or_default().trim();
    if !text.is_empty() {
        out.push_str(&format!("\nVisible text:\n{text}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_inline_script_is_built_into_the_call_not_handed_to_eval() {
        let inline = AgentCommand::parse("eval", &json!({"script": "document.title // why", "inline": true})).unwrap();
        let call = inline.script().unwrap();
        assert!(call.ends_with("evaluate(null, false, () => (document.title // why\n))"), "{call}");
        let text = AgentCommand::parse("eval", &json!({"script": "document.title", "await": true})).unwrap();
        assert!(text.script().unwrap().ends_with(r#"evaluate("document.title", true)"#));
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn commands_parse_and_refuse_bad_references() {
        assert_eq!(
            AgentCommand::parse("open", &json!({"url": "localhost:5173"})).unwrap(),
            AgentCommand::Open(BrowserUrl::parse("http://localhost:5173/").unwrap())
        );
        assert_eq!(
            AgentCommand::parse("click", &json!({"ref": "e12"})).unwrap(),
            AgentCommand::Click("e12".into())
        );
        assert!(AgentCommand::parse("click", &json!({"ref": "e"})).is_err());
        assert!(AgentCommand::parse("click", &json!({"ref": "#login"})).is_err());
        assert!(AgentCommand::parse("open", &json!({"url": "file:///etc/hosts"})).is_err());
        assert!(AgentCommand::parse("teleport", &json!({})).is_err());
        assert_eq!(
            AgentCommand::parse("wait", &json!({"text": "Welcome", "idle": true, "timeout": 5})).unwrap(),
            AgentCommand::Wait {
                conditions: json!({"text": "Welcome", "idle": true}),
                timeout: Duration::from_secs(5),
            }
        );
        for bad in [json!({}), json!({"text": "x", "timeout": 600}), json!({"ref_gone": "#x"})] {
            let refused = Failure::from_error(&AgentCommand::parse("wait", &bad).unwrap_err());
            assert_eq!(refused.status, Status::InvalidInput, "{bad}");
        }
        assert_eq!(
            AgentCommand::parse("dialog", &json!({"answer": "accept", "text": "Ada"})).unwrap(),
            AgentCommand::Dialog { accept: true, text: Some("Ada".into()) }
        );
        assert!(AgentCommand::parse("dialog", &json!({"answer": "maybe"})).is_err());
    }

    #[test]
    fn scripts_embed_arguments_as_literals() {
        let script = AgentCommand::Type {
            reference: "e4".into(),
            text: "\"); alert(1); (\"".into(),
            submit: false,
        }
        .script()
        .unwrap();
        // The text arrives as one JSON string literal, never as code.
        assert!(script.ends_with(r#"type("e4", "\"); alert(1); (\"", false)"#));
        assert!(script.starts_with(PAGE_AGENT));
        assert!(AgentCommand::Status.script().is_none());
    }

    #[test]
    fn answers_unwrap_the_page_result() {
        let raw = |value: Value| serde_json::to_string(&value.to_string()).unwrap();
        assert_eq!(
            answer(
                &AgentCommand::Click("e1".into()),
                &raw(json!({"ok": "Clicked e1."}))
            ),
            Ok("Clicked e1.".into())
        );
        let stale = Failure::from_error(
            &answer(
                &AgentCommand::Click("e9".into()),
                &raw(json!({"error": "gone", "code": "stale_ref"})),
            )
            .unwrap_err(),
        );
        assert_eq!((stale.status, stale.code.as_str(), stale.message.as_str()), (Status::InvalidTarget, "stale_ref", "gone"));
        assert_eq!(progress(&raw(json!({"ok": {"met": false, "detail": "still loading"}}))), Ok(Progress::Waiting("still loading".into())));
        assert_eq!(progress(&raw(json!({"ok": {"pending": 3}}))), Ok(Progress::Started(3)));
        assert_eq!(progress(&raw(json!({"ok": "42"}))), Ok(Progress::Met("42".into())));
        let text = answer(
            &AgentCommand::Snapshot,
            &raw(json!({"ok": {
                "title": "Login", "url": "http://localhost:3000/login",
                "headings": ["h1: Welcome back"],
                "dialogs": [], "alerts": ["Wrong password"], "frames": [], "drawn": [],
                "elements": ["[e1] input[email] \"Email\"", "[e2] button \"Sign in\""],
                "truncated": false, "text": "Welcome back\nEmail"
            }})),
        )
        .unwrap();
        assert!(text.starts_with("Page: Login\nURL: http://localhost:3000/login\n"));
        assert!(text.contains("  [e2] button \"Sign in\""));
        assert!(text.contains("Alerts and status messages:\n  Wrong password"));
        assert!(!text.contains("Open dialogs"), "empty sections are left out");
        assert!(text.contains("Visible text:\nWelcome back"));
        assert!(answer(&AgentCommand::Snapshot, "not json").is_err());
    }
}
