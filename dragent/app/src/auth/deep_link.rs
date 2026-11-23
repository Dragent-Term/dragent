//! Custom URL scheme registration for auth callbacks (`dragent://` in the
//! Release edition, `dragent-dev://` in Dev; see `dragent_paths::url_scheme`).
//!
//! Sign-in works on every platform without deep links (background polling + paste).
//! Deep links are optional accelerators:
//! - **macOS:** `CFBundleURLTypes` in `Dragent-Info.plist`
//! - **Linux:** `MimeType=x-scheme-handler/<scheme>` in the `.desktop` file (and user
//!   `~/.local/share/applications/` at startup for dev/portable builds)
//! - **Windows:** registry entry under `HKCU\Software\Classes\<scheme>` (registered at startup)
//!
//! A Dev build registers only its own `dragent-dev` scheme, so its links never
//! reach the installed app — and it never claims the installed app's `dragent`
//! links.

/// This build's custom URL scheme (`dragent_paths::url_scheme()`).
pub fn custom_scheme() -> &'static str {
    dragent_paths::url_scheme()
}

static DEEP_LINK_RECEIVER: once_cell::sync::OnceCell<async_channel::Receiver<String>> =
    once_cell::sync::OnceCell::new();

pub fn set_deep_link_receiver(rx: async_channel::Receiver<String>) {
    let _ = DEEP_LINK_RECEIVER.set(rx);
}

pub fn get_deep_link_receiver() -> Option<async_channel::Receiver<String>> {
    DEEP_LINK_RECEIVER.get().cloned()
}

/// Register the custom scheme handler where the OS expects runtime registration.
///
/// A Dev build never touches handler registration: the registry entry,
/// application menu and default handler are left alone.
pub fn register_scheme_handler() {
    if dragent_paths::is_dev() {
        log::info!("[auth] Dev build: leaving the link handler registration alone");
        return;
    }
    #[cfg(windows)]
    register_windows();
    #[cfg(target_os = "linux")]
    register_linux();
}

#[cfg(windows)]
fn register_windows() {
    use std::ffi::OsString;

    use windows_registry::{CURRENT_USER, HSTRING};

    let scheme = custom_scheme();
    let app_name = dragent_paths::app_name();
    let Ok(classes_key) = CURRENT_USER.open("Software\\Classes") else {
        log::error!("Failed to open HKCU\\Software\\Classes for URI registration");
        return;
    };

    match classes_key.create(scheme) {
        Ok(parent_key) => {
            if let Err(err) = parent_key.set_string("", app_name) {
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

/// Dev / portable Linux: register this edition's scheme in the user's application menu.
#[cfg(target_os = "linux")]
fn register_linux() {
    use std::fs;
    use std::path::PathBuf;

    let scheme = custom_scheme();
    let app_name = dragent_paths::app_name();
    let desktop_file = format!("{scheme}-url-handler.desktop");
    let mime = format!("x-scheme-handler/{scheme}");

    let Some(home) = dirs::home_dir() else {
        log::warn!("cannot register {scheme}:// handler: home directory unavailable");
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
Name={app_name}
GenericName=Terminal
Comment={app_name} auth callback handler
Exec="{exe}" %u
Icon=dragent
StartupWMClass=dragent
Terminal=false
MimeType={mime};
NoDisplay=true
"#,
        exe = exe.display()
    );

    let desktop_path = apps_dir.join(&desktop_file);
    if let Err(err) = fs::write(&desktop_path, desktop) {
        log::warn!("failed to write {}: {err}", desktop_path.display());
        return;
    }

    // Best-effort; packaged installs use `package-linux.sh` instructions instead.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&apps_dir)
        .status();
    let _ = std::process::Command::new("xdg-mime")
        .args(["default", &desktop_file, &mime])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_builds_leave_the_link_handler_registration_alone() {
        // Registering must write nothing into the user's application menu,
        // whatever this edition's desktop file would be called.
        if !dragent_paths::is_dev() {
            return;
        } // only meaningful in the Dev edition
        let home = tempfile::tempdir().unwrap();
        crate::test_env::with_vars(&[("HOME", Some(home.path()))], || {
            register_scheme_handler();
            assert!(!home
                .path()
                .join(format!(
                    ".local/share/applications/{}-url-handler.desktop",
                    custom_scheme()
                ))
                .exists());
        });
    }
}
