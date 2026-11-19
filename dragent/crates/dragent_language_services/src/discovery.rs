use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn find_project_executable(root: &Path, name: &str) -> Option<PathBuf> {
    let bin = root.join("node_modules").join(".bin");
    find_in_directory(&bin, name)
}

pub fn find_managed_executable(name: &str) -> Option<PathBuf> {
    let root = crate::installer::managed_root();
    let candidates = [
        root.join("npm").join("node_modules").join(".bin"),
        root.join("bin"),
    ];
    candidates
        .into_iter()
        .find_map(|directory| find_in_directory(&directory, name))
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    find_executable_for_project(name, Path::new("."))
}

/// Resolve a user-installed executable in the context of a project. Rustup
/// proxies are workspace-sensitive, so resolving them from Dragent's process
/// directory can select the wrong toolchain (or report a component missing).
pub fn find_executable_for_project(name: &str, project_root: &Path) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    executable_search_directories()
        .into_iter()
        .find_map(|directory| {
            let candidate = find_in_directory(&directory, name)?;
            resolve_toolchain_proxy(candidate, name, project_root)
        })
}

/// Return executable directories for both terminal and Finder/Dock launches.
/// macOS GUI applications do not reliably inherit the user's shell PATH, so a
/// release app launched from Finder can otherwise fail to find npm, rustup, or
/// go even though those tools work in the user's terminal.
fn executable_search_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut add_path = |path: &str| {
        for directory in std::env::split_paths(std::ffi::OsStr::new(path)) {
            if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
                directories.push(directory);
            }
        }
    };

    if let Ok(path) = std::env::var("PATH") {
        add_path(&path);
    }
    drop(add_path);

    // Dragent's private Node runtime must be available both to npm itself and
    // to the JavaScript language-server shims npm installs.
    let managed_node = crate::installer::managed_root().join("runtime/node");
    for directory in [managed_node.clone(), managed_node.join("bin")] {
        if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
            directories.push(directory);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(path) = login_shell_path() {
            for directory in std::env::split_paths(std::ffi::OsStr::new(&path)) {
                if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
                    directories.push(directory);
                }
            }
        }
        // Keep these fallbacks even if the login shell is unavailable. They
        // cover the standard Apple Silicon/Intel Homebrew and Rust locations.
        for directory in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            if !directories
                .iter()
                .any(|known| known == Path::new(directory))
            {
                directories.push(PathBuf::from(directory));
            }
        }
        if let Some(home) = dirs::home_dir() {
            for directory in [
                home.join(".cargo/bin"),
                home.join(".local/bin"),
                home.join(".volta/bin"),
                home.join(".asdf/shims"),
            ] {
                if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
                    directories.push(directory);
                }
            }
        }
    }

    #[cfg(windows)]
    {
        // Explorer-launched applications can retain an older PATH after a
        // toolchain is installed. Probe the conventional per-user and system
        // locations so npm, Rustup, and Go work without launching Dragent from
        // a developer terminal.
        if let Some(home) = dirs::home_dir() {
            for directory in [
                home.join(".cargo/bin"),
                home.join("go/bin"),
                home.join("AppData/Roaming/npm"),
                home.join("AppData/Local/Programs/nodejs"),
            ] {
                if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
                    directories.push(directory);
                }
            }
        }
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(variable) {
                for directory in [
                    PathBuf::from(&root).join("nodejs"),
                    PathBuf::from(root).join("Go/bin"),
                ] {
                    if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
                        directories.push(directory);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for directory in ["/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"] {
            let directory = PathBuf::from(directory);
            if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
                directories.push(directory);
            }
        }
        if let Some(home) = dirs::home_dir() {
            for directory in [
                home.join(".cargo/bin"),
                home.join(".local/bin"),
                home.join("go/bin"),
                home.join(".npm-global/bin"),
                home.join(".volta/bin"),
            ] {
                if directory.is_dir() && !directories.iter().any(|known| known == &directory) {
                    directories.push(directory);
                }
            }
        }
    }

    directories
}

/// PATH suitable for child processes started by a GUI-launched release app.
pub(crate) fn runtime_path() -> Option<OsString> {
    std::env::join_paths(executable_search_directories()).ok()
}

#[cfg(target_os = "macos")]
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| Path::new(shell).is_file())
        .unwrap_or_else(|| "/bin/zsh".to_string());
    let output = crate::background_command(shell)
        .args(["-l", "-c", r#"command printf '%s\0' \"$PATH\""#])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = output
        .stdout
        .split(|byte| *byte == 0)
        .next()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())?
        .trim();
    (!path.is_empty()).then(|| path.to_string())
}

