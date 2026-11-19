//! `<store>/installed.json`: what's installed, from where, and what each
//! plugin placed (skill files with their hashes, server entries by account).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub plugins: Vec<InstalledPlugin>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// `folder` or `git`.
    pub kind: String,
    /// The folder, or the repository's address.
    pub location: String,
    /// The commit installed from a repository.
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: Source,
    /// The installed package.
    pub path: PathBuf,
    pub installed_at_ms: i64,
    /// Off: its skills are taken out of the shared folder and its servers
    /// switched off; the package and the person's settings stay.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Skill folder → file → hash of what this plugin wrote there.
    #[serde(default)]
    pub skills: BTreeMap<String, BTreeMap<String, String>>,
    /// Accounts: each is its own copy of every server, with its own
    /// credentials. `default` is the first.
    #[serde(default = "default_accounts")]
    pub accounts: Vec<String>,
    /// Steps a removal or update couldn't finish, for the next status.
    #[serde(default)]
    pub unfinished: Vec<String>,
}

impl InstalledPlugin {
    /// Its MCP server entries, every account's. Unknown while its package is
    /// missing.
    pub fn server_entries(&self) -> Vec<String> {
        crate::Manifest::read(&self.path)
            .map(|manifest| {
                crate::servers::plan(&manifest, &self.path, &self.accounts)
                    .into_iter()
                    .map(|planned| planned.entry)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn yes() -> bool {
    true
}

fn default_accounts() -> Vec<String> {
    vec!["default".into()]
}

impl Record {
    pub fn path(store: &Path) -> PathBuf {
        store.join("installed.json")
    }

    pub fn load(store: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(Self::path(store)) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|error| format!("{} isn't readable: {error}", Self::path(store).display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Write it whole (a temp file renamed into place).
    pub fn save(&self, store: &Path) -> Result<(), String> {
        std::fs::create_dir_all(store).map_err(|error| error.to_string())?;
        let path = Self::path(store);
        let temp = store.join(".installed.json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(&temp, text).map_err(|error| error.to_string())?;
        std::fs::rename(&temp, &path).map_err(|error| error.to_string())
    }

    pub fn get(&self, id: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|plugin| plugin.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut InstalledPlugin> {
        self.plugins.iter_mut().find(|plugin| plugin.id == id)
    }

    /// Insert or replace a plugin's entry.
    pub fn put(&mut self, plugin: InstalledPlugin) {
        match self.get_mut(&plugin.id) {
            Some(existing) => *existing = plugin,
            None => self.plugins.push(plugin),
        }
        self.plugins.sort_by(|a, b| a.id.cmp(&b.id));
    }

    pub fn remove(&mut self, id: &str) {
        self.plugins.retain(|plugin| plugin.id != id);
    }

    /// Every MCP server entry an installed plugin owns.
    pub fn server_entries(&self) -> Vec<String> {
        self.plugins.iter().flat_map(InstalledPlugin::server_entries).collect()
    }

    /// Which plugin placed the shared skill folder `name`, if any.
    pub fn skill_owner(&self, name: &str) -> Option<&str> {
        self.plugins
            .iter()
            .find(|plugin| plugin.skills.contains_key(name))
            .map(|plugin| plugin.id.as_str())
    }
}
