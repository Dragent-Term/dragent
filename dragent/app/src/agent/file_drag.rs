//! Internal explorer drags copy file context into an agent composer.

use std::any::Any;
use std::path::PathBuf;
use warpui::elements::{AcceptedByDropTarget, Draggable, DraggableState, DropTargetData, Element};
use warpui::{EntityId, EventContext};

use crate::terminal::view::actions::TerminalAction;
use crate::workspace::WorkspaceAction;

#[derive(Debug, Clone)]
pub(crate) enum AgentInputDropTarget {
    Terminal(EntityId),
    Workspace {
        view_id: EntityId,
        action: fn(Vec<PathBuf>) -> WorkspaceAction,
    },
}

impl DropTargetData for AgentInputDropTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl AgentInputDropTarget {
    fn attach(&self, path: &std::path::Path, ctx: &mut EventContext) {
        match self {
            Self::Terminal(view_id) => ctx.dispatch_typed_action_for(
                *view_id,
                TerminalAction::AttachDroppedFiles {
                    paths: vec![path.to_string_lossy().into_owned()],
                },
            ),
            Self::Workspace { view_id, action } => {
                ctx.dispatch_typed_action_for(*view_id, action(vec![path.to_path_buf()]));
            }
        }
    }
}

pub(crate) fn draggable_file(
    row: Box<dyn Element>,
    state: DraggableState,
    path: PathBuf,
) -> Box<dyn Element> {
    Draggable::new(state, row)
        .with_keep_original_visible(true)
        .with_drop_target_at_pointer()
        .use_copy_cursor_when_dragging_over_drop_target()
        .with_accepted_by_drop_target_fn(|data, _| {
            if data.as_any().is::<AgentInputDropTarget>() {
                AcceptedByDropTarget::Yes
            } else {
                AcceptedByDropTarget::No
            }
        })
        .on_drop(move |ctx, _, _, data| {
            if let Some(target) =
                data.and_then(|data| data.as_any().downcast_ref::<AgentInputDropTarget>())
            {
                target.attach(&path, ctx);
            }
        })
        .finish()
}
