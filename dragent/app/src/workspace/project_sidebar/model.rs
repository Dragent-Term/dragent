//! Project sidebar model — owns the sidebar's live state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use web_time::Instant;

use warpui::{r#async::SpawnedFutureHandle, Entity, ModelContext};

use super::detect::{self, QuickAction, RuntimeInfo};
use crate::workspace::project_documents::ReadOnlyDocument;
use dragent_document::{DocumentBuffer, DocumentDiff};
use dragent_editor::{Diagnostic, DiagnosticSeverity, DocumentDiagnostics, ResolvedDiagnostic};
use dragent_persistence::PersistedWorkingCopy;
use dragent_workspace_model::ProjectDocumentIndex;

/// Lightweight snapshot of a terminal block's recent output for error harvesting.
#[derive(Clone, Debug)]
pub struct BlockSnapshot {
    pub tail_lines: Vec<String>,
}

/// A process error / warning captured from block output.
#[derive(Clone, Debug)]
pub struct ProcessError {
    pub message: String,
    pub level: ErrorLevel,
    /// When this error was recorded.
    pub recorded_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorLevel {
    Error,
    Warn,
}

/// Review check state for one CI check on the linked pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReviewCheckStatus {
    Success,
    Failure,
    Pending,
    Skipped,
    Cancelled,
    #[default]
    Unknown,
}

/// One CI check on the linked pull request.
#[derive(Clone, Debug, Default)]
pub struct ReviewCheck {
    pub name: String,
    pub status: ReviewCheckStatus,
    pub url: Option<String>,
    pub duration_text: Option<String>,
}

/// One review thread (or issue comment) on the linked pull request.
#[derive(Clone, Debug, Default)]
pub struct ReviewComment {
    /// Display id (thread id when resolvable).
    pub id: String,
    pub thread_id: Option<String>,
    pub author: String,
    pub avatar_url: Option<String>,
    pub body: String,
    pub url: Option<String>,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub is_resolved: bool,
    pub created_at: String,
    /// "review" for code threads, "comment" for conversation comments.
    pub kind: String,
}

/// Linked pull request behind the Review tab.
#[derive(Clone, Debug, Default)]
pub struct ReviewPullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// open / closed / merged / draft.
    pub state: String,
    /// approved / changes_requested / pending.
    pub review_decision: String,
    pub checks: Vec<ReviewCheck>,
    pub base_branch: String,
    pub head_branch: String,
}

/// Review tab data: pull request + comments, refreshed on a poll.
#[derive(Clone, Debug)]
pub struct ReviewPrState {
    pub pr: Option<ReviewPullRequest>,
    pub comments: Vec<ReviewComment>,
    pub error: Option<String>,
    pub non_git_folder: bool,
    pub loading_pr: bool,
    pub loading_comments: bool,
}

impl Default for ReviewPrState {
    fn default() -> Self {
        // First paint shows the loading state, never a stale empty screen.
        Self {
            pr: None,
            comments: Vec::new(),
            error: None,
            non_git_folder: false,
            loading_pr: true,
            loading_comments: false,
        }
    }
}

/// Events emitted by [`ProjectSidebarModel`].
#[derive(Clone, Debug)]
pub enum SidebarEvent {
    /// Any state changed — view should repaint.
    Changed,
    /// Derived presentation data changed without another document mutation.
    DocumentPresentationChanged,
}

/// Stable project info, refreshed when tab CWDs change.
#[derive(Clone, Debug)]
pub struct ProjectInfo {
    pub name: String,
    pub path: PathBuf,
    pub git_root: PathBuf,
    pub branch: Option<String>,
    pub status_label: Option<String>,
    pub is_monorepo: bool,
    pub runtimes: Vec<RuntimeInfo>,
    /// One-click commands discovered for this project folder.
    pub quick_actions: Vec<QuickAction>,
}

pub struct ProjectSidebarModel {
    /// One entry per unique git root across open tabs (monorepo → single entry).
    pub projects: Vec<ProjectInfo>,
    /// CWD of the active tab — used to highlight the matching project card.
    pub active_cwd: Option<PathBuf>,
    /// Completed detection is reusable when returning to a workspace. Cards
    /// themselves always describe the current context, including during a scan.
    project_info_cache: HashMap<PathBuf, ProjectInfo>,
    project_refresh_generation: u64,
    /// Errors/warnings harvested from block output.
    pub errors: Vec<ProcessError>,
    /// Bounded metadata for the active project's file tree.
    pub documents: ProjectDocumentIndex,
    pub expanded_directories: HashSet<PathBuf>,
    pub(crate) explorer_selection: Option<(PathBuf, bool)>,
    pub(crate) explorer_creation: Option<(
        PathBuf,
        bool,
        warpui::ViewHandle<crate::workspace::InputEditor>,
    )>,
    pub(crate) explorer_creation_error: Option<String>,
    /// Search results from global project search.
    pub global_search_results: Option<Arc<crate::workspace::search_panel::SearchResults>>,
    /// Directories whose immediate children have already been loaded.
    pub loaded_directories: HashSet<PathBuf>,
    pub preview: Option<ReadOnlyDocument>,
    pub buffer: Option<DocumentBuffer>,
    pub git_file_diff: Option<crate::git_status::diff_head::GitFileDiff>,
    pub git_multi_diff: Option<Arc<crate::git_status::diff_head::GitMultiDiff>>,
    git_multi_diff_cache: Option<Arc<crate::git_status::diff_head::GitMultiDiff>>,
    /// Explicit filesystem root for an immutable review surface. This keeps
    /// agent-candidate worktrees separate from the active project root.
    git_review_root: Option<PathBuf>,
    pub git_diff_loading: bool,
    /// Unsaved text retained per file while the user navigates between files.
    /// These are workspace-session drafts, matching editor buffer behavior.
    /// Draft identity includes its workspace root. Relative paths alone are
    /// not unique across tabs (for example, two projects can both contain
    /// `src/main.rs`).
    document_drafts: HashMap<(PathBuf, PathBuf), PersistedWorkingCopy>,
    workspace_root: PathBuf,
    active_base_fingerprint: Option<super::super::project_documents::FileFingerprint>,
    pub document_diff: DocumentDiff,
    document_diff_task: Option<SpawnedFutureHandle>,
    pub document_notice: Option<(bool, String)>,
    /// Workspace-relative paths currently modified or untracked in Git.
    pub git_changed_paths: HashSet<PathBuf>,
    pub git_changed_line_ranges: HashMap<PathBuf, HashMap<usize, dragent_editor::LineChangeKind>>,
    /// A changed-files query is running; another request only marks it to
    /// run once more when this one answers, so a slow repository holds one
    /// background thread instead of one per request.
    git_status_running: std::cell::Cell<bool>,
    git_status_again: std::cell::Cell<bool>,
    /// Diagnostics are keyed by document and server so a secondary server
    /// (for example ESLint) cannot erase the primary server's findings.
    language_diagnostics: HashMap<(PathBuf, PathBuf, String), Vec<Diagnostic>>,
    /// Dedupe errors by message text.
    seen_errors: HashMap<String, Instant>,
}

