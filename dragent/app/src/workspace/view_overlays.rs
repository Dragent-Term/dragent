//! Workspace overlay/modal rendering for WorkspaceView.
//!
//! Responsible for:
//! - modal and confirmation-window rendering (close tab/document, explorer
//!   prompts, hunk discard, access requests, sharing, idle warnings)
//! - explorer prompt state (ExplorerPrompt)
//!
//! Not responsible for:
//! - when overlays open/close (WorkspaceView event handlers)
//! - underlying feature logic (explorer, git, sharing)
//!
//! Methods here are impl WorkspaceView blocks: same type, same behavior,
//! organized by responsibility instead of file position.

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
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label,
    should_replace_project_tab_label, valid_project_name,
};
pub(crate) use super::review_pr_service::{
    query_review_pr_state, query_review_ship_state, CachedReviewPr, ReviewShipState,
};
use super::shape_parse::*;
use super::sidebar::color_from_hex;
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

#[derive(Clone, Debug)]
pub(crate) enum ExplorerPrompt {
    Create {
        relative_parent: PathBuf,
        directory: bool,
    },
    Rename {
        relative_path: PathBuf,
    },
}

impl WorkspaceView {
    /// Full-window terminal state shown after the host ends a joined session.
    /// Keep this independent of the terminal view so the dead mirror cannot
    /// render its normal connecting/empty surface underneath the message.
    pub(crate) fn render_shared_session_ended(
        &self,
        reason: &str,
        theme: &Theme,
        ui: FamilyId,
    ) -> Box<dyn Element> {
        let message = if reason.trim().is_empty() {
            "The host stopped sharing this session.".to_string()
        } else {
            reason.to_string()
        };
        let card = ConstrainedBox::new(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_spacing(16.0)
                    .with_child(
                        ConstrainedBox::new(
                            Container::new(render_icon(AppIcon::Share, theme.sub_text(), 34.0))
                                .with_background_color(theme.surface_2())
                                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(18.0)))
                                .finish(),
                        )
                        .with_width(76.0)
                        .with_height(76.0)
                        .finish(),
                    )
                    .with_child(
                        Text::new("Session ended".to_string(), ui, crate::theme::text_size(24.0))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .with_child(
                        Text::new(message, ui, crate::theme::text_size(14.0))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(32.0)
            .with_padding_right(32.0)
            .with_padding_top(36.0)
            .with_padding_bottom(36.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(18.0)))
            .finish(),
        )
        .with_max_width(560.0)
        .finish();

