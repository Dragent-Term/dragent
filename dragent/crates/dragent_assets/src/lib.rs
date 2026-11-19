//! Dragent asset provider — embeds Lucide icons and onboarding PNGs.
//!
//! Do not use `warp_assets` or legacy bundled SVG assets. See `.cursor/skills/dragent-licensing/`.

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use rust_embed::RustEmbed;
use warpui_core::AssetProvider;

/// Embedded Lucide icons from `app/assets/dragent/icons/`.
#[derive(Clone, Copy, RustEmbed)]
#[folder = "../../app/assets/dragent"]
#[include = "icons/**"]
#[include = "agent-images/**"]
struct DragentIconAssets;

/// Onboarding marketing screenshots from `app/assets/onboarding/`.
#[derive(Clone, Copy, RustEmbed)]
#[folder = "../../app/assets/onboarding"]
#[include = "**/*.png"]
struct OnboardingImageAssets;

/// Theme background images (Apache-2.0 Warp catalog), addressed by their
/// themes-root-relative path (e.g. `warp_bundled/foo_bg.jpg`). The themes folder
/// moved into `dragent_viewer` (it also holds the theme YAMLs).
///
/// Native-only: these are ~17MB and would bloat the wasm bundle. The web viewer
/// fetches them by URL from `/wasm-themes/…` instead (see
/// `dragent_viewer::theme::BgImage::asset_source`), so on wasm this is an empty
/// stub — nothing embedded.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, RustEmbed)]
#[folder = "../dragent_viewer/assets/themes"]
#[include = "**/*.jpg"]
#[include = "**/*.png"]
struct ThemeImageAssets;

#[cfg(target_family = "wasm")]
struct ThemeImageAssets;
#[cfg(target_family = "wasm")]
impl ThemeImageAssets {
    fn get(_path: &str) -> Option<rust_embed::EmbeddedFile> {
        None
    }
}

#[derive(Clone, Copy)]
pub struct DragentAssets;

impl AssetProvider for DragentAssets {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>> {
        DragentIconAssets::get(path)
            .map(|file| file.data)
            .or_else(|| OnboardingImageAssets::get(path).map(|file| file.data))
            .or_else(|| ThemeImageAssets::get(path).map(|file| file.data))
            .ok_or_else(|| anyhow!("no dragent asset at path {path}"))
    }
}

impl DragentAssets {
    pub fn has_onboarding_asset(path: &str) -> bool {
        OnboardingImageAssets::get(path).is_some()
    }
}
