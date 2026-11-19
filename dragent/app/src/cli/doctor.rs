//! `dragent doctor`: layer-by-layer diagnostics that find the failing layer.
//!
//! Each check is independent: one failing check never stops the others, and
//! the report's overall `status` is `ok` (every check passed or was skipped)
//! or `partial` (at least one check failed, listed in `failing`).
//!
//! Strictly read-only: no restarts, no reinstalling, no deleting data, no
//! clearing credentials, no resetting permissions, no starting the app, and
//! no writing files or databases. Checks that need the app first confirm it
//! is already running through files that already exist; with a fresh home
//! (no app, no data) every app-dependent check fails closed without
//! creating anything.
//!
//! Evidence is bounded and redacted: versions, paths under the user's home
//! shortened to `~`, operation/request ids, timestamps, the specific failing
//! layer. Never tokens, keys, cookies, environment variable values or file
//! contents.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use dragent_host::outcome::{Failure, Status};

const HELP: &str = "\
Diagnose Dragent layer by layer, for agents and people.

Usage: dragent doctor [options]

  --check ID[,ID]      Only these checks (see below); unknown ids fail with usage
  --project PATH       Scope workspace-related checks to this project
  --json               Machine-readable JSON (default for agents)
  --text               Human-readable text
  --quiet              Only failing check ids, one per line

Checks (each independent; one failing never stops the others):

  launcher      The `dragent` on PATH / in ~/.dragent/bin exists, points at
                this build, and isn't stale
  app           The app is open and answering requests; CLI/app version match
  signin        Signed in, which organisation is active, whether this terminal
                belongs to a Dragent workspace (never prints tokens)
  host          The host process is reachable
  agents        Installed agent CLIs and provider accounts
  mcp           `dragent mcp` can list its tools
  skills        Bundled skills materialized; installed copies current
  desktop       Desktop control permissions (skipped where unsupported)
  browser       The browser pane control is reachable
  automations   Scheduler state and last run errors
  hangs         Recent freeze reports (path and timestamp only)

The report's `status` is `ok`, or `partial` with the failing checks listed.
Read-only: changes nothing, starts nothing, writes no files or databases.
";

/// Every check doctor knows, as (id, what it covers). `--check` validates
/// against this list.
const CHECKS: &[(&str, &str)] = &[
    (
        "launcher",
        "The dragent launcher exists and points at this build",
    ),
    ("app", "The app is open and answering requests"),
    ("signin", "Sign-in, organisation and workspace terminal"),
    ("host", "The host process is reachable"),
    ("agents", "Installed agent CLIs and provider accounts"),
    ("mcp", "dragent mcp can list its tools"),
    ("skills", "Bundled and installed skills"),
    ("desktop", "Desktop control permissions"),
    ("browser", "The browser pane control is reachable"),
    ("automations", "Scheduler state and last run errors"),
    ("hangs", "Recent freeze reports"),
    ("machines", "This computer, and remote machines as the app sees them"),
];

/// One check's result. Statuses: `ok`, `warn`, `fail`, `skipped`.
#[derive(Clone, Debug)]
struct Check {
    id: &'static str,
    layer: &'static str,
    status: &'static str,
    code: &'static str,
    summary: String,
    evidence: Value,
    next: String,
}

impl Check {
    fn entry(&self) -> Value {
        json!({
            "id": self.id,
            "layer": self.layer,
            "status": self.status,
            "code": self.code,
            "summary": self.summary,
            "evidence": self.evidence,
            "next": self.next,
        })
    }
}

fn ok(id: &'static str, code: &'static str, summary: String, evidence: Value, next: &str) -> Check {
    Check {
        id,
        layer: id,
        status: "ok",
        code,
        summary,
        evidence: clean(evidence),
        next: next.to_string(),
    }
}

fn warn(
    id: &'static str,
    code: &'static str,
    summary: String,
    evidence: Value,
    next: &str,
) -> Check {
    Check {
        id,
        layer: id,
        status: "warn",
        code,
        summary,
        evidence: clean(evidence),
        next: next.to_string(),
    }
}

fn fail(
    id: &'static str,
    code: &'static str,
    summary: String,
    evidence: Value,
    next: &str,
) -> Check {
    Check {
        id,
        layer: id,
        status: "fail",
        code,
        summary,
        evidence: clean(evidence),
        next: next.to_string(),
    }
}

fn skipped(id: &'static str, code: &'static str, summary: String, next: &str) -> Check {
    Check {
        id,
        layer: id,
        status: "skipped",
        code,
        summary,
        evidence: json!({}),
        next: next.to_string(),
    }
}

#[derive(Clone, Debug, Default)]
struct Opts {
    checks: Option<Vec<String>>,
    project: Option<PathBuf>,
    json: bool,
    text: bool,
    quiet: bool,
    help: bool,
}

fn agent_environment() -> bool {
    let set = |name: &str| {
        std::env::var_os(name)
            .is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    };
    [
        "CI",
        "CLAUDECODE",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "GEMINI_CLI",
        "QWEN_CODE",
        "DRAGENT_JSON",
    ]
    .iter()
    .any(|name| set(name))
}

