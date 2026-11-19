//! The task detail view: one task with its title, description, comments
//! and activity, beside a panel of its properties and links.
//!
//! The Tasks page mounts this when a task is opened. It feeds data through
//! [`TaskDetailView::set_props`] and handles the [`TaskDetailEvent`]s this
//! emits (navigation, saving, copying). Everything here is display plus the
//! local editing state for the title, description, comments and menus; the
//! page owns fetching, saving and navigation.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use chrono::{Local, TimeZone, Utc};
use dragent_tasks::{
    Activity, ActivityKind, ActorKind, Comment, LinkKind, Person, Priority, ProjectRef, Status,
    Task, TaskDetail, TaskLink,
};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ClippedScrollable, ClippedScrollStateHandle, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex,
    MainAxisAlignment, MainAxisSize, ParentElement, ScrollbarWidth, Shrinkable, Wrap,
};
use warpui::fonts::FamilyId;
use warpui::{
    AppContext, Entity, EntityId, TypedActionView, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::{InputEditor, InputEditorAction, InputEditorEvent, InputMode};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{line, medium, semibold, text};
use crate::terminal::render::markdown::render_markdown_document;
use crate::theme::{text_size, Theme};

/// The page the detail lives on is this wide at most; the main column takes
/// about 760 of it and the side panel about 280.
const PAGE_MAX_WIDTH: f32 = 1060.0;
const MAIN_MAX_WIDTH: f32 = 760.0;
const SIDE_WIDTH: f32 = 280.0;

/// What the Tasks page shows this view. `openable_links` holds the ids of
/// the workspace links this machine can open; the rest live on other
/// computers.
#[derive(Clone, Debug)]
pub struct TaskDetailProps {
    pub detail: TaskDetail,
    pub members: Vec<Person>,
    pub me: Option<String>,
    pub known_labels: Vec<String>,
    pub openable_links: HashSet<String>,
    /// Local projects the task can attach to, so starting it later opens
    /// a workspace in the right checkout.
    pub projects: Vec<ProjectRef>,
    pub busy: bool,
    pub error: Option<String>,
}

/// What the view asks the page to do. Copying and opening links stay here as
/// events so the view never touches the clipboard or the browser itself.
#[derive(Clone, Debug)]
pub enum TaskDetailEvent {
    Back,
    Start,
    OpenLink(TaskLink),
    OpenUrl(String),
    CopyKey,
    CopyLink,
    Delete,
    Rename(String),
    SaveDescription(String),
    SetStatus(Status),
    SetPriority(Priority),
    SetAssignee(Option<String>),
    SetProject(Option<ProjectRef>),
    SetDue(Option<i64>),
    AddLabel(String),
    RemoveLabel(String),
    AddComment(String),
    EditComment { id: String, body: String },
    DeleteComment(String),
}

/// Which inline menu is open. Only one shows at a time; each lives in the
/// page flow (never floating) so the scrolling page keeps painting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Overflow,
    Status,
    Priority,
    Assignee,
    Project,
    Due,
    Labels,
}

#[derive(Clone, Debug)]
pub enum TaskDetailAction {
    Back,
    Primary,
    CopyKey,
    CopyLink,
    ToggleMenu(Menu),
    ConfirmDelete,
    DismissError,
    EditTitle,
    SaveTitle,
    CancelTitle,
    EditDescription,
    SaveDescription,
    CancelDescription,
    PickStatus(Status),
    PickPriority(Priority),
    PickAssignee(Option<String>),
    PickProject(Option<ProjectRef>),
    PickDue(Option<i64>),
    /// Show the calendar a month earlier (-1) or later (+1).
    DueMonth(i32),
    AddLabel(String),
    SubmitLabel,
    RemoveLabel(String),
    SendComment,
    EditComment(String),
    SaveComment,
    CancelCommentEdit,
    DeleteComment(String),
    ToggleComments,
    OpenLink(TaskLink),
    OpenUrl(String),
}

/// The primary button: starting needs no workspace yet, otherwise the task
/// already has one to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrimaryAction {
    Start,
    OpenWorkspace,
}

pub(crate) fn primary_action(task: &Task) -> PrimaryAction {
    if task.links_of(LinkKind::Workspace).next().is_some() {
        PrimaryAction::OpenWorkspace
    } else {
        PrimaryAction::Start
    }
}

/// "3h ago", "2d ago"; older entries get a date since the count stops
/// meaning much.
pub(crate) fn relative_time(now_ms: i64, at_ms: i64) -> String {
    let secs = (now_ms - at_ms).max(0) / 1000;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 30 * 86_400 {
        format!("{}d ago", secs / 86_400)
    } else {
        format_day(at_ms)
    }
}

/// "Jun 15, 2025" in this computer's time.
pub(crate) fn format_day(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|time| time.format("%b %-d, %Y").to_string())
        .unwrap_or_default()
}

/// Who a history entry speaks of: a person by name, an agent through its
/// person, Dragent itself for system entries.
pub(crate) fn actor_name(activity: &Activity) -> String {
    let base = activity
        .actor
        .as_ref()
        .map(|person| person.display().to_string())
        .unwrap_or_else(|| "Someone".to_string());
    match activity.actor_kind {
        ActorKind::Person => base,
        ActorKind::Agent => format!("{base}'s agent"),
        ActorKind::System => "Dragent".to_string(),
    }
}

fn value_text(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(|value| value.as_str().map(|text| text.to_string()))
}

fn status_label(key: &str) -> String {
    Status::from_key(key)
        .map(|status| status.label().to_string())
        .unwrap_or_else(|| key.to_string())
}

fn priority_label(key: &str) -> String {
    Priority::from_key(key)
        .map(|priority| priority.label().to_string())
        .unwrap_or_else(|| key.to_string())
}

/// An assignee value is a person object (or null when cleared); older
/// entries may hold just an id.
fn person_name(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    if let Some(id) = value.as_str() {
        return (!id.trim().is_empty()).then(|| id.to_string());
    }
    serde_json::from_value::<Person>(value.clone())
        .ok()
        .map(|person| person.display().to_string())
}

fn label_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|value| value.as_array())
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(|label| label.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// What a link value links to, in words the sentence can use.
fn link_what(value: Option<&serde_json::Value>, added: bool) -> String {
    let kind = value
        .and_then(|value| value.get("kind"))
        .and_then(|kind| kind.as_str());
    match kind {
        Some("workspace") if added => "linked a workspace".to_string(),
        Some("pull_request") if added => "linked a pull request".to_string(),
        Some("item") if added => "linked an item".to_string(),
        Some("workspace") => "removed a workspace link".to_string(),
        Some("pull_request") => "removed a pull request link".to_string(),
        Some("item") => "removed an item link".to_string(),
        _ if added => "added a link".to_string(),
        _ => "removed a link".to_string(),
    }
}

/// One quiet line per history entry, e.g. "Ama moved this from Todo to In
/// progress". The `from`/`to` values are the wire forms (status keys, person
/// objects, links); description entries carry none.
pub(crate) fn history_sentence(activity: &Activity) -> String {
    let who = actor_name(activity);
    let verb = match activity.kind {
        ActivityKind::Created => "created this".to_string(),
        ActivityKind::Title => match value_text(activity.to.as_ref()) {
            Some(title) if !title.trim().is_empty() => format!("changed the title to “{title}”"),
            _ => "changed the title".to_string(),
        },
        ActivityKind::Description => "edited the description".to_string(),
        ActivityKind::Status => {
            let from = value_text(activity.from.as_ref()).map(|key| status_label(&key));
            let to = value_text(activity.to.as_ref()).map(|key| status_label(&key));
            match (from, to) {
                (Some(from), Some(to)) => format!("moved this from {from} to {to}"),
                (None, Some(to)) => format!("set the status to {to}"),
                (Some(from), None) => format!("cleared the status (was {from})"),
                (None, None) => "changed the status".to_string(),
            }
        }
        ActivityKind::Priority => match value_text(activity.to.as_ref()) {
            Some(key) => format!("set the priority to {}", priority_label(&key)),
            None if activity.to.as_ref().is_some_and(|value| value.is_null()) => {
                "cleared the priority".to_string()
            }
            None => "changed the priority".to_string(),
        },
        ActivityKind::Assignee => {
            let from = person_name(activity.from.as_ref());
            let to = person_name(activity.to.as_ref());
            match (from, to) {
                (None, Some(to)) => format!("assigned this to {to}"),
                (Some(_), None) => "unassigned this".to_string(),
                (Some(from), Some(to)) if from != to => {
                    format!("reassigned this from {from} to {to}")
                }
                (Some(_), Some(to)) => format!("assigned this to {to}"),
                (None, None) => "changed the assignee".to_string(),
            }
        }
        ActivityKind::Labels => {
            let from = label_list(activity.from.as_ref());
            let to = label_list(activity.to.as_ref());
            let added: Vec<&String> = to.iter().filter(|label| !from.contains(label)).collect();
            let removed: Vec<&String> =
                from.iter().filter(|label| !to.contains(label)).collect();
            match (added.as_slice(), removed.as_slice()) {
                ([one], []) => format!("added the “{one}” label"),
                ([], [one]) => format!("removed the “{one}” label"),
                _ if from == to => "changed the labels".to_string(),
                _ if to.is_empty() => "removed all labels".to_string(),
                _ => format!("set the labels to {}", to.join(", ")),
            }
        }
        ActivityKind::Project => match activity.to.as_ref() {
            Some(value) if value.is_null() => "removed the project".to_string(),
            Some(value) => match serde_json::from_value::<ProjectRef>(value.clone()) {
                Ok(project) => format!("set the project to {}", project.label),
                Err(_) => "changed the project".to_string(),
            },
            None => "changed the project".to_string(),
        },
        ActivityKind::Due => match activity.to.as_ref() {
            Some(value) if value.is_null() => "removed the due date".to_string(),
            Some(value) => match value.as_i64() {
                Some(ms) => format!("set the due date to {}", format_day(ms)),
                None => "changed the due date".to_string(),
            },
            None => "changed the due date".to_string(),
        },
        ActivityKind::LinkAdded => link_what(activity.to.as_ref(), true),
        ActivityKind::LinkRemoved => link_what(activity.from.as_ref(), false),
        ActivityKind::Deleted => "deleted this".to_string(),
        ActivityKind::Restored => "restored this".to_string(),
    };
    format!("{who} {verb}")
}

/// History and comments merged oldest first; a history entry sorts before a
/// comment stamped at the same millisecond.
pub(crate) enum TimelineItem<'a> {
    History(&'a Activity),
    Comment(&'a Comment),
}

impl TimelineItem<'_> {
    pub(crate) fn at_ms(&self) -> i64 {
        match self {
            TimelineItem::History(activity) => activity.at_ms,
            TimelineItem::Comment(comment) => comment.created_at_ms,
        }
    }

    fn order(&self) -> u8 {
        match self {
            TimelineItem::History(_) => 0,
            TimelineItem::Comment(_) => 1,
        }
    }
}

pub(crate) fn timeline(detail: &TaskDetail) -> Vec<TimelineItem<'_>> {
    let mut items: Vec<TimelineItem> = detail
        .activity
        .iter()
        .map(TimelineItem::History)
        .chain(detail.comments.iter().map(TimelineItem::Comment))
        .collect();
    items.sort_by(|a, b| (a.at_ms(), a.order()).cmp(&(b.at_ms(), b.order())));
    items
}

