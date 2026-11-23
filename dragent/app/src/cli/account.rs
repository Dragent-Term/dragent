//! `dragent org`, `whoami`, `machines` and `feedback`: account things the
//! signed-in app knows (the command line never signs in itself; a second
//! sign-in would rotate the app's credentials).

use super::{ask_app, listing, pretty, requests, text, Flags};
use dragent_host::outcome::{Failure, Status};

pub(super) fn org(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    match flags.subcommand() {
        "list" => {
            let response = ask_app(requests::Request::Organisations, false)?;
            let rows = response
                .data
                .and_then(|data| data.as_array().cloned())
                .unwrap_or_default();
            Ok(listing(
                &flags,
                rows,
                |row| text(row, "id"),
                |row| {
                    format!(
                        "{}{}\t{}\t{}",
                        if row["active"].as_bool() == Some(true) {
                            "* "
                        } else {
                            "  "
                        },
                        text(row, "name"),
                        text(row, "role"),
                        text(row, "id")
                    )
                },
                "You don't belong to any organisation.",
            ))
        }
        "switch" | "use" => {
            let organisation = flags
                .argument(0)
                .ok_or("which organisation? (`dragent org switch NAME`, see `dragent org list`)")?;
            let response = ask_app(
                requests::Request::SwitchOrganisation {
                    organisation: organisation.to_string(),
                },
                false,
            )?;
            if flags.json {
                return Ok(pretty(serde_json::to_value(&response).unwrap_or_default()));
            }
            let name = response
                .data
                .as_ref()
                .map(|data| text(data, "name"))
                .unwrap_or_else(|| organisation.to_string());
            Ok(format!("Switched Dragent to {name}"))
        }
        other => Err(format!(
            "unknown org command “{other}” (use list or switch)"
        )),
    }
}

/// From the app when it's open; otherwise what it last cached.
pub(super) fn whoami(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    let live = super::live_identity();
    if live
        .as_ref()
        .is_some_and(|data| data["signed_in"].as_bool() == Some(false))
    {
        return Ok(if flags.json {
            pretty(serde_json::json!({ "signed_in": false }))
        } else {
            "This terminal's organisation is not signed in. Open that organisation in Dragent."
                .into()
        });
    }
    let data = match ask_app(requests::Request::Whoami, false) {
        Ok(response) => response.data.unwrap_or_default(),
        // The app is closed: what it saved while signed in, only while a
        // sign-in is still saved (the same check as every other command).
        Err(_) if live.is_some() => live.unwrap(),
        // Local mode with the app closed: the demo identity needs no app.
        Err(_) if crate::api::is_local_mode() => crate::api::local_mode::identity_json(),
        Err(error) => {
            match crate::organisation_context::cached_active().filter(|_| super::signed_in()) {
                Some(cached) => serde_json::json!({
                    "user_id": cached.user_id,
                    "organisation": cached.name,
                    "organisation_id": cached.organisation_id,
                    "offline": true,
                    "reason": error,
                }),
                None => serde_json::json!({ "signed_in": false }),
            }
        }
    };
    if flags.json {
        return Ok(pretty(data));
    }
    if data["signed_in"].as_bool() == Some(false) {
        return Ok("Not signed in to Dragent.".into());
    }
    let mut lines = Vec::new();
    if let Some(name) = data["name"].as_str().filter(|name| !name.is_empty()) {
        lines.push(format!("Name: {name}"));
    }
    if let Some(email) = data["email"].as_str() {
        lines.push(format!("Email: {email}"));
    } else {
        lines.push(format!("User: {}", text(&data, "user_id")));
    }
    if let Some(plan) = data["plan"].as_str() {
        lines.push(format!("Plan: {plan}"));
    }
    lines.push(format!("Organisation: {}", text(&data, "organisation")));
    if data["offline"].as_bool() == Some(true) {
        lines.push(format!(
            "(Couldn't ask Dragent: {} This is what it last saved.)",
            text(&data, "reason").trim_end_matches('.').to_string() + "."
        ));
    }
    Ok(lines.join("\n"))
}

