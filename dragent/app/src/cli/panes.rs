//! `dragent panes`: the tabs and panes of the workspace this terminal is in —
//! list them, open terminals and browsers, rearrange the split, show or close
//! one — the way the person would with the mouse, while they watch.

use dragent_host::outcome::{Failure, Status};
use serde_json::{json, Value};

use super::requests;

const HELP: &str = "\
Arrange the tabs and panes of the workspace this terminal is in. The person sees every change.

Usage: dragent panes <command>

  list                          Workspace tabs, their panes and split layout, as JSON (yours is
                                marked \"you\"; \"visible\" ones are on screen)
  open [--browser] [--tab|--right|--left|--down|--up] [--target ID] [--url URL]
       [--label NAME] [--focus] [COMMAND…]
                                A NEW terminal or browser. COMMAND defaults to a split on the
                                right of your pane (or --target), keeping your session visible.
                                Without COMMAND, opens a sidebar tab; --tab explicitly requests one
  move ID [--target ID] [--right|--left|--down|--up] [--focus]
                                Show an existing tab's session beside a pane on screen, or move a
                                pane within the split; nothing restarts
  unsplit ID                    Take a pane out of the split into its own tab (it keeps running)
  swap ID --with ID             Exchange two panes' places in the split
  resize ID --share N           Give a pane N (0.1–0.9) of the split it's in
  equalize                      Even out every split on screen
  focus ID                      Show a pane and give it the keyboard
  close ID                      Close a pane, ending its session (the person confirms when
                                something runs there; never your own pane)

Without --focus the keyboard stays where the person has it. Layout changes act on panes that
are on screen; any other target is refused rather than applied elsewhere. Results are JSON:
the affected pane ids, where the keyboard is, and the updated layout.";

pub fn run(args: &[String]) -> Result<String, String> {
    let Some((tool, arguments)) = parse(args)? else {
        return Ok(HELP.to_string());
    };
    call_tool(tool, arguments)
}

/// Run one panes command in the open app for the terminal this process runs in.
pub(crate) fn call_tool(tool: &str, arguments: Value) -> Result<String, String> {
    let request = requests::Request::PaneTool {
        pane: crate::integrations::policy::helper_pane(),
        terminal: std::env::var("DRAGENT_HOST_TERMINAL_ID")
            .ok()
            .filter(|id| !id.is_empty()),
        tool: tool.to_string(),
        arguments,
    };
    let response = super::ask_app(request, false)?;
    Ok(response
        .data
        .as_ref()
        .and_then(|data| data["output"].as_str())
        .unwrap_or_default()
        .to_string())
}

fn invalid(code: &str, message: impl Into<String>) -> String {
    Failure::new(Status::InvalidInput, code, message)
        .next("`dragent panes help` shows the commands.")
        .encode()
}

fn pane_id(word: &str) -> Result<u64, String> {
    word.parse::<u64>().map_err(|_| {
        invalid(
            "invalid_pane_id",
            format!("“{word}” isn't a pane id; `dragent panes list` shows them"),
        )
    })
}

/// The command and its arguments; `None` for help.
fn parse(args: &[String]) -> Result<Option<(&'static str, Value)>, String> {
    let Some((command, rest)) = args.split_first() else {
        return Ok(None);
    };
    let mut arguments = json!({});
    let mut words: Vec<String> = Vec::new();
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        let mut value = |name: &str| -> Result<String, String> {
            rest.next()
                .cloned()
                .ok_or_else(|| invalid("missing_value", format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--browser" => arguments["kind"] = json!("browser"),
            "--terminal" => arguments["kind"] = json!("terminal"),
            "--right" => arguments["place"] = json!("right"),
            "--left" => arguments["place"] = json!("left"),
            "--down" | "--below" => arguments["place"] = json!("down"),
            "--up" | "--above" => arguments["place"] = json!("up"),
            "--tab" => arguments["place"] = json!("tab"),
            "--focus" => arguments["focus"] = json!(true),
            "--url" => {
                arguments["url"] = json!(value("--url")?);
                arguments["kind"] = json!("browser");
            }
            "--label" => arguments["label"] = json!(value("--label")?),
            "--target" | "--beside" => arguments["target"] = json!(pane_id(&value("--target")?)?),
            "--with" => arguments["with"] = json!(pane_id(&value("--with")?)?),
            "--share" => {
                let share = value("--share")?;
                let share: f64 = share
                    .parse()
                    .map_err(|_| invalid("invalid_share", format!("--share is a number from 0.1 to 0.9, not “{share}”")))?;
                arguments["share"] = json!(share);
            }
            // Output switches the dispatcher already applied.
            "--json" | "--text" | "--quiet" => {}
            _ => words.push(arg.clone()),
        }
    }
    let first_id = |words: &[String]| -> Result<Value, String> {
        let word = words.first().ok_or_else(|| {
            invalid("missing_id", "name a pane by its id; `dragent panes list` shows them")
        })?;
        Ok(json!(pane_id(word)?))
    };
    let with_id = |mut arguments: Value, words: &[String]| -> Result<Value, String> {
        arguments["id"] = first_id(words)?;
        Ok(arguments)
    };
    Ok(Some(match command.as_str() {
        "help" | "--help" | "-h" => return Ok(None),
        "list" | "ls" => ("list", json!({})),
        "open" | "new" | "split" => {
            if arguments["kind"].is_null() {
                arguments["kind"] = json!("terminal");
            }
            let command = words.join(" ");
            if arguments["kind"] == "browser" && arguments["url"].is_null() && !command.is_empty() {
                // `dragent panes open --browser localhost:3000`
                arguments["url"] = json!(command);
            } else if !command.is_empty() {
                arguments["command"] = json!(command);
            }
            arguments["place"] = json!(super::app_tools::pane_open_place(&arguments));
            ("open", arguments)
        }
        "move" | "show" => {
            if arguments["place"].is_null() {
                arguments["place"] = json!("right");
            }
            ("move", with_id(arguments, &words)?)
        }
        "unsplit" | "detach" => ("unsplit", with_id(arguments, &words)?),
        "swap" => ("swap", with_id(arguments, &words)?),
        "resize" => ("resize", with_id(arguments, &words)?),
        "equalize" | "even" => ("equalize", json!({})),
        "focus" => ("focus", with_id(json!({}), &words)?),
        "close" => ("close", with_id(json!({}), &words)?),
        other => {
            return Err(invalid(
                "unknown_panes_command",
                format!("unknown panes command “{other}”. Run `dragent panes help`."),
            ))
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> (&'static str, Value) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        parse(&args).unwrap().unwrap()
    }

    fn refused(args: &[&str]) -> Failure {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        Failure::from_error(&parse(&args).unwrap_err())
    }

    #[test]
    fn open_reads_like_a_person_types_it() {
        assert_eq!(
            parsed(&["open", "--right", "npm", "run", "dev"]),
            ("open", json!({ "kind": "terminal", "place": "right", "command": "npm run dev" }))
        );
        assert_eq!(
            parsed(&["open", "--browser", "--down", "localhost:3000"]),
            ("open", json!({ "kind": "browser", "place": "down", "url": "localhost:3000" }))
        );
        assert_eq!(
            parsed(&["open", "--url", "https://example.com"]),
            ("open", json!({ "kind": "browser", "place": "tab", "url": "https://example.com" }))
        );
        assert_eq!(parsed(&["open"]), ("open", json!({ "kind": "terminal", "place": "tab" })));
        assert_eq!(
            parsed(&["open", "--left", "--target", "4", "--label", "tests", "--focus", "cargo", "test"]),
            (
                "open",
                json!({ "kind": "terminal", "place": "left", "target": 4, "label": "tests", "focus": true, "command": "cargo test" })
            )
        );
        assert_eq!(parsed(&["split", "--up"]).1["place"], "up");
    }

    #[test]
    fn commands_open_beside_the_caller_unless_a_place_is_explicit() {
        assert_eq!(
            parsed(&["open", "--label", "dev server", "npm", "run", "dev"]),
            ("open", json!({
                "kind": "terminal", "place": "right", "label": "dev server",
                "command": "npm run dev",
            }))
        );
        assert_eq!(parsed(&["open", "--tab", "cargo", "test"]).1["place"], "tab");
        assert_eq!(parsed(&["open", "--down", "cargo", "test"]).1["place"], "down");
        assert_eq!(parsed(&["open", "--target", "7", "cargo", "test"]).1,
            json!({"kind": "terminal", "place": "right", "target": 7, "command": "cargo test"}));
        assert_eq!(parsed(&["open", "--browser", "localhost:3000"]).1["place"], "tab");
    }

    #[test]
    fn rearranging_takes_ids_and_places() {
        assert_eq!(
            parsed(&["move", "5", "--target", "2", "--down"]),
            ("move", json!({ "id": 5, "target": 2, "place": "down" }))
        );
        assert_eq!(parsed(&["show", "5"]), ("move", json!({ "id": 5, "place": "right" })));
        assert_eq!(parsed(&["unsplit", "5"]), ("unsplit", json!({ "id": 5 })));
        assert_eq!(parsed(&["swap", "5", "--with", "6"]), ("swap", json!({ "id": 5, "with": 6 })));
        assert_eq!(parsed(&["resize", "5", "--share", "0.4"]), ("resize", json!({ "id": 5, "share": 0.4 })));
        assert_eq!(parsed(&["equalize"]), ("equalize", json!({})));
        assert_eq!(parsed(&["focus", "7", "--json"]), ("focus", json!({ "id": 7 })));
        assert_eq!(parsed(&["close", "3"]), ("close", json!({ "id": 3 })));
    }

    #[test]
    fn mistakes_are_invalid_input() {
        for args in [
            &["close", "x"][..],
            &["close"][..],
            &["swap", "5", "--with"][..],
            &["resize", "5", "--share", "big"][..],
            &["teleport"][..],
        ] {
            let failure = refused(args);
            assert_eq!(failure.status, Status::InvalidInput, "{args:?}");
            assert!(failure.retry_safe);
        }
        assert!(parse(&[]).unwrap().is_none());
    }
}
