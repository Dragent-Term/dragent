use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ArtifactId, DocumentId, PlanTaskId, ProjectId, RoleDefinitionId, SurfaceId, ThreadId,
    WorkBriefId, WorkItemId,
};

const SURFACE_ID_NAMESPACE: Uuid = Uuid::from_u128(0x7a77_b08e_25d5_4d2e_a5e4_fec9_f384_12d4);

impl SurfaceId {
    /// Preserve the durable UUID already assigned to an existing terminal pane.
    /// This is the compatibility bridge used when the Develop layout migrates.
    pub fn from_terminal_pane(pane_id: Uuid) -> Self {
        Self(pane_id)
    }

    /// Produce a stable identity for application/project singleton surfaces.
    /// Persisted identities remain preferred for user-created surface instances.
    pub fn stable(key: &str) -> Self {
        Self(Uuid::new_v5(&SURFACE_ID_NAMESPACE, key.as_bytes()))
    }
}

/// The semantic purpose of a workspace surface. Placement, visibility, and
/// dimensions intentionally do not belong here; those are layout concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    PrincipalConversation,
    WorkBrief,
    OrganizationMap,
    RoleDetail,
    Editor,
    Diff,
    Artifact,
    Tests,
    Terminal,
    Browser,
    Activity,
    Attention,
    Search,
    SourceControl,
    Settings,
    Custom,
}

impl SurfaceKind {
    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::PrincipalConversation => "principal_conversation",
            Self::WorkBrief => "work_brief",
            Self::OrganizationMap => "organization_map",
            Self::RoleDetail => "role_detail",
            Self::Editor => "editor",
            Self::Diff => "diff",
            Self::Artifact => "artifact",
            Self::Tests => "tests",
            Self::Terminal => "terminal",
            Self::Browser => "browser",
            Self::Activity => "activity",
            Self::Attention => "attention",
            Self::Search => "search",
            Self::SourceControl => "source_control",
            Self::Settings => "settings",
            Self::Custom => "custom",
        }
    }
}

/// Scope controls which project/work item may refer to a surface. It does not
/// imply synchronization; layouts remain local client state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceScope {
    Application,
    Project {
        project_id: ProjectId,
    },
    WorkItem {
        project_id: ProjectId,
        work_item_id: WorkItemId,
    },
}

impl SurfaceScope {
    pub fn project_id(self) -> Option<ProjectId> {
        match self {
            Self::Application => None,
            Self::Project { project_id } | Self::WorkItem { project_id, .. } => Some(project_id),
        }
    }

    pub fn work_item_id(self) -> Option<WorkItemId> {
        match self {
            Self::WorkItem { work_item_id, .. } => Some(work_item_id),
            Self::Application | Self::Project { .. } => None,
        }
    }
}

/// The domain object or durable runtime identity hosted by a surface. This key
/// lets the registry reuse an existing surface without inspecting UI handles.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceBinding {
    Singleton,
    TerminalPane {
        pane_id: Uuid,
    },
    BrowserPane {
        pane_id: Uuid,
    },
    Document {
        document_id: DocumentId,
    },
    Diff {
        document_id: DocumentId,
        comparison: String,
    },
    Thread {
        thread_id: ThreadId,
    },
    WorkBrief {
        brief_id: WorkBriefId,
    },
    Role {
        role_id: RoleDefinitionId,
    },
    Artifact {
        artifact_id: ArtifactId,
    },
    Task {
        task_id: PlanTaskId,
    },
    Path {
        relative_path: PathBuf,
    },
    Custom {
        namespace: String,
        key: String,
    },
}

/// Logical identity used for surface reuse. `SurfaceId` identifies one runtime
/// surface; this key identifies what that surface presents.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SurfaceKey {
    pub kind: SurfaceKind,
    pub scope: SurfaceScope,
    pub binding: SurfaceBinding,
}

impl SurfaceKey {
    pub fn new(kind: SurfaceKind, scope: SurfaceScope, binding: SurfaceBinding) -> Self {
        Self {
            kind,
            scope,
            binding,
        }
    }

