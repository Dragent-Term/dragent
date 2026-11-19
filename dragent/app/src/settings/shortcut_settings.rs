//! Shortcuts, developer permissions, and computer-use settings.
//!
//! Persistence for three preference groups that live on [`AppSettings`]:
//!
//! * [`ShortcutSettings`] — user keybinding overrides (`action-id -> chord`),
//!   the terminal shortcut policy (`app-first` | `terminal-first`), and the
//!   Ctrl+Tab traversal order (MRU | creation). Optionally mirrored to
//!   `~/.dragent/keybindings.json` with load-merge.
//! * [`DevPermissionsSettings`] — keep-awake behaviour while agents run.
//! * [`ComputerUseSettings`] — the master switch, confirmation policy, and
//!   allowlist gating the [`ComputerUseGate`] executor.
//!
//! The OS-facing helpers in this module are deliberately honest: where the
//! platform offers no query API the permission state reports `Unknown`
//! instead of guessing, and the wake lock is a no-op (with a log line) on
//! platforms without an implementation.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

// ── Enums ────────────────────────────────────────────────────────────────────

/// Who wins when a keystroke could be both an app chord and terminal input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalShortcutPolicy {
    /// While a command runs, bytes go to the PTY first.
    TerminalFirst,
    /// App chords win (default, current behaviour). Values this version
    /// doesn't know, such as a retired name, read as this rather than
    /// failing the whole settings file.
    #[default]
    #[serde(other)]
    AppFirst,
}

impl TerminalShortcutPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::AppFirst => "App first",
            Self::TerminalFirst => "Terminal first",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::AppFirst => "App shortcuts win over terminal input",
            Self::TerminalFirst => "Running commands receive keys first",
        }
    }

    pub fn app_chords_win(self) -> bool {
        matches!(self, Self::AppFirst)
    }
}

/// Traversal order for Ctrl+Tab / Next-tab / Previous-tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CtrlTabOrder {
    /// Most-recently-used first. Each press re-orders the list, so repeated
    /// presses alternate between the two latest tabs.
    Mru,
    /// Left-to-right strip order: what Next/Previous tab always did.
    #[default]
    Creation,
}

impl CtrlTabOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mru => "Recently used",
            Self::Creation => "Creation order",
        }
    }
}

/// Keep-awake behaviour for long agent runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AwakeMode {
    /// Always hold a wake lock while the app runs.
    On,
    /// Never hold a wake lock.
    #[default]
    Off,
    /// Hold a wake lock only while agents run (and the keep-awake toggle is on).
    Auto,
}

impl AwakeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::On => "On",
            Self::Off => "Off",
            Self::Auto => "Auto",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::On => "Keep the machine awake while Dragent runs",
            Self::Off => "Let the machine sleep normally",
            Self::Auto => "Keep awake only while agents are running",
        }
    }
}

// ── Shortcut settings ────────────────────────────────────────────────────────