/// The local (year, month) a moment falls in.
pub(crate) fn month_of(ms: i64) -> (i32, u32) {
    use chrono::Datelike;
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|time| (time.year(), time.month()))
        .unwrap_or((1970, 1))
}

/// `delta` months after (year, month).
pub(crate) fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let index = year * 12 + month as i32 - 1 + delta;
    (index.div_euclid(12), (index.rem_euclid(12) + 1) as u32)
}

/// A day picked on the calendar: local noon that day, so it reads as the
/// same day in every nearby time zone.
pub(crate) fn day_ms(year: i32, month: u32, day: u32) -> Option<i64> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    Local
        .from_local_datetime(&date.and_hms_opt(12, 0, 0)?)
        .earliest()
        .map(|time| time.timestamp_millis())
}

/// The weeks of a month for a calendar, Monday first: each day of the
/// month in its weekday's column, `None` before the 1st and after the end.
pub(crate) fn month_grid(year: i32, month: u32) -> Vec<[Option<u32>; 7]> {
    use chrono::Datelike;
    let Some(first) = chrono::NaiveDate::from_ymd_opt(year, month, 1) else {
        return Vec::new();
    };
    let (next_year, next_month) = shift_month(year, month, 1);
    let days = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .map(|next| (next - first).num_days() as u32)
        .unwrap_or(30);
    let offset = first.weekday().num_days_from_monday() as usize;
    let mut weeks = Vec::new();
    let mut week = [None; 7];
    let mut column = offset;
    for day in 1..=days {
        week[column] = Some(day);
        column += 1;
        if column == 7 {
            weeks.push(week);
            week = [None; 7];
            column = 0;
        }
    }
    if column > 0 {
        weeks.push(week);
    }
    weeks
}

/// A due-date preset counted from now.
pub(crate) fn due_preset_ms(days_later: i64, now_ms: i64) -> i64 {
    now_ms + days_later * 86_400_000
}

pub struct TaskDetailView {
    view_id: EntityId,
    ui_font: FamilyId,
    mono_font: FamilyId,
    theme: Theme,
    props: Option<TaskDetailProps>,
    task_id: Option<String>,
    editing_title: bool,
    editing_description: bool,
    editing_comment: Option<String>,
    open_menu: Option<Menu>,
    /// The month the due-date calendar shows (year, month 1-12); `None`
    /// shows the task's due month, else this month.
    due_month: Option<(i32, u32)>,
    /// Whether the full comment list shows; otherwise only the latest few
    /// do, behind a "show earlier" toggle.
    comments_expanded: bool,
    /// The error text dismissed locally; a new error still shows.
    error_dismissed: Option<String>,
    composer_draft: String,
    label_draft: String,
    title_editor: ViewHandle<InputEditor>,
    description_editor: ViewHandle<InputEditor>,
    comment_editor: ViewHandle<InputEditor>,
    composer_editor: ViewHandle<InputEditor>,
    label_editor: ViewHandle<InputEditor>,
    scroll: ClippedScrollStateHandle,
    mouse: RefCell<HashMap<String, MouseStateHandle>>,
}

fn plain_editor(
    ctx: &mut ViewContext<TaskDetailView>,
    placeholder: &str,
    ui_font: FamilyId,
    theme: Theme,
    key: &str,
) -> ViewHandle<InputEditor> {
    ctx.add_typed_action_view(|inner| {
        InputEditor::new(
            placeholder,
            ui_font,
            text_size(13.0),
            theme.main_text(),
            theme.disabled_text(),
            theme.accent,
            theme,
            InputMode::Plain { key: key.into() },
            inner.focus_handle(),
        )
    })
}

fn multiline_editor(
    ctx: &mut ViewContext<TaskDetailView>,
    placeholder: &str,
    ui_font: FamilyId,
    theme: Theme,
    key: &str,
    max_height: u32,
) -> ViewHandle<InputEditor> {
    ctx.add_typed_action_view(|inner| {
        InputEditor::new(
            placeholder,
            ui_font,
            text_size(13.0),
            theme.main_text(),
            theme.disabled_text(),
            theme.accent,
            theme,
            InputMode::MultiLine {
                key: key.into(),
                max_height,
            },
            inner.focus_handle(),
        )
    })
}

impl TaskDetailView {
    pub fn new(
        ctx: &mut ViewContext<Self>,
        ui_font: FamilyId,
        mono_font: FamilyId,
        theme: Theme,
    ) -> Self {
        let title_editor = ctx.add_typed_action_view(|inner| {
            InputEditor::new(
                "Task title",
                ui_font,
                text_size(16.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "task_detail_title".into(),
                },
                inner.focus_handle(),
            )
        });
        let description_editor = multiline_editor(
            ctx,
            "Describe the work…",
            ui_font,
            theme,
            "task_detail_description",
            320,
        );
        let comment_editor = multiline_editor(
            ctx,
            "Write a comment…",
            ui_font,
            theme,
            "task_detail_comment",
            240,
        );
        let composer_editor = multiline_editor(
            ctx,
            "Write a comment…",
            ui_font,
            theme,
            "task_detail_composer",
            160,
        );
        let label_editor = plain_editor(ctx, "New label…", ui_font, theme, "task_detail_label");
        ctx.subscribe_to_view(&composer_editor, |view, editor, _event, ctx| {
            let draft = ctx.update_view(&editor, |editor, _| editor.get_text().to_string());
            if draft != view.composer_draft {
                view.composer_draft = draft;
                ctx.notify();
            }
        });
        ctx.subscribe_to_view(&label_editor, |view, editor, _event, ctx| {
            let draft = ctx.update_view(&editor, |editor, _| editor.get_text().to_string());
            if draft != view.label_draft {
                view.label_draft = draft;
                ctx.notify();
            }
        });
        Self {
            view_id: ctx.view_id(),
            ui_font,
            mono_font,
            theme,
            props: None,
            task_id: None,
            editing_title: false,
            editing_description: false,
            editing_comment: None,
            open_menu: None,
            due_month: None,
            comments_expanded: false,
            error_dismissed: None,
            composer_draft: String::new(),
            label_draft: String::new(),
            title_editor,
            description_editor,
            comment_editor,
            composer_editor,
            label_editor,
            scroll: ClippedScrollStateHandle::default(),
            mouse: RefCell::default(),
        }
    }

    /// The view and its editors follow the app's theme.
    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        self.theme = theme;
        for editor in [
            &self.title_editor,
            &self.description_editor,
            &self.comment_editor,
            &self.composer_editor,
            &self.label_editor,
        ] {
            ctx.update_view(editor, |editor, ctx| editor.set_theme(theme, ctx));
        }
        ctx.notify();
    }

    /// Fresh data from the page. Switching tasks resets the local editing
    /// state; refreshing the same task leaves it alone so typing and open
    /// menus survive.
    pub fn set_props(&mut self, props: TaskDetailProps, ctx: &mut ViewContext<Self>) {
        let task_id = props.detail.task.id.clone();
        if self.task_id.as_deref() != Some(task_id.as_str()) {
            self.task_id = Some(task_id);
            self.editing_title = false;
            self.editing_description = false;
            self.editing_comment = None;
            self.open_menu = None;
            self.comments_expanded = false;
            self.composer_draft.clear();
            self.label_draft.clear();
            let composer = self.composer_editor.clone();
            ctx.update_view(&composer, |editor, editor_ctx| editor.set_text("", editor_ctx));
            let label = self.label_editor.clone();
            ctx.update_view(&label, |editor, editor_ctx| editor.set_text("", editor_ctx));
        }
        self.props = Some(props);
        ctx.notify();
    }

    fn mouse(&self, key: impl Into<String>) -> MouseStateHandle {
        self.mouse
            .borrow_mut()
            .entry(key.into())
            .or_default()
            .clone()
    }

    fn editor_text(
        &self,
        editor: &ViewHandle<InputEditor>,
        ctx: &mut ViewContext<Self>,
    ) -> String {
        let editor = editor.clone();
        ctx.update_view(&editor, |editor, _| editor.get_text().to_string())
    }

    fn set_editor_text(
        &self,
        editor: &ViewHandle<InputEditor>,
        text: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let editor = editor.clone();
        let text = text.to_string();
        ctx.update_view(&editor, |editor, editor_ctx| {
            editor.set_text(&text, editor_ctx)
        });
    }

    fn start_edit_title(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(props) = self.props.as_ref() else {
            return;
        };
        if props.busy {
            return;
        }
        self.open_menu = None;
        self.editing_title = true;
        self.set_editor_text(
            &self.title_editor.clone(),
            &props.detail.task.title,
            ctx,
        );
        let editor = self.title_editor.clone();
        ctx.focus(&editor);
        ctx.notify();
    }

    fn start_edit_description(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(props) = self.props.as_ref() else {
            return;
        };
        if props.busy {
            return;
        }
        self.open_menu = None;
        self.editing_description = true;
        self.set_editor_text(
            &self.description_editor.clone(),
            &props.detail.task.description,
            ctx,
        );
        let editor = self.description_editor.clone();
        ctx.focus(&editor);
        ctx.notify();
    }

    fn start_edit_comment(&mut self, id: &str, ctx: &mut ViewContext<Self>) {
        let body = self
            .props
            .as_ref()
            .and_then(|props| {
                props
                    .detail
                    .comments
                    .iter()
                    .find(|comment| comment.id == id)
            })
            .map(|comment| comment.body.clone());
        let Some(body) = body else {
            return;
        };
        if self.props.as_ref().is_some_and(|props| props.busy) {
            return;
        }
        self.editing_comment = Some(id.to_string());
        self.set_editor_text(&self.comment_editor.clone(), &body, ctx);
        let editor = self.comment_editor.clone();
        ctx.focus(&editor);
        ctx.notify();
    }
}

impl Entity for TaskDetailView {
    type Event = TaskDetailEvent;
}

