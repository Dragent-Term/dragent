//! Embeds every file of every bundled skill package (`skills/<name>/…`):
//! SKILL.md, its references, helper scripts and small assets. The native
//! agent, the external-agent installer and session plugins all read this one
//! catalog, so instructions and resources always come from the same build.

use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Packages stay small: they're compiled into the app.
const MAX_FILE_BYTES: u64 = 512 * 1024;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest.join("skills");
    println!("cargo:rerun-if-changed=skills");

    let mut packages = Vec::new();
    let mut names: Vec<PathBuf> = fs::read_dir(&root)
        .expect("skills directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_plain_dir(path))
        .collect();
    names.sort();
    for dir in names {
        let name = dir.file_name().and_then(|n| n.to_str()).expect("UTF-8 skill name").to_string();
        assert!(
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "skill folder `{name}` must use lowercase letters, digits and hyphens"
        );
        assert!(dir.join("SKILL.md").is_file(), "skill `{name}` has no SKILL.md");
        let mut files = Vec::new();
        collect(&dir, &dir, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));
        packages.push((name, dir, files));
    }

    // Version: changes whenever any packaged byte or path changes, so an
    // installed copy is never a mix of two builds' files.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let mut code = String::from("pub const BUNDLED_SKILLS: &[BundledSkill] = &[\n");
    for (name, dir, files) in &packages {
        feed(name.as_bytes());
        let skill_md = dir.join("SKILL.md");
        writeln!(
            code,
            "    BundledSkill {{ name: {name:?}, content: include_str!({:?}), files: &[",
            skill_md.display().to_string()
        )
        .unwrap();
        for (relative, path, executable) in files {
            feed(relative.as_bytes());
            feed(&fs::read(path).expect("read skill file"));
            writeln!(
                code,
                "        BundledFile {{ path: {relative:?}, bytes: include_bytes!({:?}), executable: {executable} }},",
                path.display().to_string()
            )
            .unwrap();
        }
        code.push_str("    ] },\n");
    }
    code.push_str("];\n");
    writeln!(code, "pub const BUNDLE_VERSION: &str = \"{hash:016x}\";").unwrap();

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("bundled_skills.rs");
    fs::write(out, code).expect("write bundled_skills.rs");
}

fn is_plain_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
        && !path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

/// Regular files under `dir`, as (`/`-separated relative path, absolute path,
/// executable). Links, hidden files and anything outside the package are
/// refused, so nothing unexpected is compiled in or installed.
fn collect(base: &Path, dir: &Path, files: &mut Vec<(String, PathBuf, bool)>) {
    let mut entries: Vec<_> = fs::read_dir(dir).expect("read skill folder").flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_str().expect("UTF-8 skill file name");
        if file_name.starts_with('.') {
            continue;
        }
        let meta = fs::symlink_metadata(&path).expect("skill file metadata");
        assert!(!meta.file_type().is_symlink(), "{} is a link; packages hold plain files", path.display());
        if meta.is_dir() {
            collect(base, &path, files);
            continue;
        }
        assert!(meta.len() <= MAX_FILE_BYTES, "{} is over the package file size limit", path.display());
        let relative = path.strip_prefix(base).expect("inside the package");
        assert!(
            relative.components().all(|c| matches!(c, Component::Normal(_))),
            "{} escapes its package",
            path.display()
        );
        let relative = relative
            .components()
            .map(|c| c.as_os_str().to_str().expect("UTF-8"))
            .collect::<Vec<_>>()
            .join("/");
        files.push((relative.clone(), path, is_executable(&meta, &relative)));
    }
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata, relative: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0 || relative.starts_with("scripts/")
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata, relative: &str) -> bool {
    relative.starts_with("scripts/")
}
