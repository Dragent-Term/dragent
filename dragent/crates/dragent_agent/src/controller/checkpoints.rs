//! Turn checkpoints: capture, restore, and revert.
//!
//! Moved verbatim from `app/src/agent/controller/checkpoints.rs`; same
//! behavior. Zero WarpUI deps: `ctx_free_log` / `checkpoint_message_id` come
//! from `super::session_helpers`, snapshot paths from
//! `crate::checkpoint_store`, and the text helpers live in
//! `super::checkpoint_helpers`. No method here emits events, so no sink seam.

use std::path::PathBuf;

use super::core::AgentController;
use super::session_helpers::{checkpoint_message_id, ctx_free_log};
use crate::approval_policy::is_safe_readonly_command;
use crate::block::AgentBlockKind;
use crate::checkpoint_store::write_target_paths;

impl AgentController {
    /// Snapshot the worktree before this turn's first mutation, so the whole
    /// agent turn can be reverted in one click. `git stash create` writes a
    /// commit object without touching the worktree or index (tracked files
    /// only — brand-new files aren't covered and are reported on revert).
    /// Snapshot the pre-write state of every file `tool_name`/`args_json`
    /// will mutate, under the current user message's checkpoint. Called before
    /// each write tool runs (including during auto-resumed continuations, so
    /// their edits attach to the originating user prompt).
    pub(crate) fn capture_write_checkpoint(&mut self, tool_name: &str, args_json: &str) {
        let Some(message_id) = self.current_query_block_id else {
            return;
        };
        let paths = write_target_paths(tool_name, args_json, &self.cwd);
        for path in paths {
            self.checkpoints.capture(message_id, &path);
        }
        if self.checkpoints.has_checkpoint(message_id) {
            self.mark_message_checkpoint(message_id);
        }
    }

    pub(crate) fn mark_message_checkpoint(&mut self, message_id: usize) {
        let mut changed = false;
        if let Some(block) = self.find_block_mut_by_id(message_id) {
            if let AgentBlockKind::UserQuery { checkpoint, .. } = &mut block.kind {
                if checkpoint.is_none() {
                    *checkpoint = Some(format!("msg-{message_id}"));
                    changed = true;
                }
            }
        }
        if changed {
            self.update_block_in_db(message_id);
        }
    }

    pub(crate) fn register_shell_checkpoint(&mut self, call_id: &str, args_json: &str) {
        let Some(message_id) = self.current_query_block_id else {
            return;
        };
        let value = serde_json::from_str::<serde_json::Value>(args_json).unwrap_or_default();
        let command = value["command"].as_str().unwrap_or_default();
        if command.is_empty() || is_safe_readonly_command(command) {
            return;
        }
        let root = value["cwd"]
            .as_str()
            .filter(|cwd| !cwd.trim().is_empty())
            .map(PathBuf::from)
            .map(|cwd| {
                if cwd.is_absolute() {
                    cwd
                } else {
                    self.cwd.join(cwd)
                }
            })
            .unwrap_or_else(|| self.cwd.clone());
        self.checkpoints
            .register_shell_command(call_id, message_id, &root);
    }

    pub fn ingest_shell_checkpoint(&mut self, call_id: &str) -> bool {
        let Some((message_id, changed_files)) = self.checkpoints.ingest_shell_command(call_id)
        else {
            return false;
        };
        if changed_files == 0 || !self.checkpoints.has_checkpoint(message_id) {
            return false;
        }
        self.mark_message_checkpoint(message_id);
        log::info!(
            "[checkpoint] captured {changed_files} shell-mutated file(s) for message {message_id}"
        );
        true
    }

    /// Whether any restore checkpoint exists (→ show the diff-bar revert button).
    pub fn has_turn_checkpoint(&self) -> bool {
        self.checkpoints.latest_message().is_some()
    }

    /// The most recent message id with a live checkpoint, if any (what
    /// `revert_last_turn` would target).
    pub fn checkpoints_latest_message(&self) -> Option<usize> {
        self.checkpoints.latest_message()
    }

    /// Restore files to before the most recent message that changed files.
    pub fn revert_last_turn(&mut self) -> String {
        let Some(message_id) = self.checkpoints.latest_message() else {
            return "No agent checkpoint to revert.".to_string();
        };
        self.restore_to_block(message_id)
    }

    /// Whether `block_id` has a loaded checkpoint the controller can actually
    /// restore. Durable snapshots are loaded into this store when the pane or
    /// conversation is restored, so the same condition drives the button both
    /// before and after an app restart.
    pub fn has_live_checkpoint(&self, block_id: usize) -> bool {
        self.checkpoints.has_checkpoint(block_id)
    }

