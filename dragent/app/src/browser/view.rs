//! A browser pane: one bar (navigation, address, page and pane actions) over
//! the page, or over a status page (start, waiting for a local server,
//! failed, crashed). The engine is
//! started only once the pane is on screen and is released after it has been
//! hidden for a while; returning to the pane reloads its address.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use dragent_workspace_model::{BrowserPaneSession, BrowserUrl, SurfaceDescriptor};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element, Empty,
    EventHandler, Expanded, Flex, MainAxisAlignment, MainAxisSize, ParentElement, SavePosition, Shrinkable,
    Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{AppContext, Entity, EntityId, TypedActionView, UpdateView, View, ViewContext, ViewHandle};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::agent::{self, AgentCommand, AgentReply};
use super::native::{Engine, PageState};
use super::surface::{BrowserSurface, Cover, SurfaceState};
use super::{engines, ports};
use crate::icons::{render_icon, AppIcon};
use crate::theme::{text_size, Theme};
use crate::workspace::{InputEditor, InputEditorEvent, InputMode};

mod chrome;
mod design;

/// A page that has not committed within this time is reported as failed.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// How often a pane waiting for a local server checks whether it is up.
const SERVER_POLL: Duration = Duration::from_millis(800);
/// How often the start page refreshes its list of running local servers.
const SUGGESTION_POLL: Duration = Duration::from_secs(3);
/// How often a `wait` (or an awaited `eval`) checks the page again.
const AGENT_POLL: Duration = Duration::from_millis(250);
/// How long an awaited `eval` may take to settle.
const EVAL_SETTLE: Duration = Duration::from_secs(30);
/// Agent commands wait this long for a page to finish loading, then run on
/// whatever is there.
const AGENT_WAIT: Duration = Duration::from_secs(25);
/// A phone counts as using a tab this long after its last action. Its live
/// view asks for a picture more often than this while the tab is open.
const PHONE_HOLD: Duration = Duration::from_secs(6);
/// Width, in points, of the page capture sent to a phone.
const PHONE_PICTURE_POINTS: f64 = 520.0;

/// A menu over the page waits at most this long for the page capture drawn
/// beneath it; after that the page is hidden without one.
const COVER_WAIT: Duration = Duration::from_millis(300);
/// After a change made from a menu over the page, how long to wait (for the
/// page to lay out again, and for further presses) before picturing it anew.
/// Visited pages offered under the address bar while typing.
const SUGGESTIONS: usize = 6;
const COVER_RECAPTURE: Duration = Duration::from_millis(120);
/// Page zoom steps, as in desktop browsers.
const ZOOM_STEPS: &[f64] = &[0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

#[derive(Clone, Debug)]
pub enum BrowserAction {
    Go,
    Back,
    Forward,
    Reload,
    ReloadOrStop,
    HardReload,
    DevTools,
    OpenExternally,
    CopyAddress,
    Print,
    /// -1 zooms out, 1 zooms in, 0 resets.
    Zoom(i8),
    Open(BrowserUrl),
    Retry,
    DismissNotice,
    FocusAddressBar,
    ToggleMenu,
    CloseMenu,
    Escape,
    SplitPane,
    ClosePane,
    ToggleDesign,
    SendDesign,
    /// Mark up a screenshot of the page.
    DesignDraw,
    ToggleDesignMenu,
    CloseDesignMenu,
    /// Send design notes to this session or agent (an `AgentTarget` key).
    SelectDesignTarget(String),
    ClearDesignSearch,
    /// The desktop takes this tab back from the phone that is using it.
    TakeBackFromPhone,
    /// Open the find bar (or select its text when it is open).
    Find,
    /// 1 finds the next match, -1 the previous one.
    FindNext(i8),
    CloseFind,
    /// Lay the page out as a phone shows it, or back.
    ToggleMobile,
    /// The mobile view's page width, in points, while its edge is dragged.
    MobileWidth(f32),
    /// Save a picture of the page to Downloads and copy it.
    Screenshot,
    ShowDownloads,
    /// Switch the ⋯ menu to another of its pages.
    MenuPage(MenuPage),
    ClearHistory,
    /// Import the cookies of one of `import_sources`.
    ImportCookies(usize),
    /// Move the highlight in the address bar's suggestions (1 down, -1 up).
    MoveSuggestion(i8),
    /// Hand the link the page asked about to its app, or forget it.
    OpenAppLink,
    DismissAppLink,
    /// Remove this profile's cookies, cache, site storage and history.
    ClearBrowsingData,
}

/// What the ⋯ menu shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MenuPage {
    #[default]
    Actions,
    History,
    /// Asking before clearing browsing data.
    ConfirmClear,
    /// Other browsers' profiles to import sign-ins from.
    Import,
}

/// Answers from the engine that arrive off the view.
enum EngineReply {
    Found { query: String, result: super::find::FindResult },
    #[cfg(not(target_family = "wasm"))]
    Cookies { label: String, read: Result<Vec<dragent_host::browser_cookies::ImportedCookie>, String> },
    Screenshot(Result<(PathBuf, Vec<u8>), String>),
}

/// The width a page is first laid out at in the mobile view, in points (a
/// current mid-size phone).
const MOBILE_WIDTH: f32 = 393.0;
/// The narrowest the mobile view goes: the smallest phones in common use.
const MOBILE_MIN_WIDTH: f32 = 360.0;
/// The widest the mobile view's edges can be dragged (a large tablet).
const MOBILE_MAX_WIDTH: f32 = 1366.0;
/// The grab strip on each side of the mobile page.
const MOBILE_GRIP_WIDTH: f32 = 10.0;

/// The mobile page's width once its edge is dragged to `pointer_x`: the
/// page stays centred, so each point the edge moves widens it by two.
fn dragged_mobile_width(drag: MobileDrag, pointer_x: f32) -> f32 {
    let width = drag.start_width + 2.0 * drag.side * (pointer_x - drag.start_x);
    width.clamp(MOBILE_MIN_WIDTH, MOBILE_MAX_WIDTH).round()
}

/// An edge of the mobile page being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MobileDrag {
    start_x: f32,
    start_width: f32,
    /// 1 for the right edge, -1 for the left one.
    side: f32,
}

pub enum BrowserEvent {
    /// Title, address, or loading state changed.
    Changed,
    /// The pane took keyboard focus (the page was clicked or focused).
    Focused,
    /// The page asked for a new window; open it as another browser tab.
    OpenInNewTab(BrowserUrl),
    /// The bar's pane actions, which the workspace carries out.
    SplitPane,
    ClosePane,
    DesignTargetsRequested,
    SendDesign { target: String, prompt: String, files: Vec<PathBuf> },
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Blank pane: a start page lists running local servers. No engine.
    Start,
    /// Checking whether a local address has a server, before loading it.
    Probing,
    /// A local address with nothing listening yet; loads once it answers.
    WaitingForServer,
    /// The page itself (the engine starts once the pane is visible).
    Page,
    Failed(String),
    Crashed,
}

pub struct BrowserView {
    view_id: EntityId,
    session: BrowserPaneSession,
    project_root: Option<PathBuf>,
    theme: Theme,
    font: FamilyId,
    address: ViewHandle<InputEditor>,
    address_text: String,
    address_focused: bool,
    page: Rc<RefCell<PageState>>,
    surface: Rc<RefCell<SurfaceState>>,
    parent: Option<Rc<dyn warpui::platform::Window>>,
    phase: Phase,
    loading: bool,
    /// The bar's split button stacks (top/bottom) instead of splitting side
    /// by side: the opposite of the split this pane sits in, as a terminal
    /// pane's header does. Set by the workspace as the layout changes.
    split_vertical: bool,
    /// The current navigation has committed (its first response arrived).
    committed: bool,
    title: String,
    notice: Option<String>,
    focused: bool,
    active: bool,
    /// Invalidates timers and probes started for an earlier navigation.
    generation: u64,
    idle_timer: Option<u64>,
    idle_sequence: u64,
    suggestions: Vec<BrowserUrl>,
    menu_open: bool,
    /// The agent picker beside Send while design mode is on, with its search.
    design_menu_open: bool,
    design_search: ViewHandle<InputEditor>,
    design_search_text: String,
    design_scroll: warpui::elements::ClippedScrollStateHandle,
    zoom: f64,
    can_go_back: bool,
    can_go_forward: bool,
    /// `dragent browser` commands waiting for the page.
    agent_queue: Vec<(AgentCommand, AgentReply)>,
    /// The phone using this tab now: its name and when it last acted.
    phone_hold: Option<(String, web_time::Instant)>,
    /// The phone asked for its mobile view: the page is laid out at this
    /// size (the phone's own page area, in points) while it uses the tab.
    phone_viewport: Option<(f32, f32)>,
    /// The desktop took the tab back: the phone is refused until it opens
    /// the tab again.
    phone_taken_back: bool,
    /// `wait`s and awaited `eval`s checking the page until they're done.
    agent_polls: Vec<AgentPoll>,
    agent_poll_armed: bool,
    /// What the page last downloaded, or opened in a new tab, for `status`.
    last_download: Option<String>,
    last_new_tab: Option<String>,
    cover_wait_armed: bool,
    mouse: RefCell<HashMap<String, MouseStateHandle>>,
    design: crate::browser::design::DesignState,
    design_document: u64,
    menu_page: MenuPage,
    /// Invalidates a pending re-capture of the page under a menu.
    cover_sequence: u64,
    find_open: bool,
    find: ViewHandle<InputEditor>,
    find_text: String,
    find_result: super::find::FindResult,
    /// The page is laid out, and identifies itself, as a phone.
    mobile: bool,
    mobile_width: f32,
    mobile_drag: Rc<std::cell::Cell<Option<MobileDrag>>>,
    /// This pane's browser profile, where its history is kept.
    profile: PathBuf,
    /// A link the page wants another app to open, waiting for the person.
    app_link: Option<String>,
    /// Visited pages matching what is typed in the address bar.
    address_matches: Vec<super::history::Visit>,
    /// The highlighted suggestion, which Enter opens.
    address_pick: Option<usize>,
    window_id: warpui::WindowId,
    /// The page's icon address, as the page declared it.
    favicon: Option<String>,
    /// The site's icon on disk, for the tab.
    favicon_path: Option<PathBuf>,
    /// Browser profiles offered on the menu's Import page.
    #[cfg(not(target_family = "wasm"))]
    import_sources: Vec<dragent_host::browser_cookies::Source>,
    /// An import under way: cookies still to add, how many were, and from where.
    #[cfg(not(target_family = "wasm"))]
    import_queue: Option<(Vec<dragent_host::browser_cookies::ImportedCookie>, usize, String)>,
    replies: async_channel::Sender<EngineReply>,
    #[cfg(target_os = "linux")]
    gtk_pump_armed: bool,
}