/// Default keymap. `(action_id, default_chord, label, section)`.
///
/// Action ids are stable: they are the keys of `custom_bindings`, the names
/// of the editable keymap bindings in `workspace/bindings.rs`, and the keys
/// of `~/.dragent/keybindings.json`.
pub const DEFAULT_BINDINGS: &[(&str, &str, &str, &str)] = &[
    // Tabs & windows
    ("new-tab", "cmdorctrl-t", "New tab", "Tabs & windows"),
    ("new-window", "cmdorctrl-n", "New window", "Tabs & windows"),
    (
        "new-agent-tab",
        "cmdorctrl-shift-N",
        "New agent tab",
        "Tabs & windows",
    ),
    ("close-tab", "cmdorctrl-w", "Close tab", "Tabs & windows"),
    ("split-right", "cmdorctrl-d", "Split right", "Tabs & windows"),
    (
        "split-down",
        "cmdorctrl-shift-D",
        "Split down",
        "Tabs & windows",
    ),
    (
        "prev-tab",
        "cmdorctrl-shift-[",
        "Previous tab",
        "Tabs & windows",
    ),
    (
        "next-tab",
        "cmdorctrl-shift-]",
        "Next tab",
        "Tabs & windows",
    ),
    (
        "open-settings",
        "cmdorctrl-,",
        "Open / close settings",
        "Tabs & windows",
    ),
    (
        "toggle-agents-panel",
        "cmdorctrl-shift-A",
        "Toggle agents panel",
        "Tabs & windows",
    ),
    // Terminal clipboard (keymap-registered on TerminalView).
    ("terminal-copy", "cmd-c", "Copy output", "Terminal"),
    ("terminal-paste", "cmdorctrl-v", "Paste", "Terminal"),
    // Terminal-embedded chords (handled in the terminal key handler, which
    // consults `custom_chord_matches` so overrides apply there too).
    (
        "history-search",
        "cmdorctrl-r",
        "Command history search",
        "Search & palettes",
    ),
    (
        "workflow-palette",
        "cmdorctrl-shift-P",
        "Workflow / command palette",
        "Search & palettes",
    ),
    (
        "cycle-input-type",
        "cmdorctrl-shift-I",
        "Cycle input type (shell / agent)",
        "Search & palettes",
    ),
    (
        "toggle-rich-input",
        "ctrl-g",
        "Toggle rich multiline input",
        "Agent input",
    ),
];

/// Validates a user-entered chord such as `cmdorctrl-shift-P` or `ctrl-g`.
/// Whitespace separates sequence steps; every step must parse as a
/// [`warpui::keymap::Keystroke`].
pub fn validate_chord(chord: &str) -> Result<(), String> {
    let chord = chord.trim();
    if chord.is_empty() {
        return Err("Enter a shortcut like cmdorctrl-t".to_string());
    }
    for step in chord.split_whitespace() {
        warpui::keymap::Keystroke::parse(step)
            .map(|_| ())
            .map_err(|_| format!("Unknown shortcut “{step}” — use forms like cmdorctrl-t, ctrl-g, shift-tab"))?;
    }
    Ok(())
}

/// True when `ks` matches the user's override chord for `action_id`.
/// Pure helper (no settings access) so terminal handlers and tests share it.
pub fn chord_text_matches(chord: &str, ks: &warpui::keymap::Keystroke) -> bool {
    // Single-step fast path: compare normalized forms, which also folds
    // `cmdorctrl` to the platform's real modifier.
    match warpui::keymap::Keystroke::parse(chord.trim()) {
        Ok(want) => want.normalized() == ks.normalized(),
        Err(_) => {
            // Multi-step sequences never match a single keystroke here; the
            // keymap matcher owns those. Report no match honestly.
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShortcutSettings {
    /// Action-id → chord overrides. Empty = all defaults.
    #[serde(default)]
    pub custom_bindings: HashMap<String, String>,
    #[serde(default)]
    pub terminal_shortcut_policy: TerminalShortcutPolicy,
    #[serde(default)]
    pub ctrl_tab_order: CtrlTabOrder,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            custom_bindings: HashMap::new(),
            terminal_shortcut_policy: TerminalShortcutPolicy::AppFirst,
            ctrl_tab_order: CtrlTabOrder::Creation,
        }
    }
}

impl ShortcutSettings {
    pub fn default_chord(action_id: &str) -> Option<&'static str> {
        DEFAULT_BINDINGS
            .iter()
            .find(|(id, _, _, _)| *id == action_id)
            .map(|(_, chord, _, _)| *chord)
    }

    /// Chord actually in effect for `action_id` (override or default).
    pub fn effective_chord(&self, action_id: &str) -> Option<String> {
        if let Some(custom) = self.custom_bindings.get(action_id) {
            return Some(custom.clone());
        }
        Self::default_chord(action_id).map(str::to_string)
    }

    pub fn set_binding(&mut self, action_id: &str, chord: &str) -> Result<(), String> {
        validate_chord(chord)?;
        let chord = chord.trim().to_string();
        // A chord identical to the default is stored as "no override" so
        // Reset semantics stay exact and the mirror file stays small.
        if Self::default_chord(action_id).is_some_and(|d| d == chord) {
            self.custom_bindings.remove(action_id);
        } else {
            self.custom_bindings
                .insert(action_id.to_string(), chord);
        }
        Ok(())
    }

    /// Forget one override. Returns true when one existed.
    pub fn reset_binding(&mut self, action_id: &str) -> bool {
        self.custom_bindings.remove(action_id).is_some()
    }

    pub fn reset_all_bindings(&mut self) {
        self.custom_bindings.clear();
    }

    /// Mirror file: `~/.dragent/keybindings.json`, a plain action-id → chord
    /// map.
    pub fn mirror_path() -> PathBuf {
        dragent_paths::home().join("keybindings.json")
    }

    /// Merge the mirror file into these settings (mirror wins). Unknown ids
    /// are kept — forward-compatible with hand-edited files.
    pub fn load_mirror_merge(&mut self) {
        let raw = std::fs::read_to_string(Self::mirror_path()).unwrap_or_default();
        if raw.trim().is_empty() {
            return;
        }
        if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&raw) {
            for (id, chord) in map {
                if validate_chord(&chord).is_ok() {
                    self.custom_bindings.insert(id, chord.trim().to_string());
                } else {
                    log::warn!("[shortcuts] ignoring invalid chord in keybindings.json");
                }
            }
        }
    }

    pub fn save_mirror(&self) {
        let path = Self::mirror_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.custom_bindings) {
            let _ = fs::write(path, json);
        }
    }
}