/// SSH machines added in Settings → Machines.
pub(super) fn machines(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    let wanted = match flags.subcommand() {
        "list" | "" => None,
        "show" => Some(flags.argument(0).ok_or_else(|| {
            Failure::new(
                Status::InvalidInput,
                "missing_argument",
                "usage: dragent machines show ID",
            )
            .next("`dragent machines list` gives the ids.")
            .encode()
        })?),
        other => {
            return Err(Failure::new(
                Status::InvalidInput,
                "unknown_command",
                format!("unknown machines command “{other}” (use list or show)"),
            )
            .encode())
        }
    };
    let registry = dragent_host::machine_registry::MachineRegistry::load()?;
    // Connection state is the app's: it holds the connections. Asking never
    // connects anything.
    let live: Option<Vec<serde_json::Value>> = super::instance::app_running()
        .then(|| ask_app(requests::Request::Machines, false).ok())
        .flatten()
        .and_then(|response| response.data)
        .and_then(|data| data["machines"].as_array().cloned());
    let rows: Vec<serde_json::Value> = registry
        .machines
        .iter()
        .filter(|machine| wanted.is_none_or(|id| machine.id == id || machine.name() == id))
        .map(|machine| {
            let projects: Vec<serde_json::Value> = registry
                .projects
                .iter()
                .filter(|project| project.machine_id == machine.id)
                .map(|project| {
                    serde_json::json!({
                        "path": project.root,
                        "local_stand_in": crate::remote_fs::alias(&machine.id, &project.root),
                    })
                })
                .collect();
            let state = live
                .as_ref()
                .and_then(|rows| rows.iter().find(|row| row["id"] == machine.id.as_str()))
                .cloned();
            let mut row = serde_json::json!({
                "id": machine.id,
                "name": machine.name(),
                "kind": "ssh",
                "status": state
                    .as_ref()
                    .and_then(|state| state["status"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "unknown".into()),
                "projects": projects,
                "provides": dragent_host::scope::ssh_machine_provides(),
            });
            if let Some(state) = state {
                for key in ["error", "os", "host_version", "agents"] {
                    if !state[key].is_null() {
                        row[key] = state[key].clone();
                    }
                }
            } else {
                row["note"] = serde_json::json!(
                    "Dragent isn't open, so whether it's connected isn't known; the person connects it in Settings → Machines."
                );
            }
            row
        })
        .collect();
    if let Some(id) = wanted {
        let Some(row) = rows.into_iter().next() else {
            return Err(Failure::new(
                Status::InvalidTarget,
                "no_such_machine",
                format!("There's no machine “{id}”."),
            )
            .next("`dragent machines list` gives the ids.")
            .encode());
        };
        return Ok(pretty(row));
    }
    Ok(listing(
        &flags,
        rows,
        |row| text(row, "id"),
        |row| {
            let projects = row["projects"].as_array().map(Vec::len).unwrap_or(0);
            format!(
                "{}\t{}\t{} project{}\t{}",
                text(row, "name"),
                text(row, "status").replace('_', " "),
                projects,
                if projects == 1 { "" } else { "s" },
                text(row, "id")
            )
        },
        "No machines added. Add one in Dragent: Settings → Machines.",
    ))
}

/// Private feedback to the Dragent team, sent by the open app as the
/// signed-in user.
///
/// Where it goes: `POST /api/v1/feedback` from the app, private to the
/// Dragent team — never published publicly. What is transmitted is text
/// only: the reviewable draft below, plus account and organisation added by
/// the app, with the app version and OS. The CLI uploads no files: a local
/// screenshot or log path named in the draft stays a local path, it is not
/// uploaded or attached.
pub(super) fn feedback(args: &[String]) -> Result<String, String> {
    // `--draft` arrives here as a switch (see `SWITCHES`); older parsing
    // treated it as a value flag, so also accept `--draft=...` explicitly.
    let mut is_draft = false;
    let mut cleaned: Vec<String> = Vec::with_capacity(args.len());
    for arg in args {
        if arg == "--draft" {
            is_draft = true;
        } else if let Some(rest) = arg.strip_prefix("--draft=") {
            is_draft = !matches!(rest, "0" | "false" | "no");
        } else {
            cleaned.push(arg.clone());
        }
    }
    let flags = Flags::parse(&cleaned)?;
    if flags.on("draft") {
        is_draft = true;
    }
    if flags.subcommand() == "help"
        || flags
            .positional
            .iter()
            .any(|p| p == "help" && flags.positional.len() == 1)
    {
        return Ok(feedback_help());
    }
    let fields = collect_feedback_fields(&flags)?;
    if fields.is_empty() {
        return Err(Failure::new(
            Status::InvalidInput,
            "feedback_empty",
            "what would you like to tell us? (`dragent feedback --help` for fields)",
        )
        .next("Describe the problem or idea with --observed/--expected, --body, or plain text.")
        .encode());
    }
    let draft = render_feedback_draft(&fields, &feedback_platform(), feedback_build());
    if is_draft {
        // Reviewable draft: exactly what would be sent, without sending.
        if flags.json {
            return Ok(pretty(serde_json::json!({
                "sent": false,
                "draft": draft,
                "destination": feedback_destination(),
                "note": "reviewable draft; nothing was sent (--draft)",
            })));
        }
        return Ok(draft);
    }
    match ask_app(
        requests::Request::Feedback {
            text: draft.clone(),
        },
        false,
    ) {
        Ok(_) => {
            if flags.json {
                return Ok(pretty(serde_json::json!({
                    "sent": true,
                    "destination": feedback_destination(),
                    "kind": fields.kind,
                })));
            }
            Ok(
                "Thanks, your feedback was sent to the Dragent team (private; not published)."
                    .into(),
            )
        }
        Err(error) => {
            let failure = Failure::from_error(&error);
            let draft_path = match save_feedback_draft(&draft) {
                Ok(path) => path.display().to_string(),
                Err(save_error) => format!("(couldn't save the draft: {save_error})"),
            };
            let mut next = format!("Draft kept at {draft_path}. Review it and retry when Dragent is open and signed in.");
            if let Some(previous) = failure.next.as_deref().filter(|n| !n.is_empty()) {
                next.push_str(&format!(" {previous}"));
            }
            let mut out = Failure::new(failure.status, &failure.code, failure.message).next(next);
            if let Some(request_id) = failure.request_id {
                out = out.request(request_id);
            }
            Err(out.encode())
        }
    }
}

/// Where private CLI feedback goes, for drafts, help and results.
fn feedback_destination() -> &'static str {
    "Dragent team (private) via POST /api/v1/feedback from the open app as the signed-in user; text only, no file uploads"
}

