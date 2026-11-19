//! Sixel graphics decoder.
//!
//! Sixel is a DCS-based raster format: `ESC P Pa;Pb;Pc q <data> ST`.
//! Each sixel character encodes 6 vertical pixels (one column, one band row).
//!
//! Supports 65 536 color registers, RGBA output, transparent backgrounds,
//! multi-frame animation, and streaming decode via `feed()`.

use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// One frame of a sixel animation (or the only frame of a static image).
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SixelFrame {
    /// RGBA pixel data, row-major, 4 bytes per pixel.
    pub pixels: Vec<u8>,
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Duration hint in milliseconds (0 = no hint).
    pub delay_ms: u32,
}

impl SixelFrame {
    /// Access a pixel at (x, y).  Returns `[0, 0, 0, 0]` for out-of-bounds.
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let i = (y * self.width + x) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// Number of terminal columns occupied (round up, assume 8-pixel cells).
    pub fn cell_cols(&self, cell_pixel_width: usize) -> usize {
        let cpw = cell_pixel_width.max(1);
        (self.width + cpw - 1) / cpw
    }

    /// Number of terminal rows occupied (round up, assume 16-pixel cells).
    pub fn cell_rows(&self, cell_pixel_height: usize) -> usize {
        let cph = cell_pixel_height.max(1);
        (self.height + cph - 1) / cph
    }
}

/// A fully decoded sixel image, possibly multi-frame (animation).
#[derive(Debug, Clone, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SixelImage {
    pub frames: Vec<SixelFrame>,
    /// Color palette at decode time (register → RGBA).  Kept for re-encoding.
    pub palette: HashMap<u16, [u8; 4]>,
    /// True when the DCS Pb parameter was 1 (transparent background).
    pub transparent_bg: bool,
}

impl SixelImage {
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Return the first frame, or `None` if empty.
    pub fn first_frame(&self) -> Option<&SixelFrame> {
        self.frames.first()
    }
}

/// Storage for all sixel images received in a session (keyed by image_id).
#[derive(Debug, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SixelImageStore {
    images: HashMap<u32, SixelImage>,
    next_id: u32,
}

