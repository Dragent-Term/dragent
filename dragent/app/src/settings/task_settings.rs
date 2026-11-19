//! Tasks + SSH host settings persisted in `AppSettings`.
//!
//! * [`TaskSettings`] — default task view/source, visible providers,
//!   default repos & Linear teams.
//! * [`HostSettings`] + [`HostOverride`] — default host for new projects,
//!   per-host display label / worktree location, local runtime (host vs WSL),
//!   WSL distro, SSH workspace-routing toggles.

use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

// ── Task settings ─────────────────────────────────────────────────────────

fn default_view_preset() -> String {
    "all".to_string()
}

fn default_source() -> String {
    "github".to_string()
}

/// Every task service is offered until the user hides one, as before this
/// setting existed.
fn default_visible_providers() -> Vec<String> {
    TASK_PROVIDER_KEYS.iter().map(|key| key.to_string()).collect()
}

/// All provider keys the Tasks UI can toggle, beyond GitHub, Linear and
/// Jira, so other trackers keep working when enabled.
pub const TASK_PROVIDER_KEYS: &[&str] = &[
    "github", "linear", "jira", "gitlab", "plane", "youtrack", "asana", "trello", "monday",
];

/// View presets offered by the Tasks section.
pub const TASK_VIEW_PRESETS: &[&str] = &["all", "open", "mine", "closed"];

