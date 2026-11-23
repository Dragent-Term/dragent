//! Prompt-first project screen for a selected project.
//!
//! When a project is selected, dragent shows this instead of the old
//! action-card launcher: title, context selectors, and a prompt composer.
//! Submitting creates a worktree-backed workspace, then opens a tab
//! running the chosen agent with the prompt.
//! Dropdown menus render as anchored popovers (see render_overlays).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use warpui::elements::{
    Align, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, EventHandler, Expanded, Flex, Image,
    MainAxisSize, ParentElement, SavePosition, ScrollbarWidth, Shrinkable, Text, Wrap,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::{EntityId, EventContext};
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::image_cache::CacheOption;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;

use super::action::{PromptAgent, PromptMenu, WorkspaceAction};
use super::project_links::{LinkKind, PromptLink};

/// Device chip label for the context bar: the selected local execution
/// backend (WSL distro when chosen, else "This Mac / Windows / Linux").
pub(crate) fn device_label() -> String {
    // A configured WSL backend wins everywhere so the chip names where local
    // agent runs actually execute.
    if let Some(distro) = crate::settings::AppSettings::global()
        .hosts
        .effective_wsl_distro()
    {
        return format!("WSL: {distro}");
    }
    #[cfg(target_os = "macos")]
    {
        "This Mac".to_string()
    }
    #[cfg(target_os = "windows")]
    {
        "This Windows".to_string()
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        "This Linux".to_string()
    }
}

pub(crate) struct ProjectPromptProps<'a> {
    pub workspace_view_id: EntityId,
    pub theme: &'a Theme,
    pub ui_font: FamilyId,
    pub mono_font: FamilyId,
    pub editor: &'a warpui::ViewHandle<super::InputEditor>,
    pub project_root: &'a PathBuf,
    pub project_name: String,
    pub agent: PromptAgent,
    /// The request goes to a team the agent leads; the chip's label.
    pub team_on: bool,
    pub lead_model: Option<String>,
    pub team_agents: Vec<PromptAgent>,
    pub prompt_empty: bool,
    pub busy: bool,
    pub attachment_names: Vec<String>,
    pub attachment_images: Vec<Option<PathBuf>>,
    pub linked_items: Vec<PromptLink>,
    /// The task being started, as its key and title.
    pub task: Option<(String, String)>,
    pub attachment_scroll: &'a ClippedScrollStateHandle,
    pub voice_ui: crate::voice::VoiceUiState,
    pub voice_spin: usize,
    pub notice: Option<&'a str>,
    pub current_branch: Option<String>,
    pub worktree_name: String,
    pub device_label: String,
    /// The project's machine while it isn't connected (remote projects).
    pub machine: Option<PromptMachine>,
    pub error: Option<&'a str>,
    pub available_width: f32,
    pub mouse_states: &'a RefCell<HashMap<String, MouseStateHandle>>,
}

/// A remote project's machine, as the project screen reports it.
pub(crate) struct PromptMachine {
    pub id: String,
    pub name: String,
    pub status: crate::settings::machines_store::MachineStatus,
    pub error: Option<String>,
    /// SSH works but Dragent's runtime there stopped or died.
    pub runtime_down: bool,
}

/// Why the project can't work right now, and the fix, above the composer.
fn machine_banner(
    props: &ProjectPromptProps<'_>,
    machine: &PromptMachine,
) -> Option<Box<dyn Element>> {
    use crate::machine_ui::{self as mui, Size, Tone, Variant};
    use crate::settings::machines_store::MachineStatus;
    let theme = props.theme;
    let ui = props.ui_font;
    let connect = |label: &str| {
        mui::button(
            theme,
            ui,
            props.workspace_view_id,
            mouse_state(props.mouse_states, "prompt_machine_connect"),
            Some(AppIcon::Plug),
            label,
            Variant::Secondary,
            Size::Xs,
            true,
            WorkspaceAction::Remote(super::remote_project::RemoteAction::Connect(
                machine.id.clone(),
            )),
        )
    };
    Some(match machine.status {
        MachineStatus::Connected => return None,
        MachineStatus::Connecting => mui::notice(
            theme,
            ui,
            Tone::Info,
            &format!("Connecting to {}…", machine.name),
            Some("This project's files, Git and terminals are on the machine.".into()),
            None,
        ),
        MachineStatus::Error if machine.runtime_down => mui::notice(
            theme,
            ui,
            Tone::Error,
            &format!("Dragent stopped running on {}", machine.name),
            machine.error.clone(),
            Some(connect("Restart")),
        ),
        MachineStatus::Error => mui::notice(
            theme,
            ui,
            Tone::Error,
            &format!("Couldn't reach {}", machine.name),
            machine.error.clone(),
            Some(connect("Try again")),
        ),
        MachineStatus::Idle => mui::notice(
            theme,
            ui,
            Tone::Neutral,
            &format!("{} isn't connected", machine.name),
            Some("Connect to work in this project. Its files stay on the machine.".into()),
            Some(connect("Connect")),
        ),
    })
}

fn mouse_state(
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: impl Into<String>,
) -> MouseStateHandle {
    states.borrow_mut().entry(key.into()).or_default().clone()
}

pub(crate) fn render(props: ProjectPromptProps<'_>) -> Box<dyn Element> {
    let theme = props.theme;
    let ui = props.ui_font;
    let id = props.workspace_view_id;

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(18.0)
        .with_child(title_block(theme, ui))
        .with_child(context_bar(
            theme,
            ui,
            id,
            &props.project_name,
            props.current_branch.as_deref(),
            &props.worktree_name,
            &props.device_label,
            props.mouse_states,
        ));
    if let Some(banner) = props
        .machine
        .as_ref()
        .and_then(|m| machine_banner(&props, m))
    {
        body = body.with_child(banner);
    }

    let mut composer = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.0);
    if !props.attachment_names.is_empty() || !props.linked_items.is_empty() || props.task.is_some()
    {
        composer = composer.with_child(attachment_strip(
            theme,
            props.mono_font,
            id,
            &props.attachment_names,
            &props.attachment_images,
            &props.linked_items,
            props.task.as_ref(),
            props.attachment_scroll,
            props.mouse_states,
        ));
    }
    body = body.with_child(
        composer
            .with_child(prompt_box(
                theme,
                props.mono_font,
                id,
                props.editor,
                props.agent,
                (props.team_on, &props.team_agents),
                props.lead_model.as_deref(),
                props.prompt_empty,
                props.busy,
                props.voice_ui,
                props.voice_spin,
                props.mouse_states,
            ))
            .finish(),
    );
    if let Some(notice) = props.notice {
        body = body.with_child(
            Text::new(notice.to_string(), ui, crate::theme::text_size(11.5))
                .with_color(theme.ui_red())
                .finish(),
        );
    }
    body = body.with_child(community_line(theme, ui, id, props.mouse_states));

    Align::new(
        ConstrainedBox::new(body.finish())
            .with_width(props.available_width.clamp(360.0, 680.0))
            .finish(),
    )
    .finish()
}

/// Star on GitHub, centred under the composer.
fn community_line(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    Container::new(
        Align::new(crate::community::render_star_button(
            theme,
            ui,
            crate::community::state(),
            false,
            mouse_state(states, "prompt-community-star"),
            move |ectx| ectx.dispatch_typed_action_for(id, super::WorkspaceAction::StarOnGitHub),
        ))
        .finish(),
    )
    .with_padding_top(10.0)
    .finish()
}

