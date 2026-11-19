//! Settings sidebar navigation: searchable tree with collapsible umbrellas.
//!
//! Information architecture (grouped sections + global search):
//! top-level pages first, then the Agent / Editor & Code / Organisation
//! umbrellas, then the System label group. Search filters across labels,
//! umbrella names, and per-item keywords.

use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, Element, Flex, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, ScrollbarWidth, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui_core::elements::Fill;
use warpui_core::elements::Hoverable;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui_core::ui_components::tool_tip::Tooltip;

use crate::theme::Theme;

use super::action::{SettingsAction, SettingsSection};
use super::settings_view::SettingsView;
use super::widgets::{nav_divider, nav_item, nav_section_label, nav_umbrella_header, NAV_WIDTH};
use crate::icons::{render_icon, AppIcon};

#[derive(Clone, Copy, Debug)]
struct NavItemMeta {
    section: SettingsSection,
    label: &'static str,
    icon: &'static str,
    /// Umbrella name, or `None` for top-level items and System group rows.
    umbrella: Option<&'static str>,
    /// `Some` for System group rows (rendered under a plain label).
    system: bool,
    keywords: &'static str,
}

const UMBRELLA_AGENT: &str = "Agent";
const UMBRELLA_EDITOR: &str = "Editor & Code";
const UMBRELLA_ORG: &str = "Organisation";

const NAV_ITEMS: &[NavItemMeta] = &[
    NavItemMeta { section: SettingsSection::Usage, label: "Usage", icon: "Usage", umbrella: Some(UMBRELLA_AGENT), system: false, keywords: "usage tokens cost spend limits rate limit quota plan claude codex opencode" },
    NavItemMeta { section: SettingsSection::AgentAccounts, label: "Agent Accounts", icon: "Account", umbrella: Some(UMBRELLA_AGENT), system: false, keywords: "provider accounts personal work login authentication profiles" },
    NavItemMeta {
        section: SettingsSection::Machines,
        label: "Machines",
        icon: "Server",
        umbrella: None,
        system: false,
        keywords: "machines ssh remote server connections host runtime projects",
    },
    NavItemMeta {
        section: SettingsSection::Account,
        label: "Account",
        icon: "Account",
        umbrella: None,
        system: false,
        keywords: "account sign in profile user",
    },
    NavItemMeta {
        section: SettingsSection::Appearance,
        label: "Appearance",
        icon: "Palette",
        umbrella: None,
        system: false,
        keywords: "appearance theme font typeface size weight bold interface code cursor text display",
    },
    NavItemMeta {
        section: SettingsSection::TerminalInput,
        label: "Terminal & input",
        icon: "Terminal",
        umbrella: None,
        system: false,
        keywords: "terminal shell input behavior",
    },
    NavItemMeta {
        section: SettingsSection::KeyboardShortcuts,
        label: "Keyboard shortcuts",
        icon: "Keyboard",
        umbrella: None,
        system: false,
        keywords: "keyboard shortcuts keybindings keys",
    },
    NavItemMeta {
        section: SettingsSection::Voice,
        label: "Voice",
        icon: "Mic",
        umbrella: None,
        system: false,
        keywords: "voice dictation speech microphone stt transcription input talk",
    },
    // ── Agent umbrella ──
    NavItemMeta {
        section: SettingsSection::AgentGeneral,
        label: "Behavior",
        icon: "Sliders",
        umbrella: Some(UMBRELLA_AGENT),
        system: false,
        keywords: "agent behavior autonomy approval permissions memory sleep idle",
    },
    NavItemMeta {
        section: SettingsSection::AgentModels,
        label: "Models & keys",
        icon: "Key",
        umbrella: Some(UMBRELLA_AGENT),
        system: false,
        keywords: "models providers api keys anthropic openai llm",
    },
    NavItemMeta {
        section: SettingsSection::AgentMcp,
        label: "MCP servers",
        icon: "Server",
        umbrella: Some(UMBRELLA_AGENT),
        system: false,
        keywords: "mcp servers model context protocol tools",
    },
    NavItemMeta {
        section: SettingsSection::AgentSkills,
        label: "Skills",
        icon: "Agent",
        umbrella: Some(UMBRELLA_AGENT),
        system: false,
        keywords: "skills agent import",
    },
    NavItemMeta {
        section: SettingsSection::AgentCommands,
        label: "Commands",
        icon: "Commands",
        umbrella: Some(UMBRELLA_AGENT),
        system: false,
        keywords: "commands slash custom agent",
    },
    // ── Editor & Code umbrella ──
    NavItemMeta {
        section: SettingsSection::Editor,
        label: "Editor",
        icon: "Editor",
        umbrella: Some(UMBRELLA_EDITOR),
        system: false,
        keywords: "editor vim editing format tab spaces",
    },
    NavItemMeta {
        section: SettingsSection::CodeIndex,
        label: "Code indexing",
        icon: "Code",
        umbrella: Some(UMBRELLA_EDITOR),
        system: false,
        keywords: "code index indexing symbols search",
    },
    // ── Organisation umbrella ──
    NavItemMeta {
        section: SettingsSection::Members,
        label: "Members",
        icon: "Members",
        umbrella: Some(UMBRELLA_ORG),
        system: false,
        keywords: "members invites roles access users",
    },
    NavItemMeta {
        section: SettingsSection::Teams,
        label: "Teams",
        icon: "Teams",
        umbrella: Some(UMBRELLA_ORG),
        system: false,
        keywords: "teams groups organization",
    },
    NavItemMeta {
        section: SettingsSection::Billing,
        label: "Billing & Usage",
        icon: "Billing",
        umbrella: Some(UMBRELLA_ORG),
        system: false,
        keywords: "billing usage quota credits invoices plan seats",
    },
    NavItemMeta {
        section: SettingsSection::Projects,
        label: "Projects",
        icon: "Projects",
        umbrella: Some(UMBRELLA_ORG),
        system: false,
        keywords: "projects organization repositories",
    },
    NavItemMeta {
        section: SettingsSection::Integrations,
        label: "Integrations",
        icon: "Integrations",
        umbrella: Some(UMBRELLA_ORG),
        system: false,
        keywords: "integrations linear github slack notion connect",
    },
    // ── System group ──
    NavItemMeta {
        section: SettingsSection::Privacy,
        label: "Privacy",
        icon: "Eye",
        umbrella: None,
        system: true,
        keywords: "privacy telemetry data analytics",
    },
    NavItemMeta {
        section: SettingsSection::Notifications,
        label: "Notifications",
        icon: "Notifications",
        umbrella: None,
        system: true,
        keywords: "notifications sounds alerts desktop",
    },
    NavItemMeta {
        section: SettingsSection::RemoteAccess,
        label: "Remote Access",
        icon: "Remote",
        umbrella: None,
        system: true,
        keywords: "remote access relay ssh sessions",
    },
    NavItemMeta {
        section: SettingsSection::Mobile,
        label: "Mobile",
        icon: "Mobile",
        umbrella: None,
        system: true,
        keywords: "mobile phone pair pairing qr code companion app",
    },
    NavItemMeta {
        section: SettingsSection::DeveloperPermissions,
        label: "Developer permissions",
        icon: "Key",
        umbrella: None,
        system: true,
        keywords: "developer permissions accessibility full disk access awake sleep caffeinate",
    },
    NavItemMeta {
        section: SettingsSection::ComputerUse,
        label: "Computer use",
        icon: "Commands",
        umbrella: None,
        system: true,
        keywords: "computer use automation allowlist confirmation safety",
    },
    NavItemMeta {
        section: SettingsSection::About,
        label: "About",
        icon: "Info",
        umbrella: None,
        system: true,
        keywords: "about version updates changelog docs",
    },
];

