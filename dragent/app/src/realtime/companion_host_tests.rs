use super::*;
use dragent_host::screen_protocol::{ScreenColor, ScreenCursorShape, ScreenRow, ScreenSpan};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Calls {
    inputs: Vec<String>,
    sizes: Vec<(u16, u16)>,
    reads: Vec<u64>,
    screens: usize,
}
struct FakeTerminal {
    calls: Mutex<Calls>,
    base: u64,
}
impl TerminalAuthority for FakeTerminal {
    fn stream(
        &self,
        _offset: u64,
        _on_chunk: Box<dyn FnMut(TerminalChunk) -> bool + Send + '_>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn screen(&self) -> Result<TerminalScreen, String> {
        self.calls.lock().unwrap().screens += 1;
        Ok(screen())
    }
    fn read(&self, offset: u64) -> Result<TerminalChunk, String> {
        self.calls.lock().unwrap().reads.push(offset);
        Ok(TerminalChunk {
            data: b"abc".to_vec(),
            base: self.base,
            total: self.base + 3,
            exit_code: None,
            resume: None,
            operations: None,
        })
    }
    fn input(&self, text: &str) -> Result<(), String> {
        self.calls.lock().unwrap().inputs.push(text.to_string());
        Ok(())
    }
    fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        self.calls.lock().unwrap().sizes.push((rows, cols));
        Ok(())
    }
}
fn screen() -> TerminalScreen {
    TerminalScreen {
        revision: 1,
        output_offset: 103,
        complete: true,
        rows: 2,
        cols: 3,
        cursor_row: 1,
        cursor_col: 2,
        cursor_visible: true,
        cursor_shape: ScreenCursorShape::Default,
        alternate_screen: false,
        mouse_reporting: false,
        sgr_mouse: false,
        bracketed_paste: false,
        parser_ground: true,
        parser_prefix: Vec::new(),
        scroll_region: Some((0, 1)),
        saved_cursor: None,
        pen: Default::default(),
        pending_wrap: None,
        modes: 0,
        title: String::new(),
        cwd: String::new(),
        lines: vec![
            ScreenRow {
                wrapped: false,
                spans: vec![ScreenSpan {
                    text: "error".into(),
                    foreground: ScreenColor::Default,
                    background: ScreenColor::Default,
                    flags: 0,
                    underline_color: None,
                }],
            },
            ScreenRow {
                wrapped: false,
                spans: vec![],
            },
        ],
    }
}
fn route(contributor: &str, id: u64, authority: Arc<dyn TerminalAuthority>) -> Route {
    Route {
        contributor: contributor.into(),
        wire_tab_id: id,
        workspace_id: "workspace".into(),
        terminal_session_id: "actual-host-pty".into(),
        theme: Theme::dark(),
        authority,
    }
}
fn setup() -> (Engine, Owner, Arc<FakeTerminal>, Arc<AtomicU64>) {
    let core = Arc::new(Core::new());
    let backend = Arc::new(FakeTerminal {
        calls: Mutex::new(Calls::default()),
        base: 100,
    });
    core.registry.lock().unwrap().note(
        "window-a".into(),
        AccountKey::new("account-a", "org-a"),
        vec![route("window-a", 7, backend.clone())],
    );
    let owner = core.current().unwrap();
    let now = Arc::new(AtomicU64::new(1000));
    let clock = now.clone();
    let mut engine = Engine::new(core);
    engine.now = Arc::new(move || clock.load(Ordering::Relaxed));
    engine.connection(1);
    (engine, owner, backend, now)
}
fn attach(engine: &Engine, owner: &Owner, companion: Uuid) -> Uuid {
    let handle = Uuid::new_v4();
    let result = engine
        .execute(
            owner,
            1,
            "terminal.attach",
            json!({"handle":handle,"session_id":companion,"tab_id":7,"expires_at":2000}),
        )
        .unwrap();
    assert_eq!(result["handle"], handle.to_string());
    assert_eq!(result["rows"], 2);
    assert_eq!(result["cols"], 3);
    handle
}

#[test]
fn registry_retires_windows_and_never_relabels_old_account_routes() {
    let (engine, owner, backend, _) = setup();
    let mut registry = engine.core.registry.lock().unwrap();
    let second = super::super::desktop_catalog::tab_wire_id(1, 7).unwrap();
    registry.note(
        "window-b".into(),
        owner.account.clone(),
        vec![route("window-b", second, backend)],
    );
    assert!(registry.route(&owner, 7).is_some());
    assert!(registry.route(&owner, second).is_some());
    registry.retire("window-a");
    assert!(registry.route(&owner, 7).is_none());
    assert!(registry.route(&owner, second).is_some());
    registry.note(
        "window-c".into(),
        AccountKey::new("account-b", "org-b"),
        vec![],
    );
    assert!(registry.route(&owner, second).is_none());
    assert_ne!(registry.current().unwrap().generation, owner.generation);
    registry.retire("window-c");
    assert!(registry.current().is_none());
}