fn absolute(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    path.canonicalize().unwrap_or(path)
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut opts = Opts::default();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--json" {
            opts.json = true;
        } else if arg == "--text" {
            opts.text = true;
        } else if arg == "--quiet" {
            opts.quiet = true;
        } else if arg == "--help" || arg == "-h" || arg == "help" {
            opts.help = true;
        } else if arg == "--check" || arg == "--project" {
            let name = arg.trim_start_matches("--");
            let value = iter.next().ok_or_else(|| {
                Failure::new(
                    Status::InvalidInput,
                    "usage",
                    format!("--{name} needs a value"),
                )
                .next("See `dragent doctor --help`.")
                .encode()
            })?;
            if name == "check" {
                opts.checks = Some(
                    value
                        .split(',')
                        .map(|part| part.trim().to_string())
                        .filter(|part| !part.is_empty())
                        .collect(),
                );
            } else {
                opts.project = Some(absolute(value));
            }
        } else if let Some(name) = arg.strip_prefix("--") {
            if let Some((name, value)) = name.split_once('=') {
                match name {
                    "check" => {
                        opts.checks = Some(
                            value
                                .split(',')
                                .map(|part| part.trim().to_string())
                                .filter(|part| !part.is_empty())
                                .collect(),
                        );
                    }
                    "project" => opts.project = Some(absolute(value)),
                    _ => {
                        return Err(Failure::new(
                            Status::InvalidInput,
                            "usage",
                            format!("unknown option `--{name}` (see `dragent doctor --help`)"),
                        )
                        .next("See `dragent doctor --help`.")
                        .encode())
                    }
                }
            } else {
                return Err(Failure::new(
                    Status::InvalidInput,
                    "usage",
                    format!("unknown option `--{name}` (see `dragent doctor --help`)"),
                )
                .next("See `dragent doctor --help`.")
                .encode());
            }
        } else {
            return Err(Failure::new(
                Status::InvalidInput,
                "usage",
                format!("unknown argument `{arg}` (see `dragent doctor --help`)"),
            )
            .next("See `dragent doctor --help`.")
            .encode());
        }
    }
    if opts.json && opts.text {
        return Err(Failure::new(
            Status::InvalidInput,
            "usage",
            "use --json or --text, not both",
        )
        .encode());
    }
    if let Some(wanted) = &opts.checks {
        if wanted.is_empty() {
            return Err(Failure::new(
                Status::InvalidInput,
                "usage",
                "--check needs at least one check id",
            )
            .next(format!(
                "Checks: {}",
                CHECKS
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .encode());
        }
        for id in wanted {
            if !CHECKS.iter().any(|(known, _)| known == id) {
                return Err(Failure::new(
                    Status::InvalidInput,
                    "usage",
                    format!("unknown check `{id}`"),
                )
                .next(format!(
                    "Checks: {}",
                    CHECKS
                        .iter()
                        .map(|(id, _)| *id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .encode());
            }
        }
    }
    Ok(opts)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── Redaction ────────────────────────────────────────────────────────────────

/// Shorten a path for evidence: under the user's home becomes `~/…`.
fn shorten(path: &str) -> String {
    for home in [dragent_paths::home(), dirs::home_dir().unwrap_or_default()] {
        let home = home.display().to_string();
        if !home.is_empty()
            && home != "."
            && (path == home || path.starts_with(&format!("{home}/")))
        {
            return format!("~{}", &path[home.len()..]);
        }
    }
    path.to_string()
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Mask a token-like value inside free text. UUIDs and other operation ids
/// pass through; secrets (known prefixes, `name=value` secrets) do not.
fn redact_text(text: &str) -> String {
    let mut out = text.to_string();
    // Known secret prefixes and bearer/JWT shapes.
    for prefix in [
        "sk-", "sk-ant-", "ghp_", "gho_", "xoxb-", "xoxp-", "xoxa-", "xoxs-", "AKIA",
    ] {
        out = mask_after(&out, prefix);
    }
    out = mask_bearer(&out);
    out = mask_jwt(&out);
    out = mask_named_secrets(&out);
    // Home paths shorten to `~`.
    for home in [dragent_paths::home(), dirs::home_dir().unwrap_or_default()] {
        let home = home.display().to_string();
        if !home.is_empty() && home != "." {
            out = out.replace(home.as_str(), "~");
        }
    }
    out
}

/// Replace the secret-looking token starting at each `prefix` with `[redacted]`.
fn mask_after(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(prefix) {
        out.push_str(&rest[..pos]);
        let token: String = rest[pos..]
            .chars()
            .take_while(|ch| {
                !ch.is_whitespace() && !matches!(ch, '"' | '\'' | ',' | ';' | ')' | ']')
            })
            .collect();
        if token.len() > prefix.len() + 4 {
            out.push_str("[redacted]");
        } else {
            out.push_str(&token);
        }
        rest = &rest[pos + token.len()..];
    }
    out.push_str(rest);
    out
}

fn mask_bearer(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let lower = rest.to_lowercase();
        let Some(pos) = lower.find("bearer ") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..pos + 7]);
        let after = &rest[pos + 7..];
        let token: String = after
            .chars()
            .take_while(|ch| !ch.is_whitespace() && !matches!(ch, '"' | '\'' | ',' | ';'))
            .collect();
        if token.len() >= 8 {
            out.push_str("[redacted]");
        } else {
            out.push_str(&token);
        }
        rest = &after[token.len()..];
    }
    out
}

fn mask_jwt(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("eyJ") {
        // A JWT is three base64url parts; only mask plausible ones.
        let token: String = rest[pos..]
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            .collect();
        let dots = token.chars().filter(|ch| *ch == '.').count();
        out.push_str(&rest[..pos]);
        if dots == 2 && token.len() > 24 {
            out.push_str("[redacted]");
        } else {
            out.push_str(&token);
        }
        rest = &rest[pos + token.len()..];
    }
    out.push_str(rest);
    out
}

/// `token=VALUE`, `api_key: "VALUE"` and friends: keep the name, drop the value.
fn mask_named_secrets(text: &str) -> String {
    const NAMES: &[&str] = &[
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "auth",
        "cookie",
        "session",
        "credential",
        "private_key",
    ];
    let mut out = text.to_string();
    for name in NAMES {
        for sep in ["=", ":"] {
            let needle = format!("{name}{sep}");
            let mut rebuilt = String::with_capacity(out.len());
            let mut rest = out.as_str();
            loop {
                let lower = rest.to_lowercase();
                let Some(pos) = lower.find(needle.as_str()) else {
                    rebuilt.push_str(rest);
                    break;
                };
                rebuilt.push_str(&rest[..pos + needle.len()]);
                let after = rest[pos + needle.len()..].trim_start_matches([' ', '"', '\'']);
                let skipped = rest[pos + needle.len()..].len() - after.len();
                // Preserve the opening quote/space already skipped minimally.
                let _ = skipped;
                let value: String = after
                    .chars()
                    .take_while(|ch| !ch.is_whitespace() && !matches!(ch, '"' | '\'' | ',' | ';'))
                    .collect();
                if value.len() >= 4 {
                    rebuilt.push_str("[redacted]");
                } else {
                    rebuilt.push_str(&value);
                }
                rest = &after[value.len()..];
            }
            out = rebuilt;
        }
    }
    out
}

/// Redact every string inside evidence (objects, arrays, strings).
fn clean(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_text(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(clean).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, clean(v))).collect()),
        other => other,
    }
}

// ── Read-only app probe ──────────────────────────────────────────────────────

/// Whether the app is running, without creating anything: `app.lock` is only
/// opened when it already exists (opening it with creation would plant a
/// file in a fresh home).
fn app_running_readonly() -> bool {
    if !dragent_paths::home().join("app.lock").exists() {
        return false;
    }
    super::instance::app_running()
}

