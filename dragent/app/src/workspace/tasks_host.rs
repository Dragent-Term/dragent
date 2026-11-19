//! The Tasks page in the window: opening and closing it, opening a task's
//! workspace, and starting a task: the new-workspace prompt opens on the
//! task's project with the task as its request, and the workspace it creates
//! is linked back to the task, which moves to In progress.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dragent_tasks::{LinkKind, NewLink, ProjectRef, Status, Task};
use serde::{Deserialize, Serialize};
use warpui::{TypedActionView, UpdateView, ViewContext};

use super::view::WorkspaceView;
use super::WorkspaceAction;
use crate::integrations::Provider;
use crate::integrations::tracker::TrackerItem;

/// A Start that hit a project that isn't open locally, waiting for that
/// project to open (usually via the prefilled clone form) to resume.
#[derive(Clone, Debug)]
pub(crate) enum PendingStart {
    Task(Task),
    Item(TrackerItem),
}

impl WorkspaceView {
    pub(crate) fn open_tasks(&mut self, ctx: &mut ViewContext<Self>) {
        self.close_pull_requests(ctx);
        self.close_settings(ctx);
        self.close_automations(ctx);
        self.close_teams(ctx);
        self.close_org_menus();
        self.tasks_open = true;
        let theme = self.theme;
        let root = self.selected_project_root().or_else(|| self.active_project_root());
        let projects = project_refs_for_roots(&self.sidebar_project_roots());
        let page = self.tasks_view.clone();
        ctx.update_view(&page, |page, page_ctx| page.show(theme, root, projects, page_ctx));
        ctx.focus(&page);
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    pub(crate) fn close_tasks(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.tasks_open {
            return;
        }
        self.tasks_open = false;
        self.pending_task_start = None;
        let page = self.tasks_view.clone();
        ctx.update_view(&page, |page, _| page.hide());
        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == self.active_tab_id) {
            tab.pane.focus(ctx);
        } else {
            ctx.focus_self();
        }
        self.sync_left_panel(ctx);
        ctx.notify();
    }

    pub(crate) fn on_tasks_event(&mut self, event: &crate::tasks_page::TasksEvent, ctx: &mut ViewContext<Self>) {
        use crate::tasks_page::TasksEvent;
        match event {
            TasksEvent::Start(tasks) => {
                // An explicit start supersedes any start that waited on a
                // project to open.
                self.pending_task_start = None;
                // One at a time: each goes through the prompt, where the
                // person picks the agent and checks the request; the next
                // opens once the one before has its workspace.
                if let Some((first, rest)) = tasks.split_first() {
                    self.task_start_queue = rest.to_vec();
                    self.start_task(first.clone(), ctx);
                }
            }
            TasksEvent::OpenWorkspace(task) => {
                if !self.open_task_workspace(task, ctx) {
                    self.show_error_notice("Tasks", format!("{}'s workspace isn't on this computer.", task.key), ctx);
                }
            }
            TasksEvent::OpenUrl(url) => {
                self.handle_action(&WorkspaceAction::OpenExternalUrl { url: url.clone() }, ctx);
            }
            TasksEvent::StartItem { item, root } => {
                self.pending_task_start = None;
                self.start_tracker_item(item.clone(), root.clone(), ctx)
            }
        }
    }

    /// Show the workspace this machine has for `task`, if any.
    fn open_task_workspace(&mut self, task: &Task, ctx: &mut ViewContext<Self>) -> bool {
        let device = crate::settings::AppSettings::device_id();
        let workspace_id = task.links.iter().rev().filter(|link| link.kind == LinkKind::Workspace).find_map(|link| {
            let (machine, root) = dragent_tasks::parse_workspace_reference(&link.reference)?;
            if machine != device {
                return None;
            }
            self.workspaces
                .iter()
                .find(|workspace| workspace.root == Path::new(root))
                .map(|workspace| workspace.id)
        });
        let Some(workspace_id) = workspace_id else {
            return false;
        };
        self.close_tasks(ctx);
        self.handle_action(&WorkspaceAction::SelectProjectWorkspace { workspace_id }, ctx);
        true
    }

    /// The next selected task, once the one before has its workspace.
    pub(crate) fn start_next_queued_task(&mut self, ctx: &mut ViewContext<Self>) {
        if self.task_start_queue.is_empty() {
            return;
        }
        let next = self.task_start_queue.remove(0);
        self.start_task(next, ctx);
    }

