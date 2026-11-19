//! Signing out and into another account swaps every window's local data.
//!
//! Windows follow `OrganisationContext`: on `WillSwitch` they save and detach
//! while the previous organisation is still selected, and on `Switched` they
//! restore from the new one. If those events arrived with the selection
//! already changed (or never changed), the previous account's tabs, projects,
//! and worktrees would be saved into, or shown from, the wrong organisation.
//!
//! Its own test binary: the organisation selection is process-wide, and
//! `DRAGENT_HOME` points at a temporary directory before anything opens
//! storage, so nothing on this machine is read or written.

#![cfg(not(target_family = "wasm"))]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dragent::organisation_context::{OrganisationContext, OrganisationContextEvent};
use warpui::r#async::Timer;
use warpui::{App, Entity, ModelContext};

/// Both tests move the process-wide home and organisation selection.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Recorder;

impl Entity for Recorder {
    type Event = ();
}

type Seen = Rc<RefCell<Vec<(OrganisationContextEvent, String)>>>;

async fn wait_for(seen: &Seen, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.borrow().len() < count {
        assert!(
            Instant::now() < deadline,
            "switch did not finish: {:?}",
            seen.borrow()
        );
        Timer::after(Duration::from_millis(10)).await;
    }
}

#[test]
fn signing_out_and_into_another_account_swaps_local_data_in_order() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("DRAGENT_HOME", home.path());
    // Alice was signed in last time: her personal organisation owns `local`.
    std::fs::write(
        home.path().join("organisation.json"),
        r#"{"active":{"user_id":"alice","organisation_id":"alice-personal","name":"Alice's Team","context":"local"},"local_owner":"alice"}"#,
    )
    .unwrap();
    dragent::organisation_context::restore_at_launch();
    assert_eq!(dragent_paths::organisation_id(), "local");

    App::test((), |mut app| async move {
        app.add_singleton_model(dragent::settings::OrgStore::new);
        app.add_singleton_model(dragent::settings::AccountStore::new);
        let context = app.add_model(OrganisationContext::new_detached);
        let seen: Seen = Rc::default();
        let log = seen.clone();
        let observed = context.clone();
        let _recorder = app.add_model(move |ctx: &mut ModelContext<Recorder>| {
            ctx.subscribe_to_model(&observed, move |_, event, _| {
                log.borrow_mut()
                    .push((event.clone(), dragent_paths::organisation_id()));
            });
            Recorder
        });

        // Alice signs out: her workspaces must leave the screen.
        context.update(&mut app, |c, ctx| c.sign_out_for_tests(ctx));
        wait_for(&seen, 2).await;
        assert_eq!(
            seen.borrow().as_slice(),
            &[
                (OrganisationContextEvent::WillSwitch, "local".to_string()),
                (OrganisationContextEvent::Switched, "signed-out".to_string()),
            ],
            "windows detach under Alice's context and restore into the signed-out one"
        );
        assert!(context.read(&app, |c, _| c.active().is_none()));
        let cache = std::fs::read_to_string(home.path().join("organisation.json")).unwrap();
        assert!(!cache.contains("alice-personal"), "{cache}");

        // Bob signs in and his organisation resolves.
        context.update(&mut app, |c, ctx| c.switch_for_tests("bob-org", ctx));
        wait_for(&seen, 4).await;
        assert_eq!(
            &seen.borrow()[2..],
            &[
                (OrganisationContextEvent::WillSwitch, "signed-out".to_string()),
                (OrganisationContextEvent::Switched, "bob-org".to_string()),
            ]
        );

        // Resolving the same organisation again changes nothing on screen.
        context.update(&mut app, |c, ctx| c.switch_for_tests("bob-org", ctx));
        Timer::after(Duration::from_millis(100)).await;
        assert_eq!(seen.borrow().len(), 4);
        assert!(!context.read(&app, |c, _| c.is_switching()));
    });
}

fn save_projects(organisation: &str, roots: &[std::path::PathBuf]) {
    use dragent_persistence::{SerializedDocumentSession, SerializedSession};
    let mut conn =
        dragent_persistence::setup_connection(&dragent_persistence::database_path().unwrap())
            .unwrap();
    dragent_persistence::save_session_snapshot(
        &mut conn,
        &SerializedSession {
            tabs: Vec::new(),
            active_tab_id: 0,
            active_pane_uuid: None,
            chrome: Default::default(),
            document: SerializedDocumentSession {
                organisation_id: organisation.into(),
                root: roots[0].clone(),
                open_project_roots: roots.to_vec(),
                ..Default::default()
            },
            layouts: Default::default(),
        },
    )
    .unwrap();
}

fn restored_projects() -> Vec<std::path::PathBuf> {
    dragent::workspace::prewarm::load_persistence()
        .0
        .map(|loaded| loaded.session.document.open_project_roots)
        .unwrap_or_default()
}

/// What each window restores after a switch: the selected organisation's own
/// latest save. A process-wide restore cache used to hand every organisation
/// the first account's projects (and then saved them into that organisation).
#[test]
fn each_organisation_restores_only_its_own_latest_projects() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("DRAGENT_HOME", home.path());
    let project = |name: &str| {
        let root = home.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        root
    };
    let (alice_repo, alice_later, bob_repo) = (project("alice"), project("alice-2"), project("bob"));

    // Bob already has his own saved workspace in his organisation.
    dragent_paths::select_organisation("bob-team");
    save_projects("bob-team", &[bob_repo.clone()]);

    // Launch signed in as Alice. (Organisation ids differ from the other
    // test's: persistence keeps one initialization per id per process.)
    dragent_paths::select_organisation("alice-team");
    save_projects("alice-team", &[alice_repo.clone()]);
    dragent_persistence::warm_in_background();
    assert_eq!(restored_projects(), vec![alice_repo.clone()]);
    // A second window of the same launch shares the same restore.
    assert_eq!(restored_projects(), vec![alice_repo.clone()]);
    // Alice keeps working: a later save before she signs out.
    save_projects("alice-team", &[alice_repo.clone(), alice_later.clone()]);

    // Signed out: nothing of Alice's.
    dragent_paths::select_organisation("nobody-signed-in");
    assert!(restored_projects().is_empty(), "{:?}", restored_projects());

    // Bob signs in: only Bob's.
    dragent_paths::select_organisation("bob-team");
    assert_eq!(restored_projects(), vec![bob_repo.clone()]);

    // Alice signs back in: her latest save, not her launch-time state.
    dragent_paths::select_organisation("alice-team");
    assert_eq!(restored_projects(), vec![alice_repo, alice_later]);
}
