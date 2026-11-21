//! Overlay layers for workspace rendering.
//!
//! Responsible for: menu popovers, modals, sheets, presence overlays.
//! Each helper threads the root element through independent layers.
//! Blocks moved verbatim; same behavior.

use super::share_sheet;
use super::viewer_bar;
use super::{InputEditor, InputEditorEvent, InputMode, LanguageNavigationCapabilities};
use crate::agent::workspace_execution::{
    next_workspace_launch_action, next_workspace_retry_wait_ms, WorkspaceExecutionRuntime,
    WorkspaceLaunchAction, WorkspaceLaunchPolicy, WorkspacePipelineAction,
    WorkspaceResolvedToolAction, WorkspaceToolRequest,
};
use crate::auth::{AuthManager, AuthManagerEvent};
use crate::realtime::manager::{SessionManager, SessionManagerEvent};
use crate::settings::AccountStore;
use crate::settings::{SettingsEvent, SettingsView};
use crate::setup::UiSettings;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::{
    Theme, DEFAULT_LEFT_PANEL_WIDTH, DEFAULT_RIGHT_PANEL_WIDTH, HEADER_BAR_HEIGHT, MIN_PANEL_WIDTH,
    UI_FONT_SIZE, WORKSPACE_PADDING,
};
use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_session_protocol::{AccessLevel, ParticipantId, Role};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::{
    drag_resize_handle, resizable_state_handle, resizable_state_handle_initialized, Align,
    ChildAnchor, ChildView, ClippedScrollStateHandle, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, DragBarSide, DragResizeAxis, DragResizeElement,
    DragResizeHandle, DropTarget, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    PositionedElementAnchor, PositionedElementOffsetBounds, Resizable, ResizableStateHandle,
    SavePosition, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Entity, EntityId, GetSingletonModelHandle, ReadModel, ReadView, SingletonEntity,
    TypedActionView, UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::Cursor;
use warpui_core::platform::FilePickerConfiguration;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::process_ext::NoWindow;
use dragent_editor::{
    char_index_to_lsp_position, lsp_position_to_char_index, LanguageRequest, LanguageRequestKind,
    TextPosition, TextRange,
};
use dragent_language_services::{
    LanguageServerCatalog, LanguageServerKey, LanguageServiceClient, LanguageServiceClientConfig,
    LanguageServiceEvent, SyncedDocument,
};

use super::pane::WorkspacePane;
use super::pane_layout::{
    LayoutNode, PaneLayout, SplitDirection, SplitId, SplitSide, EDITOR_PANE_ID, MIN_PANE_HEIGHT,
    MIN_PANE_WIDTH,
};
use super::project_sidebar::{
    find_git_root, ProjectSidebarModel, ProjectSidebarView, ReviewPrState,
};
// Re-exported so existing `super::view::ReviewShipState` paths keep working
// during the migration; new code should import from `review_pr_service`.
// Re-exported so existing `super::…` and test paths keep working during the
// migration; new code should import from `project_naming`.
// Glob import during migration: shaping helpers moved to `shape_parse`
// verbatim; call sites below are unchanged.
// Glob import during migration: execution delivery helpers moved verbatim.
use super::execution_delivery::*;
// Glob import during migration: language navigation helpers moved verbatim.
use super::language_nav::*;
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label,
    should_replace_project_tab_label, valid_project_name,
};
pub(crate) use super::review_pr_service::{
    query_review_pr_state, query_review_ship_state, CachedReviewPr, ReviewShipState,
};
use super::shape_parse::*;
use super::sidebar::color_from_hex;
use super::view_overlays::ExplorerPrompt;
use super::{
    chrome_inset, content_header, header, left_panel, pane_layout, persistence, sidebar,
    split_tab_header, DropSide, PaneDropZone, ProjectFolderIntent, SidebarFocus, WorkItemView,
    WorkspaceAction, DEFAULT_SESSION_TITLE,
};
use dragent_persistence::{SerializedPaneLayout, SerializedSplitDirection};
use dragent_workspace_model::{
    task_coordination_thread_id, ActorKind, ActorRef, AgentMessage as WorkspaceAgentMessage,
    AgentMessageId, AgentMessageKind, AgentMessageRouting, AttentionItem, AttentionItemId,
    AttentionKind, AttentionResolution, AttentionStatus, BuiltInLayout, ChargingAuthorization,
    ChargingMode, CommunicationThread, ExecutionAttemptId, LayoutAxis, LayoutChromeState,
    LayoutCollection, LayoutInstance, LayoutPanelState, LayoutScope, MessageDeliveryState, Project,
    ProjectDocumentIndex, SurfaceDescriptor, SurfaceId, SurfaceKind, SurfaceLayoutNode,
    SurfaceRegistry, ThreadId, WorkItemId, WorkStatus, WorkspaceEvent, WorkspaceEventKind,
    WorkspaceState, MAX_LAYOUT_FOCUS_HISTORY,
};
use left_panel::LeftPanelView;
use persistence::{PersistedSession, WindowChrome, WriterHandles};
use uuid::Uuid;

use super::render_picker::{
    picker_mouse, render_add_project_menu, render_header_new_tab_menu, render_new_tab_picker,
};
use super::view::AddProjectMenu;
use super::view::WorkspaceView;