// ── Developer permissions ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevPermissionsSettings {
    #[serde(default)]
    pub keep_awake_while_agents_run: bool,
    #[serde(default)]
    pub awake_mode: AwakeMode,
}

impl Default for DevPermissionsSettings {
    fn default() -> Self {
        Self {
            keep_awake_while_agents_run: false,
            awake_mode: AwakeMode::Off,
        }
    }
}

/// Readable state of one macOS permission row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
}

impl PermissionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Granted => "Granted",
            Self::Denied => "Not granted",
            Self::Unknown => "Unknown",
        }
    }

    pub fn granted(self) -> bool {
        matches!(self, Self::Granted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionReport {
    pub accessibility: PermissionState,
    pub full_disk_access: PermissionState,
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Query macOS Accessibility trust. Real check on macOS (no prompt — a pure
/// query); `Unknown` everywhere else, honestly reported in the UI.
pub fn query_accessibility() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: AXIsProcessTrusted is a pure C query with no arguments.
        let trusted = unsafe { AXIsProcessTrusted() };
        return if trusted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        };
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Unknown
    }
}

/// Probe Full Disk Access by attempting to list a TCC-guarded directory.
/// Readable → Granted, permission-denied → Denied, anything else → Unknown.
pub fn query_full_disk_access() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        match fs::read_dir("/Library/Application Support/com.apple.TCC") {
            Ok(_) => PermissionState::Granted,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                PermissionState::Denied
            }
            Err(_) => PermissionState::Unknown,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = fs::read_dir(".").map(|_| ());
        PermissionState::Unknown
    }
}

pub fn query_permission_report() -> PermissionReport {
    PermissionReport {
        accessibility: query_accessibility(),
        full_disk_access: query_full_disk_access(),
    }
}

// ── Wake lock ────────────────────────────────────────────────────────────────

/// Whether any agent run is currently active. Set by the automations runtime
/// (`AutomationRuntime::reconcile`); read by [`sync_wake_lock`].
static AGENTS_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn set_agents_running(running: bool) {
    AGENTS_RUNNING.store(running, Ordering::Relaxed);
}

pub fn agents_running() -> bool {
    AGENTS_RUNNING.load(Ordering::Relaxed)
}

/// Pure decision function (tested): should a wake lock be held right now?
pub fn should_hold_wake(mode: AwakeMode, keep_awake: bool, running: bool) -> bool {
    match mode {
        AwakeMode::On => true,
        AwakeMode::Off => false,
        AwakeMode::Auto => keep_awake && running,
    }
}

fn wake_child() -> &'static Mutex<Option<std::process::Child>> {
    static CHILD: OnceLock<Mutex<Option<std::process::Child>>> = OnceLock::new();
    CHILD.get_or_init(|| Mutex::new(None))
}

