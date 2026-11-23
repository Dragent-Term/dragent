//! Root view for the shared-session **web viewer** (wasm on canvas).
//!
//! A shared session can be presented in more than one way, so the viewer
//! branches on a [`ShareMode`] instead of hard-coding one layout:
//!
//! - [`ShareMode::Tab`] — the current feature. Shows *only* the shared terminal
//!   surface plus the presence navbar (this mirrors, pixel-for-pixel, the React
//!   navbar the web join page shipped: identity left, presence stack centre,
//!   Request-edit + Open-Dragent right). No sidebar, no tab bar.
//! - [`ShareMode::FullApp`] — reserved for a future "share my whole app" flow.
//!   Renders the real [`RootView`] unchanged.
//!
//! The navbar is **fixed Dragent chrome** (a dark palette independent of the
//! host's theme). The host's theme applies to the terminal *surface* below —
//! that syncs over the session (see `TerminalSurfaceState.theme_id`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty,
    Expanded, Flex, MainAxisAlignment, MainAxisSize, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::Vector2F;
use warpui::platform::Cursor;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Entity, EntityId, GetSingletonModelHandle, ReadModel, SingletonEntity,
    TypedActionView, UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{
    DispatchEventResult, EventHandler, Hoverable, MouseInBehavior, MouseStateHandle,
};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::scene::{Border, CornerRadius, Radius};

use dragent_session_protocol::Role;

use crate::icons::{render_icon, AppIcon};
use crate::realtime::manager::{SessionManager, SessionManagerEvent};
use crate::root_view::RootView;
use crate::setup::UiSettings;
use crate::terminal::view::TerminalView;
use crate::theme::Theme;
use crate::workspace::share_sheet::avatar;

type States = RefCell<HashMap<String, MouseStateHandle>>;

/// Avatars shown before collapsing the rest into "+N". Matches the React nav.
const MAX_STACK: usize = 4;
const AVATAR_SIZE: f32 = 22.0; // 28 * 0.8, rounded — 20% smaller
const BAR_HEIGHT: f32 = 44.0; // was 56; a shorter bar leaves more room for the
                              // terminal, which is the point of the page

// Fixed Dragent chrome palette (the navbar is intentionally NOT host-themed).
const fn rgba(r: u8, g: u8, b: u8, a: u8) -> ColorU {
    ColorU { r, g, b, a }
}
const CHROME_BG: ColorU = rgba(17, 18, 22, 255); // #0a0b0e + white/2%
const CHROME_BORDER: ColorU = rgba(255, 255, 255, 26); // white/10
const HOVER_BG: ColorU = rgba(255, 255, 255, 13); // white/5
const POPOVER_BG: ColorU = rgba(20, 21, 26, 255); // #14151a
const BRAND_CYAN: ColorU = rgba(34, 211, 238, 255); // brand-cyan
const CYAN_DIM: ColorU = rgba(34, 211, 238, 230);
const EMERALD: ColorU = rgba(52, 211, 153, 255); // emerald-400
const AMBER: ColorU = rgba(252, 211, 77, 255); // amber-300
const ROSE: ColorU = rgba(251, 113, 133, 255); // rose-400
const WHITE: ColorU = rgba(255, 255, 255, 255);
const GRAY200: ColorU = rgba(229, 231, 235, 255);
const GRAY300: ColorU = rgba(209, 213, 219, 255);
const GRAY400: ColorU = rgba(156, 163, 175, 255);
const GRAY500: ColorU = rgba(107, 114, 128, 255);

/// Type size for viewer CHROME (navbar, popover, toasts, cards).
///
/// Deliberately NOT [`scaled`]: that multiplies by the HOST's terminal
/// font-size setting, so a host running large type inflated the viewer's navbar
/// — on a phone that alone can push the bar out of the layout, and it is not
/// something the viewer chose. The navbar is fixed Dragent chrome (see the
/// module docs); its typography is fixed for the same reason. Sizes stay at or
/// above ~11px so they remain legible on a phone.
fn chrome_text(px: f32) -> f32 {
    px
}

/// What the host is sharing — selected per session (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareMode {
    Tab,
    FullApp,
}

#[derive(Debug, Clone)]
pub enum ViewerAction {
    ToggleParticipants,
    CloseParticipants,
    RequestEdit,
    OpenInDragent,
}

/// One person in the session, flattened for rendering.
#[derive(Clone)]
struct Chip {
    name: String,
    avatar_url: String,
    role: Role,
}

const TOAST_MS: u64 = 4600;

#[derive(Clone, Copy)]
enum ToastKind {
    Success,
    Error,
}

/// A transient notification (edit granted / declined), auto-dismissed.
struct Toast {
    id: u64,
    kind: ToastKind,
    title: String,
    message: String,
}

