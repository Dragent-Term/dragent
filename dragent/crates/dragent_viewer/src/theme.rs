//! Dragent UI + terminal theme tokens.

use warpui::color::ColorU;

pub const WORKSPACE_PADDING: f32 = 1.0;
pub const HEADER_BAR_HEIGHT: f32 = 38.0;
pub const CONTENT_HEADER_HEIGHT: f32 = 36.0;
pub const HEADER_SEARCH_MAX_WIDTH: f32 = 420.0;
pub const HEADER_ICON_PADDING: f32 = 5.0;
pub const TAB_BAR_BORDER_HEIGHT: f32 = 1.0;
pub const PANEL_HEADER_HEIGHT: f32 = 34.0;
pub const DEFAULT_LEFT_PANEL_WIDTH: f32 = 276.0;
/// The default before 2026-11: a sidebar never resized still has it saved,
/// and moves to the new default.
pub const PREVIOUS_LEFT_PANEL_WIDTH: f32 = 230.0;
pub const DEFAULT_RIGHT_PANEL_WIDTH: f32 = 230.0;
pub const MIN_PANEL_WIDTH: f32 = 220.0;
pub const TERMINAL_PADDING_LEFT: f32 = 16.0;
pub const INPUT_BOX_RADIUS: f32 = 8.0;
pub const INPUT_BOX_MARGIN: f32 = 8.0;
pub const INPUT_MIN_HEIGHT: f32 = 44.0;
pub const INPUT_FONT_SIZE: f32 = 14.0;
pub const COMPLETION_MODAL_MAX_HEIGHT: f32 = 280.0;

pub const UI_FONT_SIZE: f32 = 14.0;
/// Default terminal mono size — the effective size is user-configurable, read
/// it via [`mono_font_size`].
pub const MONO_FONT_SIZE: f32 = 14.0;
/// Default terminal line-height ratio (matches warpui's
/// DEFAULT_UI_LINE_HEIGHT_RATIO); effective value via
/// [`terminal_line_height_ratio`].
pub const TERMINAL_LINE_HEIGHT_RATIO: f32 = 1.15;
/// First-run application theme and fallback when a persisted/custom theme can
/// no longer be resolved. Existing user selections are never replaced.
/// GitHub Dark (with Ayu blue accents).
pub const DEFAULT_THEME_ID: &str = "github_dark";

// ── User-configurable terminal typography ─────────────────────────────────────
//
// Set from EditorSettings at startup and whenever the user moves the sliders
// in Settings → Terminal and Input. Stored as f32 bit patterns in atomics so
// render paths can read them every frame without locking; 0 = "not set yet",
// which falls back to the defaults above.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static MONO_FONT_SIZE_BITS: AtomicU32 = AtomicU32::new(0);
static LINE_HEIGHT_RATIO_BITS: AtomicU32 = AtomicU32::new(0);

/// Effective terminal mono font size (user setting, or the default).
pub fn mono_font_size() -> f32 {
    match MONO_FONT_SIZE_BITS.load(Ordering::Relaxed) {
        0 => MONO_FONT_SIZE,
        bits => f32::from_bits(bits),
    }
}

/// Effective terminal line-height ratio (user setting, or the default).
pub fn terminal_line_height_ratio() -> f32 {
    match LINE_HEIGHT_RATIO_BITS.load(Ordering::Relaxed) {
        0 => TERMINAL_LINE_HEIGHT_RATIO,
        bits => f32::from_bits(bits),
    }
}

/// Scale a design-time size (specified at the 14 px default) by the user's
/// terminal font-size setting, so content surfaces (terminal blocks, agent
/// conversation, input bar) follow the Typography sliders together.
pub fn scaled_mono(px_at_default: f32) -> f32 {
    px_at_default * (mono_font_size() / MONO_FONT_SIZE)
}

/// App-wide alias of [`scaled_mono`]: every typography surface (terminal,
/// agent, chrome) scales from the same Font-size setting.
pub fn scaled(px_at_default: f32) -> f32 {
    scaled_mono(px_at_default)
}

/// The size to draw text written at `px` (a size designed for the 14 px
/// default). Sizes snap up to the app's type scale
/// labels, 13 for list rows and body text, and 14 and up unchanged. The
/// result then follows the Font-size setting like [`scaled`].
pub fn text_size(px: f32) -> f32 {
    scaled(type_scale_step(px))
}

fn type_scale_step(px: f32) -> f32 {
    if px < 13.0 {
        (px + 1.0).floor()
    } else if px < 14.0 {
        px.ceil()
    } else {
        px
    }
}

/// How much the Font-size setting enlarges text over the 14 px default.
pub fn font_scale() -> f32 {
    mono_font_size() / MONO_FONT_SIZE
}

