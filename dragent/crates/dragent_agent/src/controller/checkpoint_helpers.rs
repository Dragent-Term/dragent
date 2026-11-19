//! Checkpoint excerpt / resume-flatten helpers.
//!
//! Moved verbatim from `app/src/agent/controller/checkpoints.rs` (pure,
//! zero WarpUI deps). The app re-exports these paths.

use crate::block::{AgentBlock, AgentBlockKind};

pub fn checkpoint_excerpt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut excerpt: String = text.chars().take(max_chars).collect();
    excerpt.push_str("… [truncated]");
    excerpt
}

pub fn push_checkpoint_section(out: &mut String, title: &str, text: &str, max_chars: usize) {
    out.push_str("\n### ");
    out.push_str(title);
    out.push('\n');
    out.push_str(&checkpoint_excerpt(text, max_chars));
    out.push('\n');
}

/// Flatten only observable execution evidence. Thinking blocks and transient UI
/// state are deliberately excluded: a continuation needs facts, not hidden
/// reasoning or presentation details.
pub fn append_resume_block(out: &mut String, block: &AgentBlock, entries: &mut usize) {
    if *entries >= 64 || out.len() >= 24_000 {
        return;
    }
    match &block.kind {
        AgentBlockKind::AgentMessage { text, .. } if !text.trim().is_empty() => {
            out.push_str("- Agent progress: ");
            out.push_str(&checkpoint_excerpt(text.trim(), 2_000));
            out.push('\n');
            *entries += 1;
        }
        AgentBlockKind::ApprovalRequest {
            command,
            reason,
            status,
            ..
        } => {
            out.push_str(&format!(
                "- Permission decision ({status:?}): {} — {}\n",
                checkpoint_excerpt(command, 700),
                checkpoint_excerpt(reason, 500)
            ));
            *entries += 1;
        }
        AgentBlockKind::ActionResult {
            command,
            exit_code,
            output_preview,
            ..
        } => {
            out.push_str(&format!(
                "- Command result: `{}`; exit={exit_code:?}; output={}\n",
                checkpoint_excerpt(command, 700),
                checkpoint_excerpt(output_preview, 1_200)
            ));
            *entries += 1;
        }
        AgentBlockKind::ToolStep {
            name,
            args_summary,
            status,
            result_summary,
            ..
        } => {
            out.push_str(&format!(
                "- Tool `{}` ({status:?}): args={}{}\n",
                checkpoint_excerpt(name, 160),
                checkpoint_excerpt(args_summary, 900),
                result_summary
                    .as_deref()
                    .map(|result| format!("; result={}", checkpoint_excerpt(result, 1_500)))
                    .unwrap_or_default()
            ));
            *entries += 1;
        }
        AgentBlockKind::SystemNote { text } if !text.trim().is_empty() => {
            out.push_str("- Runtime event: ");
            out.push_str(&checkpoint_excerpt(text.trim(), 1_500));
            out.push('\n');
            *entries += 1;
        }
        AgentBlockKind::TurnGroup { children, .. }
        | AgentBlockKind::ActivityGroup {
            blocks: children, ..
        } => {
            for child in children {
                append_resume_block(out, child, entries);
                if *entries >= 64 || out.len() >= 24_000 {
                    break;
                }
            }
        }
        _ => {}
    }
}