fn title_block(theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    let logo = ConstrainedBox::new(
        Image::new(
            AssetSource::Bundled {
                path: "icons/dragent-icon.png",
            },
            CacheOption::BySize,
        )
        .contain()
        .finish(),
    )
    .with_width(44.0)
    .with_height(44.0)
    .finish();
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(14.0)
        .with_child(Align::new(logo).finish())
        .with_child(
            Text::new(
                "What are we working on?".to_string(),
                ui,
                crate::theme::text_size(23.0),
            )
            .with_color(theme.main_text())
            .finish(),
        )
        .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt_box(
    theme: &Theme,
    mono: FamilyId,
    id: EntityId,
    editor: &warpui::ViewHandle<super::InputEditor>,
    agent: PromptAgent,
    (team_on, team_agents): (bool, &[PromptAgent]),
    lead_model: Option<&str>,
    prompt_empty: bool,
    busy: bool,
    voice_ui: crate::voice::VoiceUiState,
    voice_spin: usize,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let editor_view = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(
            Container::new(
                ConstrainedBox::new(
                    Align::new(render_icon(
                        AppIcon::ChevronRight,
                        theme.ansi_green(),
                        crate::theme::scaled(crate::theme::INPUT_FONT_SIZE),
                    ))
                    .finish(),
                )
                .with_width(crate::theme::scaled(crate::theme::INPUT_FONT_SIZE))
                .with_height(crate::console::input_layout::line_height_with_spacing(
                    crate::theme::scaled(crate::theme::INPUT_FONT_SIZE),
                ))
                .finish(),
            )
            .with_padding_right(4.0)
            .finish(),
        )
        .with_child(Expanded::new(1.0, ChildView::new(editor).finish()).finish())
        .finish();
    let editor_id = editor.id();
    let editor_view = EventHandler::new(
        Container::new(editor_view)
            .with_padding_bottom(20.0)
            .finish(),
    )
    .on_left_mouse_down(move |ctx, _app, position| {
        ctx.dispatch_typed_action_for(editor_id, super::InputEditorAction::ClickAt(position));
        DispatchEventResult::StopPropagation
    })
    .finish();

    let agent_chip = agent_chip(
        theme,
        mono,
        Some(agent),
        mouse_state(states, "prompt_agent_chip"),
        move |event_ctx| {
            event_ctx.dispatch_typed_action_for(
                id,
                WorkspaceAction::OpenPromptMenu {
                    menu: PromptMenu::Agent,
                },
            );
        },
    );
    let agent_chip = SavePosition::new(agent_chip, "prompt_agent_chip").finish();
    // With a team on, the agent chosen here leads it: a crown says so.
    // Drawn on the chip's corner, so it takes no room in the toolbar.
    let agent_chip: Box<dyn Element> = if team_on {
        use warpui::elements::{
            ChildAnchor, OffsetPositioning, ParentAnchor, ParentOffsetBounds, Stack,
        };
        let crown = Container::new(render_icon(
            AppIcon::Crown,
            crate::workspace::worktree_status::WORKING_AMBER,
            9.0,
        ))
        .with_background_color(theme.surface_1())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .with_padding_left(2.0)
        .with_padding_right(2.0)
        .with_padding_top(1.0)
        .with_padding_bottom(1.0)
        .finish();
        let mut stack = Stack::new().with_child(agent_chip);
        stack.add_positioned_overlay_child(
            crown,
            OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(-5.0, -6.0),
                ParentOffsetBounds::Unbounded,
                ParentAnchor::TopLeft,
                ChildAnchor::TopLeft,
            ),
        );
        stack.finish()
    } else {
        agent_chip
    };
    let team_chip = coordination_chip(
        theme,
        mono,
        id,
        team_on,
        busy,
        agent,
        team_agents,
        mouse_state(states, "prompt_team_chip"),
    );

    let submit = Hoverable::new(mouse_state(states, "prompt_submit"), move |state| {
        let accent = theme.accent;
        let bg = if prompt_empty && agent == PromptAgent::Dragent && !busy {
            warpui::color::ColorU::new(accent.r, accent.g, accent.b, 110)
        } else if state.is_hovered() {
            warpui::color::ColorU::new(
                accent.r.saturating_add(15),
                accent.g.saturating_add(15),
                accent.b.saturating_add(15),
                accent.a,
            )
        } else {
            accent
        };
        let glyph = if busy {
            // Match the terminal agent's filled square, keeping the idle
            // button's dimensions so the controls don't move on submission.
            ConstrainedBox::new(
                Align::new(
                    ConstrainedBox::new(
                        Container::new(warpui::elements::Empty::new().finish())
                            .with_background_color(theme.background)
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                            .finish(),
                    )
                    .with_width(12.0)
                    .with_height(12.0)
                    .finish(),
                )
                .finish(),
            )
            .with_width(16.0)
            .with_height(16.0)
            .finish()
        } else if team_on {
            // A team starts rather than sends: say so on the button.
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(
                    Text::new(
                        "Start team",
                        crate::fonts::ui_role(mono),
                        crate::theme::text_size(12.0),
                    )
                    .with_color(theme.background)
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
                )
                .with_child(render_icon(AppIcon::ArrowUp, theme.background, 13.0))
                .finish()
        } else {
            render_icon(AppIcon::ArrowUp, theme.background, 16.0)
        };
        Container::new(glyph)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(if team_on && !busy { 9.0 } else { 7.5 })
            .with_padding_right(if team_on && !busy { 8.0 } else { 7.5 })
            .with_padding_top(7.5)
            .with_padding_bottom(7.5)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        if (!prompt_empty || agent != PromptAgent::Dragent) && !busy {
            event_ctx.dispatch_typed_action_for(id, WorkspaceAction::SubmitPrompt);
        }
    })
    .finish();

    let submit = SavePosition::new(submit, "prompt_submit").finish();

    let upload = Hoverable::new(mouse_state(states, "prompt_upload"), move |state| {
        Container::new(render_icon(
            AppIcon::Plus,
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            15.0,
        ))
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_background_color(if state.is_hovered() {
            theme.surface_2()
        } else {
            warpui::color::ColorU::transparent_black()
        })
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(
            id,
            WorkspaceAction::OpenPromptMenu {
                menu: PromptMenu::Add,
            },
        );
    })
    .finish();
    let upload = SavePosition::new(upload, "prompt_add").finish();

    let microphone = if crate::voice::supported() {
        Some(
            Hoverable::new(mouse_state(states, "prompt_microphone"), move |state| {
                let color = match voice_ui {
                    crate::voice::VoiceUiState::Idle => {
                        if state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        }
                    }
                    crate::voice::VoiceUiState::Connecting => theme.accent,
                    crate::voice::VoiceUiState::Recording => theme.ui_red(),
                };
                let icon = if voice_ui == crate::voice::VoiceUiState::Connecting {
                    crate::icons::render_spinner(voice_spin, color, 15.0)
                } else {
                    render_icon(AppIcon::Microphone, color, 15.0)
                };
                Container::new(icon)
                    .with_padding_left(5.0)
                    .with_padding_right(5.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .with_background_color(if state.is_hovered() {
                        theme.surface_2()
                    } else {
                        warpui::color::ColorU::transparent_black()
                    })
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action_for(id, WorkspaceAction::TogglePromptVoiceInput);
            })
            .finish(),
        )
    } else {
        None
    };

    // Reserve the trailing actions before laying out selectors. Long names
    // ellipsize within a chip, and the chips wrap inside the remaining width.
    let mut selectors = Wrap::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_run_spacing(6.0)
        .with_child(upload)
        .with_child(agent_chip)
        .with_child(team_chip);
    {
        selectors = selectors.with_child(selector_chip(
            theme,
            mono,
            id,
            false,
            lead_model.unwrap_or(if agent == PromptAgent::Dragent {
                "Current model"
            } else {
                "Agent default"
            }),
            mouse_state(states, "prompt_lead_model"),
            PromptMenu::LeadModel,
            agent,
        ));
    }
    let mut footer = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::End)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(6.0)
        .with_child(Expanded::new(1.0, selectors.finish()).finish());
    if let Some(microphone) = microphone {
        footer = footer.with_child(microphone);
    }
    let footer = footer.with_child(submit).finish();

    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(editor_view)
        .with_child(footer);
    if busy {
        content = content.with_child(
            Text::new(
                if team_on {
                    "Starting the lead…"
                } else {
                    "Creating workspace…"
                }
                .to_string(),
                mono,
                crate::theme::text_size(10.5),
            )
            .with_color(theme.sub_text())
            .finish(),
        );
    }
    let content = content.finish();

    let content = Container::new(content)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(12.0)
        .finish();
    // While a team starts, a light laps the box's edge.
    let content: Box<dyn Element> = if busy && team_on {
        Box::new(super::workspace_activity::EdgeLap::new(
            content,
            theme.accent,
            12.0,
        ))
    } else {
        content
    };
    let boxed = super::composer_input::file_drop_target(content, id, |paths| {
        WorkspaceAction::AttachPromptFiles { paths }
    });
    if !team_on || busy {
        return boxed;
    }
    // One line under the box on what a team does with the request.
    // Counted as the team chip counts: the lead and each other agent once.
    let mut others: Vec<PromptAgent> = Vec::new();
    for helper in team_agents
        .iter()
        .copied()
        .filter(|h| *h != agent && super::team_prompt::can_work(*h))
    {
        if !others.contains(&helper) {
            others.push(helper);
        }
    }
    let workers = others.len() + 1;
    let line = format!(
        "{} leads and splits the work across {} agent{}. You're asked only when it needs you.",
        agent.label(),
        workers,
        if workers == 1 { "" } else { "s" },
    );
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(boxed)
        .with_child(
            Align::new(
                Text::new(
                    line,
                    crate::fonts::ui_role(mono),
                    crate::theme::text_size(11.5),
                )
                .with_color(theme.disabled_text())
                .finish(),
            )
            .finish(),
        )
        .finish()
}