static TYPOGRAPHY_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Changes whenever the size, line height, family or weight of text
/// changes. Anything that keeps shaped text or a size it computed earlier
/// compares this with the value it saw then.
pub fn typography_generation() -> u64 {
    TYPOGRAPHY_GENERATION.load(Ordering::Relaxed) + warpui_core::fonts::roles::generation()
}

/// Publish the user's typography for all render paths — the terminal grid
/// atomics AND the framework's default line-height ratio, so the Line-height
/// slider stacks every default-ratio text element in the app.
pub fn set_terminal_typography(font_size: f32, line_height_ratio: f32) {
    let size = font_size.max(1.0).to_bits();
    let ratio = line_height_ratio.max(0.5).to_bits();
    let size_changed = MONO_FONT_SIZE_BITS.swap(size, Ordering::Relaxed) != size;
    let ratio_changed = LINE_HEIGHT_RATIO_BITS.swap(ratio, Ordering::Relaxed) != ratio;
    if size_changed || ratio_changed {
        TYPOGRAPHY_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    warpui_core::elements::set_ui_line_height_ratio(line_height_ratio.max(0.8));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemePreset {
    pub id: &'static str,
    pub name: &'static str,
}

pub const THEME_PRESETS: &[ThemePreset] = &[
    ThemePreset {
        id: "dark",
        name: "GitHub Dark",
    },
    ThemePreset {
        id: "ayu_dark",
        name: "Ayu Dark",
    },
    ThemePreset {
        id: "light",
        name: "Light",
    },
    ThemePreset {
        id: "dracula",
        name: "Dracula",
    },
    ThemePreset {
        id: "gruvbox_light",
        name: "Gruvbox Light",
    },
    ThemePreset {
        id: "dracula",
        name: "Dracula",
    },
    ThemePreset {
        id: "neon_tide",
        name: "Neon Tide",
    },
];

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub id: &'static str,
    pub background: ColorU,
    pub foreground: ColorU,
    pub accent: ColorU,
    /// True for dark themes (drives light/dark-sensitive UI + preview grouping).
    pub is_dark: bool,
    /// ANSI 0–7: black, red, green, yellow, blue, magenta, cyan, white.
    pub ansi_normal: [ColorU; 8],
    /// ANSI 8–15 (bright variants, same order).
    pub ansi_bright: [ColorU; 8],
}

fn c(r: u8, g: u8, b: u8) -> ColorU {
    ColorU::new(r, g, b, 255)
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_id(DEFAULT_THEME_ID)
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            id: "dark",
            // Keep the asset-independent dark fallback visually aligned with
            // Dragent Black, the application's canonical dark theme.
            background: c(0, 0, 0),
            foreground: c(255, 255, 255),
            accent: c(0, 194, 255),
            is_dark: true,
            ansi_normal: [
                c(97, 97, 97),
                c(255, 130, 114),
                c(180, 250, 114),
                c(254, 253, 194),
                c(165, 213, 254),
                c(255, 143, 253),
                c(208, 209, 254),
                c(241, 241, 241),
            ],
            ansi_bright: [
                c(142, 142, 142),
                c(255, 196, 189),
                c(214, 252, 185),
                c(254, 253, 213),
                c(193, 227, 254),
                c(255, 177, 254),
                c(229, 230, 254),
                c(254, 255, 255),
            ],
        }
    }

    pub fn light() -> Self {
        Self {
            id: "light",
            background: ColorU::new(245, 245, 247, 255),
            foreground: ColorU::new(28, 28, 30, 255),
            accent: ColorU::new(25, 130, 196, 255),
            is_dark: false,
            ansi_normal: [
                c(28, 28, 30),
                c(196, 54, 42),
                c(52, 140, 72),
                c(196, 132, 18),
                c(36, 110, 196),
                c(160, 59, 143),
                c(42, 143, 136),
                c(202, 202, 202),
            ],
            ansi_bright: [
                c(99, 99, 99),
                c(214, 73, 61),
                c(70, 158, 90),
                c(214, 150, 36),
                c(54, 128, 214),
                c(178, 77, 161),
                c(60, 161, 154),
                c(255, 255, 255),
            ],
        }
    }

    /// All available themes (built-in `dark`/`light` + the bundled Warp catalog +
    /// any user themes in `~/.dragent/themes/`), for the settings picker.
    pub fn all_infos() -> &'static [ThemeInfo] {
        registry::registry()
    }

    /// A small curated preset set (used by onboarding). The full catalog is
    /// [`all_infos`].
    pub fn presets() -> &'static [ThemePreset] {
        THEME_PRESETS
    }

    /// The background image (if any) for the theme with this id.
    pub fn bg_image_for(id: &str) -> Option<&'static BgImage> {
        registry::registry()
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.bg_image.as_ref())
    }

    /// Resolve a theme by its id (filename stem, or `dark`/`light`).
    pub fn from_id(id: &str) -> Self {
        if id == "light" {
            return Self::light();
        }
        if id == "dark" {
            return Self::dark();
        }
        registry::registry()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.theme)
            .or_else(|| {
                registry::registry()
                    .iter()
                    .find(|t| t.id == DEFAULT_THEME_ID)
                    .map(|t| t.theme)
            })
            // Keep startup recoverable if the bundled catalog itself cannot
            // be loaded; this built-in palette has no asset dependency.
            .unwrap_or_else(Self::dark)
    }

    pub fn surface_1(&self) -> ColorU {
        blend(self.background, self.foreground, 0.05)
    }

    pub fn surface_2(&self) -> ColorU {
        blend(self.background, self.foreground, 0.10)
    }

    pub fn surface_3(&self) -> ColorU {
        blend(self.background, self.foreground, 0.15)
    }

    pub fn outline(&self) -> ColorU {
        // OPAQUE (blended, not a translucent foreground overlay) so borders stay
        // visible over a theme background image — a `with_alpha` line vanished
        // against bright images. Looks the same on solid backgrounds.
        blend(self.background, self.foreground, 0.16)
    }

    /// Panel and region dividers (sidebar edges, header rules) — stronger than
    /// [`outline`], so panel separators read clearly even over a background image.
    pub fn border(&self) -> ColorU {
        blend(self.background, self.foreground, 0.26)
    }

    /// The theme's background forced OPAQUE. Use for surfaces that must stay solid
    /// and readable even under a background-image theme (popovers, menus, modals,
    /// dropdowns) — `background` itself is translucent then.
    pub fn solid_bg(&self) -> ColorU {
        ColorU::new(self.background.r, self.background.g, self.background.b, 255)
    }

    /// Apply the base background's alpha to `c`, so a surface that should REVEAL a
    /// theme background image inherits its translucency (fully opaque on
    /// non-image themes, where `background.a == 255`).
    fn glass(&self, c: ColorU) -> ColorU {
        ColorU::new(c.r, c.g, c.b, self.background.a)
    }

    /// Terminal pane surface fill. Under a background-image theme this is fully
    /// TRANSPARENT: the image and its single tint are painted ONCE at the
    /// workspace level behind every pane, so the terminal output, input bar, and
    /// alt-screen canvas must not stack any tint of their own — otherwise regions
    /// with more nested containers read darker than others (the uneven-overlay
    /// bug). Transparent surfaces reveal that one uniform workspace tint
    /// everywhere. On a solid theme (`background.a == 255`) this is just the
    /// opaque background, unchanged from before.
    pub fn pane_surface(&self) -> ColorU {
        if self.background.a < 255 {
            ColorU::new(0, 0, 0, 0)
        } else {
            self.background
        }
    }

    /// The pane background, forced OPAQUE. Same hue as the input bar
    /// ([`pane_surface`]), so surfaces using it read as part of the input rather
    /// than as a separate raised card — but it never lets anything through.
    ///
    /// Use for chrome that sits ON TOP of scrolling content (the history
    /// popover): [`pane_surface`] is fully transparent under a background-image
    /// theme, which would let output slide visibly beneath the panel.
    pub fn solid_background(&self) -> ColorU {
        ColorU::new(self.background.r, self.background.g, self.background.b, 255)
    }

    /// Left session sidebar and right project panel background — translucent under
    /// a background-image theme so the image shows through the sidebars too.
    pub fn sidebar_background(&self) -> ColorU {
        self.glass(blend(self.background, self.foreground, 0.02))
    }

    pub fn panel_background(&self) -> ColorU {
        self.glass(blend(self.background, self.foreground, 0.04))
    }

    pub fn sidebar_row_active(&self) -> ColorU {
        self.surface_2()
    }

    pub fn sidebar_row_hover(&self) -> ColorU {
        blend(self.background, self.foreground, 0.07)
    }

    pub fn sidebar_menu_background(&self) -> ColorU {
        self.surface_2()
    }

    pub fn sidebar_menu_hover(&self) -> ColorU {
        self.surface_3()
    }

    pub fn block_background(&self) -> ColorU {
        // Glass so the background image shows through terminal blocks too (a
        // subtle tint, not an opaque card that hides it).
        self.glass(blend(self.background, self.foreground, 0.10))
    }

    pub fn fg_overlay_1(&self) -> ColorU {
        blend(self.background, self.foreground, 0.05)
    }

    pub fn main_text(&self) -> ColorU {
        with_alpha(self.foreground, 0.90)
    }

    pub fn sub_text(&self) -> ColorU {
        with_alpha(self.foreground, 0.60)
    }

    pub fn disabled_text(&self) -> ColorU {
        with_alpha(self.foreground, 0.40)
    }

    /// ANSI palette color by 0–15 index (0–7 normal, 8–15 bright). Used by the
    /// terminal renderer so output colors match the active theme exactly.
    pub fn ansi(&self, idx: usize) -> ColorU {
        if idx < 8 {
            self.ansi_normal[idx]
        } else {
            self.ansi_bright[(idx - 8) & 7]
        }
    }

    // UI-semantic colors map to the theme's own palette (error=red, ok=green,
    // warn=yellow), so every theme drives them consistently.
    pub fn ui_green(&self) -> ColorU {
        self.ansi_normal[2]
    }

    pub fn ui_red(&self) -> ColorU {
        self.ansi_normal[1]
    }

    pub fn ui_yellow(&self) -> ColorU {
        self.ansi_normal[3]
    }

    pub fn ansi_green(&self) -> ColorU {
        self.ansi_normal[2]
    }

    pub fn ansi_red(&self) -> ColorU {
        self.ansi_normal[1]
    }

    pub fn ansi_blue(&self) -> ColorU {
        self.ansi_normal[4]
    }

    pub fn link(&self) -> ColorU {
        // Prefer the brighter blue for links so they read as interactive.
        self.ansi_bright[4]
    }

    pub fn link_surface(&self) -> ColorU {
        with_alpha(self.link(), 0.18)
    }

    pub fn accent_overlay(&self) -> ColorU {
        with_alpha(self.accent, 0.12)
    }

    pub fn accent_bg(&self) -> ColorU {
        blend(self.background, self.accent, 0.18)
    }

    /// Opaque foreground with the stronger WCAG contrast against the accent.
    /// Primary controls must not reuse `main_text`: a light accent in a dark
    /// theme would otherwise produce the gray-on-cyan combination seen in the
    /// workspace trust prompt.
    pub fn accent_text(&self) -> ColorU {
        let luminance = relative_luminance(self.accent);
        let black_contrast = (luminance + 0.05) / 0.05;
        let white_contrast = 1.05 / (luminance + 0.05);
        if black_contrast >= white_contrast {
            ColorU::new(15, 18, 22, 255)
        } else {
            ColorU::new(255, 255, 255, 255)
        }
    }

    pub fn surface_overlay(&self) -> ColorU {
        with_alpha(self.foreground, 0.08)
    }

    /// Tint an arbitrary semantic color (e.g. `ui_red`/`ui_green`) into a
    /// low-opacity surface fill, for status notices whose accent color
    /// isn't known until the call site picks it.
    pub fn tinted_surface(&self, color: ColorU) -> ColorU {
        with_alpha(color, 0.12)
    }
}

