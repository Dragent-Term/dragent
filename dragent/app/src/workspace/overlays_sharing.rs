//! Sharing and project modals for the workspace.
//!
//! Responsible for: share scope/stop modals, share/ports notices,
//! clone and create-project forms.
//! Blocks here are impl WorkspaceView: same type, same behavior.

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

use super::view::WorkspaceView;

impl WorkspaceView {
    /// Ask the host what a joining participant should see: everything already in
    /// this terminal, or a clean screen. Shown before the session starts, because
    /// the answer decides what gets broadcast and can't be changed later.
    pub(crate) fn render_share_scope_modal(
        &self,
        theme: &Theme,
        tab_id: usize,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;

        // One choice: a title, a line explaining what the participant gets, and
        // the whole card is the click target (no separate confirm step).
        let choice = |title: &'static str,
                      detail: &'static str,
                      key: &'static str,
                      accent: bool,
                      include_scrollback: bool|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("sharescope_{key}"))
                .or_default()
                .clone();
            let theme_row = *theme;
            Box::new(
                Hoverable::new(hover, move |st| {
                    let border = if st.is_hovered() || accent {
                        theme_row.accent
                    } else {
                        theme_row.outline()
                    };
                    let bg = if st.is_hovered() {
                        theme_row.surface_3()
                    } else {
                        theme_row.surface_2()
                    };
                    Container::new(
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_main_axis_size(MainAxisSize::Min)
                            .with_spacing(4.0)
                            .with_child(
                                Text::new(title.to_string(), ui, crate::theme::text_size(13.5))
                                    .with_color(theme_row.main_text())
                                    .finish(),
                            )
                            .with_child(
                                Text::new(detail.to_string(), ui, crate::theme::text_size(12.0))
                                    .with_color(theme_row.sub_text())
                                    .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(11.0)
                    .with_padding_bottom(11.0)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(border))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
                    .finish()
                })
                .with_cursor(warpui::platform::Cursor::PointingHand)
                .on_click(move |ectx, _, _| {
                    ectx.dispatch_typed_action_for(
                        id,
                        WorkspaceAction::ConfirmShareScope {
                            tab_id,
                            include_scrollback,
                        },
                    )
                }),
            )
        };

