use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::project_context::ProjectContext;
use crate::terminal::model::block_context::BlockContext;
use crate::terminal::model::terminal_model::TerminalModel;
use warpui::ReadModel;

/// Cap on how many recent completed blocks are included in a query request.
const MAX_RECENT_BLOCKS: usize = 10;

/// Lightweight copy of a completed block — single definition in
/// `dragent_agent::block` shared by the engine core and the UI.
pub use dragent_agent::block::{AttachedBlock, BlockContextSnapshot};

/// Convert the terminal model's block context into the shared snapshot.
/// (Free function rather than `From`: orphan rules forbid the app from
/// implementing a foreign trait for a foreign type.)
pub fn snapshot_from_block_context(bc: &BlockContext) -> BlockContextSnapshot {
    BlockContextSnapshot {
        block_id: bc.block_id,
        command: bc.command.clone(),
        cwd: bc.cwd.clone(),
        exit_code: bc.exit_code,
        duration_ms: bc.duration_ms,
        output_text: bc.output_text.clone(),
        output_truncated: bc.output_truncated,
        failed: bc.failed,
    }
}

/// Session snapshot types — single definitions in `dragent_agent::session`
/// shared by the engine core, the Host, and the UI.
pub use dragent_agent::session::{AgentSessionContext, CodebaseRef};

/// Build a session snapshot from the live terminal model. Reads the last
/// ≤10 completed blocks. Stays app-side: needs WarpUI model handles and
/// terminal internals (free function — orphan rules forbid inherent impls
/// on the moved type).
pub fn session_from_terminal(
    model: &warpui::ModelHandle<TerminalModel>,
    ctx: &impl ReadModel,
    is_autodetected: bool,
    project: Option<ProjectContext>,
) -> AgentSessionContext {
    let (cwd, recent_blocks) = model.read(ctx, |m, _| {
        let cwd = m.cwd().to_path_buf();
        let blocks: Vec<BlockContextSnapshot> = m.with_core(|core| {
            core.blocks()
                .blocks()
                .iter()
                .filter_map(|b| b.completion.as_deref())
                .map(snapshot_from_block_context)
                .collect()
        });
        // Keep only the last MAX_RECENT_BLOCKS, newest last.
        let recent = if blocks.len() > MAX_RECENT_BLOCKS {
            blocks[blocks.len() - MAX_RECENT_BLOCKS..].to_vec()
        } else {
            blocks
        };
        (cwd, recent)
    });
    AgentSessionContext {
        cwd,
        recent_blocks,
        is_autodetected,
        project,
        timestamp: chrono::Utc::now(),
        os: std::env::consts::OS.to_string(),
        shell: std::env::var("SHELL").unwrap_or_default(),
        codebases: Vec::new(),
        attached_blocks: Vec::new(),
        skills: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(id: usize, cmd: &str, failed: bool) -> BlockContextSnapshot {
        BlockContextSnapshot {
            block_id: id,
            command: cmd.to_string(),
            cwd: PathBuf::from("/tmp"),
            exit_code: if failed { Some(1) } else { Some(0) },
            duration_ms: Some(100),
            output_text: "output".to_string(),
            output_truncated: false,
            failed,
        }
    }

    #[test]
    fn from_block_context_copies_fields() {
        let bc = BlockContext {
            block_id: 7,
            command: "echo hi".to_string(),
            cwd: PathBuf::from("/home/user"),
            exit_code: Some(0),
            duration_ms: Some(50),
            output_text: "hi".to_string(),
            output_line_count: 1,
            output_truncated: false,
            failed: false,
        };
        let snap = snapshot_from_block_context(&bc);
        assert_eq!(snap.block_id, 7);
        assert_eq!(snap.command, "echo hi");
        assert_eq!(snap.cwd, PathBuf::from("/home/user"));
        assert_eq!(snap.exit_code, Some(0));
        assert_eq!(snap.duration_ms, Some(50));
        assert_eq!(snap.output_text, "hi");
        assert!(!snap.output_truncated);
        assert!(!snap.failed);
    }

    #[test]
    fn recent_blocks_capped_at_ten() {
        let blocks: Vec<BlockContextSnapshot> =
            (0..15).map(|i| make_snapshot(i, "cmd", false)).collect();

        let ctx = AgentSessionContext {
            cwd: PathBuf::from("/tmp"),
            recent_blocks: {
                if blocks.len() > MAX_RECENT_BLOCKS {
                    blocks[blocks.len() - MAX_RECENT_BLOCKS..].to_vec()
                } else {
                    blocks.clone()
                }
            },
            is_autodetected: true,
            project: None,
            timestamp: chrono::Utc::now(),
            os: "macos".to_string(),
            shell: "zsh".to_string(),
            codebases: Vec::new(),
            attached_blocks: Vec::new(),
            skills: Vec::new(),
        };
        assert_eq!(ctx.recent_blocks.len(), MAX_RECENT_BLOCKS);
        // Newest entries preserved (ids 5–14).
        assert_eq!(ctx.recent_blocks.first().unwrap().block_id, 5);
        assert_eq!(ctx.recent_blocks.last().unwrap().block_id, 14);
    }

    #[test]
    fn last_failed_block_finds_most_recent() {
        let ctx = AgentSessionContext {
            cwd: PathBuf::from("/tmp"),
            recent_blocks: vec![
                make_snapshot(1, "ok", false),
                make_snapshot(2, "fail", true),
                make_snapshot(3, "ok", false),
            ],
            is_autodetected: false,
            project: None,
            timestamp: chrono::Utc::now(),
            os: "macos".to_string(),
            shell: "zsh".to_string(),
            codebases: Vec::new(),
            attached_blocks: Vec::new(),
            skills: Vec::new(),
        };
        // Block 2 is the only failure.
        assert_eq!(ctx.last_failed_block().unwrap().block_id, 2);
    }

    #[test]
    fn last_failed_block_none_when_all_pass() {
        let ctx = AgentSessionContext {
            cwd: PathBuf::from("/tmp"),
            recent_blocks: vec![make_snapshot(1, "ok", false)],
            is_autodetected: false,
            project: None,
            timestamp: chrono::Utc::now(),
            os: "macos".to_string(),
            shell: "zsh".to_string(),
            codebases: Vec::new(),
            attached_blocks: Vec::new(),
            skills: Vec::new(),
        };
        assert!(ctx.last_failed_block().is_none());
    }
}
