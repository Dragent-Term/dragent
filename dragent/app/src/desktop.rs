use anyhow::Result;
use dragent_assets::DragentAssets;
use dragent_persistence::warm_in_background;
use warpui::platform;
use warpui_core::platform::app::AppCallbacks;

use crate::auth::{handle_open_url, register_scheme_handler, AuthManager};
use crate::settings::AccountStore;
// This module was the crate root (main.rs); bring the crate's modules into scope
// so its bare `agent::`, `workspace::`, `root_view::`, … paths still resolve.
use crate::{
    agent, api, auth, automation, autoupdate, confirm, console, git_status,
    process_ext, realtime, root_view, settings, terminal, theme, voice, workspace,
};

// ── Big-allocation tracer ────────────────────────────────────────────────────
// Wraps the system allocator and, when DRAGENT_TRACE_BIG_ALLOC is set, logs a
// backtrace for any single alloc/realloc over the threshold. A transient
// multi-GB RSS spike (that gets the app OOM-killed) comes from a large buffer or
// a Vec growing huge — both show up here as one big (re)alloc — so this names the
// exact call site with zero external profiling. The thread-local reentrancy guard
// stops the backtrace's own allocations from recursing.
mod big_alloc_trace {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU8, Ordering};

    const THRESHOLD: usize = 128 * 1024 * 1024; // 128 MB
    static ENABLED: AtomicU8 = AtomicU8::new(0); // 0=unknown, 1=on, 2=off

    pub fn init() {
        let on = std::env::var_os("DRAGENT_TRACE_BIG_ALLOC").is_some();
        ENABLED.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    }

    thread_local! {
        static IN_TRACE: Cell<bool> = const { Cell::new(false) };
    }

    fn report(kind: &str, size: usize) {
        if ENABLED.load(Ordering::Relaxed) != 1 || size < THRESHOLD {
            return;
        }
        IN_TRACE.with(|f| {
            if f.get() {
                return;
            }
            f.set(true);
            let bt = std::backtrace::Backtrace::force_capture();

            let ours: Vec<String> = format!("{bt}")
                .lines()
                .filter(|l| {
                    let ours = l.contains("dragent")
                        || l.contains("warpui")
                        || l.contains("app/src/")
                        || l.contains("crates/dragent");
                    ours && !l.contains("/rustc/")
                        && !l.contains("/rustlib/")
                        && !l.contains("big_alloc_trace")
                        && !l.contains("main.rs")
                })
                .take(30)
                .map(|s| s.trim().to_string())
                .collect();
            eprintln!(
                "\n[BIG_ALLOC] {kind} {} MB\n  {}\n",
                size / (1024 * 1024),
                ours.join("\n  ")
            );
            f.set(false);
        });
    }

    pub struct TracingAlloc;

    unsafe impl GlobalAlloc for TracingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            report("alloc", layout.size());
            System.alloc(layout)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout)
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            report("realloc", new_size);
            System.realloc(ptr, layout, new_size)
        }
    }
}

// Experimental allocator swap, off by default (`--features mimalloc`): the
// binary links exactly one global allocator, so the tracer is compiled out
// while mimalloc is in. This module is native-only; the wasm viewer never
// sets the feature.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(feature = "mimalloc"))]
#[global_allocator]
static GLOBAL: big_alloc_trace::TracingAlloc = big_alloc_trace::TracingAlloc;

/// Parse a shared-session join link into `(session, secret)`. Accepts this
/// edition's `<scheme>://join?session=…&secret=…` deep link and a `…/join?…`
/// web URL. Links for the other edition's scheme are rejected: they belong
/// to the other app install.
fn parse_join_url(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url).ok()?;
    let scheme = parsed.scheme();
    let is_deep_link =
        parsed.host_str() == Some("join") && scheme == dragent_paths::url_scheme();
    let is_web =
        (scheme == "https" || scheme == "http") && parsed.path().trim_end_matches('/').ends_with("/join");
    if !(is_deep_link || is_web) {
        return None;
    }
    let mut session = None;
    let mut secret = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "session" => session = Some(v.into_owned()),
            "secret" => secret = Some(v.into_owned()),
            _ => {}
        }
    }
    Some((session?, secret?))
}