    /// Open the new-workspace prompt on the task's project, with the task as
    /// the request. When the project isn't open locally, the clone form
    /// opens prefilled with its address instead of dead-ending, and the
    /// start resumes once that project opens.
    fn start_task(&mut self, task: Task, ctx: &mut ViewContext<Self>) {
        let Some(root) = self.project_for_task(&task) else {
            match task.project.clone() {
                Some(project) => {
                    self.pending_task_start = Some(PendingStart::Task(task));
                    let _ = self.begin_clone_from_url(format!("https://{}", project.key), ctx);
                }
                None => {
                    self.show_error_notice("Tasks", format!("Open or select a project first, then start {} again.", task.key), ctx);
                }
            }
            return;
        };
        self.close_tasks(ctx);
        self.handle_action(&WorkspaceAction::OpenProjectWorkspacePrompt { root }, ctx);
        let prompt = dragent_tasks::task_prompt(&task);
        self.prompt_text = prompt.clone();
        let editor = self.prompt_editor.clone();
        ctx.update_view(&editor, |editor, editor_ctx| editor.set_text(&prompt, editor_ctx));
        self.prompt_task = Some(task);
        ctx.notify();
    }

    /// Open the new-workspace prompt with a tracker item attached, rooted
    /// at the item's repository when that names an open project (the way
    /// a linked work item preselects its repo), else the project in front.
    pub(super) fn start_tracker_item(
        &mut self,
        item: crate::integrations::tracker::TrackerItem,
        root: Option<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(root) = self
            .root_for_tracker_item(&item)
            .or(root)
            .or_else(|| self.selected_project_root())
            .or_else(|| self.active_project_root())
            .filter(|root| root.is_dir())
        else {
            // The item names a repository that isn't open: offer the clone
            // form prefilled and resume once that project opens. Items
            // without a repository still need a project picked first.
            match dragent_tasks::item_project_key(&item.url) {
                Some(key) => {
                    self.pending_task_start = Some(PendingStart::Item(item));
                    let _ = self.begin_clone_from_url(format!("https://{key}"), ctx);
                }
                None => {
                    self.show_error_notice("Tasks", format!("Open or select a project first, then start {} again.", item.key), ctx);
                }
            }
            return;
        };
        self.close_tasks(ctx);
        self.handle_action(&WorkspaceAction::OpenProjectWorkspacePrompt { root }, ctx);
        let item = item.item;
        self.prompt_links.push(super::project_links::PromptLink {
            kind: super::project_links::LinkKind::Item(item.provider),
            identifier: item.key.clone(),
            title: item.title.clone(),
            url: item.url.clone(),
            body: item.body.clone(),
            number: None,
            head_branch: None,
            branch: (!item.branch.is_empty()).then(|| item.branch.clone()),
            detail: item.detail.clone(),
            reference: item.reference.clone(),
        });
        ctx.notify();
    }

    /// The local project a task belongs to: the one whose repository it
    /// names, else the selected project when the task names none.
    pub(super) fn project_for_task(&self, task: &Task) -> Option<PathBuf> {
        match &task.project {
            Some(project) => self.root_for_project_key(&project.key),
            None => self
                .selected_project_root()
                .or_else(|| self.active_project_root())
                .filter(|root| root.is_dir()),
        }
    }

    /// The open checkout whose origin matches a shared project key.
    fn root_for_project_key(&self, key: &str) -> Option<PathBuf> {
        self.sidebar_project_roots().into_iter().find(|root| {
            origin_url(root)
                .and_then(|url| dragent_tasks::project_key(&url))
                .is_some_and(|root_key| root_key == key)
        })
    }

    /// The open checkout matching a tracker item's repository, if its
    /// address names one.
    fn root_for_tracker_item(&self, item: &TrackerItem) -> Option<PathBuf> {
        dragent_tasks::item_project_key(&item.url)
            .and_then(|key| self.root_for_project_key(&key))
    }

    /// Resume a Start that waited on a project: now that a project opened,
    /// start it if its project resolves, else keep waiting.
    pub(crate) fn maybe_resume_pending_start(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(pending) = self.pending_task_start.take() else {
            return;
        };
        match pending {
            PendingStart::Task(task) if self.project_for_task(&task).is_some() => {
                self.start_task(task, ctx);
            }
            PendingStart::Item(item) if self.root_for_tracker_item(&item).is_some() => {
                self.start_tracker_item(item, None, ctx);
            }
            pending => self.pending_task_start = Some(pending),
        }
    }

