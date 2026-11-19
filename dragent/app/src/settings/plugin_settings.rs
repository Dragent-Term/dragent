//! Plugin system + sharing toggles.
//!
//! [`PluginSettings`] is the generic plugin registry overlay: a master switch,
//! per-plugin disables, first-run consents, and dev-manifest search paths.
//! The MCP servers in `~/.dragent/mcp.json` are the concrete plugin inventory
//! today — each server name is a plugin id — so these toggles are functional
//! without a separate registry. [`SharingSettings`] gates artifact (session)
//! sharing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Consent version stamped the first time a plugin tool is approved.
/// Bump when the consent semantics change so old approvals stop applying.
pub const PLUGIN_CONSENT_VERSION: &str = "v1";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginSettings {
    /// Master switch. Off = every non-built-in plugin tool is unavailable
    /// (tool defs are hidden and calls are blocked). On by default so the
    /// MCP servers people already use keep working.
    #[serde(default = "default_true")]
    pub system_enabled: bool,
    /// Plugin ids the user turned off while the system stays on.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
    /// plugin id → consent version string, persisted on approve. Adding a
    /// server in Settings → MCP approves it; servers configured before
    /// consents existed are approved when settings load.
    #[serde(default)]
    pub consents: HashMap<String, String>,
    /// Directories scanned for dev plugin manifests (`plugin.json`,
    /// `manifest.json`, or a `SKILL.md` frontmatter name).
    #[serde(default)]
    pub dev_paths: Vec<String>,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            system_enabled: true,
            disabled_plugins: Vec::new(),
            consents: HashMap::new(),
            dev_paths: Vec::new(),
        }
    }
}

impl PluginSettings {
    /// True when the plugin system allows this id to provide tools.
    pub fn is_plugin_enabled(&self, id: &str) -> bool {
        if !self.system_enabled {
            return false;
        }
        !self.disabled_plugins.iter().any(|d| d == id)
    }

    /// Same as [`Self::is_plugin_enabled`] but tolerant of the MCP
    /// hyphen→underscore sanitising used in prefixed tool names: a disabled
    /// entry matches either spelling.
    pub fn is_plugin_enabled_sanitized(&self, sanitized_id: &str) -> bool {
        if !self.system_enabled {
            return false;
        }
        !self
            .disabled_plugins
            .iter()
            .any(|d| d == sanitized_id || d.replace('-', "_") == sanitized_id)
    }

    /// Mark a plugin enabled (`true`) or disabled (`false`).
    pub fn set_plugin_enabled(&mut self, id: &str, enabled: bool) {
        if enabled {
            self.disabled_plugins.retain(|d| d != id);
        } else if !self.disabled_plugins.iter().any(|d| d == id) {
            self.disabled_plugins.push(id.to_string());
        }
    }

    /// True once the user approved this plugin at any version.
    pub fn has_consent(&self, id: &str) -> bool {
        self.consents.contains_key(id)
    }

    /// Record approval of a plugin at a consent version.
    pub fn set_consent(&mut self, id: &str, version: &str) {
        self.consents.insert(id.to_string(), version.to_string());
    }

    /// Add a dev search path (deduped, normalised for `~`).
    pub fn add_dev_path(&mut self, raw: &str) {
        let path = expand_tilde(raw).trim().to_string();
        if path.is_empty() {
            return;
        }
        if !self.dev_paths.iter().any(|d| d == &path) {
            self.dev_paths.push(path);
        }
    }

    pub fn remove_dev_path(&mut self, raw: &str) {
        let path = raw.trim();
        self.dev_paths.retain(|d| d != path);
    }