// ── Checks ───────────────────────────────────────────────────────────────────

/// The launcher script's target executable, without running anything.
fn launcher_target(script: &str) -> Option<String> {
    let pos = script.find("--dragent-cli")?;
    quoted(&script[..pos]).into_iter().next()
}

/// Single- and double-quoted chunks, in order.
fn quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' || ch == '"' {
            let quote = ch;
            let mut chunk = String::new();
            let mut closed = false;
            while let Some(next) = chars.next() {
                if next == '\\' && quote == '\'' {
                    // POSIX: '\'' ends, escapes, and reopens. Keep it simple:
                    // a backslash before a quote closes handling below.
                    if chars.peek() == Some(&'\'') {
                        chunk.push('\'');
                        chars.next();
                        continue;
                    }
                    chunk.push(next);
                    continue;
                }
                if next == quote {
                    closed = true;
                    break;
                }
                chunk.push(next);
            }
            if closed && !chunk.is_empty() {
                out.push(chunk);
            }
        }
    }
    out
}

fn launcher_paths() -> Vec<PathBuf> {
    let name = if cfg!(windows) {
        "dragent.cmd"
    } else {
        "dragent"
    };
    let mut paths = vec![
        super::user_bin_dir().join(name),
        dragent_paths::organisation_dir().join("bin").join(name),
    ];
    if cfg!(windows) {
        paths.push(
            dragent_paths::organisation_dir()
                .join("bin")
                .join(super::POSIX_SUBDIR)
                .join("dragent"),
        );
    }
    paths
}

/// `dragent` as the shell would find it on PATH.
fn on_path() -> Vec<PathBuf> {
    let name = if cfg!(windows) {
        "dragent.cmd"
    } else {
        "dragent"
    };
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() && !found.contains(&candidate) {
            found.push(candidate);
        }
        if found.len() >= 5 {
            break;
        }
    }
    found
}

/// This binary's version, read from the launcher target without side effects.
fn version_of(exe: &Path) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = exe.to_path_buf();
    std::thread::spawn(move || {
        let output = std::process::Command::new(&exe)
            .arg("--dragent-cli")
            .arg("version")
            .output();
        let _ = tx.send(output);
    });
    let output = rx.recv_timeout(Duration::from_secs(10)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.trim().strip_prefix("dragent ");
    version.map(str::to_string)
}

fn check_launcher() -> Check {
    let cli_version = env!("CARGO_PKG_VERSION").to_string();
    let running = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok());
    let mut seen: Vec<Value> = Vec::new();
    let mut current = false;
    let mut stale: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for path in launcher_paths().into_iter().chain(on_path()) {
        if seen
            .iter()
            .any(|entry| entry["path"].as_str() == Some(&shorten(&path.display().to_string())))
        {
            continue;
        }
        if !path.is_file() {
            if launcher_paths().contains(&path) {
                missing.push(shorten(&path.display().to_string()));
                seen.push(
                    json!({ "path": shorten(&path.display().to_string()), "state": "missing" }),
                );
            }
            continue;
        }
        let script = std::fs::read_to_string(&path).unwrap_or_default();
        let target = launcher_target(&script).map(PathBuf::from);
        let target_current = target
            .as_ref()
            .and_then(|target| target.canonicalize().ok())
            == running;
        let target_version = target.as_ref().and_then(|target| version_of(target));
        if target_current {
            current = true;
        } else if let (Some(target), Some(version)) = (&target, &target_version) {
            if version != &cli_version {
                stale.push(format!(
                    "{} (version {version}, this build {cli_version})",
                    shorten(&target.display().to_string())
                ));
            } else {
                stale.push(format!(
                    "{} (a different copy of {version})",
                    shorten(&target.display().to_string())
                ));
            }
        }
        seen.push(json!({
            "path": shorten(&path.display().to_string()),
            "target": target.as_ref().map(|target| shorten(&target.display().to_string())),
            "target_version": target_version,
            "current": target_current,
        }));
    }
    let evidence = json!({
        "cli_version": cli_version,
        "launchers": seen,
    });
    if current && stale.is_empty() {
        ok(
            "launcher",
            "launcher_current",
            "The launcher points at this build.".into(),
            evidence,
            "Nothing to do.",
        )
    } else if current {
        warn(
            "launcher",
            "launcher_shadowed",
            "A launcher points at this build, but another copy shadows it somewhere.".into(),
            evidence,
            "Run `dragent` with its full path (`~/.dragent/bin/dragent`) or remove the shadowing copy; rerun `dragent doctor --check launcher`.",
        )
    } else if seen
        .iter()
        .any(|entry| entry["state"].as_str() != Some("missing"))
    {
        fail(
            "launcher",
            "launcher_stale",
            format!("No launcher points at this build ({}).", stale.first().cloned().unwrap_or_else(|| "targets differ".into())),
            evidence,
            "Open the Dragent app once so it writes its launchers, then rerun `dragent doctor --check launcher`.",
        )
    } else {
        let _ = missing;
        fail(
            "launcher",
            "launcher_missing",
            "No dragent launcher is installed.".into(),
            evidence,
            "Open the Dragent app once so it writes its launchers (`~/.dragent/bin/dragent`), then rerun `dragent doctor --check launcher`.",
        )
    }
}

fn check_app() -> Check {
    let cli_version = env!("CARGO_PKG_VERSION").to_string();
    if !app_running_readonly() {
        return fail(
            "app",
            "app_not_open",
            "Dragent isn't open, so no request could be answered.".into(),
            json!({ "running": false, "cli_version": cli_version }),
            "`dragent open PATH` starts it, if the failing path needs the app; rerun `dragent doctor --check app`.",
        );
    }
    match super::ask_app_within(super::requests::Request::Whoami, Some(Duration::from_secs(5))) {
        Ok(Some(_)) => ok(
            "app",
            "app_answering",
            format!("The app is open and answered a request (CLI {cli_version})."),
            json!({ "running": true, "answering": true, "cli_version": cli_version }),
            "Nothing to do.",
        ),
        Ok(None) => fail(
            "app",
            "no_answer",
            "The app is open but didn't answer in time.".into(),
            json!({ "running": true, "answering": false, "cli_version": cli_version }),
            "It may still answer: `dragent requests show` lists recent request ids once a caller reports one. Rerun `dragent doctor --check app`; if it persists, the app's window may be stuck (see the `hangs` check).",
        ),
        Err(error) => {
            let failure = Failure::from_error(&error);
            if matches!(failure.code.as_str(), "app_too_old" | "request_unreadable") {
                fail(
                    "app",
                    "version_mismatch",
                    "The app didn't understand this command line: CLI and app builds differ.".into(),
                    json!({ "running": true, "answering": false, "cli_version": cli_version, "code": failure.code }),
                    "Update the Dragent app so it matches this command line, then rerun `dragent doctor --check app`.",
                )
            } else {
                fail(
                    "app",
                    "app_not_answering",
                    format!("The app is open but the request failed ({}).", failure.code),
                    json!({ "running": true, "answering": false, "cli_version": cli_version, "code": failure.code }),
                    "Rerun `dragent doctor --check app`; if it persists, compare the organisation in `dragent status --json` with the one you expect.",
                )
            }
        }
    }
}

