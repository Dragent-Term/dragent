//! The Tasks page: the organisation's tasks as a table grouped by status or
//! as a board, filtered by status, assignee, label and words, with tasks
//! created, moved between statuses and started from here. The tasks live on
//! the server (see `dragent_tasks`); the page looks again every 30 seconds
//! while it's open and after each change it makes.

pub(crate) mod data;
pub(crate) mod preview;
mod render;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use dragent_tasks::{LinkKind, NewTask, Person, Priority, ProjectRef, Status, Task, TaskDetail, TaskPatch};
use warpui::elements::ClippedScrollStateHandle;
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Element, Entity, EntityId, SingletonEntity, TypedActionView, UpdateView, View,
    ViewContext, ViewHandle,
};
use warpui_core::elements::{DraggableState, MouseStateHandle};

use crate::integrations::tracker::TrackerItem;
use crate::integrations::Provider;
use crate::theme::Theme;
use crate::workspace::task_detail::{TaskDetailEvent, TaskDetailProps, TaskDetailView};
use crate::workspace::{InputEditor, InputMode};

/// How often an open page looks again.
const REFRESH: Duration = Duration::from_secs(30);

/// Which tasks the status filter shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    /// Backlog through review.
    #[default]
    Open,
    /// Todo through review.
    Active,
    All,
    Only(Status),
}

impl StatusFilter {
    pub(crate) const CHOICES: [StatusFilter; 9] = [
        StatusFilter::Open,
        StatusFilter::Active,
        StatusFilter::All,
        StatusFilter::Only(Status::Backlog),
        StatusFilter::Only(Status::Todo),
        StatusFilter::Only(Status::InProgress),
        StatusFilter::Only(Status::InReview),
        StatusFilter::Only(Status::Done),
        StatusFilter::Only(Status::Canceled),
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            StatusFilter::Open => "Open",
            StatusFilter::Active => "Active",
            StatusFilter::All => "All",
            StatusFilter::Only(status) => status.label(),
        }
    }

    pub(crate) fn shows(self, status: Status) -> bool {
        match self {
            StatusFilter::Open => status.is_open(),
            StatusFilter::Active => status.is_active(),
            StatusFilter::All => true,
            StatusFilter::Only(only) => only == status,
        }
    }
}

/// Whose tasks show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AssigneeFilter {
    #[default]
    Anyone,
    Me,
    Unassigned,
}

/// Whose list the page shows: the organisation's own tasks, or a connected
/// tracker's items.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    Dragent,
    Tracker(Provider),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Table,
    Board,
}

/// Which small menu is open over the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Menu {
    Status,
    BulkStatus,
    NewStatus,
    NewPriority,
    NewAssignee,
}

#[derive(Clone, Debug)]
pub enum TasksAction {
    Refresh,
    SetSource(Source),
    /// Open a tracker item in its service.
    OpenItem(String),
    /// Start a tracker item in a workspace.
    StartItem(String),
    /// Save a tracker item as a native task, linked back to the item.
    ImportItem(String),
    /// A tracker item's card dropped on another status's column.
    MoveItem { id: String, status: Status },
    SetStatusFilter(StatusFilter),
    SetAssignee(AssigneeFilter),
    SetLabel(Option<String>),
    SetLayout(Layout),
    ToggleMenu(Menu),
    CloseMenus,
    ToggleGroup(Status),
    ToggleSelect(String),
    ClearSelection,
    /// Open a task's page.
    Open(String),
    /// A card dropped on another status's column.
    Move { id: String, status: Status },
    /// Every selected task to this status.
    SetSelectedStatus(Status),
    /// Start the selected tasks (or this one) in workspaces.
    Start(Vec<String>),
    NewTask,
    CancelNew,
    NewItem,
    CancelNewItem,
    SubmitNewItem,
    SetNewStatus(Status),
    SetNewPriority(Priority),
    SetNewAssignee(Option<String>),
    SubmitNew,
    DismissError,
}

pub enum TasksEvent {
    /// Start these tasks in workspaces (the workspace asks where and with
    /// which agent).
    Start(Vec<Task>),
    /// Show the workspace this machine has for the task.
    OpenWorkspace(Task),
    OpenUrl(String),
    /// Start a tracker item in a workspace, from this project.
    StartItem { item: TrackerItem, root: Option<PathBuf> },
}

/// The task whose page is open over the list.
pub(crate) struct OpenTask {
    pub id: String,
    pub detail: Option<TaskDetail>,
    pub busy: bool,
    pub error: Option<String>,
}

/// The new-task form, while it's open.
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub status: Status,
    pub priority: Priority,
    pub assignee: Option<String>,
    pub saving: bool,
}

/// The new-tracker-item form, while it's open.
#[derive(Clone, Debug)]
pub(crate) struct ItemDraft {
    pub saving: bool,
}

