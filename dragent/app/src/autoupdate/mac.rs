//! macOS update applier: mount the downloaded DMG, stage the new `.app` next
//! to the installed one, verify its code signature, then atomically swap the
//! bundles with `renamex_np(RENAME_SWAP)` — the same mechanism Warp uses. The
//! running process keeps its (parked) old executable; the NEXT launch starts
//! the new version.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Apply the downloaded DMG. Blocking; call from the updater thread only.
pub(super) fn apply(dmg: &Path, version: &str) -> Result<(), String> {
    let bundle = current_bundle()?;
    let bundle_parent = bundle
        .parent()
        .ok_or_else(|| "bundle has no parent directory".to_string())?;

    // Never touch an install we can't rewrite (e.g. /Applications without
    // permission, or a translocated read-only mount).
    for dir in [&bundle, &bundle_parent.to_path_buf()] {
        let probe = dir.join(".dragent-write-probe");
        std::fs::write(&probe, b"x").map_err(|e| format!("{} not writable: {e}", dir.display()))?;
        let _ = std::fs::remove_file(&probe);
    }

    let mount = mount_dmg(dmg)?;
    let result = stage_and_swap(&mount, &bundle, bundle_parent, version);
    unmount_dmg(&mount);
    result?;

    super::cleanup_other_versions(version);
    Ok(())
}

fn stage_and_swap(
    mount: &Path,
    bundle: &Path,
    bundle_parent: &Path,
    version: &str,
) -> Result<(), String> {
    // Find the .app inside the DMG.
    let new_app = std::fs::read_dir(mount)
        .map_err(|e| format!("read dmg mount: {e}"))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| "no .app found in DMG".to_string())?;

    // Stage into the SAME directory as the installed bundle so the final
    // rename is same-filesystem (required for an atomic swap).
    let staged = bundle_parent.join(format!(".Dragent-update-{version}.app"));
    let _ = std::fs::remove_dir_all(&staged);
    run(
        Command::new("ditto").arg(&new_app).arg(&staged),
        "copy new app out of DMG",
    )?;

    // Always verify the staged replacement before the existing atomic-swap
    // gate, including upgrades from unsigned or damaged installations.
    if !codesign_verifies(&staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(
            "new bundle failed Dragent signing-identity verification — update aborted".into(),
        );
    }

    // Sanity: the new bundle actually contains our executable.
    let new_exe = staged.join("Contents/MacOS/dragent");
    if !new_exe.exists() {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!(
            "new bundle missing executable at {}",
            new_exe.display()
        ));
    }

    // Atomic swap: after this, `bundle` is the new version and `staged` holds
    // the old one.
    renamex_swap(&staged, bundle)?;

    // Park the old (still-running) executable outside the old bundle so we can
    // delete the rest — deleting a running executable's file breaks fork().
    let parked_dir = super::updates_dir().join("old-executables");
    let _ = std::fs::create_dir_all(&parked_dir);
    let parked = parked_dir.join(format!("dragent-replaced-by-{version}"));
    let _ = std::fs::remove_file(&parked);
    let old_exe = staged.join("Contents/MacOS/dragent");
    if let Err(e) = std::fs::rename(&old_exe, &parked) {
        // Cross-filesystem or already parked — fall back to leaving the old
        // bundle in place; it's hidden (dot-prefixed) and cleaned next update.
        log::warn!("[autoupdate] could not park old executable: {e} — leaving old bundle");
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(&staged);
    Ok(())
}

/// The .app bundle this process is running from. Errors out for non-bundle
/// contexts (cargo run, bare binary), which disables updating in dev.
fn current_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    // …/Dragent.app/Contents/MacOS/dragent → up 3.
    let bundle = exe
        .ancestors()
        .nth(3)
        .ok_or_else(|| "executable not inside an .app bundle".to_string())?
        .to_path_buf();
    if bundle.extension().is_some_and(|e| e == "app") {
        Ok(bundle)
    } else {
        Err(format!(
            "not running from an .app bundle ({})",
            exe.display()
        ))
    }
}

fn codesign_verifies(bundle: &Path) -> bool {
    Command::new("/usr/bin/codesign")
        .args(super::signature::macos_codesign_arguments())
        .arg(bundle)
        .output()
        .is_ok_and(|o| super::signature::macos_verification_succeeded(o.status.code()))
}

/// Atomic bidirectional rename (macOS `renamex_np` with `RENAME_SWAP`).
fn renamex_swap(a: &Path, b: &Path) -> Result<(), String> {
    let ca = CString::new(a.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let cb = CString::new(b.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let rc = unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!(
            "renamex_np swap failed: {}",
            std::io::Error::last_os_error()
        ))
    }
}

fn mount_dmg(dmg: &Path) -> Result<PathBuf, String> {
    let mountpoint = super::updates_dir().join("mnt");
    let _ = std::fs::create_dir_all(&mountpoint);
    run(
        Command::new("hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-noautoopen",
                "-readonly",
                "-mountpoint",
            ])
            .arg(&mountpoint)
            .arg(dmg),
        "mount update DMG",
    )?;
    Ok(mountpoint)
}

fn unmount_dmg(mountpoint: &Path) {
    let detached = Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(mountpoint)
        .status()
        .is_ok_and(|s| s.success());
    if !detached {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(mountpoint)
            .status();
    }
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: spawn: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{what}: {} — {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    #[test]
    fn native_codesign_rejects_other_signer_unsigned_and_tampered_code() {
        // A real Apple-signed system binary demonstrates why signature
        // validity alone is insufficient; no private signing keys are needed.
        let other_signer = Path::new("/bin/ls");
        assert!(Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(other_signer)
            .status()
            .unwrap()
            .success());
        assert!(!codesign_verifies(other_signer));

        let scratch = tempfile::tempdir().unwrap();
        let unsigned = scratch.path().join("unsigned");
        std::fs::write(&unsigned, b"unsigned update").unwrap();
        assert!(!codesign_verifies(&unsigned));

        let tampered = scratch.path().join("tampered");
        std::fs::copy(other_signer, &tampered).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&tampered)
            .unwrap();
        file.seek(SeekFrom::Start(16 * 1024)).unwrap();
        let mut byte = [0];
        file.read_exact(&mut byte).unwrap();
        byte[0] ^= 1;
        file.seek(SeekFrom::Start(16 * 1024)).unwrap();
        file.write_all(&byte).unwrap();
        drop(file);
        assert!(!codesign_verifies(&tampered));
    }
}
