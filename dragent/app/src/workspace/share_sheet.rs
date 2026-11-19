//! The share-session sheet — a modal to invite people to, and manage access of,
//! a live shared terminal session.
//!
//! Layout (top → bottom): a "Live session started at …" title; an email-invite
//! row (address field + a View/Edit role selector + Invite); a "Who has access"
//! list — *Anyone with the link* (with an access dropdown) and each participant
//! (the host shows *Full access*); and a footer with the session URL, a QR
//! button, and Copy link. Dropdowns and the QR view are rendered as overlays.
//!
//! Rendered from a [`ShareSnapshot`] the workspace caches from
//! [`crate::realtime::manager::SessionManager`], plus a live `InputEditor` handle
//! for the email field. Styled to match the app's own menus and modals.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{EntityId, ViewHandle};
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};
use crate::workspace::InputEditor;

use super::action::{LinkAccess, ShareDropdown, WorkspaceAction};

type States = RefCell<HashMap<String, MouseStateHandle>>;

const PANEL_W: f32 = 560.0;
const MENU_W: f32 = 230.0;
const TRANSPARENT: ColorU = ColorU {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

/// One participant row's display data.
#[derive(Clone, Debug, Default)]
pub struct ShareParticipant {
    pub id: String,
    pub name: String,
    pub email: String,
    /// Profile photo URL; empty falls back to an initials avatar.
    pub avatar_url: String,
    /// True = Edit (can run commands), false = View.
    pub control: bool,
    /// The host — shown as "Full access", access not editable.
    pub is_host: bool,
}

/// A UI snapshot of the shared session.
#[derive(Clone, Debug)]
pub struct ShareSnapshot {
    pub active: bool,
    pub online: bool,
    /// e.g. "9:07pm on 07/23".
    pub started_label: String,
    /// What "Anyone with the link" currently grants.
    pub link_access: LinkAccess,
    /// The shareable URL, once a link exists.
    pub link_url: Option<String>,
    /// The role the email-invite field will grant (true = Edit).
    pub invite_control: bool,
    pub participants: Vec<ShareParticipant>,
    /// Which dropdown, if any, is open.
    pub open_dropdown: Option<ShareDropdown>,
    /// The current connection error, if not online (shown in the header).
    pub error: Option<String>,
    /// A share action that is waiting on the network. While set, the control
    /// that started it shows a spinner and is inert, so the user gets immediate
    /// acknowledgement instead of a button that looks like it did nothing.
    pub busy: Option<ShareOp>,
    /// Transient confirmation ("Link copied", "Invite sent") shown for a couple of
    /// seconds after an action succeeds, then cleared by a timer.
    pub notice: Option<String>,
}

/// A share action that talks to the network, so the UI can show which control is
/// waiting and confirm it afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareOp {
    /// Creating or revoking the "anyone with the link" link.
    LinkAccess,
    /// Emailing an invite.
    Invite,
}

impl Default for ShareSnapshot {
    fn default() -> Self {
        Self {
            active: false,
            online: false,
            started_label: String::new(),
            link_access: LinkAccess::None,
            link_url: None,
            invite_control: false,
            participants: Vec::new(),
            open_dropdown: None,
            error: None,
            busy: None,
            notice: None,
        }
    }
}