fn init_logging() {
    // Baseline: warn for ALL crates so noisy third-party libs (keyring, ureq,
    // ignore, globset, etc.) stay silent. Our own crates get info.
    // RUST_LOG appended after so the user can still override specific targets,
    // e.g.  RUST_LOG=dragent=debug  or  RUST_LOG=ureq=debug.
    let base = "warn,dragent=info,dragent_persistence=info,warpui=info";

    let mut builder = env_logger::Builder::new();
    builder.parse_filters(base);
    if let Ok(user) = std::env::var("RUST_LOG") {
        if !user.is_empty() {
            builder.parse_filters(&user);
        }
    }
    builder.format_timestamp_millis();
    // Same stderr output, plus a bounded in-memory copy that Give feedback
    // can attach (redacted) when the user opts in.
    builder.target(env_logger::Target::Pipe(Box::new(
        crate::diagnostics::TeeStderr::default(),
    )));
    if builder.try_init().is_ok() {
        log::info!(
            "[dragent] logging initialised (RUST_LOG overrides, e.g. RUST_LOG=dragent=debug)"
        );
    }
}

/// Select the process-wide rustls crypto provider.
///
/// Both `aws-lc-rs` and `ring` reach rustls through our dependency graph, and
/// when it sees two candidates rustls refuses to guess: the first TLS handshake
/// *panics* rather than returning an error. That killed the Deepgram voice
/// thread mid-handshake — and because a panicking thread just drops its event
/// channel, the UI saw no error at all, only a session that never went live.
///
/// Do this before any TLS work. Losing the race to another installer is fine;
/// the outcome is a provider either way, which is all we need.
fn init_tls_provider() {
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        log::debug!("[dragent] rustls crypto provider already installed");
    }
}

/// Open a new top-level OS window with a fresh, empty workspace. The framework
/// supports any number of windows; each gets its own RootView → WorkspaceView
/// (own tabs/sessions), while singleton models (auth, account, fonts) are shared.
/// Called from the New Window keybinding (via WorkspaceAction::NewWindow) and the
/// OS "new window" request (dock icon / macOS reopen).
pub fn open_new_window(ctx: &mut warpui::AppContext) {
    ctx.add_window(
        warpui::AddWindowOptions::default(),
        root_view::RootView::new_empty,
    );
}

/// Static "no-op" updater for custom menu items that never change.
#[cfg(target_os = "macos")]
fn menu_no_updates(
    _: &warpui::platform::menu::MenuItemProperties,
    _: &mut warpui::AppContext,
) -> warpui::platform::menu::MenuItemPropertyChanges {
    Default::default()
}

/// The macOS Dock right-click (application) menu. Provides "New Window"; macOS
/// prepends the list of open windows (by title) above it — but only because the
/// menu bar below registers a Window menu (`NSApp.windowsMenu`), which is what
/// feeds that list.
#[cfg(target_os = "macos")]
fn dock_menu() -> warpui::platform::menu::Menu {
    use warpui::keymap::Keystroke;
    use warpui::platform::menu::{CustomMenuItem, Menu, MenuItem};

    Menu::new(
        "Dock",
        vec![MenuItem::Custom(CustomMenuItem::new(
            "New Window",
            |ctx| open_new_window(ctx),
            menu_no_updates,
            Keystroke::parse("cmd-n").ok(),
        ))],
    )
}

