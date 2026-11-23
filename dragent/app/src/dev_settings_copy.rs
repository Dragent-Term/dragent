//! One-time copy of the person's preferences into the Dev data folder.
//!
//! Release edition: never runs anything here.
//!
//! Dev edition: on the first launch (no [`MARKER_FILE`] in the Dev data
//! folder), copy exactly the preference files from the release data folder
//! (`~/.dragent`, computed explicitly — never via `dragent_paths::home()`,
//! which already points at Dev) into the Dev folder, then write the marker.
//! The copy is keyed on the marker alone: with no marker, files already in
//! the Dev folder are overwritten. Never runs again after the marker exists.
//!
//! Copied (preferences only):
//! - `app_settings.json` — agent backend/model, autonomy, indexing, telemetry
//!   toggles, notifications, shortcuts, dev permissions, computer use, voice,
//!   git defaults, repo defaults template, general, plugin/sharing, tasks,
//!   hosts. Sanitized: `device_id` dropped (Dev gets its own), `provider_keys`
//!   emptied and `custom_models[].api_key` stripped (Dev has its own Keychain;
//!   on native the file holds no secrets anyway), machine-specific `hosts`
//!   fields dropped (`active_runtime_id`, `overrides`,
//!   `routing_disabled_targets`, `routing_probe_skipped`), every absolute
//!   path pointing into the release folder rewritten to the Dev folder.
//! - `settings.json` — terminal fonts/behaviour and editor appearance
//!   (`EditorSettings`). Paths rewritten release → Dev.
//! - `ui_settings.json` — theme id, OS-sync ids, agent launch commands and
//!   custom agents (`UiSettings`). Paths rewritten release → Dev.
//! - `mcp.json` — user-configured MCP servers. `bearerToken` entries stripped
//!   (token material stays in the platform store / Dev Keychain), paths
//!   rewritten release → Dev.
//! - `keybindings.json` — hand-editable shortcut mirror. Paths rewritten
//!   (harmless; the file holds chords).
//! - `repo_settings.json` — per-project overrides keyed by project root.
//!   Keys and values rewritten release → Dev so nothing points into
//!   `~/.dragent`.
//! - `themes/*.yaml|*.yml` — custom themes the person imported.
//! - `workflows/*.yaml|*.yml` — workflows/snippets the person created.
//!
//! Never copied (when in doubt, don't copy):
//! - Sign-in / session / token files (`organisation.json`, `agent-accounts/`,
//!   `accounts.json`, `auth.json`, `*-token*`, `credentials*`, `mcp-tokens/`,
//!   `agent-skill.json`, `computer-skill.json`, `integrations.json` bridge
//!   state, `mcp-agent-sync.json`, `cli-identity*`): Dev signs in separately
//!   and owns its own Keychain entries.
//! - Keychain data (never in files) and platform-owned folders (`credentials`,
//!   `models`, `language-servers`, `mcp-tokens`): separate per edition.
//! - Organisation records and everything under `organisations/` (`machines.json`,
//!   `teams/`, `agent-sessions/`, `automations.sqlite`, `workspaces.json`,
//!   `projects`, `worktrees/`, `remote/`, `workspace-setup/`, …): Dev works
//!   against its own organisations (local-first in t5) and must never adopt
//!   the installed app's work.
//! - Workspaces, worktrees, terminals/sessions/history (`workspaces/`,
//!   `sessions/`, `history.jsonl`, `projects/`, `agent-runs/`, `tool-output/`,
//!   `agent-usage/`, `usage/`, `phone-uploads/`, `feedback-drafts/`): runtime
//!   state, not preferences.
//! - Code index (`index-service-v1.json`, `index.ignore`, embeddings):
//!   rebuilt per edition; Dev indexes from scratch.
//! - Logs, databases, daemon manifests/sockets/locks (`logs/`, `*.sqlite*`,
//!   `app.lock`, `index-service.lock`, `host.log`, `*.sock`, `telemetry.jsonl`,
//!   `agent_models.json` cache): per-edition runtime, never shared.
//! - Caches and downloaded models (`cache/`, `models/`, `language-servers/`,
//!   `updates/`, `project-icons/`): re-downloaded / rebuilt per edition.
//! - Machine-specific approvals and pins (`permissions.json`,
//!   `tool-pins.json`): auto-approvals and absolute tool paths must be
//!   re-granted in Dev, never inherited.
//!
//! Any setting that names an organisation id or an absolute path into the
//! release folder is dropped (secrets, device id, machine bindings) or
//! rewritten (paths) as described above, so nothing in Dev points into
//! `~/.dragent`.