pub struct TasksView {
    pub(crate) view_id: EntityId,
    pub(crate) ui_font: FamilyId,
    pub(crate) mono_font: FamilyId,
    pub(crate) theme: Theme,
    visible: bool,
    /// The signed-in organisation; `None` while signed out.
    pub(crate) org: Option<String>,
    pub(crate) me: Option<String>,
    pub(crate) snapshot: Option<data::Snapshot>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) status_filter: StatusFilter,
    pub(crate) assignee: AssigneeFilter,
    pub(crate) label: Option<String>,
    pub(crate) layout: Layout,
    pub(crate) menu: Option<Menu>,
    pub(crate) collapsed: HashSet<Status>,
    pub(crate) selected: HashSet<String>,
    pub(crate) search_editor: ViewHandle<InputEditor>,
    pub(crate) search: String,
    pub(crate) title_editor: ViewHandle<InputEditor>,
    pub(crate) description_editor: ViewHandle<InputEditor>,
    pub(crate) item_place_editor: ViewHandle<InputEditor>,
    pub(crate) draft: Option<Draft>,
    pub(crate) item_draft: Option<ItemDraft>,
    pub(crate) open: Option<OpenTask>,
    pub(crate) detail_view: ViewHandle<TaskDetailView>,
    pub(crate) source: Source,
    /// Connected trackers whose items can show here.
    pub(crate) providers: Vec<Provider>,
    pub(crate) tracker_items: Vec<TrackerItem>,
    pub(crate) tracker_loading: bool,
    pub(crate) tracker_error: Option<String>,
    /// The project a tracker is asked from (code hosts answer per
    /// repository).
    pub(crate) root: Option<PathBuf>,
    /// Local projects a task can attach to (roots with a git origin),
    /// refreshed whenever the page opens.
    pub(crate) available_projects: Vec<ProjectRef>,
    /// Tracker item ids with an import in flight; their Import buttons
    /// stay disabled until the server answers, so double-clicking cannot
    /// create the task twice.
    pub(crate) importing: HashSet<String>,
    tracker_generation: u64,
    generation: u64,
    refresh_scheduled: bool,
    pub(crate) width: std::cell::Cell<f32>,
    pub(crate) mouse: RefCell<HashMap<String, MouseStateHandle>>,
    pub(crate) drags: RefCell<HashMap<String, DraggableState>>,
    pub(crate) scroll: ClippedScrollStateHandle,
    pub(crate) board_scroll: ClippedScrollStateHandle,
    pub(crate) column_scrolls: RefCell<HashMap<Status, ClippedScrollStateHandle>>,
}

impl Entity for TasksView {
    type Event = TasksEvent;
}

fn editor(
    ctx: &mut ViewContext<TasksView>,
    placeholder: &'static str,
    ui_font: FamilyId,
    theme: Theme,
    mode: InputMode,
) -> ViewHandle<InputEditor> {
    ctx.add_typed_action_view(move |inner| {
        InputEditor::new(
            placeholder,
            ui_font,
            crate::theme::text_size(13.0),
            theme.main_text(),
            theme.disabled_text(),
            theme.accent,
            theme,
            mode,
            inner.focus_handle(),
        )
    })
}

impl TasksView {
    pub fn new(ctx: &mut ViewContext<Self>, ui_font: FamilyId, mono_font: FamilyId, theme: Theme) -> Self {
        let search_editor = editor(ctx, "Search tasks", ui_font, theme, InputMode::Plain { key: "tasks_search".into() });
        ctx.subscribe_to_view(&search_editor, |view, editor, _, ctx| {
            let text = ctx.update_view(&editor, |editor, _| editor.get_text().to_string());
            if text != view.search {
                view.search = text;
                ctx.notify();
            }
        });
        let title_editor = editor(ctx, "Task title", ui_font, theme, InputMode::Plain { key: "tasks_new_title".into() });
        let description_editor = editor(
            ctx,
            "Description (optional)",
            ui_font,
            theme,
            InputMode::MultiLine {
                key: "tasks_new_description".into(),
                max_height: 220,
            },
        );
        let item_place_editor = editor(ctx, "Where (team, project, owner/repo — blank for default)", ui_font, theme, InputMode::Plain { key: "tasks_new_item_place".into() });
        let detail_view = ctx.add_typed_action_view(|inner| TaskDetailView::new(inner, ui_font, mono_font, theme));
        ctx.subscribe_to_view(&detail_view, |view, _, event, ctx| view.on_detail_event(event, ctx));
        Self {
            open: None,
            detail_view,
            source: Source::default(),
            providers: Vec::new(),
            tracker_items: Vec::new(),
            tracker_loading: false,
            tracker_error: None,
            root: None,
            available_projects: Vec::new(),
            importing: HashSet::new(),
            tracker_generation: 0,
            view_id: ctx.view_id(),
            ui_font,
            mono_font,
            theme,
            visible: false,
            org: None,
            me: None,
            snapshot: None,
            loading: true,
            error: None,
            status_filter: StatusFilter::default(),
            assignee: AssigneeFilter::default(),
            label: None,
            layout: Layout::default(),
            menu: None,
            collapsed: HashSet::new(),
            selected: HashSet::new(),
            search_editor,
            search: String::new(),
            title_editor,
            description_editor,
            item_place_editor,
            draft: None,
            item_draft: None,
            generation: 0,
            refresh_scheduled: false,
            width: std::cell::Cell::new(1100.0),
            mouse: RefCell::new(HashMap::new()),
            drags: RefCell::new(HashMap::new()),
            scroll: ClippedScrollStateHandle::new(),
            board_scroll: ClippedScrollStateHandle::new(),
            column_scrolls: RefCell::new(HashMap::new()),
        }
    }

    pub(crate) fn mouse(&self, key: &str) -> MouseStateHandle {
        self.mouse.borrow_mut().entry(key.to_string()).or_default().clone()
    }

    pub(crate) fn drag(&self, id: &str) -> DraggableState {
        self.drags.borrow_mut().entry(id.to_string()).or_default().clone()
    }

    pub(crate) fn column_scroll(&self, status: Status) -> ClippedScrollStateHandle {
        self.column_scrolls.borrow_mut().entry(status).or_default().clone()
    }

    pub(crate) fn tasks(&self) -> &[Task] {
        self.snapshot.as_ref().map(|snapshot| snapshot.tasks.as_slice()).unwrap_or(&[])
    }