fn blend(bg: ColorU, fg: ColorU, ratio: f32) -> ColorU {
    let t = ratio.clamp(0.0, 1.0);
    // Always OPAQUE. Popovers, menus, cards, and borders derive from `blend`, so
    // they must stay solid and readable even under a background-image theme.
    // Surfaces that should REVEAL the image use `glass()` (sidebar/panel/block
    // backgrounds) instead.
    ColorU::new(
        lerp(bg.r, fg.r, t),
        lerp(bg.g, fg.g, t),
        lerp(bg.b, fg.b, t),
        255,
    )
}

fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}

fn relative_luminance(color: ColorU) -> f32 {
    fn linear(channel: u8) -> f32 {
        let value = channel as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

// ── Theme registry: built-ins + bundled Warp catalog + user themes ─────────────

/// A background image for a theme (Apache-2.0 bundled asset or a user path).
#[derive(Clone, Debug)]
pub struct BgImage {
    /// Path relative to `assets/themes/` for a bundled image, or an absolute
    /// path for a user theme.
    pub asset_path: String,
    /// 0–100: how strongly the image shows behind the terminal.
    pub opacity: u8,
    /// True when `asset_path` is a bundled (embedded) asset vs an absolute file.
    pub bundled: bool,
}

impl BgImage {
    /// Build an [`AssetSource`] for this image — a bundled embedded asset
    /// (addressed by its themes-relative path) or a user file on disk.
    pub fn asset_source(&self) -> warpui_core::assets::asset_cache::AssetSource {
        use warpui_core::assets::asset_cache::AssetSource;
        if !self.bundled {
            return AssetSource::LocalFile {
                path: self.asset_path.clone(),
            };
        }
        // wasm: not embedded (would bloat the bundle) — fetch by URL.
        #[cfg(target_family = "wasm")]
        {
            theme_url_source(&self.asset_path)
        }
        #[cfg(not(target_family = "wasm"))]
        {
            AssetSource::Bundled {
                path: intern_path(&self.asset_path),
            }
        }
    }

    /// A small pre-generated thumbnail (`<name>.thumb.jpg`, ~420px) for preview
    /// cards — avoids decoding/resizing the multi-MB full-res source in the
    /// theme picker, which would jank. Falls back to the full image if no thumb.
    pub fn thumb_asset_source(&self) -> warpui_core::assets::asset_cache::AssetSource {
        use warpui_core::assets::asset_cache::AssetSource;
        let thumb = thumb_path(&self.asset_path);
        if !self.bundled {
            return AssetSource::LocalFile { path: thumb };
        }
        #[cfg(target_family = "wasm")]
        {
            theme_url_source(&thumb)
        }
        #[cfg(not(target_family = "wasm"))]
        {
            AssetSource::Bundled {
                path: intern_path(&thumb),
            }
        }
    }
}

// ── Wasm: theme images are fetched by URL, not embedded ─────────────────────────

/// Namespace marker for theme background images fetched by URL on wasm.
#[cfg(target_family = "wasm")]
struct ThemeBgAsset;
#[cfg(target_family = "wasm")]
impl warpui_core::assets::asset_cache::AsyncAssetType for ThemeBgAsset {}

/// An [`AssetSource`] that fetches a theme image from `/wasm-themes/<path>`
/// (served by the host page alongside the wasm bundle).
#[cfg(target_family = "wasm")]
fn theme_url_source(asset_path: &str) -> warpui_core::assets::asset_cache::AssetSource {
    use warpui_core::assets::asset_cache::{AssetSource, AsyncAssetId};
    let url = format!("/wasm-themes/{asset_path}");
    AssetSource::Async {
        id: AsyncAssetId::new::<ThemeBgAsset>(asset_path),
        fetch: std::sync::Arc::new(move || {
            let url = url.clone();
            Box::pin(fetch_theme_bytes(url))
        }),
    }
}

/// Browser `fetch` of a theme image's bytes.
#[cfg(target_family = "wasm")]
async fn fetch_theme_bytes(url: String) -> anyhow::Result<bytes::Bytes> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let win = web_sys::window().ok_or_else(|| anyhow::anyhow!("no window"))?;
    let resp_val = JsFuture::from(win.fetch_with_str(&url))
        .await
        .map_err(|e| anyhow::anyhow!("theme fetch failed: {e:?}"))?;
    let resp: web_sys::Response = resp_val
        .dyn_into()
        .map_err(|_| anyhow::anyhow!("theme fetch: not a Response"))?;
    if !resp.ok() {
        anyhow::bail!("theme fetch HTTP {} for {url}", resp.status());
    }
    let buf = JsFuture::from(
        resp.array_buffer()
            .map_err(|e| anyhow::anyhow!("array_buffer: {e:?}"))?,
    )
    .await
    .map_err(|e| anyhow::anyhow!("array_buffer await: {e:?}"))?;
    let array = js_sys::Uint8Array::new(&buf);
    Ok(bytes::Bytes::from(array.to_vec()))
}

/// `warp_bundled/koi_bg.jpg` → `warp_bundled/koi_bg.thumb.jpg`.
fn thumb_path(path: &str) -> String {
    match path.rfind('.') {
        Some(dot) => format!("{}.thumb.jpg", &path[..dot]),
        None => format!("{path}.thumb.jpg"),
    }
}

/// Intern a bundled asset path into a `&'static str` (AssetSource::Bundled needs
/// one). Bounded — there are only ~17 background images — so leaking is fine.
fn intern_path(path: &str) -> &'static str {
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    use std::sync::Mutex;
    static POOL: Lazy<Mutex<HashMap<String, &'static str>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));
    let mut pool = POOL.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(s) = pool.get(path) {
        return s;
    }
    let leaked: &'static str = Box::leak(path.to_string().into_boxed_str());
    pool.insert(path.to_string(), leaked);
    leaked
}