pub struct ViewerRoot {
    mode: ShareMode,
    view_id: EntityId,
    ui_font: FamilyId,
    /// Host theme applied to the terminal surface (synced over the session).
    theme: Theme,
    session: Option<String>,
    secret: Option<String>,
    mirror: Option<ViewHandle<TerminalView>>,
    full: Option<ViewHandle<RootView>>,
    participants_open: bool,
    /// This viewer requested Edit and is awaiting the host's approval.
    edit_pending: bool,
    /// Set when the host ends the session — replaces the surface with a card.
    ended: Option<String>,
    /// Transient notifications (edit granted / declined).
    toasts: Vec<Toast>,
    next_toast_id: u64,
    states: States,
    /// Shared by render closures so pointer traffic can be rate-limited without
    /// dispatching a noisy typed action for every browser mousemove.
    pointer_tx: Rc<RefCell<PointerTxState>>,
}

#[derive(Default)]
struct PointerTxState {
    last_sent: Option<web_time::Instant>,
    last_position: Option<Vector2F>,
}

impl ViewerRoot {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        Self::with_mode(ShareMode::Tab, ctx)
    }

    pub fn with_mode(mode: ShareMode, ctx: &mut ViewContext<Self>) -> Self {
        let view_id = ctx.view_id();
        let session = url_param("session");
        let secret = url_param("secret");

        let font_cache = ctx.get_singleton_model_handle::<FontCache>();
        let ui_font = ctx.update_model(&font_cache, |cache, _| {
            crate::workspace::prewarm::load_ui_font(cache)
        });
        let theme = UiSettings::load().theme();

        // Drive the mirror + re-render from session events (streamed output,
        // presence, control grant, theme).
        let sm = SessionManager::handle(ctx);
        ctx.subscribe_to_model(&sm, |this: &mut ViewerRoot, handle, event, ctx| {
            match event {
                // Streamed host output → the mirror terminal (renders identically).
                SessionManagerEvent::MirrorOutput { bytes } => {
                    if let Some(m) = this.mirror.clone() {
                        let bytes = bytes.clone();
                        ctx.update_view(&m, |tv, tctx| tv.feed_remote_output(&bytes, tctx));
                    }
                }
                // The host's own block rendering — authoritative, applied AFTER
                // the raw feed so it overwrites whatever the re-parse produced.
                SessionManagerEvent::MirrorBlock { delta } => {
                    if let Some(m) = this.mirror.clone() {
                        let delta = delta.clone();
                        ctx.update_view(&m, |tv, tctx| tv.apply_remote_block_delta(&delta, tctx));
                    }
                }
                SessionManagerEvent::MirrorAgentBlocks { blocks } => {
                    if let Some(m) = this.mirror.clone() {
                        let blocks = blocks.clone();
                        ctx.update_view(&m, |tv, tctx| tv.apply_remote_agent_blocks(blocks, tctx));
                    }
                }
                // Host's input-layer surface state → the mirror, so its AI-tool
                // input panel matches the host's (Claude Code, Cursor Agent, …).
                SessionManagerEvent::MirrorSurface { state } => {
                    if let Some(m) = this.mirror.clone() {
                        let state = state.clone();
                        ctx.update_view(&m, |tv, tctx| tv.apply_remote_surface(&state, tctx));
                    }
                }
                // Host ended the session — replace the surface with a card.
                // Exception: an anonymous viewer rejected from an edit link for
                // lack of a login is handed to the web sign-in flow instead of a
                // dead "session ended" screen; it reloads us with a token.
                SessionManagerEvent::Ended(reason) => {
                    if reason_requires_login(reason) {
                        request_web_login();
                    } else {
                        this.ended = Some(reason.clone());
                    }
                }
                // Host declined this viewer's edit request.
                SessionManagerEvent::AccessDenied => {
                    this.edit_pending = false;
                    this.push_toast(
                        ToastKind::Error,
                        "Edit request declined",
                        "The host declined your request — you can still watch.",
                        ctx,
                    );
                }
                _ => {}
            }
            let (can_edit, is_host_account, theme_id) = handle.read(ctx, |m, _| {
                (
                    m.viewer_can_edit(),
                    m.viewer_is_host_account(),
                    m.viewer_theme_id(),
                )
            });
            // Edit just granted (was pending): confirm it, clear the pill.
            if can_edit && this.edit_pending {
                this.edit_pending = false;
                this.push_toast(
                    ToastKind::Success,
                    "You can edit now",
                    "The host granted you edit access.",
                    ctx,
                );
            }
            // Enable/disable the mirror's input in lock-step with control: a
            // viewer without edit access gets the disabled padlock field.
            if let Some(m) = this.mirror.clone() {
                ctx.update_view(&m, move |tv, tctx| {
                    tv.set_remote_read_only(!can_edit, tctx);
                    // Only a controller that is the host's own account may drive
                    // the host's size; a teammate adopts the host's grid.
                    tv.set_remote_is_host_account(is_host_account, tctx);
                });
            }
            // Apply the host's theme to the terminal surface (background +
            // bundled image background) and the mirror — live, on every change.
            if let Some(id) = theme_id {
                if id.as_str() != this.theme.id {
                    let theme = Theme::from_id(&id);
                    this.theme = theme;
                    if let Some(m) = this.mirror.clone() {
                        ctx.update_view(&m, |tv, tctx| tv.set_theme(theme, tctx));
                    }
                }
            }
            ctx.notify();
        });

        let (mirror, full) = match mode {
            ShareMode::Tab => {
                // The mirror's input (typed keys + alt-screen keystrokes) forwards
                // to the host through the session. The server drops it unless this
                // viewer holds Control, so the sink can always forward.
                let (input_tx, input_rx) = async_channel::unbounded::<Vec<u8>>();
                let sink: crate::terminal::manager::pty_writer::RemoteInputSink =
                    std::sync::Arc::new(move |bytes: &[u8]| {
                        let _ = input_tx.try_send(bytes.to_vec());
                    });
                let cwd = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                let mirror = ctx
                    .add_typed_action_view(move |tctx| TerminalView::new_remote(cwd, sink, tctx));

                // Connect as a viewer using the credentials from the page URL.
                // The signed-in viewer's name (injected by the host page) makes
                // the gateway attach their identity; anonymous viewers are "Viewer".
                if let (Some(sess), Some(sec)) = (session.clone(), secret.clone()) {
                    let name = viewer_display_name();
                    let sm_conn = SessionManager::handle(ctx);
                    ctx.update_model(&sm_conn, move |m, mctx| {
                        if let Err(e) = m.start_viewer(mctx, &sess, &sec, name) {
                            log::warn!("[viewer] bad join credentials: {e}");
                        }
                        // Pump mirror input → host for the life of the session.
                        mctx.spawn_stream_local(
                            input_rx,
                            |m, bytes: Vec<u8>, _mctx| m.send_input(&bytes),
                            |_, _| {},
                        );
                    });
                }
                (Some(mirror), None)
            }
            ShareMode::FullApp => {
                let full = ctx.add_typed_action_view(RootView::new_empty);
                (None, Some(full))
            }
        };

        Self {
            mode,
            view_id,
            ui_font,
            theme,
            session,
            secret,
            mirror,
            full,
            participants_open: false,
            edit_pending: false,
            ended: None,
            toasts: Vec::new(),
            next_toast_id: 0,
            states: RefCell::new(HashMap::new()),
            pointer_tx: Rc::new(RefCell::new(PointerTxState::default())),
        }
    }

    /// Show a transient toast that auto-dismisses after [`TOAST_MS`].
    fn push_toast(
        &mut self,
        kind: ToastKind,
        title: &str,
        message: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(Toast {
            id,
            kind,
            title: title.to_string(),
            message: message.to_string(),
        });
        ctx.spawn(
            Timer::after(Duration::from_millis(TOAST_MS)),
            move |this, _, ctx| {
                this.toasts.retain(|t| t.id != id);
                ctx.notify();
            },
        );
        ctx.notify();
    }

    fn chips(&self, app: &AppContext) -> Vec<Chip> {
        let sm = SessionManager::handle(app);
        app.read_model(&sm, |m, _| {
            m.participants()
                .iter()
                .map(|p| Chip {
                    name: p.display_name.clone(),
                    avatar_url: p.avatar_url.clone(),
                    role: p.role,
                })
                .collect()
        })
    }

    fn can_edit(&self, app: &AppContext) -> bool {
        let sm = SessionManager::handle(app);
        app.read_model(&sm, |m, _| m.viewer_can_edit())
    }

    fn mouse_state(&self, key: &str) -> MouseStateHandle {
        self.states
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// The presence navbar — a 1:1 port of the React web join navbar.
    fn navbar(&self, chips: &[Chip], can_edit: bool) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.view_id;
        // On phone-width screens the desktop-only "Open in Dragent" action and
        // the wordy identity label are dropped so the bar fits (and we never ask
        // a mobile viewer to open/download the desktop app).
        let narrow = viewport_width() < 640.0;

        // `MainAxisSize::Max` fills the bar width so SpaceBetween can push the
        // three groups to left / centre / right (the column stretches this
        // container to full width — see `render`).
        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_child(self.identity())
            .with_child(self.avatar_stack(chips))
            .with_child(self.right_actions(ui, id, can_edit, narrow))
            .finish();

        // Transparent fill so the themed background (image or colour) shows
        // through the bar — only the hairline bottom border separates it. The
        // border colour is a translucent white, so the background still reads.
        Container::new(row)
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top((BAR_HEIGHT - AVATAR_SIZE) / 2.0)
            .with_padding_bottom((BAR_HEIGHT - AVATAR_SIZE) / 2.0)
            .with_border(Border::bottom(1.0).with_border_color(CHROME_BORDER))
            .finish()
    }

    fn identity(&self) -> Box<dyn Element> {
        // The real Dragent icon (bundled PNG, wasm-available via DragentAssets),
        // not the old cyan-square placeholder.
        let logo = ConstrainedBox::new(
            warpui::elements::Image::new(
                AssetSource::Bundled {
                    path: "icons/dragent-icon.png",
                },
                warpui::elements::CacheOption::Original,
            )
            .cover()
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
        )
        .with_width(AVATAR_SIZE)
        .with_height(AVATAR_SIZE)
        .finish();

        // Icon + live dot only. The "Shared session" wordmark said nothing the
        // page doesn't already make obvious, and it was the widest thing in the
        // bar — dropping it is what lets the same layout work on a phone without
        // a separate narrow branch.
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(logo)
            .with_child(dot(EMERALD, 6.0))
            .finish()
    }

    /// Overlapping, ring'd presence avatars; overflow collapses to "+N". The
    /// whole stack is one click target that toggles the participant popover.
    fn avatar_stack(&self, chips: &[Chip]) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.view_id;
        let theme = self.theme;
        let shown: Vec<Chip> = chips.iter().take(MAX_STACK).cloned().collect();
        let overflow = chips.len().saturating_sub(MAX_STACK);
        let empty = chips.is_empty();
        let hover = self.mouse_state("participant_stack");

        Box::new(
            Hoverable::new(hover, move |st| {
                let bg = if st.is_hovered() {
                    HOVER_BG
                } else {
                    ColorU::new(0, 0, 0, 0)
                };
                let inner: Box<dyn Element> = if empty {
                    Container::new(
                        Text::new("No one else here yet".to_string(), ui, chrome_text(11.5))
                            .with_color(GRAY500)
                            .finish(),
                    )
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .finish()
                } else {
                    let mut stack = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(-8.0); // -space-x-2
                    for c in &shown {
                        stack =
                            stack.with_child(ring(avatar(&c.name, "", &c.avatar_url, &theme, ui)));
                    }
                    if overflow > 0 {
                        stack = stack.with_child(plus_n(overflow, ui));
                    }
                    stack.finish()
                };
                Container::new(inner)
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0)
                    .with_background_color(bg)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(16.0)))
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ectx, _, _| {
                ectx.dispatch_typed_action_for(id, ViewerAction::ToggleParticipants)
            }),
        )
    }

    fn right_actions(
        &self,
        ui: FamilyId,
        id: EntityId,
        can_edit: bool,
        narrow: bool,
    ) -> Box<dyn Element> {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0);

        // Control state pill / request button.
        row = row.with_child(if can_edit {
            if narrow {
                state_pill_icon(AppIcon::Check, EMERALD)
            } else {
                state_pill("You can edit", AppIcon::Check, EMERALD, ui)
            }
        } else if self.edit_pending {
            if narrow {
                state_pill_icon(AppIcon::Edit, AMBER)
            } else {
                state_pill("Requested…", AppIcon::Edit, AMBER, ui)
            }
        } else {
            request_edit_button(ui, id, self.mouse_state("request_edit"))
        });

        // "Open in Dragent" is desktop-only — never shown on phone widths, where
        // there's no desktop app to hand off to.
        if !narrow {
            row = row.with_child(open_dragent_button(
                ui,
                id,
                self.mouse_state("open_dragent"),
            ));
        }

        row.finish()
    }

    /// Popover listing each participant: avatar + name + role.
    fn participants_popover(&self, chips: &[Chip]) -> Box<dyn Element> {
        let ui = self.ui_font;
        let theme = self.theme;

        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(
                Container::new(
                    Text::new("IN THIS SESSION".to_string(), ui, chrome_text(10.5))
                        .with_color(GRAY500)
                        .finish(),
                )
                .with_padding_left(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish(),
            );
        for c in chips {
            col = col.with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(10.0)
                        .with_child(avatar(&c.name, "", &c.avatar_url, &theme, ui))
                        .with_child(
                            Flex::column()
                                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                                .with_main_axis_size(MainAxisSize::Min)
                                .with_child(
                                    Text::new(display_name(&c.name), ui, chrome_text(12.5))
                                        .with_color(GRAY200)
                                        .finish(),
                                )
                                .with_child(
                                    Text::new(role_label(c.role), ui, chrome_text(10.5))
                                        .with_color(GRAY500)
                                        .finish(),
                                )
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish(),
            );
        }

        let card = Container::new(col.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(POPOVER_BG)
            .with_border(Border::all(1.0).with_border_color(CHROME_BORDER))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .finish();

        // Only the card itself swallows clicks — the surrounding centring row
        // has NO handler, so clicks in its transparent regions (over the navbar,
        // the avatar stack, and either side of the card) fall through to the
        // dismiss scrim below and close the popover.
        let card = EventHandler::new(card)
            .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
            .finish();

        // Centred under the bar (matches `left-1/2 -translate-x-1/2 mt-2`):
        // a full-width, top-aligned row centres the card horizontally, and the
        // card's top padding drops it just below the navbar.
        let positioned = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(
                Container::new(card)
                    .with_padding_top(BAR_HEIGHT + 4.0)
                    .finish(),
            )
            .finish();

        // Full-screen invisible scrim under the card: a click anywhere outside
        // the card (including re-clicking the avatar stack, which now sits
        // beneath it) dismisses the popover.
        let id = self.view_id;
        let scrim = Container::new(Align::new(Empty::new().finish()).finish())
            .with_background_color(ColorU::new(0, 0, 0, 0))
            .finish();
        let scrim = EventHandler::new(scrim)
            .on_left_mouse_down(move |ectx, _, _| {
                ectx.dispatch_typed_action_for(id, ViewerAction::CloseParticipants);
                DispatchEventResult::StopPropagation
            })
            .finish();

        Stack::new()
            .with_child(scrim)
            .with_child(positioned)
            .finish()
    }

    /// Wrap `content` with the theme's background. For an image theme: an opaque
    /// base + the (opacity-tuned) image behind the content, whose translucent
    /// terminal fill reveals it — the same layering the desktop workspace uses,
    /// so the host's image background shows here too. Plain themes just get the
    /// background colour.
    fn themed_background(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        match crate::theme::Theme::bg_image_for(self.theme.id) {
            Some(bg) => {
                let base = self.theme.background;
                let opaque_base = Container::new(Empty::new().finish())
                    .with_background_color(ColorU {
                        r: base.r,
                        g: base.g,
                        b: base.b,
                        a: 255,
                    })
                    .finish();
                // `Original` (GPU scales to cover) — `BySize` resizes the multi-MB
                // source on the UI thread and janks the first frame.
                let image = warpui::elements::Image::new(
                    bg.asset_source(),
                    warpui::elements::CacheOption::Original,
                )
                .cover()
                .with_opacity((bg.opacity as f32 / 100.0).clamp(0.0, 1.0))
                .finish();
                Stack::new()
                    .with_child(opaque_base)
                    .with_child(image)
                    .with_child(content)
                    .finish()
            }
            None => Container::new(content)
                .with_background_color(self.theme.background)
                .finish(),
        }
    }

    /// Full-screen card shown after the host ends the session.
    fn ended_screen(&self, reason: &str) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.view_id;
        let narrow = viewport_width() < 640.0;

        let icon = ConstrainedBox::new(
            Container::new(render_icon(AppIcon::Share, GRAY400, 38.0))
                .with_background_color(POPOVER_BG)
                .with_border(Border::all(1.0).with_border_color(CHROME_BORDER))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(20.0)))
                .finish(),
        )
        .with_width(if narrow { 76.0 } else { 88.0 })
        .with_height(if narrow { 76.0 } else { 88.0 })
        .finish();
        let mut card = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(14.0)
            .with_child(icon)
            .with_child(
                Text::new("Session ended".to_string(), ui, chrome_text(26.0))
                    .with_color(WHITE)
                    .with_style(Properties {
                        weight: Weight::Semibold,
                        ..Default::default()
                    })
                    .finish(),
            );
        let msg = if reason.trim().is_empty() {
            "The host stopped sharing this session.".to_string()
        } else {
            reason.to_string()
        };
        card = card.with_child(
            Container::new(
                Text::new(msg, ui, chrome_text(14.0))
                    .with_color(GRAY400)
                    .finish(),
            )
            .with_padding_left(if narrow { 4.0 } else { 28.0 })
            .with_padding_right(if narrow { 4.0 } else { 28.0 })
            .finish(),
        );
        if !narrow {
            card = card.with_child(open_dragent_button(ui, id, self.mouse_state("ended_open")));
        }

        let card = ConstrainedBox::new(
            Container::new(card.finish())
                .with_padding_left(if narrow { 24.0 } else { 42.0 })
                .with_padding_right(if narrow { 24.0 } else { 42.0 })
                .with_padding_top(if narrow { 28.0 } else { 42.0 })
                .with_padding_bottom(if narrow { 28.0 } else { 42.0 })
                .with_background_color(CHROME_BG)
                .with_border(Border::all(1.0).with_border_color(CHROME_BORDER))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(20.0)))
                .finish(),
        )
        .with_max_width(620.0)
        .finish();

        Container::new(warpui::elements::Align::new(card).finish())
            .with_padding_left(if narrow { 16.0 } else { 36.0 })
            .with_padding_right(if narrow { 16.0 } else { 36.0 })
            .with_background_color(CHROME_BG)
            .finish()
    }

    /// Bottom-right stack of transient toasts.
    fn toasts_overlay(&self) -> Box<dyn Element> {
        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::End)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(8.0);
        for t in &self.toasts {
            col = col.with_child(self.toast_card(t));
        }
        // Fill height → push to the top; fill width → push to the right. Top
        // padding clears the navbar so toasts drop in just below it.
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_child(
                        Container::new(col.finish())
                            .with_padding_right(16.0)
                            .with_padding_top(BAR_HEIGHT + 12.0)
                            .finish(),
                    )
                    .finish(),
            )
            .finish()
    }

    fn toast_card(&self, t: &Toast) -> Box<dyn Element> {
        let ui = self.ui_font;
        let accent = match t.kind {
            ToastKind::Success => EMERALD,
            ToastKind::Error => ROSE,
        };
        let text_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(
                Text::new(t.title.clone(), ui, chrome_text(12.5))
                    .with_color(WHITE)
                    .with_style(Properties {
                        weight: Weight::Semibold,
                        ..Default::default()
                    })
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(t.message.clone(), ui, chrome_text(11.5))
                        .with_color(GRAY300)
                        .finish(),
                )
                .with_padding_top(2.0)
                .finish(),
            )
            .finish();

        ConstrainedBox::new(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(10.0)
                    .with_child(dot(accent, 8.0))
                    .with_child(Expanded::new(1.0, text_col).finish())
                    .finish(),
            )
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .with_background_color(POPOVER_BG)
            .with_border(Border::all(1.0).with_border_color(CHROME_BORDER))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .finish(),
        )
        .with_width(300.0)
        .finish()
    }
}