/// A compact coordination control: only helper artwork, with a bounded stack.
fn coordination_chip(
    theme: &Theme,
    mono: FamilyId,
    id: EntityId,
    on: bool,
    busy: bool,
    lead: PromptAgent,
    helpers: &[PromptAgent],
    mouse: MouseStateHandle,
) -> Box<dyn Element> {
    let mut agents = Vec::new();
    for agent in helpers
        .iter()
        .copied()
        .filter(|agent| *agent != lead && super::team_prompt::can_work(*agent))
    {
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    let chip = Hoverable::new(mouse, move |state| {
        let color = if on {
            theme.accent
        } else if state.is_hovered() {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(render_icon(AppIcon::Layers, color, 14.0))
            .with_child(
                Text::new(
                    // On, it counts the team (the lead too); off, it offers one.
                    if on && !agents.is_empty() {
                        format!("{} agents", agents.len() + 1)
                    } else {
                        "Coordinate".to_string()
                    },
                    mono,
                    crate::theme::text_size(11.5),
                )
                .with_color(color)
                .finish(),
            );
        if on && !agents.is_empty() {
            use warpui::elements::{
                ChildAnchor, OffsetPositioning, ParentAnchor, ParentOffsetBounds, Stack,
            };
            let shown = agents.len().min(4);
            let mut stack = Stack::new();
            for (index, agent) in agents.iter().take(shown).enumerate() {
                let icon = Container::new(agent_image(*agent, theme, 16.0, 1.0))
                    .with_padding_left(3.0)
                    .with_padding_right(3.0)
                    .with_padding_top(3.0)
                    .with_padding_bottom(3.0)
                    .with_background_color(theme.surface_2())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(11.0)))
                    .finish();
                // While the team starts, each face glows in turn.
                let icon: Box<dyn Element> = if busy {
                    Box::new(super::workspace_activity::FacePulse::new(
                        icon,
                        index,
                        shown,
                        theme.accent,
                    ))
                } else {
                    icon
                };
                stack = stack.with_positioned_overlay_child(
                    icon,
                    OffsetPositioning::offset_from_parent(
                        warpui::geometry::vector::vec2f(index as f32 * 14.0, 0.0),
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::TopLeft,
                        ChildAnchor::TopLeft,
                    ),
                );
            }
            row = row.with_child(
                ConstrainedBox::new(stack.finish())
                    .with_width(22.0 + (shown - 1) as f32 * 14.0)
                    .with_height(22.0)
                    .finish(),
            );
            if agents.len() > shown {
                row = row.with_child(
                    Text::new(
                        format!("+{}", agents.len() - shown),
                        mono,
                        crate::theme::text_size(10.5),
                    )
                    .with_color(color)
                    .finish(),
                );
            }
        }
        // On, the faces say it opens; the arrow would only cost room.
        let row = if on && !agents.is_empty() {
            row
        } else {
            row.with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 12.0))
        };
        Container::new(row.finish())
            .with_background_color(if on || state.is_hovered() {
                warpui::color::ColorU::new(
                    theme.accent.r,
                    theme.accent.g,
                    theme.accent.b,
                    if on { 34 } else { 20 },
                )
            } else {
                warpui::color::ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(8.0)
            .with_padding_right(7.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(
            id,
            WorkspaceAction::OpenPromptMenu {
                menu: PromptMenu::Team,
            },
        );
    })
    .finish();
    SavePosition::new(chip, "prompt_team_chip").finish()
}

/// Whether the request goes to a team, and who's on it.
fn selector_chip(
    theme: &Theme,
    mono: FamilyId,
    id: EntityId,
    on: bool,
    label: &str,
    mouse: MouseStateHandle,
    menu: PromptMenu,
    agent: PromptAgent,
) -> Box<dyn Element> {
    let label = label.to_string();
    let chip = Hoverable::new(mouse, move |state| {
        let color = if on {
            theme.accent
        } else if state.is_hovered() {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(agent_image(agent, theme, 14.0, 1.0))
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(label.clone(), mono, crate::theme::text_size(11.5))
                            .with_color(color)
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 12.0))
                .finish(),
        )
        .with_background_color(if on || state.is_hovered() {
            warpui::color::ColorU::new(
                theme.accent.r,
                theme.accent.g,
                theme.accent.b,
                if on { 34 } else { 20 },
            )
        } else {
            warpui::color::ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(8.0)
        .with_padding_right(7.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(id, WorkspaceAction::OpenPromptMenu { menu });
    })
    .finish();
    SavePosition::new(
        ConstrainedBox::new(chip).with_width(180.0).finish(),
        if menu == PromptMenu::Team {
            "prompt_team_chip"
        } else {
            "prompt_lead_model"
        },
    )
    .finish()
}

#[allow(clippy::too_many_arguments)]
fn attachment_strip(
    theme: &Theme,
    mono: FamilyId,
    id: EntityId,
    attachment_names: &[String],
    attachment_images: &[Option<PathBuf>],
    linked_items: &[PromptLink],
    task: Option<&(String, String)>,
    scroll: &ClippedScrollStateHandle,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0);
    if let Some((key, title)) = task {
        row = row.with_child(attachment_chip(
            theme,
            mono,
            id,
            states,
            "prompt_task",
            AppIcon::TaskList,
            None,
            &format!("{key}  {title}"),
            WorkspaceAction::ClearPromptTask,
        ));
    }
    for (index, name) in attachment_names.iter().enumerate() {
        row = row.with_child(attachment_chip(
            theme,
            mono,
            id,
            states,
            &format!("prompt_attachment_{index}"),
            AppIcon::File,
            attachment_images.get(index).and_then(Option::as_ref),
            name,
            WorkspaceAction::RemovePromptAttachment { index },
        ));
    }
    for (index, link) in linked_items.iter().enumerate() {
        let icon = link_icon(link.kind);
        row = row.with_child(attachment_chip(
            theme,
            mono,
            id,
            states,
            &format!("prompt_link_{index}"),
            icon,
            None,
            &format!("{}  {}", link.identifier, link.title),
            WorkspaceAction::RemovePromptLink { index },
        ));
    }
    let clear = warpui::color::ColorU::transparent_black();
    let scroller = ClippedScrollable::horizontal(
        scroll.clone(),
        row.finish(),
        ScrollbarWidth::None,
        clear.into(),
        clear.into(),
        clear.into(),
    )
    .with_padding_start(0.0)
    .with_padding_end(0.0)
    .finish();
    Container::new(scroller)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .finish()
}

#[allow(clippy::too_many_arguments)]
fn attachment_chip(
    theme: &Theme,
    mono: FamilyId,
    id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: &str,
    icon: AppIcon,
    image_path: Option<&PathBuf>,
    label: &str,
    action: WorkspaceAction,
) -> Box<dyn Element> {
    let label = label.to_string();
    let image_path = image_path.map(|path| path.to_string_lossy().into_owned());
    Hoverable::new(mouse_state(states, key), move |state| {
        let leading: Box<dyn Element> = if let Some(path) = &image_path {
            ConstrainedBox::new(
                Image::new(
                    AssetSource::LocalFile { path: path.clone() },
                    CacheOption::Original,
                )
                .contain()
                .finish(),
            )
            .with_width(28.0)
            .with_height(28.0)
            .finish()
        } else {
            render_icon(icon, theme.sub_text(), 12.0)
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(leading)
                .with_child(
                    ConstrainedBox::new(
                        Text::new(label.clone(), mono, crate::theme::text_size(11.0))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .with_max_width(220.0)
                    .finish(),
                )
                .with_child(render_icon(AppIcon::XClose, theme.sub_text(), 11.0))
                .finish(),
        )
        .with_background_color(if state.is_hovered() {
            theme.surface_3()
        } else {
            theme.surface_2()
        })
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(id, action.clone());
    })
    .finish()
}

#[allow(clippy::too_many_arguments)]
fn context_bar(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    project_name: &str,
    current_branch: Option<&str>,
    worktree_name: &str,
    device_label: &str,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0);
    row = row.with_child(context_chip(
        theme,
        ui,
        id,
        states,
        "prompt_device",
        AppIcon::Monitor,
        device_label,
        PromptMenu::Device,
    ));
    row = row.with_child(context_chip(
        theme,
        ui,
        id,
        states,
        "prompt_project",
        AppIcon::FolderOpen,
        project_name,
        PromptMenu::Project,
    ));
    row = row.with_child(context_chip(
        theme,
        ui,
        id,
        states,
        "prompt_worktree",
        AppIcon::GitBranch,
        worktree_name,
        PromptMenu::Worktree,
    ));
    row = row.with_child(context_chip(
        theme,
        ui,
        id,
        states,
        "prompt_branch",
        AppIcon::GitBranch,
        current_branch.unwrap_or("no branch"),
        PromptMenu::Branch,
    ));
    Align::new(row.finish()).finish()
}

#[allow(clippy::too_many_arguments)]
fn context_chip(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: &str,
    icon: AppIcon,
    label: &str,
    menu: PromptMenu,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let key_owned = key.to_string();
    let trigger = Hoverable::new(mouse_state(states, key_owned.clone()), move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(render_icon(icon, theme.sub_text(), 12.0))
                .with_child(
                    Text::new(label_owned.clone(), ui, crate::theme::text_size(11.5))
                        .with_color(if state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        })
                        .finish(),
                )
                .with_child(render_icon(
                    AppIcon::ChevronDown,
                    theme.disabled_text(),
                    11.0,
                ))
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_background_color(if state.is_hovered() {
            theme.surface_2()
        } else {
            warpui::color::ColorU::transparent_black()
        })
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(id, WorkspaceAction::OpenPromptMenu { menu });
    })
    .finish();
    SavePosition::new(trigger, &key_owned).finish()
}

