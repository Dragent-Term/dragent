//! Managed language-server acquisition.
//!
//! Install recipes are catalogued here, never inferred from project content.
//! All commands run on the language-service worker, not the UI thread.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest as _, Sha256};

use crate::{CatalogCandidate, CatalogInstall};

const VSCODE_EXTRACTED_PACKAGE: &str = "vscode-langservers-extracted@latest";
const MANAGED_NODE_VERSION: &str = "v24.11.0";

fn runtime_command(program: &Path) -> Command {
    let mut command = crate::background_command(program);
    if let Some(path) = crate::discovery::runtime_path() {
        command.env("PATH", path);
    }
    command
}

#[derive(Clone, Debug)]
struct ManagedNodeRuntime {
    node: PathBuf,
    npm_cli: PathBuf,
}

fn npm_command() -> Result<Command, String> {
    // Check Dragent's runtime first because general executable discovery also
    // includes its directory. Returning that runtime's npm.cmd here would put
    // managed installs back through a platform shim.
    let runtime = if let Some(runtime) = managed_node_runtime() {
        runtime
    } else if let Some(npm) = crate::discovery::find_executable("npm") {
        return Ok(runtime_command(&npm));
    } else {
        install_managed_node()?
    };
    let mut command = runtime_command(&runtime.node);
    command.arg(&runtime.npm_cli);
    let runtime_root = managed_root().join("runtime/node");
    let cache = runtime_root.join("cache");
    let user_config = runtime_root.join("blank_user_npmrc");
    let global_config = runtime_root.join("blank_global_npmrc");
    std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    if !user_config.is_file() {
        std::fs::write(&user_config, []).map_err(|error| error.to_string())?;
    }
    if !global_config.is_file() {
        std::fs::write(&global_config, []).map_err(|error| error.to_string())?;
    }
    command
        .env("npm_config_cache", cache)
        .env("npm_config_userconfig", user_config)
        .env("npm_config_globalconfig", global_config);
    Ok(command)
}

pub(crate) fn managed_root() -> PathBuf {
    dragent_paths::platform_support().join("language-servers")
}

pub(crate) fn install(
    install: &CatalogInstall,
    candidates: &[CatalogCandidate],
    project_root: &std::path::Path,
) -> Result<PathBuf, String> {
    let executable = candidates
        .first()
        .map(|candidate| candidate.executable.as_str())
        .ok_or_else(|| "language server has no executable candidate".to_string())?;
    match install {
        CatalogInstall::Npm { packages } => install_npm(packages, executable),
        CatalogInstall::BuiltIn(install_id) => {
            install_built_in(install_id, executable, project_root)
        }
    }
}

fn install_built_in(
    install_id: &str,
    executable: &str,
    project_root: &std::path::Path,
) -> Result<PathBuf, String> {
    if !supports_built_in(install_id) {
        return Err(format!(
            "no managed installer is catalogued for {install_id}"
        ));
    }
    match install_id {
        "vtsls" => install_npm(
            &["typescript@latest", "@vtsls/language-server@latest"],
            executable,
        ),
        "vscode-extracted" => install_npm(&[VSCODE_EXTRACTED_PACKAGE], executable),
        "tailwindcss" => install_npm(&["@tailwindcss/language-server@latest"], executable),
        "yaml" => install_npm(&["yaml-language-server@latest"], executable),
        "bash" => install_npm(&["bash-language-server@latest"], executable),
        "basedpyright" => install_npm(&["basedpyright@latest"], executable),
        "rust-analyzer" => install_rust_analyzer(project_root),
        "marksman" => install_marksman(),
        "gopls" => install_go(),
        "terraform-ls" => {
            install_go_package("github.com/hashicorp/terraform-ls@latest", "terraform-ls")
        }
        _ => unreachable!("support check and installer dispatch must stay in sync"),
    }
}

pub(crate) fn supports_built_in(install_id: &str) -> bool {
    matches!(
        install_id,
        "vtsls"
            | "vscode-extracted"
            | "tailwindcss"
            | "yaml"
            | "bash"
            | "basedpyright"
            | "rust-analyzer"
            | "marksman"
            | "gopls"
            | "terraform-ls"
    )
}

fn install_npm<S: AsRef<str>>(packages: &[S], executable: &str) -> Result<PathBuf, String> {
    let root = managed_root().join("npm");
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let output = npm_command()?
        .arg("install")
        .arg("--prefix")
        .arg(&root)
        .arg("--no-audit")
        .arg("--no-fund")
        .arg("--ignore-scripts")
        .args(packages.iter().map(AsRef::as_ref))
        .output()
        .map_err(|error| error.to_string())?;
    require_success("npm", output)?;
    crate::discovery::find_managed_executable(executable)
        .ok_or_else(|| format!("installed package did not provide {executable}"))
}

