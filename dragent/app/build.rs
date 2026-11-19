// Windows application icon and version resource embedding.
#![allow(clippy::disallowed_types)]

use std::env;
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::io::Write;
#[cfg(windows)]
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/dragent/icons/dragent-icon.png");

    validate_baked_urls();

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS");
    if target_os == "windows" {
        embed_windows_resources();
    }
    if target_os == "macos" {
        // Embed an Info.plist into the bare executable so TCC-gated APIs
        // (speech recognition, microphone) don't ABORT dev builds run outside
        // the .app bundle. The bundled app uses its own Info.plist.
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let plist = format!("{manifest_dir}/assets/resources/mac/Embedded-Info.plist");
        println!("cargo:rerun-if-changed={plist}");
        println!("cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{plist}");
    }
}

/// Fail the build if CI bakes a malformed endpoint into the binary.
///
/// These are `option_env!`-read at compile time (see `api/backend.rs`) and come
/// from GitHub Actions repo variables, so a typo there is invisible until a user
/// installs the release and finds that every single server call dies at URL
/// parse. That shipped once: `RELEASE_BACKEND_URL` held a URL that had been
/// copied while visually elided, so the host arrived as
/// `gqip22kh3m…awsapprunner.com` — a Unicode ellipsis (U+2026) standing in for
/// `.us-east-1.`. Sign-in "succeeded", no account ever loaded, nothing worked.
/// A non-ASCII byte in a host is never legitimate here, so refuse to build.
fn validate_baked_urls() {
    for var in ["DRAGENT_BACKEND_URL_DEFAULT", "DRAGENT_WEB_ORIGIN_DEFAULT"] {
        println!("cargo:rerun-if-env-changed={var}");
        let Ok(value) = env::var(var) else { continue };
        if value.trim().is_empty() {
            continue; // unset/blank → the code's own fallback applies
        }
        if let Err(why) = check_url(&value) {
            panic!(
                "\n\n  {var} is malformed: {why}\n  value: {value:?}\n\n  \
                 This is baked into the binary at compile time — shipping it \
                 would break every server call in the release.\n  Fix the CI \
                 variable (GitHub → Settings → Secrets and variables → Actions).\n\n"
            );
        }
    }
}

fn check_url(value: &str) -> Result<(), String> {
    if value != value.trim() {
        return Err("has leading or trailing whitespace".into());
    }
    let Some(rest) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return Err("must start with http:// or https://".into());
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Err("has no host".into());
    }
    if let Some(bad) = host.chars().find(|c| !c.is_ascii()) {
        return Err(format!(
            "host contains the non-ASCII character {bad:?} (U+{:04X}) — \
             a truncated/auto-formatted paste?",
            bad as u32
        ));
    }
    if let Some(bad) = host
        .chars()
        .find(|c| c.is_ascii_whitespace() || c.is_ascii_control())
    {
        return Err(format!("host contains an illegal character {bad:?}"));
    }
    Ok(())
}

#[cfg(windows)]
fn embed_windows_resources() {
    use ico::{IconDir, IconImage, ResourceType};
    use image::imageops::FilterType;

    const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let source_png = manifest_dir.join("assets/dragent/icons/dragent-icon.png");
    let icon_path = out_dir.join("dragent.ico");

    let image = image::open(&source_png)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", source_png.display()));

    let mut icon_dir = IconDir::new(ResourceType::Icon);
    for size in SIZES {
        let rgba = image
            .resize_exact(size, size, FilterType::Lanczos3)
            .to_rgba8();
        let icon_image = IconImage::from_rgba_data(size, size, rgba.into_raw());
        icon_dir.add_entry(
            ico::IconDirEntry::encode(&icon_image)
                .unwrap_or_else(|err| panic!("failed to encode {size}x{size} icon: {err}")),
        );
    }
    let icon_file = fs::File::create(&icon_path)
        .unwrap_or_else(|err| panic!("failed to create {}: {err}", icon_path.display()));
    icon_dir
        .write(icon_file)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", icon_path.display()));

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let (major, minor, patch) = parse_semver_prefix(&version);

    let resource_file_path = out_dir.join("dragent.rc");
    // RC.EXE parses C-style escapes inside quoted strings, so a raw Windows path
    // with single backslashes gets mangled — e.g. the `\t` in `\turningpointKS`
    // and `\target` becomes a TAB, yielding "file not found". Double every
    // backslash so RC sees literal path separators.
    let icon_path_rc = icon_path.display().to_string().replace('\\', "\\\\");
    let mut rcfile = fs::File::create(&resource_file_path)
        .unwrap_or_else(|err| panic!("failed to create resource file: {err}"));
    write!(
        rcfile,
        r#"
#pragma code_page(65001)
#include <winres.h>
#define IDI_ICON 0x101

IDI_ICON ICON "{icon_path}"
VS_VERSION_INFO VERSIONINFO
FILEVERSION     {major},{minor},{patch},0
PRODUCTVERSION  {major},{minor},{patch},0
FILEFLAGSMASK   VS_FFI_FILEFLAGSMASK
FILEFLAGS       0
FILEOS          VOS__WINDOWS32
FILETYPE        VFT_APP
FILESUBTYPE     VFT2_UNKNOWN
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904E4"
        BEGIN
            VALUE "CompanyName",      "Dragent Contributors\0"
            VALUE "FileDescription",  "Dragent\0"
            VALUE "FileVersion",      "{version}\0"
            VALUE "LegalCopyright",   "© Dragent Contributors\0"
            VALUE "InternalName",     "dragent\0"
            VALUE "OriginalFilename", "dragent.exe\0"
            VALUE "ProductName",      "Dragent\0"
            VALUE "ProductVersion",   "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1252
    END
END
"#,
        icon_path = icon_path_rc,
        major = major,
        minor = minor,
        patch = patch,
        version = version,
    )
    .unwrap();
    drop(rcfile);

    let target = env::var("TARGET").expect("TARGET");
    if let Some(tool) = cc::windows_registry::find_tool(target.as_str(), "cl.exe") {
        for (key, value) in tool.env() {
            env::set_var(key, value);
        }
    }

    embed_resource::compile(resource_file_path, embed_resource::NONE)
        .manifest_required()
        .unwrap_or_else(|err| panic!("Unable to embed Windows resources: {err:#}"));
}

#[cfg(not(windows))]
fn embed_windows_resources() {}

#[allow(dead_code)]
fn parse_semver_prefix(version: &str) -> (u16, u16, u16) {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}