/// Anchored, height-limited popover for one prompt menu.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_prompt_menu(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui: FamilyId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    scroll: &ClippedScrollStateHandle,
    menu: PromptMenu,
    agent: PromptAgent,
    (team_on, team_agents, team_cost): (bool, &[PromptAgent], dragent_host::team::Cost),
    model_choices: &[(String, String)],
    lead_model: Option<&str>,
    model_panel: Option<crate::teams::models::ModelPanel<WorkspaceAction>>,
    installed_agents: &HashSet<PromptAgent>,
    agent_checking: bool,
    device_label: &str,
    device_remote_open: bool,
    machines: &[(String, String)],
    recent_projects: &[PathBuf],
    active_root: Option<&PathBuf>,
    selected_worktree: Option<&PathBuf>,
    branches: &[String],
    current_branch: Option<&str>,
    branch_search_editor: &warpui::ViewHandle<super::InputEditor>,
    branch_search: &str,
    agent_search_editor: &warpui::ViewHandle<super::InputEditor>,
    agent_search: &str,
    link_search_editor: &warpui::ViewHandle<super::InputEditor>,
    linked_items: &[PromptLink],
    link_results: &[PromptLink],
    link_loading: bool,
    link_error: Option<&str>,
    link_query_empty: bool,
    // Item kinds whose service is connected.
    connected: &[LinkKind],
) -> Box<dyn Element> {
    if menu == PromptMenu::LeadModel {
        return crate::teams::models::picker(
            theme,
            ui,
            workspace_view_id,
            states,
            scroll,
            agent_search_editor,
            agent_search,
            agent,
            lead_model,
            model_choices,
            |model| WorkspaceAction::SetPromptLeadModel { model },
            WorkspaceAction::ClosePromptMenus,
            WorkspaceAction::ClearPromptAgentSearch,
            model_panel,
        );
    }
    if menu == PromptMenu::Agent {
        return render_agent_menu(AgentMenuProps {
            theme,
            ui,
            states,
            scroll,
            search_editor: agent_search_editor,
            search: agent_search,
            selected: Some(agent),
            installed: installed_agents,
            checking: agent_checking,
            installed_only: false,
            with_dragent: true,
            pick: Rc::new(move |event_ctx, agent, installed| {
                event_ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    if installed {
                        WorkspaceAction::SelectPromptAgent { agent }
                    } else {
                        WorkspaceAction::OpenPromptAgentInstall { agent }
                    },
                );
            }),
            clear_search: Rc::new(move |event_ctx| {
                event_ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    WorkspaceAction::ClearPromptAgentSearch,
                );
            }),
        });
    }
    if menu == PromptMenu::Team {
        return render_team_menu(TeamMenuProps {
            theme,
            ui,
            view_id: workspace_view_id,
            states,
            scroll,
            search_editor: agent_search_editor,
            search: agent_search,
            lead: agent,
            on: team_on,
            helpers: team_agents,
            cost: team_cost,
            installed: installed_agents,
            checking: agent_checking,
        });
    }
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    match menu {
        PromptMenu::Add => {
            col = col.with_child(menu_row(
                workspace_view_id,
                theme,
                ui,
                states,
                "prompt_add_files",
                AppIcon::Asset("icons/paperclip.svg"),
                "Add attachments",
                false,
                WorkspaceAction::OpenPromptAttachmentPicker,
            ));
            col = col.with_child(menu_caption(theme, ui, "LINK WORK"));
            // Default source first (Tasks settings), then the rest.
            let mut link_kinds: Vec<LinkKind> = LinkKind::items()
                .filter(|kind| connected.contains(kind))
                .collect();
            {
                let order = crate::settings::settings_tasks_hosts::ordered_visible_providers();
                link_kinds.sort_by_key(|kind| {
                    order
                        .iter()
                        .position(|k| k.as_str() == kind.provider().key())
                        .unwrap_or(usize::MAX)
                });
            }
            let kinds = std::iter::once(LinkKind::PullRequest).chain(link_kinds);
            for kind in kinds {
                col = col.with_child(entry_row(
                    workspace_view_id,
                    theme,
                    ui,
                    states,
                    &format!("prompt_add_{kind:?}"),
                    link_mark(theme, ui, kind, 14.0),
                    &kind.label(),
                    None,
                    false,
                    WorkspaceAction::OpenPromptLinkKind { kind },
                ));
            }
            col = col.with_child(menu_row(
                workspace_view_id,
                theme,
                ui,
                states,
                "prompt_add_connect",
                AppIcon::Plug,
                if connected.len() > 1 {
                    "Connect more…"
                } else {
                    "Connect Linear, Jira, Slack…"
                },
                false,
                WorkspaceAction::OpenSettingsSection(
                    crate::settings::SettingsSection::Integrations,
                ),
            ));
        }
        PromptMenu::Links(kind) => {
            let searching_needs_words = matches!(kind, LinkKind::Item(provider) if crate::integrations::items::needs_query(provider));
            if kind != LinkKind::PullRequest && !connected.contains(&kind) {
                col = col.with_child(menu_caption(
                    theme,
                    ui,
                    &format!(
                        "Connect {} in Settings → Integrations to search it.",
                        kind.provider().label()
                    ),
                ));
                col = col.with_child(menu_row(
                    workspace_view_id,
                    theme,
                    ui,
                    states,
                    "prompt_links_connect",
                    AppIcon::Plug,
                    "Open Integrations",
                    false,
                    WorkspaceAction::OpenSettingsSection(
                        crate::settings::SettingsSection::Integrations,
                    ),
                ));
            } else if link_loading {
                col = col.with_child(menu_caption(theme, ui, "Searching…"));
            } else if link_results.is_empty() && link_error.is_none() {
                col = col.with_child(menu_caption(
                    theme,
                    ui,
                    if searching_needs_words && link_query_empty {
                        "Type to search."
                    } else {
                        "Nothing matches."
                    },
                ));
            } else {
                for (index, link) in link_results.iter().enumerate() {
                    let subtitle = [
                        Some(link.identifier.as_str()).filter(|key| !key.is_empty()),
                        link.detail.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    col = col.with_child(entry_row(
                        workspace_view_id,
                        theme,
                        ui,
                        states,
                        &format!("prompt_link_result_{index}"),
                        link_mark(theme, ui, kind, 14.0),
                        &link.title,
                        Some(subtitle).filter(|subtitle| !subtitle.is_empty()),
                        linked_items.iter().any(|item| item.url == link.url),
                        WorkspaceAction::SelectPromptLink { index },
                    ));
                }
            }
            if let Some(error) = link_error {
                col = col.with_child(menu_caption(theme, ui, error));
            }
        }
        // Rendered by `render_agent_menu` and `render_team_menu` above.
        PromptMenu::Agent | PromptMenu::Team | PromptMenu::LeadModel => {}
        PromptMenu::Device => {
            // A remote project runs on its machine; the chip names it.
            let remote = active_root.and_then(|root| crate::remote_fs::resolve(root));
            let this_device = self::device_label();
            col = col.with_child(menu_caption(theme, ui, "Run on"));
            col = col.with_child(menu_row(
                workspace_view_id,
                theme,
                ui,
                states,
                "prompt_device_this",
                AppIcon::Monitor,
                &this_device,
                remote.is_none(),
                WorkspaceAction::ClosePromptMenus,
            ));
            let _ = device_label;
            col = col.with_child(menu_row(
                workspace_view_id,
                theme,
                ui,
                states,
                "prompt_device_remote",
                AppIcon::Server,
                "Remote machine",
                false,
                WorkspaceAction::TogglePromptRemote,
            ));
            if device_remote_open || remote.is_some() {
                if machines.is_empty() {
                    col = col.with_child(menu_caption(theme, ui, "No remote machines yet."));
                }
                // Choosing a machine adds (or opens) a project on it.
                for (id, name) in machines {
                    col = col.with_child(menu_row(
                        workspace_view_id,
                        theme,
                        ui,
                        states,
                        &format!("prompt_device_machine_{id}"),
                        AppIcon::Server,
                        name,
                        remote.as_ref().is_some_and(|r| &r.machine_id == id),
                        WorkspaceAction::AddProject(super::add_project::AddProjectAction::Open {
                            machine: Some(id.clone()),
                        }),
                    ));
                }
                col = col.with_child(menu_row(
                    workspace_view_id,
                    theme,
                    ui,
                    states,
                    "prompt_device_ssh",
                    AppIcon::Settings,
                    if machines.is_empty() {
                        "Add a machine…"
                    } else {
                        "Add or manage machines…"
                    },
                    false,
                    if machines.is_empty() {
                        WorkspaceAction::ConnectPromptSsh
                    } else {
                        WorkspaceAction::ShowMachines
                    },
                ));
            }
        }
        PromptMenu::Project => {
            col = col.with_child(menu_caption(theme, ui, "Recent projects"));
            for (index, path) in recent_projects.iter().enumerate() {
                let label = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Project");
                col = col.with_child(menu_row(
                    workspace_view_id,
                    theme,
                    ui,
                    states,
                    &format!("prompt_project_{index}"),
                    AppIcon::FolderOpen,
                    label,
                    active_root.is_some_and(|root| root == path),
                    WorkspaceAction::OpenRecentProject { root: path.clone() },
                ));
            }
            col = col.with_child(menu_caption(theme, ui, "Add project"));
            col = col.with_child(menu_row(
                workspace_view_id,
                theme,
                ui,
                states,
                "prompt_project_open",
                AppIcon::FolderOpen,
                "Open project",
                false,
                WorkspaceAction::OpenFolder,
            ));
            col = col.with_child(menu_row(
                workspace_view_id,
                theme,
                ui,
                states,
                "prompt_project_create",
                AppIcon::Plus,
                "Create new project",
                false,
                WorkspaceAction::OpenCreateProject,
            ));
            col = col.with_child(menu_row(
                workspace_view_id,
                theme,
                ui,
                states,
                "prompt_project_clone",
                AppIcon::GitBranch,
                "Clone repository",
                false,
                WorkspaceAction::ShowAddProjectCloneForm,
            ));
        }
        PromptMenu::Worktree => {
            // Git branches a worktree from a commit; a new repository has none.
            let worktree_available =
                active_root.is_some_and(|root| super::project_naming::worktree_available(root));
            if active_root.is_some() {
                let label = format!("Local · {}", current_branch.unwrap_or("current checkout"));
                col = col.with_child(menu_row(
                    workspace_view_id,
                    theme,
                    ui,
                    states,
                    "prompt_worktree_current",
                    AppIcon::GitBranch,
                    &label,
                    !worktree_available
                        || selected_worktree.is_some_and(|path| Some(path) == active_root),
                    WorkspaceAction::SelectPromptCurrentCheckout,
                ));
            }
            if worktree_available {
                col = col.with_child(menu_row(
                    workspace_view_id,
                    theme,
                    ui,
                    states,
                    "prompt_worktree_new",
                    AppIcon::Plus,
                    "Worktree · new branch and checkout",
                    selected_worktree.is_none(),
                    WorkspaceAction::OpenNewWorktreeFromPrompt,
                ));
            } else {
                col = col.with_child(
                    Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(render_icon(AppIcon::Plus, theme.disabled_text(), 13.0))
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    Text::new(
                                        "Worktree · available after the first commit",
                                        ui,
                                        crate::theme::text_size(12.5),
                                    )
                                    .with_color(theme.disabled_text())
                                    .soft_wrap(false)
                                    .with_clip(ClipConfig::ellipsis())
                                    .finish(),
                                )
                                .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .finish(),
                );
            }
        }
        PromptMenu::Branch => {
            return render_branch_menu(BranchMenuProps {
                theme,
                ui,
                view_id: workspace_view_id,
                states,
                scroll,
                search_editor: branch_search_editor,
                search: branch_search,
                branches,
                selected: current_branch,
                title: "Base branch for new worktree",
                width: 250.0,
                loading: false,
                default_label: None,
                pick: Rc::new(|branch| WorkspaceAction::CreatePromptWorktreeFromBranch { branch }),
            });
        }
    }
    let list = menu_list(theme, scroll, col.finish(), 280.0);
    let mut contents = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    if let PromptMenu::Links(kind) = menu {
        contents = contents.with_child(links_header(workspace_view_id, theme, ui, states, kind));
        if kind == LinkKind::PullRequest || connected.contains(&kind) {
            let search = Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(AppIcon::Search, theme.sub_text(), 13.0))
                    .with_child(
                        Expanded::new(1.0, ChildView::new(link_search_editor).finish()).finish(),
                    )
                    .finish(),
            )
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish();
            contents = contents.with_child(search);
        }
    }

    let contents = contents.with_child(list).finish();
    menu_card(
        theme,
        contents,
        match menu {
            PromptMenu::Links(_) => 360.0,
            _ => 250.0,
        },
    )
}

/// Shared searchable branch menu for workspace creation and automations.
pub(crate) struct BranchMenuProps<'a, A> {
    pub theme: &'a Theme,
    pub ui: FamilyId,
    pub view_id: EntityId,
    pub states: &'a RefCell<HashMap<String, MouseStateHandle>>,
    pub scroll: &'a ClippedScrollStateHandle,
    pub search_editor: &'a warpui::ViewHandle<super::InputEditor>,
    pub search: &'a str,
    pub branches: &'a [String],
    pub selected: Option<&'a str>,
    pub title: &'a str,
    pub width: f32,
    pub loading: bool,
    /// An empty selection follows the project's current branch at run time.
    pub default_label: Option<&'a str>,
    pub pick: Rc<dyn Fn(String) -> A>,
}