impl BrowserView {
    /// Point the bar's split button down (stacked) or right (side by side).
    pub fn set_split_vertical(&mut self, vertical: bool, ctx: &mut ViewContext<Self>) {
        if self.split_vertical != vertical {
            self.split_vertical = vertical;
            ctx.notify();
        }
    }

    /// The address of the page shown.
    pub(crate) fn address(&self) -> &str {
        self.session.url.as_str()
    }

    pub fn new(
        session: BrowserPaneSession,
        project_root: Option<PathBuf>,
        theme: Theme,
        font: FamilyId,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let address = ctx.add_typed_action_view(|inner| {
            InputEditor::new(
                "Search or enter an address",
                font,
                text_size(12.5),
                theme.main_text(),
                theme.sub_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "browser-address".into(),
                },
                inner.focus_handle(),
            )
        });
        ctx.subscribe_to_view(&address, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.address_text = text.clone();
                if view.address_focused {
                    view.address_matches = super::history::suggest(&view.profile, text, SUGGESTIONS);
                    view.address_pick = None;
                    ctx.notify();
                }
            }
        });
        // The same search field the other agent menus open with.
        let design_search = ctx.add_typed_action_view(|inner| {
            InputEditor::new(
                "Find an agent or session",
                font,
                text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "browser-design-agent-search".into(),
                },
                inner.focus_handle(),
            )
        });
        ctx.subscribe_to_view(&design_search, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.design_search_text = text.clone();
                ctx.notify();
            }
        });
        let find = ctx.add_typed_action_view(|inner| {
            InputEditor::new(
                "Find in page",
                font,
                text_size(12.5),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "browser-find".into(),
                },
                inner.focus_handle(),
            )
        });
        ctx.subscribe_to_view(&find, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                if *text != view.find_text {
                    view.find_text = text.clone();
                    view.run_find(0, ctx);
                }
            }
        });
        let (replies, replies_in) = async_channel::unbounded();
        ctx.spawn_stream_on_main_thread(
            replies_in,
            |view, reply, ctx| view.engine_reply(reply, ctx),
            |_, _| {},
        );
        let (wake, wakes) = async_channel::bounded(1);
        ctx.spawn_stream_on_main_thread(wakes, |view, (), ctx| view.refresh(ctx), |_, _| {});
        let design = crate::browser::design::DesignState::new(&session);
        let mut view = Self {
            view_id: ctx.view_id(),
            session,
            project_root,
            theme,
            font,
            address,
            address_text: String::new(),
            address_focused: false,
            page: Rc::default(),
            surface: Rc::new(RefCell::new(SurfaceState::new(wake))),
            parent: ctx.windows().platform_window(ctx.window_id()),
            phase: Phase::Start,
            loading: false,
            split_vertical: false,
            committed: false,
            title: String::new(),
            notice: None,
            focused: false,
            active: true,
            generation: 0,
            idle_timer: None,
            idle_sequence: 0,
            suggestions: Vec::new(),
            menu_open: false,
            design_menu_open: false,
            design_search,
            design_search_text: String::new(),
            design_scroll: Default::default(),
            zoom: 1.0,
            can_go_back: false,
            can_go_forward: false,
            agent_queue: Vec::new(),
            phone_hold: None,
            phone_viewport: None,
            phone_taken_back: false,
            agent_polls: Vec::new(),
            agent_poll_armed: false,
            last_download: None,
            last_new_tab: None,
            cover_wait_armed: false,
            mouse: RefCell::default(),
            design,
            design_document: 0,
            menu_page: MenuPage::Actions,
            cover_sequence: 0,
            find_open: false,
            find,
            find_text: String::new(),
            find_result: Default::default(),
            mobile: false,
            mobile_width: MOBILE_WIDTH,
            mobile_drag: Rc::default(),
            profile: PathBuf::new(),
            app_link: None,
            address_matches: Vec::new(),
            address_pick: None,
            window_id: ctx.window_id(),
            favicon: None,
            favicon_path: None,
            #[cfg(not(target_family = "wasm"))]
            import_sources: Vec::new(),
            #[cfg(not(target_family = "wasm"))]
            import_queue: None,
            replies,
            #[cfg(target_os = "linux")]
            gtk_pump_armed: false,
        };
        view.profile = super::native::storage_dir(&view.session);
        view.favicon_path = view.cached_favicon(view.session.url.as_str());
        let url = view.session.url.clone();
        view.load(url, ctx);
        view
    }

    pub fn pane_uuid(&self) -> uuid::Uuid {
        self.session.pane_id
    }

    pub fn session(&self) -> &BrowserPaneSession {
        &self.session
    }

    pub fn surface_descriptor(&self) -> SurfaceDescriptor {
        self.session.surface()
    }

    pub fn project_root(&self) -> Option<PathBuf> {
        self.project_root.clone()
    }

    pub fn title(&self) -> String {
        if !self.title.trim().is_empty() {
            return self.title.clone();
        }
        match self.session.url.host_label() {
            label if label.is_empty() => "Browser".into(),
            label => label,
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Live state for the E2E automation bridge.
    pub fn automation_state(&self, app: &AppContext, window: warpui::WindowId) -> serde_json::Value {
        let surface = self.surface.borrow();
        let bounds = |name: &str| app.element_position_by_id_at_last_frame(window, format!("browser-{name}-{}", self.session.pane_id))
            .map(|bounds| [bounds.origin_x(), bounds.origin_y(), bounds.width(), bounds.height()]);
        serde_json::json!({
            "pane_uuid": self.session.pane_id.to_string(),
            "url": self.session.url.as_str(),
            "title": self.title(),
            "phase": format!("{:?}", self.phase),
            "loading": self.loading,
            "focused": self.focused,
            "engine": surface.engine.is_some(),
            "engine_visible": surface.engine.as_ref().is_some_and(Engine::is_visible),
            "placement": surface.placement.map(|bounds| [
                bounds.origin_x(),
                bounds.origin_y(),
                bounds.width(),
                bounds.height(),
            ]),
            "suggestions": self.suggestions.iter().map(BrowserUrl::as_str).collect::<Vec<_>>(),
            "notice": self.notice,
            "design_enabled": self.design.enabled,
            "toolbar_bounds": bounds("toolbar"),
            "controls_bounds": bounds("controls"),
            "design_notes": self.design.notes,
            "design_sending": !self.design.sending.is_empty(),
            "design_targets": self.design.targets.iter().map(|t| serde_json::json!({
                "key": t.key, "title": t.title, "detail": t.detail, "existing": t.existing,
            })).collect::<Vec<_>>(),
            "design_target": self.design.target,
            "design_menu_open": self.design_menu_open,
            "mobile": self.mobile,
            "mobile_width": self.mobile_width,
            "mobile_page_bounds": bounds("mobile-page"),
            "mobile_grip_bounds": [bounds("mobile-grip-left"), bounds("mobile-grip-right")],
        })
    }

    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        self.sync_design();
        ctx.notify();
    }

    /// Whether the workspace is showing this pane. Hidden panes stop polling
    /// and let their engine go idle; the presenter hides the page itself.
    pub fn set_active(&mut self, active: bool, ctx: &mut ViewContext<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        self.sync_design();
        #[cfg(target_os = "linux")]
        if active { self.arm_gtk_pump(ctx); }
        if active {
            match self.phase {
                Phase::Start => self.poll_suggestions(ctx),
                Phase::WaitingForServer => self.poll_server(ctx),
                _ => {}
            }
        }
        ctx.notify();
    }

    pub fn navigate(&mut self, url: BrowserUrl, ctx: &mut ViewContext<Self>) {
        self.load(url, ctx);
        ctx.emit(BrowserEvent::Changed);
    }

    /// Start showing `url`. Local addresses are checked first so a server that
    /// is still starting shows a waiting page rather than a blank one.
    fn load(&mut self, url: BrowserUrl, ctx: &mut ViewContext<Self>) {
        self.generation = self.generation.wrapping_add(1);
        if url.host_label() != self.session.url.host_label() {
            // A same-site page keeps its title until the new one arrives.
            self.title.clear();
            self.page.borrow_mut().title.clear();
        }
        self.session.url = url.clone();
        self.notice = None;
        if !self.address_focused {
            self.set_address(if url.is_blank() { "" } else { url.as_str() }, ctx);
        }
        if url.is_blank() {
            self.phase = Phase::Start;
            self.loading = false;
            self.release_engine();
            self.poll_suggestions(ctx);
        } else if let Some((host, port)) = url.loopback_port() {
            self.phase = Phase::Probing;
            self.loading = true;
            let generation = self.generation;
            ctx.spawn(
                async move { ports::is_listening(&host, port) },
                move |view, up, ctx| {
                    if view.generation != generation || view.phase != Phase::Probing {
                        return;
                    }
                    if up {
                        view.show_page(ctx);
                    } else {
                        view.phase = Phase::WaitingForServer;
                        view.loading = false;
                        view.poll_server(ctx);
                    }
                    view.flush_agent(ctx);
                    ctx.notify();
                },
            );
        } else {
            self.show_page(ctx);
        }
        ctx.notify();
    }

    fn show_page(&mut self, ctx: &mut ViewContext<Self>) {
        self.phase = Phase::Page;
        self.loading = true;
        self.committed = false;
        // A load still running for the previous address must not count as
        // this navigation's first response.
        {
            let mut page = self.page.borrow_mut();
            page.loading = false;
            page.responded = false;
        }
        let navigated = match self.surface.borrow().engine.as_ref() {
            Some(engine) => Some(engine.navigate(&self.session.url)),
            None => None,
        };
        match navigated {
            Some(Err(error)) => self.fail(error),
            // The load timeout starts when navigation really begins; a pane
            // that is not visible yet starts its engine later.
            Some(Ok(())) => self.arm_load_timeout(ctx),
            None => {}
        }
        ctx.notify();
    }

    fn fail(&mut self, message: String) {
        self.phase = Phase::Failed(message);
        self.loading = false;
    }

    fn arm_load_timeout(&mut self, ctx: &mut ViewContext<Self>) {
        let generation = self.generation;
        ctx.spawn(Timer::after(LOAD_TIMEOUT), move |view, _, ctx| {
            if view.generation != generation || view.phase != Phase::Page || view.committed {
                return;
            }
            // Hidden pages are suspended; judge them once they are shown.
            if view.surface.borrow().placement.is_none() {
                view.arm_load_timeout(ctx);
                return;
            }
            let host = view.session.url.host_label();
            view.fail(format!(
                "{host} didn't respond. Check the address or your connection."
            ));
            view.flush_agent(ctx);
            ctx.emit(BrowserEvent::Changed);
            ctx.notify();
        });
    }

    fn poll_server(&mut self, ctx: &mut ViewContext<Self>) {
        let generation = self.generation;
        let Some((host, port)) = self.session.url.loopback_port() else {
            return;
        };
        ctx.spawn(
            async move {
                Timer::after(SERVER_POLL).await;
                ports::is_listening(&host, port)
            },
            move |view, up, ctx| {
                if view.generation != generation || view.phase != Phase::WaitingForServer {
                    return;
                }
                if up {
                    view.show_page(ctx);
                    ctx.emit(BrowserEvent::Changed);
                } else if view.active {
                    view.poll_server(ctx);
                }
            },
        );
    }

    fn poll_suggestions(&mut self, ctx: &mut ViewContext<Self>) {
        let generation = self.generation;
        ctx.spawn(
            async move { ports::running_local_servers() },
            move |view, servers, ctx| {
                if view.generation != generation || view.phase != Phase::Start {
                    return;
                }
                if view.suggestions != servers {
                    view.suggestions = servers;
                    ctx.notify();
                }
                if view.active {
                    ctx.spawn(Timer::after(SUGGESTION_POLL), move |view, _, ctx| {
                        if view.generation == generation && view.active {
                            view.poll_suggestions(ctx);
                        }
                    });
                }
            },
        );
    }

    fn release_engine(&mut self) {
        let engine = self.surface.borrow_mut().engine.take();
        drop(engine);
    }

    fn set_address(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        self.address_text = text.to_string();
        let text = text.to_string();
        self.address
            .update(ctx, |editor, editor_ctx| editor.set_text(&text, editor_ctx));
    }

    /// Apply what the page reported, then start or release the engine for
    /// the pane's current visibility. Runs whenever the engine or the
    /// presenter wakes the view.
    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        let mut changed = false;
        let (design_messages, document, app_link, zoom_steps, favicon) = {
            let mut page = self.page.borrow_mut();
            (
                std::mem::take(&mut page.design_messages),
                page.document_sequence,
                page.app_link.take(),
                std::mem::take(&mut page.zoom_steps),
                page.favicon.take(),
            )
        };
        if let Some(link) = app_link {
            self.app_link = Some(link);
        }
        for step in zoom_steps {
            self.zoom = next_zoom(self.zoom, step);
            let zoom = self.zoom;
            self.with_engine(|engine| engine.set_zoom(zoom));
        }
        if let Some(favicon) = favicon {
            if self.favicon.as_ref() != Some(&favicon) {
                self.favicon = Some(favicon.clone());
                self.load_favicon(favicon, ctx);
            }
        }
        if document != self.design_document {
            self.design_document = document;
            self.design.token = uuid::Uuid::new_v4().to_string();
            self.design.capturing = false;
            self.design.send_request = None;
        }
        let was_loading = self.loading;
        let report = {
            let mut page = self.page.borrow_mut();
            (
                page.url.take(),
                page.title.clone(),
                page.loading,
                std::mem::take(&mut page.responded),
                std::mem::take(&mut page.focused),
                page.new_window.take(),
                page.blocked.take(),
                page.downloaded.take(),
                std::mem::take(&mut page.crashed),
                page.edited,
            )
        };
        let (url, title, loading, responded, focused, new_window, blocked, downloaded, crashed, edited) =
            report;

        if let Some(url) = url {
            if let Ok(url) = BrowserUrl::parse(&url) {
                if url != self.session.url {
                    if url.host_label() != self.session.url.host_label() {
                        // Another site: its cached icon at once, else none
                        // until the page names one.
                        self.favicon = None;
                        self.favicon_path = self.cached_favicon(url.as_str());
                    }
                    self.session.url = url;
                    changed = true;
                }
                // The title still belongs to the previous page; it follows below.
                super::history::record(&self.profile, self.session.url.as_str(), "");
            }
            if !self.address_focused && self.address_text != self.session.url.as_str() {
                let text = self.session.url.as_str().to_string();
                self.set_address(&text, ctx);
            }
        }
        if title != self.title {
            super::history::retitle(&self.profile, self.session.url.as_str(), &title);
            self.title = title;
            changed = true;
        }
        if self.phase == Phase::Page || matches!(self.phase, Phase::Failed(_)) {
            if (loading || responded) && !self.committed {
                // The first response arrived: the address works after all.
                self.committed = true;
                self.phase = Phase::Page;
                changed = true;
            }
            if self.committed && loading != self.loading {
                self.loading = loading;
                changed = true;
            }
        }
        // Back and forward follow every navigation the page reports.
        let history = self.surface.borrow().engine.as_ref().map(Engine::history);
        if let Some((back, forward)) = history {
            if (back, forward) != (self.can_go_back, self.can_go_forward) {
                self.can_go_back = back;
                self.can_go_forward = forward;
                changed = true;
            }
        }
        if let Some(url) = new_window {
            match BrowserUrl::parse(&url) {
                Ok(url) => {
                    self.last_new_tab = Some(url.as_str().to_string());
                    ctx.emit(BrowserEvent::OpenInNewTab(url))
                }
                Err(_) => self.notice = Some("Blocked a pop-up window from this page.".into()),
            }
        }
        if let Some(url) = blocked {
            let scheme = url.split(':').next().unwrap_or_default();
            self.notice = Some(if scheme.eq_ignore_ascii_case("file") {
                "This page tried to open a local file. Browser panes only open web pages.".into()
            } else {
                format!("This page tried to open a {scheme}: link, which browser panes don't open.")
            });
        }
        if let Some(download) = &downloaded {
            self.last_download = Some(match download {
                Ok(path) => format!("It last downloaded {}.", path.display()),
                Err(error) => format!("Its last download failed: {error}"),
            });
        }
        if let Some(download) = downloaded {
            self.notice = Some(match download {
                Ok(path) => format!(
                    "Downloaded {} to your Downloads folder.",
                    path.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                ),
                Err(error) => error,
            });
        }
        if crashed {
            self.release_engine();
            self.phase = Phase::Crashed;
            self.loading = false;
            changed = true;
        }
        if focused && !self.focused {
            // Keyboard shortcuts must act on this pane now (on_focus also
            // tells the workspace to select it).
            ctx.focus_self();
        }

        let (placement, has_engine) = {
            let mut surface = self.surface.borrow_mut();
            surface.keep_alive = edited;
            (surface.placement, surface.engine.is_some())
        };
        if self.phase == Phase::Page && !has_engine && placement.is_some() && self.parent.is_none()
        {
            // Without a native window there is nothing to embed into; failing
            // stops the surface from asking for an engine every frame.
            self.fail("The browser isn't available in this window.".into());
            changed = true;
        }
        if self.phase == Phase::Page && !has_engine {
            if let (Some(bounds), Some(parent)) = (placement, self.parent.clone()) {
                engines::make_room(&self.surface);
                let wake = self.surface.borrow().wake.clone();
                match Engine::create(parent, &self.session, bounds, self.page.clone(), wake, self.mobile) {
                    Ok(mut engine) => {
                        engine.place(Some(bounds), true);
                        if self.focused {
                            engine.focus();
                        }
                        if (self.zoom - 1.0).abs() > f64::EPSILON {
                            engine.set_zoom(self.zoom);
                        }
                        self.surface.borrow_mut().engine = Some(engine);
                        engines::register(&self.surface);
                        self.loading = true;
                        self.committed = false;
                        self.arm_load_timeout(ctx);
                        #[cfg(target_os = "linux")]
                        self.arm_gtk_pump(ctx);
                    }
                    Err(error) => self.fail(error),
                }
                changed = true;
            }
        }
        if has_engine && placement.is_none() {
            self.arm_idle_release(ctx);
        } else {
            self.idle_timer = None;
        }
        let capturing = matches!(self.surface.borrow().cover, Cover::Capturing);
        if capturing && !self.cover_wait_armed {
            self.cover_wait_armed = true;
            ctx.spawn(Timer::after(COVER_WAIT), |view, _, ctx| {
                view.cover_wait_armed = false;
                let mut surface = view.surface.borrow_mut();
                if matches!(surface.cover, Cover::Capturing) {
                    surface.cover = Cover::Unavailable;
                    surface.sync_engine();
                }
                drop(surface);
                ctx.notify();
            });
        }

        if changed {
            ctx.emit(BrowserEvent::Changed);
        }
        self.flush_agent(ctx);
        if was_loading && !self.loading {
            self.sync_design();
            // A new document has none of the old marks: search it afresh.
            if self.find_open {
                self.run_find(0, ctx);
            }
        }
        for message in design_messages { self.design_message(&message, ctx); }
        ctx.notify();
    }

    /// Release a hidden page's engine after a while, unless the user has
    /// typed into it. Coming back reloads its address.
    fn arm_idle_release(&mut self, ctx: &mut ViewContext<Self>) {
        if self.idle_timer.is_some() {
            return;
        }
        self.idle_sequence += 1;
        let token = self.idle_sequence;
        self.idle_timer = Some(token);
        ctx.spawn(Timer::after(engines::IDLE_RELEASE), move |view, _, ctx| {
            if view.idle_timer != Some(token) {
                return;
            }
            view.idle_timer = None;
            let release = {
                let surface = view.surface.borrow();
                surface.placement.is_none() && !surface.keep_alive
            };
            if release {
                view.release_engine();
                ctx.notify();
            }
        });
    }

    #[cfg(target_os = "linux")]
    fn arm_gtk_pump(&mut self, ctx: &mut ViewContext<Self>) {
        if self.gtk_pump_armed || !self.active || self.surface.borrow().engine.is_none() { return; }
        self.gtk_pump_armed = true;
        ctx.spawn(Timer::after(Duration::from_millis(16)), |view, _, ctx| {
            view.gtk_pump_armed = false;
            if view.active && view.surface.borrow().engine.is_some() {
                Engine::pump_events();
                view.arm_gtk_pump(ctx);
            }
        });
    }

    /// The phone using this tab, if one acted in the last few seconds.
    fn phone_user(&self) -> Option<&str> {
        self.phone_hold
            .as_ref()
            .filter(|(_, at)| at.elapsed() < PHONE_HOLD)
            .map(|(name, _)| name.as_str())
    }

    /// A phone acts on this tab. False when the desktop took the tab back
    /// and the phone hasn't opened it again (`claim`): the action is refused.
    /// `viewport` is the size the phone wants the page laid out at (its
    /// mobile view), or `None` for the pane's own size (its desktop view).
    pub fn phone_touch(
        &mut self,
        phone_name: &str,
        claim: bool,
        viewport: Option<(f32, f32)>,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if claim {
            self.phone_taken_back = false;
        }
        if self.phone_taken_back {
            return false;
        }
        let fresh = self.phone_user().is_none();
        self.phone_hold = Some((phone_name.to_string(), web_time::Instant::now()));
        if fresh || self.phone_viewport != viewport {
            self.phone_viewport = viewport;
            self.sync_phone_park();
            ctx.notify();
        }
        // The cover goes away by itself when the phone stops (closed, lost).
        ctx.spawn(Timer::after(PHONE_HOLD + Duration::from_millis(300)), |view, _, ctx| {
            if view.phone_hold.is_some() && view.phone_user().is_none() {
                view.phone_hold = None;
                view.sync_phone_park();
                ctx.notify();
            }
        });
        true
    }

    /// The phone left the tab.
    pub fn phone_release(&mut self, ctx: &mut ViewContext<Self>) {
        if self.phone_hold.take().is_some() {
            self.sync_phone_park();
            ctx.notify();
        }
    }

    /// While a phone uses the tab its page stays alive under the cover.
    fn sync_phone_park(&mut self) {
        let park = self.phone_user().is_some();
        let size = self.phone_viewport.filter(|_| park);
        let mut surface = self.surface.borrow_mut();
        if surface.park != park || surface.park_size != size {
            surface.park = park;
            surface.park_size = size;
            surface.sync_engine();
        }
    }

    /// Run a phone's command now, never waiting for a page to finish
    /// loading: a phone shows the page as it is at this moment, loading or
    /// not, and asks again a second later. (An agent's command waits, since
    /// the agent acts on the answer.) Going to another page answers at once;
    /// with no page on screen the answer says so.
    pub fn agent_now(&mut self, command: AgentCommand, reply: AgentReply, ctx: &mut ViewContext<Self>) {
        if let AgentCommand::Open(url) = &command {
            self.navigate(url.clone(), ctx);
            return reply(Ok("opening".into()));
        }
        let has_engine = self.surface.borrow().engine.is_some();
        if !has_engine || !matches!(self.phase, Phase::Page) {
            return reply(Err(match &self.phase {
                Phase::Start => "No page is open in this tab yet.".to_string(),
                Phase::Probing | Phase::WaitingForServer => format!(
                    "Waiting for {}: nothing is answering there yet.",
                    self.session.url.host_label()
                ),
                Phase::Failed(message) => message.clone(),
                Phase::Crashed => "The page stopped unexpectedly. Reload it.".to_string(),
                Phase::Page => "The page is starting. One moment.".to_string(),
            }));
        }
        match command {
            AgentCommand::Back | AgentCommand::Forward | AgentCommand::Reload => {
                let possible = match command {
                    AgentCommand::Back => self.can_go_back,
                    AgentCommand::Forward => self.can_go_forward,
                    _ => true,
                };
                if possible {
                    self.with_engine(match command {
                        AgentCommand::Back => Engine::back,
                        AgentCommand::Forward => Engine::forward,
                        _ => Engine::reload,
                    });
                }
                reply(Ok("ok".into()));
                ctx.notify();
            }
            command => self.run_agent(command, reply, ctx),
        }
    }

    /// Run a `dragent browser` command in this pane once the page is ready.
    pub fn agent(&mut self, command: AgentCommand, reply: AgentReply, ctx: &mut ViewContext<Self>) {
        match &command {
            AgentCommand::Open(url) => {
                self.navigate(url.clone(), ctx);
                self.agent_queue.push((AgentCommand::Status, reply));
            }
            AgentCommand::Back | AgentCommand::Forward | AgentCommand::Reload => {
                let possible = match command {
                    AgentCommand::Back => self.can_go_back,
                    AgentCommand::Forward => self.can_go_forward,
                    _ => true,
                };
                let has_engine = self.surface.borrow().engine.is_some();
                if self.phase != Phase::Page || !has_engine {
                    self.agent_queue.push((command, reply));
                    self.flush_agent(ctx);
                    return;
                }
                if !possible {
                    return reply(Err("There's no page to go to in that direction.".into()));
                }
                // Answer once the new page has loaded.
                self.loading = true;
                self.committed = false;
                {
                    let mut page = self.page.borrow_mut();
                    page.loading = false;
                    page.responded = false;
                }
                self.with_engine(match command {
                    AgentCommand::Back => Engine::back,
                    AgentCommand::Forward => Engine::forward,
                    _ => Engine::reload,
                });
                self.agent_queue.push((AgentCommand::Status, reply));
            }
            _ => self.agent_queue.push((command, reply)),
        }
        ctx.spawn(Timer::after(AGENT_WAIT), |view, _, ctx| {
            view.agent_timed_out(ctx)
        });
        self.flush_agent(ctx);
        ctx.notify();
    }

    /// Answer queued commands that can be answered now.
    fn flush_agent(&mut self, ctx: &mut ViewContext<Self>) {
        if self.agent_queue.is_empty() {
            return;
        }
        let blocked = match &self.phase {
            Phase::Probing => return,
            Phase::Page => None,
            Phase::Start => Some("No page is open. Run `dragent browser open URL`.".to_string()),
            Phase::WaitingForServer => Some(format!(
                "Waiting for {}: nothing is answering there yet. Start the server and the page \
                 opens by itself.",
                self.session.url.host_label()
            )),
            Phase::Failed(message) => Some(message.clone()),
            Phase::Crashed => {
                Some("The page stopped unexpectedly. Run `dragent browser reload`.".to_string())
            }
        };
        if let Some(reason) = blocked {
            let status = self.status_text();
            for (command, reply) in std::mem::take(&mut self.agent_queue) {
                reply(match command {
                    AgentCommand::Status => Ok(status.clone()),
                    _ => Err(reason.clone()),
                });
            }
            return;
        }
        let ready = self.surface.borrow().engine.is_some() && !self.loading;
        if ready {
            for (command, reply) in std::mem::take(&mut self.agent_queue) {
                self.run_agent(command, reply, ctx);
            }
        }
    }

    /// The page took too long: run what is queued on whatever is there.
    fn agent_timed_out(&mut self, ctx: &mut ViewContext<Self>) {
        if self.agent_queue.is_empty() {
            return;
        }
        if self.phase == Phase::Page && self.surface.borrow().engine.is_some() {
            for (command, reply) in std::mem::take(&mut self.agent_queue) {
                self.run_agent(command, reply, ctx);
            }
        } else {
            for (_, reply) in std::mem::take(&mut self.agent_queue) {
                reply(Err(
                    "The browser isn't showing a page right now. Open its tab in \
                           Dragent and try again."
                        .into(),
                ));
            }
        }
        ctx.notify();
    }

    fn run_agent(&mut self, command: AgentCommand, reply: AgentReply, ctx: &mut ViewContext<Self>) {
        match command {
            AgentCommand::Status => reply(Ok(self.status_text())),
            AgentCommand::Wait { .. } | AgentCommand::Eval { settle: true, .. } => {
                let Some(script) = command.script() else {
                    return reply(Err("That command doesn't run in the page.".into()));
                };
                let (waiting, limit) = match &command {
                    AgentCommand::Wait { timeout, .. } => (true, *timeout),
                    _ => (false, EVAL_SETTLE),
                };
                let now = std::time::Instant::now();
                self.agent_polls.push(AgentPoll {
                    deadline: now + limit,
                    limit,
                    waiting,
                    state: std::sync::Arc::new(std::sync::Mutex::new(PollState {
                        reply: Some(reply),
                        script,
                        detail: "the page hasn't been checked yet".into(),
                        in_flight: false,
                        started: now,
                        waiting,
                    })),
                });
                self.tick_agent_polls(ctx);
            }
            AgentCommand::Picture(path) => {
                let capture: Box<dyn FnOnce(Result<Vec<u8>, String>)> =
                    Box::new(move |png: Result<Vec<u8>, String>| {
                        reply(png.and_then(|bytes| {
                            std::fs::write(&path, bytes)
                                .map(|()| "saved".to_string())
                                .map_err(|error| format!("Couldn't save the picture: {error}"))
                        }))
                    });
                match self.surface.borrow().engine.as_ref() {
                    Some(engine) => engine.screenshot_small(PHONE_PICTURE_POINTS, capture),
                    None => capture(Err("The page isn't on screen.".into())),
                }
            }
            AgentCommand::Screenshot(path) => {
                let path = path.unwrap_or_else(|| {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_millis())
                        .unwrap_or_default();
                    std::env::temp_dir().join(format!("dragent-browser-{stamp}.png"))
                });
                let capture: Box<dyn FnOnce(Result<Vec<u8>, String>)> =
                    Box::new(move |png: Result<Vec<u8>, String>| {
                        reply(png.and_then(|bytes| {
                            std::fs::write(&path, bytes)
                                .map(|()| {
                                    format!("Saved a screenshot of the page to {}", path.display())
                                })
                                .map_err(|error| {
                                    format!("Couldn't save {}: {error}", path.display())
                                })
                        }))
                    });
                match self.surface.borrow().engine.as_ref() {
                    Some(engine) => engine.screenshot(capture),
                    None => capture(Err("The page isn't on screen.".into())),
                }
            }
            command => {
                let Some(script) = command.script() else {
                    return reply(Err("That command doesn't run in the page.".into()));
                };
                let reply = std::sync::Arc::new(std::sync::Mutex::new(Some(reply)));
                let answer = reply.clone();
                let ran = match self.surface.borrow().engine.as_ref() {
                    Some(engine) => engine.eval(&script, move |raw| {
                        let reply = answer.lock().ok().and_then(|mut reply| reply.take());
                        if let Some(reply) = reply {
                            reply(agent::answer(&command, &raw));
                        }
                    }),
                    None => Err("The page isn't on screen.".into()),
                };
                if let Err(error) = ran {
                    if let Some(reply) = reply.lock().ok().and_then(|mut reply| reply.take()) {
                        reply(Err(error));
                    }
                }
            }
        }
    }

    fn status_text(&self) -> String {
        let state = match &self.phase {
            Phase::Start => "No page is open.".to_string(),
            Phase::Probing => "Checking whether the server is up…".to_string(),
            Phase::WaitingForServer => format!(
                "Waiting for {}: nothing is answering there yet.",
                self.session.url.host_label()
            ),
            Phase::Page if self.loading => "Loading…".to_string(),
            Phase::Page => "Loaded.".to_string(),
            Phase::Failed(message) => format!("Couldn't open the page: {message}"),
            Phase::Crashed => "The page stopped unexpectedly.".to_string(),
        };
        let mut lines = vec![];
        if !self.title.trim().is_empty() {
            lines.push(self.title.trim().to_string());
        }
        if !self.session.url.is_blank() {
            lines.push(self.session.url.as_str().to_string());
        }
        lines.push(state);
        if let Some(download) = &self.last_download {
            lines.push(download.clone());
        }
        if let Some(url) = &self.last_new_tab {
            lines.push(format!(
                "It last opened {url} in a new browser tab (`dragent browser list` shows it)."
            ));
        }
        lines.join("\n")
    }

    /// Check each `wait` (or awaited `eval`) again: answer the ones that are
    /// done or out of time, and come back while any are left.
    fn tick_agent_polls(&mut self, ctx: &mut ViewContext<Self>) {
        let now = std::time::Instant::now();
        self.agent_polls.retain(|poll| {
            let Ok(mut state) = poll.state.lock() else {
                return false;
            };
            if state.reply.is_none() {
                return false;
            }
            if now < poll.deadline {
                return true;
            }
            let (code, next) = if poll.waiting {
                (
                    "condition_not_met",
                    "Snapshot to see the page as it is; wait longer with --timeout if it's still working.",
                )
            } else {
                (
                    "script_timeout",
                    "The promise never settled: check the script, or read `dragent browser console`.",
                )
            };
            let message = format!(
                "Waited {:.0} s and {}.",
                poll.limit.as_secs_f64(),
                state.detail
            );
            if let Some(reply) = state.reply.take() {
                reply(Err(agent::failure(
                    dragent_host::outcome::Status::Timeout,
                    code,
                    message,
                    next,
                )));
            }
            false
        });
        // A page in the middle of navigating can't answer: check next time.
        if !self.loading {
            let surface = self.surface.borrow();
            if let Some(engine) = surface.engine.as_ref() {
                for poll in &self.agent_polls {
                    let script = {
                        let Ok(mut state) = poll.state.lock() else {
                            continue;
                        };
                        if state.in_flight || state.reply.is_none() {
                            continue;
                        }
                        state.in_flight = true;
                        state.script.clone()
                    };
                    let shared = poll.state.clone();
                    let ran = engine.eval(&script, move |raw| {
                        if let Ok(mut state) = shared.lock() {
                            state.answered(&raw);
                        }
                    });
                    if ran.is_err() {
                        if let Ok(mut state) = poll.state.lock() {
                            state.in_flight = false;
                        }
                    }
                }
            }
        }
        if !self.agent_polls.is_empty() && !self.agent_poll_armed {
            self.agent_poll_armed = true;
            ctx.spawn(Timer::after(AGENT_POLL), |view, _, ctx| {
                view.agent_poll_armed = false;
                view.tick_agent_polls(ctx);
            });
        }
    }

    fn go(&mut self, ctx: &mut ViewContext<Self>) {
        let picked = self
            .address_pick
            .and_then(|pick| self.address_matches.get(pick))
            .and_then(|visit| BrowserUrl::parse(&visit.url).ok());
        self.address_matches.clear();
        self.address_pick = None;
        let typed = match picked {
            Some(url) => Ok(url),
            None => BrowserUrl::from_address_or_search(&self.address_text),
        };
        match typed {
            Ok(url) => {
                self.address_focused = false;
                self.navigate(url, ctx);
                ctx.focus_self();
            }
            Err(error) => {
                self.notice = Some(error);
                ctx.notify();
            }
        }
    }

    /// While a menu covers the page, what shows is a picture of it taken
    /// when the menu opened. After a change made from the menu (zoom), take
    /// the picture again so the change shows at once: the page is moved out
    /// of sight, where it keeps rendering, captured, then hidden again.
    /// Presses in quick succession share one capture.
    fn recapture_cover(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.surface.borrow().covered {
            return;
        }
        self.cover_sequence = self.cover_sequence.wrapping_add(1);
        let sequence = self.cover_sequence;
        ctx.spawn(Timer::after(COVER_RECAPTURE), move |view, _, _| {
            if view.cover_sequence != sequence {
                return;
            }
            let weak = Rc::downgrade(&view.surface);
            let mut surface = view.surface.borrow_mut();
            let Some(bounds) = surface.placement.filter(|_| surface.covered) else {
                return;
            };
            let Some(engine) = surface.engine.as_mut().filter(|engine| engine.can_capture()) else {
                return;
            };
            engine.park(bounds, None);
            engine.capture(Box::new(move |image| {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                let Ok(mut surface) = state.try_borrow_mut() else {
                    return;
                };
                if surface.covered {
                    if let Some(image) = image {
                        surface.cover = Cover::Ready(std::sync::Arc::new(image));
                    }
                }
                // Hidden again (or shown, if the menu closed meanwhile).
                surface.sync_engine();
                let _ = surface.wake.try_send(());
            }));
        });
    }

    /// The site's icon on disk, for the tab.
    pub fn favicon_path(&self) -> Option<&std::path::Path> {
        self.favicon_path.as_deref()
    }

    #[cfg(not(target_family = "wasm"))]
    fn cached_favicon(&self, url: &str) -> Option<PathBuf> {
        super::favicon::cached(&self.profile, url).filter(|path| path.exists())
    }

    #[cfg(target_family = "wasm")]
    fn cached_favicon(&self, _: &str) -> Option<PathBuf> {
        None
    }

    /// Show the cached icon for this site, fetching `icon_url` when there is
    /// none or it is old.
    #[cfg(not(target_family = "wasm"))]
    fn load_favicon(&mut self, icon_url: String, ctx: &mut ViewContext<Self>) {
        let page_url = self.session.url.as_str().to_string();
        if let Some(path) = self.cached_favicon(&page_url) {
            let fresh = super::favicon::is_fresh(&path);
            if self.favicon_path.as_ref() != Some(&path) {
                self.favicon_path = Some(path);
                ctx.emit(BrowserEvent::Changed);
            }
            if fresh {
                return;
            }
        }
        let profile = self.profile.clone();
        ctx.spawn(
            async move { super::favicon::fetch(&profile, &page_url, &icon_url).ok().map(|path| (page_url, path)) },
            |view, fetched, ctx| {
                let Some((page_url, path)) = fetched else { return };
                // Still the same site: show it (the image is read afresh).
                if url::Url::parse(&page_url).ok().and_then(|u| u.host_str().map(str::to_string))
                    == url::Url::parse(view.session.url.as_str()).ok().and_then(|u| u.host_str().map(str::to_string))
                {
                    view.favicon_path = Some(path);
                    ctx.emit(BrowserEvent::Changed);
                    ctx.notify();
                }
            },
        );
    }

    #[cfg(target_family = "wasm")]
    fn load_favicon(&mut self, _: String, _: &mut ViewContext<Self>) {}

    /// Read another browser profile's cookies off the UI thread (the Keychain
    /// may ask the person first), then add them in batches.
    #[cfg(not(target_family = "wasm"))]
    fn import_cookies(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(source) = self.import_sources.get(index).cloned() else { return };
        if self.surface.borrow().engine.is_none() {
            self.notice = Some("Open a page first, then import sign-ins.".into());
            return;
        }
        if self.import_queue.is_some() {
            return;
        }
        let label = source.label();
        self.notice = Some(format!("Reading sign-ins from {label}… Your Mac may ask to allow access in the Keychain."));
        let replies = self.replies.clone();
        ctx.spawn(
            async move { dragent_host::browser_cookies::read(&source) },
            move |_, read, _| {
                let _ = replies.try_send(EngineReply::Cookies { label, read });
            },
        );
    }

    /// Add the next batch of imported cookies; each one waits for the engine,
    /// so a large import is spread over frames instead of freezing the app.
    #[cfg(not(target_family = "wasm"))]
    fn import_next_cookies(&mut self, ctx: &mut ViewContext<Self>) {
        const BATCH: usize = 100;
        let Some((mut cookies, mut added, label)) = self.import_queue.take() else { return };
        let batch: Vec<_> = cookies.drain(..cookies.len().min(BATCH)).collect();
        let finished = {
            let surface = self.surface.borrow();
            match surface.engine.as_ref() {
                Some(engine) => {
                    added += batch.iter().filter(|cookie| engine.import_cookie(cookie).is_ok()).count();
                    cookies.is_empty()
                }
                // The page closed mid-import: stop with what was added.
                None => true,
            }
        };
        if finished {
            self.notice = Some(format!("Imported {added} sign-in cookies from {label}. Reloading the page."));
            self.with_engine(Engine::reload);
            ctx.notify();
            return;
        }
        self.import_queue = Some((cookies, added, label));
        ctx.spawn(Timer::after(Duration::from_millis(1)), |view, _, ctx| view.import_next_cookies(ctx));
    }

    /// Search the page for the find bar's text (see [`super::find`]).
    fn run_find(&mut self, step: i8, ctx: &mut ViewContext<Self>) {
        if self.find_text.is_empty() {
            self.find_result = Default::default();
            self.with_engine(|engine| {
                let _ = engine.script(super::find::CLEAR);
            });
            ctx.notify();
            return;
        }
        let query = self.find_text.clone();
        let replies = self.replies.clone();
        let script = super::find::script(&query, step);
        self.with_engine(|engine| {
            let _ = engine.eval(&script, move |raw| {
                if let Some(result) = super::find::parse(&raw) {
                    let _ = replies.try_send(EngineReply::Found { query: query.clone(), result });
                }
            });
        });
    }

    fn close_find(&mut self, ctx: &mut ViewContext<Self>) {
        self.find_open = false;
        self.find_result = Default::default();
        self.with_engine(|engine| {
            let _ = engine.script(super::find::CLEAR);
        });
        ctx.focus_self();
    }

    /// Save the visible page as a PNG in Downloads and put it on the
    /// clipboard.
    fn screenshot(&mut self) {
        let site = self.session.url.host_label();
        let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S");
        let name: String = format!("Screenshot of {site} {stamp}.png")
            .chars()
            .map(|c| if c.is_control() || "/\\:".contains(c) { '_' } else { c })
            .collect();
        let replies = self.replies.clone();
        let save: Box<dyn FnOnce(Result<Vec<u8>, String>)> = Box::new(move |png| {
            let saved = png.and_then(|png| {
                let folder = super::native::downloads_folder()
                    .ok_or_else(|| "There's no Downloads folder to save to.".to_string())?;
                let path = super::native::unique_file(&folder, &name)
                    .ok_or_else(|| "Couldn't name the screenshot.".to_string())?;
                std::fs::write(&path, &png)
                    .map_err(|error| format!("Couldn't save the screenshot: {error}"))?;
                Ok((path, png))
            });
            let _ = replies.try_send(EngineReply::Screenshot(saved));
        });
        match self.surface.borrow().engine.as_ref() {
            Some(engine) => engine.screenshot(save),
            None => save(Err("The page isn't on screen.".into())),
        }
    }

    fn engine_reply(&mut self, reply: EngineReply, ctx: &mut ViewContext<Self>) {
        match reply {
            EngineReply::Found { query, result } => {
                // An answer for text the find bar no longer holds is stale.
                if self.find_open && query == self.find_text {
                    self.find_result = result;
                }
            }
            EngineReply::Screenshot(Ok((path, png))) => {
                ctx.clipboard().write(ClipboardContent {
                    images: Some(vec![warpui_core::clipboard::ImageData {
                        data: png,
                        mime_type: "image/png".into(),
                        filename: path.file_name().map(|name| name.to_string_lossy().into_owned()),
                    }]),
                    ..Default::default()
                });
                self.notice = Some(format!(
                    "Saved {} to Downloads and copied it.",
                    path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()
                ));
            }
            EngineReply::Screenshot(Err(error)) => self.notice = Some(error),
            #[cfg(not(target_family = "wasm"))]
            EngineReply::Cookies { label, read } => match read {
                Ok(cookies) if cookies.is_empty() => {
                    self.notice = Some(format!("{label} has no sign-ins to import."));
                }
                Ok(cookies) => {
                    self.notice = Some(format!("Importing sign-ins from {label}…"));
                    self.import_queue = Some((cookies, 0, label));
                    self.import_next_cookies(ctx);
                }
                Err(error) => self.notice = Some(error),
            },
        }
        ctx.notify();
    }

    fn with_engine(&self, run: impl FnOnce(&Engine)) {
        if let Some(engine) = self.surface.borrow().engine.as_ref() {
            run(engine);
        }
    }

    fn mouse(&self, key: &str) -> MouseStateHandle {
        self.mouse
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    fn icon_button(
        &self,
        key: &str,
        icon: AppIcon,
        enabled: bool,
        action: BrowserAction,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let view_id = self.view_id;
        let button = Hoverable::new(self.mouse(key), move |state| {
            let color = if !enabled {
                theme.disabled_text()
            } else if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            Container::new(
                ConstrainedBox::new(Align::new(render_icon(icon, color, 15.0)).finish())
                    .with_width(28.0)
                    .with_height(28.0)
                    .finish(),
            )
            .with_background_color(if enabled && state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        });
        if !enabled {
            return button.finish();
        }
        button
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
            .finish()
    }

    fn text_button(
        &self,
        key: &str,
        label: &str,
        primary: bool,
        action: BrowserAction,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let font = self.font;
        let view_id = self.view_id;
        let label = label.to_string();
        Hoverable::new(self.mouse(key), move |state| {
            let (background, color) = match (primary, state.is_hovered()) {
                (true, false) => (theme.accent, theme.accent_text()),
                (true, true) => (theme.accent_bg(), theme.accent_text()),
                (false, false) => (theme.surface_1(), theme.main_text()),
                (false, true) => (theme.surface_2(), theme.main_text()),
            };
            Container::new(
                Text::new(label.clone(), font, text_size(12.5))
                    .with_color(color)
                    .soft_wrap(false)
                    .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                    .finish(),
            )
            .with_horizontal_padding(14.0)
            .with_vertical_padding(7.0)
            .with_background_color(background)
            .with_border(Border::all(1.0).with_border_color(if primary {
                ColorU::transparent_black()
            } else {
                theme.outline()
            }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
        .finish()
    }

    /// "This page wants to open your email app."  [Cancel] [Open]
    fn render_app_link(&self, link: &str) -> Box<dyn Element> {
        let theme = self.theme;
        let scheme = super::app_links::scheme(link).unwrap_or_default();
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(
                    AppIcon::Asset("icons/external-link.svg"),
                    theme.sub_text(),
                    13.0,
                ))
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(super::app_links::prompt(&scheme), self.font, text_size(12.0))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(self.text_button("app-link-cancel", "Cancel", false, BrowserAction::DismissAppLink))
                .with_child(self.text_button("app-link-open", "Open", true, BrowserAction::OpenAppLink))
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(6.0)
        .with_vertical_padding(4.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
    }

    fn render_notice(&self, notice: &str) -> Box<dyn Element> {
        let theme = self.theme;
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(AppIcon::Info, theme.sub_text(), 13.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(notice.to_string(), self.font, text_size(12.0))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(self.icon_button(
                    "notice-close",
                    AppIcon::XClose,
                    true,
                    BrowserAction::DismissNotice,
                ))
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(6.0)
        .with_vertical_padding(2.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
    }

    /// Centered status page used for every state without a live page.
    fn status_page(
        &self,
        icon: AppIcon,
        title: &str,
        detail: &str,
        extra: Option<Box<dyn Element>>,
        buttons: Vec<Box<dyn Element>>,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(10.0)
            .with_child(
                Container::new(
                    ConstrainedBox::new(
                        Align::new(render_icon(icon, theme.sub_text(), 22.0)).finish(),
                    )
                    .with_width(48.0)
                    .with_height(48.0)
                    .finish(),
                )
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                .with_margin_bottom(6.0)
                .finish(),
            )
            .with_child(
                Text::new(title.to_string(), self.font, text_size(15.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                ConstrainedBox::new(
                    Text::new(detail.to_string(), self.font, text_size(12.5))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_max_width(420.0)
                .finish(),
            );
        if let Some(extra) = extra {
            column = column.with_child(Container::new(extra).with_margin_top(8.0).finish());
        }
        if !buttons.is_empty() {
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(8.0);
            for button in buttons {
                row = row.with_child(button);
            }
            column = column.with_child(Container::new(row.finish()).with_margin_top(8.0).finish());
        }
        Container::new(Align::new(column.finish()).finish())
            .with_uniform_padding(24.0)
            .with_background_color(theme.background)
            .finish()
    }

    fn render_start(&self) -> Box<dyn Element> {
        let theme = self.theme;
        let servers: Box<dyn Element> = if self.suggestions.is_empty() {
            Text::new(
                "No local servers found yet. Start one in a terminal and it appears here."
                    .to_string(),
                self.font,
                text_size(12.0),
            )
            .with_color(theme.disabled_text())
            .finish()
        } else {
            let mut list = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(2.0);
            for url in &self.suggestions {
                let label = url.host_label();
                let font = self.font;
                let view_id = self.view_id;
                let open = url.clone();
                list = list.with_child(
                    Hoverable::new(self.mouse(&format!("server-{label}")), move |state| {
                        Container::new(
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_spacing(10.0)
                                .with_child(render_icon(AppIcon::Globe, theme.accent, 14.0))
                                .with_child(
                                    Expanded::new(
                                        1.0,
                                        Text::new(label.clone(), font, text_size(12.5))
                                            .with_color(theme.main_text())
                                            .finish(),
                                    )
                                    .finish(),
                                )
                                .with_child(render_icon(
                                    AppIcon::ArrowRight,
                                    if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    },
                                    13.0,
                                ))
                                .finish(),
                        )
                        .with_horizontal_padding(12.0)
                        .with_vertical_padding(8.0)
                        .with_background_color(if state.is_hovered() {
                            theme.surface_2()
                        } else {
                            theme.surface_1()
                        })
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                        .finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action_for(view_id, BrowserAction::Open(open.clone()))
                    })
                    .finish(),
                );
            }
            ConstrainedBox::new(list.finish())
                .with_width(300.0)
                .finish()
        };
        self.status_page(
            AppIcon::Globe,
            "Open a page",
            "Search or enter an address above, or open a server running on this computer.",
            Some(servers),
            vec![self.text_button(
                "start-address",
                "Enter an address",
                false,
                BrowserAction::FocusAddressBar,
            )],
        )
    }

    fn render_body(&self) -> Box<dyn Element> {
        let host = self.session.url.host_label();
        match &self.phase {
            Phase::Start => self.render_start(),
            // Keep the current page up while a local server is re-checked.
            Phase::Probing if self.surface.borrow().engine.is_some() => {
                BrowserSurface::new(self.surface.clone(), self.theme.background).finish()
            }
            Phase::Probing => Container::new(Flex::column().finish())
                .with_background_color(self.theme.background)
                .finish(),
            Phase::WaitingForServer => self.status_page(
                AppIcon::Clock,
                &format!("Waiting for {host}"),
                "Nothing is answering at this address yet. Start your dev server and this page opens as soon as it's ready.",
                None,
                vec![
                    self.text_button("waiting-retry", "Try now", false, BrowserAction::Retry),
                ],
            ),
            Phase::Failed(message) => self.status_page(
                AppIcon::TriangleAlert,
                "This page couldn't be opened",
                message,
                None,
                vec![
                    self.text_button("failed-retry", "Try again", true, BrowserAction::Retry),
                    self.text_button(
                        "failed-external",
                        "Open in your browser",
                        false,
                        BrowserAction::OpenExternally,
                    ),
                ],
            ),
            Phase::Crashed => self.status_page(
                AppIcon::TriangleAlert,
                "This page stopped unexpectedly",
                "The page's process quit, which can happen when a page uses too much memory.",
                None,
                vec![self.text_button("crashed-reload", "Reload page", true, BrowserAction::Retry)],
            ),
            Phase::Page if self.mobile => self.render_mobile_page(),
            Phase::Page => BrowserSurface::new(self.surface.clone(), self.theme.background).finish(),
        }
    }

    /// The page at a phone's width, centred on the pane's background, with
    /// a grip on each side that drags the width (never below
    /// [`MOBILE_MIN_WIDTH`]) and its size above it.
    fn render_mobile_page(&self) -> Box<dyn Element> {
        let theme = self.theme;
        let view_id = self.view_id;
        let width = self.mobile_width;
        let dragging = self.mobile_drag.get().is_some();
        let grip = |side: f32| {
            let drag = self.mobile_drag.clone();
            let mouse = self.mouse(if side > 0.0 { "mobile-grip-right" } else { "mobile-grip-left" });
            let handle = Hoverable::new(mouse, move |state| {
                let color = if state.is_hovered() || dragging {
                    theme.accent
                } else {
                    theme.outline()
                };
                ConstrainedBox::new(
                    Align::new(
                        Container::new(
                            ConstrainedBox::new(Empty::new().finish())
                                .with_width(4.0)
                                .with_height(40.0)
                                .finish(),
                        )
                        .with_background_color(color)
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                        .finish(),
                    )
                    .finish(),
                )
                .with_width(MOBILE_GRIP_WIDTH)
                .finish()
            })
            .with_cursor(Cursor::ResizeLeftRight)
            // A pressed Hoverable keeps the drag to itself by default; the
            // drag belongs to the area around the page, which follows it.
            .with_propagate_drag()
            .finish();
            let name = if side > 0.0 { "right" } else { "left" };
            let grip = EventHandler::new(handle)
                .on_left_mouse_down(move |_, _, position| {
                    drag.set(Some(MobileDrag { start_x: position.x(), start_width: width, side }));
                    DispatchEventResult::StopPropagation
                })
                .finish();
            SavePosition::new(grip, &format!("browser-mobile-grip-{name}-{}", self.session.pane_id))
                .finish()
        };
        let page = ConstrainedBox::new(
            Container::new(
                BrowserSurface::new(self.surface.clone(), theme.background).finish(),
            )
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .finish(),
        )
        .with_min_width(MOBILE_MIN_WIDTH + 2.0)
        .with_max_width(width + 2.0)
        .finish();
        let page = SavePosition::new(page, &format!("browser-mobile-page-{}", self.session.pane_id))
            .finish();
        let size = Container::new(
            Align::new(
                Text::new(format!("{width:.0} px"), self.font, text_size(11.0))
                    .with_color(if dragging { theme.main_text() } else { theme.disabled_text() })
                    .finish(),
            )
            .finish(),
        )
        .with_vertical_padding(3.0)
        .finish();
        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            // The page is the only flexible child: it takes the dragged width
            // (or what the pane has left) and the row centres it. Flexible
            // spacers beside it would split the free space three ways and
            // pin the page at a third of the pane, whatever the drag.
            .with_child(grip(-1.0))
            .with_child(Shrinkable::new(1.0, page).finish())
            .with_child(grip(1.0))
            .finish();
        let body = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(size)
                .with_child(Expanded::new(1.0, Clipped::new(row).finish()).finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .finish();
        // The drag is followed over the whole area, so the pointer may leave
        // the grip; releasing anywhere ends it.
        let (moved, released) = (self.mobile_drag.clone(), self.mobile_drag.clone());
        // A press that missed the grips (the grip stops its own) ends a drag
        // whose release landed outside this area.
        let pressed = self.mobile_drag.clone();
        EventHandler::new(body)
            .on_left_mouse_down(move |_, _, _| {
                pressed.set(None);
                DispatchEventResult::PropagateToParent
            })
            .on_mouse_dragged(move |ctx, _, position| match moved.get() {
                Some(drag) => {
                    let width = dragged_mobile_width(drag, position.x());
                    ctx.dispatch_typed_action_for(view_id, BrowserAction::MobileWidth(width));
                    DispatchEventResult::StopPropagation
                }
                None => DispatchEventResult::PropagateToParent,
            })
            .on_left_mouse_up(move |ctx, _, _| {
                if released.take().is_some() {
                    ctx.notify();
                }
                DispatchEventResult::PropagateToParent
            })
            .finish()
    }
}

impl BrowserView {
    /// The page or the address field has the keyboard right now.
    pub(crate) fn has_focus(&self) -> bool {
        self.focused || self.address_focused
    }
}

impl Entity for BrowserView {
    type Event = BrowserEvent;
}

impl View for BrowserView {
    fn ui_name() -> &'static str {
        "BrowserView"
    }

    fn on_focus(&mut self, focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.focused = true;
            self.with_engine(Engine::focus);
            if self.phase == Phase::Start {
                ctx.focus(&self.address);
            }
        } else if self.address.is_focused(ctx) {
            self.address_focused = true;
            self.address
                .update(ctx, |editor, editor_ctx| editor.select_all(editor_ctx));
        }
        ctx.emit(BrowserEvent::Focused);
        ctx.notify();
    }

    fn on_blur(&mut self, blur_ctx: &warpui::BlurContext, ctx: &mut ViewContext<Self>) {
        if blur_ctx.is_self_blurred() {
            self.focused = false;
            self.menu_open = false;
            self.with_engine(Engine::release_focus);
        } else if self.address_focused && !self.address.is_focused(ctx) {
            self.address_focused = false;
            self.address_matches.clear();
            self.address_pick = None;
            if self.address_text.trim().is_empty() || self.address_text != self.session.url.as_str()
            {
                // Leaving the field without going anywhere restores the address.
                let text = if self.session.url.is_blank() {
                    String::new()
                } else {
                    self.session.url.as_str().to_string()
                };
                self.set_address(&text, ctx);
            }
        }
        ctx.notify();
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme;
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_child(
                Container::new(self.render_bar())
                    .with_background_color(theme.surface_1())
                    .finish(),
            );
        if self.find_open {
            column = column.with_child(self.render_find_bar());
        }
        if let Some(link) = &self.app_link {
            column = column.with_child(self.render_app_link(link));
        }
        if let Some(notice) = &self.notice {
            column = column.with_child(self.render_notice(notice));
        }
        let content = Container::new(
            column
                .with_child(Expanded::new(1.0, self.render_body()).finish())
                .finish(),
        )
        .with_background_color(theme.background)
        .finish();
        // A phone is using this tab: the same cover a phone-held terminal
        // gets, with the one button that takes it back.
        if let Some(phone) = self.phone_user() {
            let cover = crate::terminal::view::terminal_overlays::render_phone_cover(
                theme,
                self.font,
                self.font,
                crate::terminal::view::terminal_overlays::browser_cover_copy(phone),
                self.mouse("phone-cover-take-back"),
                |event_ctx| event_ctx.dispatch_typed_action(BrowserAction::TakeBackFromPhone),
            );
            let mut stack = Stack::new().with_child(content);
            stack.add_overlay_child(cover);
            return stack.finish();
        }
        match self
            .menu_overlay()
            .or_else(|| self.design_menu_overlay())
            .or_else(|| self.suggestions_overlay(app))
        {
            Some((menu, catcher, offset)) => {
                let mut stack = Stack::new().with_child(content);
                stack.add_overlay_child(catcher);
                stack.with_positioned_overlay_child(menu, offset).finish()
            }
            None => content,
        }
    }
}

impl TypedActionView for BrowserView {
    type Action = BrowserAction;

    fn handle_action(&mut self, action: &BrowserAction, ctx: &mut ViewContext<Self>) {
        match action {
            BrowserAction::ToggleDesign => self.toggle_design(ctx),
            BrowserAction::SendDesign => self.send_design(ctx),
            BrowserAction::DesignDraw => self.design_draw(ctx),
            BrowserAction::ToggleDesignMenu => {
                self.menu_open = false;
                self.design_menu_open = !self.design_menu_open;
                if self.design_menu_open {
                    ctx.emit(BrowserEvent::DesignTargetsRequested);
                    ctx.update_view(&self.design_search, |editor, ec| {
                        editor.set_text("", ec);
                        ec.focus_self();
                    });
                }
            }
            BrowserAction::CloseDesignMenu => self.design_menu_open = false,
            BrowserAction::ClearDesignSearch => {
                ctx.update_view(&self.design_search, |editor, ec| {
                    editor.set_text("", ec);
                    ec.focus_self();
                });
            }
            BrowserAction::SelectDesignTarget(key) => {
                self.design_menu_open = false;
                self.select_design_target(key, ctx);
            }
            BrowserAction::Go => self.go(ctx),
            BrowserAction::MoveSuggestion(step) => {
                let count = self.address_matches.len();
                if count > 0 {
                    self.address_pick = Some(match (self.address_pick, *step > 0) {
                        (None, true) => 0,
                        (None, false) => count - 1,
                        (Some(pick), true) => (pick + 1) % count,
                        (Some(pick), false) => (pick + count - 1) % count,
                    });
                }
            }
            BrowserAction::Open(url) => {
                self.address_matches.clear();
                self.address_pick = None;
                self.navigate(url.clone(), ctx);
                ctx.focus_self();
            }
            BrowserAction::Retry => {
                let url = self.session.url.clone();
                self.release_engine_if_crashed();
                self.navigate(url, ctx);
            }
            BrowserAction::ReloadOrStop if self.loading && self.phase == Phase::Page => {
                self.with_engine(Engine::stop);
                self.loading = false;
            }
            BrowserAction::Reload | BrowserAction::ReloadOrStop | BrowserAction::HardReload => {
                if self.phase == Phase::Page && self.surface.borrow().engine.is_some() {
                    self.loading = true;
                    if matches!(action, BrowserAction::HardReload) {
                        self.with_engine(Engine::hard_reload);
                    } else {
                        self.with_engine(Engine::reload);
                    }
                } else if !self.session.url.is_blank() {
                    let url = self.session.url.clone();
                    self.navigate(url, ctx);
                }
            }
            BrowserAction::CopyAddress => {
                if !self.session.url.is_blank() {
                    ctx.clipboard().write(ClipboardContent::plain_text(
                        self.session.url.as_str().to_string(),
                    ));
                }
            }
            BrowserAction::Print => self.with_engine(Engine::print),
            BrowserAction::Zoom(direction) => {
                self.zoom = next_zoom(self.zoom, *direction);
                let zoom = self.zoom;
                self.with_engine(|engine| engine.set_zoom(zoom));
                self.recapture_cover(ctx);
            }
            BrowserAction::ToggleMenu => {
                self.design_menu_open = false;
                self.menu_open = !self.menu_open;
                self.menu_page = MenuPage::Actions;
            }
            BrowserAction::CloseMenu => {
                self.menu_open = false;
                self.menu_page = MenuPage::Actions;
            }
            BrowserAction::MenuPage(page) => {
                self.menu_open = true;
                self.menu_page = *page;
                #[cfg(not(target_family = "wasm"))]
                if *page == MenuPage::Import {
                    self.import_sources = dragent_host::browser_cookies::sources();
                }
            }
            BrowserAction::ImportCookies(index) => {
                self.menu_open = false;
                self.menu_page = MenuPage::Actions;
                #[cfg(not(target_family = "wasm"))]
                self.import_cookies(*index, ctx);
                #[cfg(target_family = "wasm")]
                let _ = index;
            }
            BrowserAction::Find => {
                self.menu_open = false;
                self.find_open = true;
                ctx.focus(&self.find);
                self.find.update(ctx, |editor, editor_ctx| editor.select_all(editor_ctx));
                if !self.find_text.is_empty() {
                    self.run_find(0, ctx);
                }
            }
            BrowserAction::FindNext(step) => {
                if !self.find_open {
                    self.find_open = true;
                    ctx.focus(&self.find);
                }
                self.run_find(*step, ctx);
            }
            BrowserAction::CloseFind => self.close_find(ctx),
            BrowserAction::ToggleMobile => {
                self.mobile = !self.mobile;
                // The user agent is fixed when the engine starts: start it
                // again, which reloads the address as the other device.
                if self.surface.borrow().engine.is_some() {
                    self.release_engine();
                    self.refresh(ctx);
                }
            }
            BrowserAction::MobileWidth(width) => {
                self.mobile_width = width.clamp(MOBILE_MIN_WIDTH, MOBILE_MAX_WIDTH);
            }
            BrowserAction::Screenshot => self.screenshot(),
            BrowserAction::ShowDownloads => {
                if let Some(folder) = super::native::downloads_folder() {
                    if let Ok(url) = url::Url::from_directory_path(&folder) {
                        ctx.open_url(url.as_str());
                    }
                }
            }
            BrowserAction::OpenAppLink => {
                if let Some(link) = self.app_link.take() {
                    ctx.open_url(&link);
                }
            }
            BrowserAction::DismissAppLink => self.app_link = None,
            BrowserAction::ClearHistory => {
                super::history::clear(&self.profile);
                self.menu_page = MenuPage::Actions;
                self.notice = Some("Cleared this browser's history.".into());
            }
            BrowserAction::ClearBrowsingData => {
                self.menu_open = false;
                self.menu_page = MenuPage::Actions;
                let cleared = match self.surface.borrow().engine.as_ref() {
                    Some(engine) => engine.clear_browsing_data(),
                    None => Err("Open a page first, then clear browsing data.".into()),
                };
                self.notice = Some(match cleared {
                    Ok(()) => {
                        super::history::clear(&self.profile);
                        self.with_engine(Engine::reload);
                        "Cleared cookies, cached files, site data and history for this organisation's browser.".into()
                    }
                    Err(error) => error,
                });
            }
            BrowserAction::Escape if self.find_open && !self.address_focused && !self.menu_open => {
                self.close_find(ctx);
            }
            BrowserAction::Escape if self.address_focused && !self.address_matches.is_empty() => {
                // The first Escape closes the suggestions; the next leaves.
                self.address_matches.clear();
                self.address_pick = None;
            }
            BrowserAction::Escape => {
                self.menu_open = false;
                self.menu_page = MenuPage::Actions;
                self.design_menu_open = false;
                if self.address_focused {
                    // Leave the field without going anywhere; the page keeps
                    // the keyboard.
                    ctx.focus_self();
                }
            }
            BrowserAction::SplitPane => ctx.emit(BrowserEvent::SplitPane),
            BrowserAction::ClosePane => ctx.emit(BrowserEvent::ClosePane),
            BrowserAction::Back => self.with_engine(Engine::back),
            BrowserAction::Forward => self.with_engine(Engine::forward),
            BrowserAction::DevTools => {
                self.with_engine(Engine::devtools);
                // The inspector connects asynchronously; if it docked anyway,
                // move it out once it is up.
                ctx.spawn(Timer::after(Duration::from_millis(600)), |view, _, ctx| {
                    if let Some(engine) = view.surface.borrow_mut().engine.as_mut() {
                        engine.undock_devtools();
                    }
                    ctx.notify();
                });
            }
            BrowserAction::OpenExternally => {
                if !self.session.url.is_blank() {
                    ctx.open_url(self.session.url.as_str());
                }
            }
            BrowserAction::TakeBackFromPhone => {
                self.phone_taken_back = true;
                self.phone_hold = None;
                self.sync_phone_park();
                ctx.notify();
            }
            BrowserAction::DismissNotice => {
                self.notice = None;
                if self.address_focused {
                    ctx.focus_self();
                }
            }
            BrowserAction::FocusAddressBar => {
                self.menu_open = false;
                ctx.focus(&self.address);
            }
        }
        ctx.notify();
    }
}

/// The next zoom step from `zoom` in `direction` (0 resets to 100%).
fn next_zoom(zoom: f64, direction: i8) -> f64 {
    match direction {
        0 => 1.0,
        d if d > 0 => ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > zoom + 0.001)
            .unwrap_or(zoom),
        _ => ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < zoom - 0.001)
            .unwrap_or(zoom),
    }
}

impl BrowserView {
    fn release_engine_if_crashed(&mut self) {
        if self.phase == Phase::Crashed {
            self.release_engine();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Press a grip, drag, release, through the real element tree.
    #[test]
    fn dragging_a_mobile_grip_resizes_the_page() {
        use dragent_workspace_model::{BrowserOwner, ProjectId};
        use warpui::geometry::vector::vec2f;
        use warpui::{App, ReadView, UpdateView};
        use warpui_core::{Event, WindowInvalidation};
        App::test(dragent_assets::DragentAssets, |mut app| async move {
            let owner = BrowserOwner {
                organisation: "test".into(),
                project_id: ProjectId::new(),
                workspace_id: uuid::Uuid::new_v4(),
            };
            let session =
                BrowserPaneSession::new(owner, BrowserUrl::parse("https://example.com/").unwrap());
            let pane = session.pane_id;
            let (window, browser) =
                app.add_window(warpui::platform::WindowStyle::NotStealFocus, |ctx| {
                    BrowserView::new(session, None, Theme::dark(), warpui::fonts::FamilyId(0), ctx)
                });
            let presenter = app.presenter(window).unwrap();
            let rebuild = |app: &mut App| {
                browser.update(app, |view, _| {
                    view.phase = Phase::Page;
                    view.mobile = true;
                });
                app.update(|ctx| {
                    let mut presenter = presenter.borrow_mut();
                    presenter.invalidate(
                        WindowInvalidation { updated: [browser.id()].into_iter().collect(), ..Default::default() },
                        ctx,
                    );
                    presenter.build_scene(vec2f(900.0, 700.0), 1.0, None, ctx);
                });
            };
            let event = |app: &mut App, event| {
                app.update(|ctx| {
                    let result = presenter.borrow_mut().dispatch_event(event, ctx);
                    for dispatched in result.actions {
                        if let warpui_core::presenter::DispatchedActionKind::Typed(action) = dispatched.kind {
                            ctx.dispatch_typed_action_for_view(window, dispatched.view_id, action.as_ref());
                        }
                    }
                })
            };
            let bounds = |name: &str| {
                presenter
                    .borrow()
                    .position_cache()
                    .get_position(format!("browser-mobile-{name}-{pane}"))
                    .unwrap()
            };
            rebuild(&mut app);
            assert_eq!(bounds("page").width(), MOBILE_WIDTH + 2.0, "the page starts at its width");
            let grip = bounds("grip-right");
            let start = grip.center();
            event(&mut app, Event::LeftMouseDown {
                position: start,
                modifiers: Default::default(),
                click_count: 1,
                is_first_mouse: false,
            });
            for step in 1..=5 {
                event(&mut app, Event::LeftMouseDragged {
                    position: start + vec2f(10.0 * step as f32, 0.0),
                    modifiers: Default::default(),
                });
                rebuild(&mut app);
            }
            event(&mut app, Event::LeftMouseUp { position: start + vec2f(50.0, 0.0), modifiers: Default::default() });
            rebuild(&mut app);
            let width = browser.read(&app, |view, _| view.mobile_width);
            assert_eq!(width, MOBILE_WIDTH + 100.0, "dragging the right edge 50 widens by 100");
            assert_eq!(bounds("page").width(), width + 2.0, "the page is laid out at the dragged width");
        });
    }

    #[test]
    fn the_mobile_view_drags_from_either_edge_and_never_below_360() {
        let right = MobileDrag { start_x: 500.0, start_width: 393.0, side: 1.0 };
        assert_eq!(dragged_mobile_width(right, 550.0), 493.0);
        let left = MobileDrag { start_x: 300.0, start_width: 393.0, side: -1.0 };
        assert_eq!(dragged_mobile_width(left, 250.0), 493.0);
        assert_eq!(dragged_mobile_width(right, 0.0), MOBILE_MIN_WIDTH);
        assert_eq!(dragged_mobile_width(right, 5000.0), MOBILE_MAX_WIDTH);
    }

    #[test]
    fn zoom_steps_like_a_desktop_browser() {
        assert_eq!(next_zoom(1.0, 1), 1.1);
        assert_eq!(next_zoom(1.0, -1), 0.9);
        assert_eq!(next_zoom(2.0, 1), 2.0);
        assert_eq!(next_zoom(0.5, -1), 0.5);
        assert_eq!(next_zoom(1.25, 0), 1.0);
        // An off-step value moves to the nearest step in that direction.
        assert_eq!(next_zoom(1.2, 1), 1.25);
        assert_eq!(next_zoom(1.2, -1), 1.1);
    }
}

/// A `wait` or awaited `eval` checking the page.
struct AgentPoll {
    deadline: std::time::Instant,
    limit: Duration,
    waiting: bool,
    state: std::sync::Arc<std::sync::Mutex<PollState>>,
}

/// What a poll shares with the page's answers, which arrive on their own.
struct PollState {
    reply: Option<AgentReply>,
    /// What to run in the page next.
    script: String,
    /// What's still missing, for the answer if time runs out.
    detail: String,
    in_flight: bool,
    started: std::time::Instant,
    waiting: bool,
}

impl PollState {
    /// The page answered one check.
    fn answered(&mut self, raw: &str) {
        self.in_flight = false;
        match agent::progress(raw) {
            Ok(agent::Progress::Met(answer)) => {
                let answer = if self.waiting {
                    format!("{answer} (after {:.1} s)", self.started.elapsed().as_secs_f64())
                } else {
                    answer
                };
                if let Some(reply) = self.reply.take() {
                    reply(Ok(answer));
                }
            }
            Ok(agent::Progress::Waiting(detail)) => self.detail = detail,
            Ok(agent::Progress::Started(id)) => {
                self.script = agent::in_page(&format!("settled({id})"));
                self.detail = "the promise hasn't settled".into();
            }
            // A page between documents doesn't answer: that's still waiting.
            Err(error)
                if dragent_host::outcome::Failure::decode(&error)
                    .is_some_and(|failure| failure.code == "page_not_answering") =>
            {
                self.detail = "the page was navigating".into();
            }
            Err(error) => {
                if let Some(reply) = self.reply.take() {
                    reply(Err(error));
                }
            }
        }
    }
}