impl Entity for ProjectSidebarModel {
    type Event = SidebarEvent;
}

impl ProjectSidebarModel {
    pub fn workspace_root(&self) -> &std::path::Path {
        &self.workspace_root
    }

    pub fn git_review_root(&self) -> Option<&std::path::Path> {
        self.git_review_root.as_deref()
    }

    pub fn set_workspace_root(&mut self, root: PathBuf) {
        self.workspace_root = root;
    }

    pub fn new() -> Self {
        Self {
            projects: Vec::new(),
            active_cwd: None,
            project_info_cache: HashMap::new(),
            project_refresh_generation: 0,
            errors: Vec::new(),
            documents: ProjectDocumentIndex::default(),
            expanded_directories: HashSet::new(),
            explorer_selection: None,
            explorer_creation: None,
            explorer_creation_error: None,
            loaded_directories: HashSet::from([PathBuf::new()]),
            global_search_results: None,
            preview: None,
            buffer: None,
            git_file_diff: None,
            git_multi_diff: None,
            git_multi_diff_cache: None,
            git_review_root: None,
            git_diff_loading: false,
            document_drafts: HashMap::new(),
            workspace_root: PathBuf::new(),
            active_base_fingerprint: None,
            document_diff: DocumentDiff::default(),
            document_diff_task: None,
            document_notice: None,
            git_changed_paths: HashSet::new(),
            git_changed_line_ranges: HashMap::new(),
            git_status_running: Default::default(),
            git_status_again: Default::default(),
            language_diagnostics: HashMap::new(),
            seen_errors: HashMap::new(),
        }
    }