    pub(crate) fn members(&self) -> &[Person] {
        self.snapshot.as_ref().map(|snapshot| snapshot.members.as_slice()).unwrap_or(&[])
    }

    pub(crate) fn task(&self, id: &str) -> Option<&Task> {
        self.tasks().iter().find(|task| task.id == id)
    }

    /// The tasks the filters let through, in the order they show.
    pub(crate) fn visible_tasks(&self) -> Vec<&Task> {
        let filter = Filter {
            status: self.status_filter,
            assignee: self.assignee,
            me: self.me.as_deref(),
            label: self.label.as_deref(),
            words: &self.search,
        };
        let mut tasks: Vec<&Task> = self.tasks().iter().filter(|task| filter.matches(task)).collect();
        sort_tasks(&mut tasks);
        tasks
    }

    /// Every label in use, for the label filter.
    pub(crate) fn labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.tasks().iter().flat_map(|task| task.labels.iter().cloned()).collect();
        labels.sort();
        labels.dedup();
        labels
    }

    /// The page came on screen, `root` being the project in front and
    /// `projects` the local projects a task can attach to.
    pub fn show(&mut self, theme: Theme, root: Option<PathBuf>, projects: Vec<ProjectRef>, ctx: &mut ViewContext<Self>) {
        self.set_theme(theme, ctx);
        self.visible = true;
        self.available_projects = projects;
        if root != self.root {
            self.root = root;
            self.tracker_items.clear();
        }
        self.load_providers(ctx);
        if matches!(self.source, Source::Tracker(_)) {
            self.load_tracker(ctx);
        }
        let (org, me) = signed_in(ctx);
        if org != self.org {
            // The list saved last time shows at once; the server's follows.
            self.snapshot = org.as_deref().and_then(data::cached);
            self.selected.clear();
            self.loading = org.is_some() && self.snapshot.is_none();
        }
        self.org = org;
        self.me = me;
        self.refresh(ctx);
    }

    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        for editor in [&self.search_editor, &self.title_editor, &self.description_editor, &self.item_place_editor] {
            ctx.update_view(editor, |editor, ctx| editor.set_theme(theme, ctx));
        }
        ctx.update_view(&self.detail_view, |detail, ctx| detail.set_theme(theme, ctx));
    }

    /// Fresh attachable projects after the open projects changed while the
    /// page stayed visible; an open detail panel picks them up at once.
    /// Unchanged lists skip the repaint.
    pub fn set_available_projects(&mut self, projects: Vec<ProjectRef>, ctx: &mut ViewContext<Self>) {
        if projects != self.available_projects {
            self.available_projects = projects;
            self.push_detail(ctx);
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.menu = None;
    }

    /// Look at the server again, off the main thread.
    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(org) = self.org.clone() else {
            self.loading = false;
            ctx.notify();
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let previous = self.snapshot.clone();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || data::load(&org, previous.as_ref()))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, loaded, ctx| {
                if view.generation != generation {
                    return;
                }
                view.loading = false;
                match loaded {
                    Ok(snapshot) => {
                        view.error = None;
                        view.selected.retain(|id| snapshot.tasks.iter().any(|task| &task.id == id));
                        view.snapshot = Some(snapshot);
                    }
                    Err(error) => view.error = Some(error),
                }
                ctx.notify();
                view.schedule_refresh(ctx);
            },
        );
        // Trackers have no server cursor: re-ask on the same tick while
        // their page stays visible. The loading flag guards overlapping
        // asks, and each answer supersedes the last by generation.
        if self.visible && matches!(self.source, Source::Tracker(_)) && !self.tracker_loading {
            self.load_tracker(ctx);
        }
    }

    fn schedule_refresh(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.visible || self.refresh_scheduled {
            return;
        }
        self.refresh_scheduled = true;
        ctx.spawn(Timer::after(REFRESH), |view, _, ctx| {
            view.refresh_scheduled = false;
            if view.visible {
                view.refresh(ctx);
            }
        });
    }

    /// A task the server sent back after a change replaces the old one at
    /// once, before the next look.
    fn apply(&mut self, task: Task) {
        let snapshot = self.snapshot.get_or_insert_with(Default::default);
        snapshot.seen_ms = snapshot.seen_ms.max(task.updated_at_ms);
        match snapshot.tasks.iter_mut().find(|known| known.id == task.id) {
            Some(known) => *known = task,
            None => snapshot.tasks.push(task),
        }
    }

    /// Run a change off the main thread; its answer replaces the task.
    fn change(
        &mut self,
        ctx: &mut ViewContext<Self>,
        work: impl FnOnce(&str) -> Result<Task, String> + Send + 'static,
        done: impl FnOnce(&mut Self, &mut ViewContext<Self>) + 'static,
    ) {
        let Some(org) = self.org.clone() else {
            return;
        };
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || work(&org))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, result, ctx| {
                match result {
                    Ok(task) => {
                        view.error = None;
                        view.apply(task);
                    }
                    Err(error) => view.error = Some(error),
                }
                done(view, ctx);
                ctx.notify();
            },
        );
    }

    fn move_task(&mut self, id: String, status: Status, ctx: &mut ViewContext<Self>) {
        let Some(task) = self.task(&id) else {
            return;
        };
        if task.status == status {
            return;
        }
        // Show it in its new place right away; the server's answer follows.
        let mut moved = task.clone();
        moved.status = status;
        self.apply(moved);
        let done_id = id.clone();
        self.change(
            ctx,
            move |org| data::set_status(org, &id, status),
            move |view, ctx| {
                view.sync_linked_status(&done_id, status, ctx);
            },
        );
    }

    /// Push a native status change to linked tracker items, best effort and
    /// only once the native change itself stuck (a server error leaves the
    /// provider alone). Providers with no such state are skipped; real
    /// failures surface as a dismissible page error.
    fn sync_linked_status(&mut self, id: &str, status: Status, ctx: &mut ViewContext<Self>) {
        if self.error.is_some() {
            return;
        }
        let Some(task) = self.task(id).cloned() else {
            return;
        };
        if task.status != status {
            return;
        }
        let items: Vec<(Provider, String)> = task
            .links_of(LinkKind::Item)
            .filter_map(|link| {
                let (provider, key) = link.reference.split_once(':')?;
                Provider::from_key(provider.trim()).map(|provider| (provider, key.trim().to_string()))
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let root = self.root.clone().unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::integrations::tracker::push_status(&root, &items, status)
                })
                .await
                .map_err(|error| error.to_string())
            },
            move |view, result: Result<Vec<String>, String>, ctx| match result {
                Ok(failures) if failures.is_empty() => {}
                Ok(failures) => {
                    view.error = Some(format!("Tracker update failed: {}", failures.join("; ")));
                    ctx.notify();
                }
                Err(error) => {
                    view.error = Some(error);
                    ctx.notify();
                }
            },
        );
    }

    fn open_new(&mut self, ctx: &mut ViewContext<Self>) {
        self.item_draft = None;
        self.draft = Some(Draft {
            status: match self.status_filter {
                StatusFilter::Only(status) => status,
                _ => Status::Todo,
            },
            priority: Priority::None,
            assignee: (self.assignee == AssigneeFilter::Me).then(|| self.me.clone()).flatten(),
            saving: false,
        });
        for editor in [&self.title_editor, &self.description_editor] {
            ctx.update_view(editor, |editor, ctx| editor.set_text("", ctx));
        }
        ctx.focus(&self.title_editor);
    }

    fn submit_new(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(draft) = self.draft.as_mut().filter(|draft| !draft.saving) else {
            return;
        };
        let title = ctx.update_view(&self.title_editor, |editor, _| editor.get_text().trim().to_string());
        if title.is_empty() {
            ctx.focus(&self.title_editor);
            return;
        }
        let description = ctx.update_view(&self.description_editor, |editor, _| editor.get_text().trim().to_string());
        draft.saving = true;
        let new = NewTask {
            title,
            description: (!description.is_empty()).then_some(description),
            status: Some(draft.status),
            priority: Some(draft.priority),
            assignee_id: draft.assignee.clone(),
            labels: self.label.clone().map(|label| vec![label]),
            ..Default::default()
        };
        self.change(ctx, move |org| data::create(org, &new), |view, _| {
            match &view.error {
                // Keep what was typed so it can be sent again.
                Some(_) => {
                    if let Some(draft) = view.draft.as_mut() {
                        draft.saving = false;
                    }
                }
                None => view.draft = None,
            }
        });
    }

    fn open_item_form(&mut self, ctx: &mut ViewContext<Self>) {
        self.draft = None;
        self.item_draft = Some(ItemDraft { saving: false });
        for editor in [&self.title_editor, &self.description_editor, &self.item_place_editor] {
            ctx.update_view(editor, |editor, ctx| editor.set_text("", ctx));
        }
        ctx.focus(&self.title_editor);
    }

    fn submit_item_form(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(provider) = self.tracker_provider() else {
            return;
        };
        let Some(draft) = self.item_draft.as_mut().filter(|draft| !draft.saving) else {
            return;
        };
        let title = ctx.update_view(&self.title_editor, |editor, _| editor.get_text().trim().to_string());
        if title.is_empty() {
            ctx.focus(&self.title_editor);
            return;
        }
        let description = ctx.update_view(&self.description_editor, |editor, _| editor.get_text().trim().to_string());
        let place = ctx.update_view(&self.item_place_editor, |editor, _| {
            let place = editor.get_text().trim().to_string();
            (!place.is_empty()).then_some(place)
        });
        draft.saving = true;
        let root = self.root.clone().unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || data::create_tracker_item(provider, &root, title, description, place))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, result: Result<TrackerItem, String>, ctx| {
                match result {
                    Ok(item) => {
                        view.tracker_error = None;
                        view.item_draft = None;
                        view.tracker_items.retain(|known| known.id() != item.id());
                        view.tracker_items.insert(0, item);
                    }
                    Err(error) => {
                        view.tracker_error = Some(error);
                        if let Some(draft) = view.item_draft.as_mut() {
                            draft.saving = false;
                        }
                    }
                }
                ctx.notify();
            },
        );
    }
}

