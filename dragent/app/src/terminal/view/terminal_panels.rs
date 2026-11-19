//! Terminal model selector and history panels.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    BranchDiffOp, TerminalView, BLOCK_LIST_BOTTOM_GAP_PX, BLOCK_MENU_EST_HEIGHT,
    CMD_HIST_CHROME_PX, CMD_HIST_ROW_PX, RESIZE_PADDING_X, RESIZE_PADDING_Y,
    SCROLL_BOTTOM_EPSILON_PX,
};
use crate::agent::input_classifier::{
    heuristic::HeuristicInputClassifier, ClassifyContext, InputClassifier,
};
use crate::agent::{DetectedInputType, InputConfig, TabSessionKind};
#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use crate::console::ai_toolbar::{self, AiPanelActions, AiTool};
use crate::console::block_filter::{block_output_lines, count_block_matches};
use crate::console::command_intel;
use crate::console::dir_listing::{resolve_entry_path, DirSort, DirView};
use crate::console::editor_settings::EditorSettings;
use crate::console::escape_parse::styled_to_colored;
use crate::console::fs_suggest::should_suggest_paths;
use crate::console::history_search::HistorySearchState;
use crate::console::input_bar;
use crate::console::input_editing;
use crate::console::input_overlays;
use crate::console::path_picker::PathPicker;
use crate::console::prefixed_history;
use crate::console::shell_aliases::ShellAliases;
use crate::console::suggest_engine::SuggestEngine;
use crate::console::workflow_search::WorkflowSearchState;
use crate::console::workflows::{Workflow, WorkflowSession, WorkflowStore};
use crate::icons::ICON_SIZE_SM;
use crate::process_ext::NoWindow;
use crate::setup::UiSettings;
use crate::terminal::input::{
    encode_keystroke, encode_paste, map_pointer_to_cell, modifiers_from_ui, mouse_event,
    scroll_arrow_bytes, should_encode_wheel_as_mouse,
    should_forward_key_to_pty_while_ai_panel_open, should_passthrough_mouse, wheel_button,
    RichInputKeyContext,
};
use crate::terminal::manager::TerminalManager;
use crate::terminal::model::block_height_tree::BlockHeightIndex;
use crate::terminal::model::{
    ShellLinkStatus, SizeInfo, SizeReason, SizeUpdate, TerminalEvent, TerminalModel,
};
use crate::terminal::render::{
    build_alt_screen_ui, build_block_options_menu, build_bootstrap_grid_ui,
    build_sticky_block_header, build_visible_block_list_column, dir_grid_content_width,
    extract_prompt_tail, sample_ai_chrome, BlockListScrollAdapter, BlockRenderContext,
    CellGlyphCache, CellMetrics, DiagramCanvasState, StickyBlockHeader, TerminalSizeElement,
    BLOCK_HEADER_HEIGHT, BLOCK_LIST_BOTTOM_POSITION_ID,
};
use crate::terminal::view::actions::{EntryHint, LinkHint, TerminalAction};
use crate::terminal::viewport::{
    is_block_list_virtualization_enabled, ScrollMode, ScrollPosition, ViewportState,
};
use crate::terminal::TerminalEventListener;
use crate::theme::{Theme, INPUT_FONT_SIZE, TERMINAL_PADDING_LEFT};
use crate::workspace::persistence::PersistedBlock;
use crate::workspace::{InputEditor, InputEditorEvent, InputMode, DEFAULT_SESSION_TITLE};
use async_channel::Sender;
use dragent_pty::{resolve_shell, PtyEvent, SessionPlatform};
use dragent_resource_policy::{ActivityLevel, MemoryPressure, ResourceBudget};
use dragent_shell_hooks::ShellKind;
use dragent_terminal::{MouseButton, MouseEventKind, TermMode};
use parking_lot::{FairMutex, RwLock};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, Dismiss, DispatchEventResult, Element, EventHandler, Expanded,
    Fill, Flex, Hoverable, MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition,
    ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, SelectableArea,
    SelectionHandle, SelectionUpdateArgs, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::Vector2F;
