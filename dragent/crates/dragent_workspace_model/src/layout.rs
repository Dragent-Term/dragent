use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{LayoutInstanceId, LayoutTemplateId, ProjectId, SurfaceId, SurfaceKind, WorkItemId};

const LAYOUT_TEMPLATE_NAMESPACE: Uuid = Uuid::from_u128(0x3075_f455_9dce_4d38_8e5d_403b_1d70_5d7e);
const LAYOUT_INSTANCE_NAMESPACE: Uuid = Uuid::from_u128(0x8975_87bd_652f_463e_954c_5a79_2036_a9f1);
pub const MAX_LAYOUT_FOCUS_HISTORY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInLayout {
    Shape,
    Coordinate,
    Develop,
    Review,
    Focus,
}

impl BuiltInLayout {
    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::Shape => "shape",
            Self::Coordinate => "coordinate",
            Self::Develop => "develop",
            Self::Review => "review",
            Self::Focus => "focus",
        }
    }

    pub fn template_id(self) -> LayoutTemplateId {
        LayoutTemplateId(Uuid::new_v5(
            &LAYOUT_TEMPLATE_NAMESPACE,
            self.stable_name().as_bytes(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LayoutTemplate {
    pub id: LayoutTemplateId,
    pub built_in: Option<BuiltInLayout>,
    pub name: String,
    pub description: String,
    pub default_tree: LayoutTemplateNode,
    pub responsive_rules: Vec<ResponsiveLayoutRule>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutTemplateNode {
    Surface {
        kind: SurfaceKind,
    },
    Tabs {
        surfaces: Vec<SurfaceKind>,
        active: SurfaceKind,
    },
    Split {
        axis: LayoutAxis,
        children: Vec<LayoutTemplateNode>,
    },
    PreserveDevelopWorkspace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResponsiveLayoutRule {
    pub maximum_width: f32,
    pub action: ResponsiveLayoutAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResponsiveLayoutAction {
    CollapseSecondaryRail,
    StackPrimarySurfaces,
    HideSupportingPanels,
}

impl LayoutTemplate {
    pub fn built_in(kind: BuiltInLayout) -> Self {
        let (name, description, default_tree) = match kind {
            BuiltInLayout::Shape => (
                "Shape",
                "Clarify the outcome, decisions, plan, and proposed team.",
                LayoutTemplateNode::Split {
                    axis: LayoutAxis::Horizontal,
                    children: vec![
                        LayoutTemplateNode::Surface {
                            kind: SurfaceKind::PrincipalConversation,
                        },
                        LayoutTemplateNode::Tabs {
                            surfaces: vec![SurfaceKind::WorkBrief, SurfaceKind::OrganizationMap],
                            active: SurfaceKind::WorkBrief,
                        },
                    ],
                },
            ),
            BuiltInLayout::Coordinate => (
                "Coordinate",
                "Supervise active agents, dependencies, attention, and evidence.",
                LayoutTemplateNode::Split {
                    axis: LayoutAxis::Horizontal,
                    children: vec![
                        LayoutTemplateNode::Surface {
                            kind: SurfaceKind::OrganizationMap,
                        },
                        LayoutTemplateNode::Tabs {
                            surfaces: vec![
                                SurfaceKind::RoleDetail,
                                SurfaceKind::Attention,
                                SurfaceKind::Activity,
                            ],
                            active: SurfaceKind::RoleDetail,
                        },
                    ],
                },
            ),
            BuiltInLayout::Develop => (
                "Develop",
                "Preserve the current editor, terminal, splits, and sidebars.",
                LayoutTemplateNode::PreserveDevelopWorkspace,
            ),
            BuiltInLayout::Review => (
                "Review",
                "Review changes, requirements, findings, tests, and evidence.",
                LayoutTemplateNode::Split {
                    axis: LayoutAxis::Horizontal,
                    children: vec![
                        LayoutTemplateNode::Surface {
                            kind: SurfaceKind::Diff,
                        },
                        LayoutTemplateNode::Tabs {
                            surfaces: vec![
                                SurfaceKind::WorkBrief,
                                SurfaceKind::Tests,
                                SurfaceKind::Artifact,
                            ],
                            active: SurfaceKind::WorkBrief,
                        },
                    ],
                },
            ),
            BuiltInLayout::Focus => (
                "Focus",
                "Temporarily maximize the selected live surface.",
                LayoutTemplateNode::Surface {
                    kind: SurfaceKind::Editor,
                },
            ),
        };

        Self {
            id: kind.template_id(),
            built_in: Some(kind),
            name: name.to_string(),
            description: description.to_string(),
            default_tree,
            responsive_rules: vec![ResponsiveLayoutRule {
                maximum_width: 900.0,
                action: ResponsiveLayoutAction::CollapseSecondaryRail,
            }],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutScope {
    Project {
        project_id: ProjectId,
    },
    WorkItem {
        project_id: ProjectId,
        work_item_id: WorkItemId,
    },
}

impl LayoutScope {
    fn stable_key(self) -> String {
        match self {
            Self::Project { project_id } => format!("project:{}", project_id.0),
            Self::WorkItem {
                project_id,
                work_item_id,
            } => format!("project:{}/work:{}", project_id.0, work_item_id.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutAxis {
    Horizontal,
    Vertical,
}

/// A placement tree references live surfaces but never contains their runtime
/// values. A surface can therefore outlive any one layout instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceLayoutNode {
    Stack {
        surfaces: Vec<SurfaceId>,
        active_surface_id: SurfaceId,
    },
    Split {
        axis: LayoutAxis,
        children: Vec<SurfaceLayoutNode>,
        /// Preferred pixel size for every child except the final flexible one.
        preferred_sizes: Vec<f32>,
    },
}

impl SurfaceLayoutNode {
    pub fn single(surface_id: SurfaceId) -> Self {
        Self::Stack {
            surfaces: vec![surface_id],
            active_surface_id: surface_id,
        }
    }

    fn collect_surface_ids(&self, target: &mut Vec<SurfaceId>) {
        match self {
            Self::Stack { surfaces, .. } => target.extend(surfaces.iter().copied()),
            Self::Split { children, .. } => {
                for child in children {
                    child.collect_surface_ids(target);
                }
            }
        }
    }

    fn validate(&self) -> Result<(), LayoutValidationError> {
        match self {
            Self::Stack {
                surfaces,
                active_surface_id,
            } => {
                if surfaces.is_empty() {
                    return Err(LayoutValidationError::EmptyStack);
                }
                if !surfaces.contains(active_surface_id) {
                    return Err(LayoutValidationError::ActiveSurfaceOutsideStack(
                        *active_surface_id,
                    ));
                }
                let unique = surfaces.iter().copied().collect::<HashSet<_>>();
                if unique.len() != surfaces.len() {
                    return Err(LayoutValidationError::DuplicateSurfacePlacement);
                }
            }
            Self::Split {
                children,
                preferred_sizes,
                ..
            } => {
                if children.len() < 2 {
                    return Err(LayoutValidationError::SplitNeedsMultipleChildren);
                }
                if preferred_sizes.len() != children.len() - 1 {
                    return Err(LayoutValidationError::InvalidSplitSizes);
                }
                if preferred_sizes
                    .iter()
                    .any(|size| !size.is_finite() || *size <= 0.0)
                {
                    return Err(LayoutValidationError::InvalidSplitSizes);
                }
                for child in children {
                    child.validate()?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LayoutPanelState {
    pub visible: bool,
    pub size: Option<f32>,
    pub maximized: bool,
    pub active_surface_id: Option<SurfaceId>,
}

impl LayoutPanelState {
    fn validate(&self) -> Result<(), LayoutValidationError> {
        if self
            .size
            .is_some_and(|size| !size.is_finite() || size <= 0.0)
        {
            return Err(LayoutValidationError::InvalidPanelSize);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LayoutChromeState {
    pub left: LayoutPanelState,
    pub right: LayoutPanelState,
    pub bottom: LayoutPanelState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LayoutInstance {
    pub id: LayoutInstanceId,
    pub template_id: LayoutTemplateId,
    pub built_in: Option<BuiltInLayout>,
    pub scope: LayoutScope,
    pub name: String,
    pub root: SurfaceLayoutNode,
    /// Surfaces known to this layout but not necessarily visible in `root`.
    pub available_surfaces: Vec<SurfaceId>,
    pub chrome: LayoutChromeState,
    pub active_surface_id: Option<SurfaceId>,
    pub focus_history: Vec<SurfaceId>,
    pub customized: bool,
    pub revision: u64,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

impl LayoutInstance {
    pub fn stable_built_in_id(scope: LayoutScope, kind: BuiltInLayout) -> LayoutInstanceId {
        let key = format!("{}:instance:{}", scope.stable_key(), kind.stable_name());
        LayoutInstanceId(Uuid::new_v5(&LAYOUT_INSTANCE_NAMESPACE, key.as_bytes()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn migrate_develop(
        scope: LayoutScope,
        root: SurfaceLayoutNode,
        available_surfaces: Vec<SurfaceId>,
        chrome: LayoutChromeState,
        active_surface_id: Option<SurfaceId>,
        now_unix_ms: i64,
    ) -> Result<Self, LayoutValidationError> {
        Self::new_built_in(
            scope,
            BuiltInLayout::Develop,
            root,
            available_surfaces,
            chrome,
            active_surface_id,
            true,
            now_unix_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_built_in(
        scope: LayoutScope,
        kind: BuiltInLayout,
        root: SurfaceLayoutNode,
        available_surfaces: Vec<SurfaceId>,
        chrome: LayoutChromeState,
        active_surface_id: Option<SurfaceId>,
        customized: bool,
        now_unix_ms: i64,
    ) -> Result<Self, LayoutValidationError> {
        let mut focus_history = Vec::new();
        if let Some(active) = active_surface_id {
            focus_history.push(active);
        }
        let instance = Self {
            id: Self::stable_built_in_id(scope, kind),
            template_id: kind.template_id(),
            built_in: Some(kind),
            scope,
            name: LayoutTemplate::built_in(kind).name,
            root,
            available_surfaces,
            chrome,
            active_surface_id,
            focus_history,
            customized,
            revision: 1,
            created_at_unix_ms: now_unix_ms,
            updated_at_unix_ms: now_unix_ms,
        };
        instance.validate()?;
        Ok(instance)
    }

    pub fn validate(&self) -> Result<(), LayoutValidationError> {
        if self.id.0.is_nil() {
            return Err(LayoutValidationError::NilInstanceId);
        }
        if self.template_id.0.is_nil() {
            return Err(LayoutValidationError::NilTemplateId);
        }
        if self.name.trim().is_empty() {
            return Err(LayoutValidationError::EmptyName);
        }
        if self.revision == 0 {
            return Err(LayoutValidationError::ZeroRevision);
        }
        self.root.validate()?;
        self.chrome.left.validate()?;
        self.chrome.right.validate()?;
        self.chrome.bottom.validate()?;

        let available = self
            .available_surfaces
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if available.len() != self.available_surfaces.len() {
            return Err(LayoutValidationError::DuplicateAvailableSurface);
        }
        let mut placed = Vec::new();
        self.root.collect_surface_ids(&mut placed);
        if placed.iter().any(|surface| !available.contains(surface)) {
            return Err(LayoutValidationError::PlacedSurfaceUnavailable);
        }
        let placed_unique = placed.iter().copied().collect::<HashSet<_>>();
        if placed_unique.len() != placed.len() {
            return Err(LayoutValidationError::DuplicateSurfacePlacement);
        }
        if self
            .active_surface_id
            .is_some_and(|surface| !available.contains(&surface))
        {
            return Err(LayoutValidationError::ActiveSurfaceUnavailable);
        }
        if [&self.chrome.left, &self.chrome.right, &self.chrome.bottom]
            .into_iter()
            .filter_map(|panel| panel.active_surface_id)
            .any(|surface| !available.contains(&surface))
        {
            return Err(LayoutValidationError::PanelSurfaceUnavailable);
        }
        if self.focus_history.len() > MAX_LAYOUT_FOCUS_HISTORY
            || self
                .focus_history
                .iter()
                .any(|surface| !available.contains(surface))
        {
            return Err(LayoutValidationError::InvalidFocusHistory);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LayoutCollection {
    pub active_layout_id: Option<LayoutInstanceId>,
    pub instances: Vec<LayoutInstance>,
}

impl LayoutCollection {
    pub fn active(&self) -> Option<&LayoutInstance> {
        let id = self.active_layout_id?;
        self.instances.iter().find(|instance| instance.id == id)
    }

    pub fn instance_for(
        &self,
        scope: LayoutScope,
        built_in: BuiltInLayout,
    ) -> Option<&LayoutInstance> {
        self.instances
            .iter()
            .find(|instance| instance.scope == scope && instance.built_in == Some(built_in))
    }

    pub fn upsert_and_activate(
        &mut self,
        instance: LayoutInstance,
    ) -> Result<(), LayoutValidationError> {
        let id = instance.id;
        self.upsert(instance)?;
        self.active_layout_id = Some(id);
        Ok(())
    }

    pub fn upsert(&mut self, instance: LayoutInstance) -> Result<(), LayoutValidationError> {
        instance.validate()?;
        let id = instance.id;
        if let Some(existing) = self.instances.iter_mut().find(|existing| existing.id == id) {
            *existing = instance;
        } else {
            self.instances.push(instance);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), LayoutValidationError> {
        let ids = self
            .instances
            .iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>();
        if ids.len() != self.instances.len() {
            return Err(LayoutValidationError::DuplicateInstanceId);
        }
        for instance in &self.instances {
            instance.validate()?;
        }
        if self
            .active_layout_id
            .is_some_and(|active| !ids.contains(&active))
        {
            return Err(LayoutValidationError::UnknownActiveLayout);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutValidationError {
    NilInstanceId,
    NilTemplateId,
    EmptyName,
    ZeroRevision,
    EmptyStack,
    ActiveSurfaceOutsideStack(SurfaceId),
    SplitNeedsMultipleChildren,
    InvalidSplitSizes,
    InvalidPanelSize,
    DuplicateSurfacePlacement,
    DuplicateAvailableSurface,
    PlacedSurfaceUnavailable,
    ActiveSurfaceUnavailable,
    PanelSurfaceUnavailable,
    InvalidFocusHistory,
    DuplicateInstanceId,
    UnknownActiveLayout,
}

impl fmt::Display for LayoutValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid workspace layout: {self:?}")
    }
}

impl std::error::Error for LayoutValidationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;
    use std::path::PathBuf;

    fn scope() -> LayoutScope {
        LayoutScope::Project {
            project_id: Project::new(PathBuf::from("/tmp/dragent-layout-test"), 0).id,
        }
    }

    fn surface() -> SurfaceId {
        SurfaceId::new()
    }

    #[test]
    fn built_in_template_and_instance_ids_are_stable() {
        assert_eq!(
            BuiltInLayout::Develop.template_id(),
            BuiltInLayout::Develop.template_id()
        );
        assert_eq!(
            LayoutInstance::stable_built_in_id(scope(), BuiltInLayout::Develop),
            LayoutInstance::stable_built_in_id(scope(), BuiltInLayout::Develop)
        );
    }

    #[test]
    fn develop_migration_preserves_all_available_surfaces() {
        let first = surface();
        let second = surface();
        let root = SurfaceLayoutNode::Split {
            axis: LayoutAxis::Horizontal,
            children: vec![
                SurfaceLayoutNode::single(first),
                SurfaceLayoutNode::single(second),
            ],
            preferred_sizes: vec![420.0],
        };
        let layout = LayoutInstance::migrate_develop(
            scope(),
            root,
            vec![first, second],
            LayoutChromeState::default(),
            Some(second),
            10,
        )
        .unwrap();

        assert_eq!(layout.built_in, Some(BuiltInLayout::Develop));
        assert_eq!(layout.active_surface_id, Some(second));
        assert_eq!(layout.available_surfaces, vec![first, second]);
        assert!(layout.customized);
    }

    #[test]
    fn layout_rejects_unknown_or_duplicate_surface_placements() {
        let first = surface();
        let duplicated = SurfaceLayoutNode::Split {
            axis: LayoutAxis::Horizontal,
            children: vec![
                SurfaceLayoutNode::single(first),
                SurfaceLayoutNode::single(first),
            ],
            preferred_sizes: vec![300.0],
        };
        assert_eq!(
            LayoutInstance::migrate_develop(
                scope(),
                duplicated,
                vec![first],
                LayoutChromeState::default(),
                Some(first),
                0,
            ),
            Err(LayoutValidationError::DuplicateSurfacePlacement)
        );

        let placed = surface();
        let available = surface();
        assert_eq!(
            LayoutInstance::migrate_develop(
                scope(),
                SurfaceLayoutNode::single(placed),
                vec![available],
                LayoutChromeState::default(),
                None,
                0,
            ),
            Err(LayoutValidationError::PlacedSurfaceUnavailable)
        );
    }

    #[test]
    fn collection_upserts_without_duplicating_a_layout() {
        let first = surface();
        let mut collection = LayoutCollection::default();
        let mut layout = LayoutInstance::migrate_develop(
            scope(),
            SurfaceLayoutNode::single(first),
            vec![first],
            LayoutChromeState::default(),
            Some(first),
            1,
        )
        .unwrap();
        collection.upsert_and_activate(layout.clone()).unwrap();
        layout.revision = 2;
        layout.updated_at_unix_ms = 2;
        collection.upsert_and_activate(layout).unwrap();

        assert_eq!(collection.instances.len(), 1);
        assert_eq!(collection.active().map(|item| item.revision), Some(2));
        assert!(collection.validate().is_ok());
    }

    #[test]
    fn layout_collection_round_trips_as_persistence_data() {
        let surface = surface();
        let layout = LayoutInstance::migrate_develop(
            scope(),
            SurfaceLayoutNode::single(surface),
            vec![surface],
            LayoutChromeState::default(),
            Some(surface),
            1,
        )
        .unwrap();
        let mut collection = LayoutCollection::default();
        collection.upsert_and_activate(layout).unwrap();

        let json = serde_json::to_string(&collection).unwrap();
        let decoded: LayoutCollection = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, collection);
    }

    #[test]
    fn layout_rejects_a_panel_surface_that_is_not_available() {
        let editor = surface();
        let terminal = surface();
        let layout = LayoutInstance::migrate_develop(
            scope(),
            SurfaceLayoutNode::single(editor),
            vec![editor],
            LayoutChromeState {
                bottom: LayoutPanelState {
                    visible: true,
                    active_surface_id: Some(terminal),
                    ..LayoutPanelState::default()
                },
                ..LayoutChromeState::default()
            },
            Some(editor),
            1,
        );

        assert_eq!(layout, Err(LayoutValidationError::PanelSurfaceUnavailable));
    }
}