impl TasksView {
    pub(crate) fn tracker_item(&self, id: &str) -> Option<&TrackerItem> {
        self.tracker_items.iter().find(|item| item.id() == id)
    }

    /// The tracker whose items show, if any.
    pub(crate) fn tracker_provider(&self) -> Option<Provider> {
        match self.source {
            Source::Tracker(provider) => Some(provider),
            Source::Dragent => None,
        }
    }

    /// The native task an item was already imported as, if any: the task
    /// linked back to it. Importing again opens this instead of
    /// duplicating, wherever the import happened from.
    pub(crate) fn imported_task(&self, item: &TrackerItem) -> Option<Task> {
        let reference = dragent_tasks::item_reference(item.provider.key(), &item.key);
        self.tasks().iter().find(|task| {
            task.links_of(LinkKind::Item).any(|link| link.reference == reference)
        }).cloned()
    }

    /// The open tracker's items the filters let through, by status.
    pub(crate) fn visible_items(&self) -> Vec<&TrackerItem> {
        let words: Vec<String> = self.search.split_whitespace().map(str::to_lowercase).collect();
        let mut items: Vec<&TrackerItem> = self
            .tracker_items
            .iter()
            .filter(|item| self.status_filter.shows(item.status))
            .filter(|item| {
                let haystack = format!("{} {}", item.key, item.title).to_lowercase();
                words.iter().all(|word| haystack.contains(word))
            })
            .collect();
        items.sort_by(|a, b| a.status.cmp(&b.status).then(b.updated_at_ms.cmp(&a.updated_at_ms)));
        items
    }