fn check_signin(project: Option<&Path>) -> Check {
    let terminal = std::env::var("DRAGENT_HOST_TERMINAL_ID")
        .ok()
        .filter(|id| !id.is_empty());
    let workspace = std::env::var("DRAGENT_HOST_WORKSPACE_ID")
        .ok()
        .filter(|id| !id.is_empty());
    if app_running_readonly() {
        let signed_in = super::signed_in();
        let organisation = super::live_organisation()
            .map(|active| active.name)
            .filter(|name| !name.is_empty());
        let evidence = json!({
            "signed_in": signed_in,
            "organisation": organisation,
            "terminal_set": terminal.is_some(),
            "workspace_set": workspace.is_some(),
            "project": project.map(|path| shorten(&path.display().to_string())),
        });
        if !signed_in {
            return fail(
                "signin",
                "signed_out",
                "This terminal is not signed in to Dragent.".into(),
                evidence,
                "Ask the person to open Dragent and sign in, then rerun `dragent doctor --check signin`.",
            );
        }
        return ok(
            "signin",
            "signed_in",
            "Signed in; organisation and terminal context read.".into(),
            evidence,
            "If commands act in the wrong organisation, compare `dragent status --json` with the visible app; never switch organisations silently.",
        );
    }
    // The app is closed: only what it saved while signed in (never tokens).
    let saved = crate::auth::has_saved_session();
    let cached = crate::organisation_context::cached_active();
    let signed_in = saved
        && cached
            .as_ref()
            .is_some_and(|active| active.context == dragent_paths::organisation_id());
    let evidence = json!({
        "signed_in": signed_in,
        "organisation": cached.map(|active| active.name).filter(|name| !name.is_empty()),
        "terminal_set": terminal.is_some(),
        "workspace_set": workspace.is_some(),
        "app_open": false,
        "project": project.map(|path| shorten(&path.display().to_string())),
        "note": "The app is closed: what it last saved, not a live check.",
    });
    if !signed_in {
        return fail(
            "signin",
            "signed_out",
            "Not signed in (and the app is closed, so this is what it last saved).".into(),
            evidence,
            "Open Dragent and sign in, then rerun `dragent doctor --check signin`.",
        );
    }
    ok(
        "signin",
        "signed_in_cached",
        "A sign-in is saved and the app is closed; live identity unchecked.".into(),
        evidence,
        "Open Dragent to verify the live identity with `dragent doctor --check signin`.",
    )
}

fn check_host() -> Check {
    match dragent_host::HostClient::connect_existing() {
        Ok(client) => {
            let alive = client.ping();
            if alive {
                ok(
                    "host",
                    "host_reachable",
                    "The host process answered.".into(),
                    json!({ "reachable": true }),
                    "Nothing to do.",
                )
            } else {
                fail(
                    "host",
                    "host_unreachable",
                    "The host endpoint exists but didn't answer.".into(),
                    json!({ "reachable": false }),
                    "Open Dragent (it owns the host) and rerun `dragent doctor --check host`.",
                )
            }
        }
        Err(error) => fail(
            "host",
            "host_unreachable",
            "Dragent's host isn't reachable.".into(),
            json!({ "reachable": false, "reason": redact_text(&error) }),
            "Open Dragent (it owns the host) and rerun `dragent doctor --check host`.",
        ),
    }
}

fn check_agents() -> Check {
    let installed = crate::workspace::detect_prompt_agents();
    let mut agents: Vec<Value> = crate::workspace::PromptAgent::ALL
        .into_iter()
        .map(|agent| {
            json!({
                "key": agent.command_key(),
                "name": agent.label(),
                "installed": agent == crate::workspace::PromptAgent::Dragent
                    || installed.contains(&agent),
            })
        })
        .collect();
    agents.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    let launchable = agents
        .iter()
        .filter(|agent| agent["installed"].as_bool() == Some(true))
        .count();
    // Provider accounts: metadata only (no live sign-in probes; those can
    // take seconds each and may be interactive).
    let mut accounts: Vec<Value> = Vec::new();
    let mut broken: Vec<String> = Vec::new();
    match dragent_host::agent_accounts::Registry::load() {
        Ok(registry) => {
            for profile in &registry.profiles {
                let binding = registry.resolve(
                    &profile.provider,
                    None,
                    &dragent_host::agent_accounts::Selection::Profile(profile.id),
                );
                if let Err(error) = &binding {
                    broken.push(format!("{}: {error}", profile.label));
                }
                accounts.push(json!({
                    "id": profile.id,
                    "agent": profile.provider,
                    "name": profile.label,
                    "binding": binding.is_ok(),
                }));
            }
        }
        Err(error) => {
            return fail(
                "agents",
                "accounts_unreadable",
                "Provider account metadata couldn't be read.".into(),
                json!({ "installed": launchable, "reason": redact_text(&error) }),
                "Check the account store is readable, then rerun `dragent doctor --check agents`.",
            );
        }
    }
    let evidence = json!({ "installed": launchable, "agents": agents, "accounts": accounts });
    if launchable == 0 {
        fail(
            "agents",
            "no_agents",
            "No agent CLI is installed.".into(),
            evidence,
            "Install one of the agents (`dragent agents list` shows what this build starts), then rerun `dragent doctor --check agents`.",
        )
    } else if !broken.is_empty() {
        warn(
            "agents",
            "account_binding_broken",
            format!("{} provider account binding{} fail{} closed.", broken.len(), if broken.len() == 1 { "" } else { "s" }, if broken.len() == 1 { "s" } else { "" }),
            json!({ "installed": launchable, "agents": agents, "accounts": accounts, "broken": broken }),
            "Rebind with `dragent agents accounts default --agent KEY --account ID|system|inherit`, or remove the stale entry with `dragent agents accounts remove ID`; rerun `dragent doctor --check agents`.",
        )
    } else {
        ok(
            "agents",
            "agents_ready",
            format!("{launchable} agent CLIs installed; provider accounts resolve."),
            evidence,
            "Nothing to do. A missing executable for one agent only matters when that agent should run: install it, then rerun this check.",
        )
    }
}

