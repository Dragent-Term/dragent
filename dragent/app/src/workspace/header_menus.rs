//! Header dropdown menus.
//!
//! Responsible for: review/layout/account/attention menu overlays and rows.
//! Moved verbatim; same behavior.

//! Global workspace title bar — toolbar, search, settings, and account controls.

use dragent_workspace_model::{AttentionItemId, BuiltInLayout};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded,
    Flex, MainAxisAlignment, MainAxisSize, OffsetPositioning, ParentElement,
    PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition, Shrinkable, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::{Clipped, DispatchEventResult, EventHandler};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::theme::{
    Theme, HEADER_BAR_HEIGHT, HEADER_ICON_PADDING, HEADER_SEARCH_MAX_WIDTH, TAB_BAR_BORDER_HEIGHT,
};

use super::execution_workspace::AttentionOverlayEntry;
use super::{InputEditor, WorkspaceAction};

/// Left navbar segment: sidebar toggle only. Rendered above the left
/// panel so the sidebar's right demarcation line passes straight through
/// the navbar instead of stopping below it.
/// Non-interactive progress row for a headless Review action in flight.
/// The existing 12-frame spinner, same asset family as the rest of the app.
fn review_menu_progress(ui_font: FamilyId, theme: &Theme, label: &str) -> Box<dyn Element> {
    let frame = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
        / 100) as usize
        % crate::icons::SPINNER_FRAMES;
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(8.0)
            .with_child(crate::icons::render_spinner(
                frame,
                theme.main_text(),
                13.0,
            ))
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(label.to_string(), ui_font, crate::theme::text_size(12.5))
                        .with_color(theme.main_text())
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .finish()
}

/// Headless result line under the actions (success notice or error).
fn review_menu_note(
    ui_font: FamilyId,
    theme: &Theme,
    message: &str,
    color: ColorU,
) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(message.to_string(), ui_font, crate::theme::text_size(11.0))
                        .with_color(color)
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .finish()
}