    /// Preview of what restoring `block_id` would do: the files that would be
    /// reverted/removed, and how many later user messages (and everything the
    /// agent did in response) would be discarded. `None` if there's nothing
    /// live to restore.
    pub fn checkpoint_restore_preview(
        &self,
        block_id: usize,
    ) -> Option<(Vec<(PathBuf, u32, u32)>, usize)> {
        let stats = self.checkpoints.preview_stats(block_id)?;
        let idx = self.blocks.iter().position(|b| b.id == block_id)?;
        let removed_user_turns = self.blocks[idx + 1..]
            .iter()
            .filter(|b| matches!(b.kind, AgentBlockKind::UserQuery { .. }))
            .count();
        Some((stats, removed_user_turns))
    }

    /// Restore files to the state before a specific user message ran
    /// Restores per message. Rewrites modified files to their
    /// prior bytes and deletes files the message created.
    pub fn restore_to_block(&mut self, block_id: usize) -> String {
        match self.checkpoints.restore(block_id) {
            Some(summary) => {
                self.agent_modified_files.clear();
                // Rewind the conversation view too: drop every block AFTER this
                // user message so the last thing on screen is that prompt, and
                // forget the LLM history / checkpoints for the discarded turns.
                self.truncate_conversation_after(block_id);
                summary
            }
            None => "That message has no restore point (it changed no files).".to_string(),
        }
    }

    /// Remove all agent blocks after the UserQuery with `block_id` (keeping
    /// that prompt as the last visible block), trim the LLM session history to
    /// match, and forget checkpoints for the discarded messages.
    pub(crate) fn truncate_conversation_after(&mut self, block_id: usize) {
        let Some(idx) = self.blocks.iter().position(|b| b.id == block_id) else {
            return;
        };
        // Count user turns kept (up to and including this one) so we can trim
        // session_history to the same point.
        let kept_user_turns = self.blocks[..=idx]
            .iter()
            .filter(|b| matches!(b.kind, AgentBlockKind::UserQuery { .. }))
            .count();
        self.blocks.truncate(idx + 1);

        // Trim session_history to the same number of user messages (a user
        // message opens each turn; keep everything up to the Nth user entry).
        let mut user_seen = 0;
        let mut cut = self.session_history.len();
        for (i, m) in self.session_history.iter().enumerate() {
            if m.role == "user" {
                user_seen += 1;
                if user_seen > kept_user_turns {
                    cut = i;
                    break;
                }
            }
        }
        self.session_history.truncate(cut);

        // Drop checkpoints for messages we just discarded (their blocks are gone).
        self.checkpoints.discard_after(block_id);
        self.current_query_block_id = Some(block_id);
        ctx_free_log(block_id, kept_user_turns);
    }

    /// Rebind the checkpoint store to the active conversation, remapping
    /// durable markers to fresh runtime block ids. Called by
    /// `core::set_persistence` (and, app-side, after session restore).
    /// `pub(crate)` so `super::core` can call it.
    pub(crate) fn bind_checkpoints_for_active_conversation(&mut self) {
        let Some(conversation_id) = self.active_conversation_id else {
            self.checkpoints.clear();
            return;
        };

        // Blocks are assigned fresh runtime IDs when restored from SQLite.
        // Their durable checkpoint marker preserves the ID used by the file
        // snapshot store, allowing it to be remapped before the UI queries it.
        let remap: std::collections::HashMap<usize, usize> = self
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                AgentBlockKind::UserQuery {
                    checkpoint: Some(marker),
                    ..
                } => checkpoint_message_id(marker).map(|old_id| (old_id, block.id)),
                _ => None,
            })
            .collect();

        self.checkpoints.bind(self.pane_uuid, conversation_id);
        self.checkpoints.remap_message_ids(&remap);

        // Older builds did not persist the marker after first capture. Runtime
        // IDs normally remain stable, so recover those messages directly from
        // the durable store and write the marker back to SQLite now.
        let live_ids: Vec<usize> = self
            .blocks
            .iter()
            .filter(|block| {
                matches!(block.kind, AgentBlockKind::UserQuery { .. })
                    && self.checkpoints.has_checkpoint(block.id)
            })
            .map(|block| block.id)
            .collect();
        for id in live_ids {
            self.mark_message_checkpoint(id);
        }
    }
}

// Keep the app's `super::checkpoints::{append_resume_block, …}` import path
// working for already-moved callers: the canonical definitions live in
// `super::checkpoint_helpers`.
pub use super::checkpoint_helpers::{
    append_resume_block, checkpoint_excerpt, push_checkpoint_section,
};