const UMBRELLAS: &[&str] = &[UMBRELLA_AGENT, UMBRELLA_EDITOR, UMBRELLA_ORG];

fn render_item(
    view: &SettingsView,
    theme: &Theme,
    ui_font: FamilyId,
    id: warpui::EntityId,
    item: &NavItemMeta,
    section: SettingsSection,
    hover_prefix: &str,
    indented: bool,
) -> Box<dyn Element> {
    let el = nav_item(
        theme,
        ui_font,
        id,
        view.hover(&format!("{hover_prefix}_{:?}", item.section)),
        item.label,
        item.icon,
        item.section,
        section == item.section,
    );
    if indented {
        Container::new(el).with_padding_left(12.0).finish()
    } else {
        el
    }
}

pub fn render_nav(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let id = view.view_id;
    let ui_font = view.ui_font;
    let section = view.section;

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(nav_brand_header(view, theme));

    let q = view.search_query.trim().to_lowercase();
    let mut list_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);

    if !q.is_empty() {
        // Search across labels, umbrella names, and keywords (registry
        // pattern, Warp match-count spirit).
        let mut match_count = 0;
        for item in NAV_ITEMS {
            let haystack = format!(
                "{} {} {}",
                item.label,
                item.umbrella.unwrap_or_default(),
                item.keywords
            )
            .to_lowercase();
            if q.split_whitespace().all(|word| haystack.contains(word)) {
                match_count += 1;
                list_col = list_col.with_child(render_item(
                    view,
                    theme,
                    ui_font,
                    id,
                    item,
                    section,
                    "nav_search",
                    false,
                ));
            }
        }
        if match_count == 0 {
            list_col = list_col.with_child(
                Container::new(
                    Text::new(
                        "No matches".to_string(),
                        ui_font,
                        crate::theme::text_size(12.5),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .with_uniform_padding(10.0)
                .finish(),
            );
        }
    } else {
        // Top-level items.
        for item in NAV_ITEMS {
            if item.umbrella.is_none() && !item.system && item.section != SettingsSection::Machines {
                list_col = list_col.with_child(render_item(
                    view, theme, ui_font, id, item, section, "nav_top", false,
                ));
            }
        }

        // Umbrellas (Warp pattern): collapsible header + indented children.
        for umbrella in UMBRELLAS {
            list_col = list_col.with_child(nav_divider(theme));
            let expanded = !view.collapsed_umbrellas.contains(umbrella);
            let child_active = NAV_ITEMS
                .iter()
                .any(|item| item.umbrella == Some(*umbrella) && item.section == section);
            list_col = list_col.with_child(nav_umbrella_header(
                theme,
                ui_font,
                id,
                view.hover(&format!("nav_umbrella_{umbrella}")),
                umbrella,
                expanded,
                child_active,
                SettingsAction::ToggleUmbrella(umbrella),
            ));
            if expanded {
                for item in NAV_ITEMS {
                    if item.umbrella == Some(*umbrella) {
                        list_col = list_col.with_child(render_item(
                            view, theme, ui_font, id, item, section, "nav_umb", true,
                        ));
                    }
                }
            }
        }

        // Remote label group (Settings → Remote → Machines).
        list_col = list_col.with_child(nav_divider(theme));
        list_col = list_col.with_child(nav_section_label(theme, ui_font, "Remote"));
        for item in NAV_ITEMS {
            if item.section == SettingsSection::Machines {
                list_col = list_col.with_child(render_item(
                    view, theme, ui_font, id, item, section, "nav_remote", false,
                ));
            }
        }

        // System label group.
        list_col = list_col.with_child(nav_divider(theme));
        list_col = list_col.with_child(nav_section_label(theme, ui_font, "System"));
        for item in NAV_ITEMS {
            if item.system {
                list_col = list_col.with_child(render_item(
                    view, theme, ui_font, id, item, section, "nav_sys", false,
                ));
            }
        }
    }

    // Explicit cap: the settings row lives under an unbounded-height
    // ancestor, so the tree cannot rely on parent constraints to scroll.
    // Room is reserved for the brand header + search above it.
    let list = ClippedScrollable::vertical(
        view.nav_scroll.clone(),
        list_col.finish(),
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.sidebar_background().into(),
    )
    .finish();
    let list = ConstrainedBox::new(list)
        .with_max_height((view.pane_height() - 202.0).max(200.0))
        .finish();
    col = col.with_child(list);

    col.finish()
}
fn nav_brand_header(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let view_id = view.view_id;
    // This header lives outside nav_scroll, so Back stays available while
    // either the settings navigation or the page content is scrolled.
    let back = Box::new(
        Hoverable::new(view.hover("settings_back"), move |state| {
            let color = if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(render_icon(AppIcon::ArrowLeft, color, 15.0))
                    .with_child(
                        Text::new("Back".to_string(), ui_font, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .with_background_color(if state.is_hovered() {
                theme.sidebar_row_hover()
            } else {
                theme.sidebar_background()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .with_cursor(warpui_core::platform::Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, SettingsAction::Close);
        }),
    );
    let logo_box = ConstrainedBox::new(
        Container::new(
            Align::new(
                Text::new("D".to_string(), ui_font, crate::theme::text_size(12.0))
                    .with_color(theme.accent_text())
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(theme.accent)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish(),
    )
    .with_width(24.0)
    .with_height(24.0)
    .finish();

    let header_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(9.0)
        .with_child(logo_box)
        .with_child(
            Text::new("Settings".to_string(), ui_font, crate::theme::text_size(15.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .finish();

    let trailing: Option<Box<dyn Element>> = if !view.search_query.is_empty() {
        let view_id = view.view_id;
        Some(Box::new(
            Hoverable::new(view.hover("settings_search_clear"), move |state| {
                let icon_color = if state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.disabled_text()
                };
                Container::new(render_icon(AppIcon::XClose, icon_color, 11.0))
                    .with_padding_left(4.0)
                    .with_padding_right(2.0)
                    .finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(
                    view_id,
                    super::action::SettingsAction::ClearSettingsSearch,
                );
            }),
        ))
    } else {
        None
    };
    // InputEditor uses an Expanded canvas internally. Give the box a finite
    // width because this header can otherwise receive an unbounded
    // horizontal constraint from the settings pane. The width matches the
    // previous layout exactly: editor width plus the leading icon + spacing.
    let search_container = crate::shared_input::shared_input(
        theme,
        &view.search_editor,
        crate::shared_input::SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: AppIcon::Search,
                size: 12.0,
            }),
            trailing,
            width: crate::shared_input::SharedInputWidth::Fixed(NAV_WIDTH - 46.0),
            ..Default::default()
        },
    );

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(back)
            .with_child(header_row)
            .with_child(search_container)
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(18.0)
    .with_padding_bottom(12.0)
    .finish()
}

#[allow(dead_code)]
pub const NAV_WIDTH_PX: f32 = NAV_WIDTH;

/// Tooltip opening to the right of a rail icon (the rail hugs the left
/// window edge, so above/below would collide with neighboring icons).
fn rail_tooltip(
    child: Box<dyn Element>,
    hovered: bool,
    label: String,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    if !hovered {
        return child;
    }
    let styles = UiComponentStyles {
        font_family_id: Some(font),
        font_size: Some(crate::theme::text_size(10.5)),
        font_color: Some(theme.main_text()),
        background: Some(Fill::Solid(theme.surface_2())),
        border_color: Some(Fill::Solid(theme.outline())),
        border_width: Some(1.0),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(5.0))),
        padding: Some(Coords {
            top: 4.0,
            bottom: 4.0,
            left: 7.0,
            right: 7.0,
        }),
        ..Default::default()
    };
    let tooltip = Tooltip::new(label, styles).build().finish();
    Stack::new()
        .with_child(child)
        .with_positioned_overlay_child(
            tooltip,
            OffsetPositioning::offset_from_parent(
                vec2f(6.0, 0.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::MiddleRight,
                ChildAnchor::MiddleLeft,
            ),
        )
        .finish()
}

/// Collapsed icon rail: shown instead of the full sidebar
/// while the theme chooser is open. Icons stay functional; the chevron at the
/// bottom restores the sidebar (and closes the chooser).
pub fn render_nav_rail(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let id = view.view_id;
    let ui_font = view.ui_font;
    let section = view.section;

    let mut list_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(2.0);
    for item in NAV_ITEMS {
        let selected = section == item.section;
        let icon = crate::settings::widgets::map_icon_str(item.icon);
        let target = item.section;
        let label = item.label.to_string();
        list_col = list_col.with_child(Box::new(
            Hoverable::new(
                view.hover(&format!("nav_rail_{:?}", item.section)),
                move |state| {
                    let hovered = state.is_hovered();
                    let button = Container::new(render_icon(
                        icon,
                        if selected {
                            theme.accent
                        } else if hovered {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        },
                        16.0,
                    ))
                    .with_uniform_padding(10.0)
                    .with_background_color(if selected {
                        theme.accent_bg()
                    } else if hovered {
                        theme.sidebar_row_hover()
                    } else {
                        warpui::color::ColorU::new(0, 0, 0, 0)
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish();
                    rail_tooltip(button, hovered, label, theme, ui_font)
                },
            )
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::SelectSection(target));
            }),
        ));
    }
    // The icon list scrolls; the expand chevron stays pinned below it.
    // Capped like the main tree (unbounded-height ancestor above).
    let list = ClippedScrollable::vertical(
        view.nav_scroll.clone(),
        list_col.finish(),
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.sidebar_background().into(),
    )
    .finish();
    let list = ConstrainedBox::new(list)
        .with_max_height((view.pane_height() - 96.0).max(200.0))
        .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(Box::new(
            Hoverable::new(view.hover("settings_rail_back"), move |state| {
                let button = Container::new(render_icon(
                    AppIcon::ArrowLeft,
                    if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    },
                    16.0,
                ))
                .with_uniform_padding(10.0)
                .with_background_color(if state.is_hovered() {
                    theme.sidebar_row_hover()
                } else {
                    theme.sidebar_background()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish();
                rail_tooltip(
                    button,
                    state.is_hovered(),
                    "Back".to_string(),
                    theme,
                    ui_font,
                )
            })
            .with_cursor(warpui_core::platform::Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::Close);
            }),
        ))
        .with_child(list)
        // Expand chevron restores the full sidebar.
        .with_child(Box::new(
            Hoverable::new(view.hover("nav_rail_expand"), move |state| {
                Container::new(render_icon(
                    AppIcon::ChevronRight,
                    if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.disabled_text()
                    },
                    14.0,
                ))
                .with_uniform_padding(10.0)
                .finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::CloseThemeChooser);
            }),
        ))
        .finish()
}
