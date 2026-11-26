use warpui::keymap::macros::*;
use warpui::keymap::{EditableBinding, FixedBinding, Keystroke, Trigger};
use warpui::AppContext;

use crate::terminal::view::actions::TerminalAction;

use super::WorkspaceAction;

/// Workspace-level shortcuts as (action-id, default chord). The id is the
/// stable key used by Settings → Keyboard (`custom_bindings`), the
/// `~/.dragent/keybindings.json` mirror, and the editable keymap binding
/// name — one id flows through all three so rebind/reset just works.
const EDITABLE_DEFAULTS: &[(&str, &str)] = &[
    ("new-tab", "cmdorctrl-t"),
    ("new-window", "cmdorctrl-n"),
    ("new-agent-tab", "cmdorctrl-shift-N"),
    ("close-tab", "cmdorctrl-w"),
    ("save-document", "cmdorctrl-s"),
    ("split-right", "cmdorctrl-d"),
    ("split-down", "cmdorctrl-shift-D"),
    ("prev-tab", "cmdorctrl-shift-["),
    ("next-tab", "cmdorctrl-shift-]"),
    ("open-settings", "cmdorctrl-,"),
    ("toggle-agents-panel", "cmdorctrl-shift-A"),
];

fn parse_trigger(chord: &str) -> Option<Trigger> {
    let steps: Result<Vec<Keystroke>, _> =
        chord.split_whitespace().map(Keystroke::parse).collect();
    steps.ok().map(Trigger::Keystrokes)
}

/// Register workspace-level keyboard shortcuts (tabs, splits, panels).
///
/// Bindings are *editable*: rebinding flows through
/// [`apply_shortcut_overrides`], and Reset = dropping the custom trigger.
pub fn register(ctx: &mut AppContext) {
    #[cfg(not(target_family="wasm"))]
    ctx.register_fixed_bindings([
        FixedBinding::new("cmdorctrl-f",crate::pull_requests::Action::FocusSearch,id!("PullRequestsView")),
        FixedBinding::new("cmdorctrl-r",crate::pull_requests::Action::Refresh,id!("PullRequestsView")),
        FixedBinding::new("cmdorctrl-enter",crate::pull_requests::Action::PrimarySubmit,id!("PullRequestsView")),
        FixedBinding::new("escape",crate::pull_requests::Action::Escape,id!("PullRequestsView")),
    ]);
    // Match when the terminal pane or workspace shell is in the responder chain.
    let context = id!("WorkspaceView") | id!("TerminalView");
    let terminal_context = id!("TerminalView");

    let workspace_action = |id: &str| match id {
        "new-tab" => WorkspaceAction::NewTab,
        "new-window" => WorkspaceAction::NewWindow,
        "new-agent-tab" => WorkspaceAction::NewAgentTab,
        "close-tab" => WorkspaceAction::CloseActive,
        "save-document" => WorkspaceAction::SaveOpenDocument,
        "split-right" => WorkspaceAction::SplitActiveTabHorizontal,
        "split-down" => WorkspaceAction::SplitActiveTabVertical,
        "prev-tab" => WorkspaceAction::SwitchTabPrev,
        "next-tab" => WorkspaceAction::SwitchTabNext,
        "open-settings" => WorkspaceAction::OpenSettings,
        _ => WorkspaceAction::ToggleAgentsPanel,
    };

    // Editable workspace actions (rebindable from Settings → Keyboard).
    let editable: Vec<EditableBinding> = EDITABLE_DEFAULTS
        .iter()
        .map(|(id, chord)| {
            EditableBinding::new(*id, *id, workspace_action(id))
                .with_key_binding(*chord)
                .with_context_predicate(context.clone())
        })
        .collect();
    ctx.register_editable_bindings(editable);

    // Fixed extras: alternate tab-switch chords. They stay fixed so a
    // rebinding can never strand the action without any chord; the primary
    // chord for each action is the editable binding above.
    ctx.register_fixed_bindings([
        FixedBinding::new(
            "cmdorctrl-shift-{",
            WorkspaceAction::SwitchTabPrev,
            context.clone(),
        ),
        FixedBinding::new(
            "cmdorctrl-shift-}",
            WorkspaceAction::SwitchTabNext,
            context.clone(),
        ),
    ]);

    // Terminal clipboard stays fixed: rebinding copy/paste is served through
    // the terminal key handler's override check, which runs before these
    // fixed bindings match. Copy: Cmd+C on macOS, Ctrl+Shift+C on
    // Windows/Linux (the terminal convention). Ctrl+C alone MUST NOT be
    // copy — it's the interrupt (SIGINT). A `cmdorctrl-c` binding would
    // capture Ctrl+C on Windows and hijack it from interrupting a running
    // command.
    ctx.register_fixed_bindings([
        FixedBinding::new("cmd-c", TerminalAction::Copy, terminal_context.clone()),
        FixedBinding::new(
            "ctrl-shift-C",
            TerminalAction::Copy,
            terminal_context.clone(),
        ),
        FixedBinding::new("cmdorctrl-v", TerminalAction::Paste, terminal_context),
    ]);

    // Dev-only perf overlay toggle (native only): never registered in release
    // or wasm builds, so it can't fire for users. `cmdorctrl-shift-O` is free
    // (workflow palette is cmdorctrl-shift-P).
    #[cfg(not(target_family="wasm"))]
    if dragent_paths::is_dev() || cfg!(debug_assertions) {
        ctx.register_fixed_bindings([FixedBinding::new(
            "cmdorctrl-shift-O",
            WorkspaceAction::TogglePerfOverlay,
            context.clone(),
        )]);
    }

    apply_shortcut_overrides(ctx);
}

/// Re-apply the user's `custom_bindings` to the live keymap: overrides
/// become custom triggers, and ids without an override return to default.
/// Called at startup (end of [`register`]) and live on every
/// `AppSettingsChanged` via [`super::sync_shortcut_bindings`], so Reset
/// restores defaults without a restart.
pub fn apply_shortcut_overrides(ctx: &mut AppContext) {
    let settings = crate::settings::AppSettings::global();
    for (id, _default_chord) in EDITABLE_DEFAULTS {
        match settings.shortcuts.custom_bindings.get(*id) {
            Some(chord) => match parse_trigger(chord) {
                Some(trigger) => ctx.set_custom_trigger(id.to_string(), trigger),
                None => log::warn!("[shortcuts] ignoring invalid chord for “{id}”: {chord}"),
            },
            None => ctx.remove_custom_trigger(*id),
        }
    }
}
