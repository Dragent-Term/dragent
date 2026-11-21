//! The organisation area at the bottom of the sidebar. It holds the same job
//! the bottom corner of a team workspace holds, built from this app's own
//! widgets:
//! the active organisation opens a menu (Settings, Switch organisation, New
//! window, Give feedback, Contact us), with Help and Log out beside it.
//!
//! The menus follow the workspace popover pattern (see `header_menus`): a
//! positioned card anchored to a `SavePosition`, over a transparent catcher
//! that dismisses on any outside click. Submenus open on hover, anchored to
//! their row.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded,
    Flex, MainAxisSize, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{AppContext, EntityId, SingletonEntity, UpdateModel, UpdateView, ViewContext};
use warpui_core::elements::{DispatchEventResult, EventHandler, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::auth::AuthManager;
use crate::icons::{render_icon, AppIcon};
use crate::organisation_context::OrganisationContext;
use crate::settings::{AccountStore, OrgStore, SettingsSection};
use crate::theme::{scaled, Theme};

use super::action::WorkspaceAction;
use super::view::WorkspaceView;

type States = RefCell<HashMap<String, MouseStateHandle>>;

/// SavePosition of the organisation trigger (the menu opens above it).
pub const ORG_MENU_TAG: &str = "sidebar_org_menu";
const HELP_MENU_TAG: &str = "sidebar_help_menu";
const SWITCH_ROW_TAG: &str = "org_menu_switch_row";
const CONTACT_ROW_TAG: &str = "org_menu_contact_row";

pub const GITHUB_URL: &str = "https://github.com/Dragent-Term/Dragent";
pub const REPORT_ISSUE_URL: &str = "https://github.com/Dragent-Term/Dragent/issues/new";
pub const SUPPORT_EMAIL: &str = "support@dragent.dev";

const MENU_WIDTH: f32 = 236.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrgSubmenu {
    Switch,
    Contact,
}

/// What the footer and menus show about the account and its organisations.
pub(crate) struct OrgSummary {
    signed_in: bool,
    name: String,
    /// Plan badge for a paid organisation ("PRO").
    plan: Option<String>,
    email: String,
    organisations: Vec<(String, String)>,
    active_id: Option<String>,
    switching: bool,
}

impl OrgSummary {
    pub(crate) fn read(app: &AppContext) -> Self {
        let signed_in = AuthManager::as_ref(app).is_logged_in();
        let store = OrgStore::as_ref(app);
        let context = OrganisationContext::as_ref(app);
        let name = store
            .active
            .as_ref()
            .map(|org| org.name.clone())
            .or_else(|| context.active().map(|org| org.name.clone()))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                if signed_in {
                    "Organisation".into()
                } else {
                    "Not signed in".into()
                }
            });
        let plan = store
            .billing
            .as_ref()
            .filter(|billing| billing.is_paid && !billing.tier.is_empty())
            .map(|billing| billing.tier.to_uppercase());
        let email = AccountStore::as_ref(app)
            .data
            .as_ref()
            .map(|data| data.user.email.clone())
            .unwrap_or_default();
        Self {
            signed_in,
            name,
            plan: plan.filter(|_| signed_in),
            email,
            organisations: store
                .organizations
                .iter()
                .flatten()
                .map(|org| (org.id.clone(), org.name.clone()))
                .collect(),
            active_id: store.active.as_ref().map(|org| org.id.clone()),
            switching: store.is_busy("switch") || context.is_switching(),
        }
    }
}

fn mouse(states: &States, key: impl Into<String>) -> MouseStateHandle {
    states.borrow_mut().entry(key.into()).or_default().clone()
}

fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("\u{2318}{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// Rounded square with the organisation's initial.
fn org_badge(theme: &Theme, ui: FamilyId, name: &str, size: f32) -> Box<dyn Element> {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "D".into());
    ConstrainedBox::new(
        Container::new(
            Align::new(
                Text::new(initial, ui, crate::theme::text_size(size * 0.58))
                    .with_color(theme.accent_text())
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(theme.accent)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(size * 0.28)))
        .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

// ── Footer ────────────────────────────────────────────────────────────────────

pub(crate) fn render_org_footer(
    summary: &OrgSummary,
    menu_open: bool,
    theme: &Theme,
    ui: FamilyId,
    workspace_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let name = if summary.switching {
        "Switching…".to_string()
    } else {
        summary.name.clone()
    };
    let plan = summary.plan.clone();
    let trigger = Hoverable::new(mouse(states, "org-footer-trigger"), move |state| {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(org_badge(&theme_owned, ui, &name, 18.0))
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(name.clone(), ui, crate::theme::text_size(13.0))
                        .with_color(if state.is_hovered() || menu_open {
                            theme_owned.main_text()
                        } else {
                            theme_owned.sub_text()
                        })
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
            );
        if let Some(plan) = &plan {
            row = row.with_child(
                Container::new(
                    Text::new(plan.clone(), ui, crate::theme::text_size(9.0))
                        .with_color(theme_owned.background)
                        .finish(),
                )
                .with_background_color(if state.is_hovered() || menu_open {
                    theme_owned.accent
                } else {
                    theme_owned.sub_text()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(1.0)
                .with_padding_bottom(1.0)
                .finish(),
            );
        }
        Container::new(row.finish())
            .with_background_color(if menu_open {
                theme_owned.fg_overlay_1()
            } else if state.is_hovered() {
                theme_owned.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(6.0)
            .with_padding_right(8.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(workspace_id, WorkspaceAction::ToggleOrgMenu);
    })
    .finish();

    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(2.0)
        .with_child(Expanded::new(1.0, SavePosition::new(trigger, ORG_MENU_TAG).finish()).finish())
        .with_child(
            SavePosition::new(
                icon_button(
                    theme,
                    mouse(states, "org-footer-help"),
                    AppIcon::Asset("icons/circle-help.svg"),
                    WorkspaceAction::ToggleHelpMenu,
                    workspace_id,
                ),
                HELP_MENU_TAG,
            )
            .finish(),
        );
    row = row.with_child(if summary.signed_in {
        icon_button(
            theme,
            mouse(states, "org-footer-logout"),
            AppIcon::Asset("icons/log-out.svg"),
            WorkspaceAction::SignOutFromHeader,
            workspace_id,
        )
    } else {
        text_button(
            theme,
            ui,
            mouse(states, "org-footer-signin"),
            "Sign in",
            WorkspaceAction::SignInFromHeader,
            workspace_id,
        )
    });

    // Star on GitHub sits above the footer's border, outside it.
    let star = crate::community::render_star_button(
        theme,
        ui,
        crate::community::state(),
        true,
        mouse(states, "org-footer-star"),
        move |ectx| ectx.dispatch_typed_action_for(workspace_id, WorkspaceAction::StarOnGitHub),
    );

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(star)
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_bottom(8.0)
                .finish(),
        )
        .with_child(
            Container::new(row.finish())
                .with_border(Border::top(1.0).with_border_color(theme.border()))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(7.0)
                .with_padding_bottom(7.0)
                .finish(),
        )
        .finish()
}

/// The footer with demo data, for the previews and README screenshots.
pub(crate) fn render_demo_org_footer(
    theme: &Theme,
    ui: FamilyId,
    workspace_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let summary = OrgSummary {
        signed_in: true,
        name: "Acme".into(),
        plan: Some("PRO".into()),
        email: "you@acme.dev".into(),
        organisations: Vec::new(),
        active_id: None,
        switching: false,
    };
    render_org_footer(&summary, false, theme, ui, workspace_id, states)
}

fn icon_button(
    theme: &Theme,
    mouse: MouseStateHandle,
    icon: AppIcon,
    action: WorkspaceAction,
    workspace_id: EntityId,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    Hoverable::new(mouse, move |state| {
        ConstrainedBox::new(
            Container::new(
                Align::new(render_icon(
                    icon,
                    if state.is_hovered() {
                        theme_owned.main_text()
                    } else {
                        theme_owned.sub_text()
                    },
                    14.0,
                ))
                .finish(),
            )
            .with_background_color(if state.is_hovered() {
                theme_owned.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish(),
        )
        .with_width(28.0)
        .with_height(28.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(workspace_id, action.clone()))
    .finish()
}

fn text_button(
    theme: &Theme,
    ui: FamilyId,
    mouse: MouseStateHandle,
    label: &'static str,
    action: WorkspaceAction,
    workspace_id: EntityId,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    Hoverable::new(mouse, move |state| {
        Container::new(
            Text::new(label.to_string(), ui, crate::theme::text_size(12.0))
                .with_color(if state.is_hovered() {
                    theme_owned.accent
                } else {
                    theme_owned.main_text()
                })
                .finish(),
        )
        .with_border(Border::all(1.0).with_border_color(if state.is_hovered() {
            theme_owned.accent
        } else {
            theme_owned.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(workspace_id, action.clone()))
    .finish()
}

// ── Menus ─────────────────────────────────────────────────────────────────────

enum Trailing {
    None,
    Shortcut(String),
    Chevron,
    Check,
}

struct Row {
    key: String,
    icon: Option<AppIcon>,
    leading: Option<Box<dyn Element>>,
    label: String,
    trailing: Trailing,
    action: Option<WorkspaceAction>,
    /// Hovering opens this submenu (`Some(None)` closes any open one).
    hover: Option<Option<OrgSubmenu>>,
    highlighted: bool,
}

impl Row {
    fn new(key: &str, icon: AppIcon, label: &str, action: WorkspaceAction) -> Self {
        Self {
            key: key.into(),
            icon: Some(icon),
            leading: None,
            label: label.into(),
            trailing: Trailing::None,
            action: Some(action),
            hover: Some(None),
            highlighted: false,
        }
    }
}

fn menu_row(
    row: Row,
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let Row {
        key,
        icon,
        leading,
        label,
        trailing,
        action,
        hover,
        highlighted,
    } = row;
    let mut leading = leading;
    let hoverable = Hoverable::new(mouse(states, format!("org-menu-{key}")), move |state| {
        let fg = theme_owned.main_text();
        let mut content = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0);
        if let Some(leading) = leading.take() {
            content = content.with_child(leading);
        } else if let Some(icon) = icon {
            content = content.with_child(render_icon(icon, theme_owned.sub_text(), 14.0));
        }
        content = content.with_child(
            Expanded::new(
                1.0,
                Text::new(label.clone(), ui, crate::theme::text_size(12.5))
                    .with_color(fg)
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .finish(),
        );
        match &trailing {
            Trailing::None => {}
            Trailing::Shortcut(text) => {
                content = content.with_child(
                    Text::new(text.clone(), ui, crate::theme::text_size(11.0))
                        .with_color(theme_owned.disabled_text())
                        .finish(),
                );
            }
            Trailing::Chevron => {
                content = content.with_child(render_icon(
                    AppIcon::ChevronRight,
                    theme_owned.disabled_text(),
                    12.0,
                ));
            }
            Trailing::Check => {
                content = content.with_child(render_icon(AppIcon::Check, theme_owned.accent, 13.0));
            }
        }
        Container::new(content.finish())
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(if state.is_hovered() || highlighted {
                theme_owned.surface_3()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
    });
    let mut hoverable = hoverable.with_cursor(Cursor::PointingHand);
    if let Some(submenu) = hover {
        hoverable = hoverable.on_hover(move |hovered, ectx, _, _| {
            if hovered {
                ectx.dispatch_typed_action_for(view_id, WorkspaceAction::HoverOrgSubmenu(submenu));
            }
        });
    }
    if let Some(action) = action {
        hoverable = hoverable
            .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone()));
    }
    hoverable.finish()
}

fn separator(theme: &Theme) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(theme.outline())
                .finish(),
        )
        .with_height(1.0)
        .finish(),
    )
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .finish()
}

fn menu_label(theme: &Theme, ui: FamilyId, text: &str) -> Box<dyn Element> {
    Container::new(
        Text::new(text.to_string(), ui, crate::theme::text_size(11.0))
            .with_color(theme.disabled_text())
            .soft_wrap(false)
            .with_clip(ClipConfig::ellipsis())
            .finish(),
    )
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(5.0)
    .with_padding_bottom(4.0)
    .finish()
}

fn menu_card(theme: &Theme, rows: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    for row in rows {
        column = column.with_child(row);
    }
    let card = Container::new(column.finish())
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_uniform_padding(4.0)
        .finish();
    // Clicks inside the card must not reach the dismiss catcher.
    EventHandler::new(ConstrainedBox::new(card).with_width(MENU_WIDTH).finish())
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish()
}

fn catcher(view_id: EntityId) -> Box<dyn Element> {
    EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(ColorU::transparent_black())
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::CloseOrgMenu);
        DispatchEventResult::StopPropagation
    })
    .finish()
}

fn above(tag: &'static str) -> OffsetPositioning {
    OffsetPositioning::offset_from_save_position_element(
        tag,
        vec2f(0.0, -6.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::TopLeft,
        ChildAnchor::BottomLeft,
    )
}

fn beside(tag: &'static str) -> OffsetPositioning {
    OffsetPositioning::offset_from_save_position_element(
        tag,
        vec2f(6.0, -4.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::TopRight,
        ChildAnchor::TopLeft,
    )
}

/// The organisation menu plus its open submenu, and the dismiss catcher.
pub(crate) fn org_menu_overlays(
    summary: &OrgSummary,
    submenu: Option<OrgSubmenu>,
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &States,
) -> (Box<dyn Element>, Vec<(Box<dyn Element>, OffsetPositioning)>) {
    let row = |row: Row| menu_row(row, theme, ui, view_id, states);
    let mut rows = Vec::new();

    let mut settings = Row::new(
        "settings",
        AppIcon::Settings,
        "Settings",
        WorkspaceAction::OpenSettings,
    );
    settings.trailing = Trailing::Shortcut(shortcut(","));
    rows.push(row(settings));

    if summary.signed_in && !summary.organisations.is_empty() {
        let switch = Row {
            trailing: Trailing::Chevron,
            action: Some(WorkspaceAction::HoverOrgSubmenu(Some(OrgSubmenu::Switch))),
            hover: Some(Some(OrgSubmenu::Switch)),
            highlighted: submenu == Some(OrgSubmenu::Switch),
            ..Row::new(
                "switch",
                AppIcon::Asset("icons/arrow-left-right.svg"),
                "Switch organisation",
                WorkspaceAction::CloseOrgMenu,
            )
        };
        rows.push(SavePosition::new(row(switch), SWITCH_ROW_TAG).finish());
    }

    let mut new_window = Row::new(
        "new-window",
        AppIcon::Asset("icons/app-window.svg"),
        "New window",
        WorkspaceAction::NewWindow,
    );
    new_window.trailing = Trailing::Shortcut(shortcut("N"));
    rows.push(row(new_window));

    rows.push(row(Row::new(
        "feedback",
        AppIcon::Asset("icons/message-square-share.svg"),
        "Give feedback",
        WorkspaceAction::OpenFeedback,
    )));

    let contact = Row {
        trailing: Trailing::Chevron,
        action: Some(WorkspaceAction::HoverOrgSubmenu(Some(OrgSubmenu::Contact))),
        hover: Some(Some(OrgSubmenu::Contact)),
        highlighted: submenu == Some(OrgSubmenu::Contact),
        ..Row::new(
            "contact",
            AppIcon::Asset("icons/messages-square.svg"),
            "Contact us",
            WorkspaceAction::CloseOrgMenu,
        )
    };
    rows.push(SavePosition::new(row(contact), CONTACT_ROW_TAG).finish());

    let mut overlays = vec![(menu_card(theme, rows), above(ORG_MENU_TAG))];
    match submenu {
        Some(OrgSubmenu::Switch) if summary.signed_in => {
            overlays.push((
                switch_submenu(summary, theme, ui, view_id, states),
                beside(SWITCH_ROW_TAG),
            ));
        }
        Some(OrgSubmenu::Contact) => {
            overlays.push((
                contact_submenu(theme, ui, view_id, states),
                beside(CONTACT_ROW_TAG),
            ));
        }
        _ => {}
    }
    (catcher(view_id), overlays)
}

fn switch_submenu(
    summary: &OrgSummary,
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let mut rows = Vec::new();
    if !summary.email.is_empty() {
        rows.push(menu_label(theme, ui, &summary.email));
    }
    for (id, name) in &summary.organisations {
        let active = summary.active_id.as_deref() == Some(id.as_str());
        rows.push(menu_row(
            Row {
                key: format!("org-{id}"),
                icon: None,
                leading: Some(org_badge(theme, ui, name, 16.0)),
                label: name.clone(),
                trailing: if active {
                    Trailing::Check
                } else {
                    Trailing::None
                },
                action: Some(if active {
                    WorkspaceAction::CloseOrgMenu
                } else {
                    WorkspaceAction::SwitchOrganisation { id: id.clone() }
                }),
                hover: None,
                highlighted: false,
            },
            theme,
            ui,
            view_id,
            states,
        ));
    }
    rows.push(separator(theme));
    let mut create = Row::new(
        "create-org",
        AppIcon::Plus,
        "Create organisation",
        WorkspaceAction::CreateOrganisation,
    );
    create.hover = None;
    rows.push(menu_row(create, theme, ui, view_id, states));
    menu_card(theme, rows)
}

fn contact_submenu(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let link = |key: &str, icon: AppIcon, label: &str, url: String| {
        let mut row = Row::new(key, icon, label, WorkspaceAction::OpenExternalUrl { url });
        row.hover = None;
        menu_row(row, theme, ui, view_id, states)
    };
    menu_card(
        theme,
        vec![
            link(
                "contact-github",
                AppIcon::GitHub,
                "GitHub",
                GITHUB_URL.into(),
            ),
            link(
                "contact-email",
                AppIcon::Asset("icons/mail.svg"),
                "Email support",
                format!("mailto:{SUPPORT_EMAIL}"),
            ),
        ],
    )
}

/// Help: documentation, keyboard shortcuts, and reporting a problem.
pub(crate) fn help_menu_overlay(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &States,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let row = |row: Row| menu_row(row, theme, ui, view_id, states);
    let menu = menu_card(
        theme,
        vec![
            row(Row::new(
                "help-docs",
                AppIcon::Asset("icons/book-open.svg"),
                "Documentation",
                WorkspaceAction::OpenDocumentation,
            )),
            row(Row::new(
                "help-shortcuts",
                AppIcon::Keyboard,
                "Keyboard shortcuts",
                WorkspaceAction::OpenSettingsSection(SettingsSection::KeyboardShortcuts),
            )),
            row(Row::new(
                "help-issue",
                AppIcon::Asset("icons/bug.svg"),
                "Report an issue",
                WorkspaceAction::OpenExternalUrl {
                    url: REPORT_ISSUE_URL.into(),
                },
            )),
        ],
    );
    (catcher(view_id), menu, above(HELP_MENU_TAG))
}

// ── Actions ───────────────────────────────────────────────────────────────────

impl WorkspaceView {
    pub(crate) fn close_org_menus(&mut self) -> bool {
        self.account_menu_open = false;
        self.usage_menu_open = false;
        let was_open = self.org_menu_open || self.help_menu_open;
        self.org_menu_open = false;
        self.help_menu_open = false;
        self.org_submenu = None;
        was_open
    }

    /// The footer trigger stays highlighted while its menu is open.
    pub(crate) fn sync_org_menu_highlight(&mut self, ctx: &mut ViewContext<Self>) {
        let open = self.org_menu_open;
        let panel = self.left_panel_view.clone();
        ctx.update_view(&panel, |panel, panel_ctx| {
            if panel.org_menu_open != open {
                panel.org_menu_open = open;
                panel_ctx.notify();
            }
        });
    }

    pub(crate) fn handle_org_menu_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::ToggleOrgMenu => {
                let open = !self.org_menu_open;
                self.close_org_menus();
                self.org_menu_open = open;
                if open {
                    // Keep the organisation list current for Switch organisation.
                    let store = OrgStore::handle(ctx);
                    ctx.update_model(&store, |s, c| {
                        s.fetch_if_stale(std::time::Duration::from_secs(60), c)
                    });
                }
            }
            WorkspaceAction::ToggleHelpMenu => {
                let open = !self.help_menu_open;
                self.close_org_menus();
                self.help_menu_open = open;
            }
            WorkspaceAction::CloseOrgMenu => {
                self.close_org_menus();
            }
            WorkspaceAction::HoverOrgSubmenu(submenu) => {
                if self.org_submenu == *submenu {
                    return;
                }
                self.org_submenu = *submenu;
            }
            WorkspaceAction::SwitchOrganisation { id } => {
                self.close_org_menus();
                let store = OrgStore::handle(ctx);
                let id = id.clone();
                ctx.update_model(&store, |s, c| s.switch_active(&id, c));
            }
            WorkspaceAction::CreateOrganisation => {
                self.close_org_menus();
                self.open_settings(ctx);
                let settings = self.settings_view.clone();
                ctx.update_view(&settings, |sv, sv_ctx| sv.open_create_organisation(sv_ctx));
            }
            WorkspaceAction::OpenSettingsSection(section) => {
                // Dismiss any account picker modal first: it renders as a
                // centered overlay above everything, so Settings would open
                // behind it if the picker stayed. Clearing here also makes
                // the picker's Manage button a single dispatch (Close was
                // previously fired alongside Open and could be dropped).
                self.agent_account_picker = None;
                self.close_org_menus();
                self.open_settings(ctx);
                let settings = self.settings_view.clone();
                let section = *section;
                ctx.update_view(&settings, |sv, sv_ctx| sv.show_section(section, sv_ctx));
            }
            _ => return,
        }
        self.sync_org_menu_highlight(ctx);
        ctx.notify();
    }
}