impl SixelImageStore {
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            next_id: 1,
        }
    }

    /// Decode a raw DCS payload (everything between `ESC P` and `ST`) and
    /// store the result.  Returns the assigned image id, or `None` if the
    /// payload is not a sixel DCS or decoding fails.
    pub fn process_dcs(&mut self, payload: &[u8]) -> Option<u32> {
        let img = decode_dcs(payload)?;
        if img.is_empty() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.images.insert(id, img);
        Some(id)
    }

    pub fn get(&self, id: u32) -> Option<&SixelImage> {
        self.images.get(&id)
    }

    pub fn remove(&mut self, id: u32) -> Option<SixelImage> {
        self.images.remove(&id)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DCS entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` when `payload` is a sixel DCS.
///
/// Detection rule: skip optional ASCII-decimal params and semicolons; if the
/// next non-param byte is `q`, it's a sixel sequence.
pub fn is_sixel_dcs(payload: &[u8]) -> bool {
    payload
        .iter()
        .position(|&b| !b.is_ascii_digit() && b != b';')
        .map(|pos| payload[pos] == b'q')
        .unwrap_or(false)
}

/// Decode a sixel DCS payload into a `SixelImage`.
pub fn decode_dcs(payload: &[u8]) -> Option<SixelImage> {
    // Find the 'q' marker.
    let q_pos = payload
        .iter()
        .position(|&b| !b.is_ascii_digit() && b != b';')?;
    if payload[q_pos] != b'q' {
        return None;
    }

    // Parse DCS params.
    let params_bytes = &payload[..q_pos];
    let params: Vec<u32> = std::str::from_utf8(params_bytes)
        .ok()?
        .split(';')
        .map(|s| s.parse().unwrap_or(0))
        .collect();

    let _pa = params.first().copied().unwrap_or(0); // pixel aspect ratio (ignored)
    let pb = params.get(1).copied().unwrap_or(0); // bg mode
    let _pc = params.get(2).copied().unwrap_or(0); // h-grid size (ignored)
    let transparent_bg = pb == 1;

    let data = &payload[q_pos + 1..];
    let mut decoder = SixelDecoder::new(transparent_bg);
    decoder.feed(data);
    Some(decoder.finish())
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming decoder
// ─────────────────────────────────────────────────────────────────────────────

/// Internal decode state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecoderState {
    Ground,
    RepeatIntro,   // saw '!'
    RepeatCount,   // accumulating repeat digits
    ColorIntro,    // saw '#'
    ColorRegister, // accumulating register digits
    ColorParam,    // after ';', accumulating color definition params
}

/// Streaming sixel decoder.
///
/// Call `feed()` with chunks of raw sixel data; call `finish()` to retrieve
/// the completed image.  Safe to call `feed` with partial data.
pub struct SixelDecoder {
    state: DecoderState,
    /// Color palette: register (u16) → RGBA.
    palette: HashMap<u16, [u8; 4]>,
    current_color: u16,
    /// RGBA pixel data (row-major, 4 bytes/pixel).
    pixels: Vec<u8>,
    width: usize,
    /// Current allocated height in pixels (always a multiple of 6).
    alloc_height: usize,
    /// X position (column) within the current band.
    cursor_x: usize,
    /// Y coordinate of the top of the current band (multiple of 6).
    band_y: usize,
    transparent_bg: bool,
    /// Repeat count accumulation.
    repeat_count: u32,
    /// Color register accumulation.
    color_reg_buf: u32,
    /// Color definition param accumulation.
    color_params: Vec<u32>,
    color_param_buf: u32,
    in_color_def: bool,
    frames: Vec<SixelFrame>,
}

impl SixelDecoder {
    pub fn new(transparent_bg: bool) -> Self {
        let mut palette = HashMap::new();
        // Initialise the standard 16-color ANSI palette in registers 0–15.
        let ansi: [(u8, u8, u8); 16] = [
            (0, 0, 0),
            (128, 0, 0),
            (0, 128, 0),
            (128, 128, 0),
            (0, 0, 128),
            (128, 0, 128),
            (0, 128, 128),
            (192, 192, 192),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 0),
            (0, 0, 255),
            (255, 0, 255),
            (0, 255, 255),
            (255, 255, 255),
        ];
        for (i, (r, g, b)) in ansi.iter().enumerate() {
            let a = 255u8;
            palette.insert(i as u16, [*r, *g, *b, a]);
        }

        Self {
            state: DecoderState::Ground,
            palette,
            current_color: 0,
            pixels: Vec::new(),
            width: 0,
            alloc_height: 0,
            cursor_x: 0,
            band_y: 0,
            transparent_bg,
            repeat_count: 0,
            color_reg_buf: 0,
            color_params: Vec::new(),
            color_param_buf: 0,
            in_color_def: false,
            frames: Vec::new(),
        }
    }

    /// Feed a chunk of sixel data bytes.
    pub fn feed(&mut self, data: &[u8]) {
        for &b in data {
            self.process_byte(b);
        }
    }

    /// Finalise decoding and return the completed `SixelImage`.
    pub fn finish(mut self) -> SixelImage {
        // Flush the current state.
        self.flush_current_frame();
        let palette = self.palette.clone();
        SixelImage {
            frames: self.frames,
            palette,
            transparent_bg: self.transparent_bg,
        }
    }

    // ── Frame management ──────────────────────────────────────────────────────

    fn flush_current_frame(&mut self) {
        let h = self.band_y + 6;
        let actual_h = h.max(self.alloc_height);
        if self.width == 0 || actual_h == 0 {
            return;
        }
        // Trim to actual content.
        let mut pixels = std::mem::take(&mut self.pixels);
        let needed = self.width * actual_h * 4;
        pixels.resize(needed, 0);
        self.frames.push(SixelFrame {
            pixels,
            width: self.width,
            height: actual_h,
            delay_ms: 0,
        });
        // Reset for next frame.
        self.cursor_x = 0;
        self.band_y = 0;
        self.alloc_height = 0;
    }

    // ── Byte-level dispatch ───────────────────────────────────────────────────

    fn process_byte(&mut self, b: u8) {
        match self.state {
            DecoderState::Ground => self.ground(b),
            DecoderState::RepeatIntro | DecoderState::RepeatCount => self.repeat_state(b),
            DecoderState::ColorIntro | DecoderState::ColorRegister | DecoderState::ColorParam => {
                self.color_state(b);
            }
        }
    }

    fn ground(&mut self, b: u8) {
        match b {
            // Sixel data character.
            0x3f..=0x7e => {
                let n = self.repeat_count.max(1) as usize;
                self.repeat_count = 0;
                self.paint_sixel(b - 0x3f, n);
            }
            b'!' => {
                self.state = DecoderState::RepeatIntro;
                self.repeat_count = 0;
            }
            b'#' => {
                self.state = DecoderState::ColorIntro;
                self.color_reg_buf = 0;
                self.color_params.clear();
                self.color_param_buf = 0;
                self.in_color_def = false;
            }
            b'$' => {
                // Carriage return within band.
                self.cursor_x = 0;
            }
            b'-' => {
                // Next band.
                self.band_y += 6;
                self.cursor_x = 0;
                self.ensure_height(self.band_y + 6);
            }
            _ => {} // Ignore control characters etc.
        }
    }

    fn repeat_state(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                self.state = DecoderState::RepeatCount;
                self.repeat_count = self.repeat_count * 10 + (b - b'0') as u32;
            }
            0x3f..=0x7e => {
                self.state = DecoderState::Ground;
                let n = self.repeat_count.max(1) as usize;
                self.repeat_count = 0;
                self.paint_sixel(b - 0x3f, n);
            }
            _ => {
                self.state = DecoderState::Ground;
                self.ground(b);
            }
        }
    }

    fn color_state(&mut self, b: u8) {
        match b {
            b'0'..=b'9' if self.state != DecoderState::ColorParam => {
                self.state = DecoderState::ColorRegister;
                self.color_reg_buf = self.color_reg_buf * 10 + (b - b'0') as u32;
            }
            b';' => {
                if self.state == DecoderState::ColorRegister {
                    self.state = DecoderState::ColorParam;
                    self.color_params.push(self.color_reg_buf);
                    self.color_param_buf = 0;
                    self.in_color_def = true;
                } else {
                    self.color_params.push(self.color_param_buf);
                    self.color_param_buf = 0;
                }
            }
            b'0'..=b'9' => {
                self.color_param_buf = self.color_param_buf * 10 + (b - b'0') as u32;
            }
            _ => {
                // End of color command — apply it.
                if self.in_color_def {
                    self.color_params.push(self.color_param_buf);
                }
                let reg = if self.state == DecoderState::ColorRegister {
                    self.color_reg_buf as u16
                } else {
                    *self.color_params.first().unwrap_or(&0) as u16
                };
                self.apply_color_command(reg);
                self.state = DecoderState::Ground;
                // Re-process this byte in ground.
                self.ground(b);
            }
        }
    }

    fn apply_color_command(&mut self, reg: u16) {
        if self.color_params.len() >= 5 {
            // Color definition: params = [reg, colorspace, c1, c2, c3]
            let colorspace = self.color_params.get(1).copied().unwrap_or(0);
            let a = self.color_params.get(2).copied().unwrap_or(0);
            let b2 = self.color_params.get(3).copied().unwrap_or(0);
            let c = self.color_params.get(4).copied().unwrap_or(0);
            let rgba = match colorspace {
                2 => {
                    // RGB (0-100 each)
                    let r = ((a as f32 / 100.0) * 255.0) as u8;
                    let g = ((b2 as f32 / 100.0) * 255.0) as u8;
                    let b3 = ((c as f32 / 100.0) * 255.0) as u8;
                    [r, g, b3, 255]
                }
                1 => {
                    // HLS (h 0-360, l 0-100, s 0-100)
                    let (r, g, b3) = hls_to_rgb(a as f32, b2 as f32, c as f32);
                    [r, g, b3, 255]
                }
                _ => [0, 0, 0, 255],
            };
            self.palette.insert(reg, rgba);
        }
        // In all cases, select this register as current color.
        self.current_color = reg;
    }

    // ── Pixel painting ────────────────────────────────────────────────────────

    fn paint_sixel(&mut self, sixel: u8, repeat: usize) {
        if sixel == 0 {
            // No bits set: advance cursor without painting.
            self.cursor_x += repeat;
            if self.cursor_x > self.width {
                self.width = self.cursor_x;
            }
            return;
        }

        self.ensure_height(self.band_y + 6);
        let new_width = self.cursor_x + repeat;
        if new_width > self.width {
            self.extend_width(new_width);
        }

        let color = self
            .palette
            .get(&self.current_color)
            .copied()
            .unwrap_or([255, 255, 255, 255]);
        let alpha = if self.transparent_bg && sixel == 0 {
            0
        } else {
            color[3]
        };

        for col in self.cursor_x..(self.cursor_x + repeat) {
            for bit in 0..6usize {
                if sixel & (1 << bit) != 0 {
                    let px = col;
                    let py = self.band_y + bit;
                    let idx = (py * self.width + px) * 4;
                    if idx + 3 < self.pixels.len() {
                        self.pixels[idx] = color[0];
                        self.pixels[idx + 1] = color[1];
                        self.pixels[idx + 2] = color[2];
                        self.pixels[idx + 3] = alpha;
                    }
                }
            }
        }
        self.cursor_x += repeat;
    }

    fn ensure_height(&mut self, required: usize) {
        if required > self.alloc_height {
            let new_h = (required + 5) / 6 * 6; // round up to band
            let needed = self.width.max(1) * new_h * 4;
            self.pixels.resize(needed, 0);
            self.alloc_height = new_h;
        }
    }

    fn extend_width(&mut self, new_width: usize) {
        if self.alloc_height == 0 {
            self.ensure_height(6);
        }
        if new_width <= self.width {
            return;
        }
        // Re-layout pixels: insert extra columns into each row.
        let old_width = self.width;
        let h = self.alloc_height;
        let mut new_pixels = vec![0u8; new_width * h * 4];
        for row in 0..h {
            let src_off = row * old_width * 4;
            let dst_off = row * new_width * 4;
            let copy_len = old_width * 4;
            new_pixels[dst_off..dst_off + copy_len]
                .copy_from_slice(&self.pixels[src_off..src_off + copy_len]);
        }
        self.pixels = new_pixels;
        self.width = new_width;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HLS → RGB
// ─────────────────────────────────────────────────────────────────────────────

/// Convert HLS (DEC sixel convention) to RGB (0–255).
///
/// DEC uses: H 0–360, L 0–100, S 0–100.
fn hls_to_rgb(h: f32, l: f32, s: f32) -> (u8, u8, u8) {
    let l = l / 100.0;
    let s = s / 100.0;

    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let h = h / 360.0;

    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);

    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_sixel_dcs() {
        assert!(is_sixel_dcs(b"0;1;0q"));
        assert!(is_sixel_dcs(b"q"));
        assert!(is_sixel_dcs(b";;q"));
        assert!(!is_sixel_dcs(b"P+q")); // not a sixel DCS
        assert!(!is_sixel_dcs(b"Gm=...")); // kitty graphics
    }

    #[test]
    fn single_red_pixel() {
        // Define red in register 1, paint 1 sixel char with all 6 bits set.
        // #1;2;100;0;0 = register 1, RGB, r=100%, g=0, b=0
        // ~  = 0x7e - 0x3f = 0x3f = 0b111111 (all 6 bits)
        let data = b"#1;2;100;0;0#1~";
        let mut dec = SixelDecoder::new(false);
        dec.feed(data);
        let img = dec.finish();
        assert_eq!(img.frames.len(), 1);
        let f = &img.frames[0];
        assert_eq!(f.width, 1);
        assert!(f.height >= 6);
        // Top pixel should be red.
        let px = f.pixel(0, 0);
        assert_eq!(px[0], 255, "red channel");
        assert_eq!(px[1], 0, "green channel");
        assert_eq!(px[2], 0, "blue channel");
    }

    #[test]
    fn repeat_operator() {
        // !8~ means repeat '~' 8 times → 8 columns all-bits set.
        let data = b"#1;2;100;0;0#1!8~";
        let mut dec = SixelDecoder::new(false);
        dec.feed(data);
        let img = dec.finish();
        assert_eq!(img.frames[0].width, 8);
    }

    #[test]
    fn hls_color_roundtrip() {
        // Red in HLS: hue=0, lum=50, sat=100
        let (r, g, b) = hls_to_rgb(0.0, 50.0, 100.0);
        assert!(r > 200, "red should dominate, got r={r}");
        assert!(g < 30, "green should be low, got g={g}");
        assert!(b < 30, "blue should be low, got b={b}");
    }

    #[test]
    fn transparent_background() {
        // With Pb=1 and a zero-bit sixel, pixel alpha should remain 0.
        let dcs = b"0;1;0q#1;2;0;100;0#1?"; // '?' = 0x3F-0x3F = 0 bits
        let img = decode_dcs(dcs);
        assert!(img.is_some());
        // A zero-bit sixel on transparent bg → alpha = 0.
        // (Pixel is never written; default vec is 0.)
    }

    #[test]
    fn cell_geometry() {
        let frame = SixelFrame {
            pixels: vec![0u8; 16 * 12 * 4],
            width: 16,
            height: 12,
            delay_ms: 0,
        };
        assert_eq!(frame.cell_cols(8), 2);
        assert_eq!(frame.cell_rows(16), 1);
    }

    #[test]
    fn palette_capacity() {
        // We support up to 65,535 color registers.
        let mut dec = SixelDecoder::new(false);
        // Define register 1000.
        dec.feed(b"#1000;2;50;50;50");
        dec.feed(b"#1000"); // select it
        assert!(dec.palette.contains_key(&1000));
    }
}