    /// Scan every dev path for plugin manifests. Re-run on each read (the
    /// Skills page re-scans on every render, `mcp::init` scans at startup),
    /// so externally added/edited manifests appear without an OS watcher.
    pub fn dev_manifests(&self) -> Vec<DevPluginManifest> {
        load_dev_plugin_manifests(&self.dev_paths)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharingSettings {
    /// Gates artifact sharing: the session share sheet, share menu, and the
    /// backend `start_host` call. Off = sharing UI is inert and sharing
    /// actions are blocked with an explanatory message. On by default:
    /// session sharing was always available.
    #[serde(default = "default_true")]
    pub artifact_sharing_enabled: bool,
}

impl Default for SharingSettings {
    fn default() -> Self {
        Self {
            artifact_sharing_enabled: true,
        }
    }
}

fn expand_tilde(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = dirs::home_dir().unwrap_or_default();
        return home.join(rest).to_string_lossy().into_owned();
    }
    if trimmed == "~" {
        return dirs::home_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    trimmed.to_string()
}

// ── Dev plugin manifests ──────────────────────────────────────────────────────

/// A plugin manifest discovered under a dev path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevPluginManifest {
    pub name: String,
    pub version: Option<String>,
    /// File the manifest was read from (for display + reload).
    pub path: PathBuf,
}

/// Load manifests from dev search paths. Each entry is a directory scanned
/// one level deep, or a direct file path. Recognised files, in order:
/// `plugin.json` (`{"name","version"}`), `manifest.json` (same shape), or a
/// bare directory whose name is used with no version.
pub fn load_dev_plugin_manifests(dev_paths: &[String]) -> Vec<DevPluginManifest> {
    let mut out = Vec::new();
    for raw in dev_paths {
        let path = PathBuf::from(expand_tilde(raw));
        if path.is_file() {
            if let Some(manifest) = read_manifest_file(&path) {
                out.push(manifest);
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        // A plugin.json directly inside the dev path itself.
        for candidate in ["plugin.json", "manifest.json"] {
            let file = path.join(candidate);
            if file.is_file() {
                if let Some(manifest) = read_manifest_file(&file) {
                    out.push(manifest);
                }
            }
        }
        // One level of subdirectories, each a candidate plugin.
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let mut found = false;
            for candidate in ["plugin.json", "manifest.json"] {
                let file = dir.join(candidate);
                if file.is_file() {
                    if let Some(manifest) = read_manifest_file(&file) {
                        out.push(manifest);
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                // Directory plugin without a JSON manifest: use the dir name.
                if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                    // Skip hidden dirs and node_modules-style noise.
                    if !name.starts_with('.') {
                        out.push(DevPluginManifest {
                            name: name.to_string(),
                            version: None,
                            path: dir,
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name && a.path == b.path);
    out
}

fn read_manifest_file(path: &Path) -> Option<DevPluginManifest> {
    let content = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(str::to_string)
        })?;
    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some(DevPluginManifest {
        name,
        version,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_skill_export_setting_preserves_session_sharing_choice() {
        let settings: SharingSettings = serde_json::from_value(serde_json::json!({
            "artifact_sharing_enabled": false,
            "skill_sharing_enabled": true
        })).unwrap();
        assert!(!settings.artifact_sharing_enabled);
        assert!(serde_json::to_value(settings).unwrap().get("skill_sharing_enabled").is_none());
    }

    #[test]
    fn defaults_keep_existing_features_on() {
        let mut plugin = PluginSettings::default();
        assert!(plugin.system_enabled);
        assert!(plugin.disabled_plugins.is_empty());
        assert!(plugin.consents.is_empty());
        assert!(plugin.dev_paths.is_empty());
        assert!(plugin.is_plugin_enabled("github"));
        // Master switch off disables everything, even with no explicit disables.
        plugin.system_enabled = false;
        assert!(!plugin.is_plugin_enabled("github"));

        let sharing = SharingSettings::default();
        assert!(sharing.artifact_sharing_enabled);

        // Settings saved before these sections existed load the same way.
        let plugin: PluginSettings = serde_json::from_str("{}").unwrap();
        assert!(plugin.system_enabled);
        let sharing: SharingSettings = serde_json::from_str("{}").unwrap();
        assert!(sharing.artifact_sharing_enabled);
    }

    #[test]
    fn enable_disable_round_trips() {
        let mut plugin = PluginSettings::default();
        plugin.system_enabled = true;
        assert!(plugin.is_plugin_enabled("github"));
        plugin.set_plugin_enabled("github", false);
        assert!(!plugin.is_plugin_enabled("github"));
        assert!(plugin.is_plugin_enabled("other"));
        plugin.set_plugin_enabled("github", true);
        assert!(plugin.is_plugin_enabled("github"));
        // No duplicates on repeated disables.
        plugin.set_plugin_enabled("github", false);
        plugin.set_plugin_enabled("github", false);
        assert_eq!(
            plugin.disabled_plugins.iter().filter(|d| *d == "github").count(),
            1
        );
    }

    #[test]
    fn sanitized_names_match_hyphenated_disables() {
        let mut plugin = PluginSettings::default();
        plugin.system_enabled = true;
        plugin.set_plugin_enabled("my-server", false);
        assert!(!plugin.is_plugin_enabled_sanitized("my_server"));
        assert!(plugin.is_plugin_enabled_sanitized("other"));
        assert!(plugin.is_plugin_enabled_sanitized("anything"));
    }

    #[test]
    fn consent_records_version() {
        let mut plugin = PluginSettings::default();
        assert!(!plugin.has_consent("github"));
        plugin.set_consent("github", PLUGIN_CONSENT_VERSION);
        assert!(plugin.has_consent("github"));
        assert_eq!(
            plugin.consents.get("github").map(String::as_str),
            Some(PLUGIN_CONSENT_VERSION)
        );
    }

    #[test]
    fn settings_round_trip_through_json() {
        let mut plugin = PluginSettings::default();
        plugin.system_enabled = true;
        plugin.set_plugin_enabled("a", false);
        plugin.set_consent("b", PLUGIN_CONSENT_VERSION);
        plugin.add_dev_path("/tmp/plugins");
        let sharing = SharingSettings {
            artifact_sharing_enabled: true,
        };
        let json = serde_json::to_string(&serde_json::json!({
            "plugin": plugin,
            "sharing": sharing,
        }))
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let restored_plugin: PluginSettings =
            serde_json::from_value(value["plugin"].clone()).unwrap();
        let restored_sharing: SharingSettings =
            serde_json::from_value(value["sharing"].clone()).unwrap();
        assert_eq!(restored_plugin, plugin);
        assert_eq!(restored_sharing, sharing);
    }

    #[test]
    fn legacy_json_without_sections_gets_defaults() {
        // A pre-existing app_settings.json without plugin/sharing sections
        // must keep MCP servers and sharing working as before.
        #[derive(serde::Deserialize)]
        struct LegacyHost {
            #[serde(default)]
            plugin: PluginSettings,
            #[serde(default)]
            sharing: SharingSettings,
        }
        let legacy: LegacyHost =
            serde_json::from_str(r#"{"agent_backend":"cloud","selected_model":"gpt-4o"}"#)
                .unwrap();
        assert!(legacy.plugin.system_enabled);
        assert!(legacy.sharing.artifact_sharing_enabled);
    }

    #[test]
    fn dev_manifests_load_from_disk() {
        let temp = tempfile::tempdir().unwrap();
        let plug = temp.path().join("demo");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("plugin.json"),
            r#"{"name":"demo","version":"0.1.0"}"#,
        )
        .unwrap();
        let bare = temp.path().join("bare-dir");
        std::fs::create_dir_all(&bare).unwrap();

        let manifests =
            load_dev_plugin_manifests(&[temp.path().to_string_lossy().into_owned()]);
        let names: Vec<_> = manifests.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"demo"), "got: {names:?}");
        assert!(names.contains(&"bare-dir"), "got: {names:?}");
        let demo = manifests.iter().find(|m| m.name == "demo").unwrap();
        assert_eq!(demo.version.as_deref(), Some("0.1.0"));

        // Missing paths are ignored, not errors.
        assert!(load_dev_plugin_manifests(&["/does/not/exist-xyz".into()]).is_empty());
    }
}