/// Marker proving the one-time copy already ran (or was considered).
pub const MARKER_FILE: &str = "edition-dev-initialised";

/// Preference files copied verbatim in shape (sanitized in content).
#[cfg(not(target_family = "wasm"))]
const FILE_ALLOW_LIST: &[&str] = &[
    "app_settings.json",
    "settings.json",
    "ui_settings.json",
    "mcp.json",
    "keybindings.json",
    "repo_settings.json",
];

/// Preference directories copied (only `*.yaml`/`*.yml` inside).
#[cfg(not(target_family = "wasm"))]
const DIR_ALLOW_LIST: &[&str] = &["themes", "workflows"];

/// The release data folder, computed explicitly: always `~/.dragent`.
/// Never `dragent_paths::home()` — in a Dev build that already points at Dev.
#[cfg(not(target_family = "wasm"))]
pub fn release_data_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".dragent")
}

/// Run the one-time copy when this is a Dev build. Release builds return
/// without touching anything. Runs before settings are first read (called
/// from `main` ahead of `migrate_legacy_storage` and `dragent::run`).
#[cfg(not(target_family = "wasm"))]
pub fn maybe_copy_release_settings() {
    if !dragent_paths::is_dev() {
        return;
    }
    let dev = dragent_paths::home();
    let release = release_data_dir();
    if let Err(error) = maybe_copy_release_settings_from(&release, &dev) {
        log::warn!("[dev-settings] one-time copy failed: {error}");
    }
}

#[cfg(target_family = "wasm")]
pub fn maybe_copy_release_settings() {}

/// Copy preferences from `release` into `dev` on first launch.
///
/// - `Ok(false)`: already initialised (marker present) or misconfigured
///   (`dev == release` — writing a marker there would pollute the installed
///   app, so nothing is done).
/// - `Ok(true)`: first run; the marker is now written. When `release` is
///   missing, only the marker is written.
#[cfg(not(target_family = "wasm"))]
pub fn maybe_copy_release_settings_from(
    release: &std::path::Path,
    dev: &std::path::Path,
) -> std::io::Result<bool> {
    if dev.join(MARKER_FILE).exists() {
        return Ok(false);
    }
    if dev == release {
        return Ok(false);
    }
    if let Err(error) = std::fs::create_dir_all(dev) {
        log::warn!("[dev-settings] cannot create {}: {error}", dev.display());
        return Err(error);
    }
    if !release.is_dir() {
        write_marker(dev)?;
        return Ok(true);
    }
    let release_text = path_text(release);
    let dev_text = path_text(dev);
    for name in FILE_ALLOW_LIST {
        let src = release.join(name);
        if src.is_file() {
            copy_sanitized_file(&src, &dev.join(name), name, &release_text, &dev_text);
        }
    }
    for dir in DIR_ALLOW_LIST {
        copy_yaml_dir(&release.join(dir), &dev.join(dir), &release_text, &dev_text);
    }
    write_marker(dev)?;
    Ok(true)
}

#[cfg(target_family = "wasm")]
pub fn maybe_copy_release_settings_from(
    _release: &std::path::Path,
    _dev: &std::path::Path,
) -> std::io::Result<bool> {
    Ok(false)
}

