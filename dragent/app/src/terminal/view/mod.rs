// `actions` (pure action/hint enums) moved to the shared `dragent_viewer` crate.
pub use dragent_viewer::terminal::view::actions;
pub(crate) mod copy_confirmation;
pub(crate) mod terminal_agent;
pub(crate) mod terminal_astrips;
pub(crate) mod terminal_blocks;
pub(crate) mod terminal_confirm;
pub(crate) mod terminal_gitops;
pub mod terminal_helpers;
pub(crate) mod terminal_keyhandler;
pub(crate) mod terminal_overlays;
mod terminal_phone_agent;

pub(crate) mod terminal_chips;
pub(crate) mod terminal_gitdiff;
pub(crate) mod terminal_panels;

pub(crate) mod terminal_history;
pub(crate) mod terminal_mouse;
pub(crate) mod terminal_render_a;

pub(crate) mod terminal_command;
pub(crate) mod terminal_scan;
pub(crate) mod terminal_voice;

pub(crate) mod terminal_input;
pub(crate) mod terminal_lifecycle;
pub(crate) mod terminal_viewport;

pub(crate) mod terminal_session;

pub(crate) mod terminal_side_shell;

pub mod terminal_agent_actions;
pub(crate) mod terminal_blocks_actions;
pub(crate) mod terminal_correction_actions;
pub(crate) mod terminal_git_actions;
pub(crate) mod terminal_history_actions;
pub(crate) mod terminal_input_actions;
pub(crate) mod terminal_side_actions;
pub(crate) mod terminal_viewer_actions;

pub mod terminal_view;

#[cfg(test)]
mod terminal_copy_tests;
#[cfg(test)]
mod terminal_focus_tests;

pub use terminal_view::TerminalView;
pub(crate) use terminal_panels::effort_slider_preview;

pub(crate) mod terminal_agent_transcript;
pub(crate) mod terminal_cli_chat;
pub(crate) mod acp_conversation;
pub(crate) mod terminal_acp_chat;