fn tint(c: ColorU, a: u8) -> ColorU {
    ColorU {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

fn role_label(control: bool) -> &'static str {
    if control {
        "Can edit"
    } else {
        "Can view"
    }
}

fn link_access_label(a: LinkAccess) -> &'static str {
    match a {
        LinkAccess::None => "No access",
        LinkAccess::View => "Can view",
        LinkAccess::Edit => "Can edit",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('\u{2026}');
    t
}

/// Render the sheet as a full-screen modal (scrim + centered panel), plus any
/// open dropdown as a positioned overlay above it.
pub fn render_share_sheet(
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    email_editor: &ViewHandle<InputEditor>,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(16.0)
        .with_child(title(snap, theme, ui, id, states));

    // Transient confirmation for the action just performed. Every control here
    // either reaches the network or writes the clipboard, and none of them
    // changed anything visible on their own — so without this a click gave no
    // sign it had worked. Timed out by the view (see `show_share_notice`).
    if let Some(notice) = snap.notice.as_deref() {
        column = column.with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(crate::icons::render_icon(
                        crate::icons::AppIcon::Check,
                        theme.ansi_green(),
                        crate::icons::ICON_SIZE_SM,
                    ))
                    .with_child(
                        Text::new(notice.to_string(), ui, crate::theme::text_size(12.0))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish(),
        );
    }
    if let Some(err) = snap.error.as_ref().filter(|_| !snap.online) {
        column = column.with_child(error_banner(err, theme, ui));
    }
    let panel = Container::new(
        column
            .with_child(email_row(snap, theme, ui, email_editor, id, states))
            .with_child(access_section(snap, theme, ui, id, states))
            .with_child(url_row(snap, theme, ui, mono, id, states))
            .with_child(stop_footer(theme, ui, id, states))
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_padding_left(24.0)
    .with_padding_right(24.0)
    .with_padding_top(22.0)
    .with_padding_bottom(22.0)
    .finish();

    // Clicks inside the panel must not dismiss the sheet.
    let panel = EventHandler::new(panel)
        .on_left_mouse_down(|_e, _a, _p| DispatchEventResult::StopPropagation)
        .finish();
    let panel = ConstrainedBox::new(panel).with_max_width(PANEL_W).finish();

    let sheet_scrim = EventHandler::new(
        Container::new(Align::new(panel).finish())
            .with_background_color(ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 150,
            })
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(id, WorkspaceAction::CloseShareSheet);
        DispatchEventResult::StopPropagation
    })
    .finish();

    let mut stack = Stack::new().with_child(sheet_scrim);
    if let Some(dd) = snap.open_dropdown.clone() {
        let (menu, offset) = dropdown(&dd, snap, theme, ui, id, states);
        // Transparent full-screen catcher: a click anywhere dismisses the dropdown.
        let catcher = EventHandler::new(
            Container::new(Empty::new().finish())
                .with_background_color(TRANSPARENT)
                .finish(),
        )
        .on_left_mouse_down(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(id, WorkspaceAction::CloseShareDropdown);
            DispatchEventResult::StopPropagation
        })
        .finish();
        stack = stack
            .with_child(catcher)
            .with_positioned_child(menu, offset);
    }
    Box::new(stack)
}

fn title(
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let live = snap.online || snap.error.is_none();
    let text = match (live, snap.started_label.is_empty()) {
        (true, true) => "Live session".to_string(),
        (true, false) => format!("Live session started at  {}", snap.started_label),
        (false, _) => "Share session".to_string(),
    };
    // Header: title (expanded) + a close button. Closing only hides the sheet —
    // sharing keeps running; ending is a separate, confirmed action.
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Box::new(Expanded::new(
            1.0,
            Text::new(text, ui, crate::theme::text_size(18.0))
                .with_color(theme.main_text())
                .finish(),
        )))
        .with_child(icon_button(
            AppIcon::XClose,
            "share_close",
            WorkspaceAction::CloseShareSheet,
            theme,
            id,
            states,
        ))
        .finish()
}

/// The sheet footer: a red "Stop sharing" button that opens the end-session
/// confirmation. Ending is deliberately separate from closing the sheet.
fn stop_footer(theme: &Theme, ui: FamilyId, id: EntityId, states: &States) -> Box<dyn Element> {
    let red = theme.ui_red();
    let hover = states
        .borrow_mut()
        .entry("share_stop".to_string())
        .or_default()
        .clone();
    let btn = Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() {
                tint(red, 30)
            } else {
                TRANSPARENT
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(7.0)
                    .with_child(render_icon(AppIcon::XClose, red, 14.0))
                    .with_child(
                        Text::new("Stop sharing".to_string(), ui, crate::theme::text_size(13.0))
                            .with_color(red)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(tint(red, 110)))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(id, WorkspaceAction::RequestStopSharing)
        }),
    );
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_child(btn)
            .finish(),
    )
    .with_padding_top(4.0)
    .finish()
}