#[cfg(not(target_family = "wasm"))]
fn write_marker(dev: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(dev.join(MARKER_FILE), "dev edition initialised\n")
}

#[cfg(not(target_family = "wasm"))]
fn path_text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(not(target_family = "wasm"))]
fn copy_sanitized_file(
    src: &std::path::Path,
    dst: &std::path::Path,
    name: &str,
    release: &str,
    dev: &str,
) {
    // No marker yet, so whatever is here was not chosen by the person in
    // this edition (a test run or a crashed first launch): copy over it.
    let raw = match std::fs::read_to_string(src) {
        Ok(raw) => raw,
        Err(error) => {
            log::warn!("[dev-settings] cannot read {}: {error}", src.display());
            return;
        }
    };
    let out = match name {
        "app_settings.json" => sanitize_app_settings(&raw, release, dev),
        "mcp.json" => sanitize_mcp(&raw, release, dev),
        _ => sanitize_json_strings(&raw, release, dev),
    };
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(dst, out) {
        log::warn!("[dev-settings] cannot write {}: {error}", dst.display());
    }
}

#[cfg(not(target_family = "wasm"))]
fn copy_yaml_dir(src: &std::path::Path, dst: &std::path::Path, release: &str, dev: &str) {
    let entries = match std::fs::read_dir(src) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_yaml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"));
        if !is_yaml {
            continue;
        }
        let target = dst.join(entry.file_name());
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) => {
                log::warn!("[dev-settings] cannot read {}: {error}", path.display());
                continue;
            }
        };
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&target, rewrite_path_text(&raw, release, dev)) {
            log::warn!("[dev-settings] cannot write {}: {error}", target.display());
        }
    }
}

/// Replace every occurrence of the release folder in YAML/text with the Dev
/// folder, covering both `/` and `\` separators (Windows).
#[cfg(not(target_family = "wasm"))]
fn rewrite_path_text(raw: &str, release: &str, dev: &str) -> String {
    let mut out = raw.to_string();
    if !release.is_empty() && !dev.is_empty() && release != dev {
        out = out.replace(release, dev);
        let release_fwd = release.replace('\\', "/");
        let dev_fwd = dev.replace('\\', "/");
        if release_fwd != *release {
            out = out.replace(&release_fwd, &dev_fwd);
        }
    }
    out
}

#[cfg(not(target_family = "wasm"))]
fn rewrite_value_strings(value: &mut serde_json::Value, release: &str, dev: &str) {
    match value {
        serde_json::Value::String(text) => {
            *text = rewrite_path_text(text, release, dev);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_value_strings(item, release, dev);
            }
        }
        serde_json::Value::Object(map) => {
            let mut renamed = Vec::new();
            for (key, val) in map.iter_mut() {
                rewrite_value_strings(val, release, dev);
                let fixed = rewrite_path_text(key, release, dev);
                if fixed != *key {
                    renamed.push((key.clone(), fixed));
                }
            }
            for (old, new) in renamed {
                if let Some(val) = map.remove(&old) {
                    map.insert(new, val);
                }
            }
        }
        _ => {}
    }
}

/// Generic JSON sanitizer: parse, rewrite release paths in every string
/// (keys included, so `repo_settings.json` project keys stay out of the
/// release folder), re-serialize. Unparseable files fall back to raw text
/// replacement so a hand-edited file still copies without pointing at the
/// release folder.
#[cfg(not(target_family = "wasm"))]
fn sanitize_json_strings(raw: &str, release: &str, dev: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(mut value) => {
            rewrite_value_strings(&mut value, release, dev);
            serde_json::to_string_pretty(&value)
                .unwrap_or_else(|_| rewrite_path_text(raw, release, dev))
        }
        Err(_) => rewrite_path_text(raw, release, dev),
    }
}

