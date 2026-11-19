//! Custom URL scheme registration for `dragent://` auth callbacks.
//!
//! Sign-in works on every platform without deep links (background polling + paste).
//! Deep links are optional accelerators:
//! - **macOS:** `CFBundleURLTypes` in `Dragent-Info.plist`
//! - **Linux:** `MimeType=x-scheme-handler/dragent` in `dragent.desktop` (and user
//!   `~/.local/share/applications/` at startup for dev/portable builds)
//! - **Windows:** registry entry under `HKCU\Software\Classes\dragent` (registered at startup)

pub const CUSTOM_SCHEME: &str = "dragent";

static DEEP_LINK_RECEIVER: once_cell::sync::OnceCell<async_channel::Receiver<String>> =
    once_cell::sync::OnceCell::new();

pub fn set_deep_link_receiver(rx: async_channel::Receiver<String>) {
    let _ = DEEP_LINK_RECEIVER.set(rx);
}

pub fn get_deep_link_receiver() -> Option<async_channel::Receiver<String>> {
    DEEP_LINK_RECEIVER.get().cloned()
}

/// Register the custom scheme handler where the OS expects runtime registration.
pub fn register_scheme_handler() {
    #[cfg(windows)]
    register_windows();
    #[cfg(target_os = "linux")]
    register_linux();
}

#[cfg(windows)]
fn register_windows() {
    use std::ffi::OsString;

    use windows_registry::{CURRENT_USER, HSTRING};

    let Ok(classes_key) = CURRENT_USER.open("Software\\Classes") else {
        log::error!("Failed to open HKCU\\Software\\Classes for URI registration");
        return;
    };

    match classes_key.create(CUSTOM_SCHEME) {
        Ok(parent_key) => {
            if let Err(err) = parent_key.set_string("", "Dragent") {
                log::error!("Could not set URI scheme display name: {err:?}");
                return;
            }
            if let Err(err) = parent_key.set_string("URL Protocol", "") {
                log::error!("Could not set URI scheme URL Protocol key: {err:?}");
                return;
            }

            let command_key = match parent_key.create("shell\\open\\command") {
                Ok(command_key) => command_key,
                Err(err) => {
                    log::error!("Could not create shell\\open\\command key: {err:?}");
                    return;
                }
            };

            let command = match std::env::current_exe() {
                Ok(path) => {
                    let mut command = OsString::new();
                    command.push("\"");
                    command.push(path.as_os_str());
                    command.push("\" \"%1\"");
                    HSTRING::from(command.as_os_str())
                }
                Err(err) => {
                    log::error!(
                        "Could not resolve executable path for URI scheme registration: {err:?}"
                    );
                    return;
                }
            };

            if let Err(err) = command_key.set_hstring("", &command) {
                log::error!("Could not set shell command for URI scheme: {err:?}");
            }
        }
        Err(err) => {
            log::error!("Failed to create URI scheme registry entry: {err:?}");
        }
    }
}

/// Dev / portable Linux: register `dragent://` in the user's application menu.
#[cfg(target_os = "linux")]
fn register_linux() {
    use std::fs;
    use std::path::PathBuf;

    let Some(home) = dirs::home_dir() else {
        log::warn!("cannot register dragent:// handler: home directory unavailable");
        return;
    };

    let apps_dir: PathBuf = home.join(".local/share/applications");
    if let Err(err) = fs::create_dir_all(&apps_dir) {
        log::warn!("cannot create {apps_dir:?} for URI handler: {err}");
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            log::warn!("cannot resolve executable for URI handler: {err}");
            return;
        }
    };

    let desktop = format!(
        r#"[Desktop Entry]
Type=Application
Name=Dragent
GenericName=Terminal
Comment=Dragent auth callback handler
Exec="{}" %u
Icon=dragent
StartupWMClass=dragent
Terminal=false
MimeType=x-scheme-handler/dragent;
NoDisplay=true
"#,
        exe.display()
    );

    let desktop_path = apps_dir.join("dragent-url-handler.desktop");
    if let Err(err) = fs::write(&desktop_path, desktop) {
        log::warn!("failed to write {}: {err}", desktop_path.display());
        return;
    }

    // Best-effort; packaged installs use `package-linux.sh` instructions instead.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&apps_dir)
        .status();
    let _ = std::process::Command::new("xdg-mime")
        .args([
            "default",
            "dragent-url-handler.desktop",
            "x-scheme-handler/dragent",
        ])
        .status();
}