fn check_mcp() -> Check {
    let catalog = crate::integrations::bridge::catalog();
    let names: Vec<&str> = catalog
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    let evidence = json!({
        "tool_count": names.len(),
        "sample": names.iter().take(10).collect::<Vec<_>>(),
    });
    if names.is_empty() {
        fail(
            "mcp",
            "mcp_no_tools",
            "`dragent mcp` lists no tools.".into(),
            evidence,
            "Check `dragent integrations list`; rerun `dragent doctor --check mcp`.",
        )
    } else {
        ok(
            "mcp",
            "mcp_lists_tools",
            format!("`dragent mcp` lists {} tools.", names.len()),
            evidence,
            "Nothing to do. A failing tool call is about that call's service or permission, not the transport: rerun the exact call, then check `integrations`.",
        )
    }
}

/// Read-only states of the installed copies of one bundled skill.
fn skill_state(
    skill: &'static dragent_host::agent_skills::BundledSkill,
    roots: &[PathBuf],
    record: &BTreeMap<String, String>,
) -> (&'static str, Option<String>) {
    let wanted = skill
        .files
        .iter()
        .find(|file| file.path == "SKILL.md")
        .map(|file| file.bytes);
    for root in roots {
        let path = root.join(skill.name).join("SKILL.md");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if Some(bytes.as_slice()) == wanted {
            return ("current", Some(shorten(&path.display().to_string())));
        }
        let key = path.display().to_string();
        if record
            .get(&key)
            .is_some_and(|digest| *digest == hash(&bytes))
        {
            return ("stale", Some(shorten(&path.display().to_string())));
        }
        return ("customized", Some(shorten(&path.display().to_string())));
    }
    ("missing", None)
}