#[test]
fn handles_are_bound_to_companion_expiry_and_current_socket() {
    let (engine, owner, backend, now) = setup();
    let companion = Uuid::new_v4();
    let handle = attach(&engine, &owner, companion);
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.input",
            json!({"handle":handle,"session_id":Uuid::new_v4(),"text":"no"})
        ),
        Err(Failure::Gone)
    );
    now.store(2000, Ordering::Relaxed);
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.input",
            json!({"handle":handle,"session_id":companion,"text":"no"})
        ),
        Err(Failure::Gone)
    );
    now.store(1000, Ordering::Relaxed);
    let handle = attach(&engine, &owner, companion);
    engine.connection(2);
    assert_eq!(
        engine.execute(
            &owner,
            2,
            "terminal.input",
            json!({"handle":handle,"session_id":companion,"text":"no"})
        ),
        Err(Failure::Gone)
    );
    assert!(backend.calls.lock().unwrap().inputs.is_empty());
}

#[test]
fn detach_frees_one_handle_without_a_daemon_call_or_pty_teardown() {
    let (engine, owner, backend, _) = setup();
    let companion = Uuid::new_v4();
    let mut handles = Vec::new();
    for _ in 0..4 {
        handles.push(attach(&engine, &owner, companion));
    }
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.attach",
            json!({"handle":Uuid::new_v4(),"session_id":companion,"tab_id":7,"expires_at":2000})
        ),
        Err(Failure::Unavailable)
    );
    assert_eq!(
        engine
            .execute(
                &owner,
                1,
                "terminal.detach",
                json!({"handle":handles[0],"session_id":companion})
            )
            .unwrap()["detached"],
        true
    );
    assert_eq!(backend.calls.lock().unwrap().screens, 4);
    attach(&engine, &owner, companion);
    assert_eq!(engine.state.lock().unwrap().handles.len(), 4);
}

#[test]
fn terminal_input_preserves_bytes_and_rejects_oversized_unicode() {
    let (engine, owner, backend, _) = setup();
    let companion = Uuid::new_v4();
    let handle = attach(&engine, &owner, companion);
    let text = "\u{1b}[Ahello😀";
    assert_eq!(
        engine
            .execute(
                &owner,
                1,
                "terminal.input",
                json!({"handle":handle,"session_id":companion,"text":text})
            )
            .unwrap()["sent"],
        true
    );
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.input",
            json!({"handle":handle,"session_id":companion,"text":"😀".repeat(4097)})
        ),
        Err(Failure::Invalid)
    );
    assert_eq!(backend.calls.lock().unwrap().inputs, vec![text.to_string()]);
}

#[test]
fn read_reports_real_ring_gap_and_resize_checks_grid_before_daemon() {
    let (engine, owner, backend, _) = setup();
    let companion = Uuid::new_v4();
    let handle = attach(&engine, &owner, companion);
    let result = engine
        .execute(
            &owner,
            1,
            "terminal.read",
            json!({"handle":handle,"session_id":companion,"offset":0}),
        )
        .unwrap();
    assert_eq!(result["base"], 100);
    assert_eq!(result["total"], 103);
    assert_eq!(result["data"], "YWJj");
    assert_eq!(result["alive"], true);
    assert_eq!(backend.calls.lock().unwrap().reads, vec![0]);
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.resize",
            json!({"handle":handle,"session_id":companion,"rows":240,"cols":512})
        ),
        Err(Failure::Invalid)
    );
    assert_eq!(
        engine
            .execute(
                &owner,
                1,
                "terminal.resize",
                json!({"handle":handle,"session_id":companion,"rows":30,"cols":100})
            )
            .unwrap()["resized"],
        true
    );
    assert_eq!(backend.calls.lock().unwrap().sizes, vec![(30, 100)]);
}

#[test]
fn retired_contributor_blocks_old_handles_without_rebinding_another_account() {
    let (engine, owner, backend, _) = setup();
    let companion = Uuid::new_v4();
    let handle = attach(&engine, &owner, companion);
    engine.core.registry.lock().unwrap().retire("window-a");
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.input",
            json!({"handle":handle,"session_id":companion,"text":"no"})
        ),
        Err(Failure::Gone)
    );
    engine.core.registry.lock().unwrap().note(
        "new".into(),
        AccountKey::new("account-b", "org-b"),
        vec![],
    );
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.input",
            json!({"handle":handle,"session_id":companion,"text":"no"})
        ),
        Err(Failure::Gone)
    );
    assert!(backend.calls.lock().unwrap().inputs.is_empty());
}