pub(crate) fn render_branch_menu<A: warpui::Action + Clone + 'static>(
    props: BranchMenuProps<'_, A>,
) -> Box<dyn Element> {
    let BranchMenuProps {
        theme,
        ui,
        view_id,
        states,
        scroll,
        search_editor,
        search,
        branches,
        selected,
        title,
        width,
        loading,
        default_label,
        pick,
    } = props;
    let query = search.trim().to_lowercase();
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(1.0);
    if let Some(label) = default_label {
        rows = rows.with_child(menu_row(
            view_id,
            theme,
            ui,
            states,
            "branch_default",
            AppIcon::GitBranch,
            label,
            selected.is_none(),
            pick(String::new()),
        ));
    }
    let mut matches = 0;
    for (index, branch) in branches.iter().enumerate() {
        if !branch.to_lowercase().contains(&query) {
            continue;
        }
        matches += 1;
        rows = rows.with_child(menu_row(
            view_id,
            theme,
            ui,
            states,
            &format!("branch_choice_{index}"),
            AppIcon::GitBranch,
            branch,
            selected == Some(branch.as_str()),
            pick(branch.clone()),
        ));
    }
    if matches == 0 {
        rows = rows.with_child(menu_caption(
            theme,
            ui,
            if loading {
                "Loading branches…"
            } else {
                "No matching branches"
            },
        ));
    }
    let search = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(render_icon(AppIcon::Search, theme.sub_text(), 13.0))
            .with_child(Expanded::new(1.0, ChildView::new(search_editor).finish()).finish())
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .finish();
    let mut contents = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(menu_caption(theme, ui, title))
        .with_child(search);
    if matches > 7 {
        contents = contents.with_child(menu_caption(
            theme,
            ui,
            &format!("Scroll for more branches ↓  ·  {matches} found"),
        ));
    }
    menu_card(
        theme,
        contents
            .with_child(menu_list(theme, scroll, rows.finish(), 280.0))
            .finish(),
        width,
    )
}

/// The scrolling, height-limited list inside a prompt menu.
pub(crate) fn menu_list(
    theme: &Theme,
    scroll: &ClippedScrollStateHandle,
    rows: Box<dyn Element>,
    max_height: f32,
) -> Box<dyn Element> {
    ConstrainedBox::new(
        ClippedScrollable::vertical(
            scroll.clone(),
            rows,
            ScrollbarWidth::Auto,
            theme.outline().into(),
            theme.sub_text().into(),
            theme.surface_1().into(),
        )
        .with_overlayed_scrollbar()
        .finish(),
    )
    .with_max_height(max_height)
    .finish()
}

/// The popover card every prompt menu sits in.
pub(crate) fn menu_card(theme: &Theme, contents: Box<dyn Element>, width: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(contents)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish(),
    )
    .with_width(width)
    .finish()
}

/// What a click in the agent menu does: choose the agent when it is
/// installed (`true`), otherwise offer to install it.
pub(crate) type AgentPick = Rc<dyn Fn(&mut EventContext, PromptAgent, bool)>;

/// Everything the agent menu needs. The prompt screen and the automation
/// form both use it, so they look and behave the same.
pub(crate) struct AgentMenuProps<'a> {
    pub theme: &'a Theme,
    pub ui: FamilyId,
    pub states: &'a RefCell<HashMap<String, MouseStateHandle>>,
    pub scroll: &'a ClippedScrollStateHandle,
    pub search_editor: &'a warpui::ViewHandle<super::InputEditor>,
    pub search: &'a str,
    pub selected: Option<PromptAgent>,
    pub installed: &'a HashSet<PromptAgent>,
    /// Detection is still running; missing agents may yet turn up.
    pub checking: bool,
    /// List only the installed agents, without the catalog to install from.
    pub installed_only: bool,
    /// Offer Dragent's own agent too (it is always installed).
    pub with_dragent: bool,
    pub pick: AgentPick,
    pub clear_search: Rc<dyn Fn(&mut EventContext)>,
}

/// The agent menu: a search field over the installed agents (Dragent is
/// always there), then the catalog the user can install.
pub(crate) fn render_agent_menu(props: AgentMenuProps<'_>) -> Box<dyn Element> {
    let AgentMenuProps {
        theme,
        ui,
        states,
        scroll,
        search_editor,
        search,
        selected,
        installed,
        checking,
        installed_only,
        with_dragent,
        pick,
        clear_search,
    } = props;
    let query = search.trim().to_lowercase();
    let matches_query =
        |listed: PromptAgent| query.is_empty() || listed.label().to_lowercase().contains(&query);
    // Disabled agents (Settings → Agent → General) stay out of the menu.
    let autonomy = crate::settings::AppSettings::global().agent.clone();
    let enabled = |listed: PromptAgent| !autonomy.is_agent_disabled(listed.command_key());
    let installed_rows: Vec<PromptAgent> = PromptAgent::ALL
        .into_iter()
        .filter(|listed| {
            (if *listed == PromptAgent::Dragent {
                with_dragent
            } else {
                installed.contains(listed)
            }) && enabled(*listed)
                && matches_query(*listed)
        })
        .collect();
    let missing_rows: Vec<PromptAgent> = PromptAgent::ALL
        .into_iter()
        .filter(|listed| {
            !installed_only
                && *listed != PromptAgent::Dragent
                && !installed.contains(listed)
                && enabled(*listed)
                && matches_query(*listed)
        })
        .collect();
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    if installed_rows.is_empty() && missing_rows.is_empty() {
        col = col.with_child(menu_caption(theme, ui, "No matching agents"));
    } else {
        if !installed_rows.is_empty() {
            col = col.with_child(menu_caption(theme, ui, "INSTALLED"));
            for listed in installed_rows {
                col = col.with_child(agent_menu_row(
                    theme,
                    ui,
                    states,
                    &format!("prompt_agent_{listed:?}"),
                    listed,
                    Some(listed) == selected,
                    true,
                    None,
                    pick.clone(),
                ));
            }
        }
        if !missing_rows.is_empty() {
            col = col.with_child(menu_caption(
                theme,
                ui,
                if checking {
                    "CHECKING AGENTS"
                } else {
                    "NOT INSTALLED"
                },
            ));
            for listed in missing_rows {
                col = col.with_child(agent_menu_row(
                    theme,
                    ui,
                    states,
                    &format!("prompt_agent_{listed:?}"),
                    listed,
                    false,
                    false,
                    None,
                    pick.clone(),
                ));
            }
        }
    }

    let search = agent_search_field(theme, states, search_editor, search, clear_search);
    let contents = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(search)
        .with_child(menu_list(theme, scroll, col.finish(), 280.0))
        .finish();
    menu_card(theme, contents, 250.0)
}

/// The search field the agent menus open with. Settings-sidebar style:
/// search icon + centered editor with an inline clear button.
/// `shared_input` keeps the caret and the text on one vertical midpoint.
pub(crate) fn agent_search_field(
    theme: &Theme,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    search_editor: &warpui::ViewHandle<super::InputEditor>,
    search: &str,
    clear_search: Rc<dyn Fn(&mut EventContext)>,
) -> Box<dyn Element> {
    let theme = *theme;
    let trailing: Option<Box<dyn Element>> = if !search.is_empty() {
        Some(Box::new(
            Hoverable::new(
                mouse_state(states, "prompt_agent_search_clear"),
                move |state| {
                    let icon_color = if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.disabled_text()
                    };
                    Container::new(render_icon(AppIcon::XClose, icon_color, 11.0))
                        .with_padding_left(4.0)
                        .with_padding_right(2.0)
                        .finish()
                },
            )
            .on_click(move |ctx, _, _| clear_search(ctx)),
        ))
    } else {
        None
    };
    crate::shared_input::shared_input(
        &theme,
        search_editor,
        crate::shared_input::SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: AppIcon::Search,
                size: 12.0,
            }),
            trailing,
            width: crate::shared_input::SharedInputWidth::Fill,
            ..Default::default()
        },
    )
}

/// Everything the Coordinate menu needs.
pub(crate) struct TeamMenuProps<'a> {
    pub theme: &'a Theme,
    pub ui: FamilyId,
    pub view_id: EntityId,
    pub states: &'a RefCell<HashMap<String, MouseStateHandle>>,
    pub scroll: &'a ClippedScrollStateHandle,
    pub search_editor: &'a warpui::ViewHandle<super::InputEditor>,
    pub search: &'a str,
    /// The agent picked on the screen: it leads.
    pub lead: PromptAgent,
    pub on: bool,
    pub helpers: &'a [PromptAgent],
    pub cost: dragent_host::team::Cost,
    pub installed: &'a HashSet<PromptAgent>,
    pub checking: bool,
}