impl WorkspaceView {
    pub(crate) fn render_menu_overlays(
        &self,
        app: &AppContext,
        theme: &Theme,
        ui: FamilyId,
        header_mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
        root_inner: Box<dyn Element>,
    ) -> Box<dyn Element> {
        let root: Box<dyn Element> = if self.new_tab_picker_open {
            let picker = if self.new_tab_picker_from_sidebar {
                render_new_tab_picker(
                    self.workspace_view_id,
                    &theme,
                    self.ui_font,
                    &self.new_tab_picker_mouse_states,
                )
            } else {
                render_header_new_tab_menu(
                    self.workspace_view_id,
                    &theme,
                    self.ui_font,
                    &self.new_tab_picker_mouse_states,
                    self.agentic_workspace_enabled,
                )
            };
            let picker = EventHandler::new(picker)
                .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
                .finish();
            let workspace_view_id = self.workspace_view_id;
            let scrim = Container::new(Align::new(Empty::new().finish()).finish())
                .with_background_color(ColorU::new(0, 0, 0, 0))
                .finish();
            let scrim = EventHandler::new(scrim)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::CloseNewTabPicker,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish();
            let offset = if self.new_tab_picker_from_sidebar {
                OffsetPositioning::offset_from_save_position_element(
                    "new_session_btn",
                    warpui::geometry::vector::vec2f(0.0, 2.0),
                    PositionedElementOffsetBounds::WindowByPosition,
                    PositionedElementAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                )
            } else {
                // Under the header + button.
                OffsetPositioning::offset_from_save_position_element(
                    super::header::NEW_TAB_BUTTON_POSITION,
                    warpui::geometry::vector::vec2f(0.0, 4.0),
                    PositionedElementOffsetBounds::WindowByPosition,
                    PositionedElementAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                )
            };
            let mut stack = Stack::new().with_child(root_inner);
            stack.add_overlay_child(scrim);
            Box::new(stack.with_positioned_overlay_child(picker, offset))
        } else {
            root_inner
        };

        // Add-project popover, anchored under the sidebar add button —
        // same overlay mechanics as the new-tab picker above.
        let root: Box<dyn Element> = match self.add_project_menu {
            AddProjectMenu::Closed => root,
            menu => {
                let content = match menu {
                    AddProjectMenu::Closed => Empty::new().finish(),
                    AddProjectMenu::Add => render_add_project_menu(
                        self.workspace_view_id,
                        &theme,
                        ui,
                        &self.new_tab_picker_mouse_states,
                    ),
                    AddProjectMenu::CloneForm => self.render_clone_url_form(&theme, ui),
                };
                let workspace_view_id = self.workspace_view_id;
                let is_clone = menu == AddProjectMenu::CloneForm;
                let content = EventHandler::new(content)
                    .on_keydown(move |event_ctx, _app, keystroke| {
                        if is_clone && matches!(keystroke.key.as_str(), "enter" | "escape") {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                if keystroke.key == "enter" {
                                    WorkspaceAction::SubmitAddProjectClone
                                } else {
                                    WorkspaceAction::CloseAddProjectMenu
                                },
                            );
                            return DispatchEventResult::StopPropagation;
                        }
                        DispatchEventResult::PropagateToParent
                    })
                    .on_left_mouse_down(|_event_ctx, _app, _pos| {
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                let workspace_view_id = self.workspace_view_id;
                let scrim = Container::new(Align::new(Empty::new().finish()).finish())
                    .with_background_color(ColorU::new(
                        0,
                        0,
                        0,
                        if menu == AddProjectMenu::CloneForm {
                            140
                        } else {
                            0
                        },
                    ))
                    .finish();
                let scrim = EventHandler::new(scrim)
                    .on_left_mouse_down(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::CloseAddProjectMenu,
                        );
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                let offset = if menu == AddProjectMenu::CloneForm {
                    OffsetPositioning::offset_from_parent(
                        warpui::geometry::vector::vec2f(0.0, 0.0),
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::Center,
                        ChildAnchor::Center,
                    )
                } else {
                    OffsetPositioning::offset_from_save_position_element(
                        "projects_add_btn",
                        warpui::geometry::vector::vec2f(0.0, 4.0),
                        PositionedElementOffsetBounds::WindowByPosition,
                        PositionedElementAnchor::BottomLeft,
                        ChildAnchor::TopLeft,
                    )
                };
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(scrim);
                Box::new(stack.with_positioned_overlay_child(content, offset))
            }
        };

        // Add Project dialog (Local or a remote machine).
        let root: Box<dyn Element> = match self.render_add_project(theme, app) {
            Some(dialog) => Stack::new().with_child(root).with_child(dialog).finish(),
            None => root,
        };

        let root: Box<dyn Element> = if let Some(project) = self.project_context_menu_root.as_ref()
        {
            let menu = super::project_context::render_project_context_menu(
                self.workspace_view_id,
                theme,
                ui,
                project,
                &self.new_tab_picker_mouse_states,
            );
            let menu = ConstrainedBox::new(menu).with_width(240.0).finish();
            let menu = EventHandler::new(menu)
                .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                .finish();
            let workspace_id = self.workspace_view_id;
            let scrim = EventHandler::new(
                Container::new(Align::new(Empty::new().finish()).finish()).finish(),
            )
            .on_left_mouse_down(move |event_ctx, _, _| {
                event_ctx.dispatch_typed_action_for(
                    workspace_id,
                    WorkspaceAction::CloseProjectContextMenu,
                );
                DispatchEventResult::StopPropagation
            })
            .finish();
            let offset = OffsetPositioning::offset_from_save_position_element(
                &format!("project_header_{}", project.display()),
                warpui::geometry::vector::vec2f(8.0, 2.0),
                PositionedElementOffsetBounds::WindowByPosition,
                PositionedElementAnchor::BottomLeft,
                ChildAnchor::TopLeft,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(scrim);
            Box::new(stack.with_positioned_overlay_child(menu, offset))
        } else {
            root
        };

        let root: Box<dyn Element> = if self.new_project_group_root.is_some() {
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(
                EventHandler::new(self.render_project_group_dialog(theme, ui))
                    .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                    .finish(),
            );
            Box::new(stack)
        } else {
            root
        };
        let root: Box<dyn Element> = if self.importable_project_worktrees.is_some() {
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(
                EventHandler::new(self.render_project_import_dialog(theme, ui))
                    .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                    .finish(),
            );
            Box::new(stack)
        } else {
            root
        };
        let root: Box<dyn Element> = if self.pending_project_removal.is_some() {
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(
                EventHandler::new(self.render_project_removal_dialog(theme, ui))
                    .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                    .finish(),
            );
            Box::new(stack)
        } else {
            root
        };

        // Centered create-project modal over a dimmed backdrop.
        let root: Box<dyn Element> = if self.create_project_open {
            let modal = self.render_create_project_modal(&theme, ui);
            let workspace_view_id = self.workspace_view_id;
            let scrim = Container::new(Align::new(Empty::new().finish()).finish())
                .with_background_color(ColorU::new(0, 0, 0, 140))
                .finish();
            let scrim = EventHandler::new(scrim)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::CloseCreateProject,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish();
            let offset = OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(0.0, 0.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(scrim);
            Box::new(stack.with_positioned_overlay_child(modal, offset))
        } else {
            root
        };

        // Account picker modal for rail launches — same modal language as
        // create-project (19px title + X, dimmed scrim, centered card).
        // Picking an option launches with that account. Closing without a
        // pick launches nothing; the shared resolver falls back to System
        // login, and typing the command by hand always uses System login.
        let root: Box<dyn Element> = if let Some(agent_id) = self.agent_account_picker.clone() {
            let definition = super::agent_definition::definition_by_id(
                &agent_id,
                &self.cached_custom_agents,
                theme.is_dark,
            );
            let agent_name = definition
                .as_ref()
                .map(|d| d.name.clone())
                .unwrap_or_else(|| agent_id.clone());
            let project_root = self.active_project_root();
            let project_name = project_root.as_ref()
                .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string));
            // A remote project lists the accounts stored on its machine
            // (cached per machine, refreshed when the picker opens).
            let remote = project_root.as_deref().and_then(crate::remote_fs::resolve);
            let registry = if remote.is_none() {
                dragent_host::agent_accounts::Registry::load().ok()
            } else {
                None
            };
            let effective = match remote.as_ref() {
                Some(resolved) => {
                    let cache = self.machine_account_caches.get(&resolved.machine_id);
                    cache
                        .and_then(|c| c.defaults.get(&agent_id))
                        .map(|selection| match selection {
                            dragent_host::agent_accounts::Selection::Profile(id) => cache
                                .and_then(|c| c.profiles.iter().find(|p| &p.id == id))
                                .map(|p| p.label.clone())
                                .unwrap_or_else(|| "System login".to_string()),
                            _ => "System login".to_string(),
                        })
                        .unwrap_or_else(|| "System login".to_string())
                }
                None => registry.as_ref().and_then(|r| {
                    let project = project_root.as_deref().and_then(|p| {
                        dragent_host::agent_accounts::project_for(p, r).or_else(|| Some(p.to_path_buf()))
                    });
                    r.resolve(&agent_id, project.as_deref(), &dragent_host::agent_accounts::Selection::Inherit)
                        .ok()
                        .map(|b| b.label)
                }).unwrap_or_else(|| "System login".to_string()),
            };
            let workspace_view_id = self.workspace_view_id;

            // Header: title + close, same as the create-project modal.
            let header = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(format!("Start {agent_name}"), ui, crate::theme::text_size(19.0))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Hoverable::new(
                        picker_mouse(&self.new_tab_picker_mouse_states, "account-pick-close".to_string()),
                        move |state| {
                            Container::new(render_icon(
                                AppIcon::XClose,
                                if state.is_hovered() { theme.main_text() } else { theme.sub_text() },
                                14.0,
                            ))
                            .with_padding_left(8.0)
                            .with_padding_right(8.0)
                            .with_padding_top(8.0)
                            .with_padding_bottom(8.0)
                            .finish()
                        },
                    )
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::CloseAgentAccountPicker);
                    })
                    .finish(),
                )
                .finish();
            let subtitle = match project_name {
                Some(name) => format!("New session in {name} — pick which login it uses. Tabs already running keep theirs."),
                None => "Pick which login the new session uses. Tabs already running keep theirs.".to_string(),
            };

            // Options: project default (shows what it resolves to), System,
            // then each saved profile. Click launches immediately.
            let mut options: Vec<(String, String, bool, dragent_host::agent_accounts::Selection)> = vec![
                ("Project default".to_string(), format!("Currently {effective}"), true, dragent_host::agent_accounts::Selection::Inherit),
                ("System login".to_string(), "Your current CLI sign-in — nothing to set up".to_string(), false, dragent_host::agent_accounts::Selection::System),
            ];
            if let Some(resolved) = remote.as_ref() {
                if let Some(cache) = self.machine_account_caches.get(&resolved.machine_id) {
                    let mut profiles: Vec<_> = cache.profiles.iter().filter(|p| p.provider == agent_id).collect();
                    profiles.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
                    for p in profiles {
                        let desc = if p.kind == dragent_host::agent_accounts::CredentialKind::ApiKey {
                            "API key · Saved login for this agent".to_string()
                        } else {
                            "Saved login for this agent".to_string()
                        };
                        options.push((p.label.clone(), desc, false, dragent_host::agent_accounts::Selection::Profile(p.id)));
                    }
                }
            } else if let Some(r) = registry.as_ref() {
                let mut profiles: Vec<_> = r.profiles.iter().filter(|p| p.provider == agent_id).collect();
                profiles.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
                for p in profiles {
                    options.push((p.label.clone(), "Saved login for this agent".to_string(), false, dragent_host::agent_accounts::Selection::Profile(p.id)));
                }
            }
            let mut list = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min);
            for (i, (label, desc, is_default, selection)) in options.into_iter().enumerate() {
                if i > 0 {
                    list = list.with_child(Container::new(Empty::new().finish())
                        .with_border(Border::top(1.0).with_border_color(theme.outline()))
                        .finish());
                }
                let ms = picker_mouse(&self.new_tab_picker_mouse_states, format!("account-pick-{agent_id}-{i}"));
                list = list.with_child(account_option_row(
                    theme, ui, self.workspace_view_id, ms,
                    &agent_id, &label, &desc, is_default, selection,
                ));
            }
            let options_card = Container::new(list.finish())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                .finish();
            // Remote fetch state: loading text while empty, error text on
            // failure. The System login option above still works.
            let remote_status: Option<String> = remote.as_ref().and_then(|resolved| {
                let cache = self.machine_account_caches.get(&resolved.machine_id);
                match cache {
                    Some(c) if c.loading && c.profiles.is_empty() => {
                        Some("Loading accounts…".to_string())
                    }
                    Some(c) if c.error.is_some() => {
                        let name = if c.machine_name.is_empty() {
                            resolved.machine_id.clone()
                        } else {
                            c.machine_name.clone()
                        };
                        Some(format!("Couldn't read accounts on {name}"))
                    }
                    None => Some("Loading accounts…".to_string()),
                    _ => None,
                }
            });

            // Footer: hand-typed note left, Cancel + Manage right.
            let footer_view = self.workspace_view_id;
            let footer = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new("Typing the command by hand always uses System login.".to_string(), ui, crate::theme::text_size(11.0))
                            .with_color(theme.disabled_text())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(10.0)
                        .with_child(
                            Hoverable::new(
                                picker_mouse(&self.new_tab_picker_mouse_states, "account-pick-cancel".to_string()),
                                move |state| {
                                    Container::new(
                                        Text::new("Cancel".to_string(), ui, crate::theme::text_size(13.0))
                                            .with_color(if state.is_hovered() { theme.main_text() } else { theme.sub_text() })
                                            .finish(),
                                    )
                                    .with_padding_left(12.0)
                                    .with_padding_right(12.0)
                                    .with_padding_top(9.0)
                                    .with_padding_bottom(9.0)
                                    .finish()
                                },
                            )
                            .with_cursor(Cursor::PointingHand)
                            .on_click(move |event_ctx, _app, _pos| {
                                event_ctx.dispatch_typed_action_for(footer_view, WorkspaceAction::CloseAgentAccountPicker);
                            })
                            .finish(),
                        )
                        .with_child({
                            let footer_view = self.workspace_view_id;
                            Hoverable::new(
                                picker_mouse(&self.new_tab_picker_mouse_states, "account-pick-manage".to_string()),
                                move |state| {
                                    Container::new(
                                        Flex::row()
                                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                            .with_spacing(8.0)
                                            .with_child(render_icon(AppIcon::Settings, theme.main_text(), 14.0))
                                            .with_child(
                                                Text::new("Manage accounts".to_string(), ui, crate::theme::text_size(13.0))
                                                    .with_color(theme.main_text())
                                                    .finish(),
                                            )
                                            .finish(),
                                    )
                                    .with_background_color(if state.is_hovered() { theme.surface_3() } else { theme.surface_2() })
                                    .with_border(Border::all(1.0).with_border_color(theme.outline()))
                                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                                    .with_padding_left(12.0)
                                    .with_padding_right(12.0)
                                    .with_padding_top(9.0)
                                    .with_padding_bottom(9.0)
                                    .finish()
                                },
                            )
                            .with_cursor(Cursor::PointingHand)
                            .on_click(move |event_ctx, _app, _pos| {
                                // Single dispatch: `OpenSettingsSection` also
                                // clears `agent_account_picker` (see
                                // `handle_org_menu_action`), so Settings opens
                                // on top with Agent Accounts selected. The old
                                // Close+Open double dispatch could drop the
                                // second action and leave Settings closed.
                                event_ctx.dispatch_typed_action_for(footer_view, WorkspaceAction::OpenSettingsSection(crate::settings::SettingsSection::AgentAccounts));
                            })
                            .finish()
                        })
                        .finish(),
                )
                .finish();

            let mut picker_column = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(12.0)
                .with_child(header)
                .with_child(Text::new(subtitle, ui, crate::theme::text_size(13.0)).with_color(theme.sub_text()).finish())
                .with_child(options_card);
            if let Some(status) = remote_status {
                picker_column = picker_column.with_child(
                    Text::new(status, ui, crate::theme::text_size(12.0)).with_color(theme.sub_text()).finish(),
                );
            }
            picker_column = picker_column.with_child(footer);
            let modal = ConstrainedBox::new(
                Container::new(
                    picker_column.finish(),
                )
                .with_padding_left(24.0)
                .with_padding_right(24.0)
                .with_padding_top(20.0)
                .with_padding_bottom(20.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                .finish(),
            )
            .with_width(480.0)
            .finish();
            let modal = EventHandler::new(modal)
                .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                .finish();
            let scrim = Container::new(Align::new(Empty::new().finish()).finish())
                .with_background_color(ColorU::new(0, 0, 0, 140))
                .finish();
            let scrim = EventHandler::new(scrim)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::CloseAgentAccountPicker);
                    DispatchEventResult::StopPropagation
                })
                .finish();
            let offset = OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(0.0, 0.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(scrim);
            Box::new(stack.with_positioned_overlay_child(modal, offset))
        } else {
            root
        };

        // Centered new-worktree modal over a dimmed backdrop.
        let root: Box<dyn Element> = if self.worktree_prompt_open {
            let modal = self.render_new_worktree_modal(&theme, ui);
            let workspace_view_id = self.workspace_view_id;
            let scrim = Container::new(Align::new(Empty::new().finish()).finish())
                .with_background_color(ColorU::new(0, 0, 0, 140))
                .finish();
            let scrim = EventHandler::new(scrim)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::CloseNewWorktree,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish();
            let offset = OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(0.0, 0.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(scrim);
            Box::new(stack.with_positioned_overlay_child(modal, offset))
        } else {
            root
        };

        #[cfg(not(target_family="wasm"))]
        let root: Box<dyn Element> = if let Some(review)=&self.review_setup {
            let modal=super::project_prompt::render_review_setup(self.workspace_view_id,&theme,ui,
                &self.new_tab_picker_mouse_states,review,&self.review_instructions_editor,self.prompt_agent,
                &crate::teams::models::display_label(self.prompt_agent,self.prompt_lead_model.as_deref(),app),
                self.project_start_busy,self.project_start_error.as_deref());
            let id=self.workspace_view_id;
            let scrim=EventHandler::new(Container::new(Align::new(Empty::new().finish()).finish())
                .with_background_color(ColorU::new(0,0,0,150)).finish())
                .on_left_mouse_down(move |ctx,_,_| {ctx.dispatch_typed_action_for(id,WorkspaceAction::CancelReviewSetup);DispatchEventResult::StopPropagation}).finish();
            let mut stack=Stack::new().with_child(root);stack.add_overlay_child(scrim);
            stack.with_positioned_overlay_child(modal,OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(0.0,0.0),ParentOffsetBounds::WindowByPosition,ParentAnchor::Center,ChildAnchor::Center)).finish()
        } else {root};

        // Prompt-screen dropdown menus (agent / device / worktree / branch).
        let root: Box<dyn Element> = if let Some(menu) = self.prompt_menu {
            // A remote project's agents are the ones on its machine.
            let machine = self.machine_agents(self.prompt_context_root().as_deref(), app);
            let (installed_agents, agent_checking) = match &machine {
                Some(machine) => (machine.agents(), !machine.checked),
                None => (self.prompt_installed_agents.clone(), self.prompt_agent_checking),
            };
            let card = super::project_prompt::render_prompt_menu(
                self.workspace_view_id,
                &theme,
                ui,
                &self.new_tab_picker_mouse_states,
                &self.prompt_menu_scroll,
                menu,
                self.prompt_agent,
                (self.prompt_team, &self.prompt_team_agents, self.prompt_team_cost),
                &crate::teams::models::choices(self.prompt_agent, app),
                self.prompt_lead_model.as_deref(),
                Some(crate::teams::models::prompt_panel(self.prompt_agent, &installed_agents, app, self.prompt_model_tab)),
                &installed_agents,
                agent_checking,
                &self
                    .open_project_roots
                    .last()
                    .and_then(|root| crate::remote_fs::machine_name(root))
                    .unwrap_or_else(super::project_prompt::device_label),
                self.prompt_device_remote_open,
                &{
                    let store = <crate::settings::machines_store::MachinesStore as warpui::SingletonEntity>::handle(app);
                    let mut machines: Vec<(String, String)> = store
                        .as_ref(app)
                        .registry
                        .machines
                        .iter()
                        .map(|m| (m.id.clone(), m.name()))
                        .collect();
                    machines.sort_by_key(|(_, name)| name.to_lowercase());
                    machines
                },
                &self.project_start_recent_projects,
                self.open_project_roots.last(),
                self.prompt_selected_worktree.as_ref(),
                &self.prompt_branches,
                self.prompt_branch.as_deref(),
                &self.prompt_branch_search_editor,
                &self.prompt_branch_search,
                &self.prompt_agent_search_editor,
                &self.prompt_agent_search,
                &self.prompt_link_search_editor,
                &self.prompt_links,
                &self.prompt_link_results,
                self.prompt_link_loading,
                self.prompt_link_error.as_deref(),
                self.prompt_link_query.trim().is_empty(),
                &super::project_links::LinkKind::items()
                    .filter(|kind| crate::integrations::is_connected(kind.provider()))
                    .collect::<Vec<_>>(),
            );
            let workspace_view_id = self.workspace_view_id;
            let card = warpui_core::elements::Dismiss::new(card)
                .on_dismiss(move |event_ctx, _app| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::ClosePromptMenus,
                    );
                })
                .prevent_interaction_with_other_elements()
                .finish();
            let anchor = match menu {
                super::action::PromptMenu::Agent => "prompt_agent_chip",
                super::action::PromptMenu::Team => "prompt_team_chip",
                super::action::PromptMenu::LeadModel => "prompt_lead_model",
                super::action::PromptMenu::Device => "prompt_device",
                super::action::PromptMenu::Project => "prompt_project",
                super::action::PromptMenu::Worktree => "prompt_worktree",
                super::action::PromptMenu::Branch => "prompt_branch",
                super::action::PromptMenu::Add | super::action::PromptMenu::Links(_) => {
                    "prompt_add"
                }
            };
            let offset = OffsetPositioning::offset_from_save_position_element(
                anchor,
                warpui::geometry::vector::vec2f(0.0, if menu == super::action::PromptMenu::LeadModel { -6.0 } else { 6.0 }),
                PositionedElementOffsetBounds::WindowByPosition,
                if menu == super::action::PromptMenu::LeadModel { PositionedElementAnchor::TopRight } else { PositionedElementAnchor::BottomLeft },
                if menu == super::action::PromptMenu::LeadModel { ChildAnchor::BottomRight } else { ChildAnchor::TopLeft },
            );
            Box::new(
                Stack::new()
                    .with_child(root)
                    .with_positioned_overlay_child(card, offset),
            )
        } else {
            root
        };

        // Agent installation runs in a centered embedded-terminal modal. The
        // picker is closed while it is open, so the install sheet owns focus.
        // Built-ins and customs share the modal; only the title differs.
        // Remote tools (Settings → Machines) use the same sheet, with the
        // installer's terminal on the machine.
        const AGENT_INSTALL: &str =
            "Dragent runs the agent's official installer in this embedded terminal. You can watch its output here.";
        // (heading, subject, description, checking, retry with sudo)
        let install: Option<(String, String, String, bool, bool)> = if let Some((title, description)) = &self.page_command {
            Some((title.clone(), String::new(), description.clone(), false, false))
        } else if let Some(remote) = &self.remote_tool_install {
            let subject = remote
                .job
                .tools
                .iter()
                .map(|tool| crate::settings::tool_catalog::label(tool))
                .collect::<Vec<_>>()
                .join(", ");
            Some((
                remote.job.title.clone(),
                subject,
                format!(
                    "Dragent runs this on {} in an embedded terminal. Answer any prompts, such as a sudo password, right here.",
                    remote.machine
                ),
                self.remote_tool_checking,
                remote.can_retry,
            ))
        } else if let Some(agent) = self.prompt_install_agent {
            let label = agent.label().to_string();
            Some((format!("Install {label}"), label, AGENT_INSTALL.to_string(), self.prompt_agent_checking, false))
        } else {
            self.custom_install_agent.as_ref().map(|custom| {
                let label = custom.name.clone();
                (format!("Install {label}"), label, AGENT_INSTALL.to_string(), self.prompt_agent_checking, false)
            })
        };
        let root: Box<dyn Element> = if let Some((heading, subject, description, checking, retry)) = install {
            let modal = super::project_prompt::render_embedded_terminal_modal(
                self.workspace_view_id,
                &theme,
                ui,
                &self.new_tab_picker_mouse_states,
                &heading,
                &subject,
                &description,
                self.prompt_install_terminal.as_ref().map(|terminal| ChildView::new(terminal).finish()),
                checking,
                self.prompt_install_error.as_deref(),
                retry,
                self.page_command.is_none(),
            );
            let workspace_view_id = self.workspace_view_id;
            let scrim = Container::new(Align::new(Empty::new().finish()).finish())
                .with_background_color(ColorU::new(0, 0, 0, 150))
                .finish();
            let scrim = EventHandler::new(scrim)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::ClosePromptAgentInstall,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish();
            let offset = OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(0.0, 0.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(scrim);
            Box::new(stack.with_positioned_overlay_child(modal, offset))
        } else {
            root
        };

        // Git-init confirm: the picked folder is not a checkout yet.
        let root: Box<dyn Element> = if let Some(pending) = self.pending_git_init_root.clone() {
            let modal = self.render_git_init_modal(&theme, ui, &pending);
            let workspace_view_id = self.workspace_view_id;
            let scrim = Container::new(Align::new(Empty::new().finish()).finish())
                .with_background_color(ColorU::new(0, 0, 0, 140))
                .finish();
            let scrim = EventHandler::new(scrim)
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::DismissGitInit,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish();
            let offset = OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(0.0, 0.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(scrim);
            Box::new(stack.with_positioned_overlay_child(modal, offset))
        } else {
            root
        };

        // The "..." tab context menu is owned by LeftPanelView, but rendered
        // here (WorkspaceView's own top-level window Stack) rather than
        // nested inside LeftPanelView's render tree — the sidebar is its own
        // bounded child view, and a positioned overlay anchored there ended
        // up clipped/blended incorrectly (looked partially transparent).
        let root: Box<dyn Element> = {
            let lp_ref = self.left_panel_view.as_ref(app);
            match lp_ref
                .sidebar_menu_tab_id
                .and_then(|id| lp_ref.tabs.iter().find(|t| t.id == id))
            {
                Some(tab) => {
                    let panel_width = lp_ref
                        .panel_state
                        .lock()
                        .ok()
                        .map(|s| s.size())
                        .unwrap_or(260.0);
                    let (menu, scrim, offset) = left_panel::render_tab_menu_overlay(
                        lp_ref.ui_font,
                        &lp_ref.theme,
                        self.workspace_view_id,
                        tab,
                        lp_ref
                            .projects
                            .iter()
                            .find(|project| {
                                Some(project.key.as_str()) == tab.project_key.as_deref()
                            })
                            .map(|project| project.groups.as_slice())
                            .unwrap_or(&[]),
                        &lp_ref.sidebar_mouse_states,
                        panel_width,
                    );
                    let mut stack = Stack::new().with_child(root);
                    stack.add_overlay_child(scrim);
                    Box::new(stack.with_positioned_overlay_child(menu, offset))
                }
                None => root,
            }
        };

        // Content-header ⋮ share popover, anchored under the button via SavePosition.
        // Split panes anchor to their own ⋮ button; the main header uses the
        // active tab. Either way the menu acts on the owning tab.
        let share_menu_tab_id = self
            .split_share_menu_tab_id
            .or_else(|| self.content_header_menu_open.then_some(self.active_tab_id));
        let root = if let Some(menu_tab_id) = share_menu_tab_id {
            match self
                .tabs
                .iter()
                .find(|t| t.id == menu_tab_id)
                .filter(|t| !t.pane.is_settings() && !t.pane.is_browser())
            {
                Some(tab) => {
                    let sharing = {
                        let sm = SessionManager::handle(app);
                        let shared =
                            sm.read(app, |m, _| if m.is_host() { m.shared_pane() } else { None });
                        shared == Some(tab.pane.pane_uuid(app))
                    };
                    let anchor_tag = if Some(tab.id) == self.split_share_menu_tab_id {
                        split_tab_header::split_share_menu_tag(tab.id)
                    } else {
                        content_header::SHARE_MENU_TAG.to_string()
                    };
                    let (menu, catcher, offset) = content_header::share_menu_overlay(
                        sharing,
                        tab.id,
                        &theme,
                        ui,
                        self.workspace_view_id,
                        &self.header_mouse_states,
                        &anchor_tag,
                    );
                    let mut stack = Stack::new().with_child(root);
                    stack.add_overlay_child(catcher);
                    Box::new(stack.with_positioned_overlay_child(menu, offset)) as Box<dyn Element>
                }
                None => root,
            }
        } else {
            root
        };

        // Workspace layout menu, anchored under the stable header switcher.
        let root = if self.layout_menu_open {
            let (menu, catcher, offset) = header::layout_menu_overlay(
                self.active_builtin_layout(),
                &theme,
                ui,
                self.workspace_view_id,
                header_mouse_state,
                self.agentic_workspace_enabled,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(catcher);
            Box::new(stack.with_positioned_overlay_child(menu, offset)) as Box<dyn Element>
        } else {
            root
        };

        // Navbar Review chevron menu, anchored under the Review button.
        let root = if self.review_menu_open {
            let (menu, catcher, offset) = header::review_menu_overlay(
                &theme,
                ui,
                self.workspace_view_id,
                header_mouse_state,
                &self.review_ship,
                self.review_action_busy,
                self.review_action_notice.as_deref(),
                None,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(catcher);
            Box::new(stack.with_positioned_overlay_child(menu, offset)) as Box<dyn Element>
        } else {
            root
        };

        // Navbar attention bell, anchored under the bell. Lists open
        // requests across all work items, then unread automation results;
        // selecting a request shows its detail card with the same actions as
        // the Coordinate rail, and routing lands on the request's work item
        // and screen.
        let root = if self.attention_bell_open {
            let entries = self.open_attention_entries();
            let selected = self.attention_popover_item;
            #[cfg(not(target_family = "wasm"))]
            let automations = self.automation_notices(app);
            #[cfg(target_family = "wasm")]
            let automations = Vec::new();
            let (menu, catcher, offset) = header::attention_menu_overlay(
                &entries,
                &automations,
                selected,
                &self.attention_response_editor,
                &self.attention_response_text.clone(),
                &theme,
                ui,
                self.workspace_view_id,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(catcher);
            Box::new(stack.with_positioned_overlay_child(menu, offset)) as Box<dyn Element>
        } else {
            root
        };

        // Past sessions: a centred list over the whole window.
        #[cfg(not(target_family = "wasm"))]
        let root = if self.past_sessions_open {
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(super::past_sessions::past_sessions_overlay(super::past_sessions::PastSessionsProps {
                sessions: &self.past_sessions,
                loading: self.past_sessions_loading,
                theme: &theme,
                ui,
                view_id: self.workspace_view_id,
                states: &self.header_mouse_states,
                scroll: &self.past_sessions_scroll,
                search_editor: &self.past_sessions_search_editor,
                search: &self.past_sessions_search,
                confirm: self.past_sessions_confirm,
                busy: self.past_sessions_busy.as_deref(),
                note: self.past_sessions_note.as_deref(),
                reclaimable: &self.past_sessions_reclaimable,
            }));
            stack.finish()
        } else {
            root
        };

        // Header account menu, anchored under the avatar.
        let root = if self.account_menu_open {
            let is_free_tier = {
                use crate::settings::AccountStore;
                let store = AccountStore::handle(app);
                app.read_model(&store, |s: &AccountStore, _| {
                    s.data
                        .as_ref()
                        .map(|d| {
                            d.user.tier.as_str() != "pro"
                                && d.user.tier.as_str() != "ultra"
                                && d.user.tier.as_str() != "mega"
                        })
                        .unwrap_or(true)
                })
            };
            let (menu, catcher, offset) = header::account_menu_overlay(
                is_free_tier,
                crate::autoupdate::update_state(),
                &theme,
                ui,
                self.workspace_view_id,
                header_mouse_state,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(catcher);
            Box::new(stack.with_positioned_overlay_child(menu, offset)) as Box<dyn Element>
        } else {
            root
        };

        // Header usage popover, anchored under the Usage button.
        #[cfg(not(target_family = "wasm"))]
        let root = if self.usage_menu_open {
            let agents = super::header_usage::popover_agents(&self.prompt_installed_agents, &self.agent_usage);
            let (menu, catcher, offset) = super::header_usage::usage_menu_overlay(
                &agents,
                self.usage_menu_agent,
                &self.agent_usage,
                self.agent_usage_loading,
                &theme,
                ui,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
                &self.usage_tabs_scroll,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(catcher);
            Box::new(stack.with_positioned_overlay_child(menu, offset)) as Box<dyn Element>
        } else {
            root
        };

        // Sidebar organisation area: the organisation menu (with its open
        // submenu) or the Help menu, anchored above their footer controls.
        let root = if self.org_menu_open || self.help_menu_open {
            let states = &self.new_tab_picker_mouse_states;
            let mut stack = Stack::new().with_child(root);
            if self.org_menu_open {
                let summary = super::org_menu::OrgSummary::read(app);
                let (catcher, menus) = super::org_menu::org_menu_overlays(
                    &summary,
                    self.org_submenu,
                    &theme,
                    ui,
                    self.workspace_view_id,
                    states,
                );
                stack.add_overlay_child(catcher);
                for (menu, offset) in menus {
                    stack = stack.with_positioned_overlay_child(menu, offset);
                }
            } else {
                let (catcher, menu, offset) =
                    super::org_menu::help_menu_overlay(&theme, ui, self.workspace_view_id, states);
                stack.add_overlay_child(catcher);
                stack = stack.with_positioned_overlay_child(menu, offset);
            }
            Box::new(stack) as Box<dyn Element>
        } else {
            root
        };

        // Host: participants popover (from the presence stack) + per-participant
        // access sub-menu. Both anchor to SavePosition tags in the content header.
        let root = if self.participants_menu_open {
            let (menu, catcher, offset) = content_header::participants_menu_overlay(
                &self.share_snapshot.participants,
                &theme,
                ui,
                self.workspace_view_id,
                &self.header_mouse_states,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(catcher);
            stack = stack.with_positioned_overlay_child(menu, offset);
            if let Some(pid) = self.participant_access_open.clone() {
                if let Some(p) = self
                    .share_snapshot
                    .participants
                    .iter()
                    .find(|p| p.id == pid)
                {
                    let (amenu, acatcher, aoffset) = content_header::participant_access_overlay(
                        p,
                        &theme,
                        ui,
                        self.workspace_view_id,
                        &self.header_mouse_states,
                    );
                    stack.add_overlay_child(acatcher);
                    stack = stack.with_positioned_overlay_child(amenu, aoffset);
                }
            }
            Box::new(stack) as Box<dyn Element>
        } else {
            root
        };

        root
    }
}

impl WorkspaceView {
    pub(crate) fn render_sheet_overlays(
        &self,
        app: &AppContext,
        theme: &Theme,
        ui: FamilyId,
        root: Box<dyn Element>,
    ) -> (Box<dyn Element>, Option<&std::path::PathBuf>) {
        let root = if let Some((tab_id, cmd)) = self.pending_close_tab.clone() {
            let modal = self.render_close_tab_modal(&theme, tab_id, &cmd);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if let Some((surface_id, summary)) = self.pending_close_workspace_tab.clone() {
            let display_name = self
                .pending_close_project_workspace
                .and_then(|id| self.workspaces.iter().find(|workspace| workspace.id == id))
                .map(|workspace| workspace.title.clone())
                .unwrap_or_else(|| self.surface_chip(app, surface_id).label);
            let modal =
                self.render_close_workspace_tab_modal(&theme, surface_id, &summary, &display_name);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if self.agents_panel_open {
            let modal = self.render_agents_modal(&theme);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        // Terminal-owned confirms only paint inside their pane's tree, so a
        // discard raised from the git sidebar (or the quit guard) while any
        // other screen is frontmost would otherwise sit invisibly in a
        // background tab. Mirror the first background tab's pending confirm
        // — or the quit guard when no terminal tab is active — at window
        // level. The owning pane still clears the request on resolve, and
        // the mirror hides for the active tab, so the two never double.
        let terminal_confirm_mirror: Option<(
            EntityId,
            String,
            String,
            String,
            bool,
            crate::terminal::view::actions::TerminalAction,
            crate::terminal::view::actions::TerminalAction,
        )> = {
            use crate::terminal::view::actions::TerminalAction;
            // The window mirror is the single renderer for terminal-owned
            // confirms (in-pane rendering is disabled): first pending
            // request across tab panes, then docked terminals.
            let background = self
                .tabs
                .iter()
                .find_map(|tab| match &tab.pane {
                    WorkspacePane::Terminal(handle) => handle
                        .read(app, |view, _| view.pending_confirm_request())
                        .map(|req| (handle.id(), req)),
                    _ => None,
                })
                .or_else(|| {
                    self.document_terminals
                        .iter()
                        .find_map(|tab| match &tab.pane {
                            WorkspacePane::Terminal(handle) => handle
                                .read(app, |view, _| view.pending_confirm_request())
                                .map(|req| (handle.id(), req)),
                            _ => None,
                        })
                });
            if let Some((pane_entity, req)) = background {
                let confirm = req
                    .on_confirm
                    .clone()
                    .map(|action| TerminalAction::ResolveConfirm {
                        confirm: true,
                        action: Box::new(action),
                    })
                    .unwrap_or(TerminalAction::DismissConfirm);
                Some((
                    pane_entity,
                    req.title,
                    req.body,
                    req.confirm_label,
                    req.danger,
                    confirm,
                    TerminalAction::DismissConfirm,
                ))
            } else {
                None
            }
        };
        let root = if let Some((
            pane_entity,
            title,
            body,
            confirm_label,
            danger,
            confirm_action,
            cancel_action,
        )) = terminal_confirm_mirror
        {
            let modal = self.render_terminal_confirm_window(
                &theme,
                &title,
                &body,
                &confirm_label,
                danger,
                confirm_action,
                cancel_action,
                pane_entity,
            );
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let pending_document_ready = self.pending_close_document.as_ref().filter(|pending| {
            self.project_sidebar_model.read(app, |model, _| {
                model.active_relative_path().as_ref() == Some(pending)
            })
        });
        let root = if let Some(pending_path) = pending_document_ready {
            let file_name = pending_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("this file")
                .to_string();
            let modal = self.render_close_document_modal(&theme, &file_name);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if let Some((rel_path, hunk_index, _, _)) = self.pending_discard_hunk.clone() {
            let modal = self.render_discard_hunk_modal(&theme, &rel_path, hunk_index);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if matches!(self.explorer_prompt, Some(ExplorerPrompt::Rename { .. })) {
            let modal = self.render_explorer_prompt_modal(&theme);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if self.pending_explorer_delete.is_some() {
            let modal = self.render_explorer_delete_modal(&theme);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if let Some((pid, name)) = self.pending_access_request.clone() {
            let modal = self.render_access_request_modal(&theme, &pid, &name);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        // Share-scope chooser: shown before a session starts, so it must sit above
        // the workspace but below the share sheet it leads into.
        let root = if let Some(tab_id) = self.share_scope_tab {
            let modal = self.render_share_scope_modal(&theme, tab_id);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if self.stop_confirm_open {
            let count = {
                let sm = SessionManager::handle(app);
                sm.read(app, |m, _| m.participants().len())
            };
            let modal = self.render_stop_sharing_modal(&theme, count);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if self.agent_sleep_notice.is_some() {
            self.overlay_agent_sleep(root, &theme)
        } else {
            root
        };

        let root = if self.idle_warning_open {
            let modal = self.render_idle_warning_modal(&theme);
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(modal);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if self.share_open {
            let sheet = share_sheet::render_share_sheet(
                &self.share_snapshot,
                &theme,
                self.ui_font,
                self.mono_font,
                &self.share_email_editor,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            );
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(sheet);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if self.share_open && self.share_qr_open {
            let qr = share_sheet::render_qr_sheet(
                &self.share_snapshot,
                &theme,
                self.ui_font,
                self.share_qr_asset.as_deref(),
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            );
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(qr);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if let Some(draft) = &self.feedback {
            let dialog = super::feedback::render_feedback_modal(
                draft,
                &theme,
                self.ui_font,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            );
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(dialog);
            Box::new(stack) as Box<dyn Element>
        } else {
            root
        };

        #[cfg(not(target_family = "wasm"))]
        let root = if let Some(ask) = self.computer_asks.front() {
            let states = &self.new_tab_picker_mouse_states;
            let modal = super::computer_tools::render_computer_ask(
                ask,
                self.computer_asks.len(),
                &theme,
                self.ui_font,
                self.workspace_view_id,
                &|key| {
                    states
                        .borrow_mut()
                        .entry(key.to_string())
                        .or_default()
                        .clone()
                },
                &self.computer_ask_scroll,
            );
            let modal = EventHandler::new(modal)
                .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                .finish();
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(modal);
            Box::new(stack) as Box<dyn Element>
        } else {
            root
        };

        let root = if self.logout_confirm_open {
            let states = &self.new_tab_picker_mouse_states;
            let modal = crate::settings::logout_confirm::logout_confirm_modal(
                &theme,
                self.ui_font,
                self.workspace_view_id,
                app,
                self.logout_dont_ask,
                |key| {
                    states
                        .borrow_mut()
                        .entry(format!("workspace-{key}"))
                        .or_default()
                        .clone()
                },
                crate::settings::logout_confirm::LogoutConfirmActions {
                    toggle_dont_ask: WorkspaceAction::ToggleLogoutDontAsk,
                    cancel: WorkspaceAction::CancelSignOut,
                    confirm: WorkspaceAction::ConfirmSignOut,
                },
            );
            // Nothing behind the dialog takes clicks while it is open.
            let modal = EventHandler::new(modal)
                .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
                .finish();
            let mut stack = Stack::new().with_child(root);
            stack.add_overlay_child(modal);
            Box::new(stack) as Box<dyn Element>
        } else {
            root
        };

        let root = if self.login_prompt_open {
            let prompt = super::auth_modals::render_login_prompt(
                &theme,
                self.ui_font,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            );
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(prompt);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        let root = if let Some(folder) = self.pending_workspace_trust.as_ref() {
            let prompt = super::trust_prompt::render(
                folder,
                &theme,
                self.ui_font,
                self.workspace_view_id,
                &self.new_tab_picker_mouse_states,
            );
            {
                let mut stack = Stack::new().with_child(root);
                stack.add_overlay_child(prompt);
                Box::new(stack) as Box<dyn Element>
            }
        } else {
            root
        };

        // Host collaborator presence sits above the entire shared window. Each
        // pointer is avatar-headed with a thin stem ending at the exact remote
        // mouse position; normalized wire coordinates are mapped to this window.
        let root: Box<dyn Element> = if self.remote_pointers.is_empty() {
            root
        } else if let Some(bounds) = app.window_bounds(&self.window_id) {
            let size = bounds.size();
            let mut stack = Stack::new().with_child(root);
            for pointer in self.remote_pointers.values() {
                let avatar = share_sheet::avatar_sized(
                    &pointer.display_name,
                    "",
                    &pointer.avatar_url,
                    &theme,
                    ui,
                    18.0,
                );
                let stem = Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(2.0)
                        .with_height(20.0)
                        .finish(),
                )
                .with_background_color(theme.accent)
                .finish();
                let cursor = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(1.0)
                    .with_child(avatar)
                    .with_child(stem)
                    .finish();
                let offset = OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(
                        pointer.position.x.clamp(0.0, 1.0) * size.x(),
                        pointer.position.y.clamp(0.0, 1.0) * size.y(),
                    ),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::BottomMiddle,
                );
                stack = stack.with_positioned_overlay_child(cursor, offset);
            }
            Box::new(stack)
        } else {
            root
        };

        let root = self.overlay_error_notice(root);
        (root, pending_document_ready)
    }
}

/// One account option in the rail launch picker: two-line label plus a
/// Default pill on the effective choice. Click launches immediately.
fn account_option_row(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    agent_id: &str,
    label: &str,
    desc: &str,
    is_default: bool,
    selection: dragent_host::agent_accounts::Selection,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let desc_owned = desc.to_string();
    let agent_owned = agent_id.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let bg = if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_main_axis_size(MainAxisSize::Min)
                            .with_child(
                                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.5))
                                    .with_color(theme.main_text())
                                    .finish(),
                            )
                            .with_child(
                                Container::new(
                                    Text::new(desc_owned.clone(), ui_font, crate::theme::text_size(12.0))
                                        .with_color(theme.sub_text())
                                        .finish(),
                                )
                                .with_padding_top(2.0)
                                .finish(),
                            )
                            .finish(),
                    )
                    .finish(),
                );
            if is_default {
                row = row.with_child(
                    Container::new(
                        Text::new("Default".to_string(), ui_font, crate::theme::text_size(11.0))
                            .with_color(theme.accent)
                            .finish(),
                    )
                    .with_background_color(theme.accent_bg())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(3.0)
                    .with_padding_bottom(3.0)
                    .finish(),
                );
            }
            Container::new(row.finish())
                .with_padding_left(14.0)
                .with_padding_right(14.0)
                .with_padding_top(11.0)
                .with_padding_bottom(11.0)
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                view_id,
                WorkspaceAction::OpenAgentWithAccount { agent_id: agent_owned.clone(), account: selection.clone() },
            );
        }),
    )
}