    pub fn set_language_diagnostics(
        &mut self,
        diagnostics: DocumentDiagnostics,
        ctx: &mut ModelContext<Self>,
    ) {
        let key = (
            diagnostics.project_root,
            diagnostics.relative_path,
            diagnostics.server_id,
        );
        if diagnostics.items.is_empty() {
            self.language_diagnostics.remove(&key);
        } else {
            self.language_diagnostics.insert(key, diagnostics.items);
        }
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn active_language_diagnostics(&self) -> Vec<Diagnostic> {
        let Some(path) = self.active_relative_path() else {
            return Vec::new();
        };
        let mut diagnostics = self
            .language_diagnostics
            .iter()
            .filter(|((root, candidate, _), _)| *root == self.workspace_root && *candidate == path)
            .flat_map(|(_, diagnostics)| diagnostics.iter().cloned())
            .collect::<Vec<_>>();
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range.start.line,
                diagnostic.range.start.character,
                diagnostic.severity,
            )
        });
        diagnostics
    }

    pub fn active_diagnostic_lines(&self) -> HashMap<usize, DiagnosticSeverity> {
        let mut lines = HashMap::new();
        for diagnostic in self.active_language_diagnostics() {
            let line = diagnostic.range.start.line as usize + 1;
            lines
                .entry(line)
                .and_modify(|severity| {
                    if diagnostic.severity < *severity {
                        *severity = diagnostic.severity;
                    }
                })
                .or_insert(diagnostic.severity);
        }
        lines
    }

    pub fn active_diagnostic_ranges(&self) -> Vec<ResolvedDiagnostic> {
        let Some(buffer) = self.buffer.as_ref() else {
            return Vec::new();
        };
        self.active_language_diagnostics()
            .into_iter()
            .map(|diagnostic| ResolvedDiagnostic {
                range: dragent_editor::lsp_range_to_char_range(buffer.text(), diagnostic.range),
                diagnostic,
            })
            .filter(|diagnostic| diagnostic.range.start < diagnostic.range.end)
            .collect()
    }

    pub fn set_documents(&mut self, documents: ProjectDocumentIndex, ctx: &mut ModelContext<Self>) {
        self.documents = documents;
        self.loaded_directories.retain(|path| {
            path.as_os_str().is_empty()
                || self
                    .documents
                    .entries()
                    .iter()
                    .any(|entry| entry.is_directory && entry.document.relative_path == *path)
        });
        self.loaded_directories.insert(PathBuf::new());
        self.expanded_directories.retain(|path| {
            self.documents
                .entries()
                .iter()
                .any(|entry| entry.is_directory && entry.document.relative_path == *path)
        });
        ctx.emit(SidebarEvent::Changed);
        self.refresh_git_status(ctx);
    }

    pub fn merge_documents(
        &mut self,
        documents: ProjectDocumentIndex,
        ctx: &mut ModelContext<Self>,
    ) {
        let entries = documents.entries().to_vec();
        self.documents.merge(entries);
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn is_directory_loaded(&self, relative_path: &std::path::Path) -> bool {
        self.loaded_directories.contains(relative_path)
    }

    pub fn loaded_directories(&self) -> Vec<PathBuf> {
        self.loaded_directories
            .union(&self.expanded_directories)
            .cloned()
            .collect()
    }

    pub fn mark_directory_loaded(&mut self, relative_path: PathBuf) {
        self.loaded_directories.insert(relative_path);
    }

    /// Remove only explorer metadata while a different tab/root is being
    /// scanned. The open document buffer is intentionally independent: a file
    /// owned by another tab must survive without its old directory tree being
    /// rendered under the newly selected tab.
    pub fn clear_documents(&mut self, ctx: &mut ModelContext<Self>) {
        self.explorer_selection = None;
        self.explorer_creation = None;
        self.explorer_creation_error = None;
        self.documents = ProjectDocumentIndex::default();
        self.expanded_directories.clear();
        self.loaded_directories.clear();
        self.git_changed_paths.clear();
        self.git_changed_line_ranges.clear();
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn git_path_changed(&self, path: &std::path::Path, is_directory: bool) -> bool {
        self.git_changed_paths
            .iter()
            .any(|changed| changed == path || (is_directory && changed.starts_with(path)))
    }

    pub fn refresh_git_status(&self, ctx: &mut ModelContext<Self>) {
        if self.git_status_running.get() {
            self.git_status_again.set(true);
            return;
        }
        let Some(root) = self
            .active_cwd
            .as_ref()
            .and_then(|cwd| detect::find_git_root(cwd))
        else {
            return;
        };
        self.git_status_running.set(true);
        let generation = self.project_refresh_generation;
        ctx.spawn(
            async move {
                (
                    detect::git_changed_paths(&root),
                    detect::git_changed_line_ranges(&root),
                )
            },
            move |model, (paths, lines), ctx| {
                model.git_status_running.set(false);
                if model.project_refresh_generation == generation {
                    model.git_changed_paths = paths;
                    model.git_changed_line_ranges = lines;
                    ctx.emit(SidebarEvent::Changed);
                }
                if model.git_status_again.replace(false) {
                    model.refresh_git_status(ctx);
                }
            },
        );
    }

    pub fn toggle_directory(&mut self, relative_path: PathBuf, ctx: &mut ModelContext<Self>) {
        if !self.expanded_directories.remove(&relative_path) {
            self.expanded_directories.insert(relative_path);
        }
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn clear_document(&mut self, ctx: &mut ModelContext<Self>) {
        self.stash_active_draft();
        self.global_search_results = None;
        self.preview = None;
        self.buffer = None;
        self.git_file_diff = None;
        self.git_multi_diff = None;
        self.git_multi_diff_cache = None;
        self.git_review_root = None;
        self.git_diff_loading = false;
        self.document_diff = DocumentDiff::default();
        self.document_notice = None;
        ctx.emit(SidebarEvent::Changed);
    }

    /// Start loading a document from `workspace_root` without allowing the
    /// previous root's relative-path draft to leak into it.
    pub fn begin_document_load(&mut self, workspace_root: PathBuf, ctx: &mut ModelContext<Self>) {
        self.stash_active_draft();
        self.global_search_results = None;
        self.workspace_root = workspace_root;
        self.preview = None;
        self.buffer = None;
        self.git_file_diff = None;
        self.git_multi_diff = None;
        self.git_multi_diff_cache = None;
        self.git_review_root = None;
        self.git_diff_loading = false;
        self.document_diff = DocumentDiff::default();
        self.document_notice = None;
        self.active_base_fingerprint = None;
        ctx.emit(SidebarEvent::Changed);
    }

    /// Explicit "Don't Save": remove the active draft and release its buffer
    /// without first stashing it.
    pub fn discard_active_document(&mut self, ctx: &mut ModelContext<Self>) {
        if let Some(buffer) = self.buffer.as_ref() {
            let key = self.draft_key(&buffer.document().relative_path);
            self.document_drafts.remove(&key);
        }
        self.preview = None;
        self.global_search_results = None;
        self.buffer = None;
        self.git_file_diff = None;
        self.git_multi_diff = None;
        self.git_multi_diff_cache = None;
        self.git_review_root = None;
        self.git_diff_loading = false;
        self.document_diff = DocumentDiff::default();
        self.document_notice = None;
        self.active_base_fingerprint = None;
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn is_directory_expanded(&self, relative_path: &std::path::Path) -> bool {
        self.expanded_directories.contains(relative_path)
    }

    pub fn is_document_visible(&self, relative_path: &std::path::Path) -> bool {
        let mut parent = relative_path.parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            if !self.expanded_directories.contains(path) {
                return false;
            }
            parent = path.parent();
        }
        true
    }

    pub fn set_preview(&mut self, mut preview: ReadOnlyDocument, ctx: &mut ModelContext<Self>) {
        self.stash_active_draft();
        self.global_search_results = None;
        self.git_file_diff = None;
        self.git_multi_diff = None;
        self.git_multi_diff_cache = None;
        self.git_review_root = None;
        self.git_diff_loading = false;
        let draft = self
            .document_drafts
            .get(&self.draft_key(&preview.relative_path))
            .cloned();
        let recovered_base =
            draft
                .as_ref()
                .map(|draft| super::super::project_documents::FileFingerprint {
                    content_hash: draft.base_hash,
                    byte_size: draft.base_size,
                    modified_unix_ms: draft.base_modified_unix_ms,
                });
        let recovered_conflict = recovered_base
            .zip(preview.fingerprint)
            .is_some_and(|(base, disk)| base != disk);
        self.active_base_fingerprint = recovered_base.or(preview.fingerprint);
        let mut restored_diff = DocumentDiff::default();
        self.buffer = if preview.error.is_none() && preview.visual_path.is_none() {
            dragent_workspace_model::DocumentRef::from_relative_path(preview.relative_path.clone())
                .and_then(|document| {
                    let text = std::mem::take(&mut preview.text);
                    let mut buffer = DocumentBuffer::new(document, text);
                    if let Some(draft) = draft {
                        buffer.replace_text(draft.draft_text);
                        restored_diff = buffer.diff();
                    }
                    Some(buffer)
                })
        } else {
            None
        };
        self.document_diff = restored_diff;
        self.document_notice = recovered_conflict.then(|| {
            (
                false,
                "Recovered draft; the file also changed on disk. Review before saving.".to_string(),
            )
        });
        if self.document_notice.is_none() {
            if let Some(capabilities) = self.buffer.as_ref().map(DocumentBuffer::capabilities) {
                if let Some(notice) = capabilities.notice() {
                    self.document_notice = Some((true, notice.to_string()));
                }
            }
        }
        self.preview = Some(preview);
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn set_git_file_diff(
        &mut self,
        diff: crate::git_status::diff_head::GitFileDiff,
        ctx: &mut ModelContext<Self>,
    ) {
        self.stash_active_draft();
        self.global_search_results = None;
        self.buffer = if diff.target == crate::git_status::diff_head::GitDiffTarget::Unstaged
            && !diff.binary
        {
            dragent_workspace_model::DocumentRef::from_relative_path(diff.relative_path.clone())
                .map(|document| DocumentBuffer::new(document, diff.new_text.clone()))
        } else {
            None
        };
        // The worktree text already exists on disk. Git-changed is not the
        // same state as editor-unsaved, so opening a diff must start clean.
        self.document_diff = DocumentDiff::default();
        self.document_notice = None;
        self.active_base_fingerprint = None;
        self.preview = Some(ReadOnlyDocument {
            relative_path: diff.relative_path.clone(),
            text: String::new(),
            visual_path: None,
            error: None,
            fingerprint: None,
        });
        self.git_file_diff = Some(diff);
        self.git_multi_diff = None;
        self.git_review_root = None;
        self.git_diff_loading = false;
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn begin_git_diff(&mut self, relative_path: PathBuf, ctx: &mut ModelContext<Self>) {
        self.stash_active_draft();
        self.global_search_results = None;
        self.git_review_root = None;
        // Keep the current review surface mounted until the next structured
        // diff is ready. Clearing it here created a blank/loading-only frame
        // whenever the user moved between changed files.
        self.git_diff_loading = true;
        self.document_notice = None;
        self.preview = Some(ReadOnlyDocument {
            relative_path,
            text: String::new(),
            visual_path: None,
            error: None,
            fingerprint: None,
        });
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn set_global_search_results(
        &mut self,
        workspace_root: PathBuf,
        relative_path: PathBuf,
        results: Arc<crate::workspace::search_panel::SearchResults>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.stash_active_draft();
        self.workspace_root = workspace_root;
        self.buffer = None;
        self.git_file_diff = None;
        self.git_multi_diff = None;
        self.git_multi_diff_cache = None;
        self.git_review_root = None;
        self.git_diff_loading = false;
        self.document_diff = DocumentDiff::default();
        self.document_notice = None;
        self.active_base_fingerprint = None;
        self.preview = Some(ReadOnlyDocument {
            relative_path,
            text: String::new(),
            visual_path: None,
            error: None,
            fingerprint: None,
        });
        self.global_search_results = Some(results);
        ctx.emit(SidebarEvent::Changed);
    }

    /// Mount the aggregate review immediately from the changed paths already
    /// known by the status model. The background snapshot replaces these
    /// lightweight rows with complete staged/unstaged diffs when ready.
    pub fn begin_git_multi_diff(&mut self, relative_path: PathBuf, ctx: &mut ModelContext<Self>) {
        self.begin_git_diff(relative_path, ctx);
        let mut paths = self.git_changed_paths.iter().cloned().collect::<Vec<_>>();
        paths.sort();
        self.git_multi_diff = Some(Arc::new(crate::git_status::diff_head::GitMultiDiff {
            title: "Uncommitted Changes".to_string(),
            subtitle: Some("Loading file details…".to_string()),
            read_only: false,
            files: paths
                .into_iter()
                .map(
                    |relative_path| crate::git_status::diff_head::GitMultiDiffFile {
                        change: crate::git_status::diff_head::GitChangedFile {
                            relative_path,
                            base_relative_path: None,
                            target: crate::git_status::diff_head::GitDiffTarget::Unstaged,
                            status: 'M',
                        },
                        diff: Err("Loading diff…".to_string()),
                    },
                )
                .collect(),
        }));
        ctx.emit(SidebarEvent::Changed);
    }

    /// Mount a loading shell for a structured diff rooted in an isolated
    /// agent checkout. The explorer and active project root remain unchanged.
    pub fn begin_read_only_git_multi_diff(
        &mut self,
        relative_path: PathBuf,
        review_root: PathBuf,
        title: String,
        subtitle: String,
        ctx: &mut ModelContext<Self>,
    ) {
        self.begin_git_diff(relative_path, ctx);
        self.buffer = None;
        self.git_file_diff = None;
        self.git_multi_diff_cache = None;
        self.git_review_root = Some(review_root);
        self.git_multi_diff = Some(Arc::new(crate::git_status::diff_head::GitMultiDiff {
            title,
            subtitle: Some(subtitle),
            read_only: true,
            files: Vec::new(),
        }));
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn set_git_multi_diff(
        &mut self,
        diff: crate::git_status::diff_head::GitMultiDiff,
        ctx: &mut ModelContext<Self>,
    ) {
        // Preview cards do not need an editor buffer. Creating one here also
        // triggered editor synchronization/highlighting before a file was opened.
        self.stash_active_draft();
        self.buffer = None;
        self.document_diff = DocumentDiff::default();
        self.document_notice = None;
        self.active_base_fingerprint = None;
        let diff = Arc::new(diff);
        self.git_multi_diff_cache = Some(Arc::clone(&diff));
        self.git_multi_diff = Some(diff);
        self.git_diff_loading = false;
        // Every card initially renders as a compact preview — exactly like the
        // others. The inline editor mounts only when the user clicks a file
        // (ActivateGitMultiFile), so the first card no longer reserves the
        // tall editor height with blank padding after its content.
        self.git_file_diff = None;
        ctx.emit(SidebarEvent::Changed);
    }

    /// Return a previously loaded file comparison without invoking Git again.
    pub fn cached_git_file_diff(
        &self,
        relative_path: &std::path::Path,
        target: crate::git_status::diff_head::GitDiffTarget,
    ) -> Option<crate::git_status::diff_head::GitFileDiff> {
        self.git_multi_diff_cache.as_ref().and_then(|multi| {
            multi.files.iter().find_map(|file| {
                (file.change.relative_path == relative_path && file.change.target == target)
                    .then(|| file.diff.as_ref().ok())
                    .flatten()
                    .cloned()
            })
        })
    }

    /// Switch the aggregate worktree review to another already-loaded file.
    /// The structured multi-diff owns the current text, so activation is an
    /// in-memory operation and never shells out to Git or blocks rendering.
    pub fn activate_git_multi_file(
        &mut self,
        relative_path: &std::path::Path,
        target: crate::git_status::diff_head::GitDiffTarget,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        if target != crate::git_status::diff_head::GitDiffTarget::Unstaged {
            return false;
        }
        let Some(diff) = self.git_multi_diff.as_ref().and_then(|multi| {
            multi.files.iter().find_map(|file| {
                (file.change.relative_path == relative_path && file.change.target == target)
                    .then(|| file.diff.as_ref().ok())
                    .flatten()
                    .filter(|diff| !diff.binary)
                    .cloned()
            })
        }) else {
            return false;
        };
        self.set_git_multi_active_file_diff(diff, ctx);
        true
    }

    /// Promote one unstaged file inside the aggregate review to the normal
    /// buffer-backed editor while retaining the surrounding multi-file diff.
    pub fn set_git_multi_active_file_diff(
        &mut self,
        diff: crate::git_status::diff_head::GitFileDiff,
        ctx: &mut ModelContext<Self>,
    ) {
        if diff.target != crate::git_status::diff_head::GitDiffTarget::Unstaged || diff.binary {
            return;
        }
        self.stash_active_draft();
        let draft = self
            .document_drafts
            .get(&self.draft_key(&diff.relative_path))
            .cloned();
        let mut restored_diff = DocumentDiff::default();
        self.buffer =
            dragent_workspace_model::DocumentRef::from_relative_path(diff.relative_path.clone())
                .map(|document| {
                    let mut buffer = DocumentBuffer::new(document, diff.new_text.clone());
                    if let Some(draft) = draft {
                        buffer.replace_text(draft.draft_text);
                        restored_diff = buffer.diff();
                    }
                    buffer
                });
        self.document_diff = restored_diff;
        self.document_notice = None;
        self.active_base_fingerprint = None;
        self.git_file_diff = Some(diff);
        self.git_diff_loading = false;
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn replace_document_text(&mut self, text: String, ctx: &mut ModelContext<Self>) -> bool {
        let changed = {
            let Some(buffer) = self.buffer.as_mut() else {
                return false;
            };
            if buffer.replace_text(text) {
                Some((
                    buffer.document().relative_path.clone(),
                    buffer.is_dirty(),
                    buffer.revision(),
                ))
            } else {
                None
            }
        };
        let Some((path, dirty, revision)) = changed else {
            return false;
        };
        self.document_diff = if dirty {
            DocumentDiff::pending()
        } else {
            DocumentDiff::default()
        };
        if !dirty {
            let key = self.draft_key(&path);
            self.document_drafts.remove(&key);
        }
        self.document_notice = None;
        log::debug!(
            "[editor-model] changed path={} revision={} dirty={} bytes={}",
            path.display(),
            revision,
            dirty,
            self.buffer.as_ref().map_or(0, |buffer| buffer.text().len())
        );
        self.schedule_document_diff(ctx);
        ctx.emit(SidebarEvent::Changed);
        true
    }

    /// Refresh the active editor after an external refactor changed its file
    /// on disk. Dirty buffers are intentionally never replaced here.
    pub fn reload_clean_document(
        &mut self,
        preview: &ReadOnlyDocument,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        if preview.error.is_some() || preview.visual_path.is_some() {
            return false;
        }
        let Some(buffer) = self.buffer.as_mut().filter(|buffer| {
            buffer.document().relative_path == preview.relative_path && !buffer.is_dirty()
        }) else {
            return false;
        };
        if !buffer.reload_saved_text(preview.text.clone()) {
            return false;
        }
        self.document_diff_task = None;
        self.document_diff = DocumentDiff::default();
        self.document_notice = None;
        self.active_base_fingerprint = preview.fingerprint;
        if let Some(active_preview) = self
            .preview
            .as_mut()
            .filter(|active| active.relative_path == preview.relative_path)
        {
            active_preview.text = preview.text.clone();
            active_preview.fingerprint = preview.fingerprint;
        }
        let key = self.draft_key(&preview.relative_path);
        self.document_drafts.remove(&key);
        ctx.emit(SidebarEvent::Changed);
        true
    }

    fn schedule_document_diff(&mut self, ctx: &mut ModelContext<Self>) {
        if self.document_diff_task.is_some() {
            return;
        }
        let Some(buffer) = self
            .buffer
            .as_ref()
            .filter(|buffer| buffer.is_dirty() && !buffer.capabilities().large_file)
        else {
            return;
        };
        let path = buffer.document().relative_path.clone();
        let revision = buffer.revision();
        let document_base = buffer.base_text().clone();
        let git_base = self
            .git_file_diff
            .as_ref()
            .filter(|diff| {
                diff.target == crate::git_status::diff_head::GitDiffTarget::Unstaged
                    && diff.relative_path == path
            })
            .map(|diff| std::sync::Arc::<str>::from(diff.old_text.as_str()));
        let current = buffer.text().to_string();
        self.document_diff_task = Some(ctx.spawn(
            async move {
                let document_diff = DocumentDiff::between(&document_base, &current);
                let git_diff = git_base.map(|base| DocumentDiff::between(&base, &current));
                (path, revision, document_diff, git_diff)
            },
            |model, (path, revision, document_diff, git_projection_diff), ctx| {
                model.document_diff_task = None;
                let current_matches = model.buffer.as_ref().is_some_and(|buffer| {
                    buffer.document().relative_path == path && buffer.revision() == revision
                });
                if current_matches {
                    model.document_diff = document_diff;
                    if let (Some(buffer), Some(file_diff)) =
                        (model.buffer.as_ref(), model.git_file_diff.as_mut())
                    {
                        if file_diff.target == crate::git_status::diff_head::GitDiffTarget::Unstaged
                            && file_diff.relative_path == path
                        {
                            file_diff.new_text = buffer.text().to_string();
                            if let Some(updated) = git_projection_diff {
                                file_diff.diff = updated;
                            }
                        }
                    }
                    ctx.emit(SidebarEvent::DocumentPresentationChanged);
                } else {
                    // Coalesce edits made during the diff into one calculation
                    // over the newest immutable snapshot.
                    model.schedule_document_diff(ctx);
                }
            },
        ));
    }

    pub fn document_snapshot(
        &self,
    ) -> Option<(
        PathBuf,
        String,
        Option<super::super::project_documents::FileFingerprint>,
    )> {
        self.buffer.as_ref().map(|buffer| {
            (
                buffer.document().relative_path.clone(),
                buffer.text().to_string(),
                self.active_base_fingerprint,
            )
        })
    }

    /// Cheap identity/revision probe for the language-service coordinator.
    /// Text is cloned separately only when the revision actually changes.
    pub fn language_document_metadata(&self) -> Option<(PathBuf, u64)> {
        self.buffer
            .as_ref()
            .map(|buffer| (buffer.document().relative_path.clone(), buffer.revision()))
    }

    pub fn language_document_text(&self, relative_path: &std::path::Path) -> Option<String> {
        self.buffer
            .as_ref()
            .filter(|buffer| buffer.document().relative_path == relative_path)
            .map(|buffer| buffer.text().to_string())
    }

    pub fn language_document_capabilities(
        &self,
        relative_path: &std::path::Path,
    ) -> Option<dragent_document::DocumentCapabilities> {
        self.buffer
            .as_ref()
            .filter(|buffer| buffer.document().relative_path == relative_path)
            .map(|buffer| buffer.capabilities())
    }

    pub fn configure_recovery(
        &mut self,
        workspace_root: PathBuf,
        copies: Vec<PersistedWorkingCopy>,
        expanded_directories: Vec<PathBuf>,
    ) {
        self.workspace_root = workspace_root.clone();
        self.document_drafts = copies
            .into_iter()
            .map(|copy| {
                (
                    (copy.workspace_root.clone(), copy.relative_path.clone()),
                    copy,
                )
            })
            .collect();
        log::info!(
            "[editor-recovery] configured root={} drafts={} expanded_dirs={}",
            self.workspace_root.display(),
            self.document_drafts.len(),
            self.expanded_directories.len()
        );
        self.expanded_directories = expanded_directories.into_iter().collect();
        // Expanded is UI state; loaded means a directory scan has completed.
        // Startup refresh includes expanded folders without pretending their
        // children have already been fetched. The root is always included in
        // refresh projections, independently of directory expansion.
        self.loaded_directories.clear();
        self.loaded_directories.insert(PathBuf::new());
    }

    pub fn working_copies(&self) -> Vec<PersistedWorkingCopy> {
        let active_path = self.active_relative_path();
        let mut copies = self
            .document_drafts
            .values()
            .filter(|copy| active_path.as_deref() != Some(copy.relative_path.as_path()))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(buffer) = self.buffer.as_ref().filter(|buffer| buffer.is_dirty()) {
            let fingerprint = self.active_base_fingerprint.unwrap_or(
                super::super::project_documents::FileFingerprint {
                    content_hash: 0,
                    byte_size: 0,
                    modified_unix_ms: None,
                },
            );
            copies.push(PersistedWorkingCopy {
                workspace_root: self.workspace_root.clone(),
                relative_path: buffer.document().relative_path.clone(),
                draft_text: buffer.text().to_string(),
                base_hash: fingerprint.content_hash,
                base_size: fingerprint.byte_size,
                base_modified_unix_ms: fingerprint.modified_unix_ms,
                revision: buffer.revision(),
                updated_at_unix_ms: chrono::Utc::now().timestamp_millis(),
            });
        }
        copies
    }

    pub fn active_relative_path(&self) -> Option<PathBuf> {
        self.buffer
            .as_ref()
            .map(|buffer| buffer.document().relative_path.clone())
    }

    /// Dirty state for any open editor tab. The active buffer owns its live
    /// diff; inactive files are represented by their bounded recovery copy.
    pub fn document_is_dirty(
        &self,
        workspace_root: &std::path::Path,
        path: &std::path::Path,
    ) -> bool {
        if self.workspace_root == workspace_root
            && self
                .active_relative_path()
                .as_deref()
                .is_some_and(|active| active == path)
        {
            return !self.document_diff.is_empty();
        }
        self.document_drafts
            .contains_key(&(workspace_root.to_path_buf(), path.to_path_buf()))
    }

    pub fn checkpoint_snapshot(&self) -> Option<(PathBuf, String)> {
        let buffer = self.buffer.as_ref()?;
        if self.document_diff.is_empty() {
            return None;
        }
        if self.document_diff.truncated {
            // Large-file diffs intentionally avoid a whole-document diff on
            // every edit. Saving remains supported; a compact checkpoint is
            // omitted rather than writing a misleading synthetic patch.
            return None;
        }
        let path = buffer.document().relative_path.clone();
        let mut patch = format!("--- a/{}\n+++ b/{}\n", path.display(), path.display());
        for hunk in &self.document_diff.hunks {
            patch.push_str("@@\n");
            for line in &hunk.lines {
                let prefix = match line.kind {
                    dragent_document::DiffLineKind::Context => ' ',
                    dragent_document::DiffLineKind::Added => '+',
                    dragent_document::DiffLineKind::Removed => '-',
                };
                patch.push(prefix);
                patch.push_str(&line.text);
                patch.push('\n');
            }
        }
        Some((path, patch))
    }

    pub fn mark_document_saved(
        &mut self,
        relative_path: &std::path::Path,
        fingerprint: super::super::project_documents::FileFingerprint,
        ctx: &mut ModelContext<Self>,
    ) {
        let key = self.draft_key(relative_path);
        self.document_drafts.remove(&key);
        if let Some(buffer) = self
            .buffer
            .as_mut()
            .filter(|buffer| buffer.document().relative_path == relative_path)
        {
            buffer.mark_saved();
            self.document_diff = DocumentDiff::default();
            self.document_notice = Some((true, "File saved".to_string()));
            self.active_base_fingerprint = Some(fingerprint);
        }
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn set_document_notice(
        &mut self,
        success: bool,
        message: impl Into<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.document_notice = Some((success, message.into()));
        ctx.emit(SidebarEvent::Changed);
    }

    pub fn revert_document(&mut self, ctx: &mut ModelContext<Self>) -> Option<String> {
        let buffer = self.buffer.as_mut()?;
        let path = buffer.document().relative_path.clone();
        buffer.revert();
        let text = buffer.text().to_string();
        let key = self.draft_key(&path);
        self.document_drafts.remove(&key);
        self.document_diff = DocumentDiff::default();
        self.document_notice = Some((true, "Changes reverted".to_string()));
        ctx.emit(SidebarEvent::Changed);
        Some(text)
    }

    fn stash_active_draft(&mut self) {
        let Some(buffer) = self.buffer.as_ref() else {
            return;
        };
        let path = buffer.document().relative_path.clone();
        if buffer.is_dirty() {
            let text = buffer.text().to_string();
            let revision = buffer.revision();
            self.upsert_draft(path, text, revision);
        } else {
            let key = self.draft_key(&path);
            self.document_drafts.remove(&key);
        }
    }

    fn upsert_draft(&mut self, path: PathBuf, text: String, revision: u64) {
        let fingerprint = self.active_base_fingerprint.unwrap_or(
            super::super::project_documents::FileFingerprint {
                content_hash: 0,
                byte_size: 0,
                modified_unix_ms: None,
            },
        );
        self.document_drafts.insert(
            (self.workspace_root.clone(), path.clone()),
            PersistedWorkingCopy {
                workspace_root: self.workspace_root.clone(),
                relative_path: path,
                draft_text: text,
                base_hash: fingerprint.content_hash,
                base_size: fingerprint.byte_size,
                base_modified_unix_ms: fingerprint.modified_unix_ms,
                revision,
                updated_at_unix_ms: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    fn draft_key(&self, path: &std::path::Path) -> (PathBuf, PathBuf) {
        (self.workspace_root.clone(), path.to_path_buf())
    }

    /// Whether the active pane's CWD is inside a git repository.
    pub fn active_pane_in_git(&self) -> bool {
        self.active_cwd
            .as_ref()
            .is_some_and(|cwd| detect::find_git_root(cwd).is_some())
    }

    /// Publish the current path and branch before starting subprocess-based
    /// detection. Background metadata may only update the context that asked
    /// for it; switching A → B → A also invalidates the first A request.
    pub fn refresh_all_cwds(
        &mut self,
        cwds: &[PathBuf],
        active_cwd: Option<PathBuf>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some((generation, to_detect)) = self.update_project_context(cwds, active_cwd) else {
            return;
        };
        ctx.emit(SidebarEvent::Changed);

        for (cwd, root) in to_detect {
            ctx.spawn(
                async move { build_project_info(&cwd, &root) },
                move |model, info, ctx| {
                    if model.apply_project_info(generation, info) {
                        ctx.emit(SidebarEvent::Changed);
                    }
                },
            );
        }
    }

    fn update_project_context(
        &mut self,
        cwds: &[PathBuf],
        active_cwd: Option<PathBuf>,
    ) -> Option<(u64, Vec<(PathBuf, PathBuf)>)> {
        // Only file reads here: runtime probes and git status must not delay
        // publishing the selected workspace's identity.
        let mut roots = Vec::new();
        for cwd in cwds {
            if let Some(git_root) = detect::find_git_root(cwd) {
                if !roots.iter().any(|(root, _)| root == &git_root) {
                    roots.push((git_root, cwd.clone()));
                }
            }
        }
        roots.sort();

        let mut current_roots: Vec<_> = self
            .projects
            .iter()
            .map(|project| (project.git_root.clone(), project.path.clone()))
            .collect();
        current_roots.sort();
        let active_unchanged = !cwds_differ(&active_cwd, &self.active_cwd);
        if roots == current_roots && active_unchanged {
            return None;
        }
        self.project_refresh_generation = self.project_refresh_generation.wrapping_add(1);

        let mut projects = Vec::new();
        let mut to_detect = Vec::new();
        for (root, cwd) in roots {
            let mut info = build_project_identity(&cwd, &root);
            if let Some(cached) = self
                .project_info_cache
                .get(&cwd)
                .filter(|cached| cached.git_root == root)
            {
                info.status_label = cached.status_label.clone();
                info.is_monorepo = cached.is_monorepo;
                info.runtimes = cached.runtimes.clone();
                info.quick_actions = cached.quick_actions.clone();
            } else {
                to_detect.push((cwd, root.clone()));
            }
            projects.push(info);
        }
        projects.sort_by(|a, b| a.name.cmp(&b.name));

        self.projects = projects;
        self.active_cwd = active_cwd;

        if !active_unchanged || self.projects.is_empty() {
            self.errors.clear();
            self.seen_errors.clear();
            self.git_changed_paths.clear();
            self.git_changed_line_ranges.clear();
        }
        Some((self.project_refresh_generation, to_detect))
    }

    fn apply_project_info(&mut self, generation: u64, mut info: ProjectInfo) -> bool {
        if generation != self.project_refresh_generation {
            return false;
        }
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.git_root == info.git_root && project.path == info.path)
        else {
            return false;
        };
        // Branch notifications can arrive while runtime detection is running.
        // Enrichment must not roll their newer branch back.
        info.branch = project.branch.clone();
        self.project_info_cache
            .insert(info.path.clone(), info.clone());
        *project = info;
        self.projects.sort_by(|a, b| a.name.cmp(&b.name));
        true
    }

    /// A checkout's branch changed: re-read it for the cards of that
    /// repository, keeping everything else on them.
    pub fn refresh_branch(&mut self, root: &std::path::Path, ctx: &mut ModelContext<Self>) {
        let root = detect::normalize_path(root);
        let generation = self.project_refresh_generation;
        let affected: Vec<PathBuf> = self
            .projects
            .iter()
            .map(|project| project.git_root.clone())
            .filter(|git_root| {
                let git_root = detect::normalize_path(git_root);
                git_root == root || root.starts_with(&git_root)
            })
            .collect();
        for git_root in affected {
            ctx.spawn(
                async move {
                    let branch = super::super::project_naming::linked_worktree_branch(&git_root)
                        .or_else(|| detect::read_git_branch(&git_root));
                    (git_root, branch)
                },
                move |model, (git_root, branch), ctx| {
                    if model.project_refresh_generation != generation {
                        return;
                    }
                    if let Some(project) = model
                        .projects
                        .iter_mut()
                        .find(|project| project.git_root == git_root)
                    {
                        if project.branch != branch {
                            project.branch = branch;
                            ctx.emit(SidebarEvent::Changed);
                        }
                    }
                },
            );
        }
    }

    /// Clear errors when the active pane leaves a git project.
    pub fn clear_active_state(&mut self, _ctx: &mut ModelContext<Self>) {
        self.errors.clear();
        self.seen_errors.clear();
    }

    /// Harvest errors/warnings from a finished command's output snapshot.
    ///
    /// Called once per completed block (not per output chunk), so it never
    /// touches the live emulator grid on the hot path.
    pub fn harvest_errors(&mut self, blocks: &[BlockSnapshot], ctx: &mut ModelContext<Self>) {
        let mut changed = false;
        for block in blocks {
            for text in &block.tail_lines {
                if let Some(level) = classify_error_line(text) {
                    let key = text.trim().to_string();
                    if self.seen_errors.contains_key(&key) {
                        continue;
                    }
                    if self.errors.len() >= 20 {
                        break;
                    }
                    self.seen_errors.insert(key.clone(), Instant::now());
                    self.errors.push(ProcessError {
                        message: key,
                        level,
                        recorded_at: Instant::now(),
                    });
                    changed = true;
                }
            }
        }

        if changed {
            ctx.emit(SidebarEvent::Changed);
        }
    }
}

fn cwds_differ(a: &Option<PathBuf>, b: &Option<PathBuf>) -> bool {
    match (a, b) {
        (None, None) => false,
        (Some(x), Some(y)) => detect::normalize_path(x) != detect::normalize_path(y),
        _ => true,
    }
}

fn build_project_identity(cwd: &PathBuf, git_root: &PathBuf) -> ProjectInfo {
    let main_checkout = super::super::project_naming::worktree_main_checkout(git_root);
    let name = main_checkout
        .as_ref()
        .and_then(|path| path.file_name().and_then(|name| name.to_str()))
        .map(str::to_string)
        .unwrap_or_else(|| detect::project_name(cwd, git_root));
    ProjectInfo {
        name,
        path: cwd.clone(),
        git_root: git_root.clone(),
        branch: super::super::project_naming::linked_worktree_branch(git_root)
            .or_else(|| detect::read_git_branch(git_root)),
        status_label: None,
        is_monorepo: false,
        runtimes: Vec::new(),
        quick_actions: Vec::new(),
    }
}

fn build_project_info(cwd: &PathBuf, git_root: &PathBuf) -> ProjectInfo {
    let mut info = build_project_identity(cwd, git_root);
    info.status_label = detect::git_status_label(git_root);
    info.is_monorepo = detect::is_monorepo(git_root);
    info.runtimes = detect::detect_runtimes(cwd);
    info.quick_actions = detect::discover_quick_actions(cwd);
    info
}

fn classify_error_line(line: &str) -> Option<ErrorLevel> {
    let l = line.to_lowercase();
    if l.trim().len() < 4 {
        return None;
    }
    if l.contains("error[")
        || l.contains("error:")
        || l.starts_with("error ")
        || l.contains("enoent")
        || l.contains("eaddrinuse")
        || l.contains("eacces")
        || l.contains("cannot find module")
        || l.contains("failed to compile")
        || l.contains("exception in thread")
        || l.contains("panic:")
        || l.contains("traceback (most recent")
        || l.contains("syntaxerror")
        || l.contains("typeerror")
        || l.contains("referenceerror")
    {
        return Some(ErrorLevel::Error);
    }
    if l.contains("warning:")
        || l.contains("warn:")
        || l.contains("deprecationwarning")
        || l.contains("experimentalwarning")
    {
        return Some(ErrorLevel::Warn);
    }
    None
}

#[cfg(test)]
mod workspace_switch_tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    struct Checkouts {
        _temp: tempfile::TempDir,
        main: PathBuf,
        linked: PathBuf,
    }

    impl Checkouts {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let main = temp.path().join("project");
            let linked = temp.path().join("automation-workspace");
            std::fs::create_dir(&main).unwrap();
            git(&main, &["init", "-q", "-b", "main"]);
            git(
                &main,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "initial",
                ],
            );
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-qb",
                    "automation/daily-review",
                    linked.to_str().unwrap(),
                ],
            );
            Self {
                _temp: temp,
                main: detect::normalize_path(&main),
                linked: detect::normalize_path(&linked),
            }
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=.git/no-test-hooks",
            ])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn select(model: &mut ProjectSidebarModel, cwd: &PathBuf) -> u64 {
        model
            .update_project_context(std::slice::from_ref(cwd), Some(cwd.clone()))
            .unwrap()
            .0
    }

    fn assert_card(model: &ProjectSidebarModel, cwd: &Path, root: &Path, branch: &str) {
        assert_eq!(model.active_cwd.as_deref(), Some(cwd));
        assert_eq!(model.projects.len(), 1);
        assert_eq!(model.projects[0].path, cwd);
        assert_eq!(model.projects[0].git_root, root);
        assert_eq!(model.projects[0].branch.as_deref(), Some(branch));
    }

    #[test]
    fn public_refresh_updates_identity_in_the_same_ui_update() {
        use warpui::{App, UpdateModel};

        let checkouts = Checkouts::new();
        App::test((), |mut app| async move {
            app.update(|ctx| {
                let sidebar = ctx.add_model(|_| ProjectSidebarModel::new());
                ctx.update_model(&sidebar, |model, model_ctx| {
                    // Assert before returning control to the UI event loop:
                    // no background callback can have published a card yet.
                    for (root, branch) in [
                        (&checkouts.main, "main"),
                        (&checkouts.linked, "automation/daily-review"),
                        (&checkouts.main, "main"),
                    ] {
                        model.refresh_all_cwds(
                            std::slice::from_ref(root),
                            Some(root.clone()),
                            model_ctx,
                        );
                        assert_card(model, root, root, branch);
                    }
                });
            });
        });
    }

    #[test]
    fn workspace_identity_updates_before_background_detection() {
        let checkouts = Checkouts::new();
        let mut model = ProjectSidebarModel::new();

        select(&mut model, &checkouts.main);
        assert_card(&model, &checkouts.main, &checkouts.main, "main");
        select(&mut model, &checkouts.linked);
        assert_card(
            &model,
            &checkouts.linked,
            &checkouts.linked,
            "automation/daily-review",
        );
        assert_eq!(model.projects[0].name, "project");
        select(&mut model, &checkouts.main);
        assert_card(&model, &checkouts.main, &checkouts.main, "main");
    }

    #[test]
    fn delayed_detection_cannot_restore_an_inactive_workspace() {
        let checkouts = Checkouts::new();
        let mut model = ProjectSidebarModel::new();
        let old_generation = select(&mut model, &checkouts.main);
        let delayed = build_project_identity(&checkouts.main, &checkouts.main);
        let generation = select(&mut model, &checkouts.linked);

        assert!(!model.apply_project_info(old_generation, delayed));
        assert_card(
            &model,
            &checkouts.linked,
            &checkouts.linked,
            "automation/daily-review",
        );
        assert!(model.apply_project_info(
            generation,
            build_project_identity(&checkouts.linked, &checkouts.linked)
        ));
        assert_card(
            &model,
            &checkouts.linked,
            &checkouts.linked,
            "automation/daily-review",
        );
    }

    #[test]
    fn rapid_round_trip_rejects_the_first_visit_even_for_the_same_root() {
        let checkouts = Checkouts::new();
        let mut model = ProjectSidebarModel::new();
        let old_generation = select(&mut model, &checkouts.main);
        let mut delayed = build_project_identity(&checkouts.main, &checkouts.main);
        delayed.status_label = Some("outdated status".into());
        select(&mut model, &checkouts.linked);
        let generation = select(&mut model, &checkouts.main);

        assert!(!model.apply_project_info(old_generation, delayed));
        assert!(model.projects[0].status_label.is_none());
        assert!(model.apply_project_info(
            generation,
            build_project_identity(&checkouts.main, &checkouts.main)
        ));
    }

    #[test]
    fn returning_to_cached_workspace_reads_its_current_branch() {
        let checkouts = Checkouts::new();
        let mut model = ProjectSidebarModel::new();
        let generation = select(&mut model, &checkouts.main);
        let mut info = build_project_identity(&checkouts.main, &checkouts.main);
        info.runtimes.push(RuntimeInfo {
            kind: detect::RuntimeKind::Rust,
            version: "cached".into(),
        });
        assert!(model.apply_project_info(generation, info));
        select(&mut model, &checkouts.linked);
        git(
            &checkouts.main,
            &["checkout", "-qb", "renamed-while-inactive"],
        );

        let (_, pending) = model
            .update_project_context(
                std::slice::from_ref(&checkouts.main),
                Some(checkouts.main.clone()),
            )
            .unwrap();
        assert!(
            pending.is_empty(),
            "completed runtime detection should be reused"
        );
        assert_card(
            &model,
            &checkouts.main,
            &checkouts.main,
            "renamed-while-inactive",
        );
        assert_eq!(model.projects[0].runtimes[0].version, "cached");
    }

    #[test]
    fn cwd_change_inside_the_same_repository_updates_the_displayed_path() {
        let checkouts = Checkouts::new();
        let nested = checkouts.main.join("src");
        std::fs::create_dir(&nested).unwrap();
        let mut model = ProjectSidebarModel::new();
        let generation = select(&mut model, &checkouts.main);
        assert!(model.apply_project_info(
            generation,
            build_project_identity(&checkouts.main, &checkouts.main)
        ));

        select(&mut model, &nested);
        assert_card(&model, &nested, &checkouts.main, "main");
        assert!(model
            .update_project_context(std::slice::from_ref(&nested), Some(nested.clone()))
            .is_none());
    }

    #[test]
    fn leaving_git_or_clearing_context_rejects_pending_cards() {
        let checkouts = Checkouts::new();
        let plain = checkouts._temp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        let mut model = ProjectSidebarModel::new();
        let generation = select(&mut model, &checkouts.main);

        select(&mut model, &plain);
        assert!(model.projects.is_empty());
        assert!(!model.apply_project_info(
            generation,
            build_project_identity(&checkouts.main, &checkouts.main)
        ));
        let generation = select(&mut model, &checkouts.linked);
        model.update_project_context(&[], None).unwrap();
        assert!(model.projects.is_empty());
        assert!(model.active_cwd.is_none());
        assert!(!model.apply_project_info(
            generation,
            build_project_identity(&checkouts.linked, &checkouts.linked)
        ));
    }

    #[test]
    fn runtime_detection_does_not_overwrite_a_newer_branch_notification() {
        let checkouts = Checkouts::new();
        let mut model = ProjectSidebarModel::new();
        let generation = select(&mut model, &checkouts.main);
        let delayed = build_project_identity(&checkouts.main, &checkouts.main);
        model.projects[0].branch = Some("new-branch".into());

        assert!(model.apply_project_info(generation, delayed));
        assert_card(&model, &checkouts.main, &checkouts.main, "new-branch");
    }
}

#[cfg(test)]
mod document_tree_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn nested_documents_require_all_parent_directories_to_be_expanded() {
        let mut model = ProjectSidebarModel::new();
        assert!(model.is_document_visible(Path::new("README.md")));
        assert!(!model.is_document_visible(Path::new("src/features/editor.rs")));

        model.expanded_directories.insert(PathBuf::from("src"));
        assert!(model.is_document_visible(Path::new("src/lib.rs")));
        assert!(!model.is_document_visible(Path::new("src/features/editor.rs")));

        model
            .expanded_directories
            .insert(PathBuf::from("src/features"));
        assert!(model.is_document_visible(Path::new("src/features/editor.rs")));
    }

    #[test]
    fn restored_expanded_directories_remain_loaded_for_refreshes() {
        let mut model = ProjectSidebarModel::new();
        model.configure_recovery(
            PathBuf::from("/workspace"),
            Vec::new(),
            vec![PathBuf::from("src"), PathBuf::from("src/features")],
        );

        let loaded = model.loaded_directories();
        assert!(loaded.contains(&PathBuf::new()));
        assert!(loaded.contains(&PathBuf::from("src")));
        assert!(loaded.contains(&PathBuf::from("src/features")));
    }

    #[test]
    fn draft_identity_includes_the_workspace_root() {
        let mut model = ProjectSidebarModel::new();
        model.workspace_root = PathBuf::from("/workspace/one");
        let first = model.draft_key(Path::new("src/main.rs"));
        model.workspace_root = PathBuf::from("/workspace/two");
        let second = model.draft_key(Path::new("src/main.rs"));

        assert_ne!(first, second);
    }

    #[test]
    fn inactive_editor_tab_reports_its_recovery_copy_as_dirty() {
        let mut model = ProjectSidebarModel::new();
        let root = PathBuf::from("/workspace/one");
        let path = PathBuf::from("src/main.rs");
        model.workspace_root = root.clone();
        model.document_drafts.insert(
            (root.clone(), path.clone()),
            PersistedWorkingCopy {
                workspace_root: root.clone(),
                relative_path: path.clone(),
                draft_text: "fn changed() {}".to_string(),
                base_hash: 1,
                base_size: 1,
                base_modified_unix_ms: None,
                revision: 1,
                updated_at_unix_ms: 1,
            },
        );

        assert!(model.document_is_dirty(&root, &path));
        assert!(!model.document_is_dirty(&root, Path::new("src/lib.rs")));
    }
}