/// The Coordinate menu, built like the agent menu: a search field, then the
/// switch, the agents the lead may give work to (with their marks), and how
/// to weigh models.
pub(crate) fn render_team_menu(props: TeamMenuProps<'_>) -> Box<dyn Element> {
    use dragent_host::team::Cost;
    let TeamMenuProps {
        theme,
        ui,
        view_id,
        states,
        scroll,
        search_editor,
        search,
        lead,
        on,
        helpers,
        cost,
        installed,
        checking,
    } = props;
    let query = search.trim().to_lowercase();
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0)
        .with_child(menu_row(
            view_id,
            theme,
            ui,
            states,
            "prompt_team_on",
            AppIcon::Layers,
            "Coordinate agents",
            on,
            WorkspaceAction::TogglePromptTeam,
        ));
    // The lead first: it's always among them.
    let mut workers: Vec<PromptAgent> = PromptAgent::ALL
        .into_iter()
        .filter(|candidate| super::team_prompt::can_work(*candidate))
        .filter(|candidate| *candidate == lead || installed.contains(candidate))
        .filter(|candidate| query.is_empty() || candidate.label().to_lowercase().contains(&query))
        .collect();
    workers.sort_by_key(|candidate| *candidate != lead);
    col = col.with_child(menu_caption(theme, ui, "AGENTS IT CAN USE"));
    if workers.is_empty() {
        col = col.with_child(menu_caption(
            theme,
            ui,
            if checking {
                "Looking for agents on this computer…"
            } else {
                "No matching agents"
            },
        ));
    }
    let pick: AgentPick = Rc::new(move |event_ctx, agent, _| {
        event_ctx.dispatch_typed_action_for(
            view_id,
            // The lead is always one of them.
            if agent == lead {
                WorkspaceAction::ClosePromptMenus
            } else {
                WorkspaceAction::TogglePromptTeamAgent { agent }
            },
        );
    });
    for candidate in workers {
        let leads = candidate == lead;
        let chosen = leads || helpers.contains(&candidate);
        // A box to tick before each agent; the lead's is always ticked.
        let icon = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(9.0)
            .with_child(team_checkbox(theme, chosen, !leads))
            .with_child(agent_image(candidate, theme, 16.0, 1.0))
            .finish();
        let pick = pick.clone();
        col = col.with_child(picker_agent_row(
            theme,
            ui,
            mouse_state(
                states,
                &format!("prompt_team_agent_{}", candidate.command_key()),
            ),
            icon,
            candidate.label().to_string(),
            None,
            leads.then_some("leads"),
            true,
            false,
            false,
            Rc::new(move |event_ctx| pick(event_ctx, candidate, true)),
        ));
    }
    if query.is_empty() {
        col = col.with_child(menu_caption(theme, ui, "MODELS"));
        for (choice, key, icon, text) in [
            (
                Cost::Cheapest,
                "cheapest",
                AppIcon::Bolt,
                "Cheapest that can do it",
            ),
            (Cost::Balanced, "balanced", AppIcon::Sliders, "Balanced"),
            (Cost::Best, "best", AppIcon::Sparkle, "Strongest"),
        ] {
            col = col.with_child(menu_row(
                view_id,
                theme,
                ui,
                states,
                &format!("prompt_team_cost_{key}"),
                icon,
                text,
                cost == choice,
                WorkspaceAction::SetPromptTeamCost { cost: choice },
            ));
        }
    }
    let clear_search: Rc<dyn Fn(&mut EventContext)> = Rc::new(move |event_ctx| {
        event_ctx.dispatch_typed_action_for(view_id, WorkspaceAction::ClearPromptAgentSearch);
    });
    let contents = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(agent_search_field(
            theme,
            states,
            search_editor,
            search,
            clear_search,
        ))
        .with_child(menu_list(theme, scroll, col.finish(), 280.0))
        .finish();
    menu_card(theme, contents, 250.0)
}

/// The chip that shows the chosen agent and opens the agent menu. `None`
/// is an agent this catalog does not know.
pub(crate) fn agent_chip(
    theme: &Theme,
    mono: FamilyId,
    agent: Option<PromptAgent>,
    mouse: MouseStateHandle,
    on_click: impl Fn(&mut EventContext) + 'static,
) -> Box<dyn Element> {
    let icon = match agent {
        Some(agent) => agent_image(agent, theme, 16.0, 1.0),
        None => render_icon(AppIcon::Agent, theme.sub_text(), 16.0),
    };
    let label = agent.map_or("Choose an agent", PromptAgent::label);
    labelled_agent_chip(theme, mono, icon, label.to_string(), mouse, on_click)
}

/// The agent chip for a session or agent from [`render_agent_target_menu`].
pub(crate) fn agent_target_chip(
    theme: &Theme,
    mono: FamilyId,
    target: Option<&AgentTarget>,
    mouse: MouseStateHandle,
    on_click: impl Fn(&mut EventContext) + 'static,
) -> Box<dyn Element> {
    let label = target.map_or_else(|| "Choose an agent".to_string(), AgentTarget::chip_label);
    labelled_agent_chip(
        theme,
        mono,
        agent_target_image(target, theme, 16.0),
        label,
        mouse,
        on_click,
    )
}

pub(crate) fn labelled_agent_chip(
    theme: &Theme,
    mono: FamilyId,
    icon: Box<dyn Element>,
    label: String,
    mouse: MouseStateHandle,
    on_click: impl Fn(&mut EventContext) + 'static,
) -> Box<dyn Element> {
    let theme = *theme;
    // The chip can sit in rows with no width limit, so it can't shrink its
    // label; a long session title is cut short instead.
    let label = if label.chars().count() > 28 {
        format!("{}…", label.chars().take(27).collect::<String>())
    } else {
        label
    };
    Hoverable::new(mouse, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(icon)
                .with_child(
                    Text::new(label, mono, crate::theme::text_size(11.5))
                        .with_color(theme.accent)
                        .finish(),
                )
                .with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 12.0))
                .finish(),
        )
        .with_background_color(if state.is_hovered() {
            warpui::color::ColorU::new(theme.accent.r, theme.accent.g, theme.accent.b, 46)
        } else {
            theme.surface_2()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(9.0)
        .with_padding_right(7.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| on_click(event_ctx))
    .finish()
}

/// Centered install sheet for an agent that is visible in the catalog but is
/// not currently available on PATH. The terminal is the same embedded terminal
/// used by the GitHub CLI sign-in flow.
#[allow(clippy::too_many_arguments)]
#[cfg(not(target_family = "wasm"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_review_setup(
    id: EntityId,
    theme: &Theme,
    ui: FamilyId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    review: &crate::pull_requests::ReviewSetup,
    editor: &warpui::ViewHandle<super::InputEditor>,
    agent: PromptAgent,
    model: &str,
    busy: bool,
    error: Option<&str>,
) -> Box<dyn Element> {
    use crate::automations::widgets::{button, field, label, ButtonKind};
    use warpui::elements::Empty;
    let reference = format!("{}{}", if review.gitlab { "!" } else { "#" }, review.number);
    let machine = crate::remote_fs::resolve(&review.target.root)
        .map(|m| format!("SSH · {}", m.machine_id))
        .unwrap_or_else(|| "This computer".into());
    let choose_agent = SavePosition::new(
        agent_chip(
            theme,
            ui,
            Some(agent),
            mouse_state(states, "review_agent"),
            move |c| {
                c.dispatch_typed_action_for(
                    id,
                    WorkspaceAction::OpenPromptMenu {
                        menu: PromptMenu::Agent,
                    },
                );
            },
        ),
        "prompt_agent_chip",
    )
    .finish();
    let choose_model = selector_chip(
        theme,
        ui,
        id,
        false,
        model,
        mouse_state(states, "review_model"),
        PromptMenu::LeadModel,
        agent,
    );
    let choices: Box<dyn Element> = if busy {
        label(
            ui,
            &format!("{} · {model}", agent.label()),
            13.0,
            theme.sub_text(),
        )
    } else {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(choose_agent)
            .with_child(choose_model)
            .finish()
    };
    let mut body=Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(12.0)
        .with_child(Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(8.0)
            .with_child(Expanded::new(1.0,crate::machine_ui::semibold(ui,if review.launch_agent{"Review with agent"}else{"Open review workspace"},20.0,theme.main_text())).finish())
            .with_child(button(theme,ui,id,mouse_state(states,"review_provider_link"),Some(AppIcon::Asset("icons/external-link.svg")),
                if review.gitlab{"Open merge request"}else{"Open pull request"},ButtonKind::Ghost,
                Some(WorkspaceAction::OpenExternalUrl{url:review.url.clone()}))).finish())
        .with_child(crate::machine_ui::semibold(ui,&format!("{reference} · {}",review.title),14.0,theme.main_text()))
        .with_child(label(ui,&format!("{}  →  {}",review.source,review.base),12.0,theme.sub_text()))
        .with_child(label(ui,&format!("{} · {machine}",review.target.label),12.0,theme.sub_text()))
        .with_child(label(ui,&format!("Pinned commit: {}",review.head),11.0,theme.sub_text()))
        .with_child(label(ui,if review.launch_agent {
            "An isolated review worktree keeps your current checkout untouched. Findings stay local until you choose to publish them."
        } else {
            "Open an existing review snapshot or create an isolated checkout at this commit. Your current checkout stays untouched. No agent will be started."
        },12.0,theme.sub_text()));
    if review.launch_agent {
        body = body
            .with_child(label(ui, "Review instructions", 12.0, theme.main_text()))
            .with_child(field(theme, editor, 160.0))
            .with_child(choices);
    }
    body=body.with_child(label(ui,if review.launch_agent {
        "A clean snapshot may be reused. New commits get a separate snapshot; previous work is preserved."
    } else {
        "Existing local edits are preserved. A newer commit gets a separate snapshot. Nothing is fetched or created until you confirm."
    },11.0,theme.sub_text()));
    if let Some(error) = error {
        body = body.with_child(label(ui, error, 12.0, theme.ui_red()));
    }
    body = body.with_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(button(
                theme,
                ui,
                id,
                mouse_state(states, "cancel_review"),
                None,
                "Cancel",
                ButtonKind::Secondary,
                (!busy).then_some(WorkspaceAction::CancelReviewSetup),
            ))
            .with_child(button(
                theme,
                ui,
                id,
                mouse_state(states, "start_review"),
                Some(if review.launch_agent {
                    AppIcon::Agent
                } else {
                    AppIcon::FolderGit
                }),
                if busy {
                    "Preparing workspace…"
                } else if review.launch_agent {
                    "Start review"
                } else {
                    "Open workspace"
                },
                ButtonKind::Primary,
                (!busy).then_some(WorkspaceAction::SubmitPrompt),
            ))
            .finish(),
    );
    let modal = Container::new(
        ConstrainedBox::new(body.finish())
            .with_width(680.0)
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_uniform_padding(24.0)
    .finish();
    EventHandler::new(modal)
        .on_keydown(move |ctx, _, key| {
            let action = if key.key == "escape" {
                Some(WorkspaceAction::CancelReviewSetup)
            } else if key.key == "enter" && (key.cmd || key.ctrl) {
                Some(WorkspaceAction::SubmitPrompt)
            } else {
                None
            };
            if let Some(action) = action {
                ctx.dispatch_typed_action_for(id, action);
                DispatchEventResult::StopPropagation
            } else {
                DispatchEventResult::PropagateToParent
            }
        })
        .finish()
}

