//! Small, UI-agnostic domain types for Dragent's engineering workspace.
//!
//! This crate intentionally contains no filesystem, database, model, or UI
//! code. It is the stable vocabulary shared by persistence, the Rust workspace
//! shell, and the future orchestration event stream.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::ArtifactId;
use crate::DocumentId;
use crate::WorkItemId;
use crate::MAX_PROJECT_DOCUMENTS;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A deliberately conservative bound for in-memory semantic history. Durable
/// persistence owns the complete audit trail; the UI only needs a recent tail.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Source,
    Markdown,
    Configuration,
    Data,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocumentRef {
    pub id: DocumentId,
    /// Always relative to the project root. Absolute paths require a separate
    /// permission record and are never silently embedded in project state.
    pub relative_path: PathBuf,
    pub kind: DocumentKind,
}

impl DocumentRef {
    pub fn from_relative_path(relative_path: PathBuf) -> Option<Self> {
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return None;
        }
        let key = relative_path.to_string_lossy();
        Some(Self {
            id: DocumentId(Uuid::new_v5(&Uuid::NAMESPACE_URL, key.as_bytes())),
            kind: document_kind_for_path(&relative_path),
            relative_path,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocumentEntry {
    pub document: DocumentRef,
    pub is_directory: bool,
    pub byte_size: Option<u64>,
    /// Matched by `.gitignore` / exclude rules. Listed but rendered dimmed;
    /// a future panel toggle can hide these.
    #[serde(default)]
    pub is_ignored: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectDocumentIndex {
    entries: Vec<DocumentEntry>,
    generation: u64,
}

impl ProjectDocumentIndex {
    pub fn replace(&mut self, entries: Vec<DocumentEntry>) {
        // Filesystem scans can overlap: the initial recursive snapshot may
        // finish before an expanded-directory scan and both can report the
        // same path. Keep the newest entry for each relative path so merge
        // timing can never produce duplicate explorer rows.
        let mut unique = HashMap::with_capacity(entries.len());
        for entry in entries {
            unique.insert(entry.document.relative_path.clone(), entry);
        }
        // Decorate once before sorting. Building lowercase component keys in
        // the comparator would repeatedly allocate O(n log n) times on large
        // repositories.
        let mut keyed = unique
            .into_values()
            .map(|entry| (DocumentSortKey::new(&entry), entry))
            .collect::<Vec<_>>();
        keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
        keyed.truncate(MAX_PROJECT_DOCUMENTS);
        self.entries = keyed.into_iter().map(|(_, entry)| entry).collect();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Add entries discovered for an expanded directory without rescanning
    /// the rest of the project.
    pub fn merge(&mut self, entries: Vec<DocumentEntry>) {
        let mut merged = std::mem::take(&mut self.entries);
        merged.extend(entries);
        self.replace(merged);
    }

    pub fn entries(&self) -> &[DocumentEntry] {
        &self.entries
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DocumentSortKey {
    components: Vec<(String, String)>,
    final_is_directory: bool,
}

impl DocumentSortKey {
    fn new(entry: &DocumentEntry) -> Self {
        Self {
            components: entry
                .document
                .relative_path
                .components()
                .map(|component| {
                    let original = component.as_os_str().to_string_lossy().into_owned();
                    (original.to_lowercase(), original)
                })
                .collect(),
            final_is_directory: entry.is_directory,
        }
    }

    fn component_is_directory(&self, index: usize) -> bool {
        index + 1 < self.components.len()
            || (index + 1 == self.components.len() && self.final_is_directory)
    }
}

impl Ord for DocumentSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        for index in 0..self.components.len().min(other.components.len()) {
            let a = &self.components[index];
            let b = &other.components[index];
            if a == b {
                continue;
            }
            let directory_order = other
                .component_is_directory(index)
                .cmp(&self.component_is_directory(index));
            if directory_order != Ordering::Equal {
                return directory_order;
            }
            let name_order = a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1));
            if name_order != Ordering::Equal {
                return name_order;
            }
        }
        self.components.len().cmp(&other.components.len())
    }
}

impl PartialOrd for DocumentSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn document_kind_for_path(path: &std::path::Path) -> DocumentKind {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "md" | "mdx" | "markdown" => DocumentKind::Markdown,
        "json" | "yaml" | "yml" | "toml" | "xml" | "env" => DocumentKind::Configuration,
        "csv" | "tsv" | "sql" => DocumentKind::Data,
        "rs" | "go" | "ts" | "tsx" | "js" | "jsx" | "py" | "java" | "kt" | "swift" | "c" | "h"
        | "cpp" | "hpp" | "css" | "scss" | "html" => DocumentKind::Source,
        _ => DocumentKind::Unknown,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    ProductBrief,
    Requirements,
    Decision,
    TechnicalPlan,
    TestPlan,
    Review,
    Design,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArtifactRef {
    pub id: ArtifactId,
    pub work_item_id: WorkItemId,
    pub relative_path: PathBuf,
    pub kind: ArtifactKind,
}
