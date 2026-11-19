//! Persistent per-tool approval preferences.
//!
//! Stored at `~/.dragent/permissions.json`:
//! ```json
//! {
//!   "always_allow": ["edit_file", "grep"],
//!   "always_deny": [],
//!   "always_allow_read_roots": ["/Users/me/projects/shared-docs"]
//! }
//! ```
//!
//! When a tool is in `always_allow`, the controller skips the approval UI and
//! executes immediately. When it is in `always_deny`, the controller auto-rejects.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Store ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionStore {
    #[serde(default)]
    pub always_allow: HashSet<String>,
    #[serde(default)]
    pub always_deny: HashSet<String>,
    /// External directories the user explicitly granted permanent read access.
    /// Kept separate from `always_allow`: allowing one sibling project must not
    /// silently authorize the same tool against every path on the machine.
    #[serde(default)]
    pub always_allow_read_roots: HashSet<PathBuf>,
}

impl PermissionStore {
    /// Load from disk, or return a fresh default if missing/corrupt.
    pub fn load() -> Self {
        let path = store_path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn is_always_allowed(&self, tool_name: &str) -> bool {
        self.always_allow.contains(tool_name)
    }

    pub fn is_always_denied(&self, tool_name: &str) -> bool {
        self.always_deny.contains(tool_name)
    }

    pub fn is_read_path_allowed(&self, path: &std::path::Path) -> bool {
        self.always_allow_read_roots
            .iter()
            .any(|root| path.starts_with(root))
    }

    /// Permanently authorize reads beneath one directory.
    pub fn allow_read_root_always(&mut self, root: &std::path::Path) {
        self.always_allow_read_roots.insert(root.to_path_buf());
        self.save();
    }

    /// Mark a tool as always approved (without a dialog). Persists immediately.
    pub fn allow_always(&mut self, tool_name: &str) {
        self.always_deny.remove(tool_name);
        self.always_allow.insert(tool_name.to_string());
        self.save();
    }

    /// Mark a tool as always rejected. Persists immediately.
    pub fn deny_always(&mut self, tool_name: &str) {
        self.always_allow.remove(tool_name);
        self.always_deny.insert(tool_name.to_string());
        self.save();
    }

    /// Remove any persisted preference for a tool (back to per-call approval).
    pub fn reset(&mut self, tool_name: &str) {
        self.always_allow.remove(tool_name);
        self.always_deny.remove(tool_name);
        self.save();
    }

    fn save(&self) {
        let path = store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

fn store_path() -> PathBuf {
    dragent_paths::home().join("permissions.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_permission_files_deserialize_without_read_roots() {
        let store: PermissionStore =
            serde_json::from_str(r#"{"always_allow":["grep"],"always_deny":[]}"#).unwrap();
        assert!(store.is_always_allowed("grep"));
        assert!(store.always_allow_read_roots.is_empty());
    }

    #[test]
    fn external_read_grants_are_directory_scoped() {
        let mut store = PermissionStore::default();
        store
            .always_allow_read_roots
            .insert(PathBuf::from("/workspace/marketing"));
        assert!(
            store.is_read_path_allowed(std::path::Path::new("/workspace/marketing/docs/page.tsx"))
        );
        assert!(!store.is_read_path_allowed(std::path::Path::new("/workspace/server/main.go")));
    }
}