impl Entity for ViewerRoot {
    type Event = ();
}

impl View for ViewerRoot {
    fn ui_name() -> &'static str {
        "ViewerRoot"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        match self.mode {
            ShareMode::FullApp => match &self.full {
                Some(full) => ChildView::new(full).finish(),
                None => Empty::new().finish(),
            },
            ShareMode::Tab => {
                // The host ended the session — show a full-screen card instead.
                if let Some(reason) = &self.ended {
                    return self.ended_screen(reason);
                }

                let chips = self.chips(app);
                let can_edit = self.can_edit(app);
                let terminal: Box<dyn Element> = match &self.mirror {
                    Some(m) => ChildView::new(m).finish(),
                    None => Empty::new().finish(),
                };
                let column = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(self.navbar(&chips, can_edit))
                    .with_child(Expanded::new(1.0, terminal).finish())
                    .finish();

                let mut stack = Stack::new().with_child(column);
                if self.participants_open && !chips.is_empty() {
                    stack = stack.with_child(self.participants_popover(&chips));
                }
                if !self.toasts.is_empty() {
                    stack = stack.with_child(self.toasts_overlay());
                }

                let content = self.themed_background(stack.finish());
                let pointer_tx = self.pointer_tx.clone();
                EventHandler::new(content)
                    .with_always_handle()
                    .on_mouse_in(
                        move |_event_ctx, app, position| {
                            let now = web_time::Instant::now();
                            let mut tx = pointer_tx.borrow_mut();
                            let moved = tx
                                .last_position
                                .is_none_or(|previous| (previous - position).length() >= 1.0);
                            let due = tx.last_sent.is_none_or(|last| {
                                now.duration_since(last) >= Duration::from_millis(33)
                            });
                            if moved && due {
                                tx.last_sent = Some(now);
                                tx.last_position = Some(position);
                                let width = viewport_width().max(1.0);
                                let height = viewport_height().max(1.0);
                                let sm = SessionManager::handle(app);
                                app.read_model(&sm, |manager, _| {
                                    manager.send_pointer(
                                        dragent_session_protocol::PointerPosition {
                                            x: (position.x() / width).clamp(0.0, 1.0),
                                            y: (position.y() / height).clamp(0.0, 1.0),
                                        },
                                    );
                                });
                            }
                            DispatchEventResult::PropagateToParent
                        },
                        Some(MouseInBehavior {
                            fire_on_synthetic_events: false,
                            fire_when_covered: true,
                        }),
                    )
                    .finish()
            }
        }
    }
}