impl View for TaskDetailView {
    fn ui_name() -> &'static str {
        "TaskDetailView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme;
        let Some(props) = self.props.as_ref() else {
            return Container::new(
                Align::new(text(self.ui_font, "Select a task.", 13.0, theme.sub_text())).finish(),
            )
            .with_background_color(theme.background)
            .finish();
        };
        let can = !props.busy;
        let now_ms = Utc::now().timestamp_millis();
        let mut page = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(self.top_bar(props, can));
        if let Some(message) = props
            .error
            .as_deref()
            .filter(|message| self.error_dismissed.as_deref() != Some(*message))
        {
            page = page.with_child(self.error_line(message, can));
        }
        if self.open_menu == Some(Menu::Overflow) {
            page = page.with_child(self.overflow_menu(can));
        }
        page = page.with_child(Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(24.0)
            .with_child(
                Expanded::new(
                    1.0,
                    ConstrainedBox::new(self.main_column(props, can, now_ms))
                        .with_max_width(MAIN_MAX_WIDTH)
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                ConstrainedBox::new(self.side_panel(props, can, now_ms))
                    .with_width(SIDE_WIDTH)
                    .finish(),
            )
            .finish());
        // The Align sits outside the width cap (as on the Automations
        // page) so the whole detail truly centers instead of hugging
        // the left on wide windows: Align takes the full width and
        // centers the capped page inside it.
        let content = Container::new(
            Align::new(
                ConstrainedBox::new(page.finish())
                    .with_max_width(PAGE_MAX_WIDTH)
                    .finish(),
            )
            .top_center()
            .finish(),
        )
        .with_padding_left(24.0)
        .with_padding_right(24.0)
        .with_padding_top(16.0)
        .with_padding_bottom(32.0)
        .finish();
        Container::new(scrollable(&theme, &self.scroll, content))
            .with_background_color(theme.background)
            .finish()
    }
}

impl TypedActionView for TaskDetailView {
    type Action = TaskDetailAction;

    fn handle_action(&mut self, action: &TaskDetailAction, ctx: &mut ViewContext<Self>) {
        match action {
            TaskDetailAction::Back => ctx.emit(TaskDetailEvent::Back),
            TaskDetailAction::Primary => {
                let link = self.props.as_ref().and_then(|props| {
                    props
                        .detail
                        .task
                        .links_of(LinkKind::Workspace)
                        .next()
                        .cloned()
                });
                match link {
                    Some(link) => ctx.emit(TaskDetailEvent::OpenLink(link)),
                    None => ctx.emit(TaskDetailEvent::Start),
                }
            }
            TaskDetailAction::CopyKey => ctx.emit(TaskDetailEvent::CopyKey),
            TaskDetailAction::CopyLink => ctx.emit(TaskDetailEvent::CopyLink),
            TaskDetailAction::ToggleMenu(menu) => {
                self.open_menu = (self.open_menu != Some(*menu)).then_some(*menu);
                ctx.notify();
            }
            TaskDetailAction::ConfirmDelete => {
                self.open_menu = None;
                ctx.emit(TaskDetailEvent::Delete);
                ctx.notify();
            }
            TaskDetailAction::DismissError => {
                self.error_dismissed = self.props.as_ref().and_then(|props| props.error.clone());
                ctx.notify();
            }
            TaskDetailAction::EditTitle => self.start_edit_title(ctx),
            TaskDetailAction::CancelTitle => {
                self.editing_title = false;
                ctx.notify();
            }
            TaskDetailAction::SaveTitle => {
                let current = self
                    .props
                    .as_ref()
                    .map(|props| props.detail.task.title.clone())
                    .unwrap_or_default();
                let renamed = self.editor_text(&self.title_editor.clone(), ctx);
                let renamed = renamed.trim().to_string();
                self.editing_title = false;
                // An empty title is invalid; keep showing the old one.
                if !renamed.is_empty() && renamed != current {
                    ctx.emit(TaskDetailEvent::Rename(renamed));
                }
                ctx.notify();
            }
            TaskDetailAction::EditDescription => self.start_edit_description(ctx),
            TaskDetailAction::CancelDescription => {
                self.editing_description = false;
                ctx.notify();
            }
            TaskDetailAction::SaveDescription => {
                let current = self
                    .props
                    .as_ref()
                    .map(|props| props.detail.task.description.clone())
                    .unwrap_or_default();
                let body = self.editor_text(&self.description_editor.clone(), ctx);
                let body = body.trim().to_string();
                self.editing_description = false;
                if body != current {
                    ctx.emit(TaskDetailEvent::SaveDescription(body));
                }
                ctx.notify();
            }
            TaskDetailAction::PickStatus(status) => {
                self.open_menu = None;
                ctx.emit(TaskDetailEvent::SetStatus(*status));
                ctx.notify();
            }
            TaskDetailAction::PickPriority(priority) => {
                self.open_menu = None;
                ctx.emit(TaskDetailEvent::SetPriority(*priority));
                ctx.notify();
            }
            TaskDetailAction::PickAssignee(assignee) => {
                self.open_menu = None;
                ctx.emit(TaskDetailEvent::SetAssignee(assignee.clone()));
                ctx.notify();
            }
            TaskDetailAction::PickProject(project) => {
                self.open_menu = None;
                ctx.emit(TaskDetailEvent::SetProject(project.clone()));
                ctx.notify();
            }
            TaskDetailAction::DueMonth(delta) => {
                let (year, month) = self.due_month.unwrap_or_else(|| {
                    let props_due = self.props.as_ref().and_then(|props| props.detail.task.due_at_ms);
                    month_of(props_due.unwrap_or_else(|| Utc::now().timestamp_millis()))
                });
                self.due_month = Some(shift_month(year, month, *delta));
                ctx.notify();
            }
            TaskDetailAction::PickDue(due) => {
                self.open_menu = None;
                self.due_month = None;
                ctx.emit(TaskDetailEvent::SetDue(*due));
                ctx.notify();
            }
            TaskDetailAction::AddLabel(label) => {
                ctx.emit(TaskDetailEvent::AddLabel(label.clone()));
            }
            TaskDetailAction::SubmitLabel => {
                let label = self.editor_text(&self.label_editor.clone(), ctx);
                let label = label.trim().to_string();
                let exists = self.props.as_ref().is_some_and(|props| {
                    props
                        .detail
                        .task
                        .labels
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(&label))
                });
                if !label.is_empty() && !exists {
                    ctx.emit(TaskDetailEvent::AddLabel(label));
                }
                self.set_editor_text(&self.label_editor.clone(), "", ctx);
                self.label_draft.clear();
                ctx.notify();
            }
            TaskDetailAction::RemoveLabel(label) => {
                ctx.emit(TaskDetailEvent::RemoveLabel(label.clone()));
            }
            TaskDetailAction::SendComment => {
                let body = self.editor_text(&self.composer_editor.clone(), ctx);
                let body = body.trim().to_string();
                if body.is_empty() {
                    return;
                }
                self.set_editor_text(&self.composer_editor.clone(), "", ctx);
                self.composer_draft.clear();
                ctx.emit(TaskDetailEvent::AddComment(body));
                ctx.notify();
            }
            TaskDetailAction::EditComment(id) => self.start_edit_comment(id, ctx),
            TaskDetailAction::CancelCommentEdit => {
                self.editing_comment = None;
                ctx.notify();
            }
            TaskDetailAction::SaveComment => {
                let Some(id) = self.editing_comment.clone() else {
                    return;
                };
                let current = self
                    .props
                    .as_ref()
                    .and_then(|props| {
                        props
                            .detail
                            .comments
                            .iter()
                            .find(|comment| comment.id == id)
                    })
                    .map(|comment| comment.body.clone())
                    .unwrap_or_default();
                let body = self.editor_text(&self.comment_editor.clone(), ctx);
                let body = body.trim().to_string();
                if body.is_empty() {
                    return;
                }
                self.editing_comment = None;
                if body != current {
                    ctx.emit(TaskDetailEvent::EditComment { id, body });
                }
                ctx.notify();
            }
            TaskDetailAction::DeleteComment(id) => {
                ctx.emit(TaskDetailEvent::DeleteComment(id.clone()));
            }
            TaskDetailAction::ToggleComments => {
                self.comments_expanded = !self.comments_expanded;
                ctx.notify();
            }
            TaskDetailAction::OpenLink(link) => {
                ctx.emit(TaskDetailEvent::OpenLink(link.clone()));
            }
            TaskDetailAction::OpenUrl(url) => {
                ctx.emit(TaskDetailEvent::OpenUrl(url.clone()));
            }
        }
    }
}

// ── Page chrome ─────────────────────────────────────────────────────────────

fn tint(color: ColorU, alpha: u8) -> ColorU {
    ColorU::new(color.r, color.g, color.b, alpha)
}