pub fn wake_lock_held() -> bool {
    let Ok(mut guard) = wake_child().lock() else {
        return false;
    };
    match guard.as_mut().map(|c| c.try_wait()) {
        // Child reaped itself (killed externally) — drop the stale handle.
        Some(Ok(Some(_))) => {
            *guard = None;
            false
        }
        Some(Ok(None)) => true,
        _ => guard.is_some(),
    }
}

/// Acquire the OS wake lock (`caffeinate` on macOS; no-op + log elsewhere).
pub fn acquire_wake_lock(reason: &str) {
    if wake_lock_held() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("caffeinate")
            .args(["-dims", "-w", &std::process::id().to_string()])
            // `caffeinate -w <pid>` exits with the pid; instead hold with
            // plain flags so the lock lives until we kill the child.
            .arg("-i")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                // `-w` + plain spawn conflict: `caffeinate -w pid` waits for
                // the pid, which is exactly "hold while Dragent runs". Keep
                // the child so `release` can kill it.
                *wake_child().lock().unwrap() = Some(child);
                log::info!("[wake] acquired ({reason})");
            }
            Err(e) => log::warn!("[wake] caffeinate spawn failed: {e}"),
        }
        return;
    }
    #[cfg(not(target_os = "macos"))]
    {
        log::info!("[wake] keep-awake requested ({reason}) — no OS implementation on this platform, ignoring");
    }
}

pub fn release_wake_lock() {
    let child = wake_child().lock().ok().and_then(|mut c| c.take());
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
        log::info!("[wake] released");
    }
}

/// Reconcile the OS wake lock with the current settings. Call after every
/// settings save, on `AppSettingsChanged`, and when agent activity changes.
pub fn sync_wake_lock(dev: &DevPermissionsSettings) {
    let want = should_hold_wake(dev.awake_mode, dev.keep_awake_while_agents_run, agents_running());
    if want {
        acquire_wake_lock("settings sync");
    } else {
        release_wake_lock();
    }
}

// ── Computer use ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputerUseSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub require_confirmation: bool,
    #[serde(default)]
    pub allowed_apps: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for ComputerUseSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            require_confirmation: true,
            allowed_apps: Vec::new(),
        }
    }
}

impl ComputerUseSettings {
    pub fn allow_app(&mut self, app: &str) {
        let app = app.trim().to_string();
        if app.is_empty() {
            return;
        }
        if !self.allowed_apps.iter().any(|a| a.eq_ignore_ascii_case(&app)) {
            self.allowed_apps.push(app);
        }
    }

    pub fn remove_app(&mut self, app: &str) -> bool {
        let before = self.allowed_apps.len();
        self.allowed_apps.retain(|a| !a.eq_ignore_ascii_case(app));
        self.allowed_apps.len() != before
    }

    pub fn is_app_allowed(&self, app: &str) -> bool {
        // Empty allowlist = every app is permitted (once the master switch
        // and confirmation gates pass). A non-empty list restricts to members.
        self.allowed_apps.is_empty() || self.allowed_apps.iter().any(|a| a.eq_ignore_ascii_case(app))
    }
}

/// One computer-use (synthetic input / automation) request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerUseRequest {
    /// Target application name, e.g. "Terminal" or "Safari".
    pub app: String,
    /// Human-readable description shown in the confirmation prompt.
    pub detail: String,
}

/// Gate every computer-use execution path through this. `confirmed` must be
/// true when the settings require confirmation (the caller shows the prompt
/// and passes the user's answer back in).
pub fn check_computer_use(
    settings: &ComputerUseSettings,
    req: &ComputerUseRequest,
    confirmed: bool,
) -> Result<(), String> {
    if !settings.enabled {
        return Err("Computer use is disabled — enable it in Settings → Computer use.".to_string());
    }
    if !settings.is_app_allowed(&req.app) {
        return Err(format!(
            "“{}” is not in the computer-use allowlist.",
            req.app
        ));
    }
    if settings.require_confirmation && !confirmed {
        return Err("This computer action needs explicit confirmation.".to_string());
    }
    Ok(())
}