/// The macOS application menu bar. Besides the usual app / File / Window commands,
/// the **"Window"** menu is the important bit: its title makes the framework mark
/// it as `NSApp.windowsMenu` (`setWindowsMenu`), which macOS auto-populates with
/// every open window — and that same list is what appears at the top of the Dock
/// menu. Each window's entry uses the title we set in `WorkspaceView`.
#[cfg(target_os = "macos")]
fn menu_bar() -> warpui::platform::menu::MenuBar {
    use warpui::keymap::Keystroke;
    use warpui::platform::menu::{CustomMenuItem, Menu, MenuBar, MenuItem};
    use warpui_core::actions::StandardAction as SA;

    // macOS treats the first menu as the application menu (shown under the app name).
    let app_menu = Menu::new(
        "Dragent",
        vec![
            MenuItem::Services,
            MenuItem::Separator,
            MenuItem::Standard(SA::Hide),
            MenuItem::Standard(SA::HideOtherApps),
            MenuItem::Standard(SA::ShowAllApps),
            MenuItem::Separator,
            MenuItem::Standard(SA::Quit),
        ],
    );

    // NOTE: deliberately no `StandardAction::Close` here — it would claim Cmd+W to
    // close the window, but the app binds Cmd+W to close the active *tab*
    // (WorkspaceAction::CloseActive). macOS menu shortcuts win over app keybindings,
    // so adding it would silently break tab-closing.
    let file_menu = Menu::new(
        "File",
        vec![MenuItem::Custom(CustomMenuItem::new(
            "New Window",
            |ctx| open_new_window(ctx),
            menu_no_updates,
            Keystroke::parse("cmd-n").ok(),
        ))],
    );

    // The title MUST be exactly "Window" — that is how `Menu::is_window_menu`
    // flags it, so the framework calls `setWindowsMenu` and macOS fills it (and
    // the Dock's window list) with the open windows.
    let window_menu = Menu::new(
        "Window",
        vec![
            MenuItem::Standard(SA::Minimize),
            MenuItem::Standard(SA::Zoom),
            MenuItem::Standard(SA::ToggleFullScreen),
            MenuItem::Separator,
            MenuItem::Standard(SA::BringAllToFront),
        ],
    );

    MenuBar::new(vec![app_menu, file_menu, window_menu])
}

/// Dock / Cmd-Tab name fix for the BARE dev binary (`cargo run`). LaunchServices
/// names unbundled processes after the executable file — lowercase "dragent" —
/// and ignores both the menu-bar title and the embedded __info_plist
/// CFBundleName. The only correction is the LS display-name private API
/// (shipped in Chromium for years). Dev-only: the .app bundle takes "Dragent"
/// from its Info.plist, and the bundle-identifier guard below keeps this from
/// ever running there. No-op if any symbol is missing.
#[cfg(target_os = "macos")]
fn fix_dev_dock_name() {
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int};

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFBundleGetMainBundle() -> *mut c_void;
        fn CFBundleGetIdentifier(bundle: *mut c_void) -> *mut c_void;
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            s: *const c_char,
            encoding: u32,
        ) -> *mut c_void;
    }
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    type GetAsn = unsafe extern "C" fn() -> *mut c_void;
    type SetItem =
        unsafe extern "C" fn(c_int, *mut c_void, *mut c_void, *mut c_void, *mut *mut c_void) -> i32;

    unsafe {
        let bundle = CFBundleGetMainBundle();
        if !bundle.is_null() && !CFBundleGetIdentifier(bundle).is_null() {
            return; // bundled build — Info.plist already carries the name
        }
        let sym = |name: &[u8]| libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr().cast());
        let get_asn = sym(b"_LSGetCurrentApplicationASN\0");
        let set_item = sym(b"_LSSetApplicationInformationItem\0");
        let name_key = sym(b"_kLSDisplayNameKey\0"); // data symbol: CFStringRef
        if get_asn.is_null() || set_item.is_null() || name_key.is_null() {
            return;
        }
        let get_asn: GetAsn = std::mem::transmute(get_asn);
        let set_item: SetItem = std::mem::transmute(set_item);
        let name = CFStringCreateWithCString(std::ptr::null(), b"Dragent\0".as_ptr().cast(), UTF8);
        // -2 = the "magic" session connection constant (same as Chromium).
        let _ = set_item(
            -2,
            get_asn(),
            *(name_key as *mut *mut c_void),
            name,
            std::ptr::null_mut(),
        );
    }
}