fn check_skills() -> Check {
    let enabled = super::agent_skill::enabled();
    let computer = super::agent_skill::computer_enabled();
    let root = dragent_host::agent_skills::bundle_root();
    let version_dir = root.join(dragent_host::agent_skills::BUNDLE_VERSION);
    let mut materialized = 0usize;
    for skill in dragent_host::agent_skills::BUNDLED_SKILLS {
        if version_dir.join(skill.name).join("SKILL.md").is_file() {
            materialized += 1;
        }
    }
    let total = dragent_host::agent_skills::BUNDLED_SKILLS.len();
    // Installed copies in external agents (read-only; never installs here).
    let home = dirs::home_dir().unwrap_or_default();
    let mut roots = vec![home.join(".agents/skills")];
    if home.join(".qwen").is_dir() {
        roots.push(home.join(".qwen/skills"));
    }
    let record: BTreeMap<String, String> =
        std::fs::read(dragent_paths::home().join("agent-skill.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value.get("written").cloned())
            .and_then(|written| serde_json::from_value(written).ok())
            .unwrap_or_default();
    let mut skills: Vec<Value> = Vec::new();
    let mut stale = 0usize;
    for skill in dragent_host::agent_skills::BUNDLED_SKILLS {
        if skill.name == dragent_host::agent_skills::COMPUTER_SKILL_NAME && !computer {
            continue;
        }
        let (state, path) = skill_state(skill, &roots, &record);
        if state == "stale" {
            stale += 1;
        }
        skills.push(json!({ "name": skill.name, "state": state, "path": path }));
    }
    skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let evidence = json!({
        "skills_version": dragent_host::agent_skills::BUNDLE_VERSION,
        "shared_folders": enabled,
        "desktop_guidance": computer,
        "materialized": format!("{materialized}/{total}"),
        "skills": skills,
    });
    if !enabled {
        ok(
            "skills",
            "skills_off",
            "Shared skill folders are turned off; nothing is installed or expected.".into(),
            evidence,
            "Nothing to do. Turn them on in Settings to teach external agents the `dragent` command.",
        )
    } else if materialized < total {
        warn(
            "skills",
            "bundle_not_materialized",
            format!("This build's skills aren't fully materialized yet ({materialized}/{total})."),
            evidence,
            "Open Dragent once (it materializes the bundle), then rerun `dragent doctor --check skills`.",
        )
    } else if stale > 0 {
        warn(
            "skills",
            "skills_stale",
            format!("{stale} installed skill{} lag{} this build; untouched files update on sync.", if stale == 1 { "" } else { "s" }, if stale == 1 { "s" } else { "" }),
            evidence,
            "Customized copies stay as they are (yours win). Untouched files update with a newer Dragent; compare with the bundled copy, then rerun `dragent doctor --check skills`.",
        )
    } else {
        ok(
            "skills",
            "skills_current",
            "Bundled skills materialized; installed copies current or customized.".into(),
            evidence,
            "Nothing to do. A customized copy is yours as a whole: compare it with the bundled one before blaming the build.",
        )
    }
}

fn check_desktop() -> Check {
    let os = std::env::consts::OS;
    if !matches!(os, "macos" | "windows" | "linux") {
        return skipped(
            "desktop",
            "desktop_unsupported",
            format!("Desktop control isn't supported on {os}."),
            "Nothing to check on this platform.",
        );
    }
    // Preflight: the exact request shape the backend validates, without
    // contacting the app. A rejection here is a build problem, not a permission one.
    if let Err(error) = dragent_computer::Request::from_tool("permissions", &json!({})) {
        return fail(
            "desktop",
            "desktop_preflight_failed",
            "The permissions check itself doesn't validate.".into(),
            json!({ "os": os, "reason": redact_text(&error) }),
            "Update Dragent, then rerun `dragent doctor --check desktop`.",
        );
    }
    if !app_running_readonly() {
        return fail(
            "desktop",
            "app_not_open",
            "Desktop permissions need the app, which isn't open.".into(),
            json!({ "os": os, "preflight": "valid" }),
            "`dragent open PATH` starts it (desktop control must also be enabled in Settings, by the person), then rerun `dragent doctor --check desktop`.",
        );
    }
    let request = super::requests::Request::ComputerTool {
        pane: crate::integrations::policy::helper_pane(),
        terminal: std::env::var("DRAGENT_HOST_TERMINAL_ID")
            .ok()
            .filter(|id| !id.is_empty()),
        tool: "permissions".into(),
        arguments: json!({}),
    };
    match super::ask_app_within(request, Some(Duration::from_secs(8))) {
        Ok(Some(response)) => ok(
            "desktop",
            "desktop_permissions",
            "Desktop permissions answered.".into(),
            json!({ "os": os, "answer": response.data }),
            "When an action is refused, read the refusal's code (`dragent capabilities --json` lists computer codes); only the person grants OS permissions.",
        ),
        Ok(None) => fail(
            "desktop",
            "no_answer",
            "Desktop permissions didn't answer in time.".into(),
            json!({ "os": os }),
            "Rerun `dragent doctor --check desktop`; if it persists, the app's window may be stuck (see the `hangs` check).",
        ),
        Err(error) => {
            let failure = Failure::from_error(&error);
            fail(
                "desktop",
                "desktop_unavailable",
                format!("Desktop permissions couldn't be read ({}).", failure.code),
                json!({ "os": os, "code": failure.code }),
                "Open Dragent with desktop control enabled in Settings, then rerun `dragent doctor --check desktop`.",
            )
        }
    }
}

fn check_browser() -> Check {
    if !app_running_readonly() {
        return fail(
            "browser",
            "app_not_open",
            "The browser pane needs the app, which isn't open.".into(),
            json!({ "reachable": false }),
            "`dragent open PATH` starts it, then rerun `dragent doctor --check browser` from a Dragent terminal.",
        );
    }
    if std::env::var("DRAGENT_HOST_TERMINAL_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .is_none()
    {
        return fail(
            "browser",
            "no_calling_pane",
            "This terminal doesn't belong to a Dragent workspace, so there is no browser pane of yours.".into(),
            json!({ "reachable": false }),
            "Run this from a Dragent terminal (one whose environment names its pane), then rerun `dragent doctor --check browser`.",
        );
    }
    let request = super::requests::Request::BrowserTool {
        pane: crate::integrations::policy::helper_pane(),
        terminal: std::env::var("DRAGENT_HOST_TERMINAL_ID")
            .ok()
            .filter(|id| !id.is_empty()),
        tool: "status".into(),
        arguments: json!({}),
    };
    match super::ask_app_within(request, Some(Duration::from_secs(8))) {
        Ok(Some(response)) => {
            let output = response
                .data
                .as_ref()
                .and_then(|data| data["output"].as_str())
                .unwrap_or_default();
            let (short, _) = bound_text(output, 500);
            ok(
                "browser",
                "browser_reachable",
                "The browser pane control answered.".into(),
                json!({ "reachable": true, "status": short }),
                "Nothing to do. A failing page is about its server or address, not the pane: rerun the exact `dragent browser` command.",
            )
        }
        Ok(None) => fail(
            "browser",
            "no_answer",
            "The browser pane didn't answer in time.".into(),
            json!({ "reachable": false }),
            "Rerun `dragent doctor --check browser`; if it persists, the app's window may be stuck (see the `hangs` check).",
        ),
        Err(error) => {
            let failure = Failure::from_error(&error);
            fail(
                "browser",
                "browser_unavailable",
                format!("The browser pane couldn't be reached ({}).", failure.code),
                json!({ "reachable": false, "code": failure.code }),
                "Rerun from a Dragent terminal while the app is open: `dragent doctor --check browser`.",
            )
        }
    }
}

fn check_automations(project: Option<&Path>) -> Check {
    let path = dragent_paths::organisation_dir().join("automations.sqlite");
    if !path.exists() {
        return ok(
            "automations",
            "no_automations",
            "No automations are defined.".into(),
            json!({ "count": 0, "project": project.map(|path| shorten(&path.display().to_string())) }),
            "Nothing to do.",
        );
    }
    let store = match dragent_automations::Store::open(&path) {
        Ok(store) => store,
        Err(error) => {
            return fail(
                "automations",
                "store_unreadable",
                "The automation store couldn't be read.".into(),
                json!({ "reason": redact_text(&format!("{error:#}")) }),
                "Check the store file is readable, then rerun `dragent doctor --check automations`.",
            );
        }
    };
    let mut automations = match store.list_automations() {
        Ok(automations) => automations,
        Err(error) => {
            return fail(
                "automations",
                "store_unreadable",
                "Automations couldn't be listed.".into(),
                json!({ "reason": redact_text(&format!("{error:#}")) }),
                "Check the store file is readable, then rerun `dragent doctor --check automations`.",
            );
        }
    };
    automations.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(wanted) = project {
        automations.retain(|automation| {
            !automation.is_session() && automation.project_root.as_path() == wanted
        });
    }
    let latest = store.latest_runs().unwrap_or_default();
    let mut failed: Vec<Value> = Vec::new();
    for automation in &automations {
        if let Some(run) = latest.get(&automation.id) {
            if run.status.as_str() == "failed" {
                failed.push(json!({
                    "automation": automation.name,
                    "run": run.id,
                    "at_ms": run.display_time_ms().or(run.result_at_ms()),
                    "error": run.error.as_ref().map(|error| json!({
                        "step": match error.step {
                            dragent_automations::RunStep::Queue => "queue",
                            dragent_automations::RunStep::Workspace => "workspace",
                            dragent_automations::RunStep::Agent => "agent",
                            dragent_automations::RunStep::Run => "run",
                        },
                        "code": error.code,
                        "message": error.message,
                    })),
                }));
            }
        }
    }
    let enabled = automations
        .iter()
        .filter(|automation| automation.enabled)
        .count();
    let scheduler = if app_running_readonly() {
        "running"
    } else {
        "idle (app closed)"
    };
    let evidence = json!({
        "count": automations.len(),
        "enabled": enabled,
        "scheduler": scheduler,
        "failed_runs": failed,
        "project": project.map(|path| shorten(&path.display().to_string())),
    });
    if !failed.is_empty() {
        warn(
            "automations",
            "run_failed",
            format!("{} automation run{} failed last.", failed.len(), if failed.len() == 1 { "" } else { "s" }),
            evidence,
            "Read the run with `dragent automations runs ID --limit 1`, fix what its error names, and let the schedule (or `dragent automations run ID`) try again; rerun `dragent doctor --check automations`.",
        )
    } else {
        ok(
            "automations",
            "automations_ok",
            format!("{} automation{} ({} enabled); scheduler {scheduler}; no failing last runs.", automations.len(), if automations.len() == 1 { "" } else { "s" }, enabled),
            evidence,
            "Nothing to do. An automation that never runs while the app is closed is expected: the scheduler lives in the app.",
        )
    }
}

/// Which machine these checks ran on, and each remote machine's state as
/// the open app knows it. Read only: never connects anything.
fn check_machines() -> Check {
    let here = dragent_host::scope::this_machine();
    let registry = match dragent_host::machine_registry::MachineRegistry::load() {
        Ok(registry) => registry,
        Err(error) => {
            return fail(
                "machines",
                "machines_unreadable",
                "The list of remote machines couldn't be read.".into(),
                json!({ "this_computer": here, "reason": redact_text(&error) }),
                "Check the machines file is readable (Settings → Machines lists them), then rerun `dragent doctor --check machines`.",
            )
        }
    };
    if registry.machines.is_empty() {
        return ok(
            "machines",
            "local_only",
            "Every check ran on this computer; no remote machines are added.".into(),
            json!({ "this_computer": here }),
            "Nothing to do.",
        );
    }
    let live = app_running_readonly()
        .then(|| {
            super::ask_app_within(super::requests::Request::Machines, Some(Duration::from_secs(5)))
                .ok()
                .flatten()
        })
        .flatten()
        .and_then(|response| response.data)
        .and_then(|data| data["machines"].as_array().cloned());
    let Some(live) = live else {
        return skipped(
            "machines",
            "app_not_open",
            format!(
                "{} remote machine{} added; whether they're connected is known only to the open app.",
                registry.machines.len(),
                if registry.machines.len() == 1 { "" } else { "s" }
            ),
            "Open Dragent, then rerun `dragent doctor --check machines`.",
        );
    };
    let rows: Vec<Value> = registry
        .machines
        .iter()
        .map(|machine| {
            let state = live.iter().find(|row| row["id"] == machine.id.as_str());
            json!({
                "name": machine.name(),
                "id": machine.id,
                "status": state.and_then(|row| row["status"].as_str()).unwrap_or("unknown"),
                "error": state.and_then(|row| row["error"].as_str()).map(redact_text),
            })
        })
        .collect();
    let failing: Vec<String> = rows
        .iter()
        .filter(|row| row["status"] == "error")
        .filter_map(|row| row["name"].as_str().map(str::to_string))
        .collect();
    let evidence = json!({ "this_computer": here, "remote": rows });
    if failing.is_empty() {
        ok(
            "machines",
            "machines_known",
            "Checks ran on this computer; remote machines are listed with their state.".into(),
            evidence,
            "A remote project's terminals, files and git run on its machine: when one fails, check that machine here first.",
        )
    } else {
        warn(
            "machines",
            "machine_connection_issue",
            format!("A remote machine has a connection issue: {}.", failing.join(", ")),
            evidence,
            "Reconnect it in Settings → Machines (the person's step; Dragent never reconnects a machine on its own), then repeat what failed.",
        )
    }
}

fn check_hangs() -> Check {
    let dir = dragent_paths::home().join("logs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return ok(
            "hangs",
            "no_hang_reports",
            "No freeze reports exist.".into(),
            json!({ "count": 0 }),
            "Nothing to do.",
        );
    };
    let mut newest: Option<(String, i64)> = None;
    let mut count = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("hang-") || !name.ends_with(".txt") {
            continue;
        }
        count += 1;
        let at_ms = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|age| age.as_millis() as i64)
            .unwrap_or(0);
        if newest.as_ref().is_none_or(|(_, at)| at_ms >= *at) {
            newest = Some((dir.join(&name).display().to_string(), at_ms));
        }
    }
    match newest {
        None => ok(
            "hangs",
            "no_hang_reports",
            "No freeze reports exist.".into(),
            json!({ "count": 0 }),
            "Nothing to do.",
        ),
        Some((path, at_ms)) => ok(
            "hangs",
            "hang_reported",
            format!("{count} freeze report{}; newest noted.", if count == 1 { "" } else { "s" }),
            json!({ "count": count, "newest": shorten(&path), "at_ms": at_ms }),
            "Only paths and timestamps are shown here. If the freeze repeats, sample the app while it is stuck and attach the newest report to feedback (`dragent feedback --draft` first).",
        ),
    }
}

