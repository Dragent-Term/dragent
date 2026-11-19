//! File attachment wire format shared by the engine core and the UI.
//! Single definition — the app re-exports this path.

use serde::{Deserialize, Serialize};

/// An image attachment riding on a session message (wire format matches the
/// server's `taskAttachment`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageAttachment {
    /// Display name (basename) for chips and history.
    pub name: String,
    pub media_type: String,
    pub data_b64: String,
    /// Where the image was loaded from — used to render thumbnails and the
    /// full-size viewer without decoding base64. Never sent to the server
    /// (the wire JSON is built field-by-field in attachments_json).
    #[serde(default)]
    pub source_path: Option<std::path::PathBuf>,
}

impl MessageAttachment {
    /// A display record to keep on the sent user-prompt block (image → thumbnail
    /// from `source_path`). Text files are inlined into the message text at
    /// submit time, so only images ride here.
    pub fn to_block_display(&self) -> super::block::UserBlockAttachment {
        super::block::UserBlockAttachment {
            name: self.name.clone(),
            is_image: true,
            path: self.source_path.clone(),
        }
    }
}