use warpui::keymap::Keystroke;
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::{IntoPixels, Pixels};
use warpui::{
    r#async::Timer, AppContext, Entity, EntityId, FocusContext, GetSingletonModelHandle,
    ModelHandle, ReadModel, TypedActionView, UpdateModel, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{LiveElement, ScrollTarget, ScrollToPositionMode};
use warpui_core::event::{KeyState, ModifiersState};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::scene::Border;
use warpui_core::scene::{CornerRadius, Radius};
use warpui_core::WindowId;
use web_time::Instant;

impl TerminalView {
    /// The model menu's rows (model, label, effort) and its current pick:
    /// Dragent's models (with an effort row per level), or the chat agent's
    /// own models when this tab shows an agent as a chat. The desktop menu
    /// and a phone's picker read the same list.
    pub(crate) fn model_menu(
        &self,
        app: &AppContext,
    ) -> (Vec<(String, String, crate::settings::AgentEffort)>, String, crate::settings::AgentEffort) {
        use warpui::SingletonEntity;
        let selected_id = self.agent_model_override.clone()
            .unwrap_or_else(|| crate::settings::AppSettings::global().selected_model.clone());

        // Active models from the account store, plus user-defined custom models.
        let account = crate::settings::AccountStore::handle(app);
        let mut models: Vec<(String, String)> = app.read_model(&account, |store, _| {
            store
                .data
                .as_ref()
                .map(|d| {
                    d.models
                        .iter()
                        .filter(|m| m.is_active)
                        .map(|m| (m.id.clone(), m.name.clone()))
                        .collect()
                })
                .unwrap_or_default()
        });
        for cm in &crate::settings::AppSettings::global().custom_models {
            models.push((cm.model_id.clone(), cm.name.clone()));
        }
        models.insert(
            0,
            (
                crate::settings::AUTO_MODEL_ID.to_string(),
                "Auto".to_string(),
            ),
        );
        let settings = crate::settings::AppSettings::global();
        let has_byok = settings.has_any_user_credential();
        if has_byok {
            models.retain(|(id, _)| {
                id == crate::settings::AUTO_MODEL_ID || settings.has_user_credential_for_model(id)
            });
        }

        // Expand each effort-capable model into one row per level, so model and
        // effort are picked in a single click instead of requiring a second
        // control. The bare name means "provider default" — which is NOT always
        // "high" (Gemini flash-lite defaults to minimal), so it is listed first
        // rather than slotted in at a level position that would misstate it.
        let selected_effort = crate::settings::AppSettings::global().selected_effort;
        let models: Vec<(String, String, crate::settings::AgentEffort)> = models
            .into_iter()
            .flat_map(|(id, name)| {
                crate::settings::AgentEffort::rows_for(&id)
                    .iter()
                    .map(|e| {
                        let label = match e.wire_value() {
                            // Bare name = provider default.
                            None => name.clone(),
                            Some(level) => format!("{name} ({level})"),
                        };
                        (id.clone(), label, *e)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        // A chat run over ACP picks among its agent's own models instead.
        let acp_picker = self.chat_model_picker(app);
        let (models, selected_id, selected_effort) = match &acp_picker {
            Some(picker) => (
                picker
                    .choices
                    .iter()
                    .map(|c| (c.id.clone(), c.name.clone(), crate::settings::AgentEffort::default()))
                    .collect(),
                picker.current.clone(),
                crate::settings::AgentEffort::default(),
            ),
            None => (models, selected_id, selected_effort),
        };

        (models, selected_id, selected_effort)
    }

    /// The agent's effort as a slider under the model list: a plain name
    /// and what it means, over a rail with a knob to drag (or a stop to
    /// click). The level is set when the knob is let go.
    fn render_effort_slider(
        &self,
        effort: &dragent_acp::Picker,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        let shown = self.effort_drag.clone().unwrap_or_else(|| effort.current.clone());
        effort_slider(
            effort,
            &shown,
            self.effort_drag.is_some(),
            self.effort_track_id(),
            self.block_render_context(app).mouse_state("effort-slider"),
            theme,
            mono,
        )
    }

    pub(crate) fn render_model_selector(
        &self,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        use warpui::SingletonEntity;
        let open = self.model_menu_open;
        let (models, selected_id, selected_effort) = self.model_menu(app);
        let account = crate::settings::AccountStore::handle(app);
        let has_byok = crate::settings::AppSettings::global().has_any_user_credential();
        let acp_picker = self.chat_model_picker(app);

        let display = models
            .iter()
            .find(|(id, _, e)| *id == selected_id && *e == selected_effort)
            .map(|(_, name, _)| name.clone())
            .unwrap_or_else(|| selected_id.clone());
        let display: String = if display.chars().count() > 22 {
            let mut s: String = display.chars().take(21).collect();
            s.push('…');
            s
        } else {
            display
        };

        let theme = *theme;

        // ── Free-model lock ───────────────────────────────────────────────────
        // When the user has no premium-capable funds (paid allowance gone AND
        // credits off/empty) the server forces the free model — so the selector
        // LOCKS: it shows the free model, never opens the picker, and explains
        // why on hover. Enabling/buying credits (or the allowance resetting)
        // unlocks it. The server (account API) is authoritative on the flag.
        let (restricted, credits_offered, is_free_tier, _free_model_name) =
            app.read_model(&account, |store, _| {
                store
                    .data
                    .as_ref()
                    .map(|d| {
                        let free = d
                            .models
                            .iter()
                            .find(|m| m.min_tier == "free")
                            .map(|m| m.name.clone());
                        (
                            // A free-tier account must not be offered a model
                            // picker, even if its balance metadata temporarily
                            // reports premium-capable funds. The server remains
                            // authoritative for billing; this is the matching
                            // UI entitlement gate.
                            d.usage.model_locked_to_free
                                || d.user.tier.eq_ignore_ascii_case("free"),
                            d.usage.deposit_balance_mc > 0 && !d.usage.allow_credit_usage,
                            d.user.tier == "free",
                            free,
                        )
                    })
                    .unwrap_or((false, false, false, None))
            });

        // Dragent's plan limits Dragent's models, not another agent's.
        let locked = restricted && !has_byok && acp_picker.is_none();
        if locked {
            let lock_name = "Auto".to_string();
            let lock_name: String = if lock_name.chars().count() > 22 {
                let mut s: String = lock_name.chars().take(21).collect();
                s.push('…');
                s
            } else {
                lock_name
            };
            // Tailor the reason: credits held-but-off is the one-toggle fix; a
            // free-tier user never had a paid allowance to "use up"; a paid user
            // here has exhausted theirs.
            let tooltip = if credits_offered {
                "This model is locked. Turn on \u{201C}Use my credits\u{201D} in Settings to use other models."
            } else if is_free_tier {
                "The free plan is limited to this model. Buy credits or upgrade to use other models."
            } else {
                "You've used up your monthly allowance \u{2014} limited to the free model until it resets. Buy credits to keep using other models."
            };
            let ms = self
                .block_render_context(app)
                .mouse_state("model-menu-locked");
            let hovered = ms.lock().unwrap().is_hovered();
            let lock_trigger = Hoverable::new(ms, move |_state| {
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(5.0)
                        .with_child(
                            Text::new(lock_name.clone(), mono, crate::theme::text_size(11.5))
                                .with_color(theme.sub_text())
                                .finish(),
                        )
                        .with_child(crate::icons::render_icon(
                            crate::icons::AppIcon::Info,
                            theme.sub_text(),
                            11.0,
                        ))
                        .finish(),
                )
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish()
            })
            .with_cursor(Cursor::Arrow)
            .finish();
            // No click handler → the picker never opens; hover shows the reason.
            return self.with_chip_tooltip(lock_trigger, hovered, tooltip);
        }

        // Plain-text trigger (no border/background) — the Antigravity/Cursor
        // look: just the model name and a chevron that brightens on hover.
        let trigger_ms = self
            .block_render_context(app)
            .mouse_state("model-menu-trigger");
        let display_t = display.clone();
        let trigger = EventHandler::new(
            Hoverable::new(trigger_ms, move |state| {
                let bright = open || state.is_hovered();
                let color = if bright {
                    theme.main_text()
                } else {
                    theme.sub_text()
                };
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(5.0)
                        .with_child(
                            Text::new(display_t.clone(), mono, crate::theme::text_size(11.5))
                                .with_color(color)
                                .finish(),
                        )
                        .with_child(crate::icons::render_icon(
                            if open {
                                crate::icons::AppIcon::ChevronDown
                            } else {
                                crate::icons::AppIcon::ChevronUp
                            },
                            color,
                            11.0,
                        ))
                        .finish(),
                )
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .finish(),
        )
        .on_left_mouse_down(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::ToggleModelMenu);
            DispatchEventResult::StopPropagation
        })
        .finish();

        if !open {
            return trigger;
        }

        // The panel ALWAYS opens. Signed-out / not-yet-fetched sessions show the
        // current model plus a hint instead of silently doing nothing.
        let signed_out_fallback = models.len() == 1 && acp_picker.is_none();
        let mut models = models;
        if signed_out_fallback {
            models.push((selected_id.clone(), display.clone(), selected_effort));
        }

        // Filter before rendering. Matching id as well as name lets a user
        // find "claude-opus-4-8" by typing the version even when the display
        // name doesn't contain it.
        let query = self.model_search.trim().to_lowercase();
        let filtered: Vec<(String, String, crate::settings::AgentEffort)> = if query.is_empty() {
            models.clone()
        } else {
            models
                .iter()
                .filter(|(id, name, _)| {
                    name.to_lowercase().contains(&query) || id.to_lowercase().contains(&query)
                })
                .cloned()
                .collect()
        };

        // Search box doubles as the header — it replaces the old static
        // "Model" label rather than adding a row, so the popover keeps its
        // height.
        let header = Container::new(crate::shared_input::shared_input(
            &theme,
            &self.model_search_editor,
            crate::shared_input::SharedInputOpts {
                ..Default::default()
            },
        ))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(4.0)
        .with_padding_bottom(6.0)
        .finish();

        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0);
        if filtered.is_empty() {
            list = list.with_child(
                Container::new(
                    Text::new(
                        format!("No model matches \"{}\"", self.model_search.trim()),
                        mono,
                        crate::theme::text_size(11.0),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .with_uniform_padding(10.0)
                .finish(),
            );
        }
        // No cap. This used to `.take(12)`, which silently hid every model
        // past the twelfth with nothing on screen to say so — the list is
        // scrollable now, so the full set is reachable.
        for (id, name, row_effort) in filtered.iter() {
            // Both must match: the same model appears once per effort level.
            let selected = *id == selected_id
                && (*id == crate::settings::AUTO_MODEL_ID || *row_effort == selected_effort);
            let ms = self
                .block_render_context(app)
                .mouse_state(&format!("model-row-{id}-{}", row_effort.label()));
            let name_c = name.clone();
            let id_click = id.clone();
            let effort_click = if id == crate::settings::AUTO_MODEL_ID {
                crate::settings::AgentEffort::default()
            } else {
                *row_effort
            };
            let row = EventHandler::new(
                Hoverable::new(ms, move |state| {
                    let hovered = state.is_hovered();
                    let bg = if selected {
                        warpui::color::ColorU::new(
                            theme.accent.r,
                            theme.accent.g,
                            theme.accent.b,
                            30,
                        )
                    } else if hovered {
                        theme.surface_3()
                    } else {
                        theme.surface_2()
                    };
                    let check: Box<dyn Element> = if selected {
                        crate::icons::render_icon(crate::icons::AppIcon::Check, theme.accent, 12.0)
                    } else {
                        warpui::elements::ConstrainedBox::new(
                            warpui::elements::Empty::new().finish(),
                        )
                        .with_width(12.0)
                        .with_height(12.0)
                        .finish()
                    };
                    Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(check)
                            .with_child(
                                Text::new(name_c.clone(), mono, crate::theme::text_size(12.0))
                                    .with_color(if selected {
                                        theme.accent
                                    } else {
                                        theme.main_text()
                                    })
                                    .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .with_background_color(bg)
                    .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                        warpui_core::scene::Radius::Pixels(5.0),
                    ))
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::SetAgentModel {
                    model_id: id_click.clone(),
                    effort: effort_click,
                });
                DispatchEventResult::StopPropagation
            })
            .finish();
            list = list.with_child(row);
        }

        if signed_out_fallback {
            list = list.with_child(
                Container::new(
                    Text::new(
                        "Sign in to load more models".to_string(),
                        mono,
                        crate::theme::text_size(10.5),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(5.0)
                .with_padding_bottom(4.0)
                .finish(),
            );
        }

        // Cap the LIST, not the popover: the height stays what twelve rows
        // used to occupy, and anything beyond scrolls.
        let scrolling_list = ConstrainedBox::new(
            ClippedScrollable::vertical(
                self.model_scroll.clone(),
                list.finish(),
                ScrollbarWidth::Auto,
                theme.outline().into(),
                theme.sub_text().into(),
                theme.surface_1().into(),
            )
            .with_overlayed_scrollbar()
            .finish(),
        )
        .with_max_height(340.0)
        .finish();

        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(scrolling_list);
        if let Some(effort) = self.chat_effort_picker(app) {
            body = body.with_child(self.render_effort_slider(&effort, app, &theme, mono));
        }
        let body = body.finish();

        let popover = ConstrainedBox::new(
            Container::new(body)
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(theme.surface_1())
                .with_border(
                    warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                )
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(8.0),
                ))
                .finish(),
        )
        .with_width(260.0)
        .finish();

        Stack::new()
            .with_child(trigger)
            .with_positioned_overlay_child(
                popover,
                OffsetPositioning::offset_from_parent(
                    Vector2F::new(0.0, -6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::BottomLeft,
                ),
            )
            .finish()
    }

    /// Session history browser trigger + popover — same self-contained
    /// trigger/`Stack`/`with_positioned_overlay_child` shape as
    /// `render_mode_selector`/`render_model_selector`, row styling matches
    /// the flat, full-width, bottom-divider style used by the slash palette.
    pub(crate) fn render_history_panel(
        &self,
        app: &AppContext,
        theme: &Theme,
        mono: FamilyId,
    ) -> Box<dyn Element> {
        let open = self.history_panel_open;
        let theme_t = *theme;
        let trigger_ms = self
            .block_render_context(app)
            .mouse_state("history-panel-trigger");
        let trigger_hovered = trigger_ms.lock().unwrap().is_hovered();
        let trigger_inner = EventHandler::new(
            Hoverable::new(trigger_ms, move |state| {
                let hot = open || state.is_hovered();
                let icon_color = if hot {
                    theme_t.accent
                } else {
                    theme_t.sub_text()
                };
                let bg = if hot {
                    warpui::color::ColorU::new(
                        theme_t.accent.r,
                        theme_t.accent.g,
                        theme_t.accent.b,
                        46,
                    )
                } else {
                    warpui::color::ColorU::new(0, 0, 0, 0)
                };
                Container::new(crate::icons::render_icon(
                    crate::icons::AppIcon::History,
                    icon_color,
                    15.0,
                ))
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(bg)
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(6.0),
                ))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .finish(),
        )
        .on_left_mouse_down(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::ToggleHistoryPanel);
            DispatchEventResult::StopPropagation
        })
        .finish();

        // Tooltip opens upward — this button sits in the input bar's bottom
        // controls row, so a downward tooltip (the branch-diff panel's usual
        // direction) would run off the bottom of the window.
        let trigger = if trigger_hovered && !open {
            let tooltip = Tooltip::new(
                "Session history".to_string(),
                self.branch_diff_tooltip_styles(),
            )
            .build()
            .finish();
            Stack::new()
                .with_child(trigger_inner)
                .with_positioned_overlay_child(
                    tooltip,
                    OffsetPositioning::offset_from_parent(
                        Vector2F::new(0.0, -6.0),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::TopMiddle,
                        ChildAnchor::BottomMiddle,
                    ),
                )
                .finish()
        } else {
            trigger_inner
        };

        if !open {
            return trigger;
        }

        let last_idx = self.conversation_summaries.len().saturating_sub(1);
        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if self.conversation_summaries.is_empty() {
            list = list.with_child(
                Container::new(Box::new(
                    Text::new(
                        "No past conversations yet".to_string(),
                        mono,
                        crate::theme::text_size(11.5),
                    )
                    .with_color(theme.sub_text()),
                ))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(10.0)
                .with_padding_bottom(10.0)
                .finish(),
            );
        }
        for (idx, summary) in self.conversation_summaries.iter().enumerate() {
            let is_last = idx == last_idx;
            let theme_row = *theme;
            let title = crate::agent::controller::synthesize_conversation_title(
                &summary.first_block_kind_json,
            );
            let ago = {
                let now_ms = web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(summary.last_activity_unix_ms);
                let secs = ((now_ms - summary.last_activity_unix_ms).max(0) / 1000) as u64;
                history_relative_time(secs)
            };
            let conversation_id = summary.conversation_id;
            let ms = self
                .block_render_context(app)
                .mouse_state(&format!("history-row-{conversation_id}"));
            let row = EventHandler::new(
                Hoverable::new(ms, move |state| {
                    let hovered = state.is_hovered();
                    let bg = if hovered {
                        theme_row.surface_3()
                    } else {
                        theme_row.surface_1()
                    };
                    let mut row_container = Container::new(
                        Flex::row()
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(10.0)
                            .with_child(Box::new(Expanded::new(
                                1.0,
                                Text::new(title.clone(), mono, crate::theme::text_size(12.5))
                                    .with_color(theme_row.main_text())
                                    .soft_wrap(false)
                                    .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                                    .finish(),
                            )))
                            .with_child(
                                Text::new(ago.clone(), mono, crate::theme::text_size(10.5))
                                    .with_color(theme_row.sub_text())
                                    .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(bg);
                    if !is_last {
                        row_container = row_container.with_border(
                            warpui_core::scene::Border::bottom(1.0)
                                .with_border_color(theme_row.outline()),
                        );
                    }
                    row_container.finish()
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx
                    .dispatch_typed_action(TerminalAction::ResumeConversation { conversation_id });
                DispatchEventResult::StopPropagation
            })
            .finish();
            list = list.with_child(row);
        }

        let popover = ConstrainedBox::new(
            Container::new(list.finish())
                .with_background_color(theme.surface_1())
                .with_border(
                    warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                )
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(8.0),
                ))
                .finish(),
        )
        .with_width(280.0)
        .finish();

        Stack::new()
            .with_child(trigger)
            .with_positioned_overlay_child(
                popover,
                OffsetPositioning::offset_from_parent(
                    Vector2F::new(0.0, -6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::BottomLeft,
                ),
            )
            .finish()
    }

    /// The disabled input field for a view-only shared-session viewer: a padlock
    /// glyph + "request edit" prompt shown where the editor would be. It takes no
    /// focus and swallows clicks (so nothing types); hovering shows how to get
    /// edit access. The keydown gate additionally drops any keystroke that would
    /// otherwise reach the PTY.
    pub(crate) fn render_readonly_input_field(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono_family;
        let ms = self
            .block_render_context(app)
            .mouse_state("remote-input-lock");
        let hovered = ms.lock().unwrap().is_hovered();

        // Reflect the host's live command line (mirrored into `command_editor` by
        // `apply_remote_surface`) so a watcher SEES what the host is typing —
        // read-only, with the lock glyph. When the host's input is empty, show the
        // "view only" hint instead.
        let host_text = self
            .command_editor
            .read(app, |e, _| e.get_text().to_string());
        let host_empty = host_text.is_empty();
        let label = if host_empty {
            "View only — request edit to type".to_string()
        } else {
            host_text
        };
        let label_color = if host_empty {
            theme.disabled_text()
        } else {
            theme.main_text()
        };

        let field = Hoverable::new(ms, move |_st| {
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(crate::icons::render_icon(
                    crate::icons::AppIcon::Lock,
                    theme.disabled_text(),
                    INPUT_FONT_SIZE,
                ))
                .with_child(Box::new(
                    Text::new(label.clone(), mono, crate::theme::text_size(INPUT_FONT_SIZE))
                        .with_color(label_color),
                ))
                .finish()
        })
        .with_cursor(Cursor::NotAllowed)
        .finish();

        // Swallow left clicks so the pane never focuses into a typing state.
        let field = EventHandler::new(field)
            .on_left_mouse_down(|_ctx, _app, _pos| DispatchEventResult::StopPropagation)
            .finish();

        self.with_chip_tooltip(
            field,
            hovered,
            "You're viewing this session — use \u{201C}Request edit\u{201D} to type.",
        )
    }

    /// The folder and git chips across the top of the input.
    pub(crate) fn render_input_chips_header(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono_family;
    let git_status = self.git_repo_status.read(app, |m, _| m.status.clone());
    let git_cwd = self.git_repo_status.read(app, |m, _| m.cwd.clone());

    // Git chips (branch / changes / ±diff) moved to the workspace bottom bar,
    // which dispatches these same actions to this session.

    let interactive_chips = {
        let pushing = self.branch_diff_op == BranchDiffOp::Pushing;
        let pulling = self.branch_diff_op == BranchDiffOp::Pulling;
        let g = git_status.as_ref();
        input_bar::InteractiveChips {
            folder: Some(self.render_folder_chip(app, git_cwd.as_path(), &theme, mono)),
            branch: None,
            changes: None,
            ahead: None,
            behind: g.filter(|g| pulling || g.behind > 0).map(|g| {
                self.render_status_chip(
                    app,
                    "chip-behind",
                    None,
                    if pulling {
                        "\u{2193} pulling…".to_string()
                    } else {
                        format!("\u{2193}{}", g.behind)
                    },
                    theme.ansi_blue(),
                    if pulling {
                        "Pulling from origin…".to_string()
                    } else {
                        format!("{} commit(s) to pull — click to pull from origin", g.behind)
                    }
                    .as_str(),
                    pulling,
                    &theme,
                    mono,
                    (!pulling).then_some(TerminalAction::PullBranchChanges),
                )
            }),
        }
    };
    let chips = input_bar::render_git_status_chips_with_diff(
        None,
        &theme,
        mono,
        Some(git_cwd.as_path()),
        None,
        self.branch_diff_op == BranchDiffOp::Pushing,
        self.branch_diff_op == BranchDiffOp::Pulling,
        interactive_chips,
    );

    let chips_header = Container::new(chips)
        .with_background_color(theme.pane_surface())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish();
        chips_header
    }

    /// The input's mic button (dictation), when this platform has dictation.
    pub(crate) fn render_input_mic_button(&self) -> Option<Box<dyn Element>> {
        let theme = self.theme;
    if !crate::voice::supported() {
        return None;
    }
    {
            // Idle: plain. Connecting: accent + border (permissions/token/
            // socket still in flight — NOT recording yet). Recording: red.
            let (icon_color, border_color) = match self.voice_ui {
                crate::voice::VoiceUiState::Idle => (theme.sub_text(), None),
                crate::voice::VoiceUiState::Connecting => (theme.accent, Some(theme.accent)),
                crate::voice::VoiceUiState::Recording => (theme.ui_red(), Some(theme.ui_red())),
            };
            // While connecting the mic glyph is REPLACED by a spinner —
            // a recoloured mic still reads as "recording now", which is
            // exactly the wrong thing to say before any audio flows.
            let glyph = if self.voice_ui == crate::voice::VoiceUiState::Connecting {
                crate::icons::render_spinner(
                    self.voice_spin,
                    icon_color,
                    crate::icons::ICON_SIZE_MD,
                )
            } else {
                crate::icons::render_icon(
                    crate::icons::AppIcon::Microphone,
                    icon_color,
                    crate::icons::ICON_SIZE_MD,
                )
            };
            let mut mic = Container::new(glyph)
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(4.0),
                ));
            if let Some(bc) = border_color {
                mic = mic.with_border(Border::all(1.0).with_border_color(bc));
            }
            let mic_btn = EventHandler::new(mic.finish())
                .on_left_mouse_down(|event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::ToggleVoiceInput);
                    DispatchEventResult::StopPropagation
                })
                .finish();
            Some(mic_btn)
        }
    }

    /// The input's Send / Stop button: ↑ sends, a filled square stops while
    /// `busy`; dimmed when idle with nothing to send.
    pub(crate) fn render_input_send_button(
        &self,
        busy: bool,
        has_text: bool,
        send: TerminalAction,
        stop: TerminalAction,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        // Tab theme color — the button matches whatever accent the
        // user picked for this tab, not a hardcoded palette.
        let salmon = theme.accent;
        let glyph_dark = theme.background;

        let (inner, action): (Box<dyn Element>, TerminalAction) = if busy {
            // Filled rounded square = stop.
            let square = Container::new(warpui::elements::Empty::new().finish())
                .with_background_color(glyph_dark)
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(2.0),
                ))
                .finish();
            (
                warpui::elements::ConstrainedBox::new(square)
                    .with_width(12.0)
                    .with_height(12.0)
                    .finish(),
                stop,
            )
        } else {
            (
                crate::icons::render_icon(crate::icons::AppIcon::ArrowUp, glyph_dark, 16.0),
                send,
            )
        };

        // Idle + empty composer → dimmed (nothing to send yet).
        let bg = if busy || has_text {
            salmon
        } else {
            warpui::color::ColorU {
                r: salmon.r,
                g: salmon.g,
                b: salmon.b,
                a: 110,
            }
        };
        let btn = Container::new(
            Container::new(inner)
                .with_padding_left(7.5)
                .with_padding_right(7.5)
                .with_padding_top(7.5)
                .with_padding_bottom(7.5)
                .finish(),
        )
        .with_background_color(bg)
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(7.0),
        ))
        .finish();
        let btn = EventHandler::new(btn)
            .on_left_mouse_down(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish();
        Container::new(btn).with_padding_left(4.0).finish()
    }

    pub(crate) fn render_command_input_box(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono_family;
        let command_running = self.model.read(app, |m, _| m.is_command_running());
        // Only hide the input bar mid-command when shell integration is actually
        // live — i.e. we are receiving real `precmd` events that will end the
        // command. Without integration (e.g. the shell hooks failed to load, as
        // happened on Windows) the block would appear "running" forever, which
        // permanently removed the input bar and locked the user out. Keeping the
        // bar visible in that case means you can always type / Ctrl-C / resubmit.
        let integration_active = self
            .model
            .read(app, |m, _| m.with_core(|c| c.integration_active()));
        if command_running && integration_active {
            return warpui::elements::Empty::new().finish();
        }

        let prompt_lines = self.shell_prompt_lines_for_input(app);
        let prompt_prefix = input_bar::render_shell_prompt_prefix(
            &prompt_lines,
            &theme,
            mono,
            crate::theme::INPUT_FONT_SIZE,
        );
        let fallback_glyph = Container::new(crate::icons::render_icon(
            crate::icons::AppIcon::ChevronRight,
            theme.ansi_green(),
            crate::theme::INPUT_FONT_SIZE,
        ))
        .with_padding_right(8.0)
        .finish();
        let prompt_child = prompt_prefix.unwrap_or(fallback_glyph);

        // A view-only shared-session viewer gets a disabled padlock field in
        // place of the live editor — no focus, no typing (see the keydown gate),
        // with a hover tooltip pointing at "Request edit".
        let editor_child = if self.remote_read_only {
            self.render_readonly_input_field(app)
        } else {
            ChildView::new(&self.command_editor).finish()
        };
        let token_hint = self
            .command_editor
            .read(app, |editor, _| editor.token_hint.clone());
        let vim_mode = self
            .command_editor
            .read(app, |editor, _| editor.vim_mode_label());
        let vim_show_cmd = self
            .command_editor
            .read(app, |editor, _| editor.vim_show_cmd().to_string());

        let chips_header = self.render_input_chips_header(app);

        let mut editor_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(prompt_child)
            .with_child(Box::new(Expanded::new(1.0, editor_child)));
        if let Some(mode) = vim_mode {
            editor_row = editor_row.with_child(input_bar::render_vim_mode_badge(
                mode,
                &vim_show_cmd,
                &theme,
                mono,
            ));
        }
        // Mode chip. Three states, cycled by `toggle_lock`:
        //   auto (classifier decides) → locked shell → locked agent → auto
        //
        // The unlocked state MUST be visible. Previously the chip rendered
        // only the detected type, so "auto, currently shell" and "pinned to
        // shell" looked identical — auto-detection appeared not to exist, and
        // there was no sign that clicking had switched it off.
        let is_agent = self.input_config.input_type == DetectedInputType::Agent;
        let locked = self.input_config.is_locked;
        let auto_on = self.editor_settings.input_autodetection_enabled;
        // Keep the trigger compact and let the icon identify the destination:
        // terminal for shell input, agent glyph for agent input. The full
        // auto/locked state remains available in the hover tooltip below.
        let mode_icon = if is_agent {
            crate::icons::AppIcon::Agent
        } else {
            crate::icons::AppIcon::Terminal
        };
        let label = String::new();
        let tooltip = if !auto_on {
            "Auto-detection is off in Settings — click to switch mode".to_string()
        } else if !locked {
            format!(
                "Auto-detecting — currently {}. Click to pin it.",
                if is_agent { "agent" } else { "shell" }
            )
        } else if !is_agent {
            "Pinned to shell — click to pin to agent".to_string()
        } else {
            "Pinned to agent — click to return to auto-detect".to_string()
        };
        // The mode chip is an editing affordance — hidden for a read-only viewer.
        if !self.remote_read_only {
            let mode_indicator_el = Container::new(self.render_status_chip(
                app,
                "chip-input-mode",
                Some(mode_icon),
                label,
                if is_agent {
                    theme.accent
                } else {
                    theme.sub_text()
                },
                &tooltip,
                locked,
                &theme,
                mono,
                Some(TerminalAction::ToggleInputTypeLock),
            ))
            .with_padding_left(6.0)
            .finish();
            editor_row = editor_row.with_child(mode_indicator_el);
        }

        let editor_row = editor_row.finish();

        let mut editor_container_inner =
            Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if let Some(ref notice) = self.input_notice {
            let row = if self.shell_starting_notice {
                input_bar::render_accent_hint_row(notice, &theme, mono)
            } else {
                input_bar::render_token_hint_row(notice, &theme, mono)
            };
            editor_container_inner = editor_container_inner.with_child(row);
        }
        editor_container_inner = editor_container_inner.with_child(editor_row);
        if let Some(hint) = token_hint {
            editor_container_inner = editor_container_inner
                .with_child(input_bar::render_token_hint_row(&hint, &theme, mono));
        }
        if let Some(vim_hint) = self.command_editor.read(app, |editor, _| {
            editor.vim_status_hint().map(str::to_string)
        }) {
            editor_container_inner = editor_container_inner
                .with_child(input_bar::render_token_hint_row(&vim_hint, &theme, mono));
        }

        // Bottom controls row — the Cursor/Claude Code layout: attach (+) and
        // the mode/model triggers hug the bottom-LEFT edge; mic and send hug
        // the bottom-right. The row carries no side padding of its own (the
        // card's 12px insets it) and no bottom padding, so the icons sit close
        // to the card's bottom edge instead of floating above it.
        //
        // AGENT INPUT ONLY: in shell mode every control here (attach, modes,
        // model, mic, send) is an agent affordance, so the whole row is
        // omitted and the shell composer stays a bare prompt.
        if self.input_config.input_type == crate::agent::DetectedInputType::Agent
            && !self.remote_read_only
        {
            let mut controls = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0);
            controls = controls.with_child(input_icon_button(
                crate::icons::AppIcon::Plus,
                if self.pending_attachments.is_empty() {
                    theme.sub_text()
                } else {
                    theme.accent
                },
                TerminalAction::OpenFilePicker,
            ));
            controls = controls.with_child(self.render_mode_selector(app, &theme, mono));
            controls = controls.with_child(self.render_model_selector(app, &theme, mono));
            controls = controls.with_child(self.render_history_panel(app, &theme, mono));
            if let Some(meter) = self.render_context_meter(app, &theme, mono) {
                controls = controls.with_child(meter);
            }
            controls = controls.with_child(Box::new(Expanded::new(
                1.0,
                warpui::elements::Empty::new().finish(),
            )));
            if let Some(mic_btn) = self.render_input_mic_button() {
                controls = controls.with_child(mic_btn);
            }

            // Send / Stop button (agent mode): the Claude Code pattern — a
            // rounded-square salmon button that is ↑ send while idle and a
            // filled-square STOP while the agent works. Stop interrupts the
            // stream AND kills every command the agent has running.
            if self.input_config.input_type == crate::agent::DetectedInputType::Agent {
                // "Busy" = the agent turn is actively streaming (which includes
                // being blocked on a foreground command). Backgrounded commands
                // (dev servers etc.) are deliberately NOT counted — they must not
                // lock the input or turn this into a process-killing Stop button.
                // is_busy, NOT is_streaming: the latter only flips once the
                // server returns a task id, which left a window right after
                // Send where the request was already in flight but the button
                // still offered Send instead of Stop.
                let agent_busy = self.agent_controller.read(app, |c, _| c.is_busy());
                let has_text = !self
                    .command_editor
                    .read(app, |e, _| e.get_text().trim().is_empty());
                controls = controls.with_child(self.render_input_send_button(
                    agent_busy,
                    has_text,
                    TerminalAction::Submit,
                    TerminalAction::StopAgentStream,
                ));
            }
            editor_container_inner = editor_container_inner.with_child(
                Container::new(controls.finish())
                    .with_padding_top(6.0)
                    .finish(),
            );
        }

        // ── The composer ─────────────────────────────────────────────────────
        // Flat and flush (user preference): no card surface, no border, no
        // rounding — the input sits directly on the pane background. Bottom
        // spacing keeps the controls row ~15px above the window bottom
        // (container 4 + margin 8 + the icons' own 3px inner padding).
        let editor_container = EventHandler::new(
            Container::new(editor_container_inner.finish())
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(10.0)
                .with_padding_bottom(4.0)
                .finish(),
        )
        .on_middle_mouse_down(|event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::Paste);
            DispatchEventResult::StopPropagation
        })
        .finish();

        let mut inner_col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        // Dragent's agent input has no folder chip row on top; shell tabs keep it.
        if !self.is_builtin_agent_tab() {
            inner_col = inner_col.with_child(chips_header);
        }

        if !self.attached_blocks.is_empty() {
            let mut attached_row = warpui::elements::Wrap::row()
                .with_main_axis_size(warpui::elements::MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Start)
                .with_spacing(6.0)
                .with_run_spacing(6.0);

            for block in &self.attached_blocks {
                let block_id = block.block_id;
                let pane_view_id = self.pane_view_id;

                let chip_content = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.0)
                    .with_child(crate::icons::render_icon(
                        crate::icons::AppIcon::Terminal,
                        theme.sub_text(),
                        crate::icons::ICON_SIZE_SM,
                    ))
                    .with_child(Box::new(
                        Text::new(format!("Block #{}", block_id), mono, crate::theme::text_size(11.0))
                            .with_color(theme.sub_text()),
                    ))
                    .with_child(crate::icons::render_icon(
                        crate::icons::AppIcon::XClose,
                        theme.disabled_text(),
                        crate::icons::ICON_SIZE_SM,
                    ))
                    .finish();

                let chip = EventHandler::new(input_bar::make_chip(chip_content, &theme))
                    .on_left_mouse_down(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action_for(
                            pane_view_id,
                            TerminalAction::DetachBlock { block_id },
                        );
                        DispatchEventResult::StopPropagation
                    })
                    .finish();

                attached_row = attached_row.with_child(chip);
            }

            let attached_header = Container::new(attached_row.finish())
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_border(Border::top(1.0).with_border_color(theme.outline()))
                .finish();

            inner_col = inner_col.with_child(attached_header);
        }

        let editor_container = match self.render_slash_palette(app, &theme, mono) {
            Some(palette) => Stack::new()
                .with_child(editor_container)
                .with_positioned_overlay_child(
                    palette,
                    OffsetPositioning::offset_from_parent(
                        Vector2F::new(0.0, -6.0),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::TopLeft,
                        ChildAnchor::BottomLeft,
                    ),
                )
                .finish(),
            None => editor_container,
        };
        inner_col = inner_col.with_child(editor_container);

        if self.history_search.open && !command_running {
            let hint = Container::new(input_bar::render_history_search_hint(&theme, mono))
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_bottom(12.0)
                .finish();
            inner_col = inner_col.with_child(hint);
        }

        if self.workflow_search.open && !command_running {
            let hint = Container::new(input_bar::render_workflow_search_hint(&theme, mono))
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_bottom(12.0)
                .finish();
            inner_col = inner_col.with_child(hint);
        }
        if let Some(session) = self.workflow_session.as_ref() {
            let arg_name = session
                .arg_ranges
                .get(session.active_arg)
                .map(|(n, _)| n.as_str())
                .unwrap_or("");
            let hint = Container::new(input_bar::render_workflow_session_hint(
                &session.workflow.name,
                arg_name,
                &theme,
                mono,
            ))
            .with_background_color(theme.pane_surface())
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_bottom(12.0)
            .finish();
            inner_col = inner_col.with_child(hint);
        }

        let input_box = Container::new(inner_col.finish());

        // Background across the whole input bar. On a solid theme this is the
        // opaque pane background; under an image theme it is transparent so the
        // single workspace-level tint shows through here exactly as it does in the
        // output area (stacking a second fill made this bar read darker than the
        // rest of the pane). Nothing is laid out UNDER the input any more — the
        // history popover is sized by its content instead of a fixed height, so it
        // can no longer spill past its band — so a transparent fill has nothing to
        // reveal.
        //
        // No bottom margin: it left a strip of bare pane between the input bar and
        // the window edge, reading as a gap between the bar and the screen. The
        // bar now runs to the bottom edge.
        Container::new(input_box.finish())
            .with_margin_left(0.0)
            .with_margin_right(0.0)
            .with_background_color(theme.pane_surface())
            .finish()
    }

    /// One-line chip surfacing indexer health — shown ONLY when something is
    /// noteworthy (initial pass running, backlog pending, circuit breaker
    /// paused, or the file cap was hit). A healthy quiet index shows nothing:
    /// the terminal should never advertise background machinery that is
    /// behaving. This is the user-visible half of the indexer's
    /// self-protection (see dragent_index::manager).

    /// Compact a command line (or output line) for one-line display: the home
    /// directory becomes `~`, and any remaining long absolute path collapses
    /// to `…/parent/basename`. Only affects presentation — the real command
    /// is untouched everywhere else.
    /// The command's NAME for status displays: program + subcommand only
    /// ("npm test", "cargo build"), never file/dir arguments. Status rows must
    /// stay one short line — a long path argument once distorted the UI into a
    /// one-character-wide column.
    pub(crate) fn command_display_name(command: &str) -> String {
        let mut out: Vec<&str> = Vec::new();
        for token in command.split_whitespace().take(4) {
            // Stop at anything path- or flag-like; keep bare words only.
            if token.contains('/') || token.contains('\\') || token.starts_with('-') {
                break;
            }
            out.push(token);
            if out.len() == 2 {
                break;
            }
        }
        let mut name = out.join(" ");
        if name.is_empty() {
            // First token was path-like (e.g. /usr/local/bin/npm) — use its basename.
            name = command
                .split_whitespace()
                .next()
                .unwrap_or("command")
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("command")
                .to_string();
        }
        if name.len() > 32 {
            crate::agent::tools_background::truncate_utf8(&mut name, 32);
            name.push('…');
        }
        name
    }

    pub(crate) fn shorten_paths_for_display(text: &str) -> String {
        let home = dirs::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default();
        text.split_whitespace()
            .map(|token| {
                let mut t = token.to_string();
                if !home.is_empty() && t.starts_with(&home) {
                    t = format!("~{}", &t[home.len()..]);
                }
                // Collapse still-long paths to their tail.
                if t.len() > 32 && t.contains('/') {
                    let parts: Vec<&str> = t.split('/').filter(|p| !p.is_empty()).collect();
                    if parts.len() > 2 {
                        t = format!("…/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]);
                    }
                }
                t
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// An effort level in plain words: its name, and what it means for the
/// work. None: the agent's own setting.
fn effort_words(level: Option<&str>) -> (String, &'static str) {
    match level {
        None => ("Agent default".into(), "Uses the agent's own setting until you pick a level."),
        Some("minimal") => ("Minimal".into(), "Answers straight away, barely thinking."),
        Some("low") => ("Light".into(), "Quick answers for simple, clear tasks."),
        Some("medium") => ("Balanced".into(), "Good for most tasks."),
        Some("high") => ("Thorough".into(), "Thinks harder and checks its work. Slower."),
        Some("xhigh") => ("Deep".into(), "Careful reasoning for tricky problems."),
        Some("max") => ("Maximum".into(), "The hardest problems. Slowest, uses the most."),
        Some(other) => {
            let mut chars = other.chars();
            let name = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
            (name, "")
        }
    }
}

/// The effort slider itself (see [`TerminalView::render_effort_slider`]).
pub(crate) fn effort_slider(
effort: &dragent_acp::Picker,
shown: &str,
dragging: bool,
track_id: String,
mouse: warpui_core::elements::MouseStateHandle,
theme: &Theme,
mono: FamilyId,
) -> Box<dyn Element> {
    use warpui::elements::{Align, ConstrainedBox, Empty, Expanded, SavePosition, Stack};
    use warpui_core::scene::{CornerRadius, Radius};
    let _ = mono;
    let theme = *theme;
    let ui = mono;
    let at = effort.choices.iter().position(|c| c.id == shown);
    let (name, about) = effort_words(at.map(|_| shown));
    const ROW: f32 = 22.0;
    const RAIL: f32 = 5.0;
    let knob = if dragging { 16.0 } else { 14.0 };
    let last = effort.choices.len().saturating_sub(1);
    let filled_color = theme.accent;
    let empty_color = theme.surface_3();
    let rail = |color: warpui::color::ColorU, round_left: bool, round_right: bool| -> Box<dyn Element> {
        let mut corners = CornerRadius::with_left(Radius::Pixels(if round_left { RAIL / 2.0 } else { 0.0 }));
        corners.merge(CornerRadius::with_right(Radius::Pixels(if round_right { RAIL / 2.0 } else { 0.0 })));
        Expanded::new(
            1.0,
            Align::new(
                Container::new(ConstrainedBox::new(Empty::new().finish()).with_height(RAIL).finish())
                    .with_background_color(color)
                    .with_corner_radius(corners)
                    .finish(),
            )
            .finish(),
        )
        .finish()
    };
    let mut stops = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    for index in 0..=last {
        let before = at.is_some_and(|at| index <= at);
        let after = at.is_some_and(|at| index < at);
        // The rail runs from the first stop's middle to the last's.
        let left: Box<dyn Element> = if index == 0 {
            Expanded::new(1.0, Empty::new().finish()).finish()
        } else {
            rail(if before { filled_color } else { empty_color }, false, false)
        };
        let right: Box<dyn Element> = if index == last {
            Expanded::new(1.0, Empty::new().finish()).finish()
        } else {
            rail(if after { filled_color } else { empty_color }, false, false)
        };
        let halves = ConstrainedBox::new(Flex::row().with_child(left).with_child(right).finish())
            .with_height(ROW)
            .finish();
        let mark: Box<dyn Element> = if at == Some(index) {
            Container::new(ConstrainedBox::new(Empty::new().finish()).with_width(knob).with_height(knob).finish())
                .with_background_color(warpui::color::ColorU::white())
                .with_border(warpui_core::scene::Border::all(2.5).with_border_color(filled_color))
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.0)))
                .finish()
        } else {
            let tick = if before { warpui::color::ColorU::new(255, 255, 255, 170) } else { theme.disabled_text() };
            Container::new(ConstrainedBox::new(Empty::new().finish()).with_width(3.0).with_height(3.0).finish())
                .with_background_color(tick)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(1.5)))
                .finish()
        };
        stops = stops.with_child(
            Expanded::new(
                1.0,
                Stack::new()
                    .with_child(halves)
                    .with_child(ConstrainedBox::new(Align::new(mark).finish()).with_height(ROW).finish())
                    .finish(),
            )
            .finish(),
        );
    }
    let track = SavePosition::new(stops.finish(), &track_id).finish();

    let small = |text: &str, color| Text::new(text.to_string(), ui, crate::theme::text_size(10.0)).with_color(color).finish();
    let heading = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(warpui::elements::MainAxisSize::Max)
        .with_spacing(6.0)
        .with_child(crate::icons::render_icon(crate::icons::AppIcon::Sparkle, theme.sub_text(), 11.0))
        .with_child(
            Expanded::new(
                1.0,
                Text::new("Thinking".to_string(), ui, crate::theme::text_size(11.5)).with_color(theme.sub_text()).finish(),
            )
            .finish(),
        )
        .with_child(
            Text::new(name, ui, crate::theme::text_size(12.0))
                .with_color(if at.is_some() { theme.main_text() } else { theme.sub_text() })
                .with_style(warpui::fonts::Properties::default().weight(warpui::fonts::Weight::Semibold))
                .finish(),
        )
        .finish();
    let ends = Flex::row()
        .with_main_axis_size(warpui::elements::MainAxisSize::Max)
        .with_child(small("Faster", theme.disabled_text()))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(small("Smarter", theme.disabled_text()))
        .finish();
    let section = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(heading)
            .with_child(Container::new(track).with_padding_top(2.0).finish())
            .with_child(ends)
            .with_child(
                Text::new(about.to_string(), ui, crate::theme::text_size(10.5))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .finish(),
    )
    .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(10.0)
    .with_padding_bottom(8.0)
    .with_margin_top(4.0)
    .finish();
    use crate::terminal::view::actions::EffortPointerPhase;
    let pointer = |phase: EffortPointerPhase| {
        move |event_ctx: &mut warpui::EventContext, _: &AppContext, position: warpui::geometry::vector::Vector2F| {
            event_ctx.dispatch_typed_action(TerminalAction::EffortPointer { x: position.x(), phase });
            DispatchEventResult::StopPropagation
        }
    };
    let section = EventHandler::new(section)
        .on_left_mouse_down(pointer(EffortPointerPhase::Press))
        .on_mouse_dragged(pointer(EffortPointerPhase::Drag))
        .on_left_mouse_up(pointer(EffortPointerPhase::Release))
        .finish();
    Hoverable::new(mouse, move |_| section)
        .with_cursor(Cursor::PointingHand)
        .finish()
}

/// The slider at each kind of level, in a menu's width, for the previews.
pub(crate) fn effort_slider_preview(font: FamilyId, theme: &Theme) -> Box<dyn Element> {
    let choices = ["low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .map(|id| dragent_acp::Choice { id: id.into(), name: id.into(), description: None })
        .collect::<Vec<_>>();
    let picker = dragent_acp::Picker { current: String::new(), choices, via: dragent_acp::SetVia::Model };
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(14.0);
    for (shown, dragging) in [("", false), ("medium", false), ("high", true), ("max", false)] {
        column = column.with_child(
            Container::new(effort_slider(&picker, shown, dragging, format!("preview-{shown}"), Default::default(), theme, font))
                .with_background_color(theme.surface_1())
                .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(warpui_core::scene::Radius::Pixels(8.0)))
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_bottom(4.0)
                .finish(),
        );
    }
    Container::new(warpui::elements::ConstrainedBox::new(column.finish()).with_width(260.0).finish())
        .with_background_color(theme.solid_background())
        .with_uniform_padding(16.0)
        .finish()
}
