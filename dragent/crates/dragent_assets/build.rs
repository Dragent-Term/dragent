//! Make cargo rebuild this crate when an asset is added, changed, or removed.
//!
//! `rust-embed` is configured with `debug-embed`, so asset bytes are compiled
//! into the binary in every profile, not read from disk at runtime. The derive
//! macro cannot tell cargo which files it read, so without this script cargo
//! sees an unchanged `lib.rs`, skips the crate, and keeps the previously
//! embedded set.
//!
//! The failure that causes is silent and very confusing: a newly added icon is
//! simply absent from the binary, `AssetProvider::get` returns "no dragent asset
//! at path …", and `Icon::paint` logs a warning and draws nothing. The UI shows
//! a blank space where the icon should be, the SVG on disk is perfectly valid,
//! and rebuilding does not help — only a `cargo clean -p dragent_assets` or an
//! unrelated edit to this crate does. Adding `icons/bot.svg` hit exactly this.
//!
//! Directories are watched recursively: cargo re-runs the build when a watched
//! directory's listing changes, which is what catches *new* files.

fn main() {
    // Relative paths, forward slashes: cargo resolves these against
    // CARGO_MANIFEST_DIR and accepts `/` on Windows too, so this stays correct
    // cross-platform. Keep in step with the `#[folder = …]` attributes in
    // src/lib.rs.
    for dir in [
        "../../app/assets/dragent",
        "../../app/assets/onboarding",
        "../dragent_viewer/assets/themes",
    ] {
        println!("cargo:rerun-if-changed={dir}");
    }
}