fn scrollable(
    theme: &Theme,
    scroll: &ClippedScrollStateHandle,
    content: Box<dyn Element>,
) -> Box<dyn Element> {
    ClippedScrollable::vertical(
        scroll.clone(),
        content,
        ScrollbarWidth::Auto,
        theme.outline().into(),
        theme.sub_text().into(),
        theme.background.into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// A button dispatching this view's own action; `None` shows it disabled
/// (while the page is busy).
#[allow(clippy::too_many_arguments)]
fn button(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    mouse: MouseStateHandle,
    icon: Option<AppIcon>,
    label_text: &str,
    kind: ButtonKind,
    action: Option<TaskDetailAction>,
) -> Box<dyn Element> {
    let theme = *theme;
    let label_text = label_text.to_string();
    let enabled = action.is_some();
    let hoverable = Hoverable::new(mouse, move |state| {
        let hovered = state.is_hovered() && enabled;
        let (background, foreground, border) = match kind {
            ButtonKind::Primary if !enabled => (
                ColorU::new(theme.accent.r, theme.accent.g, theme.accent.b, 90),
                theme.accent_text(),
                None,
            ),
            ButtonKind::Primary => (
                if hovered {
                    ColorU::new(
                        theme.accent.r.saturating_add(15),
                        theme.accent.g.saturating_add(15),
                        theme.accent.b.saturating_add(15),
                        theme.accent.a,
                    )
                } else {
                    theme.accent
                },
                theme.accent_text(),
                None,
            ),
            ButtonKind::Secondary => (
                if hovered {
                    theme.surface_2()
                } else {
                    ColorU::transparent_black()
                },
                if hovered {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                Some(theme.outline()),
            ),
            ButtonKind::Ghost => (
                if hovered {
                    theme.surface_2()
                } else {
                    ColorU::transparent_black()
                },
                if hovered {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                None,
            ),
            ButtonKind::Danger => (
                if hovered {
                    ColorU::new(theme.ui_red().r, theme.ui_red().g, theme.ui_red().b, 28)
                } else {
                    ColorU::transparent_black()
                },
                theme.ui_red(),
                Some(if hovered {
                    theme.ui_red()
                } else {
                    theme.outline()
                }),
            ),
        };
        let foreground = if enabled {
            foreground
        } else {
            theme.disabled_text()
        };
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_spacing(6.0);
        if let Some(icon) = icon {
            row = row.with_child(render_icon(icon, foreground, 13.0));
        }
        if !label_text.is_empty() {
            row = row.with_child(medium(ui, &label_text, 12.5, foreground));
        }
        let centered = ConstrainedBox::new(Align::new(row.finish()).finish())
            .with_height(text_size(18.0))
            .finish();
        let mut container = Container::new(centered)
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(if label_text.is_empty() { 7.0 } else { 12.0 })
            .with_padding_right(if label_text.is_empty() { 7.0 } else { 12.0 })
            .with_padding_top(6.0)
            .with_padding_bottom(6.0);
        if let Some(border) = border {
            container = container.with_border(Border::all(1.0).with_border_color(border));
        }
        container.finish()
    });
    match action {
        Some(action) => hoverable
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _, _| {
                event_ctx.dispatch_typed_action_for(view_id, action.clone())
            })
            .finish(),
        None => hoverable.finish(),
    }
}

fn icon_button(
    theme: &Theme,
    view_id: EntityId,
    mouse: MouseStateHandle,
    icon: AppIcon,
    action: Option<TaskDetailAction>,
) -> Box<dyn Element> {
    let theme = *theme;
    let enabled = action.is_some();
    let hoverable = Hoverable::new(mouse, move |state| {
        Container::new(
            Align::new(render_icon(
                icon,
                if enabled {
                    theme.sub_text()
                } else {
                    theme.disabled_text()
                },
                15.0,
            ))
            .finish(),
        )
        .with_background_color(if enabled && state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_uniform_padding(8.0)
        .finish()
    });
    match action {
        Some(action) => hoverable
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _, _| {
                event_ctx.dispatch_typed_action_for(view_id, action.clone())
            })
            .finish(),
        None => hoverable.finish(),
    }
}

/// One row of an open inline menu; the selected one carries a check.
fn choice_row(
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    mouse: MouseStateHandle,
    label_text: &str,
    selected: bool,
    action: Option<TaskDetailAction>,
) -> Box<dyn Element> {
    let theme = *theme;
    let label_text = label_text.to_string();
    let enabled = action.is_some();
    let hoverable = Hoverable::new(mouse, move |state| {
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        line(
                            ui,
                            &label_text,
                            12.5,
                            if selected {
                                theme.accent
                            } else {
                                theme.main_text()
                            },
                        ),
                    )
                    .finish(),
                )
                .with_child(if selected {
                    render_icon(AppIcon::Check, theme.accent, 11.0)
                } else {
                    Empty::new().finish()
                })
                .finish(),
        )
        .with_background_color(if enabled && state.is_hovered() {
            theme.surface_3()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish()
    });
    match action {
        Some(action) => hoverable
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _, _| {
                event_ctx.dispatch_typed_action_for(view_id, action.clone())
            })
            .finish(),
        None => hoverable.finish(),
    }
}

impl TaskDetailView {
    /// A month calendar for any due date: ‹ month › and a grid of days,
    /// today outlined, the due day in the accent colour.
    fn due_calendar(
        &self,
        theme: &Theme,
        ui: FamilyId,
        id: EntityId,
        due: Option<i64>,
        now_ms: i64,
        can: bool,
    ) -> Box<dyn Element> {
        use chrono::Datelike;
        let (year, month) = self.due_month.unwrap_or_else(|| month_of(due.unwrap_or(now_ms)));
        let local_day = |ms: i64| {
            Local
                .timestamp_millis_opt(ms)
                .single()
                .map(|time| (time.year(), time.month(), time.day()))
        };
        let today = local_day(now_ms);
        let due_day = due.and_then(local_day);
        let theme = *theme;
        let arrow = |key: &str, label: &str, delta: i32| -> Box<dyn Element> {
            let label = label.to_string();
            Hoverable::new(self.mouse(key), move |state| {
                Container::new(line(ui, &label, 13.0, theme.main_text()))
                    .with_background_color(if state.is_hovered() { theme.surface_3() } else { ColorU::transparent_black() })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(2.0)
                    .with_padding_bottom(2.0)
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _, _| event_ctx.dispatch_typed_action_for(id, TaskDetailAction::DueMonth(delta)))
            .finish()
        };
        let title = chrono::NaiveDate::from_ymd_opt(year, month, 1)
            .map(|date| date.format("%B %Y").to_string())
            .unwrap_or_default();
        let header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(arrow("detail_due_prev", "‹", -1))
            .with_child(semibold(ui, &title, 12.5, theme.main_text()))
            .with_child(arrow("detail_due_next", "›", 1))
            .finish();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0)
            .with_child(header);
        let mut names = Flex::row().with_main_axis_size(MainAxisSize::Max);
        for name in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
            names = names.with_child(
                Expanded::new(1.0, Align::new(line(ui, name, 11.0, theme.disabled_text())).finish()).finish(),
            );
        }
        column = column.with_child(names.finish());
        for week in month_grid(year, month) {
            let mut row = Flex::row().with_main_axis_size(MainAxisSize::Max);
            for day in week {
                let cell: Box<dyn Element> = match day {
                    None => Empty::new().finish(),
                    Some(day) => {
                        let picked = due_day == Some((year, month, day));
                        let is_today = today == Some((year, month, day));
                        let ms = day_ms(year, month, day);
                        let label = day.to_string();
                        let hoverable = Hoverable::new(self.mouse(format!("detail_due_day_{year}_{month}_{day}")), move |state| {
                            let mut cell = Container::new(
                                Align::new(line(
                                    ui,
                                    &label,
                                    12.0,
                                    if picked { theme.background } else { theme.main_text() },
                                ))
                                .finish(),
                            )
                            .with_background_color(if picked {
                                theme.accent
                            } else if can && state.is_hovered() {
                                theme.surface_3()
                            } else {
                                ColorU::transparent_black()
                            })
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                            .with_padding_top(4.0)
                            .with_padding_bottom(4.0);
                            if is_today && !picked {
                                cell = cell.with_border(Border::all(1.0).with_border_color(theme.accent));
                            }
                            cell.finish()
                        });
                        match ms.filter(|_| can) {
                            Some(ms) => hoverable
                                .with_cursor(Cursor::PointingHand)
                                .on_click(move |event_ctx, _, _| {
                                    event_ctx.dispatch_typed_action_for(id, TaskDetailAction::PickDue(Some(ms)))
                                })
                                .finish(),
                            None => hoverable.finish(),
                        }
                    }
                };
                row = row.with_child(Expanded::new(1.0, cell).finish());
            }
            column = column.with_child(row.finish());
        }
        Container::new(column.finish())
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .finish()
    }
}

fn menu_box(theme: &Theme, rows: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(1.0);
    for row in rows {
        column = column.with_child(row);
    }
    Container::new(column.finish())
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_uniform_padding(4.0)
        .finish()
}

fn divider(theme: &Theme) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(theme.outline())
            .finish(),
    )
    .with_height(1.0)
    .finish()
}

fn section_title(ui: FamilyId, theme: &Theme, title: &str) -> Box<dyn Element> {
    Container::new(semibold(ui, title, 12.5, theme.main_text()))
        .with_padding_bottom(2.0)
        .finish()
}

/// The editor in the app's input-box style; clicking its padding focuses it.
fn field(theme: &Theme, editor: &ViewHandle<InputEditor>, min_height: f32) -> Box<dyn Element> {
    let editor_id = editor.id();
    let boxed = Container::new(
        ConstrainedBox::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(ChildView::new(editor).finish())
                .finish(),
        )
        .with_min_height(min_height)
        .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .finish();
    EventHandler::new(boxed)
        .on_left_mouse_down(move |event_ctx, _, position| {
            event_ctx.dispatch_typed_action_for(editor_id, InputEditorAction::ClickAt(position));
            DispatchEventResult::StopPropagation
        })
        .finish()
}

fn status_color(theme: &Theme, status: Status) -> ColorU {
    match status {
        Status::Done => theme.ui_green(),
        Status::Canceled => theme.disabled_text(),
        Status::InReview => theme.ui_yellow(),
        Status::InProgress => theme.accent,
        Status::Todo | Status::Backlog => theme.sub_text(),
    }
}

fn priority_color(theme: &Theme, priority: Priority) -> ColorU {
    match priority {
        Priority::Urgent => theme.ui_red(),
        Priority::High => theme.ui_yellow(),
        Priority::Medium => theme.accent,
        Priority::Low => theme.sub_text(),
        Priority::None => theme.disabled_text(),
    }
}

fn pr_state_color(theme: &Theme, state: Option<&str>) -> ColorU {
    match state.unwrap_or("").to_ascii_lowercase().as_str() {
        "open" => theme.ui_green(),
        "draft" => theme.ui_yellow(),
        "merged" => theme.accent,
        "closed" => theme.disabled_text(),
        _ => theme.sub_text(),
    }
}

fn pill(theme: &Theme, ui: FamilyId, label_text: &str, color: ColorU) -> Box<dyn Element> {
    Container::new(medium(ui, label_text, 11.0, color))
        .with_background_color(tint(color, 26))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish()
}

// ── Top bar ─────────────────────────────────────────────────────────────────

