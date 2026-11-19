//! Desktop machine identities and project references. SSH addresses are editable data,
//! never identifiers. Secrets remain in the OS credential store.
use crate::remote::SshHost;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub connection: SshHost,
}
impl Machine {
    pub fn name(&self) -> String {
        if self.connection.name.is_empty() {
            self.connection.label()
        } else {
            self.connection.name.clone()
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteProject {
    pub id: String,
    pub machine_id: String,
    pub root: String,
    /// Display name chosen in Add Project; defaults to the folder name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
impl RemoteProject {
    pub fn name(&self) -> String {
        if let Some(name) = self.name.as_ref().filter(|n| !n.trim().is_empty()) {
            return name.clone();
        }
        self.root
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.root)
            .into()
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MachineRegistry {
    pub machines: Vec<Machine>,
    pub projects: Vec<RemoteProject>,
}
impl MachineRegistry {
    pub fn path() -> PathBuf {
        dragent_paths::organisation_dir().join("machines.json")
    }
    pub fn load() -> Result<Self, String> {
        let path = Self::path();
        if path.exists() {
            Self::load_at(&path)
        } else {
            Self::update_at(&path, |registry| Ok(registry.clone()))
        }
    }
    pub fn load_at(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Could not read saved machines: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let parent = path.parent().ok_or("Machine directory unavailable")?;
                let legacy = match fs::read(parent.join("ssh-hosts.json")) {
                    Ok(bytes) => serde_json::from_slice::<BTreeMap<String, SshHost>>(&bytes)
                        .map_err(|e| format!("Could not migrate saved SSH connections: {e}"))?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                    Err(e) => return Err(e.to_string()),
                };
                let history: BTreeMap<String, String> =
                    fs::read(parent.join("remote-projects.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                        .unwrap_or_default();
                let mut registry = Self::default();
                for (address, connection) in legacy {
                    let id = uuid::Uuid::new_v4().to_string();
                    if let Some(root) = history.get(&address) {
                        registry.projects.push(RemoteProject {
                            id: uuid::Uuid::new_v4().to_string(),
                            machine_id: id.clone(),
                            root: root.clone(),
                            name: None,
                        });
                    }
                    registry.machines.push(Machine { id, connection });
                }
                // Migration is persisted once, including an empty registry, so old
                // profiles cannot reappear after a machine is explicitly removed.
                registry.write_at(path)?;
                Ok(registry)
            }
            Err(e) => Err(e.to_string()),
        }
    }
    fn write_at(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
            file.write_all(&serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            fs::rename(&tmp, path).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }
    pub fn update_at<T>(
        path: &Path,
        change: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        fs::create_dir_all(path.parent().ok_or("Machine directory unavailable")?)
            .map_err(|e| e.to_string())?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        lock.lock().map_err(|e| e.to_string())?;
        let mut registry = Self::load_at(path)?;
        let value = change(&mut registry)?;
        registry.write_at(path)?;
        Ok(value)
    }
    pub fn save_machine(id: Option<&str>, connection: SshHost) -> Result<Machine, String> {
        Self::update_at(&Self::path(), |registry| {
            registry.upsert_machine(id, connection)
        })
    }
    pub fn upsert_machine(
        &mut self,
        id: Option<&str>,
        connection: SshHost,
    ) -> Result<Machine, String> {
        connection.validate()?;
        if let Some(id) = id {
            let machine = self
                .machines
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or("Machine was removed. Reopen Settings → Machines.")?;
            machine.connection = connection;
            return Ok(machine.clone());
        }
        let machine = Machine {
            id: uuid::Uuid::new_v4().to_string(),
            connection,
        };
        self.machines.push(machine.clone());
        Ok(machine)
    }
    pub fn remove_machine(&mut self, id: &str) -> Result<(), String> {
        if self.projects.iter().any(|p| p.machine_id == id) {
            return Err("Remove this machine’s saved projects first. Project files will remain on the server.".into());
        }
        self.machines.retain(|m| m.id != id);
        Ok(())
    }
    pub fn add_project(&mut self, machine_id: &str, root: String) -> Result<RemoteProject, String> {
        self.add_named_project(machine_id, root, None)
    }
    pub fn add_named_project(
        &mut self,
        machine_id: &str,
        root: String,
        name: Option<String>,
    ) -> Result<RemoteProject, String> {
        if !self.machines.iter().any(|m| m.id == machine_id) {
            return Err("Machine no longer exists".into());
        }
        if let Some(project) = self
            .projects
            .iter()
            .find(|p| p.machine_id == machine_id && p.root == root)
        {
            return Ok(project.clone());
        }
        let project = RemoteProject {
            id: uuid::Uuid::new_v4().to_string(),
            machine_id: machine_id.into(),
            root,
            name: name.filter(|n| !n.trim().is_empty()),
        };
        self.projects.push(project.clone());
        Ok(project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host(address: &str) -> SshHost {
        SshHost {
            host: address.into(),
            name: "Development".into(),
            user: Some("dev".into()),
            port: Some(22),
            identity_file: None,
            proxy_jump: None,
            config_file: None,
            forward_agent: false,
            authentication: Default::default(),
        }
    }
    #[test]
    fn changing_address_preserves_project_identity() {
        let mut registry = MachineRegistry::default();
        let machine = registry
            .upsert_machine(None, host("first.example"))
            .unwrap();
        let project = registry
            .add_project(&machine.id, "/home/dev/app".into())
            .unwrap();
        let edited = registry
            .upsert_machine(Some(&machine.id), host("second.example"))
            .unwrap();
        assert_eq!(edited.id, machine.id);
        assert_eq!(registry.projects[0].machine_id, edited.id);
        assert_eq!(
            registry.add_project(&edited.id, project.root).unwrap().id,
            project.id
        );
        assert_eq!(registry.machines.len(), 1);
    }
    #[test]
    fn machines_can_have_multiple_projects_and_cannot_be_removed_while_used() {
        let mut registry = MachineRegistry::default();
        let a = registry.upsert_machine(None, host("one")).unwrap();
        let b = registry.upsert_machine(None, host("two")).unwrap();
        registry.add_project(&a.id, "/a".into()).unwrap();
        registry.add_project(&a.id, "/b".into()).unwrap();
        registry.add_project(&b.id, "/a".into()).unwrap();
        assert_eq!(registry.projects.len(), 3);
        assert!(registry.remove_machine(&a.id).is_err());
        registry.projects.retain(|p| p.machine_id != a.id);
        registry.remove_machine(&a.id).unwrap();
        assert_eq!(registry.machines[0].id, b.id);
    }
    #[test]
    fn concurrent_windows_keep_each_others_machines() {
        let dir = std::env::temp_dir().join(format!("machine-writers-{}", uuid::Uuid::new_v4()));
        let path = dir.join("machines.json");
        let workers = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    MachineRegistry::update_at(&path, |registry| {
                        registry.upsert_machine(None, host(&format!("server-{n}")))
                    })
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(MachineRegistry::load_at(&path).unwrap().machines.len(), 8);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn migration_is_persistent_and_corruption_is_reported() {
        let dir = std::env::temp_dir().join(format!("machine-registry-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("ssh-hosts.json"),
            serde_json::to_vec(&BTreeMap::from([("dev@one:22", host("one"))])).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.join("remote-projects.json"),
            br#"{"dev@one:22":"/home/dev/app"}"#,
        )
        .unwrap();
        let path = dir.join("machines.json");
        let registry = MachineRegistry::load_at(&path).unwrap();
        assert_eq!(registry.machines.len(), 1);
        assert_eq!(registry.projects[0].machine_id, registry.machines[0].id);
        assert_eq!(
            MachineRegistry::load_at(&path).unwrap().machines[0].id,
            registry.machines[0].id
        );
        MachineRegistry::update_at(&path, |r| {
            r.projects.clear();
            r.machines.clear();
            Ok(())
        })
        .unwrap();
        assert!(MachineRegistry::load_at(&path).unwrap().machines.is_empty());
        fs::write(&path, b"invalid").unwrap();
        assert!(MachineRegistry::load_at(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
