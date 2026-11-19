//! Agent quick-access rail: a narrow sidebar beside the right sidebar,
//! visible only inside an open workspace.
//!
//! Collapsed it shows one icon per supported agent (real provider artwork,
//! dimmed with a download badge when not installed) with name tooltips;
//! clicking an installed agent opens it immediately on a brand-new
//! workspace tab, while clicking an uninstalled one opens its installer.
//! Expanded it shows icon + name + an install action per agent.
//! Installation reuses the existing project-prompt modal/flow
//! (`OpenPromptAgentInstall`) and its completion already refreshes
//! `prompt_installed_agents`, so the rail follows with no restart.
//! Launching reuses the workspace-tab system plus the same command
//! resolution as the project prompt — no parallel agent session logic.

use warpui::elements::{
    Align, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex, Image, MainAxisSize,
    ParentElement, ScrollbarWidth, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{AppContext, EntityId};
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Fill, Hoverable, MouseStateHandle};
use warpui_core::image_cache::CacheOption;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui_core::ui_components::tool_tip::Tooltip;

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme, HEADER_BAR_HEIGHT};

use super::view::WorkspaceView;
use super::agent_definition::{AgentDefinition, AgentImage};
use super::{WorkspaceAction};

pub(crate) const AGENT_RAIL_COLLAPSED_WIDTH: f32 = 41.6;
pub(crate) const AGENT_RAIL_EXPANDED_WIDTH: f32 = 182.4;
const AGENT_RAIL_ICON: f32 = 15.4;

/// Show the login picker only when choosing matters: an isolated provider
/// (claude/codex) with ≥1 saved extra profile. Single-identity System-only
/// agents and non-isolated agents (opencode, … — like Dragent, which never
/// has profiles) open directly with the default account and never see the
/// modal.
pub(crate) fn should_show_account_picker(agent_id: &str) -> bool {
    if !dragent_host::agent_accounts::capability(agent_id).isolated {
        return false;
    }
    super::workspace_tab_strip::provider_has_multiple_identities(agent_id)
}

/// Tooltip opening to the LEFT of a rail icon, clear of the rail itself:
/// the tooltip's right edge anchors to the icon's left edge so it floats
/// over the workspace content and never covers (or steals clicks from) the
/// neighboring rail icons.
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
            warpui::elements::OffsetPositioning::offset_from_parent(
                vec2f(-8.0, 0.0),
                warpui::elements::ParentOffsetBounds::WindowByPosition,
                warpui::elements::ParentAnchor::MiddleLeft,
                warpui::elements::ChildAnchor::MiddleRight,
            ),
        )
        .finish()
}

/// Both rail layouts install through the embedded-terminal install modal.
fn install_action(definition: &AgentDefinition) -> WorkspaceAction {
    match definition.builtin {
        Some(agent) => WorkspaceAction::OpenPromptAgentInstall { agent },
        None => WorkspaceAction::OpenCustomAgentInstall {
            agent_id: definition.id.clone(),
        },
    }
}

