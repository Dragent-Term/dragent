//! The same service command parser runs on both machines.
use crate::team_command_support::Flags;
use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;
pub const HELP: &str = "\
Services connected in Dragent (Settings → Integrations), from the command line.

Usage: dragent integrations <command> [options]

  list                                  Every service: connected or not, and what can be done with it
  describe [SERVICE]                    Accounts, operations with input schemas, identifiers, scope, paging, failure codes (JSON)
  search SERVICE [WORDS]                Find items; without words, recent, open or assigned ones
  show SERVICE ID                       One item in full, with its comments
  comment SERVICE ID (--body TEXT | --body-file FILE)
                                        Comment as the connected account (--body-file - reads stdin)
  status SERVICE ID STATUS              Move it to another status (Done, In Progress, closed, …)
  create SERVICE --title TEXT [--body TEXT | --body-file FILE] [--in PLACE]
                                        File a new item; PLACE is a team, project key or owner/repo
  api SERVICE [METHOD] PATH [--query NAME=VALUE]... [--body TEXT | --body-file FILE]
                                        The service's own API for what the commands above don't cover;
                                        PATH is relative to its API root (see `describe SERVICE`, api.example)

SERVICE is a key from `list` (linear, jira, github, sentry, slack, …). ID is an item's
id as `search` and `show` print it. --project PATH picks the repository for GitHub,
GitLab and Forgejo (default: the current folder). --account SELECTOR uses another
connected account of the service (a selector from `describe`, e.g. personal or team);
without it the default account is used, the same in the CLI, `dragent mcp` and agent tools.

Comments, status changes and new items are posted under the user's own account. In an
automation run they're refused unless the automation allows them.";