    /// Which connected trackers can show their items here (reading their
    /// connections can touch the keychain: off the main thread).
    fn load_providers(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            async move { tokio::task::spawn_blocking(crate::integrations::tracker::connected_trackers).await.unwrap_or_default() },
            |view, providers, ctx| {
                if let Source::Tracker(provider) = view.source {
                    if !providers.contains(&provider) {
                        view.source = Source::Dragent;
                    }
                }
                view.providers = providers;
                ctx.notify();
            },
        );
    }

    /// Ask the open tracker for its items.
    fn load_tracker(&mut self, ctx: &mut ViewContext<Self>) {
        let Source::Tracker(provider) = self.source else {
            return;
        };
        self.tracker_generation = self.tracker_generation.wrapping_add(1);
        let generation = self.tracker_generation;
        self.tracker_loading = true;
        let root = self.root.clone().unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || crate::integrations::tracker::list(provider, &root, ""))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, listed, ctx| {
                if view.tracker_generation != generation {
                    return;
                }
                view.tracker_loading = false;
                match listed {
                    Ok(items) => {
                        view.tracker_error = None;
                        view.tracker_items = items;
                    }
                    Err(error) => view.tracker_error = Some(error),
                }
                ctx.notify();
            },
        );
    }

    /// Move a tracker item to another status in its service.
    fn move_item(&mut self, id: String, status: Status, ctx: &mut ViewContext<Self>) {
        let Some(item) = self.tracker_items.iter_mut().find(|item| item.id() == id) else {
            return;
        };
        if item.status == status {
            return;
        }
        let previous = item.status;
        item.status = status;
        let moved = item.clone();
        let root = self.root.clone().unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || crate::integrations::tracker::set_status(&root, &moved, status))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, result, ctx| {
                if let Err(error) = result {
                    // Put it back where the service still has it.
                    if let Some(item) = view.tracker_items.iter_mut().find(|item| item.id() == id) {
                        item.status = previous;
                    }
                    view.tracker_error = Some(error);
                }
                ctx.notify();
            },
        );
    }

    /// Open a task's page over the list.
    fn open_task(&mut self, id: String, ctx: &mut ViewContext<Self>) {
        self.menu = None;
        self.open = Some(OpenTask {
            id: id.clone(),
            detail: None,
            busy: true,
            error: None,
        });
        self.load_detail(ctx);
    }

    /// Fetch the open task with its comments and history, off the main
    /// thread.
    fn load_detail(&mut self, ctx: &mut ViewContext<Self>) {
        let (Some(org), Some(open)) = (self.org.clone(), self.open.as_ref()) else {
            return;
        };
        let id = open.id.clone();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || data::get_detail(&org, &id))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, loaded, ctx| {
                let Some(open) = view.open.as_mut() else {
                    return;
                };
                open.busy = false;
                match loaded {
                    Ok(detail) if detail.task.id == open.id || detail.task.key == open.id => {
                        open.error = None;
                        let task = detail.task.clone();
                        open.detail = Some(detail);
                        view.apply(task);
                    }
                    Ok(_) => {}
                    Err(error) => open.error = Some(error),
                }
                view.push_detail(ctx);
                ctx.notify();
            },
        );
    }

    /// Hand the detail view what it shows.
    fn push_detail(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let Some(detail) = open.detail.clone() else {
            return;
        };
        let device = crate::settings::AppSettings::device_id();
        let openable_links = detail
            .task
            .links_of(LinkKind::Workspace)
            .filter(|link| dragent_tasks::parse_workspace_reference(&link.reference).is_some_and(|(machine, _)| machine == device))
            .map(|link| link.id.clone())
            .collect();
        let props = TaskDetailProps {
            detail,
            members: self.members().to_vec(),
            me: self.me.clone(),
            known_labels: self.labels(),
            openable_links,
            projects: self.available_projects.clone(),
            busy: open.busy,
            error: open.error.clone(),
        };
        ctx.update_view(&self.detail_view, |detail, ctx| detail.set_props(props, ctx));
    }

    /// Run a change to the open task off the main thread, then show it again.
    fn change_open(&mut self, ctx: &mut ViewContext<Self>, work: impl FnOnce(&str, &str) -> Result<(), String> + Send + 'static) {
        let (Some(org), Some(open)) = (self.org.clone(), self.open.as_mut()) else {
            return;
        };
        open.busy = true;
        let id = open.id.clone();
        self.push_detail(ctx);
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || work(&org, &id))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, result, ctx| {
                if let Some(open) = view.open.as_mut() {
                    open.error = result.err();
                }
                view.load_detail(ctx);
            },
        );
    }

    fn patch_open(&mut self, patch: TaskPatch, ctx: &mut ViewContext<Self>) {
        self.change_open(ctx, move |org, id| data::update(org, id, &patch).map(|_| ()));
    }

    /// Set the open task's status, then push it to linked tracker items
    /// the way board moves do.
    fn patch_status_open(&mut self, status: Status, ctx: &mut ViewContext<Self>) {
        let (Some(org), Some(open)) = (self.org.clone(), self.open.as_mut()) else {
            return;
        };
        let patch = TaskPatch { status: Some(status), ..Default::default() };
        let id = open.id.clone();
        let done_id = id.clone();
        open.busy = true;
        self.push_detail(ctx);
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || data::update(&org, &id, &patch).map(|_| ()))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, result: Result<(), String>, ctx| {
                if result.is_ok() {
                    view.sync_linked_status(&done_id, status, ctx);
                }
                if let Some(open) = view.open.as_mut() {
                    open.error = result.err();
                }
                view.load_detail(ctx);
            },
        );
    }

    /// Attach (or detach) a project, remembering the choice per tracker
    /// project so the next import from the same place attaches on its own.
    fn patch_project_open(&mut self, project: Option<ProjectRef>, ctx: &mut ViewContext<Self>) {
        let (Some(org), Some(open)) = (self.org.clone(), self.open.as_mut()) else {
            return;
        };
        let references: Vec<String> = open
            .detail
            .as_ref()
            .map(|detail| {
                detail
                    .task
                    .links_of(LinkKind::Item)
                    .map(|link| link.reference.clone())
                    .collect()
            })
            .unwrap_or_default();
        let patch = TaskPatch { project: Some(project.clone()), ..Default::default() };
        let id = open.id.clone();
        open.busy = true;
        self.push_detail(ctx);
        let org2 = org.clone();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || data::update(&org, &id, &patch).map(|_| ()))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |view, result: Result<(), String>, ctx| {
                if result.is_ok() {
                    crate::tasks::project_hints::remember(&org2, &references, project.as_ref());
                }
                if let Some(open) = view.open.as_mut() {
                    open.error = result.err();
                }
                view.load_detail(ctx);
            },
        );
    }

    fn on_detail_event(&mut self, event: &TaskDetailEvent, ctx: &mut ViewContext<Self>) {
        let task = self.open.as_ref().and_then(|open| open.detail.as_ref()).map(|detail| detail.task.clone());
        match event {
            TaskDetailEvent::Back => {
                self.open = None;
                self.refresh(ctx);
            }
            TaskDetailEvent::Start => {
                if let Some(task) = task {
                    ctx.emit(TasksEvent::Start(vec![task]));
                }
            }
            TaskDetailEvent::OpenLink(link) => match (link.kind, &link.url) {
                (LinkKind::Workspace, _) => {
                    if let Some(task) = task {
                        ctx.emit(TasksEvent::OpenWorkspace(task));
                    }
                }
                (_, Some(url)) => ctx.emit(TasksEvent::OpenUrl(url.clone())),
                (_, None) => {}
            },
            TaskDetailEvent::OpenUrl(url) => ctx.emit(TasksEvent::OpenUrl(url.clone())),
            TaskDetailEvent::CopyKey | TaskDetailEvent::CopyLink => {
                if let Some(task) = task {
                    let text = match event {
                        TaskDetailEvent::CopyKey => task.key.clone(),
                        _ => format!("{}: {}", task.key, task.title),
                    };
                    ctx.clipboard()
                        .write(warpui_core::clipboard::ClipboardContent::plain_text(text));
                }
            }
            TaskDetailEvent::Delete => {
                let Some(task) = task else {
                    return;
                };
                let id = task.id.clone();
                self.open = None;
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.tasks.retain(|known| known.id != id);
                }
                let Some(org) = self.org.clone() else {
                    return;
                };
                ctx.spawn(
                    async move {
                        tokio::task::spawn_blocking(move || data::delete_task(&org, &id))
                            .await
                            .map_err(|error| error.to_string())?
                    },
                    |view, result, ctx| {
                        if let Err(error) = result {
                            view.error = Some(error);
                        }
                        view.refresh(ctx);
                    },
                );
            }
            TaskDetailEvent::Rename(title) => self.patch_open(TaskPatch { title: Some(title.clone()), ..Default::default() }, ctx),
            TaskDetailEvent::SaveDescription(text) => {
                self.patch_open(TaskPatch { description: Some(text.clone()), ..Default::default() }, ctx)
            }
            TaskDetailEvent::SetStatus(status) => self.patch_status_open(*status, ctx),
            TaskDetailEvent::SetPriority(priority) => {
                self.patch_open(TaskPatch { priority: Some(*priority), ..Default::default() }, ctx)
            }
            TaskDetailEvent::SetAssignee(assignee) => {
                self.patch_open(TaskPatch { assignee_id: Some(assignee.clone()), ..Default::default() }, ctx)
            }
            TaskDetailEvent::SetProject(project) => {
                self.patch_project_open(project.clone(), ctx)
            }
            TaskDetailEvent::SetDue(due) => self.patch_open(TaskPatch { due_at_ms: Some(*due), ..Default::default() }, ctx),
            TaskDetailEvent::AddLabel(label) | TaskDetailEvent::RemoveLabel(label) => {
                let Some(task) = task else {
                    return;
                };
                let mut labels = task.labels.clone();
                let label = label.trim().to_lowercase();
                if matches!(event, TaskDetailEvent::AddLabel(_)) {
                    if label.is_empty() || labels.contains(&label) {
                        return;
                    }
                    labels.push(label);
                } else {
                    labels.retain(|known| *known != label);
                }
                self.patch_open(TaskPatch { labels: Some(labels), ..Default::default() }, ctx);
            }
            TaskDetailEvent::AddComment(body) => {
                let body = body.clone();
                self.change_open(ctx, move |org, id| data::add_comment(org, id, &body));
            }
            TaskDetailEvent::EditComment { id: comment, body } => {
                let (comment, body) = (comment.clone(), body.clone());
                self.change_open(ctx, move |org, id| data::edit_comment(org, id, &comment, &body));
            }
            TaskDetailEvent::DeleteComment(comment) => {
                let comment = comment.clone();
                self.change_open(ctx, move |org, id| data::delete_comment(org, id, &comment));
            }
        }
        ctx.notify();
    }
}