/// Rail icon from the agent definition: bundled artwork, user-supplied
/// file, or the generic glyph. Same resolution as every other surface.
fn definition_icon(
    definition: &AgentDefinition,
    theme: &Theme,
    size: f32,
    dimmed: bool,
) -> Box<dyn Element> {
    let opacity = if dimmed { 0.45 } else { 1.0 };
    let image: Option<Box<dyn Element>> = match &definition.image {
        Some(AgentImage::Bundled(path)) => Some(
            ConstrainedBox::new(
                Image::new(AssetSource::Bundled { path }, CacheOption::BySize)
                    .contain()
                    .with_opacity(opacity)
                    .finish(),
            )
            .with_width(size)
            .with_height(size)
            .finish(),
        ),
        Some(AgentImage::File(path)) => Some(
            ConstrainedBox::new(
                Image::new(
                    AssetSource::LocalFile { path: path.clone() },
                    CacheOption::BySize,
                )
                .contain()
                .with_opacity(opacity)
                .finish(),
            )
            .with_width(size)
            .with_height(size)
            .finish(),
        ),
        None => None,
    };
    if let Some(image) = image {
        return image;
    }
    ConstrainedBox::new(
        render_icon(
            AppIcon::Agent,
            if dimmed {
                theme.disabled_text()
            } else {
                theme.sub_text()
            },
            size,
        )
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

impl WorkspaceView {
    /// The agent rail column. The caller gates workspace-only visibility;
    /// this only lays out the fixed-width column.
    pub(crate) fn render_agent_rail(&self, app: &AppContext, theme: &Theme) -> Box<dyn Element> {
        // A remote project's rail shows what's installed on its machine.
        let machine = self.machine_agents(self.active_project_root().as_deref(), app);
        let ui_font: FamilyId = self.ui_font;
        let view_id: EntityId = self.workspace_view_id;
        let theme_owned = *theme;
        let expanded = self.agent_rail_expanded;
        let width = if expanded {
            AGENT_RAIL_EXPANDED_WIDTH
        } else {
            AGENT_RAIL_COLLAPSED_WIDTH
        };

        let toggle_mouse = self
            .header_mouse_states
            .borrow_mut()
            .entry("agent_rail_toggle".to_string())
            .or_default()
            .clone();
        let toggle = Hoverable::new(toggle_mouse, move |state| {
            let hovered = state.is_hovered();
            Container::new(Align::new(render_icon(
                if expanded {
                    AppIcon::ArrowRight
                } else {
                    AppIcon::ArrowLeft
                },
                if hovered {
                    theme_owned.main_text()
                } else {
                    theme_owned.sub_text()
                },
                13.0,
            )).finish())
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(if hovered {
                theme_owned.surface_1()
            } else {
                warpui::color::ColorU::new(0, 0, 0, 0)
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                view_id,
                WorkspaceAction::SetAgentRailExpanded { expanded: !expanded },
            )
        });
        let toggle: Box<dyn Element> = if expanded {
            // Expanded rows carry their own tooltips only on the install
            // action; the toggle keeps one so its chevron stays guessable.
            rail_tooltip(
                Box::new(toggle),
                // Re-read hover: the toggle's own Hoverable already gates
                // paint, but the tooltip needs the same live state.
                self.header_mouse_states
                    .borrow_mut()
                    .entry("agent_rail_toggle".to_string())
                    .or_default()
                    .lock()
                    .ok()
                    .is_some_and(|state| state.is_hovered()),
                if expanded {
                    "Collapse agents".to_string()
                } else {
                    "Expand agents".to_string()
                },
                theme,
                ui_font,
            )
        } else {
            Box::new(toggle)
        };

        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(2.0);
        // Every definition — built-ins plus user customs — from the agent
        // registry. Customs render from the cached settings snapshot so the
        // rail never hits the settings file per frame.
        for definition in super::agent_definition::all_definitions(
            &self.cached_custom_agents,
            theme.is_dark,
        ) {
            let installed = match (&machine, definition.builtin) {
                (_, Some(super::action::PromptAgent::Dragent)) => true,
                (Some(machine), Some(agent)) => agent.cli().is_some_and(|cli| machine.has(cli)),
                (Some(machine), None) => self
                    .cached_custom_agents
                    .iter()
                    .find(|custom| super::agent_definition::custom_id(&custom.id) == definition.id)
                    .and_then(|custom| {
                        custom
                            .cli
                            .clone()
                            .or_else(|| custom.launch.command.split_whitespace().next().map(str::to_string))
                    })
                    .is_some_and(|cli| machine.has(&cli)),
                (None, Some(agent)) => self.prompt_installed_agents.contains(&agent),
                (None, None) => self.custom_installed_agents.contains(&definition.id),
            };
            if expanded {
                list = list.with_child(self.render_agent_row_expanded(
                    &definition,
                    installed,
                    &theme_owned,
                    ui_font,
                    view_id,
                ));
            } else {
                list = list.with_child(self.render_agent_icon_collapsed(
                    &definition,
                    installed,
                    &theme_owned,
                    ui_font,
                    view_id,
                ));
            }
        }
        let clear = warpui::color::ColorU::new(0, 0, 0, 0);
        let scroller = ClippedScrollable::vertical(
            self.agent_rail_scroll.clone(),
            list.finish(),
            ScrollbarWidth::None,
            clear.into(),
            clear.into(),
            clear.into(),
        )
        .finish();

        let column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                // Header row: fixed HEADER_BAR_HEIGHT with the same
                // sidebar background + bottom border as the main and right
                // headers, so the divider reads as one straight line.
                Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_child(
                            ConstrainedBox::new(
                                Container::new(Align::new(toggle).finish())
                                    .with_padding_left(6.0)
                                    .with_padding_right(6.0)
                                    .finish(),
                            )
                            .with_height(HEADER_BAR_HEIGHT)
                            .finish(),
                        )
                        .finish(),
                )
                .with_background_color(theme.sidebar_background())
                .with_border(Border::bottom(1.0).with_border_color(theme.border()))
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    Container::new(scroller)
                        .with_padding_left(6.0)
                        .with_padding_right(6.0)
                        .with_padding_top(6.0)
                        .finish(),
                )
                .finish(),
            )
            .finish();
        ConstrainedBox::new(
            Container::new(column)
                .with_background_color(theme.sidebar_background())
                .with_border(Border::left(1.0).with_border_color(theme.outline()))
                .finish(),
        )
        .with_width(width)
        .finish()
    }

    /// Collapsed icon button with a name tooltip. Installed launches
    /// immediately; uninstalled opens the install modal.
    fn render_agent_icon_collapsed(
        &self,
        definition: &AgentDefinition,
        installed: bool,
        theme: &Theme,
        ui_font: FamilyId,
        view_id: EntityId,
    ) -> Box<dyn Element> {
        let agent_id = definition.id.clone();
        let mouse = self
            .header_mouse_states
            .borrow_mut()
            .entry(format!("agent_rail_{agent_id}"))
            .or_default()
            .clone();
        let hover_mouse_key = format!("agent_rail_{agent_id}");
        let theme_owned = *theme;
        let name = definition.name.clone();
        // While detection is running nothing is offered for install yet.
        let installable = !installed && !self.prompt_agent_checking;
        let install_action = install_action(definition);
        let mut icon = definition_icon(definition, theme, AGENT_RAIL_ICON, !installed);
        if installable {
            icon = Stack::new()
                .with_child(icon)
                .with_positioned_child(
                    render_icon(AppIcon::Download, theme.accent, 10.0),
                    warpui::elements::OffsetPositioning::offset_from_parent(
                        vec2f(3.0, 3.0),
                        warpui::elements::ParentOffsetBounds::Unbounded,
                        warpui::elements::ParentAnchor::BottomRight,
                        warpui::elements::ChildAnchor::BottomRight,
                    ),
                )
                .finish();
        }
        let button = Hoverable::new(mouse, move |state| {
            let hovered = state.is_hovered();
            Container::new(Align::new(icon).finish())
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_background_color(if hovered {
                    theme_owned.surface_1()
                } else {
                    warpui::color::ColorU::new(0, 0, 0, 0)
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                view_id,
                if installable {
                    install_action.clone()
                } else if !installed {
                    WorkspaceAction::SetAgentRailExpanded { expanded: true }
                } else if should_show_account_picker(&agent_id) {
                    WorkspaceAction::OpenAgentAccountPicker { agent_id: agent_id.clone() }
                } else {
                    WorkspaceAction::OpenAgentInNewWorkspaceTab {
                        agent_id: agent_id.clone(),
                    }
                },
            )
        });
        let tooltip = if installed {
            name
        } else if self.prompt_agent_checking {
            // Detection is still running: never claim "not installed" for a
            // state we have not resolved yet.
            format!("{name} (checking…)")
        } else {
            format!("Install {name}")
        };
        // Tooltip needs the live hover state, like the toggle above.
        let hovered = self
            .header_mouse_states
            .borrow_mut()
            .entry(hover_mouse_key)
            .or_default()
            .lock()
            .ok()
            .is_some_and(|state| state.is_hovered());
        rail_tooltip(Box::new(button), hovered, tooltip, theme, ui_font)
    }

    /// Expanded row: icon + name, plus an install action when missing. Row
    /// clicks launch installed agents and install uninstalled ones.
    fn render_agent_row_expanded(
        &self,
        definition: &AgentDefinition,
        installed: bool,
        theme: &Theme,
        ui_font: FamilyId,
        view_id: EntityId,
    ) -> Box<dyn Element> {
        let agent_id = definition.id.clone();
        let install_action = install_action(definition);
        let row_install_action = install_action.clone();
        let installable = !installed && !self.prompt_agent_checking;
        let mouse = self
            .header_mouse_states
            .borrow_mut()
            .entry(format!("agent_rail_row_{agent_id}"))
            .or_default()
            .clone();
        let install_hover_key = format!("agent_rail_install_{agent_id}");
        let theme_owned = *theme;
        let name = definition.name.clone();
        let icon = definition_icon(definition, theme, 14.0, !installed);
        // While detection is running, unresolved rows show no install action:
        // offering to install something that may already be present is the
        // same false state as the collapsed "(not installed)" tooltip.
        let install: Box<dyn Element> = if !installable {
            Empty::new().finish()
        } else {
            let button = EventHandler::new(
                ConstrainedBox::new(
                    Align::new(render_icon(AppIcon::Download, theme_owned.accent, 14.0)).finish(),
                )
                .with_width(28.0)
                .with_height(28.0)
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, install_action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish();
            let hovered = self
                .header_mouse_states
                .borrow_mut()
                .entry(install_hover_key)
                .or_default()
                .lock()
                .ok()
                .is_some_and(|state| state.is_hovered());
            rail_tooltip(button, hovered, "Install".to_string(), theme, ui_font)
        };
        let row = Hoverable::new(mouse, move |state| {
            let hovered = state.is_hovered();
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(10.0)
                .with_child(icon)
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(name.clone(), ui_font, crate::theme::text_size(12.0))
                            .with_color(if hovered || installed {
                                theme_owned.main_text()
                            } else {
                                theme_owned.sub_text()
                            })
                            .soft_wrap(false)
                            .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(install)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            if installed {
                ctx.dispatch_typed_action_for(
                    view_id,
                    if should_show_account_picker(&agent_id) {
                        WorkspaceAction::OpenAgentAccountPicker { agent_id: agent_id.clone() }
                    } else {
                        WorkspaceAction::OpenAgentInNewWorkspaceTab { agent_id: agent_id.clone() }
                    },
                );
            } else if installable {
                ctx.dispatch_typed_action_for(view_id, row_install_action.clone());
            }
        });
        let content = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(3.0).with_child(row.finish());
        Container::new(content.finish())
            .with_padding_left(8.0)
            .with_padding_right(4.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(theme.surface_1())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
    }
}
