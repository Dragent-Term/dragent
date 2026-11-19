//! User file attachments for agent queries ("add context")
//! flow. Images (screenshots, Figma exports) travel to the backend as base64
//! attachments and reach the model as native multimodal content; text files
//! are inlined into the message body as fenced blocks, which needs no server
//! support and keeps them greppable in history.

#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use std::path::Path;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// An image attachment riding on a session message — single definition in
/// `dragent_agent::attachments` shared by the engine core and the UI.
pub use dragent_agent::attachments::MessageAttachment;

/// A file the user attached, classified by how it reaches the model.
pub enum LoadedAttachment {
    Image(MessageAttachment),
    /// Text file: inlined into the message content at submit time.
    Text {
        name: String,
        content: String,
        path: std::path::PathBuf,
    },
}

/// A compact, serializable record of an attachment kept ON the user's prompt
/// block — single definition in `dragent_agent::block` shared by the engine
/// core and the UI.
pub use dragent_agent::block::UserBlockAttachment;

impl LoadedAttachment {
    pub fn image_path(&self) -> Option<&std::path::Path> {
        match self {
            LoadedAttachment::Image(img) => img.source_path.as_deref(),
            LoadedAttachment::Text { .. } => None,
        }
    }
    /// Path on disk, whatever the kind — used to hand file references to AI
    /// CLI tools (Claude Code etc.), which read files themselves.
    pub fn any_path(&self) -> Option<&std::path::Path> {
        match self {
            LoadedAttachment::Image(img) => img.source_path.as_deref(),
            LoadedAttachment::Text { path, .. } => Some(path),
        }
    }
    pub fn name(&self) -> &str {
        match self {
            LoadedAttachment::Image(img) => &img.name,
            LoadedAttachment::Text { name, .. } => name,
        }
    }
    pub fn is_image(&self) -> bool {
        matches!(self, LoadedAttachment::Image(_))
    }
}

/// Base64 size cap, mirroring the server's reject threshold (~5 MB decoded).
const MAX_IMAGE_B64: usize = 7 * 1024 * 1024;
/// Text files larger than this are refused — inlining a megabyte of text
/// would blow the context (and the user's budget) in one shot.
const MAX_TEXT_BYTES: u64 = 256 * 1024;

pub fn media_type_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

/// Load a user-selected file. Images become base64 attachments; anything that
/// looks like text is inlined; binary non-image files are refused with a
/// human explanation (returned as Err for the UI to surface).
pub fn load(path: &Path) -> Result<LoadedAttachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    if let Some(mt) = media_type_for(path) {
        let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {name}: {e}"))?;
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        if data_b64.len() > MAX_IMAGE_B64 {
            return Err(format!(
                "{name} is too large (max ~5 MB). Resize or compress the image."
            ));
        }
        return Ok(LoadedAttachment::Image(MessageAttachment {
            name,
            media_type: mt.to_string(),
            data_b64,
            source_path: Some(path.to_path_buf()),
        }));
    }

    let meta = std::fs::metadata(path).map_err(|e| format!("Cannot read {name}: {e}"))?;
    if meta.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "{name} is too large to inline (max 256 KB of text). Point the agent at the path instead — it can read files itself."
        ));
    }
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(LoadedAttachment::Text {
            name,
            content,
            path: path.to_path_buf(),
        }),
        Err(_) => Err(format!(
            "{name} is not an image or text file. Supported: png/jpg/webp/gif and any text file."
        )),
    }
}

