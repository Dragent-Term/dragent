//! Daemon-owned terminals for temporary UI flows outside a workspace.
use super::pty_writer::HostPtyBinding;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

type Identity = Arc<Mutex<Option<(String, String)>>>;

pub(crate) enum TransientHostEvent {
    Ready,
    Error(String),
    Exited(i32),
}

/// Closing the owning dialog cancels attachment and closes its daemon PTY,
/// including when creation completes after the dialog has already closed.
pub(crate) struct TransientHostTerminal {
    identity: Identity,
    slot: Arc<HostPtyBinding>,
    cancel: Arc<AtomicBool>,
}

impl TransientHostTerminal {
    pub(crate) fn start(
        cwd: PathBuf,
        shell: String,
        slot: Arc<HostPtyBinding>,
        cancel: Arc<AtomicBool>,
        feed: impl Fn(&dragent_host::TerminalChunk) -> bool + Send + 'static,
    ) -> (Self, async_channel::Receiver<TransientHostEvent>) {
        let identity: Identity = Arc::new(Mutex::new(None));
        let owner = Self {
            identity: identity.clone(),
            slot: slot.clone(),
            cancel: cancel.clone(),
        };
        let (tx, rx) = async_channel::unbounded();
        crate::bg::detach(move || {
            let path = cwd.to_string_lossy().into_owned();
            let mut offset = 0;
            let mut attempt = 0u32;
            while !cancel.load(Ordering::Acquire) {
                let run = (|| -> Result<bool, String> {
                    let client = dragent_host::HostClient::ensure_host()?;
                    let existing = identity.lock().unwrap().clone();
                    let (workspace, session) = if let Some(existing) = existing {
                        existing
                    } else {
                        let workspace = client.create_local_workspace(&path)?;
                        let home = client.ensure_home(&workspace, &path)?;
                        let session = client.create_terminal_with_shell(
                            &workspace,
                            &home.id,
                            None,
                            Some(&path),
                            80,
                            24,
                            Some(&shell),
                        )?;
                        let mut saved = identity.lock().unwrap();
                        if cancel.load(Ordering::Acquire) {
                            drop(saved);
                            let _ = client.close_terminal(&workspace, &session.id);
                            return Ok(true);
                        }
                        *saved = Some((workspace.clone(), session.id.clone()));
                        drop(saved);
                        slot.bind(&workspace, &session.id);
                        (workspace, session.id)
                    };
                    let _ = tx.try_send(TransientHostEvent::Ready);
                    let mut exited = false;
                    let result =
                        client.stream_terminal_operations(&workspace, &session, offset, |chunk| {
                            if cancel.load(Ordering::Acquire) {
                                return false;
                            }
                            if chunk.resume.is_some() {
                                if !feed(&chunk) { return false; }
                                offset = chunk.base;
                            }
                            if !chunk.data.is_empty() && chunk.total > offset {
                                if chunk.base < offset {
                                    return false;
                                }
                                let Some(_operations) = chunk.operations.as_deref() else {
                                    return false;
                                };
                                if !feed(&chunk) {
                                    return false;
                                }
                                offset = chunk.total;
                                attempt = 0;
                            }
                            if let Some(code) = chunk.exit_code {
                                exited = true;
                                let _ = tx.try_send(TransientHostEvent::Exited(code));
                            }
                            true
                        });
                    if exited || cancel.load(Ordering::Acquire) {
                        return Ok(true);
                    }
                    if let Ok(sessions) = client.list_terminals(&workspace) {
                        if !sessions.iter().any(|s| {
                            s.id == session
                                && matches!(s.state, dragent_host::TerminalState::Running)
                        }) {
                            let _ = tx.try_send(TransientHostEvent::Error(
                                "Terminal session ended. Reopen sign-in to retry.".into(),
                            ));
                            return Ok(true);
                        }
                    }
                    result.map(|_| false)
                })();
                match run {
                    Ok(true) => break,
                    Err(error) => {
                        let _ = tx.try_send(TransientHostEvent::Error(error));
                    }
                    Ok(false) => {}
                }
                let delay = Duration::from_millis((250u64 << attempt.min(4)).min(4_000));
                std::thread::sleep(delay);
                attempt = attempt.saturating_add(1);
            }
        });
        (owner, rx)
    }
}

impl Drop for TransientHostTerminal {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.slot.detach();
        if let Some((workspace, session)) = self.identity.lock().unwrap().take() {
            crate::bg::detach(move || {
                if let Ok(client) = dragent_host::HostClient::ensure_host() {
                    let _ = client.close_terminal(&workspace, &session);
                }
            });
        }
    }
}