#[test]
fn screen_projection_reuses_wire_grid_and_incomplete_state_is_explicit() {
    let mut screen = screen();
    let theme = Theme::dark();
    let complete = screen_result(&screen, &theme).unwrap();
    assert_eq!(complete["complete"], true);
    assert_eq!(complete["delta"]["kind"], "alt_screen");
    assert_eq!(complete["delta"]["lines"][0]["spans"][0]["text"], "error");
    assert!(
        complete["delta"]["lines"][0]["spans"][0]
            .get("fg")
            .is_none(),
        "default foreground must not acquire prose-derived colors"
    );
    // Modes travel alongside the grid for the phone's input/scroll decisions.
    assert_eq!(complete["alternate_screen"], false);
    assert_eq!(complete["mouse_reporting"], false);
    assert_eq!(complete["sgr_mouse"], false);
    assert_eq!(complete["bracketed_paste"], false);
    screen.lines[0].spans[0].foreground = ScreenColor::Indexed(3);
    let colored = screen_result(&screen, &theme).unwrap();
    let color = theme.ansi(3);
    assert_eq!(
        colored["delta"]["lines"][0]["spans"][0]["fg"],
        json!([color.r, color.g, color.b, color.a])
    );
    screen.complete = false;
    let incomplete = screen_result(&screen, &theme).unwrap();
    assert_eq!(incomplete["complete"], false);
    assert!(incomplete["delta"].is_null());
    // Resync still reports modes without a grid.
    assert_eq!(incomplete["alternate_screen"], false);
    assert_eq!(incomplete["mouse_reporting"], false);
    assert_eq!(incomplete["sgr_mouse"], false);
    assert_eq!(incomplete["bracketed_paste"], false);
}

#[test]
fn attach_and_screen_report_terminal_modes() {
    let (engine, owner, _, _) = setup();
    let companion = Uuid::new_v4();
    let handle = attach(&engine, &owner, companion);
    // Attach helper already checked grid; modes default off for the fake.
    let attach_result = engine
        .execute(
            &owner,
            1,
            "terminal.screen",
            json!({"handle":handle,"session_id":companion}),
        )
        .unwrap();
    assert_eq!(attach_result["alternate_screen"], false);
    assert_eq!(attach_result["mouse_reporting"], false);
    assert_eq!(attach_result["sgr_mouse"], false);
    assert_eq!(attach_result["bracketed_paste"], false);
}

#[test]
fn unknown_methods_extra_fields_and_unsafe_offsets_are_rejected() {
    let (engine, owner, _, _) = setup();
    let companion = Uuid::new_v4();
    let handle = attach(&engine, &owner, companion);
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.close",
            json!({"handle":handle,"session_id":companion})
        ),
        Err(Failure::Invalid)
    );
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.input",
            json!({"handle":handle,"session_id":companion,"text":"a","workspace_id":"foreign"})
        ),
        Err(Failure::Invalid)
    );
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.read",
            json!({"handle":handle,"session_id":companion,"offset":MAX_SAFE_INTEGER+1})
        ),
        Err(Failure::Invalid)
    );
    let frame = reply(Uuid::new_v4(), Err(Failure::Unavailable));
    assert!(!frame.contains("workspace"));
    assert!(frame.contains("unavailable"));
}

#[test]
fn coordination_rejects_a_retired_socket_before_dispatch() {
    let (engine, owner, _, _) = setup();
    engine.core.registry.lock().unwrap().retire("window-a");
    for method in ["coordination.list", "coordination.get"] {
        assert_eq!(
            engine.execute(&owner, 1, method, json!({})),
            Err(Failure::Gone)
        );
    }
}

struct StreamTerminal {
    snapshot: TerminalScreen,
    initial: Mutex<Option<TerminalChunk>>,
    chunks: Mutex<Vec<TerminalChunk>>,
    starts: Mutex<Vec<u64>>,
}
impl TerminalAuthority for StreamTerminal {
    fn screen(&self) -> Result<TerminalScreen, String> {
        Ok(self.snapshot.clone())
    }
    fn read(&self, _: u64) -> Result<TerminalChunk, String> {
        self.initial
            .lock()
            .unwrap()
            .take()
            .ok_or("unexpected read".into())
    }
    fn input(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn resize(&self, _: u16, _: u16) -> Result<(), String> {
        Ok(())
    }
    fn stream(
        &self,
        offset: u64,
        mut cb: Box<dyn FnMut(TerminalChunk) -> bool + Send + '_>,
    ) -> Result<(), String> {
        self.starts.lock().unwrap().push(offset);
        for chunk in self.chunks.lock().unwrap().drain(..) {
            if !cb(chunk) {
                break;
            }
        }
        Ok(())
    }
}
fn chunk(base: u64, data: &[u8], exit_code: Option<i32>) -> TerminalChunk {
    TerminalChunk {
        base,
        total: base + data.len() as u64,
        data: data.to_vec(),
        exit_code,
        resume: None,
        operations: None,
    }
}
fn stream_case(
    offset: Option<u64>,
    initial: Option<TerminalChunk>,
    complete: bool,
) -> (Vec<Value>, Vec<u64>, &'static str) {
    let (engine, owner, _, _) = setup();
    let mut snapshot = screen();
    snapshot.lines[0].spans[0].text = "abc".into();
    snapshot.complete = complete;
    let backend = Arc::new(StreamTerminal {
        snapshot,
        initial: Mutex::new(initial),
        chunks: Mutex::new(vec![chunk(103, b"x", Some(0))]),
        starts: Mutex::new(vec![]),
    });
    engine.core.registry.lock().unwrap().note(
        "window-a".into(),
        owner.account.clone(),
        vec![route("window-a", 7, backend.clone())],
    );
    let id = attach(&engine, &owner, Uuid::new_v4());
    let stop = Arc::new(AtomicBool::new(false));
    engine.state.lock().unwrap().subscriptions.insert(
        id,
        Subscription {
            generation: 1,
            stop: stop.clone(),
            stalled: false,
        },
    );
    let rx = StreamEvents::new(MAX_EVENTS);
    engine.set_events(rx.clone());
    let handle = engine.state.lock().unwrap().handles[&id].clone();
    let reason = engine.stream_subscription(&owner, 1, id, 1, &handle, offset, &stop);
    let mut frames = Vec::new();
    while let Ok(e) = rx.try_recv() {
        frames.push(e.frame["payload"].clone());
    }
    let starts = backend.starts.lock().unwrap().clone();
    (frames, starts, reason)
}

#[test]
fn companion_stream_sends_empty_initial_and_final_output() {
    let (frames, starts, reason) = stream_case(Some(103), Some(chunk(103, b"", None)), true);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["data"], "");
    assert_eq!(frames[0]["reset"], false);
    assert_eq!(frames[1]["alive"], false);
    assert_eq!(frames[1]["exit_code"], 0);
    assert_eq!(starts, [103]);
    assert_eq!(reason, "exited");
}