impl TaskDetailView {
    fn top_bar(&self, props: &TaskDetailProps, can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let task = &props.detail.task;
        let crumb = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child({
                let label_text = "Tasks".to_string();
                let hoverable = Hoverable::new(self.mouse("detail_crumb"), move |state| {
                    medium(
                        ui,
                        &label_text,
                        13.0,
                        if can && state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        },
                    )
                });
                if can {
                    hoverable
                        .with_cursor(Cursor::PointingHand)
                        .on_click(move |event_ctx, _, _| {
                            event_ctx.dispatch_typed_action_for(id, TaskDetailAction::Back)
                        })
                        .finish()
                } else {
                    hoverable.finish()
                }
            })
            .with_child(text(ui, "/", 13.0, theme.disabled_text()))
            .with_child(medium(ui, &task.key, 13.0, theme.main_text()))
            .finish();
        let primary = match primary_action(task) {
            PrimaryAction::Start => button(
                &theme,
                ui,
                id,
                self.mouse("detail_primary"),
                Some(AppIcon::Play),
                "Start",
                ButtonKind::Primary,
                can.then_some(TaskDetailAction::Primary),
            ),
            PrimaryAction::OpenWorkspace => button(
                &theme,
                ui,
                id,
                self.mouse("detail_primary"),
                Some(AppIcon::FolderGit),
                "Open workspace",
                ButtonKind::Primary,
                can.then_some(TaskDetailAction::Primary),
            ),
        };
        let actions = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(primary)
            .with_child(icon_button(
                &theme,
                id,
                self.mouse("detail_copy_key"),
                AppIcon::Copy,
                can.then_some(TaskDetailAction::CopyKey),
            ))
            .with_child(icon_button(
                &theme,
                id,
                self.mouse("detail_copy_link"),
                AppIcon::Share,
                can.then_some(TaskDetailAction::CopyLink),
            ))
            .with_child(icon_button(
                &theme,
                id,
                self.mouse("detail_overflow"),
                AppIcon::DotsVertical,
                can.then_some(TaskDetailAction::ToggleMenu(Menu::Overflow)),
            ))
            .finish();
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(crumb)
            .with_child(actions)
            .finish()
    }

    /// The ⋯ menu, in the flow under the top bar so it never floats above
    /// the scrolling page.
    fn overflow_menu(&self, can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let card = menu_box(
            &theme,
            vec![{
                let mouse = self.mouse("detail_delete");
                let label_text = "Delete".to_string();
                let hoverable = Hoverable::new(mouse, move |state| {
                    Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(render_icon(AppIcon::Trash, theme.ui_red(), 13.0))
                            .with_child(medium(ui, &label_text, 12.5, theme.ui_red()))
                            .finish(),
                    )
                    .with_background_color(if can && state.is_hovered() {
                        theme.surface_3()
                    } else {
                        ColorU::transparent_black()
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .with_padding_left(8.0)
                    .with_padding_right(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .finish()
                });
                if can {
                    hoverable
                        .with_cursor(Cursor::PointingHand)
                        .on_click(move |event_ctx, _, _| {
                            event_ctx.dispatch_typed_action_for(
                                id,
                                TaskDetailAction::ConfirmDelete,
                            )
                        })
                        .finish()
                } else {
                    hoverable.finish()
                }
            }],
        );
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_child(ConstrainedBox::new(card).with_width(200.0).finish())
            .finish()
    }

    fn error_line(&self, message: &str, can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let message = message.to_string();
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(AppIcon::TriangleAlert, theme.ui_red(), 13.0))
                .with_child(
                    Expanded::new(1.0, text(ui, &message, 12.5, theme.ui_red())).finish(),
                )
                .with_child(icon_button(
                    &theme,
                    id,
                    self.mouse("detail_dismiss_error"),
                    AppIcon::XClose,
                    can.then_some(TaskDetailAction::DismissError),
                ))
                .finish(),
        )
        .with_background_color(tint(theme.ui_red(), 18))
        .with_border(Border::all(1.0).with_border_color(tint(theme.ui_red(), 90)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(12.0)
        .with_padding_right(6.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    }
}

// ── Main column ─────────────────────────────────────────────────────────────

impl TaskDetailView {
    fn main_column(
        &self,
        props: &TaskDetailProps,
        can: bool,
        now_ms: i64,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(self.title_block(props, can))
            .with_child(self.created_by(props, now_ms))
            .with_child(divider(&theme))
            .with_child(self.description_block(props, can));
        column = column.with_child(divider(&theme));
        let items = timeline(&props.detail);
        column = column.with_child(self.comments_section(props, &items, can, now_ms));
        column = column.with_child(divider(&theme));
        column = column.with_child(self.activity_section(&items, now_ms));
        column.finish()
    }

    fn title_block(&self, props: &TaskDetailProps, can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        if self.editing_title {
            let editor = field(&theme, &self.title_editor, 40.0);
            return Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(6.0)
                .with_child(
                    EventHandler::new(editor)
                        .on_keydown(move |event_ctx, _, key| {
                            if key.key == "enter" && !key.cmd && !key.ctrl {
                                event_ctx.dispatch_typed_action_for(
                                    id,
                                    TaskDetailAction::SaveTitle,
                                );
                                DispatchEventResult::StopPropagation
                            } else if key.key == "escape" {
                                event_ctx.dispatch_typed_action_for(
                                    id,
                                    TaskDetailAction::CancelTitle,
                                );
                                DispatchEventResult::StopPropagation
                            } else {
                                DispatchEventResult::PropagateToParent
                            }
                        })
                        .finish(),
                )
                .with_child(text(
                    ui,
                    "Enter to save · Esc to cancel",
                    11.5,
                    theme.disabled_text(),
                ))
                .finish();
        }
        let title = props.detail.task.title.clone();
        let hoverable = Hoverable::new(self.mouse("detail_title"), move |_| {
            semibold(ui, &title, 20.0, theme.main_text())
        });
        if can {
            hoverable
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _, _| {
                    event_ctx.dispatch_typed_action_for(id, TaskDetailAction::EditTitle)
                })
                .finish()
        } else {
            hoverable.finish()
        }
    }

    fn created_by(&self, props: &TaskDetailProps, now_ms: i64) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let task = &props.detail.task;
        let creator = task
            .creator
            .as_ref()
            .map(|person| person.display().to_string())
            .unwrap_or_else(|| "Someone".to_string());
        text(
            ui,
            format!("Created by {creator} · {}", relative_time(now_ms, task.created_at_ms)),
            12.5,
            theme.sub_text(),
        )
    }

    fn description_block(&self, props: &TaskDetailProps, can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(section_title(ui, &theme, "Description"))
                    .with_child(if self.editing_description {
                        Empty::new().finish()
                    } else {
                        button(
                            &theme,
                            ui,
                            id,
                            self.mouse("detail_edit_description"),
                            Some(AppIcon::Edit),
                            "Edit",
                            ButtonKind::Ghost,
                            can.then_some(TaskDetailAction::EditDescription),
                        )
                    })
                    .finish(),
            );
        if self.editing_description {
            let editor = field(&theme, &self.description_editor, 120.0);
            column = column
                .with_child(
                    EventHandler::new(editor)
                        .on_keydown(move |event_ctx, _, key| {
                            // The editor keeps plain Enter for newlines and
                            // consumes Cmd/Ctrl+Enter itself (only its review
                            // key propagates that chord), so the buttons below
                            // are the guaranteed way to save.
                            if key.key == "escape" {
                                event_ctx.dispatch_typed_action_for(
                                    id,
                                    TaskDetailAction::CancelDescription,
                                );
                                DispatchEventResult::StopPropagation
                            } else {
                                DispatchEventResult::PropagateToParent
                            }
                        })
                        .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(button(
                            &theme,
                            ui,
                            id,
                            self.mouse("detail_cancel_description"),
                            None,
                            "Cancel",
                            ButtonKind::Ghost,
                            Some(TaskDetailAction::CancelDescription),
                        ))
                        .with_child(button(
                            &theme,
                            ui,
                            id,
                            self.mouse("detail_save_description"),
                            None,
                            "Save",
                            ButtonKind::Primary,
                            can.then_some(TaskDetailAction::SaveDescription),
                        ))
                        .finish(),
                );
        } else if props.detail.task.description.trim().is_empty() {
            column = column.with_child(text(ui, "No description yet.", 13.0, theme.disabled_text()));
        } else {
            column = column.with_child(render_markdown_document(
                &props.detail.task.description,
                theme,
                ui,
                self.mono_font,
                text_size(13.0),
                1.45,
            ));
        }
        column.finish()
    }
}

// The main-column impl ends here; helpers and sections below are free
// items plus their own focused impl.

// ── Comments before activity ────────────────────────────────────────────────

/// How many of the latest comments show while the rest stay collapsed
/// behind a "show earlier" toggle.
const COLLAPSED_COMMENTS: usize = 3;

/// Which comments to paint, oldest first: everything when expanded (or
/// few enough to fit), else only the latest few.
pub(crate) fn visible_comments<'a, 'b>(
    comments: &'a [&'b Comment],
    expanded: bool,
) -> &'a [&'b Comment] {
    if expanded || comments.len() <= COLLAPSED_COMMENTS {
        comments
    } else {
        &comments[comments.len() - COLLAPSED_COMMENTS..]
    }
}

/// One icon per history kind, for the activity feed.
fn activity_icon(kind: ActivityKind) -> AppIcon {
    match kind {
        ActivityKind::Created => AppIcon::Sparkle,
        ActivityKind::Title => AppIcon::Edit,
        ActivityKind::Description => AppIcon::File,
        ActivityKind::Status => AppIcon::RefreshCw,
        ActivityKind::Priority => AppIcon::Sliders,
        ActivityKind::Assignee => AppIcon::User,
        ActivityKind::Labels => AppIcon::Layers,
        ActivityKind::Project => AppIcon::FolderGit,
        ActivityKind::Due => AppIcon::Clock,
        ActivityKind::LinkAdded => AppIcon::Plus,
        ActivityKind::LinkRemoved => AppIcon::XClose,
        ActivityKind::Deleted => AppIcon::Trash,
        ActivityKind::Restored => AppIcon::Restore,
    }
}

impl TaskDetailView {
    /// Comments first: the cards (latest few when many), then the composer.
    fn comments_section(
        &self,
        props: &TaskDetailProps,
        items: &[TimelineItem],
        can: bool,
        now_ms: i64,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let ordered: Vec<&Comment> = items
            .iter()
            .filter_map(|item| match item {
                TimelineItem::Comment(comment) => Some(*comment),
                _ => None,
            })
            .collect();
        let total = ordered.len();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(10.0)
            .with_child(section_title(
                ui,
                &theme,
                &format!("Comments ({total})"),
            ));
        if total == 0 {
            column = column.with_child(text(
                ui,
                "No comments yet. Start the discussion below.",
                12.5,
                theme.disabled_text(),
            ));
        } else {
            let shown = visible_comments(&ordered, self.comments_expanded);
            let hidden = total.saturating_sub(shown.len());
            if hidden > 0 || self.comments_expanded {
                let label = if self.comments_expanded {
                    "Show fewer".to_string()
                } else {
                    format!(
                        "Show {} earlier comment{}",
                        hidden,
                        if hidden == 1 { "" } else { "s" }
                    )
                };
                column = column.with_child(
                    Flex::row()
                        .with_main_axis_alignment(MainAxisAlignment::Start)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(button(
                            &theme,
                            ui,
                            id,
                            self.mouse("detail_toggle_comments"),
                            Some(if self.comments_expanded {
                                AppIcon::ChevronUp
                            } else {
                                AppIcon::ChevronDown
                            }),
                            &label,
                            ButtonKind::Ghost,
                            Some(TaskDetailAction::ToggleComments),
                        ))
                        .finish(),
                );
            }
            for comment in shown {
                column = column.with_child(self.comment_card(props, comment, can, now_ms));
            }
        }
        column = column.with_child(self.composer(can));
        column.finish()
    }

    /// History after the comments: one designed row per entry, oldest first.
    fn activity_section(&self, items: &[TimelineItem], now_ms: i64) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let ordered: Vec<&Activity> = items
            .iter()
            .filter_map(|item| match item {
                TimelineItem::History(activity) => Some(*activity),
                _ => None,
            })
            .collect();
        let total = ordered.len();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(10.0)
            .with_child(section_title(
                ui,
                &theme,
                &format!("Activity ({total})"),
            ));
        if total == 0 {
            column = column.with_child(text(
                ui,
                "No activity yet.",
                12.5,
                theme.disabled_text(),
            ));
        } else {
            for activity in ordered {
                column = column.with_child(self.activity_row(activity, now_ms));
            }
        }
        column.finish()
    }

    /// One history entry: a quiet icon badge, the sentence, and its age.
    /// The sentence is the only flexible child, so it truncates instead of
    /// overflowing on narrow widths.
    fn activity_row(&self, activity: &Activity, now_ms: i64) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let badge = Container::new(render_icon(
            activity_icon(activity.kind),
            theme.sub_text(),
            12.0,
        ))
        .with_background_color(theme.surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_uniform_padding(5.0)
        .finish();
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(badge)
            .with_child(
                Shrinkable::new(
                    1.0,
                    line(
                        ui,
                        history_sentence(activity),
                        12.5,
                        theme.sub_text(),
                    ),
                )
                .finish(),
            )
            .with_child(text(
                ui,
                relative_time(now_ms, activity.at_ms),
                11.5,
                theme.disabled_text(),
            ))
            .finish()
    }
}