/// A red banner shown at the top of the sheet when the session can't connect.
fn error_banner(err: &str, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let red = theme.ui_red();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(9.0)
            .with_child(render_icon(AppIcon::Info, red, 15.0))
            .with_child(Box::new(Expanded::new(
                1.0,
                Text::new(format!("Couldn't start sharing — {err}"), ui, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .finish(),
            )))
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(9.0)
    .with_padding_bottom(9.0)
    .with_background_color(tint(red, 30))
    .with_border(Border::all(1.0).with_border_color(tint(red, 110)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
    .finish()
}

// ── Email-invite row ──────────────────────────────────────────────────────────

fn email_row(
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    email_editor: &ViewHandle<InputEditor>,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let field = crate::shared_input::shared_input(
        theme,
        email_editor,
        crate::shared_input::SharedInputOpts {
            trailing: Some(dropdown_trigger(
                role_label(snap.invite_control),
                "share_invite_role",
                ShareDropdown::InviteRole,
                theme,
                ui,
                id,
                states,
            )),
            ..Default::default()
        },
    );

    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(10.0)
        .with_child(Box::new(Expanded::new(1.0, field)))
        .with_child(if snap.busy == Some(ShareOp::Invite) {
            solid_button_busy("Sending…", theme, ui)
        } else {
            solid_button(
                "Invite",
                "invite",
                WorkspaceAction::InviteEmail,
                theme,
                ui,
                id,
                states,
            )
        })
        .finish()
}

// ── Who has access ────────────────────────────────────────────────────────────

fn access_section(
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(14.0)
        .with_child(
            Text::new("Who has access".to_string(), ui, crate::theme::text_size(12.5))
                .with_color(theme.disabled_text())
                .finish(),
        )
        .with_child(anyone_row(snap, theme, ui, id, states));

    for p in &snap.participants {
        col = col.with_child(participant_row(p, theme, ui, id, states));
    }
    col.finish()
}

fn anyone_row(
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(12.0)
        .with_child(icon_slot(AppIcon::Eye, theme.sub_text()))
        .with_child(Box::new(Expanded::new(
            1.0,
            Text::new("Anyone with the link".to_string(), ui, crate::theme::text_size(14.0))
                .with_color(theme.main_text())
                .finish(),
        )))
        // Changing link access revokes the old link and issues a new one — two
        // round trips, the slowest control in this sheet. Show that it is working
        // and refuse further clicks until it lands, so a second change cannot
        // race the first.
        .with_child(if snap.busy == Some(ShareOp::LinkAccess) {
            solid_button_busy("Updating…", theme, ui)
        } else {
            dropdown_trigger(
                link_access_label(snap.link_access),
                "share_link_access",
                ShareDropdown::LinkAccess,
                theme,
                ui,
                id,
                states,
            )
        })
        .finish()
}

/// A round avatar: the profile photo when a URL is set (with an initials
/// fallback while it loads or if it fails), otherwise an initials circle.
pub(crate) fn avatar(
    name: &str,
    email: &str,
    avatar_url: &str,
    theme: &Theme,
    ui: FamilyId,
) -> Box<dyn Element> {
    avatar_sized(name, email, avatar_url, theme, ui, 30.0)
}

/// Like [`avatar`], but at an explicit diameter.
///
/// The size must be passed in rather than the caller constraining the result:
/// the corner radius is half the size, so rendering at 30px inside a 24px box
/// leaves a 15px radius on a 24px circle — which reads as a rounded square, not
/// a circle.
pub(crate) fn avatar_sized(
    name: &str,
    email: &str,
    avatar_url: &str,
    theme: &Theme,
    ui: FamilyId,
    size: f32,
) -> Box<dyn Element> {
    if avatar_url.is_empty() {
        return avatar_sized_inner(name, email, None, theme, ui, size);
    }
    avatar_sized_inner(
        name,
        email,
        Some(crate::settings::pages::account::avatar_photo_source(
            avatar_url,
        )),
        theme,
        ui,
        size,
    )
}

pub(crate) fn avatar_sized_from_source(
    name: &str,
    email: &str,
    source: AssetSource,
    theme: &Theme,
    ui: FamilyId,
    size: f32,
) -> Box<dyn Element> {
    avatar_sized_inner(name, email, Some(source), theme, ui, size)
}

fn avatar_sized_inner(
    name: &str,
    email: &str,
    source: Option<AssetSource>,
    theme: &Theme,
    ui: FamilyId,
    size: f32,
) -> Box<dyn Element> {
    let initial: String = name
        .chars()
        .next()
        .or_else(|| email.chars().next())
        .unwrap_or('?')
        .to_uppercase()
        .collect();
    let inner: Box<dyn Element> = if let Some(source) = source {
        warpui::elements::Image::new(
            source,
            // Paint into the exact square avatar bounds before applying the
            // circular radius. `cover()` draws an oversized crop; the radius
            // would then belong to that larger rectangle and can leave a
            // lens/eye-shaped silhouette in the navbar presence stack.
            warpui_core::image_cache::CacheOption::Original,
        )
        .stretch()
        .square()
        .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.0)))
        .before_load(initials_circle(&initial, theme, ui, size))
        .on_load_failure(initials_circle(&initial, theme, ui, size))
        .finish()
    } else {
        initials_circle(&initial, theme, ui, size)
    };
    ConstrainedBox::new(inner)
        .with_width(size)
        .with_height(size)
        .finish()
}