/// The signed-in organisation and user, when there are both.
pub(crate) fn signed_in(app: &AppContext) -> (Option<String>, Option<String>) {
    let me = crate::settings::AccountStore::handle(app)
        .read(app, |store, _| store.data.as_ref().map(|data| data.user.id.clone()))
        .filter(|id| !id.is_empty());
    let org = me.as_ref().and_then(|_| {
        crate::organisation_context::OrganisationContext::handle(app)
            .read(app, |context, _| {
                context
                    .active()
                    .filter(|active| active.context == dragent_paths::organisation_id())
                    .map(|active| active.organisation_id.clone())
            })
            .filter(|id| !id.is_empty())
    });
    (org, me)
}

/// The page's filters, as one test for a task.
pub(crate) struct Filter<'a> {
    pub status: StatusFilter,
    pub assignee: AssigneeFilter,
    pub me: Option<&'a str>,
    pub label: Option<&'a str>,
    pub words: &'a str,
}

impl Filter<'_> {
    pub(crate) fn matches(&self, task: &Task) -> bool {
        if !self.status.shows(task.status) {
            return false;
        }
        let assignee = task.assignee.as_ref().map(|person| person.id.as_str());
        let assigned_ok = match self.assignee {
            AssigneeFilter::Anyone => true,
            AssigneeFilter::Me => self.me.is_some() && assignee == self.me,
            AssigneeFilter::Unassigned => assignee.is_none(),
        };
        if !assigned_ok {
            return false;
        }
        if let Some(label) = self.label {
            if !task.labels.iter().any(|known| known.eq_ignore_ascii_case(label)) {
                return false;
            }
        }
        let haystack = format!("{} {} {}", task.key, task.title, task.description).to_lowercase();
        self.words
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

/// Status order, then most urgent, then most recently changed.
pub(crate) fn sort_tasks(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| {
        a.status
            .cmp(&b.status)
            .then(a.priority.rank().cmp(&b.priority.rank()))
            .then(b.updated_at_ms.cmp(&a.updated_at_ms))
    });
}