/// Sources offered as the default task source.
pub const TASK_SOURCES: &[&str] = &["github", "linear", "jira"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSettings {
    #[serde(default = "default_view_preset")]
    pub default_view_preset: String,
    #[serde(default = "default_source")]
    pub default_source: String,
    #[serde(default = "default_visible_providers")]
    pub visible_providers: Vec<String>,
    #[serde(default)]
    pub default_repos: Vec<String>,
    #[serde(default)]
    pub default_linear_teams: Vec<String>,
}

impl Default for TaskSettings {
    fn default() -> Self {
        Self {
            default_view_preset: default_view_preset(),
            default_source: default_source(),
            visible_providers: default_visible_providers(),
            default_repos: Vec::new(),
            default_linear_teams: Vec::new(),
        }
    }
}

pub fn normalize_provider_key(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

impl TaskSettings {
    /// Provider keys, lower-cased and de-duplicated, in a stable order.
    pub fn visible_set(&self) -> HashSet<String> {
        self.visible_providers
            .iter()
            .map(|k| normalize_provider_key(k))
            .filter(|k| !k.is_empty())
            .collect()
    }

    pub fn is_visible(&self, provider_key: &str) -> bool {
        self.visible_set()
            .contains(&normalize_provider_key(provider_key))
    }

    pub fn set_visible(&mut self, provider_key: &str, visible: bool) {
        let key = normalize_provider_key(provider_key);
        if key.is_empty() {
            return;
        }
        if visible {
            if !self.visible_providers.iter().any(|k| normalize_provider_key(k) == key) {
                self.visible_providers.push(key);
            }
        } else {
            self.visible_providers
                .retain(|k| normalize_provider_key(k) != key);
        }
    }

    /// Ordered provider keys: default source first, then the rest of the
    /// visible set, then hidden ones (for pickers that show everything).
    pub fn ordered_providers(&self) -> Vec<String> {
        let visible = self.visible_set();
        let mut ordered = Vec::new();
        let default = normalize_provider_key(&self.default_source);
        if visible.contains(&default) {
            ordered.push(default.clone());
        }
        let mut rest: Vec<String> = visible
            .iter()
            .filter(|k| **k != default)
            .cloned()
            .collect();
        rest.sort();
        ordered.extend(rest);
        ordered
    }

    /// Default `place` when filing a new item with no explicit destination.
    pub fn default_place(&self, provider_key: &str) -> Option<String> {
        match normalize_provider_key(provider_key).as_str() {
            "linear" => self
                .default_linear_teams
                .iter()
                .find(|t| !t.trim().is_empty())
                .cloned(),
            "github" | "gitlab" | "forgejo" | "jira" | "plane" | "youtrack" => self
                .default_repos
                .iter()
                .find(|r| !r.trim().is_empty())
                .cloned(),
            _ => None,
        }
    }

    pub fn add_repo(&mut self, repo: &str) -> bool {
        let repo = repo.trim().to_string();
        if repo.is_empty() || self.default_repos.iter().any(|r| r == &repo) {
            return false;
        }
        self.default_repos.push(repo);
        true
    }

    pub fn add_team(&mut self, team: &str) -> bool {
        let team = team.trim().to_string();
        if team.is_empty() || self.default_linear_teams.iter().any(|t| t == &team) {
            return false;
        }
        self.default_linear_teams.push(team);
        true
    }
}

// ── Host / SSH settings ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_worktree_location: Option<String>,
}

impl HostOverride {
    pub fn display_label(&self) -> Option<&str> {
        self.display_label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn worktree_location(&self) -> Option<&str> {
        self.default_worktree_location
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        self.display_label().is_none() && self.worktree_location().is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LocalRuntime {
    #[default]
    Auto,
    Host,
    Wsl,
}

impl LocalRuntime {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Host => "This machine",
            Self::Wsl => "WSL",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Auto, Self::Host, Self::Wsl]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSettings {
    /// Machine id new projects default to (`None` = first available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_runtime_id: Option<String>,
    #[serde(default)]
    pub overrides: HashMap<String, HostOverride>,
    /// Master switch for the SSH workspace-routing path.
    #[serde(default)]
    pub ssh_routing_enabled: bool,
    /// Target ids that never use workspace routing (plain SSH instead).
    #[serde(default)]
    pub routing_disabled_targets: Vec<String>,
    /// Targets skipped by connection probing.
    #[serde(default)]
    pub routing_probe_skipped: Vec<String>,
    #[serde(default)]
    pub local_runtime: LocalRuntime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsl_distro: Option<String>,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            active_runtime_id: None,
            overrides: HashMap::new(),
            ssh_routing_enabled: false,
            routing_disabled_targets: Vec::new(),
            routing_probe_skipped: Vec::new(),
            local_runtime: LocalRuntime::Auto,
            wsl_distro: None,
        }
    }
}

impl HostSettings {
    pub fn override_for(&self, id: &str) -> Option<&HostOverride> {
        self.overrides.get(id)
    }

    pub fn override_mut(&mut self, id: &str) -> &mut HostOverride {
        self.overrides.entry(id.to_string()).or_default()
    }

    fn prune(&mut self, id: &str) {
        if self.overrides.get(id).is_some_and(|o| o.is_empty()) {
            self.overrides.remove(id);
        }
    }

    pub fn set_display_label(&mut self, id: &str, label: &str) {
        let label = label.trim();
        if label.is_empty() {
            if let Some(o) = self.overrides.get_mut(id) {
                o.display_label = None;
            }
        } else {
            self.override_mut(id).display_label = Some(label.to_string());
        }
        self.prune(id);
    }

    pub fn set_worktree_location(&mut self, id: &str, location: &str) {
        let location = location.trim();
        if location.is_empty() {
            if let Some(o) = self.overrides.get_mut(id) {
                o.default_worktree_location = None;
            }
        } else {
            self.override_mut(id).default_worktree_location =
                Some(location.to_string());
        }
        self.prune(id);
    }

    /// Effective display name: override wins, else the saved fallback.
    pub fn display_name_owned(&self, id: &str, fallback: &str) -> String {
        self.overrides
            .get(id)
            .and_then(|o| o.display_label())
            .unwrap_or(fallback)
            .to_string()
    }

    /// Effective remote base directory: override wins, else the machine's
    /// reported home (or `~` when unknown).
    pub fn worktree_base(&self, id: &str, home_fallback: Option<&str>) -> String {
        if let Some(loc) = self.overrides.get(id).and_then(|o| o.worktree_location()) {
            return loc.to_string();
        }
        home_fallback.unwrap_or("~").to_string()
    }

    /// Whether `target` may use the SSH workspace-routing path.
    pub fn routing_enabled_for(&self, target: &str) -> bool {
        if !self.ssh_routing_enabled {
            return false;
        }
        let target = target.trim();
        if target.is_empty() {
            return false;
        }
        !self.routing_disabled_targets.iter().any(|t| t == target)
            && !self.routing_probe_skipped.iter().any(|t| t == target)
    }

    pub fn toggle_disabled_target(&mut self, target: &str) -> bool {
        let target = target.trim().to_string();
        if target.is_empty() {
            return false;
        }
        if let Some(pos) = self.routing_disabled_targets.iter().position(|t| t == &target) {
            self.routing_disabled_targets.remove(pos);
            false
        } else {
            self.routing_disabled_targets.push(target);
            true
        }
    }

    pub fn toggle_probe_skipped(&mut self, target: &str) -> bool {
        let target = target.trim().to_string();
        if target.is_empty() {
            return false;
        }
        if let Some(pos) = self.routing_probe_skipped.iter().position(|t| t == &target) {
            self.routing_probe_skipped.remove(pos);
            false
        } else {
            self.routing_probe_skipped.push(target);
            true
        }
    }

    /// Which backend local agent runs use. `Auto` prefers WSL only when a
    /// distro is selected and the OS can run it; otherwise the host.
    pub fn effective_wsl_distro(&self) -> Option<&str> {
        match self.local_runtime {
            LocalRuntime::Wsl => self
                .wsl_distro
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            LocalRuntime::Host => None,
            LocalRuntime::Auto => self
                .wsl_distro
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter(|_| cfg!(target_os = "windows")),
        }
    }

    /// Human label for the execution backend, shown in SSH/Machines and the
    /// new-project device chip.
    pub fn execution_backend_label(&self) -> String {
        match self.effective_wsl_distro() {
            Some(distro) => format!("WSL: {distro}"),
            None => "This machine".to_string(),
        }
    }

    /// Prefix a local shell command for the selected backend. WSL wraps in
    /// `wsl -d <distro> -- …`; the host runs unchanged.
    pub fn wrap_local_command(&self, command: &str) -> String {
        match self.effective_wsl_distro() {
            Some(distro) => format!("wsl -d {distro} -- {command}"),
            None => command.to_string(),
        }
    }

    /// Pick the default machine for a new project: the configured host when
    /// it still exists, else the first available.
    pub fn default_machine<'a>(&self, available: &'a [String]) -> Option<&'a String> {
        if let Some(active) = self.active_runtime_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(found) = available.iter().find(|id| id.as_str() == active) {
                return Some(found);
            }
        }
        available.first()
    }
}

