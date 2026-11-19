//! Every icon `AppIcon::path()` names must actually be embedded.
//!
//! A missing icon asset fails silently at runtime: `AssetProvider::get` errors,
//! `Icon::paint` logs a warning and draws nothing, and the UI just shows a gap
//! where the glyph should be. Nothing in the type system connects the string in
//! `icons.rs` to a file on disk, and `debug-embed` means a stale build can drop
//! a newly added file (see build.rs). This closes both gaps at test time.
//!
//! Reads the source rather than an `ALL` array so it cannot drift: adding a
//! variant is automatically covered.

use dragent_assets::DragentAssets;
use warpui_core::AssetProvider;

#[test]
fn every_referenced_icon_is_embedded() {
    let src_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../dragent_viewer/src/icons.rs");
    let src = std::fs::read_to_string(&src_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", src_path.display()));

    // Pull every "icons/….svg" string literal out of the path() match arms.
    let mut paths: Vec<&str> = src
        .match_indices("\"icons/")
        .filter_map(|(start, _)| {
            let rest = &src[start + 1..];
            rest.find('"').map(|end| &rest[..end])
        })
        .filter(|p| p.ends_with(".svg"))
        .collect();
    paths.sort_unstable();
    paths.dedup();

    assert!(
        paths.len() > 20,
        "extracted only {} icon paths — the parse probably broke, not the assets",
        paths.len()
    );

    let missing: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|p| match DragentAssets.get(p) {
            Ok(bytes) => bytes.is_empty(),
            Err(_) => true,
        })
        .collect();

    assert!(
        missing.is_empty(),
        "{} icon(s) referenced by AppIcon::path() are not embedded: {missing:?}\n\
         If the file exists on disk, dragent_assets is stale — \
         `cargo clean -p dragent_assets` and rebuild.",
        missing.len()
    );
    println!("checked {} embedded icons", paths.len());
}

/// Guards the test above from passing vacuously: `get` must actually fail for a
/// path that is not embedded.
#[test]
fn missing_icon_is_detected() {
    assert!(DragentAssets
        .get("icons/definitely-not-an-icon.svg")
        .is_err());
}

#[test]
fn dynamic_language_assets_are_embedded() {
    for path in ["icons/go.svg", "icons/typescript.svg", "icons/rust.svg"] {
        let bytes = DragentAssets
            .get(path)
            .unwrap_or_else(|e| panic!("missing dynamic language asset {path}: {e}"));
        assert!(!bytes.is_empty(), "dynamic language asset {path} is empty");
    }
}
