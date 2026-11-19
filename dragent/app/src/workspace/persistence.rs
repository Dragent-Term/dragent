//! Session persistence — SQLite via `dragent_persistence` (async writer thread writer thread).

pub use dragent_persistence::{
    PersistedAgentBlock, PersistedBlock, PersistedColoredLine, PersistedRunStatus,
    PersistedSession, PersistedTextSpan, RestoredAgentBlockLists, RestoredBlockLists,
    SerializedDockedTerminal, SerializedDocumentSession, SerializedHostTerminalBinding,
    SerializedSession, SerializedTab, SerializedWorkspaceTab, SerializedWorkspaceTabWorkItem,
    WindowChrome, WriterHandles,
};

use std::time::Duration;
use web_time::{SystemTime, UNIX_EPOCH};
// Matches `Block::created_at` (std on native, browser clock on wasm).
use web_time::Instant;

use crate::console::escape_parse::{ColoredLine, TextSpan};
use crate::terminal::model::block::{Block, BlockStatus};
use warpui::color::ColorU;

pub fn persist_snapshot(writer: &WriterHandles, session: SerializedSession) {
    writer.snapshot(session);
}

pub fn clear_pane_data(
    writer: &WriterHandles,
    pane_uuid: uuid::Uuid,
    project_root: Option<std::path::PathBuf>,
) {
    writer.clear_pane_data(pane_uuid, project_root);
}

pub fn clear_agent_blocks(
    writer: &WriterHandles,
    pane_uuid: uuid::Uuid,
    project_root: Option<std::path::PathBuf>,
) {
    writer.clear_agent_blocks(pane_uuid, project_root);
}

pub fn block_to_persisted(block: &Block) -> PersistedBlock {
    let now_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let elapsed_ms = block.created_at.elapsed().as_millis() as i64;
    let created_at_unix_ms = if block.created_at_unix_ms > 0 {
        block.created_at_unix_ms
    } else {
        now_unix_ms - elapsed_ms
    };
    PersistedBlock {
        command: block.command.clone(),
        cwd: block.cwd.clone(),
        lines: block.lines.iter().map(colored_line_to_persisted).collect(),
        status: block_status_to_persisted(block.status),
        exit_code: block.status.exit_code(),
        started_at_unix_ms: None,
        duration_ms: block.duration_ms,
        expanded: block.expanded,
        bookmarked: block.bookmarked,
        created_at_unix_ms,
    }
}

pub fn persisted_block_to_block(block: PersistedBlock, id: usize) -> Block {
    let created_at_unix_ms = block.created_at_unix_ms;
    let created_at = if block.created_at_unix_ms > 0 {
        let now_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let elapsed_ms = (now_unix_ms - block.created_at_unix_ms).max(0) as u64;
        Instant::now()
            .checked_sub(Duration::from_millis(elapsed_ms))
            .unwrap_or_else(Instant::now)
    } else {
        Instant::now()
    };
    let mut b = Block {
        id,
        command: block.command,
        remote_filesystem: crate::remote_fs::is_remote(&block.cwd),
        cwd: block.cwd,
        status: persisted_status_to_block(block.status, block.exit_code),
        started_at: None,
        created_at,
        created_at_unix_ms,
        duration_ms: block.duration_ms,
        expanded: block.expanded,
        bookmarked: block.bookmarked,
        lines: std::sync::Arc::new({
            let lines: Vec<_> = block
                .lines
                .into_iter()
                .map(persisted_line_to_colored)
                .collect();
            let max = crate::terminal::model::COMPLETED_BLOCK_MAX_LINES;
            let start = lines.len().saturating_sub(max);
            lines[start..].to_vec()
        }),
        emulator: None,
        live_line_count: 0,
        completion: None,
        // Restored blocks re-derive their listing locally.
        dir_entries: None,
    };
    b.recompute_completion();
    b
}

fn block_status_to_persisted(status: BlockStatus) -> PersistedRunStatus {
    match status {
        BlockStatus::Pending | BlockStatus::Running => PersistedRunStatus::Running,
        BlockStatus::Completed {
            exit_code: Some(0) | None,
        } => PersistedRunStatus::Success,
        BlockStatus::Completed { exit_code: Some(_) } => PersistedRunStatus::Error,
    }
}

fn persisted_status_to_block(status: PersistedRunStatus, exit_code: Option<i32>) -> BlockStatus {
    match status {
        PersistedRunStatus::Running => BlockStatus::Completed { exit_code: None },
        PersistedRunStatus::Success => BlockStatus::Completed { exit_code },
        PersistedRunStatus::Error => BlockStatus::Completed {
            exit_code: exit_code.or(Some(1)),
        },
    }
}

fn colored_line_to_persisted(line: &ColoredLine) -> PersistedColoredLine {
    PersistedColoredLine {
        spans: line
            .spans
            .iter()
            .map(|s| PersistedTextSpan {
                text: s.text.clone(),
                color: s.color.map(|c| [c.r, c.g, c.b, c.a]),
            })
            .collect(),
        wrapped: line.wrapped,
    }
}

fn persisted_line_to_colored(line: PersistedColoredLine) -> ColoredLine {
    ColoredLine {
        wrapped: line.wrapped,
        spans: line
            .spans
            .into_iter()
            .map(|s| TextSpan {
                text: s.text,
                color: s.color.map(|[r, g, b, a]| ColorU::new(r, g, b, a)),
                bg: None,
                // Persisted blocks don't carry bg/SGR (like `bg` above) — restore
                // unstyled; live output keeps full styling.
                flags: 0,
            })
            .collect(),
    }
}