impl TypedActionView for ViewerRoot {
    type Action = ViewerAction;

    fn handle_action(&mut self, action: &ViewerAction, ctx: &mut ViewContext<Self>) {
        match action {
            ViewerAction::ToggleParticipants => {
                self.participants_open = !self.participants_open;
                ctx.notify();
            }
            ViewerAction::CloseParticipants => {
                if self.participants_open {
                    self.participants_open = false;
                    ctx.notify();
                }
            }
            ViewerAction::RequestEdit => {
                let sm = SessionManager::handle(ctx);
                ctx.read_model(&sm, |m, _| m.request_control());
                self.edit_pending = true;
                ctx.notify();
            }
            ViewerAction::OpenInDragent => {
                open_in_dragent_app(self.session.as_deref(), self.secret.as_deref());
            }
        }
    }
}

// ── Bits ──────────────────────────────────────────────────────────────────────

fn display_name(name: &str) -> String {
    if name.trim().is_empty() {
        "Guest".to_string()
    } else {
        name.to_string()
    }
}

fn role_label(role: Role) -> String {
    match role {
        Role::Host => "Host · Full access".to_string(),
        Role::Controller => "Can edit".to_string(),
        Role::Viewer => "Viewing".to_string(),
    }
}

fn dot(color: ColorU, size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(size / 2.0)))
            .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