/// Shorten to `max` characters, reporting whether anything was cut.
fn bound_text(text: &str, max: usize) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return (text.to_string(), false);
    }
    let mut short: String = chars.into_iter().take(max).collect();
    short.push('…');
    (short, true)
}

fn overall(checks: &[Check]) -> &'static str {
    if checks.iter().any(|check| check.status == "fail") {
        "partial"
    } else {
        "ok"
    }
}

pub fn run(args: &[String]) -> Result<String, String> {
    let opts = parse(args)?;
    if opts.help {
        return Ok(HELP.trim_end().to_string());
    }
    // JSON for agents unless the caller asked for text; `--quiet` prints ids.
    let json = opts.json || (!opts.text && !opts.quiet && agent_environment());
    let wanted = |id: &str| {
        opts.checks
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|wanted| wanted == id))
    };
    let mut checks: Vec<Check> = Vec::new();
    // Each check is independent: one failing (or panicking, which can't
    // happen: every check returns) never stops the others.
    if wanted("launcher") {
        checks.push(check_launcher());
    }
    if wanted("app") {
        checks.push(check_app());
    }
    if wanted("signin") {
        checks.push(check_signin(opts.project.as_deref()));
    }
    if wanted("host") {
        checks.push(check_host());
    }
    if wanted("agents") {
        checks.push(check_agents());
    }
    if wanted("mcp") {
        checks.push(check_mcp());
    }
    if wanted("skills") {
        checks.push(check_skills());
    }
    if wanted("desktop") {
        checks.push(check_desktop());
    }
    if wanted("browser") {
        checks.push(check_browser());
    }
    if wanted("automations") {
        checks.push(check_automations(opts.project.as_deref()));
    }
    if wanted("hangs") {
        checks.push(check_hangs());
    }
    if wanted("machines") {
        checks.push(check_machines());
    }
    let report = build_report(&checks, &opts);
    Ok(render(&report, &checks, &opts, json))
}

fn build_report(checks: &[Check], opts: &Opts) -> Value {
    let failing: Vec<&str> = checks
        .iter()
        .filter(|check| check.status == "fail")
        .map(|check| check.id)
        .collect();
    json!({
        "status": overall(checks),
        "generated_at_ms": now_ms(),
        "cli_version": env!("CARGO_PKG_VERSION"),
        "project": opts.project.as_ref().map(|path| shorten(&path.display().to_string())),
        "checks": checks.iter().map(Check::entry).collect::<Vec<_>>(),
        "failing": failing,
    })
}