/// Ensure npm is available without requiring users to install Node themselves.
/// Official Node LTS archives include npm and are isolated under Dragent's
/// language-server cache rather than modifying the user's system.
fn install_managed_node() -> Result<ManagedNodeRuntime, String> {
    if let Some(runtime) = managed_node_runtime() {
        return Ok(runtime);
    }

    const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
    let (platform, architecture, archive_extension) =
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => ("win", "x64", "zip"),
            ("windows", "aarch64") => ("win", "arm64", "zip"),
            ("macos", "x86_64") => ("darwin", "x64", "tar.gz"),
            ("macos", "aarch64") => ("darwin", "arm64", "tar.gz"),
            ("linux", "x86_64") => ("linux", "x64", "tar.gz"),
            ("linux", "aarch64") => ("linux", "arm64", "tar.gz"),
            (os, arch) => return Err(format!("Node does not publish a runtime for {os}/{arch}")),
        };
    let folder = format!("node-{MANAGED_NODE_VERSION}-{platform}-{architecture}");
    let archive_name = format!("{folder}.{archive_extension}");
    let release_root = format!("https://nodejs.org/dist/{MANAGED_NODE_VERSION}");
    let url = format!("{release_root}/{archive_name}");
    let runtime_root = managed_root().join("runtime");
    let destination = runtime_root.join("node");
    let extraction = runtime_root.join(format!(".node-extract-{}", std::process::id()));
    let archive = runtime_root.join(format!(
        ".node-download-{}.{archive_extension}",
        std::process::id()
    ));
    std::fs::create_dir_all(&extraction).map_err(|error| error.to_string())?;

    let result = (|| {
        let checksum_manifest = ureq::get(&format!("{release_root}/SHASUMS256.txt"))
            .set("Accept", "text/plain")
            .set("User-Agent", "Dragent-Language-Services")
            .call()
            .map_err(|error| format!("unable to download Node checksums: {error}"))?
            .into_string()
            .map_err(|error| format!("invalid Node checksum response: {error}"))?;
        let expected_checksum = checksum_for_file(&checksum_manifest, &archive_name)
            .ok_or_else(|| format!("Node checksum manifest has no {archive_name}"))?;
        let response = ureq::get(&url)
            .set("Accept", "application/octet-stream")
            .set("User-Agent", "Dragent-Language-Services")
            .call()
            .map_err(|error| format!("unable to download Node {MANAGED_NODE_VERSION}: {error}"))?;
        let mut source = response.into_reader().take(MAX_ARCHIVE_BYTES + 1);
        let mut target = std::fs::File::create(&archive).map_err(|error| error.to_string())?;
        let bytes = std::io::copy(&mut source, &mut target).map_err(|error| error.to_string())?;
        if bytes > MAX_ARCHIVE_BYTES {
            return Err("Node download exceeded the 256 MiB safety limit".to_string());
        }
        target.flush().map_err(|error| error.to_string())?;
        drop(target);
        let actual_checksum = sha256_file(&archive)?;
        if actual_checksum != expected_checksum {
            return Err(format!(
                "Node archive checksum mismatch for {archive_name}: expected {expected_checksum}, got {actual_checksum}"
            ));
        }

        let tar = crate::discovery::find_executable("tar").ok_or_else(|| {
            "the operating system's tar utility is required to unpack Node".to_string()
        })?;
        let output = runtime_command(&tar)
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&extraction)
            .output()
            .map_err(|error| error.to_string())?;
        require_success("tar", output)?;
        let extracted = extraction.join(&folder);
        if !extracted.is_dir() {
            return Err("Node archive did not contain the expected runtime directory".to_string());
        }
        if destination.exists() {
            std::fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(extracted, &destination).map_err(|error| error.to_string())?;
        let runtime = managed_node_runtime().ok_or_else(|| {
            "downloaded Node runtime did not include node and npm-cli.js".to_string()
        })?;
        let validation = runtime_command(&runtime.node)
            .arg(&runtime.npm_cli)
            .arg("--version")
            .output()
            .map_err(|error| format!("unable to validate managed npm: {error}"))?;
        require_success("managed npm --version", validation)?;
        Ok(runtime)
    })();
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_dir_all(&extraction);
    result
}

fn checksum_for_file(manifest: &str, file_name: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let checksum = fields.next()?;
        (fields.next()? == file_name).then(|| checksum.to_ascii_lowercase())
    })
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn managed_node_runtime() -> Option<ManagedNodeRuntime> {
    let root = managed_root().join("runtime/node");
    #[cfg(windows)]
    let (node, npm_cli) = (
        root.join("node.exe"),
        root.join("node_modules/npm/bin/npm-cli.js"),
    );
    #[cfg(not(windows))]
    let (node, npm_cli) = (
        root.join("bin/node"),
        root.join("lib/node_modules/npm/bin/npm-cli.js"),
    );
    (node.is_file() && npm_cli.is_file()).then_some(ManagedNodeRuntime { node, npm_cli })
}

