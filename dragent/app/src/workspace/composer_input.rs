//! File drops share the same attachment pipeline as the composer's picker.

use std::path::PathBuf;
use warpui::elements::{DispatchEventResult, DropTarget, Element, EventHandler};
use warpui::{EntityId, ViewContext};

use super::{view::WorkspaceView, WorkspaceAction};

pub(super) fn file_drop_target(
    content: Box<dyn Element>,
    view_id: EntityId,
    action: fn(Vec<PathBuf>) -> WorkspaceAction,
) -> Box<dyn Element> {
    let content = DropTarget::new(
        content,
        crate::agent::file_drag::AgentInputDropTarget::Workspace { view_id, action },
    )
    .finish();
    EventHandler::new(content)
        .on_files_dropped(move |ctx, _, paths, _| {
            ctx.dispatch_typed_action_for(
                view_id,
                action(paths.iter().map(PathBuf::from).collect()),
            );
            DispatchEventResult::StopPropagation
        })
        .finish()
}

impl WorkspaceView {
    pub(crate) fn receive_prompt_context_files(
        &mut self,
        paths: Vec<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        // The submitted prompt owns its attachments until the workspace opens.
        if self.project_start_busy {
            return;
        }
        self.prompt_notice = None;
        for path in paths {
            if self.prompt_attachments.len() >= 5 {
                self.prompt_notice = Some("Maximum 5 attachments per message.".into());
                break;
            }
            match crate::agent::attachments::load(&path) {
                Ok(file) => self.prompt_attachments.push(file),
                Err(error) => self.prompt_notice = Some(error),
            }
        }
        ctx.notify();
    }
}
