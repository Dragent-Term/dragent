//! Wasm stubs for the native PTY layer.
//!
//! The web viewer renders remote-session output into a headless
//! [`dragent_terminal::Emulator`] and never spawns a real PTY. These types exist
//! only to satisfy the app's type signatures on wasm; every method is an
//! unreachable no-op (or an `Err`) — nothing here is expected to run.
#![allow(dead_code, unused_variables)]

use anyhow::{bail, Result};
use async_channel::Receiver as AsyncReceiver;
use std::path::PathBuf;

use crate::messages::PtyEvent;

/// Mirrors `event_loop::READ_BUFFER_SIZE`.
pub const READ_BUFFER_SIZE: usize = 0x10_0000;

#[derive(Debug, Clone, Copy, Default)]
pub struct PtyFeedStats {
    pub bytes_processed: usize,
}

pub type FeedCallback = Box<dyn FnMut(&[u8]) -> PtyFeedStats + Send>;
pub type SyncBufferLenCallback = Box<dyn Fn() -> Option<usize> + Send + Sync>;
pub type WakeupCallback = Box<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Default)]
pub struct PtyConfig {
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub output: Vec<String>,
    pub exit_code: i32,
}

/// Opaque handle. Never constructed on wasm — `spawn_interactive` always errors.
#[derive(Debug, Clone)]
pub struct PtySessionHandle {
    _private: (),
}

impl PtySessionHandle {
    pub fn write(&self, _data: &[u8]) -> Result<()> {
        bail!("PTY unavailable on wasm")
    }
    pub fn resize(&self, _rows: u16, _cols: u16) -> Result<()> {
        bail!("PTY unavailable on wasm")
    }
    pub fn interrupt(&self) -> Result<()> {
        bail!("PTY unavailable on wasm")
    }
    pub fn terminate(&self) -> Result<()> {
        bail!("PTY unavailable on wasm")
    }
    pub fn set_read_paused(&self, _paused: bool) -> Result<()> {
        Ok(())
    }
    pub fn shutdown(&self) -> Result<()> {
        Ok(())
    }
    pub fn line_limit(&self) -> Option<usize> {
        None
    }
    pub fn foreground_running(&self) -> bool {
        false
    }
}

pub fn spawn_interactive(
    _config: PtyConfig,
    _feed: FeedCallback,
    _sync_buffer_len: SyncBufferLenCallback,
    _on_wakeup: WakeupCallback,
) -> Result<(PtySessionHandle, AsyncReceiver<PtyEvent>)> {
    bail!("PTY spawn unavailable on wasm")
}

pub fn spawn_interactive_with_args(
    _config: PtyConfig,
    _args: &[String],
    _env: &[(&str, String)],
    _feed: FeedCallback,
    _sync_buffer_len: SyncBufferLenCallback,
    _on_wakeup: WakeupCallback,
) -> Result<(PtySessionHandle, AsyncReceiver<PtyEvent>)> {
    bail!("PTY spawn unavailable on wasm")
}

pub fn execute_command(_command: &str, _cwd: Option<PathBuf>) -> CommandResult {
    CommandResult {
        output: Vec::new(),
        exit_code: -1,
    }
}

#[derive(Debug, Clone)]
pub struct ShellChoice {
    pub label: String,
    pub path: String,
}

pub fn resolve_shell(preferred: Option<String>) -> String {
    preferred.unwrap_or_else(|| "/bin/sh".to_string())
}

pub fn available_shells() -> &'static [ShellChoice] {
    &[]
}

pub fn configure_std_shell_command(_cmd: &mut std::process::Command, _shell: &str) {}