pub(crate) fn render_embedded_terminal_modal(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui: FamilyId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    heading: &str,
    subject: &str,
    description: &str,
    terminal: Option<Box<dyn Element>>,
    checking: bool,
    error: Option<&str>,
    retry_elevated: bool,
    show_verification: bool,
) -> Box<dyn Element> {
    let heading = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(5.0)
        .with_child(
            Text::new(heading.to_string(), ui, crate::theme::text_size(20.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(description.to_string(), ui, crate::theme::text_size(12.0))
                .with_color(theme.sub_text())
                .finish(),
        );

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(14.0)
        .with_child(Box::new(heading));
    if let Some(terminal) = terminal {
        body = body.with_child(
            Container::new(ConstrainedBox::new(terminal).with_height(300.0).finish())
                .with_background_color(theme.solid_bg())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
                .finish(),
        );
    } else if error.is_none() {
        body = body.with_child(
            Text::new(
                format!("No automatic installer is available for {subject}."),
                ui,
                crate::theme::text_size(12.0),
            )
            .with_color(theme.ansi(3))
            .finish(),
        );
    }
    if error.is_none() && checking {
        body = body.with_child(
            Text::new(
                format!("Checking {subject}…"),
                ui,
                crate::theme::text_size(12.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        );
    }

    #[cfg(not(target_family = "wasm"))]
    let control = |key: &str, icon, title: &str, action, width| {
        ConstrainedBox::new(crate::automations::widgets::button(
            theme,
            ui,
            workspace_view_id,
            mouse_state(states, key),
            Some(icon),
            title,
            crate::automations::widgets::ButtonKind::Secondary,
            Some(action),
        ))
        .with_width(width)
        .finish()
    };
    #[cfg(target_family = "wasm")]
    let control = |key: &str, icon, title: &str, action, width| {
        ConstrainedBox::new(menu_row(
            workspace_view_id,
            theme,
            ui,
            states,
            key,
            icon,
            title,
            false,
            action,
        ))
        .with_width(width)
        .finish()
    };
    let mut controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(control(
            "prompt_agent_install_close",
            AppIcon::XClose,
            "Close",
            WorkspaceAction::ClosePromptAgentInstall,
            140.0,
        ));
    if show_verification {
        controls = controls.with_child(control(
            "prompt_agent_install_check",
            AppIcon::Check,
            "Check again",
            WorkspaceAction::CheckPromptAgentInstall,
            160.0,
        ));
    }
    if retry_elevated {
        controls = controls.with_child(control(
            "prompt_agent_install_retry",
            AppIcon::RotateCcw,
            "Retry with administrator rights",
            WorkspaceAction::RetryRemoteToolElevated,
            260.0,
        ));
    }
    body = body.with_child(Box::new(controls));

    let sheet = Container::new(
        ConstrainedBox::new(body.finish())
            .with_width(720.0)
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_uniform_padding(20.0)
    .finish();
    // A click in the sheet stays in the sheet. The terminal focuses itself on
    // a click and lets it go on; past the sheet it reached the workspace's
    // "click on nothing" handler, which took the keyboard back from the
    // terminal, so typing and pasting went nowhere.
    EventHandler::new(sheet)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish()
}

/// The glyph for linked work of `kind` in one colour (attachment chips):
/// the pull request glyph, or its service's logo.
pub(crate) fn link_icon(kind: LinkKind) -> AppIcon {
    match kind {
        LinkKind::PullRequest => AppIcon::PullRequest,
        LinkKind::Item(provider) => match provider.logo() {
            crate::integrations::Logo::Svg(path) => AppIcon::Asset(path),
            crate::integrations::Logo::Letters(_) => AppIcon::TaskList,
        },
    }
}

/// The mark for linked work of `kind` in its brand colour (menus).
fn link_mark(theme: &Theme, ui: FamilyId, kind: LinkKind, size: f32) -> Box<dyn Element> {
    match kind {
        LinkKind::PullRequest => render_icon(AppIcon::PullRequest, theme.main_text(), size),
        LinkKind::Item(provider) => crate::integrations::marks::mark(theme, ui, provider, size),
    }
}

/// The search menu's header: back to the Add menu, and what's searched.
fn links_header(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui: FamilyId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    kind: LinkKind,
) -> Box<dyn Element> {
    let back = Hoverable::new(mouse_state(states, "prompt_links_back"), move |state| {
        Container::new(render_icon(
            AppIcon::ArrowLeft,
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            13.0,
        ))
        .with_uniform_padding(5.0)
        .with_background_color(if state.is_hovered() {
            theme.surface_3()
        } else {
            warpui::color::ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::OpenPromptMenu {
                menu: PromptMenu::Add,
            },
        );
    })
    .finish();
    let title = match kind {
        LinkKind::PullRequest => "Pull requests".to_string(),
        LinkKind::Item(provider) => format!("{} {}s", provider.label(), provider.item()),
    };
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(back)
            .with_child(link_mark(theme, ui, kind, 14.0))
            .with_child(
                Text::new(title, ui, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(4.0)
    .with_padding_right(8.0)
    .with_padding_top(2.0)
    .with_padding_bottom(6.0)
    .finish()
}

/// A menu row with its own leading mark, a title, and an optional muted
/// second line (an item's key and where it lives).
#[allow(clippy::too_many_arguments)]
fn entry_row<A: warpui::Action + Clone + 'static>(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui: FamilyId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: &str,
    leading: Box<dyn Element>,
    title: &str,
    subtitle: Option<String>,
    checked: bool,
    action: A,
) -> Box<dyn Element> {
    let title = title.to_string();
    Hoverable::new(mouse_state(states, key), move |state| {
        let mut labels = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(2.0)
            .with_child(
                Text::new(title.clone(), ui, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            );
        if let Some(subtitle) = &subtitle {
            labels = labels.with_child(
                Text::new(subtitle.clone(), ui, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            );
        }
        let leading_box = ConstrainedBox::new(Align::new(leading).finish())
            .with_width(16.0)
            .finish();
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(9.0)
                .with_child(leading_box)
                .with_child(Expanded::new(1.0, labels.finish()).finish())
                .with_child(if checked {
                    render_icon(AppIcon::Check, theme.accent, 13.0)
                } else {
                    ConstrainedBox::new(warpui::elements::Empty::new().finish())
                        .with_width(13.0)
                        .finish()
                })
                .finish(),
        )
        .with_background_color(if state.is_hovered() {
            theme.surface_3()
        } else {
            theme.surface_2()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(if subtitle.is_some() { 6.0 } else { 7.0 })
        .with_padding_bottom(if subtitle.is_some() { 6.0 } else { 7.0 })
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
    })
    .finish()
}

pub(crate) fn menu_caption(theme: &Theme, ui: FamilyId, text: &str) -> Box<dyn Element> {
    Container::new(
        Text::new(text.to_string(), ui, crate::theme::text_size(11.0))
            .with_color(theme.disabled_text())
            .finish(),
    )
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(4.0)
    .with_padding_bottom(2.0)
    .finish()
}

pub(crate) fn agent_image(
    agent: PromptAgent,
    theme: &Theme,
    size: f32,
    opacity: f32,
) -> Box<dyn Element> {
    let Some(path) = agent.image_path(theme.is_dark) else {
        return render_icon(AppIcon::Agent, theme.sub_text(), size);
    };
    let image = ConstrainedBox::new(
        Image::new(AssetSource::Bundled { path }, CacheOption::BySize)
            .contain()
            .with_opacity(opacity)
            .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish();
    image
}

#[allow(clippy::too_many_arguments)]
/// The team menu's tick box: filled with the accent and a tick when the
/// agent is in the team, an empty outline when not. The lead's can't be
/// unticked, so it's drawn a little quieter.
fn team_checkbox(theme: &Theme, checked: bool, editable: bool) -> Box<dyn Element> {
    let side = 15.0;
    let inner: Box<dyn Element> = if checked {
        Align::new(render_icon(AppIcon::Check, theme.background, 11.0)).finish()
    } else {
        warpui::elements::Empty::new().finish()
    };
    let mut boxed = Container::new(
        ConstrainedBox::new(inner)
            .with_width(side)
            .with_height(side)
            .finish(),
    )
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
    boxed = if checked {
        let accent = theme.accent;
        let fill = if editable {
            accent
        } else {
            warpui::color::ColorU::new(accent.r, accent.g, accent.b, 150)
        };
        // The same width of edge as an empty box, so the rows line up.
        boxed
            .with_background_color(fill)
            .with_border(warpui_core::scene::Border::all(1.5).with_border_color(fill))
    } else {
        boxed.with_border(warpui_core::scene::Border::all(1.5).with_border_color(theme.outline()))
    };
    boxed.finish()
}

fn agent_menu_row(
    theme: &Theme,
    ui: FamilyId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: &str,
    agent: PromptAgent,
    checked: bool,
    installed: bool,
    // A muted word after the name ("leads").
    note: Option<&'static str>,
    pick: AgentPick,
) -> Box<dyn Element> {
    picker_agent_row(
        theme,
        ui,
        mouse_state(states, key),
        agent_image(agent, theme, 16.0, if installed { 1.0 } else { 0.45 }),
        agent.label().to_string(),
        None,
        note,
        installed,
        checked,
        false,
        Rc::new(move |event_ctx| pick(event_ctx, agent, installed)),
    )
}

/// One row of an agent menu: artwork, name (and a second line when there is
/// one), then a check when chosen or "Install" when the agent is missing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn picker_agent_row(
    theme: &Theme,
    ui: FamilyId,
    mouse: MouseStateHandle,
    icon: Box<dyn Element>,
    title: String,
    detail: Option<String>,
    note: Option<&'static str>,
    available: bool,
    checked: bool,
    highlighted: bool,
    on_click: Rc<dyn Fn(&mut EventContext)>,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(mouse, move |state| {
        let mut name = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(
                Shrinkable::new(
                    1.0,
                    Text::new(title, ui, crate::theme::text_size(12.5))
                        .with_color(if available {
                            theme.main_text()
                        } else {
                            theme.disabled_text()
                        })
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
            );
        if let Some(note) = note {
            name = name.with_child(
                Text::new(note.to_string(), ui, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            );
        }
        let mut words = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(1.0)
            .with_child(name.finish());
        if let Some(detail) = detail {
            words = words.with_child(
                Text::new(detail, ui, crate::theme::text_size(11.0))
                    .with_color(theme.sub_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            );
        }
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(icon)
                .with_child(Expanded::new(1.0, words.finish()).finish())
                .with_child(if !available {
                    Text::new("Install".to_string(), ui, crate::theme::text_size(11.0))
                        .with_color(theme.accent)
                        .finish()
                } else if checked {
                    render_icon(AppIcon::Check, theme.main_text(), 13.0)
                } else {
                    ConstrainedBox::new(warpui::elements::Empty::new().finish())
                        .with_width(13.0)
                        .finish()
                })
                .finish(),
        )
        .with_background_color(if highlighted || state.is_hovered() {
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
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| on_click(event_ctx))
    .finish()
}

/// Something feedback can go to: an agent session already running in this
/// workspace, or an installed agent to start. Diff comments and browser
/// design notes both choose from these, in the same menu.
#[derive(Clone, Debug)]
pub(crate) struct AgentTarget {
    /// `existing:<pane uuid>` or `new:<agent id>`.
    pub key: String,
    /// The agent's name, e.g. "Claude Code".
    pub name: String,
    /// What the menu shows first: the session's tab title, or the agent name.
    pub title: String,
    /// The line under it: which agent and what it's doing, or "New session".
    pub detail: String,
    pub agent: String,
    pub existing: bool,
    pub image: Option<super::AgentImage>,
}

impl AgentTarget {
    pub(crate) fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [&self.name, &self.title, &self.detail, &self.agent]
                .iter()
                .any(|text| text.to_lowercase().contains(query))
    }

    /// What a chip shows once this is chosen: the session, or "New Codex".
    pub(crate) fn chip_label(&self) -> String {
        if self.existing {
            self.title.clone()
        } else {
            format!("New {}", self.name)
        }
    }
}

/// The targets matching `search`, in menu order: running sessions first.
pub(crate) fn visible_agent_targets<'a>(
    targets: &'a [AgentTarget],
    search: &str,
) -> Vec<&'a AgentTarget> {
    let query = search.trim().to_lowercase();
    let (running, new): (Vec<_>, Vec<_>) = targets
        .iter()
        .filter(|target| target.matches(&query))
        .partition(|target| target.existing);
    running.into_iter().chain(new).collect()
}

/// The chosen target when the menu opens: the session last chosen, else the
/// most recently active one, else the agent last started, else the first.
pub(crate) fn default_agent_target(
    targets: &[AgentTarget],
    settings: &crate::setup::UiSettings,
) -> Option<String> {
    let listed = |key: &&String| targets.iter().any(|target| target.key == **key);
    settings
        .diff_comment_terminal
        .as_ref()
        .filter(listed)
        .cloned()
        .or_else(|| targets.iter().find(|t| t.existing).map(|t| t.key.clone()))
        .or_else(|| settings.diff_comment_agent.as_ref().filter(listed).cloned())
        .or_else(|| targets.first().map(|t| t.key.clone()))
}

/// Remember a choice for the next comment or batch of notes.
pub(crate) fn remember_agent_target(key: &str) {
    let mut settings = crate::setup::UiSettings::load();
    if key.starts_with("existing:") {
        settings.diff_comment_terminal = Some(key.to_string());
    } else {
        settings.diff_comment_agent = Some(key.to_string());
    }
    settings.save();
}

/// A target's artwork, or the terminal glyph when it has none.
pub(crate) fn agent_target_image(
    target: Option<&AgentTarget>,
    theme: &Theme,
    size: f32,
) -> Box<dyn Element> {
    let source = match target.and_then(|t| t.image.as_ref()) {
        Some(super::AgentImage::Bundled(path)) => Some(AssetSource::Bundled { path }),
        Some(super::AgentImage::File(path)) => Some(AssetSource::LocalFile { path: path.clone() }),
        None => None,
    };
    let icon = match source {
        Some(source) => Image::new(source, CacheOption::BySize).contain().finish(),
        None if target.is_some() => render_icon(AppIcon::Terminal, theme.sub_text(), size),
        None => render_icon(AppIcon::Agent, theme.sub_text(), size),
    };
    ConstrainedBox::new(icon)
        .with_width(size)
        .with_height(size)
        .finish()
}

/// Everything the session-aware agent menu needs.
pub(crate) struct AgentTargetMenuProps<'a> {
    pub theme: &'a Theme,
    pub ui: FamilyId,
    pub states: &'a RefCell<HashMap<String, MouseStateHandle>>,
    pub scroll: &'a ClippedScrollStateHandle,
    pub search_editor: &'a warpui::ViewHandle<super::InputEditor>,
    pub search: &'a str,
    pub targets: &'a [AgentTarget],
    pub selected: Option<&'a str>,
    /// The row the arrow keys are on, for menus that take keys.
    pub active: Option<usize>,
    pub pick: Rc<dyn Fn(&mut EventContext, String)>,
    pub clear_search: Rc<dyn Fn(&mut EventContext)>,
}

/// The agent menu with sessions: the prompt screen's menu (search field,
/// captions, rows with artwork) listing the sessions running in this
/// workspace, then the installed agents that would start a new one.
pub(crate) fn render_agent_target_menu(props: AgentTargetMenuProps<'_>) -> Box<dyn Element> {
    let AgentTargetMenuProps {
        theme,
        ui,
        states,
        scroll,
        search_editor,
        search,
        targets,
        selected,
        active,
        pick,
        clear_search,
    } = props;
    let visible = visible_agent_targets(targets, search);
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    if visible.is_empty() {
        col = col.with_child(menu_caption(
            theme,
            ui,
            if targets.is_empty() {
                "No agents found in this workspace"
            } else {
                "No matching agents"
            },
        ));
    }
    let mut running_caption = true;
    let mut new_caption = true;
    for (index, target) in visible.into_iter().enumerate() {
        if target.existing && running_caption {
            col = col.with_child(menu_caption(theme, ui, "RUNNING IN THIS WORKSPACE"));
            running_caption = false;
        }
        if !target.existing && new_caption {
            col = col.with_child(menu_caption(theme, ui, "START A NEW SESSION"));
            new_caption = false;
        }
        let key = target.key.clone();
        let pick = pick.clone();
        col = col.with_child(picker_agent_row(
            theme,
            ui,
            mouse_state(states, format!("agent_target_{}", target.key)),
            agent_target_image(Some(target), theme, 16.0),
            target.title.clone(),
            // A session titled after its agent needs no second "Codex" line.
            target
                .existing
                .then(|| target.detail.clone())
                .filter(|detail| *detail != target.title),
            None,
            true,
            selected == Some(target.key.as_str()),
            active == Some(index),
            Rc::new(move |event_ctx| pick(event_ctx, key.clone())),
        ));
    }
    let search = agent_search_field(theme, states, search_editor, search, clear_search);
    let contents = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(search)
        .with_child(menu_list(theme, scroll, col.finish(), 300.0))
        .finish();
    menu_card(theme, contents, 280.0)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn menu_row<A: warpui::Action + Clone + 'static>(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui: FamilyId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: &str,
    icon: AppIcon,
    label: &str,
    checked: bool,
    action: A,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Hoverable::new(mouse_state(states, key), move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(icon, theme.main_text(), 13.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(label_owned.clone(), ui, crate::theme::text_size(12.5))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(if checked {
                    render_icon(AppIcon::Check, theme.main_text(), 13.0)
                } else {
                    ConstrainedBox::new(warpui::elements::Empty::new().finish())
                        .with_width(13.0)
                        .finish()
                })
                .finish(),
        )
        .with_background_color(if state.is_hovered() {
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
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
    })
    .finish()
}

#[cfg(test)]
mod agent_target_tests {
    use super::*;

    fn target(key: &str, title: &str, existing: bool) -> AgentTarget {
        AgentTarget {
            key: key.into(),
            name: title.into(),
            title: title.into(),
            detail: if existing {
                "Claude Code".into()
            } else {
                "Start a new session".into()
            },
            agent: "claude".into(),
            existing,
            image: None,
        }
    }

    #[test]
    fn running_sessions_come_first_and_search_narrows() {
        let targets = [
            target("new:claude", "Claude Code", false),
            target("existing:a", "Fix login", true),
            target("new:codex", "Codex", false),
        ];
        let keys = |search| {
            visible_agent_targets(&targets, search)
                .iter()
                .map(|t| t.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(""), ["existing:a", "new:claude", "new:codex"]);
        assert_eq!(keys("codex"), ["new:codex"]);
        assert_eq!(
            target("new:codex", "Codex", false).chip_label(),
            "New Codex"
        );
        assert_eq!(
            target("existing:a", "Fix login", true).chip_label(),
            "Fix login"
        );
    }

    #[test]
    fn the_default_is_the_last_session_then_a_running_one_then_the_last_agent() {
        let targets = [
            target("existing:a", "A", true),
            target("existing:b", "B", true),
            target("new:codex", "Codex", false),
        ];
        let mut settings = crate::setup::UiSettings::default();
        assert_eq!(
            default_agent_target(&targets, &settings).as_deref(),
            Some("existing:a")
        );
        settings.diff_comment_terminal = Some("existing:b".into());
        assert_eq!(
            default_agent_target(&targets, &settings).as_deref(),
            Some("existing:b")
        );
        // A session that has ended falls back to one still running.
        settings.diff_comment_terminal = Some("existing:gone".into());
        assert_eq!(
            default_agent_target(&targets, &settings).as_deref(),
            Some("existing:a")
        );
        settings.diff_comment_agent = Some("new:codex".into());
        assert_eq!(
            default_agent_target(&targets[2..], &settings).as_deref(),
            Some("new:codex")
        );
    }
}
