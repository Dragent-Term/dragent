//! `dragent work`: how an agent CLI working on a work item task reaches
//! its team. Only agents Dragent started on a task can use it; the app
//! recognises them by their terminal (see `workspace::cli_workers`).

use std::io::Read as _;
use std::time::Duration;

use serde_json::{json, Value};

use super::{requests, Flags};

const HELP: &str = "\
For an agent working on a Dragent work item task, in its own terminal.

Usage: dragent work <command>

  status                               The task, and what makes it done
  ask QUESTION                         Ask the work item's lead; waits for the answer
  report TEXT                          Tell the lead something that affects the plan
  message ROLE TEXT                    Tell another role something
  handoff ROLE TEXT                    Hand another role something to take over
  knowledge                            What the team has recorded
  note TEXT [--kind KIND]              Record a decision, discovery or limitation
  submit (--summary TEXT | --summary-file FILE)
                                       Finish the task: what you changed, how you checked it
                                       (--summary-file - reads stdin)";

/// How long `ask` waits for the lead before saying the answer will be
/// typed into the session instead (`cli_workers::ASK_WAIT_MS`).
const ASK_WAIT: Duration = Duration::from_secs(85);

pub fn run(args: &[String]) -> Result<String, String> {
    let Some((tool, arguments)) = parse(args)? else {
        return Ok(HELP.to_string());
    };
    let request = requests::Request::WorkTool {
        pane: crate::integrations::policy::helper_pane(),
        terminal: std::env::var("DRAGENT_HOST_TERMINAL_ID")
            .ok()
            .filter(|id| !id.is_empty()),
        tool: tool.to_string(),
        arguments,
    };
    let wait = (tool == "ask").then_some(ASK_WAIT);
    match super::ask_app_within(request, wait)? {
        Some(response) => Ok(text_of(response.data)),
        None => Ok("The lead hasn't answered yet. Its answer will be typed into this session when \
                    it comes: carry on with what doesn't depend on it, or stop and wait."
            .into()),
    }
}

/// The tool and its arguments a command asks for; `None` for help.
fn parse(args: &[String]) -> Result<Option<(&'static str, Value)>, String> {
    let flags = Flags::parse(args)?;
    let words = |from: usize| flags.positional.get(from..).unwrap_or_default().join(" ");
    if flags.positional.is_empty() {
        return Ok(None);
    }
    Ok(Some(match flags.subcommand() {
        "help" => return Ok(None),
        "status" => ("status", json!({})),
        "knowledge" => ("knowledge", json!({})),
        "ask" => ("ask", json!({ "text": words(1) })),
        "report" => ("report", json!({ "text": words(1) })),
        "note" => ("note", json!({ "text": words(1), "kind": flags.value("kind") })),
        "message" => ("message", json!({ "role": flags.argument(0).unwrap_or_default(), "text": words(2) })),
        "handoff" => ("handoff", json!({ "role": flags.argument(0).unwrap_or_default(), "text": words(2) })),
        "submit" | "done" => ("submit", json!({ "summary": summary(&flags, &words(1))? })),
        other => return Err(format!("unknown work command “{other}”. Run `dragent work help`.")),
    }))
}

/// `--summary`, `--summary-file` (`-` for stdin), or the words after `submit`.
fn summary(flags: &Flags, words: &str) -> Result<String, String> {
    match (flags.value("summary"), flags.value("summary-file")) {
        (Some(_), Some(_)) => Err("use --summary or --summary-file, not both".into()),
        (Some(text), None) => Ok(text.to_string()),
        (None, Some("-")) => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("couldn't read stdin: {error}"))?;
            Ok(text)
        }
        (None, Some(file)) => std::fs::read_to_string(super::absolute(file))
            .map_err(|error| format!("couldn't read {file}: {error}")),
        (None, None) => Ok(words.to_string()),
    }
}

fn text_of(data: Option<Value>) -> String {
    data.as_ref()
        .and_then(|data| data["output"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn commands_become_the_tools_the_app_runs() {
        assert!(parse(&args(&["help"])).unwrap().is_none());
        assert!(parse(&args(&[])).unwrap().is_none());
        let (tool, arguments) = parse(&args(&["ask", "Which", "API", "version?"])).unwrap().unwrap();
        assert_eq!((tool, arguments["text"].as_str()), ("ask", Some("Which API version?")));
        let (tool, arguments) = parse(&args(&["handoff", "Frontend", "the", "endpoint", "is", "ready"]))
            .unwrap()
            .unwrap();
        assert_eq!(tool, "handoff");
        assert_eq!(arguments["role"], "Frontend");
        assert_eq!(arguments["text"], "the endpoint is ready");
        let (tool, arguments) = parse(&args(&["submit", "--summary", "Added /v2/users; tests pass"]))
            .unwrap()
            .unwrap();
        assert_eq!((tool, arguments["summary"].as_str()), ("submit", Some("Added /v2/users; tests pass")));
        let (_, arguments) = parse(&args(&["note", "Use", "v2", "--kind", "decision"])).unwrap().unwrap();
        assert_eq!((arguments["text"].as_str(), arguments["kind"].as_str()), (Some("Use v2"), Some("decision")));
        assert!(parse(&args(&["submit", "--summary", "a", "--summary-file", "b"])).is_err());
        assert!(parse(&args(&["fly"])).is_err());
    }
}