pub struct Parsed {
    pub tool: &'static str,
    pub arguments: Value,
    pub root: PathBuf,
    pub quiet: bool,
}
pub fn parse(args: &[String]) -> Result<Option<Parsed>, String> {
    let flags = Flags::parse(args)?;
    let root = flags
        .value("project")
        .map(super::team_command_support::absolute)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let service = || {
        flags
            .argument(0)
            .map(str::to_string)
            .ok_or_else(|| "say which service (see `dragent integrations list`)".to_string())
    };
    let id = || {
        flags
            .argument(1)
            .map(str::to_string)
            .ok_or_else(|| "say which item (its id from `search`)".to_string())
    };
    let (tool, arguments) = match flags.subcommand() {
        "help" => return Ok(None),
        "list" | "services" => ("services", json!({})),
        "describe" => ("describe", json!({"service": flags.argument(0)})),
        "api" => {
            let (method, path) = match (flags.argument(1), flags.argument(2)) {
                (Some(method), Some(path)) if is_method(method) => (method.to_uppercase(), path),
                (Some(path), None) if !is_method(path) => ("GET".to_string(), path),
                (Some(path), Some(_)) if !is_method(path) => return Err("give the method before the path: api SERVICE METHOD PATH".into()),
                _ => return Err("say which path (see `dragent integrations describe SERVICE`, api.example)".into()),
            };
            let mut query = serde_json::Map::new();
            for pair in flags.all("query") {
                let (name, value) = pair.split_once('=').ok_or("--query takes NAME=VALUE")?;
                query.insert(name.to_string(), json!(value));
            }
            (
                "api",
                json!({"service": service()?, "method": method, "path": path, "query": query, "body": body(&flags)?}),
            )
        }
        "search" | "find" => {
            let words = flags.positional.get(2..).unwrap_or_default().join(" ");
            ("search", json!({"service": service()?, "query": words}))
        }
        "show" | "read" | "get" => ("read", json!({"service": service()?, "id": id()?})),
        "comment" => (
            "comment",
            json!({"service": service()?, "id": id()?, "text": body(&flags)?.ok_or("write the comment with --body or --body-file")?}),
        ),
        "status" | "move" => {
            let status = flags.positional.get(3..).unwrap_or_default().join(" ");
            if status.trim().is_empty() {
                return Err("say which status to move it to".into());
            }
            (
                "set_status",
                json!({"service": service()?, "id": id()?, "status": status}),
            )
        }
        "create" | "new" => (
            "create",
            json!({
                "service": service()?,
                "title": flags.value("title").ok_or("give it a --title")?,
                "body": body(&flags)?.unwrap_or_default(),
                "place": flags.value("in"),
            }),
        ),
        other => {
            return Err(format!(
                "unknown command “{other}”. Run `dragent integrations help`."
            ));
        }
    };
    let mut arguments = arguments;
    if let Some(account) = flags.value("account") {
        arguments["account"] = json!(account);
    }
    Ok(Some(Parsed {
        tool,
        arguments,
        root,
        quiet: flags.quiet,
    }))
}
fn is_method(word: &str) -> bool {
    matches!(word.to_uppercase().as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE")
}
fn body(flags: &Flags) -> Result<Option<String>, String> {
    match (flags.value("body"), flags.value("body-file")) {
        (Some(_), Some(_)) => Err("use --body or --body-file, not both".into()),
        (Some(text), None) => Ok(Some(text.to_string())),
        (None, Some("-")) => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("couldn't read stdin: {error}"))?;
            Ok(Some(text))
        }
        (None, Some(file)) => std::fs::read_to_string(super::team_command_support::absolute(file))
            .map(Some)
            .map_err(|error| format!("couldn't read {file}: {error}")),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }
    fn parsed(list: &[&str]) -> Parsed {
        parse(&args(list)).unwrap().unwrap()
    }

    #[test]
    fn describe_takes_an_optional_service() {
        assert_eq!(parsed(&["describe"]).arguments["service"], Value::Null);
        let one = parsed(&["describe", "linear"]);
        assert_eq!((one.tool, one.arguments["service"].as_str()), ("describe", Some("linear")));
    }

    #[test]
    fn the_account_travels_with_every_command() {
        let search = parsed(&["search", "linear", "retry", "uploads", "--account", "team"]);
        assert_eq!(search.arguments["account"], "team");
        assert_eq!(search.arguments["query"], "retry uploads");
        assert_eq!(parsed(&["show", "linear", "ENG-1"]).arguments.get("account"), None);
        assert_eq!(parsed(&["comment", "jira", "P-1", "--body", "x", "--account=personal"]).arguments["account"], "personal");
    }

    #[test]
    fn api_calls_default_to_get_and_carry_query_pairs() {
        let read = parsed(&["api", "gitlab", "/projects/7/issues", "--query", "page=2", "--query", "state=opened"]);
        assert_eq!(read.tool, "api");
        assert_eq!(read.arguments["method"], "GET");
        assert_eq!(read.arguments["path"], "/projects/7/issues");
        assert_eq!(read.arguments["query"], json!({"page": "2", "state": "opened"}));
        let write = parsed(&["api", "linear", "post", "/graphql", "--body", "{\"query\":\"{ viewer { id } }\"}"]);
        assert_eq!(write.arguments["method"], "POST");
        assert!(write.arguments["body"].as_str().unwrap().contains("viewer"));
        assert!(parse(&args(&["api", "gitlab"])).is_err());
        assert!(parse(&args(&["api", "gitlab", "/x", "--query", "nope"])).is_err());
    }

    /// Every `dragent integrations …` line in the skill parses.
    #[test]
    fn the_skills_commands_parse() {
        let mut seen = 0;
        for text in [
            include_str!("../skills/dragent-integrations/SKILL.md"),
            include_str!("../skills/dragent-integrations/references/examples.md"),
        ] {
            for line in text.lines().map(str::trim).filter(|l| l.starts_with("dragent integrations ")) {
                let line = line.split(" #").next().unwrap();
                let mut words: Vec<String> = Vec::new();
                for (index, part) in line.split('"').enumerate() {
                    if index % 2 == 1 {
                        words.push(part.to_string());
                    } else {
                        words.extend(part.split_whitespace().map(str::to_string));
                    }
                }
                let words: Vec<String> = words.into_iter().skip(2).filter(|w| w != "--json").collect();
                match parse(&words) {
                    Ok(_) => seen += 1,
                    // The skill's files are placeholders.
                    Err(error) if error.starts_with("couldn't read") => seen += 1,
                    Err(error) => panic!("`{line}`: {error}"),
                }
            }
        }
        assert!(seen >= 12, "{seen}");
    }
}
