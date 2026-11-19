//! Font resolution shared by app text, editors, and terminals.
//!
//! Inter, Fira Code, and JetBrains Mono are bundled for every platform. Apple
//! system fonts are resolved through Core Text and never copied into the bundle.

use std::sync::OnceLock;

use warpui_core::fonts::{roles, Cache as FontCache, FamilyId};

use crate::console::editor_settings::EditorSettings;

#[cfg(target_os = "macos")]
pub const DEFAULT_TERMINAL_FONT_LABEL: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_TERMINAL_FONT_LABEL: &str = "Fira Code";

/// Bundled Inter is the interface font on every platform by default.
pub const DEFAULT_UI_FONT_LABEL: &str = "Inter";

/// The macOS system font, offered as an interface font choice.
#[cfg(target_os = "macos")]
pub const SYSTEM_UI_FONT_LABEL: &str = "SF Pro";

/// The interface and code font roles. Every view keeps these ids; the
/// families and weight behind them follow Settings → Appearance → Text, so
/// a change reaches all text at once (see [`apply_typography`]).
static ROLES: OnceLock<(FamilyId, FamilyId)> = OnceLock::new();

fn roles(cache: &mut FontCache) -> (FamilyId, FamilyId) {
    if let Some(roles) = ROLES.get() {
        return *roles;
    }
    let fresh = (roles::create(FamilyId(0)), roles::create(FamilyId(0)));
    let roles = *ROLES.get_or_init(|| fresh);
    apply_typography(cache);
    roles
}

/// The font for all interface text.
pub fn ui_family(cache: &mut FontCache) -> FamilyId {
    roles(cache).0
}

/// Use the current interface role in render helpers without a font-cache borrow.
pub fn ui_role(fallback: FamilyId) -> FamilyId {
    ROLES.get().map(|roles| roles.0).unwrap_or(fallback)
}

/// The font for terminals, editors, diffs and code everywhere.
pub fn code_family(cache: &mut FontCache) -> FamilyId {
    roles(cache).1
}

/// Point both roles at the families and weight in the saved settings.
/// Returns whether any text now draws differently; callers then repaint.
///
/// `Some` overrides the terminal code font for the
/// shared code role; `None` follows the terminal font. The code role is shared
/// by terminals, editors and diffs, so an editor override also re-points
/// terminal text until the roles split.
pub fn apply_typography(cache: &mut FontCache) -> bool {
    let (ui, code) = roles(cache);
    let settings = EditorSettings::global();
    let shift = roles::shift_for(settings.editor.font_weight);
    let ui_changed = roles::retarget(
        ui,
        interface_family(cache, settings.terminal.ui_font_family.as_deref()),
        shift,
    );
    let code_pref = settings
        .editor
        .font_family
        .as_deref()
        .or(settings.terminal.font_family.as_deref());
    let code_changed = roles::retarget(code, terminal_family(cache, code_pref), shift);
    ui_changed || code_changed
}

/// Resolve the source-editor family: explicit editor override wins,
/// otherwise the terminal code font (empty = follow terminal).
pub fn editor_family_name() -> Option<String> {
    let settings = EditorSettings::global();
    settings
        .editor
        .font_family
        .clone()
        .or(settings.terminal.font_family.clone())
}

/// Resolve the chosen interface family; bundled Inter is the default.
fn interface_family(cache: &mut FontCache, preferred: Option<&str>) -> FamilyId {
    if let Some(family) = preferred.filter(|family| !family.eq_ignore_ascii_case("Inter")) {
        #[cfg(target_os = "macos")]
        let loaded = if family.eq_ignore_ascii_case(SYSTEM_UI_FONT_LABEL) {
            cache.load_system_font("SF Pro Text")
        } else {
            concrete_family(cache, family)
        };
        #[cfg(not(target_os = "macos"))]
        let loaded = concrete_family(cache, family);
        if let Ok(id) = loaded {
            return id;
        }
        log::warn!("[fonts] interface font '{family}' unavailable; using {DEFAULT_UI_FONT_LABEL}");
    }
    bundled_inter(cache)
        .or_else(|_| cache.load_system_font("Inter"))
        .or_else(|_| cache.load_system_font("Segoe UI"))
        .or_else(|_| cache.load_system_font("Arial"))
        .unwrap_or_else(|_| terminal_family(cache, None))
}

/// A family by name: the bundled monospace families, else an installed one.
fn concrete_family(cache: &mut FontCache, family: &str) -> anyhow::Result<FamilyId> {
    if family.eq_ignore_ascii_case("Fira Code") {
        bundled_family(cache)
    } else if family.eq_ignore_ascii_case("JetBrains Mono") {
        bundled_jetbrains_mono(cache)
    } else {
        cache.load_system_font(family)
    }
}

/// Bundled Inter 4.1 from the official release, under SIL OFL 1.1.
pub fn bundled_inter(cache: &mut FontCache) -> anyhow::Result<FamilyId> {
    cache.load_family_from_bytes(
        "Inter",
        vec![
            include_bytes!("../assets/fonts/Inter-Light.ttf").to_vec(),
            include_bytes!("../assets/fonts/Inter-Regular.ttf").to_vec(),
            include_bytes!("../assets/fonts/Inter-Medium.ttf").to_vec(),
            include_bytes!("../assets/fonts/Inter-SemiBold.ttf").to_vec(),
            include_bytes!("../assets/fonts/Inter-Bold.ttf").to_vec(),
            include_bytes!("../assets/fonts/Inter-Italic.ttf").to_vec(),
            include_bytes!("../assets/fonts/Inter-BoldItalic.ttf").to_vec(),
        ],
    )
}

/// Load the bundled Fira Code family (Regular/Bold/Medium/Light) into `cache`.
pub fn bundled_family(cache: &mut FontCache) -> anyhow::Result<FamilyId> {
    cache.load_family_from_bytes(
        "Fira Code",
        vec![
            include_bytes!("../assets/fonts/FiraCode-Regular.ttf").to_vec(),
            include_bytes!("../assets/fonts/FiraCode-Bold.ttf").to_vec(),
            include_bytes!("../assets/fonts/FiraCode-Medium.ttf").to_vec(),
            include_bytes!("../assets/fonts/FiraCode-Light.ttf").to_vec(),
        ],
    )
}

/// A second bundled monospace family, available even without system fonts.
pub fn bundled_jetbrains_mono(cache: &mut FontCache) -> anyhow::Result<FamilyId> {
    cache.load_family_from_bytes(
        "JetBrains Mono",
        vec![
            include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").to_vec(),
            include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf").to_vec(),
        ],
    )
}

/// Resolve the chosen code family. Menlo is the macOS default.
fn terminal_family(cache: &mut FontCache, preferred: Option<&str>) -> FamilyId {
    if let Some(family) = preferred {
        if let Ok(id) = concrete_family(cache, family) {
            return id;
        }
        log::warn!(
            "[fonts] preferred font '{family}' unavailable; using {}",
            DEFAULT_TERMINAL_FONT_LABEL
        );
    }
    #[cfg(target_os = "macos")]
    if let Ok(id) = cache.load_system_font("Menlo") {
        return id;
    }
    bundled_family(cache)
        .or_else(|_| cache.load_system_font("Consolas"))
        .unwrap_or(FamilyId(0))
}