impl View for TasksView {
    fn ui_name() -> &'static str {
        "TasksView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        render::page(self, app)
    }
}

impl TypedActionView for TasksView {
    type Action = TasksAction;

    fn handle_action(&mut self, action: &TasksAction, ctx: &mut ViewContext<Self>) {
        match action {
            TasksAction::Refresh => match self.source {
                Source::Dragent => {
                    self.loading = self.snapshot.is_none();
                    self.refresh(ctx);
                }
                Source::Tracker(_) => self.load_tracker(ctx),
            },
            TasksAction::SetSource(source) => {
                if self.source != *source {
                    self.source = *source;
                    self.selected.clear();
                    self.draft = None;
                    self.item_draft = None;
                    self.tracker_items.clear();
                    self.tracker_error = None;
                    if matches!(source, Source::Tracker(_)) {
                        self.load_tracker(ctx);
                    }
                }
            }
            TasksAction::OpenItem(id) => {
                if let Some(item) = self.tracker_item(id) {
                    ctx.emit(TasksEvent::OpenUrl(item.url.clone()));
                }
            }
            TasksAction::StartItem(id) => {
                if let Some(item) = self.tracker_item(id).cloned() {
                    ctx.emit(TasksEvent::StartItem { item, root: self.root.clone() });
                }
            }
            TasksAction::ImportItem(id) => {
                let Some(item) = self.tracker_item(id).cloned() else {
                    return;
                };
                // Already imported, here or elsewhere: open that task
                // instead of duplicating it.
                if let Some(task) = self.imported_task(&item) {
                    self.open_task(task.id, ctx);
                    return;
                }
                // change() drops silently without an org, leaving no
                // callback to clear the in-flight mark.
                if self.org.is_none() || !self.importing.insert(id.clone()) {
                    return;
                }
                    let importing = id.clone();
                    self.change(
                        ctx,
                        move |org| data::import_item(org, &item),
                        move |view, _| {
                            view.importing.remove(&importing);
                        },
                    );
            }
            TasksAction::MoveItem { id, status } => self.move_item(id.clone(), *status, ctx),
            TasksAction::SetStatusFilter(filter) => {
                self.status_filter = *filter;
                self.menu = None;
            }
            TasksAction::SetAssignee(assignee) => self.assignee = *assignee,
            TasksAction::SetLabel(label) => self.label = label.clone(),
            TasksAction::SetLayout(layout) => self.layout = *layout,
            TasksAction::ToggleMenu(menu) => {
                self.menu = (self.menu.as_ref() != Some(menu)).then(|| menu.clone());
            }
            TasksAction::CloseMenus => self.menu = None,
            TasksAction::ToggleGroup(status) => {
                if !self.collapsed.remove(status) {
                    self.collapsed.insert(*status);
                }
            }
            TasksAction::ToggleSelect(id) => {
                if !self.selected.remove(id) {
                    self.selected.insert(id.clone());
                }
            }
            TasksAction::ClearSelection => self.selected.clear(),
            TasksAction::Open(id) => self.open_task(id.clone(), ctx),
            TasksAction::Move { id, status } => self.move_task(id.clone(), *status, ctx),
            TasksAction::SetSelectedStatus(status) => {
                self.menu = None;
                let ids: Vec<String> = self.selected.iter().cloned().collect();
                for id in ids {
                    self.move_task(id, *status, ctx);
                }
                self.selected.clear();
            }
            TasksAction::Start(ids) => {
                let tasks: Vec<Task> = ids.iter().filter_map(|id| self.task(id).cloned()).collect();
                if !tasks.is_empty() {
                    self.selected.clear();
                    ctx.emit(TasksEvent::Start(tasks));
                }
            }
            TasksAction::NewTask => self.open_new(ctx),
            TasksAction::CancelNew => {
                self.draft = None;
                self.menu = None;
            }
            TasksAction::NewItem => self.open_item_form(ctx),
            TasksAction::CancelNewItem => {
                self.item_draft = None;
            }
            TasksAction::SubmitNewItem => self.submit_item_form(ctx),
            TasksAction::SetNewStatus(status) => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.status = *status;
                }
                self.menu = None;
            }
            TasksAction::SetNewPriority(priority) => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.priority = *priority;
                }
                self.menu = None;
            }
            TasksAction::SetNewAssignee(assignee) => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.assignee = assignee.clone();
                }
                self.menu = None;
            }
            TasksAction::SubmitNew => self.submit_new(ctx),
            TasksAction::DismissError => self.error = None,
        }
        ctx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_task_retry_clears_the_previous_error() {
        use warpui::{App, ReadView, UpdateView};
        App::test((), |mut app| async move {
            let (_, page) = app.add_window(warpui::platform::WindowStyle::NotStealFocus, |ctx| {
                TasksView::new(ctx, FamilyId(0), FamilyId(0), Theme::dark())
            });
            let (failed, failure) = futures::channel::oneshot::channel();
            page.update(&mut app, |view, ctx| {
                view.org = Some("test-org".into());
                view.change(
                    ctx,
                    |_| Err("could not create the task".into()),
                    move |_, _| { let _ = failed.send(()); },
                );
            });
            failure.await.unwrap();
            assert!(page.read(&app, |view, _| view.error.is_some()));

            let (saved, success) = futures::channel::oneshot::channel();
            page.update(&mut app, |view, ctx| {
                view.change(
                    ctx,
                    |_| Ok(task("DRA-1", Status::Todo, Priority::None, None, 1)),
                    move |view, _| {
                        let _ = saved.send((view.error.clone(), view.task("dra-1").is_some()));
                    },
                );
            });
            let (error, created) = success.await.unwrap();
            assert_eq!(error, None, "the retry completion must see a successful save");
            assert!(created);
        });
    }

    fn task(key: &str, status: Status, priority: Priority, assignee: Option<&str>, updated: i64) -> Task {
        Task {
            id: key.to_lowercase(),
            key: key.into(),
            number: 1,
            title: format!("{key} title"),
            description: String::new(),
            status,
            priority,
            assignee: assignee.map(|id| Person {
                id: id.into(),
                ..Default::default()
            }),
            creator: None,
            project: None,
            labels: vec!["bug".into()],
            due_at_ms: None,
            created_at_ms: 0,
            updated_at_ms: updated,
            started_at_ms: None,
            completed_at_ms: None,
            links: Vec::new(),
            comment_count: 0,
        }
    }

    #[test]
    fn filters_combine_status_assignee_label_and_words() {
        let mine = task("DRA-1", Status::Todo, Priority::None, Some("me"), 1);
        let done = task("DRA-2", Status::Done, Priority::None, Some("me"), 1);
        let theirs = task("DRA-3", Status::Backlog, Priority::None, Some("them"), 1);
        let nobody = task("DRA-4", Status::InProgress, Priority::None, None, 1);
        let filter = |status, assignee, words| Filter {
            status,
            assignee,
            me: Some("me"),
            label: None,
            words,
        };
        assert!(filter(StatusFilter::Open, AssigneeFilter::Me, "").matches(&mine));
        assert!(!filter(StatusFilter::Open, AssigneeFilter::Me, "").matches(&done));
        assert!(filter(StatusFilter::All, AssigneeFilter::Me, "").matches(&done));
        assert!(!filter(StatusFilter::Active, AssigneeFilter::Anyone, "").matches(&theirs), "backlog isn't active");
        assert!(filter(StatusFilter::Open, AssigneeFilter::Unassigned, "").matches(&nobody));
        assert!(filter(StatusFilter::Open, AssigneeFilter::Anyone, "dra-4 TITLE").matches(&nobody));
        assert!(!filter(StatusFilter::Open, AssigneeFilter::Anyone, "dra-4 missing").matches(&nobody));
        let labelled = Filter {
            label: Some("BUG"),
            ..filter(StatusFilter::All, AssigneeFilter::Anyone, "")
        };
        assert!(labelled.matches(&mine));
        let signed_out = Filter {
            me: None,
            ..filter(StatusFilter::All, AssigneeFilter::Me, "")
        };
        assert!(!signed_out.matches(&mine), "no one is 'me' while signed out");
    }

    #[test]
    fn tasks_sort_by_status_then_urgency_then_recency() {
        let a = task("DRA-1", Status::InProgress, Priority::Low, None, 5);
        let b = task("DRA-2", Status::Todo, Priority::None, None, 1);
        let c = task("DRA-3", Status::InProgress, Priority::Urgent, None, 1);
        let d = task("DRA-4", Status::InProgress, Priority::Low, None, 9);
        let mut tasks = vec![&a, &b, &c, &d];
        sort_tasks(&mut tasks);
        let keys: Vec<&str> = tasks.iter().map(|task| task.key.as_str()).collect();
        assert_eq!(keys, ["DRA-2", "DRA-3", "DRA-4", "DRA-1"]);
    }
}