/// A selectable theme: its id/name, resolved palette, and optional background.
#[derive(Clone, Debug)]
pub struct ThemeInfo {
    pub id: &'static str,
    pub name: String,
    pub theme: Theme,
    pub bg_image: Option<BgImage>,
}

mod registry {
    use super::*;
    use once_cell::sync::Lazy;

    /// Bundled Warp theme specs (Apache-2.0). See `app/assets/themes/LICENSE`.
    #[derive(rust_embed::RustEmbed)]
    #[folder = "assets/themes"]
    #[include = "**/*.yaml"]
    struct BundledThemes;

    #[derive(serde::Deserialize)]
    struct RawTheme {
        accent: String,
        background: RawBackground,
        foreground: String,
        // A plain string ("darker"/"lighter") for most themes, but Warp allows
        // a nested `custom: {..opacity overrides}` map instead (see Pink City) —
        // we don't consume those per-surface overrides, but the field must still
        // parse as *something* or the whole theme is dropped by the `?` below.
        #[serde(default)]
        details: Option<serde_yaml::Value>,
        terminal_colors: RawTermColors,
        #[serde(default)]
        background_image: Option<RawBgImage>,
    }
    #[derive(serde::Deserialize)]
    struct RawTermColors {
        normal: RawAnsi,
        bright: RawAnsi,
    }
    #[derive(serde::Deserialize)]
    struct RawAnsi {
        black: String,
        red: String,
        green: String,
        yellow: String,
        blue: String,
        magenta: String,
        cyan: String,
        white: String,
    }
    #[derive(serde::Deserialize)]
    struct RawBgImage {
        path: String,
        #[serde(default)]
        opacity: Option<u32>,
    }

