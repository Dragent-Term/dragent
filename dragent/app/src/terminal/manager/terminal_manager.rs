use std::path::Path;

use dragent_pty::{
    resolve_shell, spawn_interactive, FeedCallback, PtyConfig, PtyEvent, PtySessionHandle,
    Receiver, SyncBufferLenCallback, WakeupCallback,
};
use dragent_shell_hooks::ShellKind;

use super::pty_writer::{HostPtyBinding, PtyWriter};

#[derive(Debug)]
pub enum TerminalManagerError {
    SpawnFailed(String),
}

/// Owns the interactive PTY session. Drop before the view so the reader thread stops cleanly.
pub struct TerminalManager {
    writer: PtyWriter,
}

impl TerminalManager {
    /// Spawn an interactive shell and return the manager plus lifecycle events (exit/error).
    #[cfg(any(test, feature = "performance-harness"))]
    pub fn spawn(
        shell: Option<String>,
        cwd: impl AsRef<Path>,
        feed: FeedCallback,
        sync_buffer_len: SyncBufferLenCallback,
        on_wakeup: WakeupCallback,
    ) -> Result<(Self, Receiver<PtyEvent>), TerminalManagerError> {
        let config = PtyConfig {
            shell: shell.clone(),
            cwd: Some(cwd.as_ref().to_path_buf()),
            rows: 24,
            cols: 80,
            ..PtyConfig::default()
        };

        let (handle, rx) = spawn_interactive(config, feed, sync_buffer_len, on_wakeup)
            .map_err(|e| TerminalManagerError::SpawnFailed(e.to_string()))?;
        install_shell_hooks(&handle, shell.as_deref());

        Ok((Self::new(handle), rx))
    }

    #[cfg(any(test, feature = "performance-harness"))]
    pub fn new(handle: PtySessionHandle) -> Self {
        Self {
            writer: PtyWriter::new(handle),
        }
    }

    /// A manager for a host-OWNED tab session: no local shell is ever
    /// spawned. Input/resize route to the daemon's PTY through `binding`
    /// (buffered until bound); output arrives via `feed_remote_output`
    /// from the workspace's host-tab pump.
    pub fn host(binding: std::sync::Arc<HostPtyBinding>) -> Self {
        Self {
            writer: PtyWriter::host(binding),
        }
    }

    /// A manager with NO local shell — for a shared-session mirror. Its writer
    /// forwards all input to the host's PTY via `sink` instead of a local PTY.
    pub fn remote(sink: super::pty_writer::RemoteInputSink) -> Self {
        Self {
            writer: PtyWriter::remote(sink),
        }
    }

    pub fn writer(&self) -> &PtyWriter {
        &self.writer
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.writer.shutdown();
    }
}

fn install_shell_hooks(handle: &PtySessionHandle, shell: Option<&str>) {
    let shell = resolve_shell(shell.map(str::to_owned));
    let kind = ShellKind::from_program(&shell);

    match dragent_shell_hooks::install_bootstrap(kind) {
        Ok(Some(path)) => {
            let command = dragent_shell_hooks::source_command(kind, &path);
            if let Err(err) = handle.write(command.as_bytes()) {
                log::warn!("Failed to write shell hook bootstrap: {err:#}");
                return;
            }
            if let Err(err) = handle.write(b"\r") {
                log::warn!("Failed to submit shell hook bootstrap: {err:#}");
            }
        }
        Ok(None) => {
            log::info!("Shell {shell:?} has no terminal block integration");
        }
        Err(err) => {
            log::warn!("Failed to install shell hooks: {err:#}");
        }
    }
}
