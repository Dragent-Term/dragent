//! The automations runtime end to end, short of launching a real agent:
//! saving plans the next occurrence, "Run now" claims a run, a worker
//! prepares its workspace (real git worktrees, branches, session folders),
//! and the outcome comes back through the runtime. The agent key is unknown
//! on purpose, so runs stop at the agent step. The `dragent automations`
//! command line is exercised through the real binary against the same
//! store.
//!
//! Its own test binary: it points `DRAGENT_HOME` at a temporary directory
//! before anything opens storage.

#![cfg(not(target_family = "wasm"))]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use dragent::automations::{Account, AutomationRuntime, LIMIT_MESSAGE};
use dragent::organisation_context::OrganisationContext;
use dragent_automations::{Automation, Run, RunFilter, RunStatus, RunStep, WorkspaceMode};
use warpui::r#async::Timer;
use warpui::{App, ModelHandle};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repository(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    std::fs::write(root.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "init"]);
    git(root, &["branch", "release"]);
    // Local configuration new worktrees should get.
    std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
}

/// A dummy sign-in for the command line, which refuses to run signed out:
/// a saved session (any refresh token) and the organisation the app last
/// confirmed (user `me`, as the app account below), on the `local` context
/// this test's app uses. Nothing here
/// reaches a server: the automations commands edit the local store.
fn sign_in_for_tests(home: &Path) {
    std::fs::write(
        home.join("organisation.json"),
        r#"{"active":{"user_id":"me","organisation_id":"test-org","name":"Test","context":"local"}}"#,
    )
    .unwrap();
}