    /// A Warp theme's `background` is EITHER a single hex string OR a vertical
    /// gradient map (`{top, bottom}`). We render a solid base, so a gradient is
    /// collapsed to the average of its stops. Without this, gradient-background
    /// themes (e.g. `snowy`) failed to deserialize and were silently DROPPED from
    /// the catalog — so selecting them fell back to the default dark theme (light
    /// text) while their image still showed, leaving unreadable text on the image.
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum RawBackground {
        Solid(String),
        Gradient {
            top: String,
            #[serde(default)]
            bottom: Option<String>,
        },
    }

    fn bg_color(b: &RawBackground) -> Option<ColorU> {
        match b {
            RawBackground::Solid(s) => hex(s),
            RawBackground::Gradient { top, bottom } => {
                let t = hex(top)?;
                match bottom.as_deref().and_then(hex) {
                    Some(btm) => Some(ColorU::new(
                        ((t.r as u16 + btm.r as u16) / 2) as u8,
                        ((t.g as u16 + btm.g as u16) / 2) as u8,
                        ((t.b as u16 + btm.b as u16) / 2) as u8,
                        255,
                    )),
                    None => Some(t),
                }
            }
        }
    }

    fn hex(s: &str) -> Option<ColorU> {
        let s = s.trim().trim_start_matches('#');
        if s.len() < 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(ColorU::new(r, g, b, 255))
    }