/// OS and architecture gathered automatically for the draft.
fn feedback_platform() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Build gathered automatically for the draft.
fn feedback_build() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn feedback_help() -> String {
    format!(
        "Send private feedback to the Dragent team (text only, not published).\n\n\
Usage: dragent feedback [TEXT] [options]\n\n\
  --kind KIND            bug | feature | general (default: general)\n\
  --observed TEXT        What you saw (measured observation)\n\
  --expected TEXT        What you expected or want instead\n\
  --steps TEXT           How to reproduce it\n\
  --frequency TEXT       e.g. always, sometimes, once\n\
  --cause TEXT           Suspected cause (kept separate from observations)\n\
  --evidence TEXT        Supporting detail, redacted (pasted text only)\n\
  --body TEXT            Additional free-form detail\n\
  --body-file FILE       Read additional detail from FILE\n\
  --draft                Print exactly what would be sent, without sending\n\n\
Destination: {dest}.\n\
The app adds account, organisation, app version and OS on send. \
A local screenshot or log path is never uploaded — only text you paste is sent. \
Redact tokens and secrets before sending.",
        dest = feedback_destination()
    )
}

/// Structured feedback fields, kept separate so observations stay distinct
/// from suspected causes.
#[derive(Debug, Default)]
struct FeedbackFields {
    kind: String,
    observed: String,
    expected: String,
    steps: String,
    frequency: String,
    cause: String,
    evidence: String,
    body: String,
}

impl FeedbackFields {
    fn is_empty(&self) -> bool {
        self.observed.trim().is_empty()
            && self.expected.trim().is_empty()
            && self.steps.trim().is_empty()
            && self.frequency.trim().is_empty()
            && self.cause.trim().is_empty()
            && self.evidence.trim().is_empty()
            && self.body.trim().is_empty()
    }
}