    /// The workspace created from a task's prompt: link it to the task, and
    /// move a task not yet started to In progress.
    pub(crate) fn link_started_task(&mut self, task: Task, root: &Path, branch: &str, ctx: &mut ViewContext<Self>) {
        let (Some(org), _) = crate::tasks_page::signed_in(ctx) else {
            return;
        };
        let link = NewLink {
            kind: LinkKind::Workspace,
            title: branch.to_string(),
            url: None,
            reference: dragent_tasks::workspace_reference(
                &crate::settings::AppSettings::device_id(),
                &root.to_string_lossy(),
            ),
            state: None,
            by_agent: false,
        };
        let start = matches!(task.status, Status::Backlog | Status::Todo);
        let id = task.id.clone();
        let root_path = root.to_path_buf();
        let items: Vec<(String, String)> = task
            .links_of(LinkKind::Item)
            .filter_map(|link| {
                let (provider, key) = link.reference.split_once(':')?;
                Some((provider.trim().to_string(), key.trim().to_string()))
            })
            .collect();
        remember_started(root, &task);
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::tasks_page::data::add_link(&org, &id, &link)?;
                    if start {
                        crate::tasks_page::data::set_status(&org, &id, Status::InProgress)?;
                        // The agent started working: tell linked trackers
                        // too, best effort, without failing the start.
                        let _ = start_tracker_sync(&root_path, &items);
                    }
                    Ok::<(), String>(())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |view, result, ctx| {
                if let Err(error) = result {
                    view.show_error_notice("Tasks", format!("{} wasn't linked to its workspace: {error}", task.key), ctx);
                }
            },
        );
    }

    /// A pull request showed up (or changed) for a workspace: when that
    /// workspace was started from a task, link the pull request to it and
    /// move the task forward: In review once it's open, Done once merged.
    /// It never moves a task back.
    pub(crate) fn sync_task_pull_request(
        &mut self,
        root: &Path,
        pr: &super::project_sidebar::ReviewPullRequest,
        ctx: &mut ViewContext<Self>,
    ) {
        let mut started = load_started();
        let Some(entry) = started.workspaces.get_mut(&root_key(root)) else {
            return;
        };
        if entry.pr_url.as_deref() == Some(pr.url.as_str()) && entry.pr_state.as_deref() == Some(pr.state.as_str()) {
            return;
        }
        let (Some(org), _) = crate::tasks_page::signed_in(ctx) else {
            return;
        };
        entry.pr_url = Some(pr.url.clone());
        entry.pr_state = Some(pr.state.clone());
        let task_id = entry.task_id.clone();
        let key = entry.key.clone();
        save_started(&started);
        let link = NewLink {
            kind: LinkKind::PullRequest,
            title: format!("#{} {}", pr.number, pr.title),
            url: Some(pr.url.clone()),
            reference: pr.url.clone(),
            state: Some(pr.state.clone()),
            by_agent: false,
        };
        let state = pr.state.clone();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::tasks_page::data::add_link(&org, &task_id, &link)?;
                    let task = crate::tasks_page::data::get_task(&org, &task_id)?;
                    if let Some(status) = status_after_pull_request(task.status, &state) {
                        crate::tasks_page::data::set_status(&org, &task_id, status)?;
                    }
                    Ok::<(), String>(())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |_, result, _| {
                if let Err(error) = result {
                    log::warn!("[tasks] {key}: pull request not linked: {error}");
                }
            },
        );
    }
}

/// Where a task goes when its pull request is `state`: forward only.
fn status_after_pull_request(current: Status, state: &str) -> Option<Status> {
    let next = match state {
        "merged" => Status::Done,
        "open" => Status::InReview,
        _ => return None,
    };
    (current.is_open() && current < next).then_some(next)
}

/// Workspaces this machine started from tasks, and the pull request state
/// last sent for each (so a refresh doesn't send it again).
#[derive(Default, Serialize, Deserialize)]
struct StartedWorkspaces {
    #[serde(default)]
    workspaces: BTreeMap<String, Started>,
}

#[derive(Serialize, Deserialize)]
struct Started {
    task_id: String,
    key: String,
    #[serde(default)]
    pr_url: Option<String>,
    #[serde(default)]
    pr_state: Option<String>,
}

fn started_path() -> PathBuf {
    dragent_paths::organisation_dir().join("tasks").join("workspaces.json")
}

fn root_key(root: &Path) -> String {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf()).to_string_lossy().into_owned()
}