        Container::new(Align::new(card).finish())
            .with_background_color(theme.background)
            .finish()
    }

    pub(crate) fn refresh_share_snapshot(&mut self, ctx: &mut ViewContext<Self>) {
        // The host row is the local account (its email/name aren't in the session
        // protocol), followed by any remote participants the gateway reports.
        let account = AccountStore::handle(ctx);
        let (host_name, host_email, host_avatar) = ctx.read_model(&account, |s, _| {
            s.data
                .as_ref()
                .map(|d| {
                    let name = if d.user.display_name.is_empty() {
                        "You".to_string()
                    } else {
                        d.user.display_name.clone()
                    };
                    (name, d.user.email.clone(), d.user.avatar_url.clone())
                })
                .unwrap_or_else(|| ("You".to_string(), String::new(), String::new()))
        });

        let open_dropdown = self.share_open_dropdown.clone();
        let sm = SessionManager::handle(ctx);
        self.share_snapshot =
            ctx.read_model(&sm, |m, _| {
                let link_access = match m.link_access() {
                    Some(AccessLevel::Control) => super::action::LinkAccess::Edit,
                    Some(_) => super::action::LinkAccess::View,
                    None => super::action::LinkAccess::None,
                };
                let mut participants = vec![share_sheet::ShareParticipant {
                    id: "__host__".to_string(),
                    name: host_name.clone(),
                    email: host_email.clone(),
                    avatar_url: host_avatar.clone(),
                    control: true,
                    is_host: true,
                }];
                participants.extend(m.participants().iter().map(|p| {
                    share_sheet::ShareParticipant {
                        id: p.id.0.to_string(),
                        name: if p.display_name.is_empty() {
                            "Guest".to_string()
                        } else {
                            p.display_name.clone()
                        },
                        email: String::new(),
                        avatar_url: p.avatar_url.clone(),
                        control: p.role == Role::Controller,
                        is_host: false,
                    }
                }));
                share_sheet::ShareSnapshot {
                    active: m.is_active(),
                    online: m.is_online(),
                    started_label: m.started_label().to_string(),
                    link_access,
                    link_url: m.share_link().map(str::to_string),
                    invite_control: m.invite_control(),
                    participants,
                    open_dropdown,
                    error: m.last_error().map(str::to_string),
                    busy: m.share_busy(),
                    notice: None, // view-owned, re-applied below
                }
            });
        // The snapshot is rebuilt wholesale from the manager, so the transient
        // notice (which the VIEW owns and times out) has to be put back.
        self.share_snapshot.notice = self.share_notice.clone();
    }

    /// Show a short-lived confirmation in the share sheet, cleared automatically.
    /// The epoch guards against an older timer clearing a newer notice.
    pub(crate) fn show_share_notice(
        &mut self,
        text: impl Into<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.share_notice = Some(text.into());
        self.share_notice_epoch = self.share_notice_epoch.wrapping_add(1);
        let epoch = self.share_notice_epoch;
        self.share_snapshot.notice = self.share_notice.clone();
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_millis(2200)),
            move |view, _, ctx| {
                if view.share_notice_epoch == epoch {
                    view.share_notice = None;
                    view.share_snapshot.notice = None;
                    ctx.notify();
                }
            },
        );
        ctx.notify();
    }

    pub(crate) fn show_ports_notice(
        &mut self,
        text: impl Into<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.ports_notice = Some(text.into());
        self.ports_notice_epoch = self.ports_notice_epoch.wrapping_add(1);
        let epoch = self.ports_notice_epoch;
        ctx.spawn(
            warpui::r#async::Timer::after(std::time::Duration::from_millis(2500)),
            move |view, _, ctx| {
                if view.ports_notice_epoch == epoch {
                    view.ports_notice = None;
                    ctx.notify();
                }
            },
        );
        ctx.notify();
    }

    /// Confirmation modal shown when closing a tab that has a running
    /// foreground process (e.g. a dev server).
    /// Window-level mirror of a terminal pane's pending confirm (discard,
    /// quit guard, …). Terminal-owned confirms only paint inside their own
    /// pane's tree, so a discard raised from the git sidebar while the
    /// editor is frontmost would otherwise sit invisibly in a background
    /// tab. The owning pane still clears the request on resolve, and this
    /// mirror only renders for inactive tabs, so the two never double.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_terminal_confirm_window(
        &self,
        theme: &Theme,
        title: &str,
        body: &str,
        confirm_label: &str,
        danger: bool,
        confirm_action: crate::terminal::view::actions::TerminalAction,
        cancel_action: crate::terminal::view::actions::TerminalAction,
        pane_entity: EntityId,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let states = &self.new_tab_picker_mouse_states;
        let confirm_color = if danger { theme.ui_red() } else { theme.accent };
        let button = |label: String,
                      color: warpui::color::ColorU,
                      border: warpui::color::ColorU,
                      key: &'static str,
                      action: crate::terminal::view::actions::TerminalAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("wtermconfirm_{key}"))
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
                .on_click(move |ectx, _, _| {
                    ectx.dispatch_typed_action_for(pane_entity, action.clone())
                }),
            )
        };
        let buttons = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(10.0)
            .with_child(button(
                "Cancel".to_string(),
                theme.sub_text(),
                theme.outline(),
                "cancel",
                cancel_action,
            ))
            .with_child(button(
                confirm_label.to_string(),
                confirm_color,
                confirm_color,
                "confirm",
                confirm_action,
            ))
            .finish();
        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new(title.to_string(), ui, crate::theme::text_size(15.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(body.to_string(), ui, crate::theme::text_size(13.0))
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

    pub(crate) fn render_close_tab_modal(
        &self,
        theme: &Theme,
        tab_id: usize,
        cmd: &str,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;
        let worktree = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.project_root.as_ref())
            .filter(|root| {
                !self
                    .tabs
                    .iter()
                    .any(|tab| tab.id != tab_id && tab.project_root.as_ref() == Some(root))
            })
            .filter(|root| super::project_naming::worktree_main_checkout(root).is_some());
        let is_worktree = worktree.is_some();
        let mut short = cmd.to_string();
        if short.chars().count() > 48 {
            short = short.chars().take(48).collect::<String>();
            short.push('\u{2026}');
        }

        let button = |label: String,
                      color: warpui::color::ColorU,
                      border: warpui::color::ColorU,
                      key: &'static str,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("closetab_{key}"))
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
                "Cancel".to_string(),
                theme.sub_text(),
                theme.outline(),
                "cancel",
                WorkspaceAction::CancelCloseTab,
            ))
            .with_child(button(
                if self.closing_worktree_tab == Some(tab_id) {
                    "Deleting…".to_string()
                } else if is_worktree {
                    "Delete tab & worktree".to_string()
                } else {
                    "Delete tab".to_string()
                },
                theme.ui_red(),
                theme.ui_red(),
                "confirm",
                WorkspaceAction::CloseTabConfirmed { tab_id },
            ))
            .finish();

        let mut body = if let Some(root) = worktree {
            format!("This will delete the tab and its worktree at {}. Uncommitted changes in that worktree will be lost.", root.display())
        } else {
            "This will close the tab and stop its session.".to_string()
        };
        if !cmd.is_empty() {
            body.push_str(&format!(
                " \u{201c}{short}\u{201d} is still running and will be stopped."
            ));
        }

        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new(
                        "Delete this tab?".to_string(),
                        ui,
                        crate::theme::text_size(15.0),
                    )
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

    pub(crate) fn render_close_document_modal(
        &self,
        theme: &Theme,
        file_name: &str,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;
        let button = |label: &'static str,
                      key: &'static str,
                      color: warpui::color::ColorU,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("closedoc_{key}"))
                .or_default()
                .clone();
            Box::new(
                Hoverable::new(hover, move |state| {
                    Container::new(
                        Text::new(label.to_string(), ui, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(if state.is_hovered() {
                        warpui::color::ColorU::new(color.r, color.g, color.b, 28)
                    } else {
                        warpui::color::ColorU::new(0, 0, 0, 0)
                    })
                    .with_border(Border::all(1.0).with_border_color(color))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish()
                })
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())),
            )
        };

        let buttons = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(10.0)
            .with_child(button(
                "Cancel",
                "cancel",
                theme.sub_text(),
                WorkspaceAction::CancelCloseOpenDocument,
            ))
            .with_child(button(
                "Don't Save",
                "discard",
                theme.ui_red(),
                WorkspaceAction::DiscardOpenDocumentAndClose,
            ))
            .with_child(button(
                "Save",
                "save",
                theme.accent,
                WorkspaceAction::SaveOpenDocumentAndClose,
            ))
            .finish();

        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new(
                        format!("Do you want to save the changes you made to {file_name}?"),
                        ui,
                        crate::theme::text_size(15.0),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .with_child(
                    Text::new(
                        "Your changes will be lost if you don't save them.".to_string(),
                        ui,
                        crate::theme::text_size(13.0),
                    )
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

        Container::new(
            Align::new(ConstrainedBox::new(dialog).with_max_width(500.0).finish()).finish(),
        )
        .with_background_color(warpui::color::ColorU::new(0, 0, 0, 150))
        .finish()
    }

    pub(crate) fn render_explorer_prompt_modal(&self, theme: &Theme) -> Box<dyn Element> {
        let Some(prompt) = self.explorer_prompt.as_ref() else {
            return Empty::new().finish();
        };
        let (title, description, confirm_label) = match prompt {
            ExplorerPrompt::Create { directory, .. } if *directory => (
                "New folder",
                "Create a folder in the selected location.",
                "Create",
            ),
            ExplorerPrompt::Create { .. } => (
                "New file",
                "Create a file in the selected location.",
                "Create",
            ),
            ExplorerPrompt::Rename { .. } => (
                "Rename",
                "Enter a new name for this file or folder.",
                "Rename",
            ),
        };
        let confirm_label = if self.explorer_rename_pending {
            "Updating…"
        } else {
            confirm_label
        };
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;
        let button = |label: &'static str,
                      key: &'static str,
                      color: warpui::color::ColorU,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("explorer_prompt_{key}"))
                .or_default()
                .clone();
            Box::new(
                Hoverable::new(hover, move |state| {
                    Container::new(
                        Text::new(label.to_string(), ui, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(if state.is_hovered() {
                        warpui::color::ColorU::new(color.r, color.g, color.b, 28)
                    } else {
                        warpui::color::ColorU::transparent_black()
                    })
                    .with_border(Border::all(1.0).with_border_color(color))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish()
                })
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())),
            )
        };

        let input = crate::shared_input::shared_input(
            theme,
            &self.explorer_prompt_editor,
            crate::shared_input::SharedInputOpts {
                width: crate::shared_input::SharedInputWidth::Fill,
                radius: 8.0,
                border: Some(if self.explorer_operation_error.is_some() {
                    theme.ui_red()
                } else {
                    theme.outline()
                }),
                pad_left: 10.0,
                pad_right: 10.0,
                pad_top: 7.0,
                pad_bottom: 7.0,
                ..Default::default()
            },
        );
        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(10.0)
            .with_child(
                Text::new(title.to_string(), ui, crate::theme::text_size(15.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                Text::new(description.to_string(), ui, crate::theme::text_size(13.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_child(input);

        content = content.with_child(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_spacing(10.0)
                    .with_child(button(
                        "Cancel",
                        "cancel",
                        theme.sub_text(),
                        WorkspaceAction::ExplorerCancelPrompt,
                    ))
                    .with_child(button(
                        confirm_label,
                        "confirm",
                        theme.accent,
                        WorkspaceAction::ExplorerCommitPrompt,
                    ))
                    .finish(),
            )
            .with_padding_top(4.0)
            .finish(),
        );
        let dialog = Container::new(content.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .with_padding_left(20.0)
            .with_padding_right(20.0)
            .with_padding_top(20.0)
            .with_padding_bottom(20.0)
            .finish();
        Container::new(
            Align::new(ConstrainedBox::new(dialog).with_max_width(440.0).finish()).finish(),
        )
        .with_background_color(warpui::color::ColorU::new(0, 0, 0, 150))
        .finish()
    }

    pub(crate) fn render_explorer_delete_modal(&self, theme: &Theme) -> Box<dyn Element> {
        let Some((relative_path, directory)) = self.pending_explorer_delete.as_ref() else {
            return Empty::new().finish();
        };
        let name = relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("this item");
        let item_kind = if *directory { "folder" } else { "file" };
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;
        let button = |label: &'static str,
                      key: &'static str,
                      color: warpui::color::ColorU,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("explorer_delete_{key}"))
                .or_default()
                .clone();
            Box::new(
                Hoverable::new(hover, move |state| {
                    Container::new(
                        Text::new(label.to_string(), ui, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(if state.is_hovered() {
                        warpui::color::ColorU::new(color.r, color.g, color.b, 28)
                    } else {
                        warpui::color::ColorU::transparent_black()
                    })
                    .with_border(Border::all(1.0).with_border_color(color))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish()
                })
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())),
            )
        };
        let buttons = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(10.0)
            .with_child(button(
                "Cancel",
                "cancel",
                theme.sub_text(),
                WorkspaceAction::ExplorerCancelDelete,
            ))
            .with_child(button(
                "Delete",
                "confirm",
                theme.ui_red(),
                WorkspaceAction::ExplorerConfirmDelete,
            ))
            .finish();
        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new(
                        format!("Delete {item_kind} ‘{name}’?"),
                        ui,
                        crate::theme::text_size(15.0),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .with_child(
                    Text::new(
                        "This permanently removes it from disk and cannot be undone.".to_string(),
                        ui,
                        crate::theme::text_size(13.0),
                    )
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
        Container::new(
            Align::new(ConstrainedBox::new(dialog).with_max_width(480.0).finish()).finish(),
        )
        .with_background_color(warpui::color::ColorU::new(0, 0, 0, 150))
        .finish()
    }

    pub(crate) fn render_explorer_error_modal(
        &self,
        theme: &Theme,
        message: &str,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let accent = theme.accent;
        let hover = self
            .new_tab_picker_mouse_states
            .borrow_mut()
            .entry("explorer_error_ok".to_string())
            .or_default()
            .clone();
        let ok = Hoverable::new(hover, move |state| {
            Container::new(
                Text::new("OK".to_string(), ui, crate::theme::text_size(13.0))
                    .with_color(accent)
                    .finish(),
            )
            .with_padding_left(18.0)
            .with_padding_right(18.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .with_background_color(if state.is_hovered() {
                warpui::color::ColorU::new(accent.r, accent.g, accent.b, 28)
            } else {
                warpui::color::ColorU::transparent_black()
            })
            .with_border(Border::all(1.0).with_border_color(accent))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, WorkspaceAction::ExplorerDismissError)
        });
        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new(
                        "File operation failed".to_string(),
                        ui,
                        crate::theme::text_size(15.0),
                    )
                    .with_color(theme.main_text())
                    .finish(),
                )
                .with_child(
                    Text::new(message.to_string(), ui, crate::theme::text_size(13.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(
                    Container::new(
                        Flex::row()
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_main_axis_alignment(MainAxisAlignment::End)
                            .with_child(Box::new(ok))
                            .finish(),
                    )
                    .with_padding_top(4.0)
                    .finish(),
                )
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
        Container::new(
            Align::new(ConstrainedBox::new(dialog).with_max_width(460.0).finish()).finish(),
        )
        .with_background_color(warpui::color::ColorU::new(0, 0, 0, 150))
        .finish()
    }

    pub(crate) fn render_discard_hunk_modal(
        &self,
        theme: &Theme,
        rel_path: &str,
        hunk_index: usize,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;
        let button = |label: &'static str,
                      key: &'static str,
                      color: warpui::color::ColorU,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("discardhunk_{key}"))
                .or_default()
                .clone();
            Box::new(
                Hoverable::new(hover, move |state| {
                    Container::new(
                        Text::new(label.to_string(), ui, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(if state.is_hovered() {
                        warpui::color::ColorU::new(color.r, color.g, color.b, 28)
                    } else {
                        warpui::color::ColorU::new(0, 0, 0, 0)
                    })
                    .with_border(Border::all(1.0).with_border_color(color))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .finish()
                })
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())),
            )
        };
        let dialog = Container::new(
            Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_main_axis_size(MainAxisSize::Min).with_spacing(12.0)
                .with_child(Text::new(format!("Discard changes in {} (hunk {})?", rel_path, hunk_index + 1), ui, crate::theme::text_size(15.0)).with_color(theme.main_text()).finish())
                .with_child(Text::new("This will discard the changes in this hunk from the working tree. This cannot be undone.".to_string(), ui, crate::theme::text_size(13.0)).with_color(theme.sub_text()).finish())
                .with_child(Container::new(
                    Flex::row().with_main_axis_size(MainAxisSize::Max).with_main_axis_alignment(MainAxisAlignment::End).with_spacing(10.0)
                        .with_child(button("Cancel", "cancel", theme.sub_text(), WorkspaceAction::CancelDiscardGitHunk))
                        .with_child(button("Discard", "discard", theme.ui_red(), WorkspaceAction::ConfirmDiscardGitHunk))
                        .finish()
                ).with_padding_top(4.0).finish())
                .finish(),
        ).with_background_color(theme.surface_1()).with_border(Border::all(1.0).with_border_color(theme.outline())).with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0))).with_padding_left(20.0).with_padding_right(20.0).with_padding_top(20.0).with_padding_bottom(20.0).finish();
        Container::new(
            Align::new(ConstrainedBox::new(dialog).with_max_width(500.0).finish()).finish(),
        )
        .with_background_color(warpui::color::ColorU::new(0, 0, 0, 150))
        .finish()
    }

    /// Host prompt: a view-only participant asked to be upgraded to Edit.
    pub(crate) fn render_access_request_modal(
        &self,
        theme: &Theme,
        participant_id: &str,
        name: &str,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.workspace_view_id;
        let states = &self.new_tab_picker_mouse_states;
        let pid = participant_id.to_string();

        let button = |label: String,
                      color: warpui::color::ColorU,
                      border: warpui::color::ColorU,
                      key: &'static str,
                      action: WorkspaceAction|
         -> Box<dyn Element> {
            let hover = states
                .borrow_mut()
                .entry(format!("access_{key}"))
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
                "Decline".to_string(),
                theme.sub_text(),
                theme.outline(),
                "decline",
                WorkspaceAction::ResolveAccessRequest {
                    request_id: pid.clone(),
                    approve: false,
                },
            ))
            .with_child(button(
                "Grant edit".to_string(),
                theme.ui_green(),
                theme.ui_green(),
                "approve",
                WorkspaceAction::ResolveAccessRequest {
                    request_id: pid.clone(),
                    approve: true,
                },
            ))
            .finish();

        let body = format!(
            "{name} is watching your session and is asking for edit access. \
             They will be able to run commands and type in your terminal."
        );
        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new(
                        "Grant edit access?".to_string(),
                        ui,
                        crate::theme::text_size(15.0),
                    )
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

    /// Confirmation before ending a shared session ("End session?").
    /// Inactivity-warning modal: the shared session has been idle 10 minutes and
    /// will auto-end in 5 more unless the host keeps it. "Keep session" resets the
    /// countdown; "End now" ends it immediately (as inactivity).
    pub(crate) fn render_idle_warning_modal(&self, theme: &Theme) -> Box<dyn Element> {
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
                .entry(format!("idlewarn_{key}"))
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
                "End now".to_string(),
                theme.ui_red(),
                theme.ui_red(),
                "end",
                WorkspaceAction::EndIdleSession,
            ))
            .with_child(button(
                "Keep session".to_string(),
                theme.accent,
                theme.accent,
                "keep",
                WorkspaceAction::KeepSessionAlive,
            ))
            .finish();

        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(12.0)
                .with_child(
                    Text::new("Still there?".to_string(), ui, crate::theme::text_size(15.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(
                        "This shared session has been inactive for 10 minutes. It will \
                         automatically end in 5 minutes unless you keep it open."
                            .to_string(),
                        ui,
                        crate::theme::text_size(13.0),
                    )
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