fn render(report: &Value, checks: &[Check], opts: &Opts, json: bool) -> String {
    if opts.quiet {
        return report["failing"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }
    if json {
        return serde_json::to_string_pretty(report).unwrap_or_default();
    }
    render_text(report, checks)
}

fn render_text(report: &Value, checks: &[Check]) -> String {
    let mut out = format!(
        "Doctor · {} · {} check{}\n",
        report["status"].as_str().unwrap_or("ok"),
        checks.len(),
        if checks.len() == 1 { "" } else { "s" },
    );
    for check in checks {
        out.push_str(&format!(
            "[{}] {}: {}\n  {}\n",
            check.status, check.id, check.summary, check.next
        ));
    }
    let failing: Vec<&str> = checks
        .iter()
        .filter(|check| check.status == "fail")
        .map(|check| check.id)
        .collect();
    if !failing.is_empty() {
        out.push_str(&format!("Failing: {}\n", failing.join(", ")));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    fn passing(id: &'static str) -> Check {
        ok(
            id,
            "fine",
            format!("{id} is fine"),
            json!({}),
            "Nothing to do.",
        )
    }

    fn failing(id: &'static str) -> Check {
        fail(
            id,
            "broken",
            format!("{id} is broken"),
            json!({}),
            "Fix it.",
        )
    }

    #[test]
    fn one_failing_check_yields_partial_with_the_others_intact() {
        let checks = vec![passing("app"), failing("host"), passing("mcp")];
        let opts = Opts::default();
        let report = build_report(&checks, &opts);
        assert_eq!(report["status"], "partial");
        assert_eq!(report["failing"], json!(["host"]));
        assert_eq!(report["checks"].as_array().unwrap().len(), 3);
        assert_eq!(report["checks"][0]["status"], "ok");
        assert_eq!(report["checks"][1]["status"], "fail");
        assert_eq!(report["checks"][2]["status"], "ok");

        let all_ok = vec![passing("app"), passing("host")];
        assert_eq!(build_report(&all_ok, &opts)["status"], "ok");
        assert!(build_report(&all_ok, &opts)["failing"]
            .as_array()
            .unwrap()
            .is_empty());

        // Skips and warnings never hide a failure, and alone stay `ok`.
        let mixed = vec![
            passing("app"),
            skipped(
                "desktop",
                "desktop_unsupported",
                "not here".into(),
                "Nothing.",
            ),
            warn(
                "automations",
                "run_failed",
                "a run failed".into(),
                json!({}),
                "Look.",
            ),
        ];
        assert_eq!(build_report(&mixed, &opts)["status"], "ok");
    }

    #[test]
    fn evidence_redacts_secrets_and_shortens_home_paths() {
        let home = dragent_paths::home().display().to_string();
        let evidence = json!({
            "path": format!("{home}/bin/dragent"),
            "detail": "login with api_key=sk-abcdef1234567890 failed",
            "bearer": "Bearer abcdefghijklmnop",
            "id": "6f1c2d3e-0000-4000-8000-000000000001",
        });
        let check = ok(
            "launcher",
            "launcher_current",
            "fine".into(),
            evidence,
            "Nothing.",
        );
        let text = serde_json::to_string(&check.entry()).unwrap();
        assert!(!text.contains("sk-abcdef"), "{text}");
        assert!(!text.contains("abcdefghijklmnop"), "{text}");
        assert!(!text.contains(&home), "{text}");
        assert!(text.contains('~'), "{text}");
        assert!(text.contains("[redacted]"), "{text}");
        // Operation ids survive redaction.
        assert!(
            text.contains("6f1c2d3e-0000-4000-8000-000000000001"),
            "{text}"
        );
    }

    #[test]
    fn check_filtering_runs_only_some_and_rejects_unknown_ids() {
        let opts = parse(&args(&["--check", "app,host"])).unwrap();
        assert_eq!(
            opts.checks,
            Some(vec!["app".to_string(), "host".to_string()])
        );
        let opts = parse(&args(&["--check=launcher"])).unwrap();
        assert_eq!(opts.checks, Some(vec!["launcher".to_string()]));
        let error = parse(&args(&["--check", "frobnicate"])).unwrap_err();
        let failure = Failure::from_error(&error);
        assert_eq!(
            (failure.status, failure.code.as_str()),
            (Status::InvalidInput, "usage")
        );
        let error = parse(&args(&["--check", ""])).unwrap_err();
        assert!(error.contains("--check needs at least one"), "{error}");
        let opts = parse(&args(&["--project", "/tmp", "--json"])).unwrap();
        assert!(opts.project.is_some() && opts.json);
    }

    #[test]
    fn a_skipped_check_carries_its_reason() {
        let check = skipped(
            "desktop",
            "desktop_unsupported",
            "not on this OS".into(),
            "Nothing.",
        );
        assert_eq!(check.status, "skipped");
        assert_eq!(check.entry()["code"], "desktop_unsupported");
        let report = build_report(&[check], &Opts::default());
        assert_eq!(report["status"], "ok");
        assert!(report["failing"].as_array().unwrap().is_empty());
    }

    #[test]
    fn launcher_scripts_resolve_to_their_target() {
        assert_eq!(
            launcher_target("#!/bin/sh\nexec '/Applications/Dragent/dragent' '--dragent-organisation=o' --dragent-cli \"$@\"\n"),
            Some("/Applications/Dragent/dragent".to_string())
        );
        assert_eq!(
            launcher_target(
                "@echo off\r\n\"C:\\Program Files\\Dragent\\dragent.exe\" --dragent-cli %*\r\n"
            ),
            Some("C:\\Program Files\\Dragent\\dragent.exe".to_string())
        );
        assert_eq!(launcher_target("nothing to run here"), None);
    }

    #[test]
    fn quiet_lists_only_failing_checks() {
        let checks = vec![passing("app"), failing("host")];
        let mut opts = Opts::default();
        opts.quiet = true;
        let report = build_report(&checks, &opts);
        assert_eq!(render(&report, &checks, &opts, false), "host");
        let ok_checks = vec![passing("app")];
        let ok_report = build_report(&ok_checks, &opts);
        assert_eq!(render(&ok_report, &ok_checks, &opts, false), "");
    }
}