#[test]
fn companion_stream_seeds_missing_evicted_and_future_offsets() {
    for (offset, initial) in [
        (None, None),
        (Some(0), Some(chunk(100, b"abc", None))),
        (Some(999), Some(chunk(103, b"", None))),
    ] {
        let (frames, starts, reason) = stream_case(offset, initial, true);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["reset"], true);
        assert_eq!(frames[0]["base"], 103);
        assert_eq!(frames[1]["base"], 103);
        assert_eq!(starts, [103]);
        assert_eq!(reason, "exited");
    }
}

#[test]
fn companion_stream_fails_incomplete_seed_without_sending_output() {
    let (frames, starts, reason) = stream_case(None, None, false);
    assert!(frames.is_empty());
    assert!(starts.is_empty());
    assert_eq!(reason, "failed");
}

#[test]
fn companion_stream_splits_raw_but_keeps_seed_atomic_and_exit_last() {
    let (engine, owner, _, _) = setup();
    let rx = StreamEvents::new(MAX_EVENTS);
    engine.set_events(rx.clone());
    let id = attach(&engine, &owner, Uuid::new_v4());
    engine.state.lock().unwrap().subscriptions.insert(
        id,
        Subscription {
            generation: 1,
            stop: Arc::new(AtomicBool::new(false)),
            stalled: false,
        },
    );
    engine.send_output(
        owner.clone(),
        1,
        id,
        1,
        10,
        10 + (MAX_READ * 2) as u64,
        vec![b'x'; MAX_READ * 2],
        false,
        0,
        0,
        false,
        Some(2),
    );
    let first = rx.try_recv().unwrap().frame;
    let last = rx.try_recv().unwrap().frame;
    assert_eq!(first["payload"]["alive"], true);
    assert!(first["payload"].get("exit_code").is_none());
    assert_eq!(last["payload"]["alive"], false);
    assert_eq!(last["payload"]["base"], 10 + MAX_READ as u64);
    engine.send_output(
        owner.clone(),
        1,
        id,
        1,
        10,
        10,
        vec![b'x'; MAX_READ * 2],
        true,
        24,
        80,
        true,
        None,
    );
    assert_eq!(rx.try_recv().unwrap().frame["payload"]["base"], 10);
    assert!(rx.try_recv().is_err());
    engine.send_output(owner, 1, id, 1, 10, 10, vec![], false, 0, 0, false, Some(0));
    assert_eq!(rx.try_recv().unwrap().frame["payload"]["alive"], false);
}

#[test]
fn companion_stream_checks_session_and_bounds_cancelled_workers() {
    let (engine, owner, _, _) = setup();
    let session = Uuid::new_v4();
    let id = attach(&engine, &owner, session);
    for method in ["terminal.subscribe", "terminal.unsubscribe"] {
        assert_eq!(
            engine.execute(
                &owner,
                1,
                method,
                json!({"handle":id,"session_id":Uuid::new_v4()})
            ),
            Err(Failure::Gone)
        );
    }
    engine.state.lock().unwrap().workers = MAX_HANDLES;
    assert_eq!(
        engine.execute(
            &owner,
            1,
            "terminal.subscribe",
            json!({"handle":id,"session_id":session})
        ),
        Err(Failure::Unavailable)
    );
}

#[test]
fn companion_replay_window_rolls_without_losing_recent_duplicate_protection() {
    let mut window = ReplayWindow::default();
    let first = Uuid::new_v4();
    assert!(window.insert(first));
    assert!(!window.insert(first));
    let mut last = first;
    for _ in 0..70_000 {
        last = Uuid::new_v4();
        assert!(window.insert(last));
    }
    assert_eq!(window.ids.len(), 32768);
    assert_eq!(window.order.len(), 32768);
    assert!(!window.insert(last));
    assert!(window.insert(first));
}