    pub fn terminal(project_id: ProjectId, pane_id: Uuid) -> Self {
        Self::new(
            SurfaceKind::Terminal,
            SurfaceScope::Project { project_id },
            SurfaceBinding::TerminalPane { pane_id },
        )
    }

    pub fn application_singleton(kind: SurfaceKind) -> Self {
        Self::new(kind, SurfaceScope::Application, SurfaceBinding::Singleton)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SurfaceDescriptor {
    pub id: SurfaceId,
    pub key: SurfaceKey,
    pub created_at_unix_ms: i64,
}

impl SurfaceDescriptor {
    pub fn new(key: SurfaceKey, created_at_unix_ms: i64) -> Self {
        Self {
            id: SurfaceId::new(),
            key,
            created_at_unix_ms,
        }
    }

    pub fn with_id(id: SurfaceId, key: SurfaceKey, created_at_unix_ms: i64) -> Self {
        Self {
            id,
            key,
            created_at_unix_ms,
        }
    }

    pub fn migrated_terminal(project_id: ProjectId, pane_id: Uuid) -> Self {
        Self::with_id(
            SurfaceId::from_terminal_pane(pane_id),
            SurfaceKey::terminal(project_id, pane_id),
            0,
        )
    }

    pub fn application_singleton(kind: SurfaceKind) -> Self {
        let stable_key = format!("application:{}", kind.stable_name());
        Self::with_id(
            SurfaceId::stable(&stable_key),
            SurfaceKey::application_singleton(kind),
            0,
        )
    }

    pub fn project_singleton(project_id: ProjectId, kind: SurfaceKind) -> Self {
        let stable_key = format!("project:{}:{}", project_id.0, kind.stable_name());
        Self::with_id(
            SurfaceId::stable(&stable_key),
            SurfaceKey::new(
                kind,
                SurfaceScope::Project { project_id },
                SurfaceBinding::Singleton,
            ),
            0,
        )
    }

    pub fn work_item_singleton(
        project_id: ProjectId,
        work_item_id: WorkItemId,
        kind: SurfaceKind,
    ) -> Self {
        let stable_key = format!(
            "project:{}:work:{}:{}",
            project_id.0,
            work_item_id.0,
            kind.stable_name()
        );
        Self::with_id(
            SurfaceId::stable(&stable_key),
            SurfaceKey::new(
                kind,
                SurfaceScope::WorkItem {
                    project_id,
                    work_item_id,
                },
                SurfaceBinding::Singleton,
            ),
            0,
        )
    }

    pub fn validate(&self) -> Result<(), SurfaceRegistryError> {
        if self.id.0.is_nil() {
            return Err(SurfaceRegistryError::NilSurfaceId);
        }
        match &self.key.binding {
            SurfaceBinding::TerminalPane { pane_id } if pane_id.is_nil() => Err(
                SurfaceRegistryError::InvalidBinding("terminal pane id cannot be nil".to_string()),
            ),
            SurfaceBinding::BrowserPane { pane_id } if pane_id.is_nil() => Err(
                SurfaceRegistryError::InvalidBinding("browser pane id cannot be nil".to_string()),
            ),
            SurfaceBinding::Diff { comparison, .. } if comparison.trim().is_empty() => Err(
                SurfaceRegistryError::InvalidBinding("diff comparison cannot be empty".to_string()),
            ),
            SurfaceBinding::Path { relative_path }
                if relative_path.as_os_str().is_empty() || relative_path.is_absolute() =>
            {
                Err(SurfaceRegistryError::InvalidBinding(
                    "surface path must be a non-empty project-relative path".to_string(),
                ))
            }
            SurfaceBinding::Custom { namespace, key }
                if namespace.trim().is_empty() || key.trim().is_empty() =>
            {
                Err(SurfaceRegistryError::InvalidBinding(
                    "custom surface namespace and key cannot be empty".to_string(),
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceRegistryError {
    NilSurfaceId,
    InvalidBinding(String),
    DuplicateId(SurfaceId),
    DuplicateKey {
        key: SurfaceKey,
        existing_id: SurfaceId,
    },
}

impl fmt::Display for SurfaceRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NilSurfaceId => write!(formatter, "surface id cannot be nil"),
            Self::InvalidBinding(message) => formatter.write_str(message),
            Self::DuplicateId(id) => write!(formatter, "surface id {:?} is already registered", id),
            Self::DuplicateKey { key, existing_id } => write!(
                formatter,
                "surface key {:?} is already registered as {:?}",
                key, existing_id
            ),
        }
    }
}

impl std::error::Error for SurfaceRegistryError {}

/// Owns live surface values independently from any layout tree. A layout stores
/// `SurfaceId`s; hiding, moving, or switching layouts therefore does not drop
/// the underlying editor, terminal, agent conversation, or other live value.
#[derive(Debug)]
pub struct SurfaceRegistry<T> {
    entries: HashMap<SurfaceId, SurfaceEntry<T>>,
    ids_by_key: HashMap<SurfaceKey, SurfaceId>,
}

#[derive(Debug)]
struct SurfaceEntry<T> {
    descriptor: SurfaceDescriptor,
    value: T,
}

impl<T> Default for SurfaceRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> SurfaceRegistry<T> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            ids_by_key: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn register(
        &mut self,
        descriptor: SurfaceDescriptor,
        value: T,
    ) -> Result<SurfaceId, SurfaceRegistryError> {
        descriptor.validate()?;
        if self.entries.contains_key(&descriptor.id) {
            return Err(SurfaceRegistryError::DuplicateId(descriptor.id));
        }
        if let Some(existing_id) = self.ids_by_key.get(&descriptor.key) {
            return Err(SurfaceRegistryError::DuplicateKey {
                key: descriptor.key,
                existing_id: *existing_id,
            });
        }

        let id = descriptor.id;
        self.ids_by_key.insert(descriptor.key.clone(), id);
        self.entries.insert(id, SurfaceEntry { descriptor, value });
        Ok(id)
    }

    /// Reuse the live value already registered for `key`, creating it only when
    /// absent. The factory receives the allocated stable runtime identity.
    pub fn get_or_insert_with(
        &mut self,
        key: SurfaceKey,
        created_at_unix_ms: i64,
        create: impl FnOnce(SurfaceId) -> T,
    ) -> Result<SurfaceId, SurfaceRegistryError> {
        if let Some(id) = self.ids_by_key.get(&key) {
            return Ok(*id);
        }
        let descriptor = SurfaceDescriptor::new(key, created_at_unix_ms);
        descriptor.validate()?;
        let id = descriptor.id;
        let value = create(id);
        self.register(descriptor, value)
    }

    pub fn id_for_key(&self, key: &SurfaceKey) -> Option<SurfaceId> {
        self.ids_by_key.get(key).copied()
    }

    pub fn descriptor(&self, id: SurfaceId) -> Option<&SurfaceDescriptor> {
        self.entries.get(&id).map(|entry| &entry.descriptor)
    }

    pub fn get(&self, id: SurfaceId) -> Option<&T> {
        self.entries.get(&id).map(|entry| &entry.value)
    }

    pub fn get_mut(&mut self, id: SurfaceId) -> Option<&mut T> {
        self.entries.get_mut(&id).map(|entry| &mut entry.value)
    }

    pub fn remove(&mut self, id: SurfaceId) -> Option<T> {
        let entry = self.entries.remove(&id)?;
        self.ids_by_key.remove(&entry.descriptor.key);
        Some(entry.value)
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &SurfaceDescriptor> {
        self.entries.values().map(|entry| &entry.descriptor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    fn project_id() -> ProjectId {
        Project::new(PathBuf::from("/tmp/dragent-surface-test"), 0).id
    }

    #[test]
    fn terminal_migration_preserves_the_existing_pane_uuid() {
        let pane_id = Uuid::new_v4();
        let descriptor = SurfaceDescriptor::migrated_terminal(project_id(), pane_id);

        assert_eq!(descriptor.id, SurfaceId(pane_id));
        assert_eq!(
            descriptor.key.binding,
            SurfaceBinding::TerminalPane { pane_id }
        );
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn layout_removal_does_not_remove_a_registered_surface() {
        let mut registry = SurfaceRegistry::new();
        let descriptor = SurfaceDescriptor::migrated_terminal(project_id(), Uuid::new_v4());
        let id = registry
            .register(descriptor, String::from("live terminal"))
            .unwrap();

        // A layout would only discard its SurfaceId reference. The registry
        // continues owning the runtime value until an explicit close removes it.
        let mut layout = vec![id];
        layout.clear();

        assert_eq!(registry.get(id).map(String::as_str), Some("live terminal"));
    }

    #[test]
    fn logical_key_reuses_the_existing_runtime_surface() {
        let mut registry = SurfaceRegistry::new();
        let key = SurfaceKey::application_singleton(SurfaceKind::Settings);
        let first = registry
            .get_or_insert_with(key.clone(), 1, |_| String::from("settings"))
            .unwrap();
        let second = registry
            .get_or_insert_with(key, 2, |_| panic!("must not recreate the surface"))
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn duplicate_ids_and_keys_are_rejected() {
        let mut registry = SurfaceRegistry::new();
        let descriptor = SurfaceDescriptor::migrated_terminal(project_id(), Uuid::new_v4());
        let id = descriptor.id;
        let key = descriptor.key.clone();
        registry.register(descriptor, 1).unwrap();

        let duplicate_id = SurfaceDescriptor::with_id(
            id,
            SurfaceKey::application_singleton(SurfaceKind::Settings),
            0,
        );
        assert!(matches!(
            registry.register(duplicate_id, 2),
            Err(SurfaceRegistryError::DuplicateId(found)) if found == id
        ));

        let duplicate_key = SurfaceDescriptor::new(key.clone(), 0);
        assert!(matches!(
            registry.register(duplicate_key, 3),
            Err(SurfaceRegistryError::DuplicateKey { key: found, existing_id })
                if found == key && existing_id == id
        ));
    }

    #[test]
    fn removal_releases_both_identity_indexes() {
        let mut registry = SurfaceRegistry::new();
        let key = SurfaceKey::application_singleton(SurfaceKind::Settings);
        let id = registry
            .get_or_insert_with(key.clone(), 0, |_| String::from("settings"))
            .unwrap();

        assert_eq!(registry.remove(id).as_deref(), Some("settings"));
        assert_eq!(registry.id_for_key(&key), None);
        assert!(registry.is_empty());
    }

    #[test]
    fn descriptors_are_serializable_without_runtime_values() {
        let descriptor = SurfaceDescriptor::application_singleton(SurfaceKind::Settings);
        let encoded = serde_json::to_string(&descriptor).unwrap();
        let decoded: SurfaceDescriptor = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn project_singletons_are_stable_and_project_scoped() {
        let project_id = project_id();
        let first = SurfaceDescriptor::project_singleton(project_id, SurfaceKind::Editor);
        let second = SurfaceDescriptor::project_singleton(project_id, SurfaceKind::Editor);

        assert_eq!(first.id, second.id);
        assert_eq!(first.key.scope, SurfaceScope::Project { project_id });
    }

    #[test]
    fn work_item_singletons_are_stable_and_work_scoped() {
        let project = ProjectId::new();
        let work = WorkItemId::new();
        let first = SurfaceDescriptor::work_item_singleton(
            project,
            work,
            SurfaceKind::PrincipalConversation,
        );
        let second = SurfaceDescriptor::work_item_singleton(
            project,
            work,
            SurfaceKind::PrincipalConversation,
        );

        assert_eq!(first, second);
        assert_eq!(first.key.scope.work_item_id(), Some(work));
    }
}
