//! Lightweight editor tab chrome. Document buffers stay in the project model;
//! this module renders only paths, dirty state, and navigation actions.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dragent_editor::FileKind;
use dragent_workspace_model::{WorkItemId, WorkStatus};
use warpui::elements::{
    AcceptedByDropTarget, Align, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DispatchEventResult, DragAxis, Draggable, DraggableState,
    DropTarget, DropTargetData, Element, EventHandler, Expanded, Flex, Image, MainAxisSize,
    ParentElement, SavePosition, ScrollbarWidth, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::image_cache::CacheOption;
use warpui_core::platform::Cursor;
use warpui_core::scene::Border;
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::ide_chrome::{self, IdeMetrics};
use super::WorkspaceAction;

pub struct HeaderTab {
    pub path: PathBuf,
    /// Optional presentation label for virtual documents such as Git diffs.
    /// The path remains the stable tab identity; the label is only chrome.
    pub label: Option<String>,
    pub active: bool,
    pub dirty: bool,
    /// Work items share the editor tab strip without pretending to be files.
    /// `path` above is only a stable local chrome key for these virtual tabs.
    pub work_item: Option<(WorkItemId, WorkStatus)>,
    pub agent: Option<super::action::WorkspaceAgentTabId>,
}

pub(crate) struct AgentEditorTab {
    pub id: super::action::WorkspaceAgentTabId,
    pub expanded: bool,
    pub scroll: ClippedScrollStateHandle,
}

pub(super) fn agent_tab_index(
    tabs: &[AgentEditorTab],
    id: super::action::WorkspaceAgentTabId,
) -> Option<usize> {
    tabs.iter().position(|tab| tab.id == id)
}

pub(super) fn agent_tab_chrome_key(id: super::action::WorkspaceAgentTabId) -> PathBuf {
    PathBuf::from(format!(
        ".dragent-agent/{}/{}/{}",
        id.work_item_id.0,
        id.task_id.0,
        id.attempt_id
            .map(|attempt| attempt.0.to_string())
            .unwrap_or_else(|| "task".into())
    ))
}

pub fn position_id(path: &Path) -> String {
    format!("editor-tab:{}", path.display())
}

/// Returns the clean preview slot that may be reused for an explorer open.
/// Dirty previews are permanent tabs and must never be replaced implicitly.
pub(super) fn preview_replacement_index(
    open_paths: &[PathBuf],
    preview_path: Option<&Path>,
    preview_is_dirty: bool,
) -> Option<usize> {
    if preview_is_dirty {
        return None;
    }
    preview_path.and_then(|preview| open_paths.iter().position(|path| path == preview))
}

#[derive(Clone, Debug)]
struct EditorTabDropTarget {
    path: PathBuf,
}

impl DropTargetData for EditorTabDropTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn render(
    tabs: Vec<HeaderTab>,
    theme: &Theme,
    editor_font: FamilyId,
    view_id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    drag_states: &RefCell<HashMap<PathBuf, DraggableState>>,
    scroll_state: ClippedScrollStateHandle,
) -> Box<dyn Element> {
    let live_paths = tabs
        .iter()
        .map(|tab| tab.path.clone())
        .collect::<std::collections::HashSet<_>>();
    drag_states
        .borrow_mut()
        .retain(|path, _| live_paths.contains(path));

    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(0.0);

    for tab in tabs {
        let path = tab.path;
        let active = tab.active;
        let dirty = tab.dirty;
        let label = tab.label.unwrap_or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Untitled")
                .to_string()
        });
        let asset = FileKind::from_path(&path).icon_asset();
        let work_item = tab.work_item;
        let agent = tab.agent;
        let key = format!("editor_tab:{}", path.display());
        let mouse = states.borrow_mut().entry(key).or_default().clone();
        let theme = *theme;
        let select_path = path.clone();
        let close_path = path.clone();
        let source_path = path.clone();
        let accepted_source_path = source_path.clone();
        let ghost_label = label.clone();
        let drag_state = drag_states
            .borrow_mut()
            .entry(path.clone())
            .or_default()
            .clone();
        let tab_element = Hoverable::new(mouse, move |state| {
            let hovered = state.is_hovered();
            let color = if active || hovered {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            let trailing = if hovered {
                EventHandler::new(
                    ConstrainedBox::new(
                        Align::new(render_icon(AppIcon::XClose, color, 12.0)).finish(),
                    )
                    .with_width(IdeMetrics::TAB_TRAILING)
                    .with_height(IdeMetrics::TAB_TRAILING)
                    .finish(),
                )
                .on_left_mouse_down({
                    let close_path = close_path.clone();
                    move |ctx, _, _| {
                        if let Some(id) = agent {
                            ctx.dispatch_typed_action_for(
                                view_id,
                                WorkspaceAction::CloseWorkspaceAgentTab(id),
                            );
                        } else if work_item.is_some() {
                            ctx.dispatch_typed_action_for(
                                view_id,
                                WorkspaceAction::CloseWorkItemTab,
                            );
                        } else {
                            ctx.dispatch_typed_action_for(
                                view_id,
                                WorkspaceAction::CloseDocumentTab {
                                    relative_path: close_path.clone(),
                                },
                            );
                        }
                        DispatchEventResult::StopPropagation
                    }
                })
                .finish()
            } else if dirty {
                // Same 20px trailing slot as the close button: a dirty tab
                // keeps the shared 34px bar height instead of shrinking.
                ConstrainedBox::new(
                    Align::new(
                        Text::new("●".to_string(), editor_font, crate::theme::text_size(10.0))
                            .with_color(theme.ui_yellow())
                            .finish(),
                    )
                    .finish(),
                )
                .with_width(IdeMetrics::TAB_TRAILING)
                .with_height(IdeMetrics::TAB_TRAILING)
                .finish()
            } else {
                ConstrainedBox::new(warpui::elements::Empty::new().finish())
                    .with_width(IdeMetrics::TAB_TRAILING)
                    .with_height(IdeMetrics::TAB_TRAILING)
                    .finish()
            };
            let leading = if work_item.is_some() || agent.is_some() {
                ConstrainedBox::new(render_icon(AppIcon::Agent, theme.accent, 13.0))
                    .with_width(IdeMetrics::TAB_ICON)
                    .with_height(IdeMetrics::TAB_ICON)
                    .finish()
            } else {
                ConstrainedBox::new(
                    Image::new(AssetSource::Bundled { path: asset }, CacheOption::BySize)
                        .contain()
                        .finish(),
                )
                .with_width(IdeMetrics::TAB_ICON)
                .with_height(IdeMetrics::TAB_ICON)
                .finish()
            };
            let content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(IdeMetrics::TAB_GAP)
                .with_child(leading)
                .with_child(
                    Expanded::new(
                        1.0,
                        Clipped::new(
                            Text::new(label.clone(), editor_font, crate::theme::text_size(12.0))
                                .with_color(color)
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish(),
                )
                .with_child(trailing)
                .finish();
            let background = ide_chrome::tab_background(&theme, active, hovered);
            // File tabs never paint the accent rail: the active file reads
            // from the tab background alone. The top geometry is still
            // reserved so every tab keeps one height and selecting a tab
            // never shifts its label.
            let rail = if active && (work_item.is_some() || agent.is_some()) {
                ide_chrome::tab_active_rail(&theme, true)
            } else {
                warpui::color::ColorU::new(0, 0, 0, 0)
            };
            let body = Container::new(content)
                .with_padding_left(IdeMetrics::TAB_PAD_LEFT)
                .with_padding_right(IdeMetrics::TAB_PAD_RIGHT)
                .with_padding_top(IdeMetrics::TAB_PAD_Y)
                .with_padding_bottom(IdeMetrics::TAB_PAD_Y)
                .with_background_color(background)
                .with_border(Border::top(IdeMetrics::TAB_ACTIVE_RAIL).with_border_color(rail))
                .finish();
            // The divider is a separate outer edge: it stays one pixel, spans
            // the complete tab height, and remains visible for the sole tab.
            Container::new(body)
                .with_background_color(background)
                .with_border(Border::right(1.0).with_border_color(ide_chrome::tab_divider(&theme)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            if let Some(id) = agent {
                ctx.dispatch_typed_action_for(view_id, WorkspaceAction::SelectWorkspaceAgentTab(id))
            } else if work_item.is_some() {
                // The work-item surface currently has no implicit tab-click
                // entry point. A dedicated launcher will own that transition.
            } else {
                ctx.dispatch_typed_action_for(
                    view_id,
                    WorkspaceAction::SelectOpenDocument {
                        relative_path: select_path.clone(),
                    },
                )
            }
        });
        if agent.is_some() || work_item.is_some() {
            row = row.with_child(
                ConstrainedBox::new(
                    SavePosition::new(Box::new(tab_element), &position_id(&path)).finish(),
                )
                .with_width(IdeMetrics::TAB_WIDTH)
                .finish(),
            );
            continue;
        }
        let ghost_leading = if work_item.is_some() {
            ConstrainedBox::new(render_icon(AppIcon::Agent, theme.accent, 13.0))
                .with_width(IdeMetrics::TAB_ICON)
                .with_height(IdeMetrics::TAB_ICON)
                .finish()
        } else {
            ConstrainedBox::new(
                Image::new(AssetSource::Bundled { path: asset }, CacheOption::BySize)
                    .contain()
                    .finish(),
            )
            .with_width(IdeMetrics::TAB_ICON)
            .with_height(IdeMetrics::TAB_ICON)
            .finish()
        };
        let ghost = ConstrainedBox::new(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_spacing(IdeMetrics::TAB_GAP)
                    .with_child(ghost_leading)
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(ghost_label, editor_font, crate::theme::text_size(12.0))
                                .with_color(theme.main_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(IdeMetrics::TAB_PAD_LEFT)
            .with_padding_right(IdeMetrics::TAB_PAD_LEFT)
            .with_padding_top(IdeMetrics::TAB_PAD_Y)
            .with_padding_bottom(IdeMetrics::TAB_PAD_Y)
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.accent))
            .finish(),
        )
        .with_width(IdeMetrics::TAB_WIDTH)
        .finish();

        let draggable = Draggable::new(drag_state, Box::new(tab_element))
            .with_drag_axis(DragAxis::HorizontalOnly)
            .with_keep_original_visible(true)
            .with_alternate_drag_element(ghost)
            .with_accepted_by_drop_target_fn(move |data, _| {
                match data.as_any().downcast_ref::<EditorTabDropTarget>() {
                    Some(target) if target.path != accepted_source_path => {
                        AcceptedByDropTarget::Yes
                    }
                    _ => AcceptedByDropTarget::No,
                }
            })
            .on_drop(move |ctx, _, _, data| {
                let Some(target) =
                    data.and_then(|value| value.as_any().downcast_ref::<EditorTabDropTarget>())
                else {
                    return;
                };
                if target.path != source_path {
                    ctx.dispatch_typed_action_for(
                        view_id,
                        WorkspaceAction::ReorderDocumentTab {
                            source_path: source_path.clone(),
                            target_path: target.path.clone(),
                        },
                    );
                }
            })
            .finish();

        row = row.with_child(
            ConstrainedBox::new(
                SavePosition::new(
                    DropTarget::new(draggable, EditorTabDropTarget { path: path.clone() }).finish(),
                    &position_id(&path),
                )
                .finish(),
            )
            .with_width(IdeMetrics::TAB_WIDTH)
            .finish(),
        );
    }

    let transparent = warpui::color::ColorU::new(0, 0, 0, 0);
    // Bare scrollable: the host bar (editor pane header) owns the background
    // and bottom border, so the tabs never paint a doubled edge.
    // Zero scrollbar padding: `Scrollable` otherwise reserves 2+2px below
    // the row even with `ScrollbarWidth::None`, leaving an empty strip
    // between the file cards and the navbar divider.
    Container::new(
        ClippedScrollable::horizontal(
            scroll_state,
            row.finish(),
            ScrollbarWidth::None,
            transparent.into(),
            transparent.into(),
            transparent.into(),
        )
        .with_padding_start(0.0)
        .with_padding_end(0.0)
        .finish(),
    )
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_tabs_reuse_only_the_same_work_task_and_attempt() {
        use super::super::action::WorkspaceAgentTabId;
        use dragent_workspace_model::{ExecutionAttemptId, PlanTaskId};

        let id = WorkspaceAgentTabId {
            work_item_id: WorkItemId::new(),
            task_id: PlanTaskId::new(),
            attempt_id: Some(ExecutionAttemptId::new()),
        };
        let tabs = vec![AgentEditorTab {
            id,
            expanded: true,
            scroll: ClippedScrollStateHandle::new(),
        }];
        assert_eq!(agent_tab_index(&tabs, id), Some(0));
        assert!(tabs[agent_tab_index(&tabs, id).unwrap()].expanded);
        for other in [
            WorkspaceAgentTabId {
                work_item_id: WorkItemId::new(),
                ..id
            },
            WorkspaceAgentTabId {
                task_id: PlanTaskId::new(),
                ..id
            },
            WorkspaceAgentTabId {
                attempt_id: Some(ExecutionAttemptId::new()),
                ..id
            },
            WorkspaceAgentTabId {
                attempt_id: None,
                ..id
            },
        ] {
            assert_eq!(agent_tab_index(&tabs, other), None);
            assert_ne!(agent_tab_chrome_key(id), agent_tab_chrome_key(other));
        }
    }

    #[test]
    fn clean_preview_reuses_its_existing_slot() {
        let paths = vec![PathBuf::from("src/lib.rs"), PathBuf::from("README.md")];
        assert_eq!(
            preview_replacement_index(&paths, Some(Path::new("README.md")), false),
            Some(1)
        );
    }

    #[test]
    fn dirty_preview_is_promoted_instead_of_replaced() {
        let paths = vec![PathBuf::from("src/lib.rs")];
        assert_eq!(
            preview_replacement_index(&paths, Some(Path::new("src/lib.rs")), true),
            None
        );
    }

    #[test]
    fn tab_position_ids_are_stable_and_path_specific() {
        assert_eq!(
            position_id(Path::new("src/lib.rs")),
            "editor-tab:src/lib.rs"
        );
        assert_ne!(
            position_id(Path::new("src/lib.rs")),
            position_id(Path::new("src/main.rs"))
        );
    }
}