/// Navbar Review chevron dropdown: Push only when
/// the branch needs it (no upstream or commits ahead), Create PR only for
/// project checkouts — greyed with the reason when zero commits are ahead.
/// A muted "Up to date" row fills the menu when nothing applies.
pub fn review_menu_overlay(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    ship: &super::view::ReviewShipState,
    busy: Option<super::action_review_git::ReviewGitAction>,
    notice: Option<&str>,
    error: Option<&str>,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    // Stretch: tiles fill the card width; the card itself stays
    // content-sized, hugging the widest row.
    let mut items = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    let push_busy = busy == Some(super::action_review_git::ReviewGitAction::Push);
    let pr_busy = busy == Some(super::action_review_git::ReviewGitAction::CreatePr);

    if ship.needs_push() {
        let label = if push_busy {
            "Pushing…".to_string()
        } else if ship.ahead > 0 {
            format!(
                "Push {} commit{}",
                ship.ahead,
                if ship.ahead == 1 { "" } else { "s" }
            )
        } else {
            "Push".to_string()
        };
        items = items.with_child(if push_busy {
            review_menu_progress(ui_font, theme, &label)
        } else {
            review_menu_item(
                ui_font,
                theme,
                view_id,
                mouse_state("review_menu_push"),
                AppIcon::ArrowUp,
                &label,
                None,
                Some(WorkspaceAction::PushBranch),
            )
        });
    }

    if ship.can_create_pr() {
        // No global toast system: the greyed row carries its reason inline
        // as detail text under the item.
        let blocked = ship.create_pr_block_reason();
        items = items.with_child(if pr_busy {
            review_menu_progress(ui_font, theme, "Creating PR…")
        } else {
            review_menu_item(
                ui_font,
                theme,
                view_id,
                mouse_state("review_menu_create"),
                AppIcon::PullRequest,
                "Create PR",
                blocked.as_deref(),
                blocked
                    .is_none()
                    .then_some(WorkspaceAction::CreatePullRequest),
            )
        });
    }

    if !ship.needs_push() && !ship.can_create_pr() {
        items = items.with_child(
            Container::new(
                Text::new(
                    "Everything is up to date".to_string(),
                    ui_font,
                    crate::theme::text_size(12.5),
                )
                .with_color(theme.disabled_text())
                .finish(),
            )
            .with_background_color(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish(),
        );
    }

    // Headless result: success notice or error under the actions, in place
    // of watching a terminal. Cloud-hosted apps toast here; this menu
    // carries the result inline instead.
    if let Some(notice) = notice {
        items = items.with_child(review_menu_note(
            ui_font,
            theme,
            notice,
            theme.ui_green(),
        ));
    }
    if let Some(error) = error {
        items = items.with_child(review_menu_note(
            ui_font,
            theme,
            error,
            theme.ui_red(),
        ));
    }

    // Dynamic width: measure the longest visible string and size the card
    // to fit it exactly (icon + spacing + tile/card padding + slack), so the
    // menu hugs its content at any row combination — never fullscreen,
    // never truncating, never wrapping.
    let blocked_reason = ship
        .can_create_pr()
        .then(|| ship.create_pr_block_reason())
        .flatten();
    let push_text: &str = if push_busy {
        "Pushing…"
    } else if ship.ahead > 0 {
        // Measure the longest plausible spelling, not this render's count.
        "Push 99 commits"
    } else {
        "Push"
    };
    let mut measure: Vec<(&str, f32)> = Vec::new();
    if ship.needs_push() {
        measure.push((push_text, 12.5));
    }
    if ship.can_create_pr() {
        measure.push(("Create PR", 12.5));
        if let Some(blocked) = blocked_reason.as_deref() {
            measure.push((blocked, 10.5));
        }
    }
    if !ship.needs_push() && !ship.can_create_pr() {
        measure.push(("Everything is up to date", 12.5));
    }
    if let Some(note) = notice {
        measure.push((note, 11.0));
    }
    if let Some(error) = error {
        measure.push((error, 11.0));
    }
    // Rows with icons carry icon + spacing chrome; note rows are text-only.
    let has_icon_row = ship.needs_push() || ship.can_create_pr() || push_busy || pr_busy;
    let text_w = measure
        .iter()
        .map(|(text, font)| text.chars().count() as f32 * font * 0.55)
        .fold(0.0f32, f32::max);
    let card_w = text_w + if has_icon_row { 21.0 } else { 0.0 } + 32.0 + 8.0;
    let menu = ConstrainedBox::new(
        Container::new(items.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish(),
    )
    .with_width(card_w.max(1.0))
    .finish();

    let catcher = EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(ColorU::new(0, 0, 0, 0))
            .finish(),
    )
    .on_left_mouse_down(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(view_id, WorkspaceAction::CloseReviewMenu);
        DispatchEventResult::StopPropagation
    })
    .finish();
    let offset = OffsetPositioning::offset_from_save_position_element(
        REVIEW_MENU_TAG,
        vec2f(0.0, 6.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    (menu, catcher, offset)
}

/// One dropdown row. Greyed rows (detail set, no action) explain themselves
/// inline as detail text under the item.
#[allow(clippy::too_many_arguments)]
fn review_menu_item(
    ui_font: FamilyId,
    theme: &Theme,
    view_id: EntityId,
    mouse: MouseStateHandle,
    icon: AppIcon,
    title: &str,
    detail: Option<&str>,
    action: Option<WorkspaceAction>,
) -> Box<dyn Element> {
    let enabled = action.is_some();
    let title = title.to_string();
    let detail = detail.map(str::to_string);
    let row = Hoverable::new(mouse, move |state| {
        let mut text_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(1.0)
            .with_child(
                Text::new(title.clone(), ui_font, crate::theme::text_size(12.5))
                    .with_color(if enabled {
                        theme.main_text()
                    } else {
                        theme.disabled_text()
                    })
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            );
        if let Some(detail) = detail.as_deref() {
            let message = detail.to_string();
            text_col = text_col.with_child(
                Text::new(message, ui_font, crate::theme::text_size(10.5))
                    .with_color(theme.disabled_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            );
        }
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(8.0)
                .with_child(render_icon(
                    icon,
                    if enabled {
                        theme.main_text()
                    } else {
                        theme.disabled_text()
                    },
                    13.0,
                ))
                .with_child(Expanded::new(1.0, text_col.finish()).finish())
                .finish(),
        )
        .with_background_color(if enabled && state.is_hovered() {
            theme.surface_3()
        } else {
            theme.surface_2()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    });
    if let Some(action) = action {
        return row
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action_for(view_id, action.clone());
            })
            .finish();
    }
    row.finish()
}

fn layout_icon(layout: BuiltInLayout) -> AppIcon {
    match layout {
        BuiltInLayout::Shape => AppIcon::MessageSquare,
        BuiltInLayout::Coordinate => AppIcon::Grid,
        BuiltInLayout::Develop => AppIcon::Code,
        BuiltInLayout::Review => AppIcon::Eye,
        BuiltInLayout::Focus => AppIcon::Maximize,
    }
}

use dragent_workspace_model::LayoutTemplate;

pub fn layout_menu_overlay(
    active_layout: BuiltInLayout,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
    agentic_workspace_enabled: bool,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let layouts = [
        BuiltInLayout::Shape,
        BuiltInLayout::Coordinate,
        BuiltInLayout::Develop,
        BuiltInLayout::Review,
        BuiltInLayout::Focus,
    ];
    let mut rows = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for layout in layouts {
        // Shape, Coordinate and Review belong to the earlier work item
        // workflow (Teams replace it): only shown to those who kept it on.
        if !agentic_workspace_enabled
            && matches!(layout, BuiltInLayout::Shape | BuiltInLayout::Coordinate | BuiltInLayout::Review)
        {
            continue;
        }
        let key = match layout {
            BuiltInLayout::Shape => "layout_shape",
            BuiltInLayout::Coordinate => "layout_coordinate",
            BuiltInLayout::Develop => "layout_develop",
            BuiltInLayout::Review => "layout_review",
            BuiltInLayout::Focus => "layout_focus",
        };
        rows = rows.with_child(layout_menu_row(
            layout,
            layout == active_layout,
            // Work-item layouts remain visible as future destinations, but
            // only Develop is currently clickable. A dedicated launcher will
            // own entry into Shape/Coordinate/Review.
            layout == BuiltInLayout::Develop,
            theme,
            ui_font,
            view_id,
            mouse_state(key),
            "Soon",
        ));
    }

    let menu = ConstrainedBox::new(
        Container::new(rows.finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish(),
    )
    .with_width(292.0)
    .finish();

    let catcher = EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(ColorU::new(0, 0, 0, 0))
            .finish(),
    )
    .on_left_mouse_down(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(view_id, WorkspaceAction::CloseLayoutMenu);
        DispatchEventResult::StopPropagation
    })
    .finish();
    let offset = OffsetPositioning::offset_from_save_position_element(
        LAYOUT_MENU_TAG,
        vec2f(0.0, 6.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomLeft,
        ChildAnchor::TopLeft,
    );
    (menu, catcher, offset)
}

fn layout_menu_row(
    layout: BuiltInLayout,
    active: bool,
    enabled: bool,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    mouse_state: MouseStateHandle,
    locked_label: &'static str,
) -> Box<dyn Element> {
    let template = LayoutTemplate::built_in(layout);
    let theme_owned = *theme;
    let row = Hoverable::new(mouse_state, move |state| {
        let foreground = if enabled {
            theme_owned.main_text()
        } else {
            theme_owned.disabled_text()
        };
        let background = if state.is_hovered() && enabled {
            theme_owned.surface_3()
        } else if active {
            theme_owned.fg_overlay_1()
        } else {
            ColorU::transparent_black()
        };
        let trailing: Box<dyn Element> = if active {
            render_icon(AppIcon::Check, theme_owned.accent, 13.0)
        } else if !enabled {
            Text::new(
                locked_label.to_string(),
                ui_font,
                crate::theme::text_size(10.0),
            )
            .with_color(theme_owned.disabled_text())
            .finish()
        } else {
            Empty::new().finish()
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(9.0)
                .with_child(render_icon(layout_icon(layout), foreground, 15.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_spacing(2.0)
                            .with_child(
                                Text::new(
                                    template.name.clone(),
                                    ui_font,
                                    crate::theme::text_size(12.0),
                                )
                                .with_color(foreground)
                                .finish(),
                            )
                            .with_child(
                                Text::new(
                                    template.description.clone(),
                                    ui_font,
                                    crate::theme::text_size(10.5),
                                )
                                .with_color(if enabled {
                                    theme_owned.sub_text()
                                } else {
                                    theme_owned.disabled_text()
                                })
                                .finish(),
                            )
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(trailing)
                .finish(),
        )
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    });

    if enabled {
        row.with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _, _| {
                event_ctx.dispatch_typed_action_for(
                    view_id,
                    WorkspaceAction::SelectWorkspaceLayout(layout),
                );
            })
            .finish()
    } else {
        row.finish()
    }
}

/// SavePosition tag the account menu anchors to (the header avatar).
pub const ACCOUNT_MENU_TAG: &str = "header_account_menu";

/// The signed-in account control: the user's avatar, which opens the account
/// menu. Sized to sit in the header's icon row alongside the toolbar buttons.
pub(crate) fn render_account_avatar_button(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    name: &str,
    avatar_url: &str,
) -> Box<dyn Element> {
    // Keep the avatar just a touch larger than the 16px settings glyph while
    // avoiding an oversized account control in the compact header.
    const SIZE: f32 = 18.0;
    let theme_owned = *theme;
    let name = name.to_string();
    let avatar_url = avatar_url.to_string();
    let button = Hoverable::new(mouse_state, move |state| {
        // The ring is ALWAYS drawn (accent on hover, outline otherwise) so the
        // avatar reads as a control at rest, not only once the pointer is over
        // it — and so its size doesn't shift on hover.
        let ring = if state.is_hovered() {
            theme_owned.accent
        } else {
            theme_owned.outline()
        };
        Container::new(
            // Ask the avatar for THIS size rather than constraining a larger
            // one: its corner radius is half the size it was built at, so a
            // 30px avatar squeezed into 24px kept a 15px radius and rendered as
            // a rounded square instead of a circle.
            crate::workspace::share_sheet::avatar_sized(
                &name,
                "",
                &avatar_url,
                &theme_owned,
                ui_font,
                SIZE,
            ),
        )
        // Radius = half the bordered box (SIZE + 2×1px border) so the ring is a
        // true circle around a circular avatar.
        .with_border(Border::all(1.0).with_border_color(ring))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(SIZE / 2.0 + 1.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::ToggleAccountMenu);
    })
    .finish();
    SavePosition::new(button, ACCOUNT_MENU_TAG).finish()
}

/// The account menu: documentation, an upgrade prompt for free accounts, and
/// sign out. Returns (menu, click-catcher, positioning) for the caller to stack,
/// matching `content_header::share_menu_overlay`.
pub fn account_menu_overlay(
    is_free_tier: bool,
    update_state: crate::autoupdate::UpdateState,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let mut rows = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    let (update_label, update_accent) = match update_state {
        crate::autoupdate::UpdateState::Ready(version) => {
            (format!("Restart to update ({version})"), true)
        }
        crate::autoupdate::UpdateState::Available(version) => {
            (format!("Update available ({version})"), true)
        }
        crate::autoupdate::UpdateState::Unknown => ("Check for updates".to_string(), false),
    };
    rows = rows.with_child(account_menu_row(
        &update_label,
        AppIcon::ArrowUp,
        WorkspaceAction::UpdateAppFromHeader,
        update_accent,
        theme,
        ui_font,
        view_id,
        mouse_state("acct_app_update"),
    ));

    rows = rows.with_child(account_menu_row(
        "Documentation",
        AppIcon::Info,
        WorkspaceAction::OpenDocumentation,
        false,
        theme,
        ui_font,
        view_id,
        mouse_state("acct_docs"),
    ));

    // Only a free account is offered the upgrade — a paying customer being told
    // to upgrade to what they already have reads as broken.
    if is_free_tier {
        rows = rows.with_child(account_menu_row(
            "Upgrade to Pro",
            AppIcon::ArrowUp,
            WorkspaceAction::OpenBillingFromHeader,
            true,
            theme,
            ui_font,
            view_id,
            mouse_state("acct_upgrade"),
        ));
    }

    rows = rows.with_child(account_menu_row(
        "Sign out",
        AppIcon::ArrowRight,
        WorkspaceAction::SignOutFromHeader,
        false,
        theme,
        ui_font,
        view_id,
        mouse_state("acct_signout"),
    ));

    let menu = ConstrainedBox::new(
        Container::new(rows.finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
    )
    .with_max_width(220.0)
    .finish();

    // Full-screen transparent catcher: a click anywhere else dismisses.
    let catcher = EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(ColorU::new(0, 0, 0, 0))
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::CloseAccountMenu);
        DispatchEventResult::StopPropagation
    })
    .finish();

    let offset = OffsetPositioning::offset_from_save_position_element(
        ACCOUNT_MENU_TAG,
        vec2f(0.0, 6.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    (menu, catcher, offset)
}

/// SavePosition tag the attention bell dropdown anchors to.
pub const ATTENTION_BELL_TAG: &str = "workspace_attention_bell";

/// Navbar bell with an open-request count badge. Always visible so the
/// human notices new approvals/answers from any screen; the badge only
/// appears while something needs them.
pub fn render_attention_bell(
    ui_font: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    open_count: usize,
    unread_results: usize,
    bell_open: bool,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let count = open_count + unread_results;
    let count_label = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    // Someone blocked on an answer is urgent; a result to read isn't.
    let badge_color = if open_count > 0 {
        theme.ui_red()
    } else {
        theme.accent
    };
    let button = Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        let urgent = count > 0;
        let icon_color = if urgent {
            badge_color
        } else if bell_open || hovered {
            theme_owned.main_text()
        } else {
            theme_owned.sub_text()
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(render_icon(AppIcon::Inbox, icon_color, 13.0));
        if urgent {
            row = row.with_child(
                Container::new(
                    Text::new(count_label.clone(), ui_font, crate::theme::text_size(10.5))
                        .with_color(badge_color)
                        .finish(),
                )
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .with_padding_top(1.0)
                .with_padding_bottom(1.0)
                .with_border(Border::all(1.0).with_border_color(badge_color))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish(),
            );
        }
        Container::new(row.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(if bell_open {
                theme_owned.fg_overlay_1()
            } else if hovered {
                theme_owned.surface_1()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::ToggleWorkspaceAttentionBell,
        );
    })
    .finish();
    SavePosition::new(button, ATTENTION_BELL_TAG).finish()
}

/// An automation result nobody has opened yet, as a row in the bell's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationNotice {
    pub run_id: String,
    pub automation_id: String,
    pub name: String,
    /// How the run ended and how long ago: `Failed · 5m`.
    pub detail: String,
}

/// How many automation results the list shows; the rest are in the inbox.
pub(crate) const MAX_AUTOMATION_NOTICES: usize = 6;

/// Attention dropdown: the request list and unread automation results, or the
/// selected request's detail card, with its own quick-resolve actions (approve/answer/always-allow) for
/// fast turnaround from any screen. The Coordinate rail's equivalent buttons
/// were removed — Coordinate is read-only now — but this popover keeps its
/// own resolve path deliberately, since it has capabilities (free-text
/// answers and standing tool approval) without opening the dormant work-item
/// surface. Anchored under the navbar bell; a click anywhere else dismisses.
#[allow(clippy::too_many_arguments)]
pub fn attention_menu_overlay(
    entries: &[AttentionOverlayEntry],
    automations: &[AutomationNotice],
    selected: Option<AttentionItemId>,
    response_editor: &ViewHandle<InputEditor>,
    response_text: &str,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let catcher = EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(ColorU::new(0, 0, 0, 0))
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::ToggleWorkspaceAttentionBell);
        DispatchEventResult::StopPropagation
    })
    .finish();
    let offset = OffsetPositioning::offset_from_save_position_element(
        ATTENTION_BELL_TAG,
        vec2f(0.0, 6.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    if let Some(selected_id) = selected {
        if let Some(entry) = entries.iter().find(|entry| entry.item.id == selected_id) {
            let card = super::execution_workspace::attention_popover_card(
                entry,
                entries.len(),
                response_editor,
                response_text,
                view_id,
                theme,
                ui_font,
            );
            return (card, catcher, offset);
        }
    }
    let mut rows = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    if entries.is_empty() && automations.is_empty() {
        rows = rows.with_child(
            Container::new(
                Text::new(
                    "All caught up — no open requests.".to_string(),
                    ui_font,
                    crate::theme::text_size(11.5),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_uniform_padding(10.0)
            .finish(),
        );
    }
    for entry in entries {
        rows = rows.with_child(attention_menu_row(entry, theme, ui_font, view_id));
    }
    for notice in automations.iter().take(MAX_AUTOMATION_NOTICES) {
        rows = rows.with_child(automation_notice_row(notice, theme, ui_font, view_id));
    }
    if automations.len() > MAX_AUTOMATION_NOTICES {
        rows = rows.with_child(automation_inbox_row(
            automations.len() - MAX_AUTOMATION_NOTICES,
            theme,
            ui_font,
            view_id,
        ));
    }
    let menu = ConstrainedBox::new(
        Container::new(rows.finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
    )
    .with_width(380.0)
    .finish();
    (menu, catcher, offset)
}

fn attention_menu_row(
    entry: &AttentionOverlayEntry,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let item_id = entry.item.id;
    let kind = super::execution_workspace::attention_label(entry.item.kind).to_string();
    let summary = entry.item.summary.clone();
    // Row-local hover state like the Coordinate task cards (single-use
    // elements rebuild per frame; no shared key needed).
    let context = match (&entry.work_title, entry.task_title.as_deref()) {
        (Some(work), Some(task)) => format!("{work} · {task}"),
        (Some(work), None) => work.clone(),
        (None, Some(task)) => task.to_string(),
        (None, None) => super::execution_workspace::format_time(entry.item.created_at_unix_ms),
    };
    let row = Hoverable::new(
        super::execution_workspace::stable_mouse(format!("bell:row:{item_id:?}")),
        move |state| {
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(2.0)
                    .with_child(
                        Text::new(summary.clone(), ui_font, crate::theme::text_size(11.5))
                            .with_color(theme_owned.main_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .with_child(
                        Text::new(
                            format!("{kind} · {context}"),
                            ui_font,
                            crate::theme::text_size(10.0),
                        )
                        .with_color(theme_owned.disabled_text())
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                    )
                    .finish(),
            )
            .with_uniform_padding(8.0)
            .with_background_color(if state.is_hovered() {
                theme_owned.surface_1()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(
            view_id,
            WorkspaceAction::SelectWorkspaceAttentionItem {
                attention_item_id: Some(item_id),
            },
        );
    })
    .finish();
    Container::new(row)
        .with_padding_left(2.0)
        .with_padding_right(2.0)
        .with_padding_top(1.0)
        .with_padding_bottom(1.0)
        .finish()
}

/// One unread automation result. Clicking opens its automation and clears it.
fn automation_notice_row(
    notice: &AutomationNotice,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let name = notice.name.clone();
    let detail = notice.detail.clone();
    let action = WorkspaceAction::OpenAutomationNotice {
        run_id: notice.run_id.clone(),
        automation_id: notice.automation_id.clone(),
    };
    let row = Hoverable::new(
        super::execution_workspace::stable_mouse(format!("bell:automation:{}", notice.run_id)),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(render_icon(AppIcon::Clock, theme_owned.sub_text(), 13.0))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Flex::column()
                                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                                .with_spacing(2.0)
                                .with_child(
                                    Text::new(name.clone(), ui_font, crate::theme::text_size(11.5))
                                        .with_color(theme_owned.main_text())
                                        .soft_wrap(false)
                                        .with_clip(ClipConfig::ellipsis())
                                        .finish(),
                                )
                                .with_child(
                                    Text::new(
                                        format!("Automation · {detail}"),
                                        ui_font,
                                        crate::theme::text_size(10.0),
                                    )
                                    .with_color(theme_owned.disabled_text())
                                    .soft_wrap(false)
                                    .with_clip(ClipConfig::ellipsis())
                                    .finish(),
                                )
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .with_uniform_padding(8.0)
            .with_background_color(if state.is_hovered() {
                theme_owned.surface_1()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(view_id, action.clone());
    })
    .finish();
    Container::new(row)
        .with_padding_left(2.0)
        .with_padding_right(2.0)
        .with_padding_top(1.0)
        .with_padding_bottom(1.0)
        .finish()
}

/// More unread results than the list shows: the way to the rest.
fn automation_inbox_row(
    more: usize,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let row = Hoverable::new(
        super::execution_workspace::stable_mouse("bell:automation-inbox".to_string()),
        move |state| {
            Container::new(
                Text::new(
                    format!("{more} more in the Automations inbox"),
                    ui_font,
                    crate::theme::text_size(11.0),
                )
                .with_color(if state.is_hovered() {
                    theme_owned.main_text()
                } else {
                    theme_owned.sub_text()
                })
                .finish(),
            )
            .with_uniform_padding(8.0)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(view_id, WorkspaceAction::OpenAutomationInbox);
    })
    .finish();
    Container::new(row)
        .with_padding_left(2.0)
        .with_padding_right(2.0)
        .finish()
}

use super::header::{account_menu_row, LAYOUT_MENU_TAG, REVIEW_MENU_TAG};