impl TaskDetailView {
    fn comment_card(
        &self,
        props: &TaskDetailProps,
        comment: &Comment,
        can: bool,
        now_ms: i64,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let author = comment
            .author
            .as_ref()
            .map(|person| person.display().to_string())
            .unwrap_or_else(|| "Someone".to_string());
        let mut when = relative_time(now_ms, comment.created_at_ms);
        if comment.edited_at_ms.is_some() {
            when.push_str(" · edited");
        }
        let is_mine = props.me.as_deref().is_some_and(|me| {
            comment
                .author
                .as_ref()
                .is_some_and(|person| person.id == me)
        });
        // The actions sit in the header, grouped before the time; a nested
        // row of fixed children only, so it never hits the flexible-child
        // infinite-constraint panic.
        let mut actions = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0);
        if is_mine && self.editing_comment.as_deref() != Some(comment.id.as_str()) {
            let edit_id = comment.id.clone();
            let delete_id = comment.id.clone();
            actions = actions
                .with_child(icon_button(
                    &theme,
                    id,
                    self.mouse(format!("detail_comment_edit_{edit_id}")),
                    AppIcon::Edit,
                    can.then_some(TaskDetailAction::EditComment(edit_id)),
                ))
                .with_child(icon_button(
                    &theme,
                    id,
                    self.mouse(format!("detail_comment_delete_{delete_id}")),
                    AppIcon::Trash,
                    can.then_some(TaskDetailAction::DeleteComment(delete_id)),
                ));
        }
        actions = actions.with_child(text(ui, &when, 11.5, theme.disabled_text()));
        let header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(medium(ui, &author, 12.5, theme.main_text()))
            .with_child(actions.finish())
            .finish();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(header);
        if is_mine && self.editing_comment.as_deref() != Some(comment.id.as_str()) {
            let edit_id = comment.id.clone();
            let delete_id = comment.id.clone();
            column = column.with_child(
                Flex::row()
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.0)
                    .with_child(icon_button(
                        &theme,
                        id,
                        self.mouse(format!("detail_comment_edit_{edit_id}")),
                        AppIcon::Edit,
                        can.then_some(TaskDetailAction::EditComment(edit_id)),
                    ))
                    .with_child(icon_button(
                        &theme,
                        id,
                        self.mouse(format!("detail_comment_delete_{delete_id}")),
                        AppIcon::Trash,
                        can.then_some(TaskDetailAction::DeleteComment(delete_id)),
                    ))
                    .finish(),
            );
        }
        if self.editing_comment.as_deref() == Some(comment.id.as_str()) {
            let editor = field(&theme, &self.comment_editor, 80.0);
            let comment_id = comment.id.clone();
            let save_id = comment_id.clone();
            column = column
                .with_child(
                    EventHandler::new(editor)
                        .on_keydown(move |event_ctx, _, key| {
                            if key.key == "escape" {
                                event_ctx.dispatch_typed_action_for(
                                    id,
                                    TaskDetailAction::CancelCommentEdit,
                                );
                                DispatchEventResult::StopPropagation
                            } else {
                                DispatchEventResult::PropagateToParent
                            }
                        })
                        .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(button(
                            &theme,
                            ui,
                            id,
                            self.mouse(format!("detail_comment_cancel_{comment_id}")),
                            None,
                            "Cancel",
                            ButtonKind::Ghost,
                            Some(TaskDetailAction::CancelCommentEdit),
                        ))
                        .with_child(button(
                            &theme,
                            ui,
                            id,
                            self.mouse(format!("detail_comment_save_{save_id}")),
                            None,
                            "Save",
                            ButtonKind::Primary,
                            can.then_some(TaskDetailAction::SaveComment),
                        ))
                        .finish(),
                );
        } else {
            column = column.with_child(render_markdown_document(
                &comment.body,
                theme,
                ui,
                self.mono_font,
                text_size(13.0),
                1.45,
            ));
        }
        Container::new(column.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .finish()
    }

    fn composer(&self, can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let editor = field(&theme, &self.composer_editor, 72.0);
        // The editor consumes Cmd/Ctrl+Enter for its own keys (only its
        // review key propagates that chord), so this fires if propagation
        // ever covers this editor; the Comment button always sends.
        let editor = EventHandler::new(editor)
            .on_keydown(move |event_ctx, _, key| {
                if key.key == "enter" && (key.cmd || key.ctrl) {
                    event_ctx
                        .dispatch_typed_action_for(id, TaskDetailAction::SendComment);
                    DispatchEventResult::StopPropagation
                } else {
                    DispatchEventResult::PropagateToParent
                }
            })
            .finish();
        let ready = can && !self.composer_draft.trim().is_empty();
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(section_title(ui, &theme, "Comment"))
            .with_child(editor)
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(text(
                        ui,
                        "Enter for a new line · ⌘↵ to send",
                        11.5,
                        theme.disabled_text(),
                    ))
                    .with_child(button(
                        &theme,
                        ui,
                        id,
                        self.mouse("detail_send_comment"),
                        None,
                        "Comment",
                        ButtonKind::Primary,
                        ready.then_some(TaskDetailAction::SendComment),
                    ))
                    .finish(),
            )
            .finish()
    }
}

// ── Side panel ──────────────────────────────────────────────────────────────

impl TaskDetailView {
    fn side_panel(
        &self,
        props: &TaskDetailProps,
        can: bool,
        now_ms: i64,
    ) -> Box<dyn Element> {
        let mut panel = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(self.properties_card(props, can, now_ms))
            .with_child(self.labels_card(props, can));
        let workspaces: Vec<&TaskLink> =
            props.detail.task.links_of(LinkKind::Workspace).collect();
        if !workspaces.is_empty() {
            panel = panel.with_child(self.workspaces_card(props, &workspaces, can));
        }
        let pulls: Vec<&TaskLink> = props
            .detail
            .task
            .links_of(LinkKind::PullRequest)
            .collect();
        if !pulls.is_empty() {
            panel = panel.with_child(self.pulls_card(&pulls, can));
        }
        let items: Vec<&TaskLink> = props.detail.task.links_of(LinkKind::Item).collect();
        if !items.is_empty() {
            panel = panel.with_child(self.tracker_card(&items, can));
        }
        panel.finish()
    }

