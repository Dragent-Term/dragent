//! `dragent` in terminals outside Dragent: the launcher folder goes on
//! PATH through the user's shell profile (or, on Windows, their user PATH).
//! Only added when the user asks.

use std::path::{Path, PathBuf};

const MARKER: &str = "# Added by Dragent: the `dragent` command";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Syntax {
    Posix,
    Fish,
}

/// The profile file new terminals read, for the user's shell.
pub struct Profile {
    pub path: PathBuf,
    syntax: Syntax,
}

impl Profile {
    /// `~/.zshrc`-style display path.
    pub fn display(&self) -> String {
        match dirs::home_dir().and_then(|home| self.path.strip_prefix(home).ok().map(Path::to_path_buf)) {
            Some(rest) => format!("~/{}", rest.display()),
            None => self.path.display().to_string(),
        }
    }

    pub fn installed(&self) -> bool {
        std::fs::read_to_string(&self.path).is_ok_and(|text| text.contains(MARKER))
    }

    /// Append the PATH line (once).
    pub fn install(&self, bin: &Path) -> Result<(), String> {
        if self.installed() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("couldn't create {}: {error}", parent.display()))?;
        }
        let existing = std::fs::read_to_string(&self.path).unwrap_or_default();
        let separator = if existing.is_empty() || existing.ends_with("\n\n") {
            ""
        } else if existing.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let text = format!("{existing}{separator}{MARKER}\n{}\n", path_line(self.syntax, bin));
        std::fs::write(&self.path, text)
            .map_err(|error| format!("couldn't update {}: {error}", self.display()))
    }
}

/// The profile for `shell` (a path or name), on this platform.
pub fn profile_for(shell: &str, home: &Path) -> Option<Profile> {
    let name = Path::new(shell).file_name()?.to_str()?;
    let (path, syntax) = match name {
        "zsh" => (home.join(".zshrc"), Syntax::Posix),
        "bash" if cfg!(target_os = "macos") => (home.join(".bash_profile"), Syntax::Posix),
        "bash" => (home.join(".bashrc"), Syntax::Posix),
        // Its own file, so nothing of the user's is edited.
        "fish" => (home.join(".config/fish/conf.d/dragent.fish"), Syntax::Fish),
        "sh" | "dash" | "ksh" => (home.join(".profile"), Syntax::Posix),
        _ => return None,
    };
    Some(Profile { path, syntax })
}

/// The user's profile: the shell Dragent starts, else their login shell.
pub fn profile() -> Option<Profile> {
    if cfg!(windows) {
        return None;
    }
    let shell = crate::console::editor_settings::EditorSettings::global()
        .terminal
        .default_shell
        .clone()
        .or_else(|| std::env::var("SHELL").ok())?;
    profile_for(&shell, &dirs::home_dir()?)
}

/// Where "Add to shell" puts the launcher folder.
pub enum Setup {
    Profile(Profile),
    /// Windows: the user's `Path` environment variable.
    UserPath,
}

impl Setup {
    /// The setup for this computer, if Dragent can do one.
    pub fn current() -> Option<Self> {
        if cfg!(windows) {
            Some(Self::UserPath)
        } else {
            profile().map(Self::Profile)
        }
    }

    /// Where it goes, for the user: `~/.zshrc`, "your user PATH".
    pub fn place(&self) -> String {
        match self {
            Self::Profile(profile) => profile.display(),
            Self::UserPath => "your user PATH".into(),
        }
    }

    pub fn installed(&self, bin: &Path) -> bool {
        match self {
            Self::Profile(profile) => profile.installed(),
            #[cfg(windows)]
            Self::UserPath => user_path::contains(bin),
            #[cfg(not(windows))]
            Self::UserPath => {
                let _ = bin;
                false
            }
        }
    }

    pub fn install(&self, bin: &Path) -> Result<(), String> {
        match self {
            Self::Profile(profile) => profile.install(bin),
            #[cfg(windows)]
            Self::UserPath => user_path::add(bin),
            #[cfg(not(windows))]
            Self::UserPath => {
                let _ = bin;
                Err("Only on Windows.".into())
            }
        }
    }
}

/// The user's `Path` variable (`HKCU\Environment`). Read and written raw so
/// its type is kept: it's usually `REG_EXPAND_SZ`, and rewriting it as a
/// plain string would stop entries like `%USERPROFILE%\…` expanding.
#[cfg(windows)]
mod user_path {
    use std::path::Path;