        let cancel_hover = states
            .borrow_mut()
            .entry("sharescope_cancel".to_string())
            .or_default()
            .clone();
        let theme_c = *theme;
        let cancel: Box<dyn Element> = Box::new(
            Hoverable::new(cancel_hover, move |st| {
                let color = if st.is_hovered() {
                    theme_c.main_text()
                } else {
                    theme_c.sub_text()
                };
                Container::new(
                    Text::new("Cancel".to_string(), ui, crate::theme::text_size(12.5))
                        .with_color(color)
                        .finish(),
                )
                .with_padding_top(6.0)
                .finish()
            })
            .with_cursor(warpui::platform::Cursor::PointingHand)
            .on_click(move |ectx, _, _| {
                ectx.dispatch_typed_action_for(id, WorkspaceAction::CancelShareScope)
            }),
        );

        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(10.0)
                .with_child(
                    Text::new(
                        "Share this terminal".to_string(),
                        ui,
                        crate::theme::text_size(15.0),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .with_child(
                    Text::new(
                        "Choose what people see when they join.".to_string(),
                        ui,
                        crate::theme::text_size(12.5),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .with_child(choice(
                    "Include scrollback",
                    "They see everything already in this terminal, then follow along live.",
                    "with",
                    true,
                    true,
                ))
                .with_child(choice(
                    "Start on a clean screen",
                    "They see nothing from before — only output from now on.",
                    "clean",
                    false,
                    false,
                ))
                .with_child(Align::new(cancel).right().finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(20.0)
        .with_padding_right(20.0)
        .with_padding_top(20.0)
        .with_padding_bottom(18.0)
        .finish();

        let centered = ConstrainedBox::new(dialog).with_max_width(460.0).finish();
        Container::new(Align::new(centered).finish())
            .with_background_color(warpui::color::ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 150,
            })
            .finish()
    }

    pub(crate) fn render_stop_sharing_modal(
        &self,
        theme: &Theme,
        participant_count: usize,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;

        let button = |label: String,
                      color: warpui::color::ColorU,
                      border: warpui::color::ColorU,
                      key: &'static str,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("stopshare_{key}"))
                .or_default()
                .clone();
            Box::new(
                Hoverable::new(hover, move |st| {
                    let bg = if st.is_hovered() {
                        warpui::color::ColorU {
                            r: color.r,
                            g: color.g,
                            b: color.b,
                            a: 28,
                        }
                    } else {
                        warpui::color::ColorU::new(0, 0, 0, 0)
                    };
                    Container::new(
                        Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(border))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish()
                })
                .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
            )
        };

        let buttons = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(10.0)
            .with_child(button(
                "Keep sharing".to_string(),
                theme.sub_text(),
                theme.outline(),
                "cancel",
                WorkspaceAction::CancelStopSharing,
            ))
            .with_child(button(
                "End session".to_string(),
                theme.ui_red(),
                theme.ui_red(),
                "confirm",
                WorkspaceAction::StopSharing,
            ))
            .finish();

        let body = match participant_count {
            0 => "No one has joined yet. Ending the session will close the link.".to_string(),
            1 => "1 person is connected. Ending the session will disconnect them and invalidate the link.".to_string(),
            n => format!(
                "{n} people are connected. Ending the session will disconnect them and invalidate the link."
            ),
        };
        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new("End session?".to_string(), ui, crate::theme::text_size(15.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(body, ui, crate::theme::text_size(13.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(Container::new(buttons).with_padding_top(4.0).finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(20.0)
        .with_padding_right(20.0)
        .with_padding_top(20.0)
        .with_padding_bottom(20.0)
        .finish();

        let centered = ConstrainedBox::new(dialog).with_max_width(440.0).finish();
        Container::new(Align::new(centered).finish())
            .with_background_color(warpui::color::ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 150,
            })
            .finish()
    }
}

impl WorkspaceView {
    pub(crate) fn render_clone_url_form(&self, theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
        let label = |text: &str, size: f32, color| {
            Text::new(text.to_string(), ui, crate::theme::text_size(size))
                .with_color(color)
                .soft_wrap(true)
                .finish()
        };
        let input = |editor| {
            crate::shared_input::shared_input(
                theme,
                editor,
                crate::shared_input::SharedInputOpts {
                    height: Some(38.0),
                    pad_left: 12.0,
                    pad_right: 12.0,
                    ..Default::default()
                },
            )
        };
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(label("Clone repository", 20.0, theme.main_text()))
            .with_child(label(
                "Bring an existing repository into Dragent.",
                13.0,
                theme.sub_text(),
            ))
            .with_child(
                Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_spacing(8.0)
                        .with_child(label("Repository URL", 12.0, theme.main_text()))
                        .with_child(input(&self.clone_url_editor))
                        .finish(),
                )
                .with_padding_top(12.0)
                .finish(),
            )
            .with_child(
                Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_spacing(8.0)
                        .with_child(label("Project directory", 12.0, theme.main_text()))
                        .with_child(
                            Flex::row()
                                .with_spacing(8.0)
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_child(
                                    Expanded::new(1.0, input(&self.clone_location_editor)).finish(),
                                )
                                .with_child(Self::primary_button(
                                    theme,
                                    ui,
                                    self.workspace_view_id,
                                    &self.new_tab_picker_mouse_states,
                                    "clone_browse",
                                    "Browse",
                                    WorkspaceAction::BrowseCloneLocation,
                                ))
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_top(8.0)
                .finish(),
            );
        let destination = super::project_naming::clone_target_name(&self.clone_url_text)
            .map(|name| format!("A new folder named {name} will be created here."))
            .unwrap_or_else(|| "Supports HTTPS and SSH repository URLs.".to_string());
        column = column.with_child(label(&destination, 12.0, theme.sub_text()));

        let footer = if self.clone_busy {
            label(
                "Cloning repository… This may take a few moments.",
                13.0,
                theme.accent,
            )
        } else {
            Flex::row()
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_spacing(8.0)
                .with_child(Self::primary_button(
                    theme,
                    ui,
                    self.workspace_view_id,
                    &self.new_tab_picker_mouse_states,
                    "clone_cancel",
                    "Cancel",
                    WorkspaceAction::CloseAddProjectMenu,
                ))
                .with_child(Self::primary_button(
                    theme,
                    ui,
                    self.workspace_view_id,
                    &self.new_tab_picker_mouse_states,
                    "clone_submit",
                    "Clone repository",
                    WorkspaceAction::SubmitAddProjectClone,
                ))
                .finish()
        };
        column = column.with_child(Container::new(footer).with_padding_top(16.0).finish());
        ConstrainedBox::new(
            Container::new(column.finish())
                .with_padding_left(28.0)
                .with_padding_right(28.0)
                .with_padding_top(26.0)
                .with_padding_bottom(24.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                .finish(),
        )
        .with_width(520.0)
        .finish()
    }

    pub(crate) fn render_create_project_modal(
        &self,
        theme: &Theme,
        ui: FamilyId,
    ) -> Box<dyn Element> {
        let workspace_view_id = self.workspace_view_id;
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(
                                "Create a new project".to_string(),
                                ui,
                                crate::theme::text_size(19.0),
                            )
                            .with_color(theme.main_text())
                            .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        Hoverable::new(
                            Self::add_project_mouse(
                                &self.new_tab_picker_mouse_states,
                                "create_close",
                            ),
                            move |state| {
                                Container::new(render_icon(
                                    AppIcon::XClose,
                                    if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    },
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
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::CloseCreateProject,
                            );
                        })
                        .finish(),
                    )
                    .finish(),
            )
            .with_child(
                Text::new(
                    "Create a blank folder and initialize it as a Git repository.".to_string(),
                    ui,
                    crate::theme::text_size(13.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(
                Text::new("Project name".to_string(), ui, crate::theme::text_size(12.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(crate::shared_input::shared_input(
                theme,
                &self.create_project_name_editor,
                crate::shared_input::SharedInputOpts {
                    width: crate::shared_input::SharedInputWidth::Fill,
                    radius: 8.0,
                    background: Some(theme.surface_2()),
                    border: Some(theme.outline()),
                    pad_left: 10.0,
                    pad_right: 10.0,
                    pad_top: 9.0,
                    pad_bottom: 9.0,
                    ..Default::default()
                },
            ))
            .with_child(
                Text::new("Location".to_string(), ui, crate::theme::text_size(12.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(
                        Expanded::new(
                            1.0,
                            crate::shared_input::shared_input(
                                theme,
                                &self.create_project_location_editor,
                                crate::shared_input::SharedInputOpts {
                                    ..Default::default()
                                },
                            ),
                        )
                        .finish(),
                    )
                    .with_child(
                        Hoverable::new(
                            Self::add_project_mouse(
                                &self.new_tab_picker_mouse_states,
                                "create_browse",
                            ),
                            move |state| {
                                Container::new(render_icon(
                                    AppIcon::FolderOpen,
                                    if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    },
                                    15.0,
                                ))
                                .with_background_color(if state.is_hovered() {
                                    theme.surface_3()
                                } else {
                                    theme.surface_2()
                                })
                                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                                .with_padding_left(10.0)
                                .with_padding_right(10.0)
                                .with_padding_top(9.0)
                                .with_padding_bottom(9.0)
                                .finish()
                            },
                        )
                        .with_cursor(Cursor::PointingHand)
                        .on_click(move |event_ctx, _app, _pos| {
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::BrowseCreateProjectLocation,
                            );
                        })
                        .finish(),
                    )
                    .finish(),
            );

        column = column.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_spacing(10.0)
                .with_child(
                    Hoverable::new(
                        Self::add_project_mouse(&self.new_tab_picker_mouse_states, "create_cancel"),
                        move |state| {
                            Container::new(
                                Text::new("Cancel".to_string(), ui, crate::theme::text_size(13.0))
                                    .with_color(if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    })
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
                        event_ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::CloseCreateProject,
                        );
                    })
                    .finish(),
                )
                .with_child(Self::primary_button(
                    theme,
                    ui,
                    self.workspace_view_id,
                    &self.new_tab_picker_mouse_states,
                    "create_submit",
                    "Create project",
                    WorkspaceAction::SubmitCreateProject,
                ))
                .finish(),
        );
        let modal = ConstrainedBox::new(
            Container::new(column.finish())
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
        EventHandler::new(modal)
            .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
            .finish()
    }

    pub(crate) fn render_new_worktree_modal(
        &self,
        theme: &Theme,
        ui: FamilyId,
    ) -> Box<dyn Element> {
        let workspace_view_id = self.workspace_view_id;
        let project = self
            .worktree_base_root
            .as_ref()
            .map(|root| super::project_naming::worktree_project_name(root))
            .unwrap_or_else(|| "project".to_string());
        let source = self
            .worktree_start_branch
            .as_deref()
            .map(|branch| format!("branch {branch}"))
            .unwrap_or_else(|| "the project's current HEAD".to_string());
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(
                                if self.worktree_rename_branch { "Rename branch" } else { "New worktree" }.to_string(),
                                ui,
                                crate::theme::text_size(19.0),
                            )
                            .with_color(theme.main_text())
                            .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        Hoverable::new(
                            Self::add_project_mouse(
                                &self.new_tab_picker_mouse_states,
                                "worktree_close",
                            ),
                            move |state| {
                                Container::new(render_icon(
                                    AppIcon::XClose,
                                    if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    },
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
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::CloseNewWorktree,
                            );
                        })
                        .finish(),
                    )
                    .finish(),
            )
            .with_child(
                Text::new(
                    if self.worktree_rename_branch {
                        "Rename the checked-out branch on the workspace's machine.".to_string()
                    } else { format!(
                        "Check out a new branch as a subfolder under ~/.dragent/worktrees/{project}/, branched from {source}."
                    ) },
                    ui,
                    crate::theme::text_size(13.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(
                Text::new("Branch name".to_string(), ui, crate::theme::text_size(12.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                crate::shared_input::shared_input(
                    theme,
                    &self.worktree_branch_editor,
                    crate::shared_input::SharedInputOpts {
                        width: crate::shared_input::SharedInputWidth::Fill,
                        radius: 8.0,
                        background: Some(theme.surface_2()),
                        border: Some(theme.outline()),
                        pad_left: 10.0,
                        pad_right: 10.0,
                        pad_top: 9.0,
                        pad_bottom: 9.0,
                        ..Default::default()
                    },
                ),
            );

        column = column.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_spacing(10.0)
                .with_child(
                    Hoverable::new(
                        Self::add_project_mouse(
                            &self.new_tab_picker_mouse_states,
                            "worktree_cancel",
                        ),
                        move |state| {
                            Container::new(
                                Text::new("Cancel".to_string(), ui, crate::theme::text_size(13.0))
                                    .with_color(if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    })
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
                        event_ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::CloseNewWorktree,
                        );
                    })
                    .finish(),
                )
                .with_child(Self::primary_button(
                    theme,
                    ui,
                    self.workspace_view_id,
                    &self.new_tab_picker_mouse_states,
                    "worktree_submit",
                    if self.worktree_busy {
                        if self.worktree_rename_branch { "Renaming…" } else { "Creating…" }
                    } else if self.worktree_rename_branch {
                        "Rename branch"
                    } else {
                        "Create worktree"
                    },
                    WorkspaceAction::SubmitNewWorktree,
                ))
                .finish(),
        );
        let modal = ConstrainedBox::new(
            Container::new(column.finish())
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
        EventHandler::new(modal)
            .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
            .finish()
    }

    pub(crate) fn render_git_init_modal(
        &self,
        theme: &Theme,
        ui: FamilyId,
        pending: &std::path::Path,
    ) -> Box<dyn Element> {
        let workspace_view_id = self.workspace_view_id;
        let path = pending.to_string_lossy().into_owned();
        let column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(
                Text::new(
                    "Not a git repository".to_string(),
                    ui,
                    crate::theme::text_size(19.0),
                )
                .with_color(theme.main_text())
                .finish(),
            )
            .with_child(
                Text::new(
                    format!(
                        "{path} is not a git checkout. Initialize one so branches, worktrees, and source control work?"
                    ),
                    ui,
                    crate::theme::text_size(13.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_spacing(10.0)
                    .with_child(
                        Hoverable::new(
                            Self::add_project_mouse(
                                &self.new_tab_picker_mouse_states,
                                "git_init_cancel",
                            ),
                            move |state| {
                                Container::new(
                                    Text::new(
                                        "Cancel".to_string(),
                                        ui,
                                        crate::theme::text_size(13.0),
                                    )
                                    .with_color(if state.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    })
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
                            event_ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::DismissGitInit,
                            );
                        })
                        .finish(),
                    )
                    .with_child(Self::primary_button(
                        theme,
                        ui,
                        self.workspace_view_id,
                        &self.new_tab_picker_mouse_states,
                        "git_init_confirm",
                        "Initialize git",
                        WorkspaceAction::ConfirmGitInit,
                    ))
                    .finish(),
            );
        let modal = ConstrainedBox::new(
            Container::new(column.finish())
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
        EventHandler::new(modal)
            .on_left_mouse_down(|_event_ctx, _app, _pos| DispatchEventResult::StopPropagation)
            .finish()
    }
}