fn initials_circle(initial: &str, theme: &Theme, ui: FamilyId, size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Align::new(
                Text::new(initial.to_string(), ui, crate::theme::text_size(12.0))
                    .with_color(theme.accent_text())
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(theme.accent)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(size / 2.0)))
        .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

fn participant_row(
    p: &ShareParticipant,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let avatar = avatar(&p.name, &p.email, &p.avatar_url, theme, ui);

    let name = if p.name.is_empty() {
        p.email.clone()
    } else {
        p.name.clone()
    };
    let mut identity = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(1.0)
        .with_child(
            Text::new(name, ui, crate::theme::text_size(14.0))
                .with_color(theme.main_text())
                .finish(),
        );
    if !p.email.is_empty() {
        identity = identity.with_child(
            Text::new(p.email.clone(), ui, crate::theme::text_size(11.5))
                .with_color(theme.sub_text())
                .finish(),
        );
    }

    let access: Box<dyn Element> = if p.is_host {
        Container::new(
            Text::new("Full access".to_string(), ui, crate::theme::text_size(12.5))
                .with_color(theme.disabled_text())
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish()
    } else {
        dropdown_trigger(
            role_label(p.control),
            &format!("share_part_{}", p.id),
            ShareDropdown::Participant(p.id.clone()),
            theme,
            ui,
            id,
            states,
        )
    };

    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(12.0)
        .with_child(avatar)
        .with_child(Box::new(Expanded::new(1.0, identity.finish())))
        .with_child(access)
        .finish()
}

// ── URL / QR / Copy footer ────────────────────────────────────────────────────

fn url_row(
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let url = snap
        .link_url
        .clone()
        .unwrap_or_else(|| "Hidden until shared publicly".to_string());
    let url_box = Container::new(
        Text::new(truncate(&url, 44), mono, crate::theme::text_size(12.0))
            .with_color(theme.sub_text())
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(10.0)
    .with_padding_bottom(10.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish();

    let copy_action = snap
        .link_url
        .clone()
        .map(|u| WorkspaceAction::CopyShareValue { value: u })
        .unwrap_or(WorkspaceAction::CopyShareValue {
            value: String::new(),
        });

    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(10.0)
        .with_child(Box::new(Expanded::new(1.0, url_box)))
        .with_child(icon_button(
            AppIcon::Grid,
            "qr",
            WorkspaceAction::OpenShareQr,
            theme,
            id,
            states,
        ))
        .with_child(labeled_button(
            AppIcon::Copy,
            "Copy link",
            "copylink",
            copy_action,
            theme,
            ui,
            id,
            states,
        ))
        .finish()
}

// ── Dropdowns ─────────────────────────────────────────────────────────────────

fn dropdown(
    dd: &ShareDropdown,
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> (Box<dyn Element>, OffsetPositioning) {
    let (rows, tag): (Vec<Box<dyn Element>>, String) = match dd {
        ShareDropdown::InviteRole => (
            vec![
                menu_row(
                    AppIcon::Eye,
                    "Can view",
                    !snap.invite_control,
                    WorkspaceAction::SetInviteRole { control: false },
                    theme,
                    ui,
                    id,
                    states,
                ),
                menu_row(
                    AppIcon::Edit,
                    "Can edit",
                    snap.invite_control,
                    WorkspaceAction::SetInviteRole { control: true },
                    theme,
                    ui,
                    id,
                    states,
                ),
            ],
            "share_invite_role".to_string(),
        ),
        ShareDropdown::LinkAccess => (
            vec![
                menu_row(
                    AppIcon::User,
                    "Only people invited",
                    snap.link_access == LinkAccess::None,
                    WorkspaceAction::SetLinkAccess {
                        access: LinkAccess::None,
                    },
                    theme,
                    ui,
                    id,
                    states,
                ),
                menu_divider(theme),
                menu_header("Anyone with the link", theme, ui),
                menu_row(
                    AppIcon::Eye,
                    "Can view",
                    snap.link_access == LinkAccess::View,
                    WorkspaceAction::SetLinkAccess {
                        access: LinkAccess::View,
                    },
                    theme,
                    ui,
                    id,
                    states,
                ),
                menu_row(
                    AppIcon::Edit,
                    "Can edit",
                    snap.link_access == LinkAccess::Edit,
                    WorkspaceAction::SetLinkAccess {
                        access: LinkAccess::Edit,
                    },
                    theme,
                    ui,
                    id,
                    states,
                ),
            ],
            "share_link_access".to_string(),
        ),
        ShareDropdown::Participant(pid) => {
            let control = snap
                .participants
                .iter()
                .find(|p| &p.id == pid)
                .map(|p| p.control)
                .unwrap_or(false);
            (
                vec![
                    menu_row(
                        AppIcon::Eye,
                        "Can view",
                        !control,
                        WorkspaceAction::SetParticipantAccess {
                            participant: pid.clone(),
                            control: false,
                        },
                        theme,
                        ui,
                        id,
                        states,
                    ),
                    menu_row(
                        AppIcon::Edit,
                        "Can edit",
                        control,
                        WorkspaceAction::SetParticipantAccess {
                            participant: pid.clone(),
                            control: true,
                        },
                        theme,
                        ui,
                        id,
                        states,
                    ),
                    menu_divider(theme),
                    menu_row(
                        AppIcon::XClose,
                        "Remove from session",
                        false,
                        WorkspaceAction::RemoveParticipant {
                            participant: pid.clone(),
                        },
                        theme,
                        ui,
                        id,
                        states,
                    ),
                ],
                format!("share_part_{pid}"),
            )
        }
    };

    let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for row in rows {
        col = col.with_child(row);
    }
    let menu = Container::new(col.finish())
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_padding_left(4.0)
        .with_padding_right(4.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish();
    let menu = ConstrainedBox::new(menu).with_max_width(MENU_W).finish();

    let offset = OffsetPositioning::offset_from_save_position_element(
        &tag,
        vec2f(0.0, 4.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    (menu, offset)
}

fn menu_row(
    icon: AppIcon,
    label: &str,
    selected: bool,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let key = format!("mrow_{label}");
    let hover = states.borrow_mut().entry(key).or_default().clone();
    let label = label.to_string();
    let sel_bg = theme.accent;
    let hover_bg = theme.surface_3();
    let sel_fg = ColorU {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    let idle_fg = theme.main_text();
    Box::new(
        Hoverable::new(hover, move |st| {
            let (bg, fg) = if selected {
                (sel_bg, sel_fg)
            } else if st.is_hovered() {
                (hover_bg, idle_fg)
            } else {
                (TRANSPARENT, idle_fg)
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(9.0)
                    .with_child(render_icon(icon, fg, 14.0))
                    .with_child(
                        Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                            .with_color(fg)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(9.0)
            .with_padding_right(9.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
    )
}

fn menu_header(label: &str, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(label.to_string(), ui, crate::theme::text_size(11.0))
            .with_color(theme.disabled_text())
            .finish(),
    )
    .with_padding_left(9.0)
    .with_padding_right(9.0)
    .with_padding_top(4.0)
    .with_padding_bottom(2.0)
    .finish()
}

fn menu_divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(Empty::new().finish())
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish()
}

// ── Shared button primitives ──────────────────────────────────────────────────

/// A right-aligned "label ⌄" that opens a dropdown; tagged via `SavePosition` so
/// the menu can anchor to it.
fn dropdown_trigger(
    label: &str,
    tag: &str,
    kind: ShareDropdown,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("trg_{tag}"))
        .or_default()
        .clone();
    let label = label.to_string();
    let color = theme.sub_text();
    let hover_bg = theme.surface_3();
    let content = Hoverable::new(hover, move |st| {
        let bg = if st.is_hovered() {
            hover_bg
        } else {
            TRANSPARENT
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.0)
                .with_child(
                    Text::new(label.clone(), ui, crate::theme::text_size(12.5))
                        .with_color(color)
                        .finish(),
                )
                .with_child(render_icon(AppIcon::ChevronDown, color, 12.0))
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(6.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    })
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(
            id,
            WorkspaceAction::OpenShareDropdown { kind: kind.clone() },
        )
    });
    SavePosition::new(Box::new(content), tag).finish()
}

/// A filled button (e.g. Invite).
/// A filled button that is WAITING on the network: it says so, dims, and drops
/// clicks. Without this the control looked identical mid-flight, so the only
/// feedback for a round trip was the result arriving some time later.
fn solid_button_busy(label: &str, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(label.to_string(), ui, crate::theme::text_size(12.5))
            .with_color(theme.sub_text())
            .finish(),
    )
    .with_padding_left(14.0)
    .with_padding_right(14.0)
    .with_padding_top(7.0)
    .with_padding_bottom(7.0)
    .with_background_color(theme.surface_3())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish()
}

fn solid_button(
    label: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("solid_{key}"))
        .or_default()
        .clone();
    let label = label.to_string();
    let base = theme.surface_3();
    let hovered = tint(theme.main_text(), 30);
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() { hovered } else { base };
            Container::new(
                Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_padding_left(18.0)
            .with_padding_right(18.0)
            .with_padding_top(9.0)
            .with_padding_bottom(9.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
    )
}

/// An icon + label button (e.g. Copy link).
fn labeled_button(
    icon: AppIcon,
    label: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("lbl_{key}"))
        .or_default()
        .clone();
    let label = label.to_string();
    let fg = theme.main_text();
    let base = theme.surface_2();
    let hovered = theme.surface_3();
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() { hovered } else { base };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(7.0)
                    .with_child(render_icon(icon, fg, 14.0))
                    .with_child(
                        Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                            .with_color(fg)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(9.0)
            .with_padding_bottom(9.0)
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
    )
}

/// A square icon-only button (e.g. QR).
fn icon_button(
    icon: AppIcon,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("icb_{key}"))
        .or_default()
        .clone();
    let fg = theme.main_text();
    let base = theme.surface_2();
    let hovered = theme.surface_3();
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() { hovered } else { base };
            Container::new(render_icon(icon, fg, 16.0))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(10.0)
                .with_padding_bottom(10.0)
                .with_background_color(bg)
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
    )
}

/// A non-interactive leading icon (fixed slot) for a row.
fn icon_slot(icon: AppIcon, color: ColorU) -> Box<dyn Element> {
    Container::new(render_icon(icon, color, 18.0))
        .with_padding_left(2.0)
        .with_padding_right(2.0)
        .finish()
}

// ── QR sub-sheet ──────────────────────────────────────────────────────────────

const QR_PANEL_W: f32 = 460.0;

/// The QR sub-modal shown over the share sheet: a big QR card plus copy/download.
/// (The QR bitmap itself is wired in a follow-up; this renders the frame + URL.)
pub fn render_qr_sheet(
    snap: &ShareSnapshot,
    theme: &Theme,
    ui: FamilyId,
    qr_asset: Option<&str>,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let url = snap.link_url.clone().unwrap_or_default();

    let header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(10.0)
        .with_child(icon_button(
            AppIcon::ArrowLeft,
            "qr_back",
            WorkspaceAction::CloseShareQr,
            theme,
            id,
            states,
        ))
        .with_child(Box::new(Expanded::new(
            1.0,
            Align::new(
                Text::new("Share session QR code".to_string(), ui, crate::theme::text_size(15.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .finish(),
        )))
        .with_child(
            Text::new("ESC".to_string(), ui, crate::theme::text_size(11.0))
                .with_color(theme.disabled_text())
                .finish(),
        )
        .with_child(icon_button(
            AppIcon::XClose,
            "qr_x",
            WorkspaceAction::CloseShareQr,
            theme,
            id,
            states,
        ))
        .finish();

    // White QR card holding the generated bitmap (or a fallback message).
    let card_inner: Box<dyn Element> = match qr_asset {
        Some(asset_id) => ConstrainedBox::new(
            warpui::elements::Image::new(
                warpui_core::assets::asset_cache::AssetSource::Raw {
                    id: asset_id.to_string(),
                },
                warpui_core::image_cache::CacheOption::BySize,
            )
            .contain()
            .finish(),
        )
        .with_width(240.0)
        .with_height(240.0)
        .finish(),
        None => ConstrainedBox::new(
            Align::new(
                Text::new(
                    if url.is_empty() {
                        "Turn on link access to get a QR".to_string()
                    } else {
                        truncate(&url, 30)
                    },
                    ui,
                    crate::theme::text_size(11.0),
                )
                .with_color(ColorU {
                    r: 40,
                    g: 40,
                    b: 40,
                    a: 255,
                })
                .finish(),
            )
            .finish(),
        )
        .with_width(240.0)
        .with_height(240.0)
        .finish(),
    };
    let card = Container::new(card_inner)
        .with_background_color(ColorU {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(18.0)))
        .with_padding_left(20.0)
        .with_padding_right(20.0)
        .with_padding_top(20.0)
        .with_padding_bottom(20.0)
        .finish();

    let footer = Align::new(
        Flex::row()
            .with_spacing(10.0)
            .with_child(icon_button(
                AppIcon::Copy,
                "qr_copy",
                WorkspaceAction::CopyShareValue { value: url.clone() },
                theme,
                id,
                states,
            ))
            .finish(),
    )
    .finish();

    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(22.0)
            .with_child(header)
            .with_child(Align::new(card).finish())
            .with_child(footer)
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_padding_left(22.0)
    .with_padding_right(22.0)
    .with_padding_top(18.0)
    .with_padding_bottom(20.0)
    .finish();

    let panel = EventHandler::new(panel)
        .on_left_mouse_down(|_e, _a, _p| DispatchEventResult::StopPropagation)
        .finish();
    let panel = ConstrainedBox::new(panel)
        .with_max_width(QR_PANEL_W)
        .finish();

    let scrim = EventHandler::new(
        Container::new(Align::new(panel).finish())
            .with_background_color(ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 170,
            })
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(id, WorkspaceAction::CloseShareQr);
        DispatchEventResult::StopPropagation
    })
    .finish();
    scrim
}
