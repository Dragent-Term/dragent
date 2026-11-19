//! Kitty Graphics Protocol — in-process image storage and placement.
//!
//! The [Kitty Graphics Protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
//! uses APC sequences (`ESC _ G <key=value pairs> ; <base64-data> ST`) to
//! transfer image data and placement instructions.
//!
//! This module:
//! - Parses incoming APC G sequences into [`KittyCommand`] structs.
//! - Stores image data in a [`KittyImageStore`] keyed by image ID.
//! - Produces response bytes to send back to the PTY.
//!
//! Dragent adds one extension beyond the spec: images are stored with a
//! monotonically increasing *placement serial* so the renderer can detect
//! stale placements during resize without comparing pixel buffers.

use base64::Engine as _;
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// Wire types
// ─────────────────────────────────────────────────────────────────────────────

/// Pixel format of the image data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum KittyFormat {
    /// 24-bit RGB (3 bytes/pixel).
    Rgb = 24,
    /// 32-bit RGBA (4 bytes/pixel).
    Rgba = 32,
    /// PNG — entire file, decompressed by the renderer.
    Png = 100,
}

impl KittyFormat {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            24 => Some(Self::Rgb),
            32 => Some(Self::Rgba),
            100 => Some(Self::Png),
            _ => None,
        }
    }
}

/// The action the client requests (key `a=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum KittyAction {
    /// Transmit data (store it).
    #[default]
    Transmit,
    /// Transmit data AND immediately display.
    TransmitAndDisplay,
    /// Display a previously stored image.
    Display,
    /// Query: does the terminal support the protocol?
    Query,
    /// Delete a placement or image.
    Delete,
    /// Transmit a frame for an animation.
    TransmitFrame,
    /// Control animation.
    AnimationControl,
    /// Compose cells.
    Compose,
}

impl KittyAction {
    fn from_char(c: char) -> Self {
        match c {
            't' => Self::Transmit,
            'T' => Self::TransmitAndDisplay,
            'p' => Self::Display,
            'q' => Self::Query,
            'd' => Self::Delete,
            'f' => Self::TransmitFrame,
            'a' => Self::AnimationControl,
            'c' => Self::Compose,
            _ => Self::Transmit,
        }
    }
}

/// A parsed Kitty Graphics command.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct KittyCommand {
    /// Image ID (`i=`). 0 means auto-assign.
    pub image_id: u32,
    /// Placement ID (`p=`). 0 means default placement.
    pub placement_id: u32,
    /// Requested action.
    pub action: KittyAction,
    /// Pixel format (`f=`).
    pub format: Option<KittyFormat>,
    /// Pixel width (`s=`) — required for raw RGB/RGBA.
    pub pixel_width: u32,
    /// Pixel height (`v=`) — required for raw RGB/RGBA.
    pub pixel_height: u32,
    /// Number of columns to occupy (`c=`). 0 = auto.
    pub columns: u32,
    /// Number of rows to occupy (`r=`). 0 = auto.
    pub rows_count: u32,
    /// `m=0` — this is the final (or only) chunk; `m=1` — more chunks follow.
    pub more_chunks: bool,
    /// Base64-decoded raw payload bytes.
    pub data: Vec<u8>,
    /// Whether the client wants a response (`q=`): 0 = yes, 1 = no OK, 2 = no.
    pub quiet: u8,
}

impl Default for KittyCommand {
    fn default() -> Self {
        Self {
            image_id: 0,
            placement_id: 0,
            action: KittyAction::Transmit,
            format: None,
            pixel_width: 0,
            pixel_height: 0,
            columns: 0,
            rows_count: 0,
            more_chunks: false,
            data: Vec::new(),
            quiet: 0,
        }
    }
}