    use windows::core::w;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, LPARAM, WPARAM};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    fn open() -> Result<Key, String> {
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(HKEY_CURRENT_USER, w!("Environment"), None, KEY_READ | KEY_WRITE, &mut key)
        };
        if status != ERROR_SUCCESS {
            return Err(format!("couldn't open your environment settings (error {})", status.0));
        }
        Ok(Key(key))
    }

    /// The user Path and its type (`REG_EXPAND_SZ` when there's none yet).
    fn read(key: &Key) -> Result<(String, REG_VALUE_TYPE), String> {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let status = unsafe {
            RegQueryValueExW(key.0, w!("Path"), None, Some(&mut kind), None, Some(&mut size))
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        if status != ERROR_SUCCESS {
            return Err(format!("couldn't read your PATH (error {})", status.0));
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                w!("Path"),
                None,
                Some(&mut kind),
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("couldn't read your PATH (error {})", status.0));
        }
        let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Ok((String::from_utf16_lossy(&buffer[..end]), kind))
    }

    /// `%NAME%` references expanded, for comparing entries.
    fn expand(entry: &str) -> String {
        let mut out = String::new();
        let mut rest = entry;
        while let Some(start) = rest.find('%') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            match after.find('%') {
                Some(end) => {
                    let name = &after[..end];
                    out.push_str(&std::env::var(name).unwrap_or_else(|_| format!("%{name}%")));
                    rest = &after[end + 1..];
                }
                None => {
                    out.push_str(&rest[start..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn same(entry: &str, bin: &Path) -> bool {
        let normal = |text: &str| text.trim().trim_end_matches(['\\', '/']).to_lowercase();
        normal(&expand(entry)) == normal(&bin.display().to_string())
    }

    pub fn contains(bin: &Path) -> bool {
        open()
            .and_then(|key| read(&key))
            .is_ok_and(|(path, _)| path.split(';').any(|entry| same(entry, bin)))
    }

    pub fn add(bin: &Path) -> Result<(), String> {
        let key = open()?;
        let (path, kind) = read(&key)?;
        if path.split(';').any(|entry| same(entry, bin)) {
            return Ok(());
        }
        let existing = path.trim().trim_end_matches(';');
        let path = if existing.is_empty() {
            bin.display().to_string()
        } else {
            format!("{existing};{}", bin.display())
        };
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let bytes: Vec<u8> = wide.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        let kind = if kind == REG_SZ { REG_SZ } else { REG_EXPAND_SZ };
        let status = unsafe { RegSetValueExW(key.0, w!("Path"), None, kind, Some(&bytes)) };
        if status != ERROR_SUCCESS {
            return Err(format!("couldn't update your PATH (error {})", status.0));
        }
        // New terminals started from Explorer see it without signing out.
        unsafe {
            let _ = SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(w!("Environment").as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                5000,
                None,
            );
        }
        Ok(())
    }
}

/// `bin` as written in a profile: relative to `$HOME` when it can be.
fn path_line(syntax: Syntax, bin: &Path) -> String {
    let home_relative = dirs::home_dir()
        .and_then(|home| bin.strip_prefix(home).ok().map(Path::to_path_buf));
    match syntax {
        Syntax::Posix => {
            let bin = match &home_relative {
                Some(rest) => format!("$HOME/{}", rest.display()),
                None => bin.display().to_string(),
            };
            format!("export PATH=\"{}:$PATH\"", bin.replace('"', "\\\""))
        }
        Syntax::Fish => {
            let bin = match &home_relative {
                Some(rest) => format!("$HOME/{}", rest.display()),
                None => bin.display().to_string(),
            };
            let bin = format!("\"{}\"", bin.replace('"', "\\\""));
            format!("contains {bin} $PATH; or set -gx PATH {bin} $PATH")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_shell_gets_its_own_profile() {
        let home = Path::new("/home/me");
        assert_eq!(profile_for("/bin/zsh", home).unwrap().path, home.join(".zshrc"));
        assert_eq!(
            profile_for("/usr/local/bin/fish", home).unwrap().path,
            home.join(".config/fish/conf.d/dragent.fish")
        );
        assert!(profile_for("/usr/bin/nu", home).is_none());
    }

    #[test]
    fn installing_appends_once_and_keeps_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile {
            path: dir.path().join(".zshrc"),
            syntax: Syntax::Posix,
        };
        std::fs::write(&profile.path, "alias ll='ls -l'").unwrap();
        assert!(!profile.installed());
        profile.install(Path::new("/opt/dragent/bin")).unwrap();
        profile.install(Path::new("/opt/dragent/bin")).unwrap();
        let text = std::fs::read_to_string(&profile.path).unwrap();
        assert_eq!(
            text,
            format!("alias ll='ls -l'\n\n{MARKER}\nexport PATH=\"/opt/dragent/bin:$PATH\"\n")
        );
        assert!(profile.installed());
    }

    #[test]
    fn the_line_works_in_its_shell() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin dir");
        std::fs::create_dir_all(&bin).unwrap();
        let profile = Profile {
            path: dir.path().join("profile.sh"),
            syntax: Syntax::Posix,
        };
        profile.install(&bin).unwrap();
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(". '{}'; printf %s \"$PATH\"", profile.path.display()))
            .output()
            .unwrap();
        let path = String::from_utf8(output.stdout).unwrap();
        assert!(path.starts_with(&format!("{}:", bin.display())), "{path}");
    }
}
