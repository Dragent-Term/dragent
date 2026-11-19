//! Confirmation guards for critical / destructive actions.
//!
//! A per-view [`ConfirmRequest`] the terminal view holds while a
//! confirmation dialog is open — reused for checkpoint reversion and any
//! other destructive action.
//!
//! Deliberately no app-quit guard: quitting always terminates the UI
//! immediately. Host-owned work (agent runs, worktrees, daemon PTYs)
//! survives via the detached host process and is re-adopted on relaunch,
//! and the persisted session restores the screens as left.

use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use uuid::Uuid;

use crate::terminal::view::actions::TerminalAction;

// ── Foreground-command registry ───────────────────────────────────────────────
//
// Long-running foreground shell commands (yarn dev, npm run dev, a dev server)
// aren't agent tasks, so they aren't in the agent registry. This tracks, per
// pane, whether such a command is live — so closing a tab can guard against
// silently killing it.

static FOREGROUND_COMMANDS: Lazy<Mutex<HashMap<Uuid, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Record (or clear) the foreground command running in `pane`. Pass `None`
/// when the command finishes or the prompt goes idle.
pub fn set_foreground_command(pane: Uuid, command: Option<String>) {
    let mut map = FOREGROUND_COMMANDS
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    match command {
        Some(cmd) => {
            map.insert(pane, cmd);
        }
        None => {
            map.remove(&pane);
        }
    }
}

/// The foreground command running in `pane`, if any.
pub fn foreground_command_for(pane: Uuid) -> Option<String> {
    FOREGROUND_COMMANDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&pane)
        .cloned()
}

/// True if any pane has a foreground command running (for the tab-close guard).
pub fn any_foreground_running() -> bool {
    !FOREGROUND_COMMANDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_empty()
}

/// A pending in-app confirmation dialog. Held by the terminal view; when the
/// user confirms, `on_confirm` is dispatched.
#[derive(Clone, Debug)]
pub struct ConfirmRequest {
    pub title: String,
    pub body: String,
    /// Label of the confirming button (e.g. "Revert").
    pub confirm_label: String,
    /// True → confirm button styled as destructive (red).
    pub danger: bool,
    /// Action dispatched when the user confirms.
    pub on_confirm: Option<TerminalAction>,
}

impl ConfirmRequest {
    /// A destructive-action guard that dispatches `action` on confirm.
    pub fn destructive(
        title: impl Into<String>,
        body: impl Into<String>,
        confirm_label: impl Into<String>,
        action: TerminalAction,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            confirm_label: confirm_label.into(),
            danger: true,
            on_confirm: Some(action),
        }
    }
}