    /// One property row: a quiet name with the value beside it; clicking
    /// opens the row's choices inline below.
    fn property_row(
        &self,
        name: &str,
        value: &str,
        value_color: ColorU,
        menu: Menu,
        can: bool,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let open = self.open_menu == Some(menu);
        let name = name.to_string();
        let value = value.to_string();
        let hoverable = Hoverable::new(
            self.mouse(format!("detail_menu_{name}")),
            move |state| {
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(text(ui, &name, 12.5, theme.sub_text()))
                        .with_child(
                            Shrinkable::new(
                                1.0,
                                Flex::row()
                                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                    .with_spacing(4.0)
                                    .with_child(Shrinkable::new(
                                        1.0,
                                        line(ui, &value, 12.5, value_color),
                                    )
                                    .finish())
                                    .with_child(render_icon(
                                        AppIcon::ChevronDown,
                                        theme.disabled_text(),
                                        11.0,
                                    ))
                                    .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .with_background_color(if open {
                    theme.surface_3()
                } else if can && state.is_hovered() {
                    theme.surface_2()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish()
            },
        );
        if can {
            hoverable
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _, _| {
                    event_ctx.dispatch_typed_action_for(id, TaskDetailAction::ToggleMenu(menu))
                })
                .finish()
        } else {
            hoverable.finish()
        }
    }

    fn properties_card(
        &self,
        props: &TaskDetailProps,
        can: bool,
        now_ms: i64,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let task = &props.detail.task;
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0);
        body = body.with_child(self.property_row(
            "Status",
            task.status.label(),
            status_color(&theme, task.status),
            Menu::Status,
            can,
        ));
        if self.open_menu == Some(Menu::Status) {
            let rows = Status::ALL
                .iter()
                .map(|status| {
                    choice_row(
                        &theme,
                        ui,
                        id,
                        self.mouse(format!("detail_status_{}", status.key())),
                        status.label(),
                        *status == task.status,
                        can.then_some(TaskDetailAction::PickStatus(*status)),
                    )
                })
                .collect();
            body = body.with_child(menu_box(&theme, rows));
        }
        body = body.with_child(self.property_row(
            "Priority",
            task.priority.label(),
            priority_color(&theme, task.priority),
            Menu::Priority,
            can,
        ));
        if self.open_menu == Some(Menu::Priority) {
            let rows = Priority::ALL
                .iter()
                .map(|priority| {
                    choice_row(
                        &theme,
                        ui,
                        id,
                        self.mouse(format!("detail_priority_{}", priority.key())),
                        priority.label(),
                        *priority == task.priority,
                        can.then_some(TaskDetailAction::PickPriority(*priority)),
                    )
                })
                .collect();
            body = body.with_child(menu_box(&theme, rows));
        }
        let assignee = task
            .assignee
            .as_ref()
            .map(|person| person.display().to_string())
            .unwrap_or_else(|| "Unassigned".to_string());
        body = body.with_child(self.property_row(
            "Assignee",
            &assignee,
            theme.main_text(),
            Menu::Assignee,
            can,
        ));
        if self.open_menu == Some(Menu::Assignee) {
            let mut rows = vec![choice_row(
                &theme,
                ui,
                id,
                self.mouse("detail_assignee_none"),
                "Unassigned",
                task.assignee.is_none(),
                can.then_some(TaskDetailAction::PickAssignee(None)),
            )];
            for member in &props.members {
                let member_id = member.id.clone();
                let label_text = if member.email.trim().is_empty()
                    || member.email.trim() == member.display().trim()
                {
                    member.display().to_string()
                } else {
                    format!("{} · {}", member.display(), member.email.trim())
                };
                rows.push(choice_row(
                    &theme,
                    ui,
                    id,
                    self.mouse(format!("detail_assignee_{member_id}")),
                    &label_text,
                    task.assignee.as_ref().is_some_and(|assignee| assignee.id == member_id),
                    can.then_some(TaskDetailAction::PickAssignee(Some(member_id))),
                ));
            }
            body = body.with_child(menu_box(&theme, rows));
        }
        let project = task
            .project
            .as_ref()
            .map(|project| project.label.clone())
            .unwrap_or_else(|| "No project".to_string());
        body = body.with_child(self.property_row(
            "Project",
            &project,
            theme.main_text(),
            Menu::Project,
            can,
        ));
        if self.open_menu == Some(Menu::Project) {
            let mut rows = vec![choice_row(
                &theme,
                ui,
                id,
                self.mouse("detail_project_none"),
                "No project",
                task.project.is_none(),
                can.then_some(TaskDetailAction::PickProject(None)),
            )];
            for project in &props.projects {
                rows.push(choice_row(
                    &theme,
                    ui,
                    id,
                    self.mouse(format!("detail_project_{}", project.key)),
                    &project.label,
                    task.project
                        .as_ref()
                        .is_some_and(|current| current.key == project.key),
                    can.then_some(TaskDetailAction::PickProject(Some(project.clone()))),
                ));
            }
            if let Some(current) = task.project.clone() {
                if !props.projects.iter().any(|project| project.key == current.key) {
                    rows.push(choice_row(
                        &theme,
                        ui,
                        id,
                        self.mouse(format!("detail_project_{}", current.key)),
                        &current.label,
                        true,
                        None,
                    ));
                }
            }
            body = body.with_child(menu_box(&theme, rows));
        }
        let (due_value, due_color) = match task.due_at_ms {
            Some(ms) => {
                let mut label_text = format_day(ms);
                let mut color = theme.main_text();
                if ms < now_ms {
                    label_text.push_str(" · overdue");
                    color = theme.ui_red();
                }
                (label_text, color)
            }
            None => ("No due date".to_string(), theme.main_text()),
        };
        body = body.with_child(self.property_row(
            "Due",
            &due_value,
            due_color,
            Menu::Due,
            can,
        ));
        if self.open_menu == Some(Menu::Due) {
            let mut rows = vec![choice_row(
                &theme,
                ui,
                id,
                self.mouse("detail_due_none"),
                "No due date",
                task.due_at_ms.is_none(),
                can.then_some(TaskDetailAction::PickDue(None)),
            )];
            for (days, name) in [(1, "Tomorrow"), (7, "Next week"), (30, "Next month")] {
                let ms = due_preset_ms(days, now_ms);
                rows.push(choice_row(
                    &theme,
                    ui,
                    id,
                    self.mouse(format!("detail_due_{days}")),
                    &format!("{name} · {}", format_day(ms)),
                    task.due_at_ms == Some(ms),
                    can.then_some(TaskDetailAction::PickDue(Some(ms))),
                ));
            }
            rows.push(divider(&theme));
            rows.push(self.due_calendar(&theme, ui, id, task.due_at_ms, now_ms, can));
            body = body.with_child(menu_box(&theme, rows));
        }
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(semibold(ui, "Properties", 12.5, theme.main_text()))
                .with_child(body.finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(12.0)
        .finish()
    }

    fn labels_card(&self, props: &TaskDetailProps, can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let task = &props.detail.task;
        let mut chips = Wrap::row().with_spacing(6.0).with_run_spacing(6.0);
        for label_text in &task.labels {
            let remove = label_text.clone();
            let chip_mouse = self.mouse(format!("detail_label_{label_text}"));
            let chip_label = label_text.clone();
            let chip = Hoverable::new(chip_mouse, move |_| {
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(5.0)
                        .with_child(medium(ui, &chip_label, 11.5, theme.main_text()))
                        .with_child(render_icon(AppIcon::XClose, theme.disabled_text(), 10.0))
                        .finish(),
                )
                .with_background_color(theme.surface_3())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(7.0)
                .with_padding_right(7.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish()
            });
            let chip = if can {
                chip.with_cursor(Cursor::PointingHand)
                    .on_click(move |event_ctx, _, _| {
                        event_ctx.dispatch_typed_action_for(
                            id,
                            TaskDetailAction::RemoveLabel(remove.clone()),
                        )
                    })
                    .finish()
            } else {
                chip.finish()
            };
            chips = chips.with_child(chip);
        }
        chips = chips.with_child({
            let hoverable = Hoverable::new(self.mouse("detail_add_label"), move |state| {
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(4.0)
                        .with_child(render_icon(AppIcon::Plus, theme.sub_text(), 11.0))
                        .finish(),
                )
                .with_background_color(if can && state.is_hovered() {
                    theme.surface_3()
                } else {
                    ColorU::transparent_black()
                })
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(7.0)
                .with_padding_right(7.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish()
            });
            if can {
                hoverable
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |event_ctx, _, _| {
                        event_ctx.dispatch_typed_action_for(
                            id,
                            TaskDetailAction::ToggleMenu(Menu::Labels),
                        )
                    })
                    .finish()
            } else {
                hoverable.finish()
            }
        });
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(chips.finish());
        if self.open_menu == Some(Menu::Labels) {
            let mut rows: Vec<Box<dyn Element>> = props
                .known_labels
                .iter()
                .filter(|known| !task.labels.iter().any(|label| label == *known))
                .map(|known| {
                    choice_row(
                        &theme,
                        ui,
                        id,
                        self.mouse(format!("detail_suggest_{known}")),
                        known,
                        false,
                        can.then_some(TaskDetailAction::AddLabel(known.clone())),
                    )
                })
                .collect();
            if rows.is_empty() {
                rows.push(text(
                    ui,
                    "No other labels yet.",
                    12.0,
                    theme.disabled_text(),
                ));
            }
            rows.push(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(
                        Expanded::new(1.0, field(&theme, &self.label_editor, 30.0)).finish(),
                    )
                    .with_child(button(
                        &theme,
                        ui,
                        id,
                        self.mouse("detail_submit_label"),
                        None,
                        "Add",
                        ButtonKind::Secondary,
                        (can && !self.label_draft.trim().is_empty())
                            .then_some(TaskDetailAction::SubmitLabel),
                    ))
                    .finish(),
            );
            body = body.with_child(menu_box(&theme, rows));
        }
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(semibold(ui, "Labels", 12.5, theme.main_text()))
                .with_child(body.finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(12.0)
        .finish()
    }

    fn workspaces_card(
        &self,
        props: &TaskDetailProps,
        workspaces: &[&TaskLink],
        can: bool,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0);
        for link in workspaces {
            let openable = props.openable_links.contains(&link.id);
            let state = link.state.clone().unwrap_or_default();
            let title = link.title.clone();
            let mut row = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(4.0)
                .with_child(line(ui, &title, 12.5, theme.main_text()));
            if !state.trim().is_empty() {
                row = row.with_child(text(ui, &state, 11.5, theme.sub_text()));
            }
            row = row.with_child(if openable {
                Flex::row()
                    .with_main_axis_alignment(MainAxisAlignment::Start)
                    .with_child(button(
                        &theme,
                        ui,
                        id,
                        self.mouse(format!("detail_open_ws_{}", link.id)),
                        Some(AppIcon::FolderGit),
                        "Open",
                        ButtonKind::Secondary,
                        can.then_some(TaskDetailAction::OpenLink((*link).clone())),
                    ))
                    .finish()
            } else {
                text(ui, "On another computer", 11.5, theme.disabled_text())
            });
            body = body.with_child(row.finish());
        }
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(semibold(ui, "Workspaces", 12.5, theme.main_text()))
                .with_child(body.finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(12.0)
        .finish()
    }

    /// Pull requests open at their address; the state pill says where each
    /// stands.
    fn pulls_card(&self, pulls: &[&TaskLink], can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0);
        for link in pulls {
            // Rebuilt on every hover frame: the row can't be built once and
            // moved into the closure.
            let title = link.title.clone();
            let state = link.state.clone().unwrap_or_default();
            let state_color = pr_state_color(&theme, link.state.as_deref());
            let build_row = move |ui: FamilyId, theme: Theme| {
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(
                        Shrinkable::new(1.0, line(ui, &title, 12.5, theme.main_text()))
                            .finish(),
                    )
                    .with_child(if state.trim().is_empty() {
                        Empty::new().finish()
                    } else {
                        pill(&theme, ui, &state, state_color)
                    })
                    .finish()
            };
            match (link.url.clone(), can) {
                (Some(url), true) => {
                    let inner = build_row;
                    let hoverable = Hoverable::new(
                        self.mouse(format!("detail_pr_{}", link.id)),
                        move |hovered| {
                            Container::new(inner(ui, theme))
                                .with_background_color(if hovered.is_hovered() {
                                    theme.surface_2()
                                } else {
                                    ColorU::transparent_black()
                                })
                                .with_corner_radius(CornerRadius::with_all(
                                    Radius::Pixels(6.0),
                                ))
                                .with_padding_left(6.0)
                                .with_padding_right(6.0)
                                .with_padding_top(5.0)
                                .with_padding_bottom(5.0)
                                .finish()
                        },
                    );
                    body = body.with_child(
                        hoverable
                            .with_cursor(Cursor::PointingHand)
                            .on_click(move |event_ctx, _, _| {
                                event_ctx.dispatch_typed_action_for(
                                    id,
                                    TaskDetailAction::OpenUrl(url.clone()),
                                )
                            })
                            .finish(),
                    );
                }
                _ => {
                    body = body.with_child(build_row(ui, theme));
                }
            }
        }
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(semibold(ui, "Pull requests", 12.5, theme.main_text()))
                .with_child(body.finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(12.0)
        .finish()
    }

    fn tracker_card(&self, items: &[&TaskLink], can: bool) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let id = self.view_id;
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0);
        for link in items {
            let title = link.title.clone();
            let build_row = move |ui: FamilyId, theme: Theme| {
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(render_icon(AppIcon::TaskList, theme.sub_text(), 13.0))
                    .with_child(
                        Shrinkable::new(1.0, line(ui, &title, 12.5, theme.main_text()))
                            .finish(),
                    )
                    .finish()
            };
            match (link.url.clone(), can) {
                (Some(url), true) => {
                    let inner = build_row;
                    let hoverable = Hoverable::new(
                        self.mouse(format!("detail_item_{}", link.id)),
                        move |hovered| {
                            Container::new(inner(ui, theme))
                                .with_background_color(if hovered.is_hovered() {
                                    theme.surface_2()
                                } else {
                                    ColorU::transparent_black()
                                })
                                .with_corner_radius(CornerRadius::with_all(
                                    Radius::Pixels(6.0),
                                ))
                                .with_padding_left(6.0)
                                .with_padding_right(6.0)
                                .with_padding_top(5.0)
                                .with_padding_bottom(5.0)
                                .finish()
                        },
                    );
                    body = body.with_child(
                        hoverable
                            .with_cursor(Cursor::PointingHand)
                            .on_click(move |event_ctx, _, _| {
                                event_ctx.dispatch_typed_action_for(
                                    id,
                                    TaskDetailAction::OpenUrl(url.clone()),
                                )
                            })
                            .finish(),
                    );
                }
                _ => {
                    body = body.with_child(build_row(ui, theme));
                }
            }
        }
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(semibold(ui, "Linked item", 12.5, theme.main_text()))
                .with_child(body.finish())
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(12.0)
        .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn person(id: &str, name: &str) -> Person {
        Person {
            id: id.into(),
            name: name.into(),
            email: String::new(),
        }
    }

    fn activity(
        kind: ActivityKind,
        actor: Option<Person>,
        actor_kind: ActorKind,
        from: Option<serde_json::Value>,
        to: Option<serde_json::Value>,
    ) -> Activity {
        Activity {
            id: "a1".into(),
            task_id: "t1".into(),
            actor,
            actor_kind,
            kind,
            from,
            to,
            at_ms: 1_000,
        }
    }

    fn task() -> Task {
        Task {
            id: "t1".into(),
            key: "DRA-12".into(),
            number: 12,
            title: "Fix the login redirect".into(),
            description: "It loops.".into(),
            status: Status::Todo,
            priority: Priority::High,
            assignee: None,
            creator: Some(person("u1", "Ama")),
            project: None,
            labels: Vec::new(),
            due_at_ms: None,
            created_at_ms: 1_000,
            updated_at_ms: 2_000,
            started_at_ms: None,
            completed_at_ms: None,
            links: Vec::new(),
            comment_count: 0,
        }
    }

    fn detail() -> TaskDetail {
        TaskDetail {
            task: task(),
            comments: Vec::new(),
            activity: Vec::new(),
        }
    }

    #[test]
    fn relative_time_counts_up_then_dates() {
        let now = 1_730_000_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now + 60_000), "just now");
        assert_eq!(relative_time(now, now - 5 * 60_000), "5m ago");
        assert_eq!(relative_time(now, now - 3 * 3_600_000), "3h ago");
        assert_eq!(relative_time(now, now - 2 * 86_400_000), "2d ago");
        // Mid-June stays mid-June in every time zone.
        let old = 1_718_438_400_000;
        assert_eq!(relative_time(now, old), format_day(old));
        assert!(format_day(old).contains("2024"));
    }