/// Open the native multi-select file picker WITHOUT blocking the caller, and
/// deliver the chosen paths (empty on cancel) into `tx`.
///
/// NEVER run a modal loop synchronously from a UI action handler: on macOS
/// `runModal` pumps a nested event loop while warpui's AppContext RefCell is
/// still mutably borrowed by the dispatch that invoked us → "RefCell already
/// borrowed" panic the moment the window redraws. `beginWithCompletionHandler`
/// returns immediately; the completion fires later on a clean main-loop stack.
pub fn pick_files_async(tx: async_channel::Sender<Vec<std::path::PathBuf>>) {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            log::warn!("[attach] file picker called off the main thread");
            let _ = tx.send_blocking(Vec::new());
            return;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(true);
        // The block retains the panel (it must outlive this call); completion
        // runs on the main thread after the user closes the panel.
        let panel_for_block = panel.clone();
        let handler = block2::RcBlock::new(move |response: objc2_app_kit::NSModalResponse| {
            let paths = if response == NSModalResponseOK {
                panel_for_block
                    .URLs()
                    .iter()
                    .filter_map(|url| url.path().map(|p| std::path::PathBuf::from(p.to_string())))
                    .collect()
            } else {
                Vec::new()
            };
            let _ = tx.send_blocking(paths);
        });
        panel.beginWithCompletionHandler(&handler);
    }
    #[cfg(windows)]
    {
        // IFileOpenDialog needs an STA apartment; run it on its own thread and
        // send the result back — the UI thread never blocks.
        crate::bg::detach(move || {
            let paths = pick_files_windows_sta();
            let _ = tx.send_blocking(paths);
        });
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        log::warn!("[attach] file picker not yet implemented on this platform");
        let _ = tx.send_blocking(Vec::new());
    }
}

#[cfg(windows)]
fn pick_files_windows_sta() -> Vec<std::path::PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST,
        FOS_FORCEFILESYSTEM, SIGDN_FILESYSPATH,
    };

    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return Vec::new();
        }
        let result = (|| -> windows::core::Result<Vec<std::path::PathBuf>> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let opts = dialog.GetOptions()?;
            dialog.SetOptions(
                opts | FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM,
            )?;
            // No owner hwnd: the dialog opens app-modal-ish but unparented,
            // which is fine for a utility picker.
            dialog.Show(None)?;
            let items = dialog.GetResults()?;
            let count = items.GetCount()?;
            let mut paths = Vec::with_capacity(count as usize);
            for i in 0..count {
                let item = items.GetItemAt(i)?;
                let raw: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                if !raw.is_null() {
                    paths.push(std::path::PathBuf::from(
                        raw.to_string().unwrap_or_default(),
                    ));
                    windows::Win32::System::Com::CoTaskMemFree(Some(raw.as_ptr() as *const _));
                }
            }
            Ok(paths)
        })();
        CoUninitialize();
        match result {
            Ok(paths) => paths,
            Err(e) => {
                // "Cancelled" arrives as an error HRESULT — silence it.
                if e.code() != windows::Win32::Foundation::ERROR_CANCELLED.to_hresult() {
                    log::warn!("[attach] file dialog failed: {e}");
                }
                Vec::new()
            }
        }
    }
}

/// Append inlined text-file blocks to a query.
pub fn inline_text_files(query: &str, files: &[(String, String)]) -> String {
    if files.is_empty() {
        return query.to_string();
    }
    let mut out = String::with_capacity(query.len() + 256);
    out.push_str(query);
    for (name, content) in files {
        out.push_str(&format!("\n\n[Attached file: {name}]\n```\n{content}\n```"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_types_from_extension() {
        assert_eq!(media_type_for(Path::new("a/shot.PNG")), Some("image/png"));
        assert_eq!(media_type_for(Path::new("design.jpeg")), Some("image/jpeg"));
        assert_eq!(media_type_for(Path::new("notes.md")), None);
    }

    #[test]
    fn text_files_inline_as_fenced_blocks() {
        let q = inline_text_files("fix this", &[("spec.md".into(), "# Spec".into())]);
        assert!(q.starts_with("fix this"));
        assert!(q.contains("[Attached file: spec.md]"));
        assert!(q.contains("# Spec"));
    }

    #[test]
    fn load_classifies_image_text_and_binary() {
        let dir = std::env::temp_dir().join(format!("attach-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Tiny valid-enough PNG payload (we don't validate magic bytes; extension rules).
        std::fs::write(dir.join("a.png"), [137u8, 80, 78, 71]).unwrap();
        std::fs::write(dir.join("b.txt"), "hello").unwrap();
        std::fs::write(dir.join("c.bin"), [0u8, 159, 146, 150]).unwrap();

        assert!(matches!(
            load(&dir.join("a.png")),
            Ok(LoadedAttachment::Image(_))
        ));
        match load(&dir.join("b.txt")) {
            Ok(LoadedAttachment::Text { content, .. }) => assert_eq!(content, "hello"),
            other => panic!("expected text, got {:?}", other.is_ok()),
        }
        assert!(load(&dir.join("c.bin")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