fn find_in_directory(directory: &Path, name: &str) -> Option<PathBuf> {
    let direct = directory.join(name);
    #[cfg(windows)]
    {
        // npm's `.bin` directories contain both extensionless POSIX shell
        // scripts and Windows `.cmd` launchers. Selecting the extensionless
        // file first fails with ERROR_BAD_EXE_FORMAT (193). On Windows, honor
        // PATHEXT before considering an explicitly suffixed direct path.
        let extensions = std::env::var_os("PATHEXT")
            .and_then(|value| value.to_str().map(str::to_owned))
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
        if direct.is_file()
            && direct.extension().is_some_and(|actual| {
                extensions.split(';').any(|extension| {
                    actual
                        .to_string_lossy()
                        .eq_ignore_ascii_case(extension.trim_start_matches('.'))
                })
            })
        {
            return Some(direct);
        }
        for extension in extensions.split(';') {
            let candidate = directory.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        return None;
    }
    #[cfg(not(windows))]
    {
        if direct.is_file() {
            return Some(direct);
        }
        None
    }
}

fn resolve_toolchain_proxy(candidate: PathBuf, name: &str, project_root: &Path) -> Option<PathBuf> {
    if name != "rust-analyzer" {
        return Some(candidate);
    }
    let canonical = std::fs::canonicalize(&candidate).ok()?;
    let is_rustup_proxy = canonical
        .file_name()
        .is_some_and(|target| target == "rustup" || target == "rustup.exe");
    if !is_rustup_proxy {
        return executable_works(&candidate, &["--help"], project_root).then_some(candidate);
    }
    // Invoke the canonical rustup binary. Executing the `rust-analyzer`
    // symlink changes argv[0], causing rustup to act as the analyzer proxy and
    // interpret `which rust-analyzer` as analyzer arguments.
    let output = crate::background_command(canonical)
        .args(["which", "rust-analyzer"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let resolved = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    (resolved.is_file() && executable_works(&resolved, &["--help"], project_root))
        .then_some(resolved)
}

pub(crate) fn executable_works(path: &Path, args: &[&str], project_root: &Path) -> bool {
    let mut command = crate::background_command(path);
    command.args(args).current_dir(project_root);
    if let Some(path) = runtime_path() {
        command.env("PATH", path);
    }
    command.output().is_ok_and(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_missing_path_is_not_discovered() {
        assert!(find_executable("/definitely/not/a/dragent-language-server").is_none());
    }

    #[test]
    fn project_local_node_server_is_preferred_without_mutating_the_project() {
        let root = std::env::temp_dir().join(format!(
            "dragent-project-lsp-discovery-{}",
            std::process::id()
        ));
        let bin = root.join("node_modules").join(".bin");
        let posix_shim = bin.join("example-language-server");
        #[cfg(windows)]
        let executable = bin.join("example-language-server.cmd");
        #[cfg(not(windows))]
        let executable = posix_shim.clone();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&bin).unwrap();
        // npm installs both of these on Windows. The extensionless file is a
        // POSIX shell script and must never be passed to CreateProcessW.
        std::fs::write(&posix_shim, "#!/bin/sh\n").unwrap();
        #[cfg(windows)]
        std::fs::write(&executable, "test").unwrap();

        let discovered = find_project_executable(&root, "example-language-server").unwrap();
        #[cfg(windows)]
        assert!(
            discovered
                .to_string_lossy()
                .eq_ignore_ascii_case(&executable.to_string_lossy()),
            "expected Windows command shim {executable:?}, got {discovered:?}"
        );
        #[cfg(not(windows))]
        assert_eq!(discovered, executable);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn rustup_proxy_is_resolved_through_real_rustup_in_workspace() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root =
            std::env::temp_dir().join(format!("dragent-rustup-proxy-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("workspace-marker"), "test").unwrap();
        let analyzer = root.join("toolchain-rust-analyzer");
        std::fs::write(&analyzer, "#!/bin/sh\n[ \"$1\" = --help ]\n").unwrap();
        let rustup = root.join("rustup");
        std::fs::write(
            &rustup,
            format!(
                "#!/bin/sh\ncase \"$0\" in *rust-analyzer) exit 9;; esac\n[ -f workspace-marker ] || exit 8\n[ \"$1\" = which ] || exit 7\nprintf '%s\\n' '{}'\n",
                analyzer.display()
            ),
        )
        .unwrap();
        for path in [&analyzer, &rustup] {
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(path, permissions).unwrap();
        }
        let proxy = root.join("rust-analyzer");
        symlink(&rustup, &proxy).unwrap();

        assert_eq!(
            resolve_toolchain_proxy(proxy, "rust-analyzer", &root),
            Some(analyzer)
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
