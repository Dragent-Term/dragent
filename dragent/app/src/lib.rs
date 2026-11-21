#![allow(dead_code, unused_imports, unused_variables, unused_mut)]
//! The Dragent app crate. Built as a **lib** so the WASM web viewer can run the
//! app's real views (`RootView` etc.), and as a **bin** (`main.rs`) for desktop.
//! Native execution subsystems are `#[cfg(not(target_family = "wasm"))]`-gated so
//! this compiles for the browser.

pub mod agent;
pub mod api;
pub mod auth;
pub mod automation;
#[cfg(not(target_family = "wasm"))]
pub mod automations;
pub mod autoupdate;
pub mod bg;
pub mod channel_ext;
pub mod chat;
#[cfg(not(target_family = "wasm"))]
pub mod cli;
pub mod community;
pub mod confirm;
pub mod console;
pub mod diagnostics;
mod error_notice;
pub mod fonts;
pub mod git_cmd;
pub mod git_features;
pub mod git_status;
#[cfg(not(target_family = "wasm"))]
pub mod integrations;
pub mod machine_ui;
pub mod notifications;
pub mod organisation_context;
#[cfg(not(target_family = "wasm"))]
pub mod pull_requests;
#[cfg(not(target_family = "wasm"))]
pub mod tasks;
#[cfg(not(target_family = "wasm"))]
pub mod tasks_page;
#[cfg(not(target_family = "wasm"))]
pub mod teams;
#[cfg(not(target_family = "wasm"))]
pub mod ui_preview;
pub use dragent_viewer::icons;
pub mod browser;
pub mod process_ext;
pub mod realtime;
pub mod remote_fs;
pub mod root_view;
pub mod settings;
pub mod setup;
pub mod shared_input;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod team_source;
pub mod terminal;
pub mod viewer_view;
pub mod voice;
pub mod workspace;
pub mod workspace_features;

// `theme` now lives in the shared, native-free `dragent_viewer` crate (consumed
// by both the desktop app and the wasm web viewer). Re-export it so the app's
// existing `crate::theme::…` paths keep resolving.
pub use dragent_viewer::theme;

// The desktop app entry (winit App, menus, allocator, TLS init) is native-only.
#[cfg(not(target_family = "wasm"))]
mod desktop;
#[cfg(not(target_family = "wasm"))]
mod hang_watchdog;
#[cfg(not(target_family = "wasm"))]
mod window_frame;
pub mod worktree_sweep;
#[cfg(not(target_family = "wasm"))]
pub use desktop::run;