pub fn run() -> Result<()> {
    // Marks this process as the running app for `dragent open`.
    crate::cli::instance::hold();
    // Computer use works the desktop from a helper process (this executable
    // again), so an app that hangs or crashes it never reaches the UI.
    if let Ok(exe) = std::env::current_exe() {
        dragent_computer::use_helper(vec![
            exe.into_os_string(),
            dragent_computer::SERVICE_ARG.into(),
        ]);
    }
    // GUI-subsystem binaries (see crate attribute) start with no console. When
    // launched from a terminal (cargo run, PowerShell), re-attach to the parent
    // console so logs still print; when launched from Explorer/taskbar there is
    // no parent console and this quietly fails.
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    let mut is_deeplink_launch = false;

    #[cfg(windows)]
    {
        // Debug and installed release builds must be independently runnable.
        // Sharing one port made `target/debug/dragent.exe` silently forward a
        // new-window request to the installed release and exit, so developers
        // appeared to test new code while actually exercising the old binary.
        let instance_address = if cfg!(debug_assertions) {
            "127.0.0.1:49326"
        } else {
            "127.0.0.1:49325"
        };
        let (url_tx, url_rx) = async_channel::unbounded::<String>();
        match std::net::TcpListener::bind(instance_address) {
            Ok(listener) => {
                let url_tx_clone = url_tx.clone();
                crate::bg::detach(move || {
                    for stream in listener.incoming() {
                        if let Ok(stream) = stream {
                            use std::io::BufRead;
                            let mut reader = std::io::BufReader::new(stream);
                            let mut buf = String::new();
                            if reader.read_line(&mut buf).is_ok() {
                                let msg = buf
                                    .trim_end_matches('\n')
                                    .trim_end_matches('\r')
                                    .to_string();
                                if !msg.is_empty() {
                                    let _ = url_tx_clone.send_blocking(msg);
                                }
                            }
                        }
                    }
                });
                let args: Vec<String> = std::env::args().collect();
                let deep_link_prefix = format!("{}://", dragent_paths::url_scheme());
                if args.len() > 1 && args[1].starts_with(&deep_link_prefix) {
                    is_deeplink_launch = true;
                    let _ = url_tx.send_blocking(args[1].clone());
                }
                auth::set_deep_link_receiver(url_rx);
            }
            Err(_) => {
                let args: Vec<String> = std::env::args().collect();
                if let Ok(mut stream) = std::net::TcpStream::connect(instance_address) {
                    use std::io::Write;
                    let deep_link_prefix = format!("{}://", dragent_paths::url_scheme());
                    let msg = if args.len() > 1 && args[1].starts_with(&deep_link_prefix) {
                        // Deep link: the running instance is (or is about to be)
                        // mid-auth-flow. Focus it rather than spawning a second
                        // window that would race it for the same one-time code.
                        format!("{}\n", args[1])
                    } else {
                        // Plain relaunch — taskbar/Start Menu/desktop icon — with
                        // no URL to route. Open an ADDITIONAL window, matching
                        // macOS's Dock-icon-while-running behavior, rather than
                        // just refocusing the one that's already open.
                        "NEWWINDOW\n".to_string()
                    };
                    let _ = stream.write_all(msg.as_bytes());
                    let _ = stream.flush();
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
                return Ok(());
            }
        }
    }

    big_alloc_trace::init();
    init_logging();
    // Dev-only benchmark hooks (scripts/bench): startup timer, --bench-mem
    // 5 s memory logger. No-ops unless the dev build is asked for them.
    crate::bench::note_process_start();
    crate::bench::maybe_start_mem_logger();
    crate::bench::install_frame_hook();
    crate::perf_overlay::maybe_autostart();
    init_tls_provider();
    // If an update was downloaded on a previous run, hand off to its installer
    // now (silently upgrades in place and relaunches) — before any UI exists.
    #[cfg(windows)]
    if !is_deeplink_launch {
        autoupdate::apply_pending_on_startup();
    }
    register_scheme_handler();
    // Local data is scoped to the signed-in user's active organisation; open
    // the one confirmed last time until the server answers.
    crate::organisation_context::restore_at_launch();
    // Provider keys live in the Keychain, and a read there can wait on a
    // macOS access prompt. Read them all on one background thread from now
    // on; this (the UI) thread only ever sees what has arrived. Comes before
    // the first settings load and before threads that load settings start.
    settings::credentials::start_keychain_warmup();
    warm_in_background();
    std::thread::spawn(agent::mcp::init);
    // Parse the bundled/user theme catalog away from the UI thread. The
    // Appearance page can then paint immediately even when the current theme
    // is a built-in that did not otherwise initialize the lazy registry.
    crate::bg::detach(|| {
        let started = std::time::Instant::now();
        let count = crate::theme::Theme::all_infos().len();
        log::debug!(
            "[themes] catalog pre-warmed in {}ms ({count} themes)",
            started.elapsed().as_millis()
        );
    });
    // Every terminal session expands shell aliases and searches workflows;
    // read both off the UI thread before the first session is restored.
    let _ = console::shell_aliases::ShellAliases::load();
    console::shell_history::load_in_background();
    crate::bg::detach(console::workflows::WorkflowStore::warm);

    // Publish the persisted terminal typography before the first frame so
    // grid metrics, overlays, and cell sizing all agree from the start.
    {
        let editor_settings = console::editor_settings::EditorSettings::global();
        theme::set_terminal_typography(
            editor_settings.terminal.clamped_font_size(),
            editor_settings.terminal.line_height_ratio(),
        );
    }

    // Apply persisted Code-and-Index settings before any repo registration.
    // The embedding model is loaded on the first repo registration instead of
    // competing with UI/auth/font initialization during application startup.
    {
        let index_settings = settings::AppSettings::global().index.clone();
        let manager = dragent_index::get_manager();
        manager.set_auto_index(index_settings.auto_index);
        manager.set_respect_gitignore(index_settings.respect_gitignore);
        if !index_settings.enabled {
            manager.set_enabled(false);
        }
    }
    crate::startup_timing::mark(crate::startup_timing::Phase::Settings);
    // The host service's first answer, whichever caller reaches it first.
    dragent_host::client::on_first_connect(|| {
        crate::startup_timing::mark(crate::startup_timing::Phase::Host)
    });
    // Keep the host up while the app runs; it closes by itself when idle
    // with no app, and this brings it straight back if it goes away.
    dragent_host::client::hold_presence();

    // NOTE: no token pre-warm here — deliberately. The server ROTATES refresh
    // tokens on every exchange and revokes the whole token family if a revoked
    // token is reused. A pre-warm exchange here raced AuthManager's silent
    // restore (both exchanging the same stored token), so whichever finished
    // second triggered the family revocation and destroyed the session on
    // every app launch. The AuthManager restore (200ms after startup) is the
    // ONLY startup exchanger and it pre-warms API_CLIENT via set_tokens().

    // Background auto-update: check the release manifest periodically, download
    // and stage/apply updates; they take effect on the next launch.
    std::thread::spawn(autoupdate::start_background);

    let dev_terminal = std::env::var("DRAGENT_DEV_TERMINAL")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    let mut callbacks = AppCallbacks::default();
    callbacks.on_open_urls = Some(Box::new(|urls, ctx| {
        for url in urls {
            let automation_prefix = format!("{}://automations/", dragent_paths::url_scheme());
            if let Some(id) = url
                .strip_prefix(&automation_prefix)
                .map(|id| id.trim_end_matches('/').to_string())
                .filter(|id| !id.is_empty())
            {
                // A copied automation link: open it in the active window.
                if let Some(window_id) = ctx.windows().active_window() {
                    if let (Some(view_id), Some(presenter)) =
                        (ctx.focused_view_id(window_id), ctx.presenter(window_id))
                    {
                        let chain = presenter.borrow().ancestors(view_id);
                        ctx.dispatch_typed_action(
                            window_id,
                            &chain,
                            &crate::workspace::WorkspaceAction::OpenAutomationLink(id),
                            log::Level::Info,
                        );
                    }
                }
            } else if agent::mcp::is_mcp_oauth_callback(&url) {
                agent::mcp::handle_mcp_oauth_callback(&url);
            } else if let Some((session, secret)) = parse_join_url(&url) {
                // Shared-session join link → route through the SessionManager
                // singleton, which the workspace subscribes to (no view ctx here).
                use warpui::{SingletonEntity, UpdateModel};
                let sm = crate::realtime::manager::SessionManager::handle(ctx);
                ctx.update_model(&sm, |m, mctx| m.request_join(mctx, session, secret));
            } else {
                handle_open_url(ctx, &url);
            }
        }
    }));
    // A clicked automation alert shows its run in the active window.
    callbacks.on_notification_clicked = Some(Box::new(|response, ctx| {
        let Some(action) = response.data().and_then(|data| {
            crate::automations::run_id_from_alert(data)
                .map(|run_id| crate::workspace::WorkspaceAction::OpenAutomationRun(run_id.to_string()))
                .or_else(|| {
                    crate::teams::team_from_alert(data).map(|team| crate::workspace::WorkspaceAction::OpenTeams {
                        team: Some(team.to_string()),
                    })
                })
        }) else {
            return;
        };
        let window_id = ctx
            .windows()
            .active_window()
            .or_else(|| ctx.window_ids().next());
        let Some(window_id) = window_id else {
            return;
        };
        if let (Some(view_id), Some(presenter)) =
            (ctx.focused_view_id(window_id), ctx.presenter(window_id))
        {
            let chain = presenter.borrow().ancestors(view_id);
            ctx.dispatch_typed_action(window_id, &chain, &action, log::Level::Info);
        }
    }));
    // Open a new window when the OS requests one (dock-icon click / macOS reopen
    // with no open windows). NOT fired by the New Window menu/keybinding.
    callbacks.on_new_window_requested = Some(Box::new(|ctx| open_new_window(ctx)));
    // No close guard: quitting or closing the window always terminates the
    // UI immediately. Long-lived work (host agent runs, worktrees, PTY
    // sessions owned by the host daemon) survives: the daemon is a detached
    // process re-discovered via its endpoint file, and the next launch
    // restores the persisted session and re-adopts live runs — so the user
    // comes back to the screens exactly as left.
    callbacks.on_should_close_window = Some(Box::new(|window, ctx| {
        use warpui::UpdateView;
        if let Some(root) = ctx.root_view::<root_view::RootView>(window) {
            ctx.update_view(&root, |root, root_ctx| root.persist_workspace(root_ctx));
        }
        use warpui_core::platform::app::ApproveTerminateResult;
        ApproveTerminateResult::Terminate
    }));
    // App-level Quit (Dock/menu Quit, Cmd+Q) goes through a SEPARATE callback
    // than the window close button — same policy: terminate immediately, no
    // "close the running process?" prompt. Session state is flushed by
    // `on_window_will_close`, so relaunch restores everything.
    callbacks.on_should_terminate_app = Some(Box::new(|_| {
        use warpui_core::platform::app::ApproveTerminateResult;
        ApproveTerminateResult::Terminate
    }));
    // Cmd+Q on macOS terminates without closing each window first, so the
    // main window is still alive here and its frame can be read directly.
    callbacks.on_will_terminate = Some(Box::new(|ctx| {
        use warpui::UpdateView;
        let windows = ctx.window_ids().collect::<Vec<_>>();
        for window in windows {
            if let Some(root) = ctx.root_view::<root_view::RootView>(window) {
                ctx.update_view(&root, |root, root_ctx| root.persist_workspace(root_ctx));
            }
        }
        crate::window_frame::persist_live(ctx);
        dragent_persistence::flush_session_on_close();
    }));
    callbacks.on_window_will_close = Some(Box::new(|closed, ctx| {
        crate::window_frame::persist_closed(closed.as_ref());
        // This callback runs AFTER the framework removed the closing window, so
        // `window_ids()` is the count of REMAINING windows. Only tear down the app
        // when the last window closes; closing one of several windows just closes
        // that window and leaves the app (and the others) running.
        if ctx.window_ids().next().is_some() {
            return; // other windows remain — keep the app running
        }
        log::info!("[main] last window closed — flushing local recovery state");
        // Preserve workspace layout, terminal transcripts, agent history, and
        // dirty editor backups. The process-wide writer acknowledgment makes
        // shutdown deterministic without deleting user recovery data.
        dragent_persistence::flush_session_on_close();
        ctx.terminate_app(warpui::platform::TerminationMode::ForceTerminate, None);
    }));

    let mut app_builder = platform::AppBuilder::new(callbacks, Box::new(DragentAssets), None);

    #[cfg(target_os = "macos")]
    {
        use std::borrow::Cow;

        use warpui::platform::mac::AppExt;
        use warpui_core::assets::AssetProvider as _;

        if let Ok(icon) = DragentAssets.get("icons/dragent-icon.png") {
            app_builder.set_dev_icon(Cow::Owned(icon.into_owned()));
        }

        // Menu bar — its "Window" menu registers NSApp.windowsMenu, which macOS
        // uses to list open windows at the top of the Dock menu.
        app_builder.set_menu_bar_builder(|_| menu_bar());
        // Right-click Dock menu: "New Window" (+ the OS-provided open-window list).
        app_builder.set_dock_menu_builder(|_| dock_menu());
    }

    let _ = app_builder.run(move |ctx| {
        #[cfg(target_os = "macos")]
        fix_dev_dock_name();

        // First, so a freeze anywhere after it leaves a report.
        ctx.add_singleton_model(crate::hang_watchdog::HangWatchdog::new);
        // Idle build output in agents' worktrees (several GB each) is removed
        // at start and every few hours; see `worktree_sweep`.
        crate::worktree_sweep::start();
        ctx.add_singleton_model(AuthManager::new);
        ctx.add_singleton_model(AccountStore::new);
        ctx.add_singleton_model(crate::settings::OrgStore::new);
        ctx.add_singleton_model(crate::organisation_context::OrganisationContext::new);
        ctx.add_singleton_model(crate::settings::machines_store::MachinesStore::new);
        ctx.add_singleton_model(crate::automations::AutomationRuntime::with_account);
        // This process may talk to Dragent's server for connected services.
        crate::integrations::remote::set_in_app();
        ctx.add_singleton_model(crate::cli::inbox::CliInbox::new);
        ctx.add_singleton_model(crate::realtime::manager::SessionManager::new);

        if dev_terminal && cfg!(any(test, feature = "performance-harness")) {
            #[cfg(any(test, feature = "performance-harness"))]
            terminal::dev_window::open_dev_window(ctx);
        } else {
            workspace::register_bindings(ctx);
            crate::browser::register_bindings(ctx);
            // Re-apply persisted shortcut overrides (register() already does;
            // this is the documented startup entry point) and reconcile the
            // keep-awake lock with the loaded settings.
            workspace::sync_shortcut_bindings(ctx);
            crate::settings::shortcut_settings::sync_wake_lock(
                &crate::settings::AppSettings::global().dev_permissions,
            );
            // Reopen exactly where the previous session closed so panes and
            // PTYs keep their size; first launch fills the usable screen area.
            let options = crate::window_frame::launch_options(ctx);
            let (window_id, _) = ctx.add_window(options, root_view::RootView::new);
            crate::window_frame::set_main_window(window_id);
            crate::startup_timing::mark(crate::startup_timing::Phase::Window);
            // Benchmark hook: process-launch → first-window timing (dev only).
            crate::bench::note_window_open();
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_links_parse_per_edition_but_web_links_parse_for_both() {
        let scheme = dragent_paths::url_scheme();
        let own =
            format!("{scheme}://join?session=abc&secret=xyz");
        assert_eq!(
            parse_join_url(&own),
            Some(("abc".to_string(), "xyz".to_string()))
        );
        // The other edition's deep link belongs to the other install.
        let other = if scheme == "dragent" {
            "dragent-dev://join?session=abc&secret=xyz"
        } else {
            "dragent://join?session=abc&secret=xyz"
        };
        assert_eq!(parse_join_url(other), None, "must reject {other}");
        // Web join URLs work for both editions.
        for web in [
            "https://app.example.test/join?session=abc&secret=xyz",
            "http://localhost:3000/join/?session=abc&secret=xyz",
        ] {
            assert_eq!(
                parse_join_url(web),
                Some(("abc".to_string(), "xyz".to_string())),
                "{web}"
            );
        }
        // Non-join links are not join links.
        assert_eq!(parse_join_url(&format!("{scheme}://pair?c=x")), None);
    }
}