    #[test]
    fn history_names_person_agent_and_system_actors() {
        let created = activity(
            ActivityKind::Created,
            Some(person("u1", "Ama")),
            ActorKind::Person,
            None,
            None,
        );
        assert_eq!(history_sentence(&created), "Ama created this");
        let agent = activity(
            ActivityKind::Status,
            Some(person("u1", "Ama")),
            ActorKind::Agent,
            Some(json!("todo")),
            Some(json!("in_progress")),
        );
        assert_eq!(
            history_sentence(&agent),
            "Ama's agent moved this from Todo to In progress"
        );
        let system = activity(ActivityKind::Deleted, None, ActorKind::System, None, None);
        assert_eq!(history_sentence(&system), "Dragent deleted this");
        let missing = activity(ActivityKind::Restored, None, ActorKind::Person, None, None);
        assert_eq!(history_sentence(&missing), "Someone restored this");
    }

    #[test]
    fn history_covers_every_kind() {
        let ama = Some(person("u1", "Ama"));
        let due_expected = format!("Ama set the due date to {}", format_day(1_718_438_400_000));
        let cases: Vec<(Activity, &str)> = vec![
            (
                activity(
                    ActivityKind::Title,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!("Old")),
                    Some(json!("New")),
                ),
                "Ama changed the title to “New”",
            ),
            (
                activity(ActivityKind::Description, ama.clone(), ActorKind::Person, None, None),
                "Ama edited the description",
            ),
            (
                activity(
                    ActivityKind::Priority,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!("medium")),
                    Some(json!("high")),
                ),
                "Ama set the priority to High",
            ),
            (
                activity(
                    ActivityKind::Priority,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!("high")),
                    Some(serde_json::Value::Null),
                ),
                "Ama cleared the priority",
            ),
            (
                activity(
                    ActivityKind::Assignee,
                    ama.clone(),
                    ActorKind::Person,
                    Some(serde_json::Value::Null),
                    Some(json!({"id": "u2", "name": "Bo"})),
                ),
                "Ama assigned this to Bo",
            ),
            (
                activity(
                    ActivityKind::Assignee,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!({"id": "u2", "name": "Bo"})),
                    Some(serde_json::Value::Null),
                ),
                "Ama unassigned this",
            ),
            (
                activity(
                    ActivityKind::Labels,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!(["bug"])),
                    Some(json!(["bug", "ui"])),
                ),
                "Ama added the “ui” label",
            ),
            (
                activity(
                    ActivityKind::Labels,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!(["bug", "ui"])),
                    Some(json!(["bug"])),
                ),
                "Ama removed the “ui” label",
            ),
            (
                activity(
                    ActivityKind::Project,
                    ama.clone(),
                    ActorKind::Person,
                    Some(serde_json::Value::Null),
                    Some(json!({"key": "github.com/acme/app", "label": "acme/app"})),
                ),
                "Ama set the project to acme/app",
            ),
            (
                activity(
                    ActivityKind::Project,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!({"key": "github.com/acme/app", "label": "acme/app"})),
                    Some(serde_json::Value::Null),
                ),
                "Ama removed the project",
            ),
            (
                activity(
                    ActivityKind::Due,
                    ama.clone(),
                    ActorKind::Person,
                    Some(serde_json::Value::Null),
                    Some(json!(1_718_438_400_000i64)),
                ),
                &due_expected,
            ),
            (
                activity(
                    ActivityKind::LinkAdded,
                    ama.clone(),
                    ActorKind::Agent,
                    None,
                    Some(json!({"kind": "pull_request", "title": "Fix it"})),
                ),
                "Ama's agent linked a pull request",
            ),
            (
                activity(
                    ActivityKind::LinkRemoved,
                    ama.clone(),
                    ActorKind::Person,
                    Some(json!({"kind": "workspace", "title": "dra-12"})),
                    None,
                ),
                "Ama removed a workspace link",
            ),
        ];
        for (entry, expected) in cases {
            assert_eq!(history_sentence(&entry), expected);
        }
    }

    #[test]
    fn timeline_merges_history_and_comments_oldest_first() {
        let mut full = detail();
        full.activity = vec![
            activity(ActivityKind::Created, None, ActorKind::Person, None, None),
            Activity {
                at_ms: 3_000,
                ..activity(ActivityKind::Description, None, ActorKind::Person, None, None)
            },
        ];
        full.comments = vec![
            Comment {
                id: "c1".into(),
                task_id: "t1".into(),
                author: Some(person("u1", "Ama")),
                actor_kind: ActorKind::Person,
                body: "First".into(),
                created_at_ms: 2_000,
                edited_at_ms: None,
            },
            Comment {
                id: "c2".into(),
                task_id: "t1".into(),
                author: Some(person("u2", "Bo")),
                actor_kind: ActorKind::Person,
                body: "Second".into(),
                created_at_ms: 3_000,
                edited_at_ms: None,
            },
        ];
        let order: Vec<i64> = timeline(&full).iter().map(|item| item.at_ms()).collect();
        assert_eq!(order, vec![1_000, 2_000, 3_000, 3_000]);
        // The history entry sorts before the comment stamped together.
        let tied: Vec<bool> = timeline(&full)
            .iter()
            .filter(|item| item.at_ms() == 3_000)
            .map(|item| matches!(item, TimelineItem::Comment(_)))
            .collect();
        assert_eq!(tied, vec![false, true]);
    }

    #[test]
    fn primary_action_follows_workspace_links() {
        assert_eq!(primary_action(&task()), PrimaryAction::Start);
        let mut with_workspace = task();
        with_workspace.links.push(TaskLink {
            id: "w1".into(),
            kind: LinkKind::Workspace,
            title: "dra-12".into(),
            url: None,
            reference: "dev-1:/tmp/dra-12".into(),
            state: Some("working".into()),
            added_by: None,
            created_at_ms: 1_000,
        });
        assert_eq!(
            primary_action(&with_workspace),
            PrimaryAction::OpenWorkspace
        );
        // Pull requests alone still mean Start.
        let mut with_pull = task();
        with_pull.links.push(TaskLink {
            id: "p1".into(),
            kind: LinkKind::PullRequest,
            title: "Fix it".into(),
            url: Some("https://example.test/pr/1".into()),
            reference: "github:acme/app#1".into(),
            state: Some("open".into()),
            added_by: None,
            created_at_ms: 1_000,
        });
        assert_eq!(primary_action(&with_pull), PrimaryAction::Start);
    }

    fn comment(id: &str, at_ms: i64) -> Comment {
        Comment {
            id: id.into(),
            task_id: "t1".into(),
            author: None,
            actor_kind: ActorKind::Person,
            body: id.into(),
            created_at_ms: at_ms,
            edited_at_ms: None,
        }
    }

    #[test]
    fn comments_collapse_to_the_latest_few_until_expanded() {
        let owned: Vec<Comment> = (0..5).map(|n| comment(&format!("c{n}"), 1_000 + n)).collect();
        let ordered: Vec<&Comment> = owned.iter().collect();
        // Few enough: everything shows, collapsed or not.
        let few: Vec<&Comment> = owned[..COLLAPSED_COMMENTS].iter().collect();
        assert_eq!(visible_comments(&few, false).len(), COLLAPSED_COMMENTS);
        assert_eq!(visible_comments(&few, true).len(), COLLAPSED_COMMENTS);
        // Many: collapsed shows only the latest few, oldest first.
        let shown = visible_comments(&ordered, false);
        assert_eq!(shown.len(), COLLAPSED_COMMENTS);
        assert_eq!(shown.last().unwrap().id, "c4");
        assert_eq!(shown.first().unwrap().id, "c2");
        // Expanded shows everything.
        let shown = visible_comments(&ordered, true);
        assert_eq!(shown.len(), ordered.len());
        assert_eq!(shown.first().unwrap().id, "c0");
    }

    #[test]
    fn every_history_kind_has_an_activity_icon() {
        for kind in [
            ActivityKind::Created,
            ActivityKind::Title,
            ActivityKind::Description,
            ActivityKind::Status,
            ActivityKind::Priority,
            ActivityKind::Assignee,
            ActivityKind::Labels,
            ActivityKind::Project,
            ActivityKind::Due,
            ActivityKind::LinkAdded,
            ActivityKind::LinkRemoved,
            ActivityKind::Deleted,
            ActivityKind::Restored,
        ] {
            let _ = activity_icon(kind);
        }
    }

    #[test]
    fn the_due_calendar_lays_out_any_month() {
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 5, -17), (2024, 12));
        // November 2026 starts on a Sunday: the 1st sits in the last column.
        let weeks = month_grid(2026, 11);
        assert_eq!(weeks[0], [None, None, None, None, None, None, Some(1)]);
        assert_eq!(weeks.iter().flatten().flatten().count(), 30);
        // A leap February.
        assert_eq!(month_grid(2028, 2).iter().flatten().flatten().count(), 29);
        let picked = day_ms(2027, 3, 14).unwrap();
        assert_eq!(format_day(picked), "Mar 14, 2027");
        assert_eq!(month_of(picked), (2027, 3));
    }
}