/// Parse an APC G payload (everything between `ESC _ G` and `ST`).
///
/// Format: `<key>=<val>[,<key>=<val>,...][;<base64-data>]`
pub fn parse_kitty_apc(payload: &[u8]) -> Option<KittyCommand> {
    let text = std::str::from_utf8(payload).ok()?;
    // Split at the first ';' to separate key-value pairs from payload data.
    let (kv_part, b64_data) = match text.find(';') {
        Some(idx) => (&text[..idx], &text[idx + 1..]),
        None => (text, ""),
    };

    let mut cmd = KittyCommand::default();

    for pair in kv_part.split(',') {
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or_default().trim();
        let val = it.next().unwrap_or_default().trim();
        match key {
            "a" => {
                cmd.action = val
                    .chars()
                    .next()
                    .map(KittyAction::from_char)
                    .unwrap_or_default();
            }
            "i" => {
                cmd.image_id = val.parse().unwrap_or(0);
            }
            "p" => {
                cmd.placement_id = val.parse().unwrap_or(0);
            }
            "f" => {
                cmd.format = val.parse::<u8>().ok().and_then(KittyFormat::from_u8);
            }
            "s" => {
                cmd.pixel_width = val.parse().unwrap_or(0);
            }
            "v" => {
                cmd.pixel_height = val.parse().unwrap_or(0);
            }
            "c" => {
                cmd.columns = val.parse().unwrap_or(0);
            }
            "r" => {
                cmd.rows_count = val.parse().unwrap_or(0);
            }
            "m" => {
                cmd.more_chunks = val == "1";
            }
            "q" => {
                cmd.quiet = val.parse().unwrap_or(0);
            }
            _ => {}
        }
    }

    if !b64_data.is_empty() {
        cmd.data = base64::engine::general_purpose::STANDARD
            .decode(b64_data)
            .unwrap_or_default();
    }

    Some(cmd)
}

// ─────────────────────────────────────────────────────────────────────────────
// Image store
// ─────────────────────────────────────────────────────────────────────────────

/// A stored image in the in-process kitty store.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct KittyImage {
    pub image_id: u32,
    pub format: KittyFormat,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// Raw decoded pixel data (RGB / RGBA / PNG bytes).
    pub data: Vec<u8>,
    /// Monotonically increasing counter incremented on every store/overwrite.
    pub serial: u64,
}

/// In-process store for kitty graphics images.
///
/// Images are keyed by the 32-bit image ID assigned by the client program.
/// The store assigns sequential IDs when the client requests `i=0`.
#[derive(Debug, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct KittyImageStore {
    images: HashMap<u32, KittyImage>,
    next_id: u32,
    serial: u64,
    /// Pending chunk accumulator: (image_id → partial data).
    pending: HashMap<u32, Vec<u8>>,
}

impl KittyImageStore {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ..Default::default()
        }
    }

    /// Process a parsed command. Returns a response to send to the PTY (if
    /// `quiet == 0`), and the final `image_id` if an image was stored.
    pub fn process(&mut self, mut cmd: KittyCommand) -> (Option<Vec<u8>>, Option<u32>) {
        // Auto-assign ID.
        if cmd.image_id == 0 {
            cmd.image_id = self.next_id;
            self.next_id += 1;
        }

        match cmd.action {
            KittyAction::Query => {
                let resp = if cmd.quiet == 0 {
                    Some(self.ok_response(cmd.image_id, cmd.placement_id))
                } else {
                    None
                };
                return (resp, None);
            }
            KittyAction::Delete => {
                self.images.remove(&cmd.image_id);
                return (None, None);
            }
            KittyAction::Transmit | KittyAction::TransmitAndDisplay => {
                // Accumulate chunks.
                let entry = self.pending.entry(cmd.image_id).or_default();
                entry.extend_from_slice(&cmd.data);

                if !cmd.more_chunks {
                    // Finalise the image.
                    let data = self.pending.remove(&cmd.image_id).unwrap_or_default();
                    let format = cmd.format.unwrap_or(KittyFormat::Rgba);
                    self.serial += 1;
                    let serial = self.serial;
                    self.images.insert(
                        cmd.image_id,
                        KittyImage {
                            image_id: cmd.image_id,
                            format,
                            pixel_width: cmd.pixel_width,
                            pixel_height: cmd.pixel_height,
                            data,
                            serial,
                        },
                    );
                    let resp = if cmd.quiet == 0 {
                        Some(self.ok_response(cmd.image_id, cmd.placement_id))
                    } else {
                        None
                    };
                    return (resp, Some(cmd.image_id));
                }
                return (None, None);
            }
            _ => {}
        }
        (None, None)
    }

    /// Look up a stored image by ID.
    pub fn get(&self, id: u32) -> Option<&KittyImage> {
        self.images.get(&id)
    }

    /// Remove all images.
    pub fn clear(&mut self) {
        self.images.clear();
        self.pending.clear();
    }

    fn ok_response(&self, image_id: u32, placement_id: u32) -> Vec<u8> {
        // ESC _ G i=<id>,p=<pid> ; OK ST
        format!("\x1b_Gi={},p={};OK\x1b\\", image_id, placement_id).into_bytes()
    }
}