fn cli(exe: &Path, home: &Path, args: &[&str]) -> (bool, String) {
    sign_in_for_tests(home);
    let output = Command::new(exe)
        .arg("automations")
        .args(args)
        .env("DRAGENT_HOME", home)
        .env("DRAGENT_REFRESH_TOKEN", "test-refresh-token")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

async fn wait_until(
    app: &App,
    runtime: &ModelHandle<AutomationRuntime>,
    done: impl Fn(&AutomationRuntime) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !runtime.read(app, |runtime, _| done(runtime)) {
        assert!(
            Instant::now() < deadline,
            "runtime did not settle: {:#?}",
            runtime.read(app, |runtime, _| runtime
                .automations()
                .iter()
                .map(|a| (runtime.latest_run(&a.id), runtime.scheduled_run(&a.id)))
                .collect::<Vec<_>>())
        );
        Timer::after(Duration::from_millis(25)).await;
    }
}

/// Queue a run and wait until it finishes; returns it.
async fn run_once(app: &mut App, runtime: &ModelHandle<AutomationRuntime>, id: &str) -> Run {
    let before = runtime.read(app, |runtime, _| {
        runtime.runs(id, RunFilter::All, 100).len()
    });
    runtime
        .update(app, |runtime, ctx| runtime.run_now(id, ctx))
        .unwrap();
    // Finished in the store, and its slot released: the worker records the
    // outcome a moment before the runtime frees the automation, and a run
    // asked for in between is refused as already running.
    wait_until(app, runtime, |runtime| {
        let runs = runtime.runs(id, RunFilter::All, 100);
        runs.len() > before && runs[0].status.is_finished() && !runtime.is_active(id)
    })
    .await;
    runtime
        .read(app, |runtime, _| runtime.latest_run(id))
        .unwrap()
}

#[test]
fn runs_prepare_their_workspaces_and_record_the_outcome() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("DRAGENT_HOME", home.path());
    let home_path = home.path().canonicalize().unwrap();
    let repo = home_path.join("project");
    repository(&repo);
    // Git runs through the host service, as in the app: start one (this
    // package's own binary) for the temporary home.
    let exe = Path::new(env!("CARGO_BIN_EXE_dragent"));
    std::env::set_var("DRAGENT_HOST_EXE", exe);
    let host = (0..6)
        .find_map(|_| dragent_host::HostClient::ensure_host_with(exe).ok())
        .expect("host service starts");

    let run_host = host.clone();
    App::test((), |mut app| async move {
        app.add_singleton_model(OrganisationContext::new_detached);
        let runtime = app.add_singleton_model(AutomationRuntime::new);

        // Invalid definitions are refused with a reason.
        let mut nightly = Automation::new(
            "Nightly",
            repo.clone(),
            "0 9 * * *",
            "no-such-agent",
            "  ",
            WorkspaceMode::default(),
            0,
        );
        let refused = runtime.update(&mut app, |runtime, ctx| runtime.save(nightly.clone(), ctx));
        assert!(refused.unwrap_err().contains("prompt"));

        nightly.prompt = "Review yesterday's changes".into();
        nightly.tags = vec!["Nightly".into(), "nightly".into()];
        let saved = runtime
            .update(&mut app, |runtime, ctx| runtime.save(nightly.clone(), ctx))
            .unwrap();
        assert_eq!(
            saved.tags,
            vec!["nightly".to_string()],
            "tags are normalized"
        );
        // Saving plans the next occurrence.
        let planned = runtime
            .read(&app, |runtime, _| runtime.scheduled_run(&saved.id))
            .expect("next occurrence planned");
        assert_eq!(planned.status, RunStatus::Scheduled);
        assert!(planned.scheduled_at_ms.unwrap() > chrono::Utc::now().timestamp_millis());

        // A new worktree each run, with the project's ignored config copied.
        let run = run_once(&mut app, &runtime, &saved.id).await;
        assert_eq!(run.status, RunStatus::Failed, "{run:?}");
        let error = run.error.clone().unwrap();
        assert_eq!(
            (error.step, error.code.as_str()),
            (RunStep::Agent, "unknown_agent")
        );
        let worktree = run.workspace_path.clone().expect("worktree recorded");
        assert!(
            worktree.join("README.md").is_file(),
            "{}",
            worktree.display()
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join(".env")).unwrap(),
            "SECRET=1\n",
            "gitignored config copied into the worktree"
        );
        let branch = run.branch.clone().unwrap();
        assert!(branch.starts_with("automation/nightly-"), "{branch}");
        // The slot is free again; the failure shows in the counts, the
        // "Needs attention" group, and the sidebar badge until seen.
        assert!(!runtime.read(&app, |runtime, _| runtime.is_active(&saved.id)));
        let counts = runtime.read(&app, |runtime, _| runtime.run_counts(&saved.id));
        assert_eq!(counts[&RunFilter::Failed], 1);
        assert!(runtime.read(&app, |runtime, _| runtime.failing().contains(&saved.id)));
        assert_eq!(
            runtime.read(&app, |runtime, _| runtime.unseen_failures()),
            1
        );
        runtime.update(&mut app, |runtime, ctx| runtime.mark_failures_seen(ctx));
        assert_eq!(
            runtime.read(&app, |runtime, _| runtime.unseen_failures()),
            0
        );
        assert_eq!(
            runtime.read(&app, |runtime, _| runtime.stats().failed_7d),
            1
        );

        // The header learns which automation a sidebar workspace came from.
        let placed = worktree.canonicalize().unwrap();
        runtime.update(&mut app, |runtime, ctx| {
            runtime.record_run_workspace(&run, &placed, ctx)
        });
        let marked = runtime
            .read(&app, |runtime, _| runtime.run_workspace(&placed))
            .unwrap();
        assert_eq!(marked.automation_name, "Nightly");

        // An existing branch: a worktree on it, reused by the next run.
        let mut on_branch = saved.clone();
        on_branch.workspace = WorkspaceMode::ExistingBranch {
            branch: "release".into(),
        };
        runtime
            .update(&mut app, |runtime, ctx| runtime.save(on_branch, ctx))
            .unwrap();
        let first = run_once(&mut app, &runtime, &saved.id).await;
        assert_eq!(first.branch.as_deref(), Some("release"));
        let release = first.workspace_path.clone().unwrap();
        assert_ne!(
            release.canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
        let again = run_once(&mut app, &runtime, &saved.id).await;
        let again_error = again.error.clone();
        assert_eq!(
            again.workspace_path.map(|p| p.canonicalize().unwrap()),
            Some(release.canonicalize().unwrap()),
            "the branch's worktree is reused: {:?}",
            again_error
        );

        // Pinned to the project's checkout: no worktree at all.
        let mut pinned = saved.clone();
        pinned.workspace = WorkspaceMode::Pinned { path: None };
        pinned.continue_session = true;
        let pinned = runtime
            .update(&mut app, |runtime, ctx| runtime.save(pinned, ctx))
            .unwrap();
        let checkout = run_once(&mut app, &runtime, &saved.id).await;
        assert_eq!(
            checkout.workspace_path.as_deref(),
            Some(repo.as_path()),
            "{checkout:?}"
        );
        assert_eq!(checkout.branch.as_deref(), Some("main"));
        assert!(checkout.snapshot.continue_session);

        // A session: no project, a fresh folder of its own each run.
        let mut session = pinned.clone();
        session.project_root = Default::default();
        session.workspace = WorkspaceMode::Pinned { path: None };
        let session = runtime
            .update(&mut app, |runtime, ctx| runtime.save(session, ctx))
            .unwrap();
        assert!(session.is_session());
        let sessions = runtime.read(&app, |runtime, _| runtime.sessions_root());
        let one = run_once(&mut app, &runtime, &saved.id).await;
        let two = run_once(&mut app, &runtime, &saved.id).await;
        let (one, two) = (one.workspace_path.unwrap(), two.workspace_path.unwrap());
        assert!(
            one.starts_with(&sessions) && one.is_dir(),
            "{}",
            one.display()
        );
        assert_ne!(one, two);

        // Prompt history: every saved prompt, newest first, restorable.
        let mut edited = session.clone();
        edited.prompt = "Summarise yesterday".into();
        runtime
            .update(&mut app, |runtime, ctx| runtime.save(edited, ctx))
            .unwrap();
        let history = runtime.read(&app, |runtime, _| runtime.prompt_versions(&saved.id));
        assert_eq!(history[0].prompt, "Summarise yesterday");
        let original = history.last().unwrap().clone();
        assert_eq!(original.prompt, "Review yesterday's changes");
        runtime
            .update(&mut app, |runtime, ctx| {
                runtime.restore_prompt(&saved.id, original.id, ctx)
            })
            .unwrap();
        let restored = runtime
            .read(&app, |runtime, _| runtime.automation(&saved.id))
            .unwrap();
        assert_eq!(restored.prompt, "Review yesterday's changes");

        // The command line edits the same store; the app notices. Agents
        // must be ones Dragent can start.
        let create = |agent: &str| {
            cli(
                exe,
                &home_path,
                &[
                    "create",
                    "--name",
                    "Docs refresh",
                    "--prompt",
                    "Update the docs",
                    "--schedule",
                    "0 8 * * 1",
                    "--project",
                    repo.to_str().unwrap(),
                    "--agent",
                    agent,
                    "--json",
                ],
            )
        };
        let (ok, out) = create("no-such-agent");
        assert!(!ok && out.contains("unknown agent no-such-agent"), "{out}");
        let (ok, out) = create("codex");
        assert!(ok, "{out}");
        let created: serde_json::Value = serde_json::from_str(&out).unwrap();
        let cli_id = created["id"].as_str().unwrap().to_string();
        wait_until(&app, &runtime, |runtime| {
            runtime.automation(&cli_id).is_some()
        })
        .await;
        let (ok, out) = cli(
            exe,
            &home_path,
            &["prompt", "set", &cli_id, "--prompt", "Refresh the docs"],
        );
        assert!(ok, "{out}");
        let (ok, out) = cli(exe, &home_path, &["list"]);
        assert!(ok && out.contains("Docs refresh"), "{out}");
        let (ok, out) = cli(exe, &home_path, &["prompt", "history", &cli_id, "--json"]);
        assert!(ok, "{out}");
        let versions: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(versions[0]["source"], "agent");
        let (ok, out) = cli(exe, &home_path, &["nonsense"]);
        assert!(!ok && out.contains("unknown command"), "{out}");

        // The free plan holds 4 automations; Pro is unlimited.
        runtime.update(&mut app, |runtime, ctx| {
            runtime.set_account(
                Account {
                    user_id: Some("me".into()),
                    pro: false,
                },
                ctx,
            )
        });
        for name in ["Third", "Fourth"] {
            let mut more = saved.clone();
            more.id = uuid::Uuid::new_v4().to_string();
            more.name = name.into();
            runtime
                .update(&mut app, |runtime, ctx| runtime.save(more, ctx))
                .unwrap();
        }
        let mut fifth = saved.clone();
        fifth.id = uuid::Uuid::new_v4().to_string();
        fifth.name = "Fifth".into();
        let refused = runtime.update(&mut app, |runtime, ctx| runtime.save(fifth.clone(), ctx));
        assert_eq!(refused.unwrap_err(), LIMIT_MESSAGE);
        let (ok, out) = cli(
            exe,
            &home_path,
            &[
                "create",
                "--name",
                "Fifth",
                "--prompt",
                "x",
                "--schedule",
                "0 9 * * *",
                "--session",
            ],
        );
        assert!(!ok && out.contains("4 automations"), "{out}");
        runtime.update(&mut app, |runtime, ctx| {
            runtime.set_account(
                Account {
                    user_id: Some("me".into()),
                    pro: true,
                },
                ctx,
            )
        });
        runtime
            .update(&mut app, |runtime, ctx| runtime.save(fifth, ctx))
            .unwrap();

        // A configured terminal agent exercises the real launch, PTY, prompt
        // delivery, and continuation path without network access or AI costs.
        #[cfg(unix)]
        {
            let script = home_path.join("record_agent.py");
            let events = home_path.join("agent-events.jsonl");
            std::fs::write(
                &script,
                r#"import json, os, re, sys
log = sys.argv[1]
def record(prompt):
    with open(log, 'a') as output:
        output.write(json.dumps({'pid': os.getpid(), 'cwd': os.getcwd(), 'prompt': prompt}) + '\n')
    print('RECORDED', flush=True)
record(sys.argv[2])
for line in sys.stdin:
    record(re.sub(r'\x1b\[(?:200|201)~', '', line).strip())
"#,
            )
            .unwrap();
            let command = format!("python3 -u '{}' '{}'", script.display(), events.display());
            std::fs::write(
                home_path.join("ui_settings.json"),
                serde_json::json!({
                    "onboarding_completed": true, "theme_id": "dragent-dark",
                    "agent_command_configs": { "claude": {
                        "command": command, "prompt_command": "",
                        "prompt_suffix": "", "prompt_stdin": false
                    }}
                })
                .to_string(),
            )
            .unwrap();
            let mut probe = Automation::new(
                "Launch probe",
                repo.clone(),
                "0 9 * * *",
                "claude",
                "first prompt",
                WorkspaceMode::default(),
                0,
            );
            runtime
                .update(&mut app, |runtime, ctx| runtime.save(probe.clone(), ctx))
                .unwrap();
            let mut terminals = Vec::new();
            for mode in [
                WorkspaceMode::default(),
                WorkspaceMode::ExistingBranch {
                    branch: "release".into(),
                },
                WorkspaceMode::Pinned { path: None },
            ] {
                probe.workspace = mode;
                runtime
                    .update(&mut app, |runtime, ctx| runtime.save(probe.clone(), ctx))
                    .unwrap();
                let run = run_once(&mut app, &runtime, &probe.id).await;
                assert!(run.error.is_none(), "real agent launch: {run:?}");
                let terminal = run.terminal.clone().expect("agent terminal recorded");
                let count = terminals.len() + 1;
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    let records = std::fs::read_to_string(&events).unwrap_or_default();
                    if records.lines().count() >= count {
                        let event: serde_json::Value =
                            serde_json::from_str(records.lines().last().unwrap()).unwrap();
                        // Runs lead with the note that nobody is watching.
                        let prompt = event["prompt"].as_str().unwrap();
                        assert!(
                            prompt.starts_with("This is a scheduled Dragent automation run")
                                && prompt.ends_with("\n\nfirst prompt"),
                            "{prompt}"
                        );
                        assert_eq!(
                            Path::new(event["cwd"].as_str().unwrap())
                                .canonicalize()
                                .unwrap(),
                            run.workspace_path.unwrap().canonicalize().unwrap()
                        );
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "agent did not receive initial prompt"
                    );
                    Timer::after(Duration::from_millis(25)).await;
                }
                terminals.push(terminal);
            }
            // Fresh conversation in the same pinned checkout means a new PTY.
            let fresh = run_once(&mut app, &runtime, &probe.id).await;
            let fresh = fresh.terminal.expect("fresh terminal");
            assert_ne!(fresh.session_id, terminals.last().unwrap().session_id);
            terminals.push(fresh.clone());
            // Wait until the last process has consumed its initial prompt.
            let deadline = Instant::now() + Duration::from_secs(10);
            while std::fs::read_to_string(&events)
                .unwrap_or_default()
                .lines()
                .count()
                < 4
            {
                assert!(Instant::now() < deadline, "fresh agent did not launch");
                Timer::after(Duration::from_millis(25)).await;
            }
            probe.prompt = "continued prompt".into();
            probe.continue_session = true;
            runtime
                .update(&mut app, |runtime, ctx| runtime.save(probe.clone(), ctx))
                .unwrap();
            let continued = run_once(&mut app, &runtime, &probe.id).await;
            assert!(continued.error.is_none(), "continued: {continued:?}");
            assert_eq!(continued.terminal.unwrap().session_id, fresh.session_id);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let records = std::fs::read_to_string(&events).unwrap_or_default();
                let rows: Vec<serde_json::Value> = records
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                // The stand-in agent records each line it reads: the note that
                // nobody is watching arrives first, then the prompt itself.
                if let Some(at) = rows
                    .iter()
                    .position(|row| row["prompt"] == "continued prompt")
                {
                    assert!(at > 3, "the continued prompt came after the first run");
                    assert_eq!(
                        rows[3]["pid"], rows[at]["pid"],
                        "same agent process gets next prompt"
                    );
                    assert!(
                        rows[4..at].iter().any(|row| row["prompt"]
                            .as_str()
                            .unwrap_or_default()
                            .starts_with("This is a scheduled Dragent automation run")),
                        "the unattended note leads the continued prompt"
                    );
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "continued prompt never reached the agent"
                );
                Timer::after(Duration::from_millis(25)).await;
            }
            run_host
                .close_terminal(&fresh.workspace_id, &fresh.session_id)
                .unwrap();
            let restarted = run_once(&mut app, &runtime, &probe.id).await;
            assert!(
                restarted.error.is_none(),
                "restart after exit: {restarted:?}"
            );
            let restarted = restarted.terminal.expect("fallback terminal");
            assert_ne!(restarted.session_id, fresh.session_id);
            terminals.push(restarted);
            for terminal in terminals {
                let _ = run_host.close_terminal(&terminal.workspace_id, &terminal.session_id);
            }
            runtime.update(&mut app, |runtime, ctx| runtime.delete(&probe.id, ctx));
        }

        // Pausing blocks manual runs; deleting removes everything.
        runtime
            .update(&mut app, |runtime, ctx| {
                runtime.set_enabled(&saved.id, false, ctx)
            })
            .unwrap();
        assert!(runtime
            .update(&mut app, |runtime, ctx| runtime.run_now(&saved.id, ctx))
            .is_err());
        assert!(runtime
            .read(&app, |runtime, _| runtime.scheduled_run(&saved.id))
            .is_none());
        runtime.update(&mut app, |runtime, ctx| runtime.delete(&saved.id, ctx));
        assert!(runtime
            .read(&app, |runtime, _| runtime.automation(&saved.id))
            .is_none());
        assert!(runtime
            .read(&app, |runtime, _| runtime.prompt_versions(&saved.id))
            .is_empty());
        let (ok, out) = cli(exe, &home_path, &["delete", &cli_id]);
        assert!(ok, "{out}");
    });
    let _ = host.shutdown();
}