    fn ansi_arr(a: &RawAnsi) -> Option<[ColorU; 8]> {
        Some([
            hex(&a.black)?,
            hex(&a.red)?,
            hex(&a.green)?,
            hex(&a.yellow)?,
            hex(&a.blue)?,
            hex(&a.magenta)?,
            hex(&a.cyan)?,
            hex(&a.white)?,
        ])
    }

    /// Relative luminance heuristic for the light/dark flag when `details` is absent.
    fn is_dark(bg: ColorU) -> bool {
        let l = 0.2126 * bg.r as f32 + 0.7152 * bg.g as f32 + 0.0722 * bg.b as f32;
        l < 128.0
    }

    /// "some_theme-v2" → "Some Theme V2".
    fn display_name(stem: &str) -> String {
        if stem == "github_dark" {
            return "GitHub Dark".to_string();
        }
        if stem == "dragent_dark" {
            return "Dragent Black".to_string();
        }
        stem.split(['_', '-'])
            .filter(|s| !s.is_empty())
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Resolve a `background_image.path` (relative to the themes root when it
    /// contains a slash, else relative to the theme file's own directory).
    fn resolve_bundled_image(theme_rel_path: &str, img_path: &str) -> String {
        if img_path.contains('/') {
            img_path.to_string()
        } else {
            match theme_rel_path.rsplit_once('/') {
                Some((dir, _)) => format!("{dir}/{img_path}"),
                None => img_path.to_string(),
            }
        }
    }

    fn parse(id: &'static str, name: String, rel_path: &str, raw: &str) -> Option<ThemeInfo> {
        let t: RawTheme = serde_yaml::from_str(raw).ok()?;
        let bg = bg_color(&t.background)?;
        let mut theme = Theme {
            id,
            background: bg,
            foreground: hex(&t.foreground)?,
            accent: hex(&t.accent)?,
            is_dark: match t.details.as_ref().and_then(|v| v.as_str()) {
                Some("darker") => true,
                Some("lighter") => false,
                _ => is_dark(bg),
            },
            ansi_normal: ansi_arr(&t.terminal_colors.normal)?,
            ansi_bright: ansi_arr(&t.terminal_colors.bright)?,
        };
        let bg_image = t.background_image.map(|b| BgImage {
            asset_path: resolve_bundled_image(rel_path, &b.path),
            opacity: b.opacity.unwrap_or(100).min(100) as u8,
            bundled: true,
        });
        // With a background image, the BASE background is translucent so the
        // image (painted behind the workspace) shows through the terminal. The
        // derived surface colors (surface_1/panel_background/sidebar_background/…)
        // go through `blend`, which forces alpha 255 — so sidebars, popovers,
        // menus and modals that use those stay fully OPAQUE and readable; only a
        // DIRECT `theme.background` fill (the terminal base) reveals the image.
        if bg_image.is_some() {
            // Make the terminal clearly translucent so the image is visible when
            // selected — even themes with a LOW `opacity` value (which otherwise
            // left the terminal near-opaque, so you saw only the color). A fixed,
            // moderately-translucent alpha keeps text readable while the image
            // shows through.
            theme.background = ColorU::new(
                theme.background.r,
                theme.background.g,
                theme.background.b,
                96,
            );
        }
        Some(ThemeInfo {
            id,
            name,
            theme,
            bg_image,
        })
    }

    fn load() -> Vec<ThemeInfo> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        // Built-ins first so their ids are stable and can't be shadowed.
        for t in [Theme::dark(), Theme::light()] {
            seen.insert(t.id.to_string());
            out.push(ThemeInfo {
                id: t.id,
                name: display_name(t.id),
                theme: t,
                bg_image: None,
            });
        }
        // Bundled Warp catalog.
        for rel in BundledThemes::iter() {
            let stem = rel
                .rsplit('/')
                .next()
                .unwrap_or(&rel)
                .trim_end_matches(".yaml");
            if stem.is_empty() || seen.contains(stem) {
                continue;
            }
            let Some(file) = BundledThemes::get(&rel) else {
                continue;
            };
            let Ok(raw) = std::str::from_utf8(&file.data) else {
                continue;
            };
            let id: &'static str = Box::leak(stem.to_string().into_boxed_str());
            if let Some(info) = parse(id, display_name(stem), &rel, raw) {
                seen.insert(stem.to_string());
                out.push(info);
            }
        }
        // User themes: ~/.dragent/themes/*.yaml (override by id).
        // No filesystem in the browser, so the wasm viewer ships bundled themes only.
        #[cfg(not(target_family = "wasm"))]
        {
            let dir = dragent_paths::home().join("themes");
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) != Some("yaml") {
                        continue;
                    }
                    let stem = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if stem.is_empty() {
                        continue;
                    }
                    let Ok(raw) = std::fs::read_to_string(&p) else {
                        continue;
                    };
                    let id: &'static str = Box::leak(stem.clone().into_boxed_str());
                    if let Some(mut info) = parse(id, display_name(&stem), "", &raw) {
                        // User image paths are absolute-ish; mark not-bundled.
                        if let Some(img) = info.bg_image.as_mut() {
                            img.bundled = false;
                            if !img.asset_path.starts_with('/') {
                                img.asset_path =
                                    dir.join(&img.asset_path).to_string_lossy().into_owned();
                            }
                        }
                        // Replace a same-id bundled theme if present.
                        out.retain(|t| t.id != info.id || !seen.contains(&stem));
                        out.push(info);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    static REGISTRY: Lazy<Vec<ThemeInfo>> = Lazy::new(load);

    pub fn registry() -> &'static [ThemeInfo] {
        &REGISTRY
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn accent_text_selects_the_higher_contrast_foreground() {
        let light_accent = Theme::dark();
        assert_eq!(light_accent.accent_text(), ColorU::new(15, 18, 22, 255));

        let mut dark_accent = Theme::dark();
        dark_accent.accent = ColorU::new(18, 24, 31, 255);
        assert_eq!(dark_accent.accent_text(), ColorU::new(255, 255, 255, 255));
    }

    #[test]
    fn registry_loads_full_catalog() {
        let all = Theme::all_infos();
        // The catalog is deliberately CURATED, not exhaustive: near-identical
        // variants of the same theme (the base16 `black_metal`/`atelier`
        // families, the monokai_pro/vitesse/snazzy sets, …) were dropped in
        // favour of one representative each. Assert a sane floor — enough that a
        // failure to embed the folder is still caught — rather than a big number
        // that just re-encodes however many files happen to be present.
        assert!(
            all.len() >= 18,
            "expected the curated catalog (18+ incl. built-ins), got {}",
            all.len()
        );
        // Built-ins present.
        assert!(all.iter().any(|t| t.id == "dark"));
        assert!(all.iter().any(|t| t.id == "light"));
        assert!(all.iter().any(|t| t.id == DEFAULT_THEME_ID));
        // A known Warp theme resolves with a real palette.
        let dracula = Theme::from_id("dracula");
        assert_eq!(dracula.id, "dracula");
        assert_ne!(
            dracula.ansi_normal[1], dracula.ansi_normal[2],
            "red==green means palette not parsed"
        );
        // At least one theme ships a background image.
        assert!(
            all.iter().any(|t| t.bg_image.is_some()),
            "no background-image themes loaded"
        );
        // Gradient-background themes (`background: {top, bottom}`) must parse and
        // load, not be silently dropped — otherwise selecting them falls back to
        // the default dark theme and their light text is unreadable on the image.
        let dark_city = all
            .iter()
            .find(|t| t.id == "dark_city")
            .expect("dark_city (gradient background) should be in the catalog");
        assert!(
            dark_city.bg_image.is_some(),
            "dark_city should carry its background image"
        );
        // dark_city is a dark theme; its gradient stops average dark.
        assert!(dark_city.theme.is_dark, "dark_city is a dark theme");
        assert!(
            dark_city.theme.background.r < 60,
            "dark_city gradient bg should be dark"
        );

        // Dragent's own background-image themes load with their image + opacity.
        for id in ["stellar", "onyx", "cosmos"] {
            let t = all
                .iter()
                .find(|t| t.id == id)
                .unwrap_or_else(|| panic!("theme `{id}` should be in the catalog"));
            let bg = t
                .bg_image
                .as_ref()
                .unwrap_or_else(|| panic!("theme `{id}` should carry a background image"));
            assert!(
                bg.opacity > 0 && bg.opacity <= 100,
                "`{id}` opacity in range"
            );
            assert!(
                t.theme.is_dark,
                "`{id}` is a dark theme (light text over the image)"
            );
        }
    }

    #[test]
    fn github_dark_is_the_application_default_and_fallback() {
        let default = Theme::default();
        assert_eq!(default.id, DEFAULT_THEME_ID);
        assert_eq!(default.id, "github_dark");
        assert!(default.is_dark);
        // GitHub Dark surfaces with the Ayu blue accent.
        assert_eq!(default.accent, ColorU::new(0x53, 0xbd, 0xfa, 255));
        assert_eq!(Theme::from_id("missing-theme").id, DEFAULT_THEME_ID);
        assert_eq!(Theme::from_id("light").id, "light");
        assert!(default.is_dark);
    }

    #[test]
    fn github_dark_keeps_its_ayu_blue_accent() {
        let github = Theme::from_id("github_dark");
        assert_eq!(github.id, "github_dark");
        assert_eq!(github.background, c(0x0d, 0x11, 0x17));
        assert_eq!(github.foreground, c(0xc9, 0xd1, 0xd9));
        assert_eq!(github.accent, Theme::from_id("ayu_dark").accent);
        assert_eq!(github.accent, c(0x53, 0xbd, 0xfa));
        assert_eq!(github.accent_text(), c(15, 18, 22));
    }

    #[test]
    fn github_dark_secondary_terminal_text_remains_readable() {
        // CLI labels, paths and hints use SGR 90 (ANSI palette index 8).
        let theme = Theme::from_id("github_dark");
        let secondary = theme.ansi(8);
        assert_eq!(secondary, c(0x8b, 0x94, 0x9e));
        let luminance = |color: ColorU| {
            let channel = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
        };
        let contrast = (luminance(secondary) + 0.05) / (luminance(theme.background) + 0.05);
        assert!(contrast >= 4.5, "secondary text contrast was {contrast}");
    }

    #[test]
    fn ayu_dark_is_in_the_catalog() {
        let info = Theme::all_infos()
            .iter()
            .find(|theme| theme.id == "ayu_dark")
            .expect("saved Ayu Dark selections should remain available");
        assert_eq!(info.name, "Ayu Dark");
        assert!(info.theme.is_dark);
        assert_eq!(info.theme.background, c(0x0a, 0x0e, 0x14));
    }

    #[test]
    fn text_sizes_snap_to_the_type_scale() {
        let steps = [9.0, 9.5, 10.0, 10.5, 11.0, 11.5, 12.0, 12.5, 13.0, 13.5, 14.0, 17.0];
        let snapped = steps.map(type_scale_step);
        assert_eq!(
            snapped,
            [10.0, 10.0, 11.0, 11.0, 12.0, 12.0, 13.0, 13.0, 13.0, 14.0, 14.0, 17.0]
        );
    }
}