/// Wrap a stacked avatar in a ring so overlapping avatars stay separated
/// (React `border-2 border-[#0a0b0e]`).
fn ring(inner: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(inner)
        .with_border(Border::all(2.0).with_border_color(CHROME_BG))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
            AVATAR_SIZE / 2.0 + 2.0,
        )))
        .finish()
}

/// The "+N" chip that caps the presence stack.
fn plus_n(n: usize, ui: FamilyId) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Align::new(
                Text::new(format!("+{n}"), ui, chrome_text(10.0))
                    .with_color(GRAY300)
                    .with_style(Properties {
                        weight: Weight::Semibold,
                        ..Default::default()
                    })
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(ColorU::new(255, 255, 255, 26))
        .with_border(Border::all(2.0).with_border_color(CHROME_BG))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
            AVATAR_SIZE / 2.0 + 2.0,
        )))
        .finish(),
    )
    .with_width(AVATAR_SIZE)
    .with_height(AVATAR_SIZE)
    .finish()
}

/// A non-interactive control-state pill ("You can edit" / "Requested…").
/// Icon-only state pill for phone widths: the label ("You can edit") costs more
/// horizontal room than the navbar has, and squeezed the presence stack and the
/// request button. The icon alone carries the same state.
fn state_pill_icon(icon: AppIcon, color: ColorU) -> Box<dyn Element> {
    Container::new(render_icon(icon, color, 14.0))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(ColorU::new(color.r, color.g, color.b, 26))
        .with_border(Border::all(1.0).with_border_color(ColorU::new(color.r, color.g, color.b, 51)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish()
}

fn state_pill(label: &str, icon: AppIcon, color: ColorU, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(render_icon(icon, color, 13.0))
            .with_child(
                Text::new(label.to_string(), ui, chrome_text(12.0))
                    .with_color(color)
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .with_background_color(ColorU::new(color.r, color.g, color.b, 26))
    .with_border(Border::all(1.0).with_border_color(ColorU::new(color.r, color.g, color.b, 51)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish()
}

/// A small dark tooltip bubble, shown above whatever it is stacked onto.
fn tooltip_bubble(label: &str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(label.to_string(), ui, chrome_text(11.0))
            .with_color(GRAY200)
            .finish(),
    )
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(5.0)
    .with_padding_bottom(5.0)
    .with_background_color(POPOVER_BG)
    .with_border(Border::all(1.0).with_border_color(CHROME_BORDER))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}

/// Request-edit control for a viewer WITHOUT edit permission: an icon-only
/// stroked button. The label lived in the bar permanently for a control that is
/// pressed once, so it is a tooltip now — which is also what keeps the bar
/// usable on a phone, where a text button plus the presence stack would not fit.
fn request_edit_button(ui: FamilyId, id: EntityId, hover: MouseStateHandle) -> Box<dyn Element> {
    let hovered = hover.lock().unwrap().is_hovered();
    let button = Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() {
                HOVER_BG
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            Container::new(Align::new(render_icon(AppIcon::Edit, GRAY200, 14.0)).finish())
                // Square-ish so the icon sits centred, not letterboxed in a pill.
                .with_padding_left(7.0)
                .with_padding_right(7.0)
                .with_padding_top(7.0)
                .with_padding_bottom(7.0)
                .with_background_color(bg)
                .with_border(Border::all(1.0).with_border_color(CHROME_BORDER))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, ViewerAction::RequestEdit)),
    );
    if !hovered {
        return button;
    }
    Stack::new()
        .with_child(button)
        .with_positioned_overlay_child(
            tooltip_bubble("Request edit access", ui),
            OffsetPositioning::offset_from_parent(
                Vector2F::new(0.0, -6.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::TopMiddle,
                ChildAnchor::BottomMiddle,
            ),
        )
        .finish()
}

fn open_dragent_button(ui: FamilyId, id: EntityId, hover: MouseStateHandle) -> Box<dyn Element> {
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() {
                CYAN_DIM
            } else {
                BRAND_CYAN
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(
                        Text::new("Open Dragent".to_string(), ui, chrome_text(12.0))
                            .with_color(WHITE)
                            .with_style(Properties {
                                weight: Weight::Semibold,
                                ..Default::default()
                            })
                            .finish(),
                    )
                    .with_child(
                        Text::new("↗".to_string(), ui, chrome_text(12.0))
                            .with_color(WHITE)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(id, ViewerAction::OpenInDragent)
        }),
    )
}

/// The signed-in viewer's display name, injected by the host page
/// (`window.__DRAGENT_DISPLAY_NAME__`). Falls back to "Viewer" (anonymous/native).
fn viewer_display_name() -> String {
    #[cfg(target_family = "wasm")]
    {
        crate::realtime::browser_global("__DRAGENT_DISPLAY_NAME__")
            .unwrap_or_else(|| "Viewer".to_string())
    }
    #[cfg(not(target_family = "wasm"))]
    {
        "Viewer".to_string()
    }
}

/// Current browser viewport width in CSS pixels, for responsive layout. Off
/// wasm (desktop viewer) it returns a wide default so the full navbar shows.
/// warpui re-renders on resize, so reading this each frame tracks the width.
fn viewport_width() -> f32 {
    #[cfg(target_family = "wasm")]
    {
        web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|v| v.as_f64())
            .map(|w| w as f32)
            .filter(|w| *w > 0.0)
            .unwrap_or(1280.0)
    }
    #[cfg(not(target_family = "wasm"))]
    {
        1280.0
    }
}

fn viewport_height() -> f32 {
    #[cfg(target_family = "wasm")]
    {
        web_sys::window()
            .and_then(|w| w.inner_height().ok())
            .and_then(|v| v.as_f64())
            .map(|h| h as f32)
            .filter(|h| *h > 0.0)
            .unwrap_or(800.0)
    }
    #[cfg(not(target_family = "wasm"))]
    {
        800.0
    }
}

/// Read a query param from the page URL (`?session=…&secret=…`).
fn url_param(name: &str) -> Option<String> {
    #[cfg(target_family = "wasm")]
    {
        let search = web_sys::window()?.location().search().ok()?;
        if search.is_empty() {
            return None;
        }
        let parsed = url::Url::parse(&format!("http://x/{search}")).ok()?;
        parsed
            .query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let _ = name;
        None
    }
}

/// The desktop app's join deep link for this edition:
/// `<scheme>://join?session=…&secret=…`.
fn dragent_join_link(session: Option<&str>, secret: Option<&str>) -> String {
    let scheme = dragent_paths::url_scheme();
    match (session, secret) {
        (Some(s), Some(sec)) => {
            let q = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("session", s)
                .append_pair("secret", sec)
                .finish();
            format!("{scheme}://join?{q}")
        }
        _ => format!("{scheme}://join"),
    }
}

/// Whether an `Ended` reason means the viewer must sign in — an anonymous
/// viewer hit an **edit** link, which the gateway only admits with an account.
/// Matches the gateway's "login required" wording.
fn reason_requires_login(reason: &str) -> bool {
    reason.to_ascii_lowercase().contains("login")
}

/// Ask the host web page to show its sign-in card (via the `__DRAGENT_NEEDS_LOGIN__`
/// hook `WasmViewer` installs). React swaps the canvas for the login UI and
/// reloads the viewer with a token afterwards, so the session opens straight
/// away. No-op off wasm (a native viewer has no host page to defer to).
fn request_web_login() {
    #[cfg(target_family = "wasm")]
    {
        use wasm_bindgen::JsCast;
        if let Some(win) = web_sys::window() {
            if let Ok(cb) = js_sys::Reflect::get(
                &win,
                &wasm_bindgen::JsValue::from_str("__DRAGENT_NEEDS_LOGIN__"),
            ) {
                if let Some(func) = cb.dyn_ref::<js_sys::Function>() {
                    let _ = func.call0(&wasm_bindgen::JsValue::NULL);
                }
            }
        }
    }
}

fn open_in_dragent_app(session: Option<&str>, secret: Option<&str>) {
    let link = dragent_join_link(session, secret);
    #[cfg(target_family = "wasm")]
    {
        if let Some(win) = web_sys::window() {
            let _ = win.location().set_href(&link);
        }
    }
    #[cfg(not(target_family = "wasm"))]
    {
        log::info!("[viewer] Open in Dragent → {link}");
    }
}