fn install_rust_analyzer(project_root: &std::path::Path) -> Result<PathBuf, String> {
    let rustup = crate::discovery::find_executable("rustup")
        .ok_or_else(|| "rustup is required to install rust-analyzer".to_string())?;
    let output = runtime_command(&rustup)
        .args(["component", "add", "rust-analyzer"])
        .current_dir(project_root)
        .output()
        .map_err(|error| error.to_string())?;
    require_success("rustup", output)?;
    crate::discovery::find_executable_for_project("rust-analyzer", project_root)
        .ok_or_else(|| "rustup installed no usable rust-analyzer binary".to_string())
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Marksman follows the standalone extension strategy: prefer a user binary,
/// otherwise download the release asset into Dragent's isolated managed bin.
/// Keeping it out of the shared npm tree prevents unrelated Node dependency
/// updates from breaking Markdown support.
fn install_marksman() -> Result<PathBuf, String> {
    const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
    let (asset_name, executable_name) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => ("marksman-macos", "marksman"),
        ("linux", "aarch64") => ("marksman-linux-arm64", "marksman"),
        ("linux", "x86_64") => ("marksman-linux-x64", "marksman"),
        ("windows", _) => ("marksman.exe", "marksman.exe"),
        (os, arch) => {
            return Err(format!(
                "Marksman does not publish a binary for {os}/{arch}"
            ));
        }
    };

    let release: GithubRelease =
        ureq::get("https://api.github.com/repos/artempyanykh/marksman/releases/latest")
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "Dragent-Language-Services")
            .call()
            .map_err(|error| format!("unable to check the latest Marksman release: {error}"))?
            .into_json()
            .map_err(|error| format!("invalid Marksman release response: {error}"))?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            format!(
                "Marksman {} has no asset named {asset_name}",
                release.tag_name
            )
        })?;

    let bin = managed_root().join("bin");
    std::fs::create_dir_all(&bin).map_err(|error| error.to_string())?;
    let destination = bin.join(executable_name);
    let staging = bin.join(format!(
        ".{executable_name}.download-{}",
        std::process::id()
    ));
    let result = (|| {
        let response = ureq::get(&asset.browser_download_url)
            .set("Accept", "application/octet-stream")
            .set("User-Agent", "Dragent-Language-Services")
            .call()
            .map_err(|error| format!("unable to download Marksman: {error}"))?;
        let mut source = response.into_reader().take(MAX_BINARY_BYTES + 1);
        let mut target = std::fs::File::create(&staging).map_err(|error| error.to_string())?;
        let bytes = std::io::copy(&mut source, &mut target).map_err(|error| error.to_string())?;
        if bytes > MAX_BINARY_BYTES {
            return Err("Marksman download exceeded the 128 MiB safety limit".to_string());
        }
        target.flush().map_err(|error| error.to_string())?;
        make_executable(&staging)?;
        let validation = runtime_command(&staging)
            .arg("--version")
            .output()
            .map_err(|error| format!("unable to validate Marksman: {error}"))?;
        require_success("marksman --version", validation)?;
        if destination.exists() {
            std::fs::remove_file(&destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&staging, &destination).map_err(|error| error.to_string())?;
        std::fs::write(bin.join("marksman.version"), &release.tag_name)
            .map_err(|error| error.to_string())?;
        Ok(destination.clone())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result
}

fn make_executable(_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(_path)
            .map_err(|error| error.to_string())?
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(_path, permissions).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn install_go() -> Result<PathBuf, String> {
    install_go_package("golang.org/x/tools/gopls@latest", "gopls")
}

fn install_go_package(package: &str, executable: &str) -> Result<PathBuf, String> {
    let go = crate::discovery::find_executable("go").map_or_else(install_managed_go, Ok)?;
    let bin = managed_root().join("bin");
    std::fs::create_dir_all(&bin).map_err(|error| error.to_string())?;
    let output = runtime_command(&go)
        .env("GOBIN", &bin)
        .args(["install", package])
        .output()
        .map_err(|error| error.to_string())?;
    require_success("go install", output)?;
    crate::discovery::find_managed_executable(executable)
        .ok_or_else(|| format!("go install produced no {executable} binary"))
}

#[derive(serde::Deserialize)]
struct GoRelease {
    stable: bool,
    files: Vec<GoReleaseFile>,
}

#[derive(serde::Deserialize)]
struct GoReleaseFile {
    filename: String,
    os: String,
    arch: String,
    version: String,
    sha256: String,
    kind: String,
}

/// Install an isolated official Go toolchain when the host has no Go runtime.
/// This lets Go-distributed servers (gopls and terraform-ls) install without
/// modifying PATH or requiring a system package manager.
fn install_managed_go() -> Result<PathBuf, String> {
    let runtime_root = managed_root().join("runtime");
    let destination = runtime_root.join("go");
    #[cfg(windows)]
    let executable = destination.join("bin/go.exe");
    #[cfg(not(windows))]
    let executable = destination.join("bin/go");
    if executable.is_file() {
        return Ok(executable);
    }

    const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => return Err(format!("Go does not publish a runtime for {os}/{arch}")),
    };
    if !matches!(os, "windows" | "macos" | "linux") {
        return Err(format!("Go does not publish a runtime for {os}/{arch}"));
    }
    let go_os = if os == "macos" { "darwin" } else { os };
    let releases: Vec<GoRelease> = ureq::get("https://go.dev/dl/?mode=json")
        .set("Accept", "application/json")
        .set("User-Agent", "Dragent-Language-Services")
        .call()
        .map_err(|error| format!("unable to query official Go releases: {error}"))?
        .into_json()
        .map_err(|error| format!("invalid Go release response: {error}"))?;
    let file = releases
        .iter()
        .filter(|release| release.stable)
        .flat_map(|release| release.files.iter())
        .find(|file| file.os == go_os && file.arch == arch && file.kind == "archive")
        .ok_or_else(|| format!("official Go releases contain no archive for {go_os}/{arch}"))?;

    std::fs::create_dir_all(&runtime_root).map_err(|error| error.to_string())?;
    let archive = runtime_root.join(format!(".go-download-{}", std::process::id()));
    let extraction = runtime_root.join(format!(".go-extract-{}", std::process::id()));
    std::fs::create_dir_all(&extraction).map_err(|error| error.to_string())?;
    let result = (|| {
        let response = ureq::get(&format!("https://go.dev/dl/{}", file.filename))
            .set("Accept", "application/octet-stream")
            .set("User-Agent", "Dragent-Language-Services")
            .call()
            .map_err(|error| format!("unable to download Go {}: {error}", file.version))?;
        let mut source = response.into_reader().take(MAX_ARCHIVE_BYTES + 1);
        let mut target = std::fs::File::create(&archive).map_err(|error| error.to_string())?;
        let bytes = std::io::copy(&mut source, &mut target).map_err(|error| error.to_string())?;
        if bytes > MAX_ARCHIVE_BYTES {
            return Err("Go download exceeded the 256 MiB safety limit".to_string());
        }
        target.flush().map_err(|error| error.to_string())?;
        drop(target);
        let actual = sha256_file(&archive)?;
        if actual != file.sha256.to_ascii_lowercase() {
            return Err(format!(
                "Go archive checksum mismatch: expected {}, got {actual}",
                file.sha256
            ));
        }
        let tar = crate::discovery::find_executable("tar").ok_or_else(|| {
            "the operating system's tar utility is required to unpack Go".to_string()
        })?;
        require_success(
            "tar",
            runtime_command(&tar)
                .args(["-xf"])
                .arg(&archive)
                .arg("-C")
                .arg(&extraction)
                .output()
                .map_err(|error| error.to_string())?,
        )?;
        let extracted = extraction.join("go");
        if !extracted.is_dir() {
            return Err("Go archive did not contain the expected runtime directory".to_string());
        }
        if destination.exists() {
            std::fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(extracted, &destination).map_err(|error| error.to_string())?;
        if !executable.is_file() {
            return Err("downloaded Go runtime did not include the go executable".to_string());
        }
        std::fs::write(runtime_root.join("go.version"), &file.version)
            .map_err(|error| error.to_string())?;
        Ok(executable.clone())
    })();
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_dir_all(&extraction);
    result
}

fn require_success(program: &str, output: Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr
        .chars()
        .rev()
        .take(800)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    Err(if detail.trim().is_empty() {
        format!("{program} exited with {}", output.status)
    } else {
        format!("{program} exited with {}: {}", output.status, detail.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_exact_node_archive_checksum() {
        let manifest = "aaaa  node-v1-linux-x64.tar.gz\nbbbb  node-v1-win-arm64.zip\n";
        assert_eq!(
            checksum_for_file(manifest, "node-v1-win-arm64.zip").as_deref(),
            Some("bbbb")
        );
        assert_eq!(checksum_for_file(manifest, "node-v1-win-x64.zip"), None);
    }

    #[test]
    fn vscode_extracted_recipe_uses_the_working_unscoped_package() {
        assert_eq!(
            VSCODE_EXTRACTED_PACKAGE,
            "vscode-langservers-extracted@latest"
        );
        assert!(!VSCODE_EXTRACTED_PACKAGE.starts_with('@'));
    }
}