fn load_started() -> StartedWorkspaces {
    std::fs::read_to_string(started_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_started(started: &StartedWorkspaces) {
    let path = started_path();
    let Ok(text) = serde_json::to_string_pretty(started) else {
        return;
    };
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

fn remember_started(root: &Path, task: &Task) {
    let mut started = load_started();
    started.workspaces.insert(
        root_key(root),
        Started {
            task_id: task.id.clone(),
            key: task.key.clone(),
            pr_url: None,
            pr_state: None,
        },
    );
    save_started(&started);
}

/// The task-linkable projects among local roots: each root whose git
/// origin maps to a shared project key, labelled by its folder name.
/// Keys deduplicate (several checkouts can share one remote) and sort by
/// label for the picker.
pub(crate) fn project_refs_for_roots(roots: &[PathBuf]) -> Vec<ProjectRef> {
    let mut out: Vec<ProjectRef> = Vec::new();
    for root in roots {
        let Some(key) =
            origin_url(root).and_then(|url| dragent_tasks::project_key(&url))
        else {
            continue;
        };
        if out.iter().any(|known| known.key == key) {
            continue;
        }
        let label = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| key.clone());
        out.push(ProjectRef { key, label });
    }
    out.sort_by(|a, b| {
        a.label
            .to_lowercase()
            .cmp(&b.label.to_lowercase())
    });
    out
}

/// Best-effort mirror of a start onto linked tracker items, without
/// failing the start itself: providers with no such state are skipped,
/// real failures are only logged (the native task already moved).
fn start_tracker_sync(root: &Path, items: &[(String, String)]) {
    let resolved: Vec<(Provider, String)> = items
        .iter()
        .filter_map(|(provider, key)| {
            Provider::from_key(provider).map(|provider| (provider, key.clone()))
        })
        .collect();
    for failure in crate::integrations::tracker::push_status(root, &resolved, Status::InProgress) {
        log::warn!("[tasks] tracker sync on start failed: {failure}");
    }
}

/// The `origin` remote's address from a checkout's git config, read from
/// the file (no git process on the UI thread).
pub(crate) fn origin_url(root: &Path) -> Option<String> {
    let config = std::fs::read_to_string(root.join(".git").join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
        } else if in_origin {
            if let Some(url) = line.strip_prefix("url").map(str::trim).and_then(|rest| rest.strip_prefix('=')) {
                return Some(url.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_roots_become_linkable_projects() {
        let base = std::env::temp_dir().join(format!("dragent-projects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let checkout = |name: &str, origin: Option<&str>| {
            let root = base.join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            if let Some(url) = origin {
                std::fs::write(
                    root.join(".git/config"),
                    format!("[remote \"origin\"]\n\turl = {url}\n"),
                )
                .unwrap();
            }
            root
        };
        let zebra = checkout("zebra", Some("git@github.com:Acme/App.git"));
        let _apple = checkout("apple", Some("https://github.com/acme/app"));
        let _plain = checkout("plain", None);
        let roots = vec![
            zebra.clone(),
            base.join("apple"),
            base.join("plain"),
            base.join("missing"),
        ];
        let projects = project_refs_for_roots(&roots);
        let _ = std::fs::remove_dir_all(&base);
        // Same remote twice collapses to one project; the folder without
        // an origin (and the missing folder) offer nothing to attach.
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].key, "github.com/acme/app");
        assert!(!projects[0].label.is_empty());
    }

    #[test]
    fn project_choices_sort_by_label() {
        let base = std::env::temp_dir().join(format!("dragent-project-sort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let checkout = |name: &str| {
            let root = base.join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            std::fs::write(
                root.join(".git/config"),
                format!("[remote \"origin\"]\n\turl = git@github.com:acme/{name}.git\n"),
            )
            .unwrap();
            root
        };
        let roots = vec![checkout("zebra"), checkout("apple"), checkout("Mango")];
        let projects = project_refs_for_roots(&roots);
        let _ = std::fs::remove_dir_all(&base);
        let labels: Vec<&str> = projects.iter().map(|project| project.label.as_str()).collect();
        assert_eq!(labels, vec!["apple", "Mango", "zebra"]);
    }

    #[test]
    fn a_pull_request_only_moves_its_task_forward() {
        assert_eq!(status_after_pull_request(Status::InProgress, "open"), Some(Status::InReview));
        assert_eq!(status_after_pull_request(Status::Todo, "merged"), Some(Status::Done));
        assert_eq!(status_after_pull_request(Status::InReview, "open"), None, "already there");
        assert_eq!(status_after_pull_request(Status::Done, "open"), None, "never reopened");
        assert_eq!(status_after_pull_request(Status::Canceled, "merged"), None);
        assert_eq!(status_after_pull_request(Status::InProgress, "draft"), None);
        assert_eq!(status_after_pull_request(Status::InProgress, "closed"), None);
    }

    #[test]
    fn the_origin_remote_is_read_from_the_checkout() {
        let dir = std::env::temp_dir().join(format!("dragent-origin-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(
            dir.join(".git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:other/app.git\n[remote \"origin\"]\n\turl = git@github.com:Acme/App.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        )
        .unwrap();
        let url = origin_url(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(url.as_deref(), Some("git@github.com:Acme/App.git"));
        assert_eq!(dragent_tasks::project_key(url.as_deref().unwrap()).as_deref(), Some("github.com/acme/app"));
    }
}