fn collect_feedback_fields(flags: &Flags) -> Result<FeedbackFields, String> {
    let kind = flags
        .value("kind")
        .map(str::to_lowercase)
        .unwrap_or_else(|| "general".to_string());
    if !matches!(kind.as_str(), "bug" | "feature" | "general") {
        return Err(Failure::new(
            Status::InvalidInput,
            "feedback_kind",
            format!("--kind must be bug, feature or general, not “{kind}”"),
        )
        .next("Use --kind bug for something broken, feature for a request, general otherwise.")
        .encode());
    }
    let mut body_parts: Vec<String> = Vec::new();
    if !flags.positional.is_empty() {
        // `--text` is a global output switch, so plain words carry the
        // free-form text (the old `dragent feedback TEXT` still works).
        body_parts.push(flags.positional.join(" "));
    }
    if let Some(body) = flags.value("body") {
        body_parts.push(body.to_string());
    }
    if let Some(file) = flags.value("body-file") {
        let content = std::fs::read_to_string(file)
            .map_err(|error| format!("couldn't read {file}: {error}"))?;
        body_parts.push(content);
    }
    // Backwards compatibility: `--text TEXT` with `=` still carries a message.
    if let Some(legacy) = flags.value("text") {
        body_parts.push(legacy.to_string());
    }
    Ok(FeedbackFields {
        kind,
        observed: flags.value("observed").unwrap_or_default().to_string(),
        expected: flags.value("expected").unwrap_or_default().to_string(),
        steps: flags.value("steps").unwrap_or_default().to_string(),
        frequency: flags.value("frequency").unwrap_or_default().to_string(),
        cause: flags.value("cause").unwrap_or_default().to_string(),
        evidence: flags.value("evidence").unwrap_or_default().to_string(),
        body: body_parts
            .into_iter()
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    })
}

/// Exactly what would be sent as `Request::Feedback { text }`. The backend
/// only takes text, so every field stays client-side in this draft text.
fn render_feedback_draft(fields: &FeedbackFields, platform: &str, build: &str) -> String {
    let mut out = vec![format!("Feedback kind: {}", fields.kind)];
    let section = |title: &str, value: &str, out: &mut Vec<String>| {
        if !value.trim().is_empty() {
            out.push(format!("{title}:\n{}", value.trim()));
        }
    };
    section("Observed (what happened)", &fields.observed, &mut out);
    section("Expected (what should happen)", &fields.expected, &mut out);
    section("Steps to reproduce", &fields.steps, &mut out);
    if !fields.frequency.trim().is_empty() {
        out.push(format!("Frequency: {}", fields.frequency.trim()));
    }
    section(
        "Suspected cause (unverified, separate from observations above)",
        &fields.cause,
        &mut out,
    );
    section(
        "Evidence (pasted text only; local paths below were NOT uploaded)",
        &fields.evidence,
        &mut out,
    );
    section("Additional detail", &fields.body, &mut out);
    out.push(format!("App version: {build}\nOS: {platform}"));
    out.push(
        "(Sent with `dragent feedback`; account and organisation are added by the app. \
         Private to the Dragent team; not published.)"
            .to_string(),
    );
    out.join("\n\n")
}

/// Preserve the draft locally when sending fails, and say where it is.
fn save_feedback_draft(draft: &str) -> Result<std::path::PathBuf, String> {
    save_feedback_draft_to(&dragent_paths::home().join("feedback-drafts"), draft)
}