#[test]
fn companion_cancel_invalidates_generation_and_stall_timeout_is_explicit() {
    let (engine, owner, _, _) = setup();
    let session = Uuid::new_v4();
    let id = attach(&engine, &owner, session);
    let stop = Arc::new(AtomicBool::new(false));
    engine.state.lock().unwrap().subscriptions.insert(
        id,
        Subscription {
            generation: 7,
            stop: stop.clone(),
            stalled: false,
        },
    );
    assert!(engine.is_generation(id, 7));
    engine
        .execute(
            &owner,
            1,
            "terminal.unsubscribe",
            json!({"handle":id,"session_id":session}),
        )
        .unwrap();
    assert!(stop.load(Ordering::Acquire));
    assert!(!engine.is_generation(id, 7));
    let stop = Arc::new(AtomicBool::new(false));
    engine.state.lock().unwrap().subscriptions.insert(
        id,
        Subscription {
            generation: 8,
            stop: stop.clone(),
            stalled: false,
        },
    );
    engine.set_events(StreamEvents::new(1));
    let blocked = Instant::now();
    for _ in 0..2 {
        engine.send_output(
            owner.clone(),
            1,
            id,
            8,
            0,
            1,
            vec![b'x'],
            false,
            0,
            0,
            true,
            None,
        );
    }
    assert!(stop.load(Ordering::Acquire));
    assert!(engine.state.lock().unwrap().subscriptions[&id].stalled);
    assert!(blocked.elapsed() >= EVENT_STALL);
    assert!(blocked.elapsed() < EVENT_STALL + Duration::from_secs(1));
    engine.connection(2);
    assert!(!engine.is_generation(id, 8));
}