/// Minimal gated executor: the gate runs first; `run` only executes when the
/// gate passes. All current and future computer-use/automation entry points
/// must go through here.
pub fn execute_computer_action<T>(
    settings: &ComputerUseSettings,
    req: &ComputerUseRequest,
    confirmed: bool,
    run: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    check_computer_use(settings, req, confirmed)?;
    log::info!("[computer-use] executing for app “{}” ({})", req.app, req.detail);
    run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_shortcut_policies_read_as_the_default() {
        let parse = |raw: &str| serde_json::from_str::<TerminalShortcutPolicy>(raw).unwrap();
        assert_eq!(parse("\"terminal_first\""), TerminalShortcutPolicy::TerminalFirst);
        assert_eq!(parse("\"app_first\""), TerminalShortcutPolicy::AppFirst);
        assert_eq!(parse("\"a_retired_name\""), TerminalShortcutPolicy::AppFirst);
        assert_eq!(serde_json::to_string(&TerminalShortcutPolicy::AppFirst).unwrap(), "\"app_first\"");
    }

    #[test]
    fn defaults_are_safe() {
        let s = ShortcutSettings::default();
        assert!(s.custom_bindings.is_empty());
        assert_eq!(s.terminal_shortcut_policy, TerminalShortcutPolicy::AppFirst);
        assert_eq!(s.ctrl_tab_order, CtrlTabOrder::Creation);

        let d = DevPermissionsSettings::default();
        assert!(!d.keep_awake_while_agents_run);
        assert_eq!(d.awake_mode, AwakeMode::Off);

        let c = ComputerUseSettings::default();
        assert!(!c.enabled);
        assert!(c.require_confirmation);
        assert!(c.allowed_apps.is_empty());
    }

    #[test]
    fn legacy_json_loads_with_safe_defaults() {
        let s: ShortcutSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.terminal_shortcut_policy, TerminalShortcutPolicy::AppFirst);
        assert_eq!(s.ctrl_tab_order, CtrlTabOrder::Creation);
        let d: DevPermissionsSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(d.awake_mode, AwakeMode::Off);
        let c: ComputerUseSettings = serde_json::from_str("{}").unwrap();
        assert!(!c.enabled);
        // require_confirmation defaults true even when absent.
        assert!(c.require_confirmation);
    }

    #[test]
    fn shortcut_round_trip() {
        let mut s = ShortcutSettings::default();
        s.set_binding("new-tab", "ctrl-t").unwrap();
        s.terminal_shortcut_policy = TerminalShortcutPolicy::TerminalFirst;
        s.ctrl_tab_order = CtrlTabOrder::Mru;
        let json = serde_json::to_string(&s).unwrap();
        let back: ShortcutSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.effective_chord("new-tab").as_deref(), Some("ctrl-t"));
        assert_eq!(
            back.effective_chord("close-tab").as_deref(),
            Some("cmdorctrl-w")
        );
    }

    #[test]
    fn invalid_chords_rejected() {
        assert!(validate_chord("").is_err());
        assert!(validate_chord("   ").is_err());
        assert!(validate_chord("not a valid chord !!!").is_err());
        assert!(validate_chord("cmdorctrl-t").is_ok());
        assert!(validate_chord("ctrl-g").is_ok());
        assert!(validate_chord("cmdorctrl-shift-P").is_ok());
    }

    #[test]
    fn default_chord_rebind_collapses_to_no_override() {
        let mut s = ShortcutSettings::default();
        s.set_binding("new-tab", "cmdorctrl-t").unwrap();
        assert!(s.custom_bindings.is_empty());
    }

    #[test]
    fn reset_semantics() {
        let mut s = ShortcutSettings::default();
        assert!(!s.reset_binding("new-tab"));
        s.set_binding("new-tab", "ctrl-t").unwrap();
        assert!(s.reset_binding("new-tab"));
        assert!(s.custom_bindings.is_empty());
        s.set_binding("new-tab", "ctrl-t").unwrap();
        s.set_binding("close-tab", "ctrl-w").unwrap();
        s.reset_all_bindings();
        assert!(s.custom_bindings.is_empty());
    }

    #[test]
    fn every_default_binding_parses() {
        for (id, chord, _, _) in DEFAULT_BINDINGS {
            assert!(validate_chord(chord).is_ok(), "{id}: {chord}");
        }
        // Ids are unique.
        let mut ids: Vec<&str> = DEFAULT_BINDINGS.iter().map(|(id, _, _, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), DEFAULT_BINDINGS.len());
    }

    #[test]
    fn chord_text_matches_folds_cmdorctrl() {
        let ks = warpui::keymap::Keystroke::parse("cmdorctrl-t").unwrap();
        // On macOS cmdrorctrl == cmd; elsewhere == ctrl. Either way one of
        // the two spellings matches the parsed keystroke.
        let cmd = warpui::keymap::Keystroke::parse("cmd-t").unwrap();
        let ctrl = warpui::keymap::Keystroke::parse("ctrl-t").unwrap();
        assert!(chord_text_matches("cmdorctrl-t", &ks));
        assert!(chord_text_matches("cmdorctrl-t", &cmd) || chord_text_matches("cmdorctrl-t", &ctrl));
        assert!(!chord_text_matches("cmdorctrl-t", &warpui::keymap::Keystroke::parse("cmd-w").unwrap()));
    }

    #[test]
    fn wake_decision_matrix() {
        assert!(should_hold_wake(AwakeMode::On, false, false));
        assert!(!should_hold_wake(AwakeMode::Off, true, true));
        assert!(should_hold_wake(AwakeMode::Auto, true, true));
        assert!(!should_hold_wake(AwakeMode::Auto, true, false));
        assert!(!should_hold_wake(AwakeMode::Auto, false, true));
    }

    #[test]
    fn computer_gate_matrix() {
        let disabled = ComputerUseSettings::default();
        let req = ComputerUseRequest {
            app: "Terminal".into(),
            detail: "type ls".into(),
        };
        assert!(check_computer_use(&disabled, &req, true).is_err());

        let mut on = ComputerUseSettings {
            enabled: true,
            ..Default::default()
        };
        // Confirmation required by default.
        assert!(check_computer_use(&on, &req, false).is_err());
        assert!(check_computer_use(&on, &req, true).is_ok());

        on.require_confirmation = false;
        assert!(check_computer_use(&on, &req, false).is_ok());

        on.allowed_apps = vec!["Safari".into()];
        assert!(check_computer_use(&on, &req, true).is_err());
        let safari = ComputerUseRequest {
            app: "safari".into(),
            detail: "open page".into(),
        };
        assert!(check_computer_use(&on, &safari, true).is_ok());
    }

    #[test]
    fn gated_executor_blocks_without_running() {
        let off = ComputerUseSettings::default();
        let req = ComputerUseRequest {
            app: "Terminal".into(),
            detail: "x".into(),
        };
        let mut ran = false;
        let res: Result<(), String> =
            execute_computer_action(&off, &req, true, || {
                ran = true;
                Ok(())
            });
        assert!(res.is_err());
        assert!(!ran);

        let on = ComputerUseSettings {
            enabled: true,
            require_confirmation: false,
            allowed_apps: vec![],
        };
        let res: Result<&str, String> =
            execute_computer_action(&on, &req, false, || Ok("did it"));
        assert_eq!(res.unwrap(), "did it");
    }

    #[test]
    fn allowlist_add_remove_is_case_insensitive() {
        let mut c = ComputerUseSettings::default();
        c.allow_app("Terminal");
        c.allow_app("terminal");
        assert_eq!(c.allowed_apps.len(), 1);
        assert!(c.remove_app("TERMINAL"));
        assert!(c.allowed_apps.is_empty());
        assert!(!c.remove_app("nope"));
    }

    #[test]
    fn permission_report_shape() {
        let r = query_permission_report();
        // Must always return *something* honest; on CI (Linux) both are Unknown.
        let _ = (r.accessibility.label(), r.full_disk_access.label());
    }
}