fn save_feedback_draft_to(
    dir: &std::path::Path,
    draft: &str,
) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("couldn't create {}: {error}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("feedback-{stamp}-{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&path, draft)
        .map_err(|error| format!("couldn't write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn draft_renders_every_field_and_keeps_observations_from_causes() {
        let fields = FeedbackFields {
            kind: "bug".into(),
            observed: "The pane shows a spinner forever".into(),
            expected: "The file list appears".into(),
            steps: "1. Open a workspace\n2. Watch the pane".into(),
            frequency: "always".into(),
            cause: "I think the watcher misses the first event".into(),
            evidence: "log excerpt: watcher timeout after 10s".into(),
            body: "Extra context".into(),
        };
        let draft = render_feedback_draft(&fields, "macos aarch64", "0.2.3");
        for expected in [
            "Feedback kind: bug",
            "Observed (what happened):\nThe pane shows a spinner forever",
            "Expected (what should happen):\nThe file list appears",
            "Steps to reproduce:\n1. Open a workspace",
            "Frequency: always",
            "Suspected cause (unverified, separate from observations above):\nI think the watcher",
            "Evidence (pasted text only; local paths below were NOT uploaded):\nlog excerpt",
            "Additional detail:\nExtra context",
            "App version: 0.2.3",
            "OS: macos aarch64",
        ] {
            assert!(
                draft.contains(expected),
                "draft misses {expected:?}\n{draft}"
            );
        }
        // Observations and suspected causes stay in separate sections.
        let observed_at = draft.find("Observed (what happened)").unwrap();
        let cause_at = draft.find("Suspected cause").unwrap();
        assert!(observed_at < cause_at);
        assert!(
            draft.find("watcher timeout").unwrap() != draft.find("I think the watcher").unwrap()
        );
    }

    #[test]
    fn draft_mode_prints_what_would_be_sent_without_sending() {
        // No app is running in unit tests: `Ok` proves nothing was sent,
        // since a real send would fail while the app is closed.
        let out = feedback(&args(&[
            "--draft",
            "--kind",
            "bug",
            "--observed",
            "spinner forever",
            "--expected",
            "file list",
            "--steps",
            "open workspace",
            "--frequency",
            "always",
            "--cause",
            "maybe watcher",
            "--evidence",
            "/tmp/shot.png (local path, not uploaded)",
        ]))
        .unwrap();
        assert!(out.contains("Feedback kind: bug"), "{out}");
        assert!(out.contains("spinner forever"), "{out}");
        assert!(out.contains("App version:"), "{out}");
        assert!(out.contains("OS:"), "{out}");
        // A local screenshot path is never described as uploaded.
        assert!(!out.to_lowercase().contains("uploaded screenshot"), "{out}");
        assert!(
            out.contains("NOT uploaded") || out.contains("not uploaded"),
            "{out}"
        );
    }

    #[test]
    fn body_file_feeds_the_draft() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("body.md");
        std::fs::write(&file, "from the file").unwrap();
        let out = feedback(&args(&[
            "--draft",
            "--kind",
            "feature",
            "--body-file",
            file.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(out.contains("Feedback kind: feature"), "{out}");
        assert!(out.contains("from the file"), "{out}");
    }

    #[test]
    fn bad_kind_is_invalid_input() {
        let error = feedback(&args(&["--draft", "--kind", "urgent"])).unwrap_err();
        let failure = Failure::from_error(&error);
        assert_eq!(
            (failure.status, failure.code.as_str()),
            (Status::InvalidInput, "feedback_kind")
        );
    }

    #[test]
    fn failed_send_keeps_the_draft_and_returns_a_structured_failure() {
        let dir = tempfile::tempdir().unwrap();
        let draft = render_feedback_draft(
            &FeedbackFields {
                kind: "bug".into(),
                observed: "x".into(),
                ..Default::default()
            },
            "macos aarch64",
            "0.2.3",
        );
        let path = save_feedback_draft_to(dir.path(), &draft).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), draft);
        // The same shape `feedback` returns when `ask_app` fails: the
        // original status/code, with the draft location in `next`.
        let original = Failure::new(
            Status::Unavailable,
            "app_not_open",
            "Dragent isn't open. Open it, then run this again.",
        );
        let encoded = Failure::new(original.status, &original.code, original.message.clone())
            .next(format!(
                "Draft kept at {}. Review it and retry.",
                path.display()
            ))
            .encode();
        let decoded = Failure::from_error(&encoded);
        assert_eq!(decoded.status, Status::Unavailable);
        assert_eq!(decoded.code, "app_not_open");
        assert!(decoded.next.unwrap().contains(&path.display().to_string()));
    }

    #[test]
    fn platform_and_build_are_gathered_automatically() {
        let platform = feedback_platform();
        assert!(platform.contains(std::env::consts::OS), "{platform}");
        assert!(platform.contains(std::env::consts::ARCH), "{platform}");
        assert!(!feedback_build().is_empty());
    }
}