#[test]
fn companion_slow_socket_drains_1000_frames_in_order_without_ending() {
    let (engine, owner, _, _) = setup();
    let id = attach(&engine, &owner, Uuid::new_v4());
    let stop = Arc::new(AtomicBool::new(false));
    engine.state.lock().unwrap().subscriptions.insert(
        id,
        Subscription {
            generation: 1,
            stop: stop.clone(),
            stalled: false,
        },
    );
    let queue = StreamEvents::new(MAX_EVENTS);
    engine.set_events(queue.clone());
    let engine = Arc::new(engine);
    let producer = engine.clone();
    let producer_owner = owner.clone();
    let writer = std::thread::spawn(move || {
        for base in 0..1000 {
            producer.send_output(
                producer_owner.clone(),
                1,
                id,
                1,
                base,
                base + 1,
                vec![b'x'],
                false,
                0,
                0,
                true,
                None,
            );
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    // Hold the socket loop until the queue fills, proving real backpressure.
    while queue.frames.lock().unwrap().len() < MAX_EVENTS {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!writer.is_finished());
    for base in 0..1000 {
        let event = loop {
            if let Ok(event) = queue.try_recv() {
                break event;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        };
        engine
            .deliver_event(event, &owner, 1, |frame| {
                assert_eq!(frame["event"], "terminal.output");
                assert_eq!(frame["payload"]["base"], base);
                assert_eq!(frame["payload"]["data"], "eA==");
                // Fake socket loop drains more slowly than the PTY producer.
                std::thread::sleep(Duration::from_millis(1));
                Ok(())
            })
            .unwrap();
    }
    writer.join().unwrap();
    assert!(queue.try_recv().is_err());
    assert!(!stop.load(Ordering::Acquire));
    assert!(!engine.state.lock().unwrap().subscriptions[&id].stalled);
}

#[test]
fn companion_unsubscribe_releases_a_blocked_producer() {
    let (engine, owner, _, _) = setup();
    let session = Uuid::new_v4();
    let id = attach(&engine, &owner, session);
    engine.state.lock().unwrap().subscriptions.insert(
        id,
        Subscription {
            generation: 1,
            stop: Arc::new(AtomicBool::new(false)),
            stalled: false,
        },
    );
    let queue = StreamEvents::new(1);
    engine.set_events(queue.clone());
    let engine = Arc::new(engine);
    let producer = engine.clone();
    let producer_owner = owner.clone();
    let writer = std::thread::spawn(move || {
        for base in 0..2 {
            producer.send_output(
                producer_owner.clone(),
                1,
                id,
                1,
                base,
                base + 1,
                vec![b'x'],
                false,
                0,
                0,
                true,
                None,
            );
        }
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while queue.frames.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    engine
        .execute(
            &owner,
            1,
            "terminal.unsubscribe",
            json!({"handle":id,"session_id":session}),
        )
        .unwrap();
    writer.join().unwrap();
    assert!(Instant::now() < deadline);
    assert_eq!(queue.frames.lock().unwrap().len(), 1);
}

/// Run alone with DRAGENT_COMPANION_TEST_HOST pointing to a built host binary.
/// The daemon and terminal are isolated; the relay is a local WebSocket sink.
#[test]
#[ignore = "requires an isolated real daemon executable; run with --test-threads=1"]
fn companion_real_daemon_input_to_host_event_latency() {
    let executable = std::env::var("DRAGENT_COMPANION_TEST_HOST").expect("host binary path");
    let base = std::env::temp_dir().join(format!("companion-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    struct Cleanup {
        base: std::path::PathBuf,
        env: Vec<(&'static str, Option<std::ffi::OsString>)>,
        client: Option<HostClient>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(client) = &self.client {
                let _ = client.shutdown();
            }
            for (name, value) in &self.env {
                if let Some(value) = value {
                    std::env::set_var(name, value);
                } else {
                    std::env::remove_var(name);
                }
            }
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }
    let mut cleanup = Cleanup {
        base: base.clone(),
        env: vec![],
        client: None,
    };
    for (name, path) in [
        (dragent_host::ENDPOINT_FILE_ENV, base.join("endpoint.json")),
        ("DRAGENT_WORKSPACE_DB", base.join("db.sqlite")),
        ("DRAGENT_HOME", base.join("data")),
        ("ZDOTDIR", base.clone()),
    ] {
        cleanup.env.push((name, std::env::var_os(name)));
        std::env::set_var(name, path);
    }
    let client = HostClient::ensure_host_with(std::path::Path::new(&executable)).unwrap();
    cleanup.client = Some(client.clone());
    let home = client
        .ensure_home("latency", base.to_str().unwrap())
        .unwrap();
    let terminal = client
        .create_terminal(
            "latency",
            &home.id,
            Some("stty -icanon -echo; printf STREAM_READY; cat"),
            None,
            80,
            24,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let c = client
            .read_terminal_chunk("latency", &terminal.id, 0, MAX_READ)
            .unwrap();
        if String::from_utf8_lossy(&c.data).contains("STREAM_READY") {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let core = Arc::new(Core::new());
    let account = AccountKey::new("test", "test");
    core.registry.lock().unwrap().note(
        "window".into(),
        account,
        vec![route(
            "window",
            7,
            Arc::new(DaemonTerminal {
                client: client.clone(),
                workspace: "latency".into(),
                session: terminal.id.clone(),
            }),
        )],
    );
    let owner = core.current().unwrap();
    let engine = Arc::new(Engine::new(core));
    engine.connection(1);
    let rx = StreamEvents::new(MAX_EVENTS);
    engine.set_events(rx.clone());
    let id = Uuid::new_v4();
    let session = Uuid::new_v4();
    engine
        .execute(
            &owner,
            1,
            "terminal.attach",
            json!({"handle":id,"session_id":session,"tab_id":7,"expires_at":epoch_ms()+60_000}),
        )
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (received_tx, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(tcp).unwrap();
        while let Ok(tungstenite::Message::Text(text)) = socket.read() {
            let _ = received_tx.send((
                Instant::now(),
                serde_json::from_str::<Value>(&text).unwrap(),
            ));
        }
    });
    let mut socket = super::super::dial(&format!("ws://{address}/test"), "test").unwrap();
    super::super::set_socket_read_timeout(&mut socket, Duration::from_millis(2));
    let writer_stop = Arc::new(AtomicBool::new(false));
    let stop = writer_stop.clone();
    let writer_engine = engine.clone();
    let writer_owner = owner.clone();
    let writer = std::thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            for _ in 0..32 {
                let Ok(event) = rx.try_recv() else {
                    break;
                };
                writer_engine
                    .deliver_event(event, &writer_owner, 1, |frame| {
                        socket
                            .send(tungstenite::Message::Text(frame.to_string().into()))
                            .map_err(|_| ())
                    })
                    .unwrap();
            }
            // Exercise the same real socket read timeout as the relay loop.
            match socket.read() {
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                other => panic!("unexpected relay read: {other:?}"),
            }
        }
        let _ = socket.close(None);
    });
    engine
        .execute(
            &owner,
            1,
            "terminal.subscribe",
            json!({"handle":id,"session_id":session}),
        )
        .unwrap();
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap().1["payload"]["reset"],
        true
    );
    let mut samples = vec![];
    for key in b"abcdefghij".iter().cycle().take(60) {
        let start = Instant::now();
        engine
            .execute(
                &owner,
                1,
                "terminal.input",
                json!({"handle":id,"session_id":session,"text":(*key as char).to_string()}),
            )
            .unwrap();
        loop {
            let (at, frame) = received.recv_timeout(Duration::from_secs(5)).unwrap();
            let data = base64::engine::general_purpose::STANDARD
                .decode(frame["payload"]["data"].as_str().unwrap())
                .unwrap();
            if data.contains(key) {
                samples.push(at.duration_since(start));
                break;
            }
        }
    }
    samples.sort();
    eprintln!(
        "input to host-event on local WebSocket: p50 {:?}, p95 {:?}, max {:?}, n={}",
        samples[29],
        samples[56],
        samples[59],
        samples.len()
    );
    assert!(samples[56] < Duration::from_millis(100));
    engine
        .execute(
            &owner,
            1,
            "terminal.unsubscribe",
            json!({"handle":id,"session_id":session}),
        )
        .unwrap();
    let cancelled = Instant::now();
    while engine.state.lock().unwrap().workers != 0 {
        assert!(
            cancelled.elapsed() < Duration::from_secs(46),
            "idle stream did not stop"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!(
        "idle cancelled stream stopped after {:?}",
        cancelled.elapsed()
    );
    writer_stop.store(true, Ordering::Release);
    writer.join().unwrap();
    reader.join().unwrap();
    client.close_terminal("latency", &terminal.id).unwrap();
}

#[test]
fn companion_stale_events_never_reach_socket_after_replace_detach_or_expiry() {
    let (engine, owner, _, now) = setup();
    let session = Uuid::new_v4();
    let id = attach(&engine, &owner, session);
    let stop = Arc::new(AtomicBool::new(false));
    engine.state.lock().unwrap().subscriptions.insert(
        id,
        Subscription {
            generation: 2,
            stop: stop.clone(),
            stalled: false,
        },
    );
    let event = |generation| StreamEvent {
        owner: owner.clone(),
        connection: 1,
        handle: id,
        generation,
        frame: json!({"event":"terminal.output"}),
    };
    engine
        .deliver_event(event(1), &owner, 1, |_| panic!("old generation delivered"))
        .unwrap();
    let mut sent = false;
    engine
        .deliver_event(event(2), &owner, 1, |_| {
            sent = true;
            Ok(())
        })
        .unwrap();
    assert!(sent);
    now.store(2000, Ordering::Release);
    engine
        .deliver_event(event(2), &owner, 1, |_| panic!("expired delivered"))
        .unwrap();
    now.store(1000, Ordering::Release);
    engine
        .execute(
            &owner,
            1,
            "terminal.detach",
            json!({"handle":id,"session_id":session}),
        )
        .unwrap();
    assert!(stop.load(Ordering::Acquire));
    engine
        .deliver_event(event(2), &owner, 1, |_| panic!("detached delivered"))
        .unwrap();
}

#[derive(Default)]
struct FakeOpener {
    calls: Mutex<Vec<(AccountKey, usize, NewTabKind)>>,
    result: Mutex<Option<Result<u64, Failure>>>,
    /// Run while "on the UI thread", e.g. an account switch mid-open.
    during: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl TabOpener for FakeOpener {
    fn open(
        &self,
        account: &AccountKey,
        workspace_id: usize,
        kind: NewTabKind,
    ) -> Result<u64, Failure> {
        self.calls
            .lock()
            .unwrap()
            .push((account.clone(), workspace_id, kind));
        if let Some(during) = self.during.lock().unwrap().take() {
            during();
        }
        self.result.lock().unwrap().take().unwrap_or(Ok(5))
    }
}
fn new_tab_setup() -> (Engine, Owner, Arc<FakeOpener>) {
    let (engine, owner, _, _) = setup();
    engine.core.registry.lock().unwrap().note_workspaces(
        "window-a",
        &owner.account,
        2,
        vec![("w2w3".into(), 3)],
    );
    let opener = Arc::new(FakeOpener::default());
    engine
        .core
        .openers
        .lock()
        .unwrap()
        .insert("window-a".into(), opener.clone());
    (engine, owner, opener)
}
fn new_tab(engine: &Engine, owner: &Owner, params: Value) -> Result<Value, Failure> {
    engine.execute(owner, 1, "workspace.newTab", params)
}

#[test]
fn new_tab_opens_in_the_owning_window_and_answers_the_wire_id() {
    let (engine, owner, opener) = new_tab_setup();
    for (kind, expected) in [
        ("terminal", NewTabKind::Terminal),
        ("browser", NewTabKind::Browser),
        ("dragent_cli", NewTabKind::DragentCli),
    ] {
        let result = new_tab(&engine, &owner, json!({"workspace_id":"w2w3","kind":kind})).unwrap();
        assert_eq!(
            result,
            json!({"id": super::super::desktop_catalog::tab_wire_id(2, 5).unwrap()})
        );
        let call = opener.calls.lock().unwrap().pop().unwrap();
        assert_eq!(call, (owner.account.clone(), 3, expected));
    }
}

#[test]
fn new_tab_rejects_bad_params_before_the_window() {
    let (engine, owner, opener) = new_tab_setup();
    for params in [
        json!({"workspace_id":"w2w3","kind":"agent_coordination"}),
        json!({"workspace_id":"w2w3","kind":"Terminal"}),
        json!({"workspace_id":"w2w3"}),
        json!({"kind":"terminal"}),
        json!({"workspace_id":"","kind":"terminal"}),
        json!({"workspace_id":"../w2w3","kind":"terminal"}),
        json!({"workspace_id":"w".repeat(65),"kind":"terminal"}),
        json!({"workspace_id":"w2w3","kind":"terminal","command":"rm -rf /"}),
        json!({"workspace_id":3,"kind":"terminal"}),
    ] {
        assert_eq!(new_tab(&engine, &owner, params), Err(Failure::Invalid));
    }
    assert!(opener.calls.lock().unwrap().is_empty());
}

#[test]
fn new_tab_only_reaches_workspaces_this_account_published() {
    let (engine, owner, opener) = new_tab_setup();
    assert_eq!(
        new_tab(&engine, &owner, json!({"workspace_id":"w2w4","kind":"terminal"})),
        Err(Failure::Gone)
    );
    // Another account's view can't publish workspaces for this owner.
    {
        let mut registry = engine.core.registry.lock().unwrap();
        registry.note_workspaces(
            "window-a",
            &AccountKey::new("account-b", "org-b"),
            2,
            vec![("w2w9".into(), 9)],
        );
        // Nor can a window that was never noted under it.
        registry.note_workspaces("window-z", &owner.account, 7, vec![("w7w1".into(), 1)]);
    }
    for id in ["w2w9", "w7w1"] {
        assert_eq!(
            new_tab(&engine, &owner, json!({"workspace_id":id,"kind":"terminal"})),
            Err(Failure::Gone)
        );
    }
    // A stale owner (account changed since) gets nothing.
    engine.core.registry.lock().unwrap().note(
        "window-a".into(),
        AccountKey::new("account-b", "org-b"),
        vec![],
    );
    assert_eq!(
        new_tab(&engine, &owner, json!({"workspace_id":"w2w3","kind":"terminal"})),
        Err(Failure::Gone)
    );
    assert!(opener.calls.lock().unwrap().is_empty());
}

#[test]
fn new_tab_retired_window_and_missing_listener_fail_cleanly() {
    let (engine, owner, opener) = new_tab_setup();
    engine.core.openers.lock().unwrap().clear();
    assert_eq!(
        new_tab(&engine, &owner, json!({"workspace_id":"w2w3","kind":"browser"})),
        Err(Failure::Unavailable)
    );
    engine
        .core
        .openers
        .lock()
        .unwrap()
        .insert("window-a".into(), opener.clone());
    *opener.result.lock().unwrap() = Some(Err(Failure::Gone));
    assert_eq!(
        new_tab(&engine, &owner, json!({"workspace_id":"w2w3","kind":"browser"})),
        Err(Failure::Gone)
    );
    // Workspaces leave with their window.
    let mut registry = engine.core.registry.lock().unwrap();
    registry.note("window-b".into(), owner.account.clone(), vec![]);
    registry.retire("window-a");
    drop(registry);
    assert_eq!(
        new_tab(&engine, &owner, json!({"workspace_id":"w2w3","kind":"browser"})),
        Err(Failure::Gone)
    );
}

#[test]
fn new_tab_answer_is_dropped_when_the_account_changes_mid_open() {
    let (engine, owner, opener) = new_tab_setup();
    let core = engine.core.clone();
    *opener.during.lock().unwrap() = Some(Box::new(move || {
        core.registry.lock().unwrap().note(
            "window-a".into(),
            AccountKey::new("account-b", "org-b"),
            vec![],
        );
    }));
    assert_eq!(
        new_tab(&engine, &owner, json!({"workspace_id":"w2w3","kind":"terminal"})),
        Err(Failure::Gone)
    );
}

#[test]
fn window_opener_answers_from_the_ui_side_and_fails_when_closed() {
    let (requests, receiver) = async_channel::bounded(8);
    let opener = WindowOpener { requests };
    let account = AccountKey::new("account-a", "org-a");
    let ui = std::thread::spawn(move || {
        let request = receiver.recv_blocking().unwrap();
        assert_eq!(request.workspace_id, 3);
        assert_eq!(request.kind, NewTabKind::DragentCli);
        request.answer(Ok(11));
    });
    assert_eq!(opener.open(&account, 3, NewTabKind::DragentCli), Ok(11));
    ui.join().unwrap();
    assert!(!opener.live());
    assert_eq!(
        opener.open(&account, 3, NewTabKind::Terminal),
        Err(Failure::Unavailable)
    );
}

#[test]
fn host_auth_keeps_token_inband_and_adds_e2e_marker() {
    let auth = super::host_auth_json("tok-abc", "dev-1", "user-1");
    assert_eq!(auth["type"], "relay-host-auth");
    assert_eq!(auth["v"], 1);
    assert_eq!(auth["e2e"], 1);
    assert_eq!(auth["access_token"], "tok-abc");
    assert_eq!(auth["device_id"], "dev-1");
    assert_eq!(auth["expected_user_id"], "user-1");
    // No URL token: the token only rides inside this first socket message.
    let s = auth.to_string();
    assert!(s.contains("tok-abc"));
}

#[test]
fn host_notify_validates_phone_and_blob() {
    let _ = super::queued_notifies_for_test();
    let blob = "A".repeat(1494);
    assert!(super::send_host_notify("phone-1", &blob).is_ok());
    assert!(super::send_host_notify("", &blob).is_err());
    assert!(super::send_host_notify("phone-1", "too-short").is_err());
    assert!(super::send_host_notify("phone-1", &"B".repeat(1495)).is_err());
    assert!(super::send_host_notify("phone-1", &"!".repeat(1494)).is_err());
    let queued = super::queued_notifies_for_test();
    assert_eq!(queued, vec![("phone-1".to_string(), blob)]);
    // Invalid intake never queues (the relay would count-and-drop it).
    assert!(super::queued_notifies_for_test().is_empty());
}