/// Distros from `wsl -l -q` on Windows; empty elsewhere or on failure.
fn wsl_distro_cache() -> &'static std::sync::Mutex<Vec<String>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The WSL distros found by the last [`refresh_wsl_distros`]. Never blocks,
/// so settings can call it while rendering.
pub fn available_wsl_distros() -> Vec<String> {
    wsl_distro_cache().lock().map(|distros| distros.clone()).unwrap_or_default()
}

/// List the installed WSL distros again (`wsl -l -q`, so off the main
/// thread). Always empty off Windows.
pub fn refresh_wsl_distros() -> Vec<String> {
    #[cfg(target_os = "windows")]
    let fresh = {
        use crate::process_ext::NoWindow as _;
        std::process::Command::new("wsl")
            .args(["-l", "-q"])
            .no_window()
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| parse_wsl_list(&o.stdout))
            .unwrap_or_default()
    };
    #[cfg(not(target_os = "windows"))]
    let fresh = Vec::new();
    if let Ok(mut distros) = wsl_distro_cache().lock() {
        *distros = fresh.clone();
    }
    fresh
}

/// `wsl -l -q` prints UTF-16LE; read UTF-8 too in case that changes.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_wsl_list(bytes: &[u8]) -> Vec<String> {
    let text = if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c == '\0' || c == '\u{feff}'))
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_list_reads_utf16_and_utf8() {
        let utf16: Vec<u8> = "\u{feff}Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_wsl_list(&utf16), ["Ubuntu", "Debian"]);
        assert_eq!(parse_wsl_list(b"Ubuntu-22.04\n\n"), ["Ubuntu-22.04"]);
    }

    #[test]
    fn task_defaults_offer_every_service() {
        let s = TaskSettings::default();
        assert_eq!(s.default_view_preset, "all");
        assert_eq!(s.default_source, "github");
        assert_eq!(s.visible_providers, TASK_PROVIDER_KEYS);
        assert!(s.default_repos.is_empty());
        assert!(s.default_linear_teams.is_empty());
    }

    #[test]
    fn task_legacy_json_gets_defaults() {
        let s: TaskSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.default_view_preset, "all");
        assert!(s.is_visible("github"));
        // Saved before this setting existed: nothing hidden.
        assert!(s.is_visible("gitlab") && s.is_visible("trello"));
        let app: serde_json::Value = serde_json::from_str(r#"{"agent_backend":"cloud"}"#).unwrap();
        assert!(app.get("tasks").is_none());
    }

    #[test]
    fn visible_toggle_round_trips() {
        let mut s = TaskSettings::default();
        assert!(s.is_visible("GitHub"));
        s.set_visible("github", false);
        assert!(!s.is_visible("github"));
        s.set_visible("plane", false);
        assert!(!s.is_visible("plane"));
        s.set_visible("plane", true);
        assert!(s.is_visible("plane"));
        let json = serde_json::to_string(&s).unwrap();
        let back: TaskSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn ordered_providers_put_default_first() {
        let s = TaskSettings::default();
        let ordered = s.ordered_providers();
        assert_eq!(ordered.first().map(String::as_str), Some("github"));
    }

    #[test]
    fn host_overrides_prefer_labels_and_paths() {
        let mut h = HostSettings::default();
        assert_eq!(h.display_name_owned("m1", "Stage"), "Stage");
        h.set_display_label("m1", "  Prod  ");
        assert_eq!(h.display_name_owned("m1", "Stage"), "Prod");
        assert_eq!(h.worktree_base("m1", Some("/home/u")), "/home/u");
        h.set_worktree_location("m1", "/srv/work");
        assert_eq!(h.worktree_base("m1", Some("/home/u")), "/srv/work");
        h.set_display_label("m1", "");
        h.set_worktree_location("m1", " ");
        assert!(!h.overrides.contains_key("m1"));
    }

    #[test]
    fn routing_gates_disabled_and_skipped() {
        let mut h = HostSettings::default();
        assert!(!h.routing_enabled_for("host-a"));
        h.ssh_routing_enabled = true;
        assert!(h.routing_enabled_for("host-a"));
        h.toggle_disabled_target("host-a");
        assert!(!h.routing_enabled_for("host-a"));
        h.toggle_disabled_target("host-a");
        assert!(h.routing_enabled_for("host-a"));
        h.toggle_probe_skipped("host-a");
        assert!(!h.routing_enabled_for("host-a"));
    }

    #[test]
    fn local_runtime_wraps_wsl_commands() {
        let mut h = HostSettings::default();
        h.local_runtime = LocalRuntime::Wsl;
        h.wsl_distro = Some("Ubuntu".into());
        assert_eq!(h.effective_wsl_distro(), Some("Ubuntu"));
        assert_eq!(h.wrap_local_command("ls"), "wsl -d Ubuntu -- ls");
        h.local_runtime = LocalRuntime::Host;
        assert_eq!(h.effective_wsl_distro(), None);
        assert_eq!(h.wrap_local_command("ls"), "ls");
    }

    #[test]
    fn default_machine_prefers_active_runtime() {
        let mut h = HostSettings::default();
        let ids = vec!["a".to_string(), "b".to_string()];
        assert_eq!(h.default_machine(&ids).map(String::as_str), Some("a"));
        h.active_runtime_id = Some("b".into());
        assert_eq!(h.default_machine(&ids).map(String::as_str), Some("b"));
        h.active_runtime_id = Some("gone".into());
        assert_eq!(h.default_machine(&ids).map(String::as_str), Some("a"));
    }
}