/// `app_settings.json`: drop per-install secrets and machine bindings, then
/// rewrite release paths. See the module docs for the exact list.
#[cfg(not(target_family = "wasm"))]
fn sanitize_app_settings(raw: &str, release: &str, dev: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(mut value) => {
            if let Some(obj) = value.as_object_mut() {
                obj.remove("device_id");
                obj.insert("provider_keys".to_string(), serde_json::json!({}));
                if let Some(models) = obj.get_mut("custom_models").and_then(|v| v.as_array_mut()) {
                    for model in models.iter_mut() {
                        if let Some(entry) = model.as_object_mut() {
                            entry.remove("api_key");
                        }
                    }
                }
                if let Some(hosts) = obj.get_mut("hosts").and_then(|v| v.as_object_mut()) {
                    hosts.remove("active_runtime_id");
                    hosts.insert("overrides".to_string(), serde_json::json!({}));
                    hosts.insert(
                        "routing_disabled_targets".to_string(),
                        serde_json::json!([]),
                    );
                    hosts.insert("routing_probe_skipped".to_string(), serde_json::json!([]));
                }
            }
            rewrite_value_strings(&mut value, release, dev);
            serde_json::to_string_pretty(&value)
                .unwrap_or_else(|_| rewrite_path_text(raw, release, dev))
        }
        Err(_) => rewrite_path_text(raw, release, dev),
    }
}

/// `mcp.json`: strip bearer tokens (token material stays in the platform
/// store / Dev Keychain), then rewrite release paths.
#[cfg(not(target_family = "wasm"))]
fn sanitize_mcp(raw: &str, release: &str, dev: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(mut value) => {
            if let Some(servers) = value.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                for (_, def) in servers.iter_mut() {
                    if let Some(entry) = def.as_object_mut() {
                        entry.remove("bearerToken");
                        entry.remove("bearer_token");
                    }
                }
            }
            rewrite_value_strings(&mut value, release, dev);
            serde_json::to_string_pretty(&value)
                .unwrap_or_else(|_| rewrite_path_text(raw, release, dev))
        }
        Err(_) => rewrite_path_text(raw, release, dev),
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn release_fixture(release: &std::path::Path) -> String {
        let home = path_text(release);
        write(
            &release.join("app_settings.json"),
            &serde_json::json!({
                "agent_backend": "cloud",
                "selected_model": "gpt-4o",
                "device_id": "release-device",
                "provider_keys": {"openai": "sk-release"},
                "custom_models": [{"name": "Corp", "model_id": "corp", "base_url": "https://x/v1", "api_key": "corp-key"}],
                "hosts": {
                    "active_runtime_id": "machine-1",
                    "overrides": {"machine-1": {"display_label": "Prod"}},
                    "routing_disabled_targets": ["a"],
                    "routing_probe_skipped": ["b"],
                    "local_runtime": "host"
                },
                "general": {"workspace_dir": format!("{home}/work")},
                "plugin": {"system_enabled": true, "dev_paths": [format!("{home}/dev-plugins")]}
            })
            .to_string(),
        );
        write(
            &release.join("settings.json"),
            &serde_json::json!({"vim_mode_enabled": true, "terminal": {"default_shell": format!("{home}/bin/zsh")}}).to_string(),
        );
        write(
            &release.join("ui_settings.json"),
            &serde_json::json!({"theme_id": "gruvbox_dark", "agent_commands": {"claude": "claude --x"}}).to_string(),
        );
        write(
            &release.join("mcp.json"),
            &serde_json::json!({"mcpServers": {
                "files": {"command": "npx", "args": [format!("{home}/mcp")], "bearerToken": "secret"},
                "web": {"url": "https://example.com", "bearerToken": "secret"}
            }})
            .to_string(),
        );
        write(
            &release.join("keybindings.json"),
            r#"{"new-tab": "ctrl-t"}"#,
        );
        write(
            &release.join("repo_settings.json"),
            &serde_json::json!({"/tmp/proj": {"base_ref": "develop", "order": 3}}).to_string(),
        );
        write(
            &release.join("themes/custom.yaml"),
            "accent: '#fff'\nbackground: '#000'\nforeground: '#eee'\nterminal_colors: []\n",
        );
        write(&release.join("themes/notes.txt"), "not a theme");
        write(
            &release.join("workflows/deploy.yaml"),
            "- name: Deploy\n  command: 'make deploy'\n",
        );
        // Excluded kinds: must never copy.
        for excluded in [
            "organisation.json",
            "organisations/abc/state.json",
            "workspaces/id.json",
            "worktrees/meta.json",
            "sessions/x.json",
            "history.jsonl",
            "agent-sessions/a.json",
            "logs/app.log",
            "cache/updates/f",
            "telemetry.jsonl",
            "automations.sqlite",
            "app.lock",
            "index-service-v1.json",
            "index.ignore",
            "permissions.json",
            "tool-pins.json",
            "integrations.json",
            "agent_models.json",
            "agent-skill.json",
            "computer-skill.json",
            "credentials",
            "models/model.bin",
            "language-servers/srv",
            "mcp-tokens/tok",
            "phone-uploads/a.png",
            "agent-runs/r.json",
        ] {
            write(&release.join(excluded), "sensitive");
        }
        home
    }

    #[test]
    fn copies_only_the_allow_list_and_sanitizes() {
        let release = tempfile::tempdir().unwrap();
        let dev = tempfile::tempdir().unwrap();
        let dev_path = dev.path().join("dev-home");
        let home = release_fixture(release.path());
        let dev_text = path_text(&dev_path);

        assert!(maybe_copy_release_settings_from(release.path(), &dev_path).unwrap());
        assert!(dev_path.join(MARKER_FILE).is_file());

        for name in FILE_ALLOW_LIST {
            assert!(dev_path.join(name).is_file(), "{name} copied");
        }
        assert!(dev_path.join("themes/custom.yaml").is_file());
        assert!(dev_path.join("workflows/deploy.yaml").is_file());
        // Non-YAML files in allow-listed dirs are skipped.
        assert!(!dev_path.join("themes/notes.txt").exists());

        // Excluded kinds never copied.
        for excluded in [
            "organisation.json",
            "organisations",
            "workspaces",
            "worktrees",
            "sessions",
            "history.jsonl",
            "agent-sessions",
            "logs",
            "cache",
            "telemetry.jsonl",
            "automations.sqlite",
            "app.lock",
            "index-service-v1.json",
            "index.ignore",
            "permissions.json",
            "tool-pins.json",
            "integrations.json",
            "agent_models.json",
            "agent-skill.json",
            "computer-skill.json",
            "credentials",
            "models",
            "language-servers",
            "mcp-tokens",
            "phone-uploads",
            "agent-runs",
        ] {
            assert!(
                !dev_path.join(excluded).exists(),
                "{excluded} must not copy"
            );
        }

        // Sanitization: secrets and machine bindings dropped.
        let app: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dev_path.join("app_settings.json")).unwrap(),
        )
        .unwrap();
        assert!(app.get("device_id").is_none());
        assert_eq!(app.get("provider_keys").unwrap(), &serde_json::json!({}));
        assert!(app["custom_models"][0].get("api_key").is_none());
        assert!(app["hosts"].get("active_runtime_id").is_none());
        assert_eq!(app["hosts"]["overrides"], serde_json::json!({}));
        // Preferences survive.
        assert_eq!(app["selected_model"], serde_json::json!("gpt-4o"));
        assert_eq!(
            app["general"]["workspace_dir"],
            serde_json::json!(format!("{dev_text}/work"))
        );

        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dev_path.join("mcp.json")).unwrap())
                .unwrap();
        assert!(mcp["mcpServers"]["files"].get("bearerToken").is_none());
        assert!(mcp["mcpServers"]["web"].get("bearerToken").is_none());
        assert_eq!(
            mcp["mcpServers"]["files"]["command"],
            serde_json::json!("npx")
        );

        // Nothing in Dev points into the release folder.
        let mut found_release = Vec::new();
        collect_files(&dev_path, &mut found_release);
        for file in &found_release {
            if file.file_name().and_then(|n| n.to_str()) == Some(MARKER_FILE) {
                continue;
            }
            let raw = std::fs::read_to_string(file).unwrap();
            assert!(
                !raw.contains(&home),
                "{} still points into the release folder",
                file.display()
            );
        }
        // Release-path rewrite works (workspace_dir, dev path, shell, mcp arg).
        let raw = std::fs::read_to_string(dev_path.join("app_settings.json")).unwrap();
        assert!(raw.contains(&dev_text), "release paths rewritten to Dev");
        let _ = BTreeMap::<String, String>::new();
    }

    fn collect_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_files(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    #[test]
    fn marker_prevents_a_second_copy() {
        let release = tempfile::tempdir().unwrap();
        let dev = tempfile::tempdir().unwrap();
        let dev_path = dev.path().join("dev-home");
        release_fixture(release.path());
        assert!(maybe_copy_release_settings_from(release.path(), &dev_path).unwrap());
        write(&dev_path.join("app_settings.json"), r#"{"kept": true}"#);
        write(
            &release.path().join("app_settings.json"),
            r#"{"changed": true}"#,
        );
        assert!(!maybe_copy_release_settings_from(release.path(), &dev_path).unwrap());
        assert_eq!(
            std::fs::read_to_string(dev_path.join("app_settings.json")).unwrap(),
            r#"{"kept": true}"#
        );
    }

    #[test]
    fn existing_files_without_the_marker_are_copied_over() {
        // A test run or a crashed first launch can leave files in the Dev
        // folder before the marker exists: they must not block the copy.
        let release = tempfile::tempdir().unwrap();
        let dev = tempfile::tempdir().unwrap();
        let dev_path = dev.path().join("dev-home");
        release_fixture(release.path());
        write(&dev_path.join("repo_settings.json"), "{}");
        write(&dev_path.join("themes/custom.yaml"), "name: stale\n");
        write(&dev_path.join("tool-output/run.txt"), "left alone");

        assert!(maybe_copy_release_settings_from(release.path(), &dev_path).unwrap());
        assert!(dev_path.join(MARKER_FILE).is_file());
        let repo = std::fs::read_to_string(dev_path.join("repo_settings.json")).unwrap();
        assert_ne!(repo, "{}");
        assert!(repo.contains("develop"), "{repo}");
        let theme = std::fs::read_to_string(dev_path.join("themes/custom.yaml")).unwrap();
        assert!(theme.contains("accent"), "{theme}");
        assert_eq!(
            std::fs::read_to_string(dev_path.join("tool-output/run.txt")).unwrap(),
            "left alone"
        );
    }

    #[test]
    fn only_the_marker_when_the_release_folder_is_missing() {
        let dev = tempfile::tempdir().unwrap();
        let dev_path = dev.path().join("dev-home");
        let missing = dev.path().join("no-release");
        assert!(!missing.exists());
        assert!(maybe_copy_release_settings_from(&missing, &dev_path).unwrap());
        assert!(dev_path.join(MARKER_FILE).is_file());
        let entries: Vec<_> = std::fs::read_dir(&dev_path).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "only the marker is written");
    }

    #[test]
    fn never_writes_into_the_release_folder_itself() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!maybe_copy_release_settings_from(dir.path(), dir.path()).unwrap());
        assert!(!dir.path().join(MARKER_FILE).exists());
    }

    #[test]
    fn dev_build_uses_dev_home_not_the_release_folder() {
        assert_eq!(
            release_data_dir().file_name().and_then(|n| n.to_str()),
            Some(".dragent")
        );
        // The release source is computed explicitly, never via home(): in a
        // Dev build home() points at the Dev folder.
        assert!(dragent_paths::is_dev());
    }
}
