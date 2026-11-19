//! Computer use: let an agent see and work a desktop app the way a person
//! would. It reads a window as an accessibility tree (numbered elements, an
//! optional screenshot), then clicks, types, presses keys, scrolls or sets
//! values. Each platform reads and acts through its own accessibility layer
//! (macOS Accessibility, Windows UI Automation, Linux AT-SPI), with
//! synthetic mouse and keyboard input as the fallback.
//!
//! One worker thread owns the platform state, so the element handles a
//! snapshot keeps stay valid for the actions that follow it. The app runs
//! that worker in a helper process ([`use_helper`]), so a hung or crashing
//! app costs a restart of the helper, never Dragent. Whether an agent may act
//! at all (on/off, allowed apps, asking first) is the app's decision, made
//! before [`run`] is called.

mod clipboard;
pub mod errors;
mod helper;
pub use helper::{serve, stop_helper, use_helper, SERVICE_ARG};
pub mod keys;
mod options;
pub use options::TargetOptions;
pub mod tree;

// Under test the fake desktop stands in, so the real one goes unused.
#[cfg(target_os = "linux")]
#[cfg_attr(test, allow(dead_code))]
mod linux;
#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
#[cfg_attr(test, allow(dead_code))]
mod unsupported;
#[cfg(target_os = "windows")]
#[cfg_attr(test, allow(dead_code))]
mod win;

#[cfg(all(target_os = "linux", not(test)))]
use linux as platform;
#[cfg(all(target_os = "macos", not(test)))]
use macos as platform;
#[cfg(all(
    not(test),
    not(any(target_os = "macos", target_os = "windows", target_os = "linux"))
))]
use unsupported as platform;
#[cfg(all(target_os = "windows", not(test)))]
use win as platform;

#[cfg(test)]
mod testing;
#[cfg(test)]
use testing as platform;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use tree::{AppInfo, Rect, WindowInfo};

/// Most elements a snapshot numbers; the rest are cut with a note.
const SHOWN_ELEMENTS: usize = 700;
/// Most elements read from a window before rendering.
const READ_ELEMENTS: usize = 3000;
/// How long one request may take, including a slow app.
const ANSWER_WITHIN: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Down,
    Up,
    Left,
    Right,
}

impl Direction {
    pub fn word(self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Up => "up",
            Direction::Left => "left",
            Direction::Right => "right",
        }
    }
}

/// What an agent asks for. Tool names are the snake_case variant names.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum Request {
    Permissions,
    Capabilities,
    /// Internal wrapper for window and observation options.
    WithOptions {
        request: Box<Request>,
        options: TargetOptions,
    },
    Apps,
    Windows {
        app: String,
    },
    Snapshot {
        app: String,
        /// Window number from `windows`, 1-based; the focused one otherwise.
        #[serde(default)]
        window: Option<usize>,
        #[serde(default = "yes")]
        screenshot: bool,
    },
    Click {
        app: String,
        /// Element number from the latest snapshot of this app.
        #[serde(default)]
        element: Option<usize>,
        /// Window-relative points, used when no element is given.
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        count: u32,
    },
    SetValue {
        app: String,
        element: usize,
        value: String,
    },
    Type {
        app: String,
        text: String,
    },
    Key {
        app: String,
        key: String,
    },
    Scroll {
        app: String,
        #[serde(default)]
        element: Option<usize>,
        #[serde(default)]
        direction: Direction,
        #[serde(default = "five")]
        amount: u32,
    },
    Focus {
        app: String,
    },
    Paste {
        app: String,
        text: String,
    },
    PerformAction {
        app: String,
        element: usize,
        name: String,
    },
    Drag {
        app: String,
        #[serde(default)]
        from_element: Option<usize>,
        #[serde(default)]
        to_element: Option<usize>,
        #[serde(default)]
        from_x: Option<f64>,
        #[serde(default)]
        from_y: Option<f64>,
        #[serde(default)]
        to_x: Option<f64>,
        #[serde(default)]
        to_y: Option<f64>,
    },
    /// The app's menu bar: each menu and its items, or the items of the
    /// menu `path` names ("File", "File > Export").
    Menus {
        app: String,
        #[serde(default)]
        path: Option<String>,
    },
    /// Choose the menu item `path` names: "File > Export > PDF…".
    ChooseMenu {
        app: String,
        path: String,
    },
    /// Move a window (screen points), resize it, or change its state.
    SetWindow {
        app: String,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        width: Option<f64>,
        #[serde(default)]
        height: Option<f64>,
        #[serde(default)]
        state: Option<WindowState>,
    },
    ModifiedClick {
        app: String,
        #[serde(default)]
        element: Option<usize>,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        count: u32,
        modifiers: String,
    },
    ScrollAt {
        app: String,
        x: f64,
        y: f64,
        #[serde(default)]
        direction: Direction,
        #[serde(default = "five")]
        amount: u32,
    },
}

fn yes() -> bool {
    true
}
fn one() -> u32 {
    1
}
fn five() -> u32 {
    5
}

impl Request {
    /// Read a tool call: `tool` is the request's name (`click`), `arguments`
    /// its fields.
    pub fn from_tool(tool: &str, arguments: &Value) -> Result<Self, String> {
        let mut object = match arguments {
            Value::Object(map) => map.clone(),
            Value::Null => serde_json::Map::new(),
            _ => return Err("Arguments must be an object.".into()),
        };
        object.retain(|_, value| !value.is_null());
        let name = tool.replace('-', "_");
        if matches!(name.as_str(), "press_key" | "hotkey") {
            let key = object
                .get("key")
                .and_then(Value::as_str)
                .ok_or("Name a key")?;
            let combo = keys::parse(key)?;
            let modified = combo.meta || combo.ctrl || combo.alt || combo.shift;
            if (name == "hotkey") != modified {
                return Err(
                    "Use press-key for a single key, and hotkey for a modifier chord.".into(),
                );
            }
        }
        let name = match name.as_str() {
            "list_apps" => "apps",
            "list_windows" => "windows",
            "get_app_state" => "snapshot",
            "type_text" => "type",
            "press_key" | "hotkey" => "key",
            "paste_text" => "paste",
            "perform_secondary_action" => "perform_action",
            "menu" => "choose_menu",
            "window" | "move_window" | "resize_window" => "set_window",
            other => other,
        };
        if name == "with_options" {
            return Err("Use a computer command name.".into());
        }
        if name == "scroll"
            && (object.contains_key("x") || object.contains_key("y"))
            && object.contains_key("element")
        {
            return Err(
                "invalid_argument: Choose a scroll element or coordinates, rather than both."
                    .into(),
            );
        }
        let name = if name == "click"
            && object
                .get("modifiers")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        {
            "modified_click"
        } else if name == "scroll" && (object.contains_key("x") || object.contains_key("y")) {
            "scroll_at"
        } else {
            name
        };
        object.insert("tool".into(), Value::String(name.into()));
        let request: Self = serde_json::from_value(Value::Object(object)).map_err(|error| {
            let message = error.to_string();
            if message.contains("unknown variant") {
                format!("“{tool}” isn't a computer command.")
            } else {
                format!("{tool}: {message}")
            }
        })?;
        request.validate()?;
        let fields = serde_json::to_value(&request).map_err(|e| e.to_string())?;
        for key in object_keys(arguments) {
            if fields.get(key).is_none()
                && !matches!(
                    key,
                    "action"
                        | "window_id"
                        | "window_index"
                        | "restore_window"
                        | "screenshot"
                        | "no_screenshot"
                )
            {
                return Err(format!("invalid_argument: {tool} does not accept {key}."));
            }
        }
        let options = TargetOptions::from_arguments(arguments)?;
        if request.app().is_none()
            && (options.restore_window
                || options.window_id.is_some()
                || options.window_index.is_some())
        {
            return Err("invalid_argument: Window selectors require an app target.".into());
        }
        if matches!(
            request,
            Self::Snapshot {
                window: Some(_),
                ..
            }
        ) && (options.window_id.is_some() || options.window_index.is_some())
        {
            return Err("invalid_argument: Choose only one window selector.".into());
        }
        if options != TargetOptions::default() {
            Ok(Self::WithOptions {
                request: Box::new(request),
                options,
            })
        } else {
            Ok(request)
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.app().is_some_and(|app| app.trim().is_empty()) {
            return Err("Name an app.".into());
        }
        match self {
            Self::WithOptions { request, options } => {
                TargetOptions::from_arguments(
                    &serde_json::to_value(options).map_err(|e| e.to_string())?,
                )?;
                request.validate()?;
            }
            Self::ModifiedClick {
                app,
                element,
                x,
                y,
                button,
                count,
                modifiers,
            } => {
                Self::Click {
                    app: app.clone(),
                    element: *element,
                    x: *x,
                    y: *y,
                    button: *button,
                    count: *count,
                }
                .validate()?;
                keys::modifiers(modifiers)?;
            }
            Self::Paste { text, .. } if text.contains('\0') || text.len() > 1_048_576 => {
                return Err(
                    "Clipboard text must be at most 1 MiB and contain no NUL characters.".into(),
                )
            }
            Self::PerformAction { name, .. } if name.trim().is_empty() => {
                return Err("Name an advertised accessibility action.".into())
            }
            Self::ChooseMenu { path, .. } if menu_path(path).is_empty() => {
                return Err(
                    "invalid_argument: Name a menu item by its path, like \"File > Export > PDF…\"."
                        .into(),
                )
            }
            Self::SetWindow {
                x,
                y,
                width,
                height,
                state,
                ..
            } => {
                if x.is_some() != y.is_some() || width.is_some() != height.is_some() {
                    return Err("invalid_argument: Give x with y, and width with height.".into());
                }
                if x.is_none() && width.is_none() && state.is_none() {
                    return Err("invalid_argument: Say where to move it, what size, or minimize/restore/zoom.".into());
                }
                let fine = |value: &Option<f64>, low: f64| {
                    value.is_none_or(|value| value.is_finite() && value >= low && value <= 20_000.0)
                };
                if !fine(x, -20_000.0) || !fine(y, -20_000.0) || !fine(width, 50.0) || !fine(height, 50.0) {
                    return Err("invalid_argument: Use screen points: a size from 50 up, positions within ±20000.".into());
                }
            }
            Self::ScrollAt { x, y, amount, .. } => {
                if !x.is_finite()
                    || !y.is_finite()
                    || *x < 0.0
                    || *y < 0.0
                    || !(1..=50).contains(amount)
                {
                    return Err(
                        "Use valid window coordinates and a scroll amount from 1 to 50.".into(),
                    );
                }
            }
            Self::Drag {
                from_element,
                to_element,
                from_x,
                from_y,
                to_x,
                to_y,
                ..
            } => {
                let coords = [from_x, from_y, to_x, to_y];
                let elements = from_element.is_some() && to_element.is_some();
                if elements {
                    if coords.iter().any(|c| c.is_some()) {
                        return Err("Use a pair of drag elements or four coordinates.".into());
                    }
                } else if from_element.is_some()
                    || to_element.is_some()
                    || !coords
                        .iter()
                        .all(|c| c.is_some_and(|n| n.is_finite() && n >= 0.0))
                {
                    return Err(
                        "Drag requires two element indexes or all four window coordinates.".into(),
                    );
                }
            }
            Self::Click {
                element,
                x,
                y,
                count,
                ..
            } => {
                if !(1..=3).contains(count) {
                    return Err("Click count must be 1, 2 or 3.".into());
                }
                match (element, x, y) {
                    (Some(_), None, None) => {}
                    (None, Some(x), Some(y))
                        if x.is_finite() && y.is_finite() && *x >= 0.0 && *y >= 0.0 => {}
                    _ => {
                        return Err(
                            "Use an element number or a pair of nonnegative x/y coordinates."
                                .into(),
                        )
                    }
                }
            }
            Self::Snapshot {
                window: Some(0), ..
            } => return Err("Window numbers start at 1.".into()),
            Self::Scroll { amount, .. } if !(1..=50).contains(amount) => {
                return Err("Scroll amount must be between 1 and 50.".into())
            }
            Self::Type { text, .. } | Self::SetValue { value: text, .. }
                if text.len() > 1_048_576 =>
            {
                return Err("Text exceeds 1 MiB.".into())
            }
            Self::Key { key, .. } => {
                keys::parse(key)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn bind_app(&mut self, target: String) {
        match self {
            Self::Permissions | Self::Capabilities | Self::Apps => {}
            Self::WithOptions { request, .. } => request.bind_app(target),
            Self::Paste { app, .. }
            | Self::PerformAction { app, .. }
            | Self::Drag { app, .. }
            | Self::ModifiedClick { app, .. }
            | Self::ScrollAt { app, .. }
            | Self::Menus { app, .. }
            | Self::ChooseMenu { app, .. }
            | Self::SetWindow { app, .. } => *app = target,
            Self::Windows { app }
            | Self::Snapshot { app, .. }
            | Self::Click { app, .. }
            | Self::SetValue { app, .. }
            | Self::Type { app, .. }
            | Self::Key { app, .. }
            | Self::Scroll { app, .. }
            | Self::Focus { app } => *app = target,
        }
    }

    pub fn app(&self) -> Option<&str> {
        match self {
            Request::Permissions | Request::Capabilities | Request::Apps => None,
            Request::WithOptions { request, .. } => request.app(),
            Request::Paste { app, .. }
            | Request::PerformAction { app, .. }
            | Request::Drag { app, .. }
            | Request::ModifiedClick { app, .. }
            | Request::ScrollAt { app, .. }
            | Request::Menus { app, .. }
            | Request::ChooseMenu { app, .. }
            | Request::SetWindow { app, .. } => Some(app),
            Request::Windows { app }
            | Request::Snapshot { app, .. }
            | Request::Click { app, .. }
            | Request::SetValue { app, .. }
            | Request::Type { app, .. }
            | Request::Key { app, .. }
            | Request::Scroll { app, .. }
            | Request::Focus { app } => Some(app),
        }
    }

    /// Every action that changes desktop state needs the confirmation policy.
    pub fn changes_things(&self) -> bool {
        if let Self::WithOptions { request, options } = self {
            return request.changes_things() || options.restore_window;
        }
        matches!(
            self,
            Request::Paste { .. }
                | Request::PerformAction { .. }
                | Request::Drag { .. }
                | Request::ModifiedClick { .. }
                | Request::ScrollAt { .. }
                | Request::Click { .. }
                | Request::SetValue { .. }
                | Request::Type { .. }
                | Request::Key { .. }
                | Request::Focus { .. }
                | Request::Scroll { .. }
                | Request::ChooseMenu { .. }
                | Request::SetWindow { .. }
        )
    }
}

/// A window state `set_window` can ask for.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    Minimize,
    /// Out of the Dock (or full screen) and in front.
    Restore,
    /// The app's own zoom: fills the screen, or back.
    Zoom,
}

/// What `set_window` changes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowChange {
    pub position: Option<(f64, f64)>,
    pub size: Option<(f64, f64)>,
    pub state: Option<WindowState>,
}

/// One item of an app's menus.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MenuEntry {
    pub title: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    /// It opens a submenu (its items are in `children` when read).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub submenu: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MenuEntry>,
}

/// `File > Export > PDF…` as its titles.
pub fn menu_path(path: &str) -> Vec<String> {
    path.split('>')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// A menu shortcut as AT-SPI gives it (the accelerator part of a key
/// binding, like `<Control><Shift>s` or `<Primary>o`) in the words the
/// other platforms use: `Ctrl+Shift+S`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn accelerator_text(raw: &str) -> Option<String> {
    let mut rest = raw.trim();
    let mut parts: Vec<String> = Vec::new();
    while let Some(after) = rest.strip_prefix('<') {
        let (modifier, tail) = after.split_once('>')?;
        parts.push(
            match modifier.to_lowercase().as_str() {
                "control" | "ctrl" | "primary" => "Ctrl",
                "shift" => "Shift",
                "alt" | "mod1" => "Alt",
                "super" | "meta" | "mod4" => "Super",
                _ => return None,
            }
            .to_string(),
        );
        rest = tail;
    }
    let key = rest.trim();
    if key.is_empty() {
        return None;
    }
    let mut chars = key.chars();
    parts.push(match (chars.next(), chars.next()) {
        (Some(only), None) => only.to_uppercase().to_string(),
        _ => key.to_string(),
    });
    Some(parts.join("+"))
}

/// Whether a menu title is the one asked for: case, spacing and a trailing
/// ellipsis don't matter.
pub fn same_menu_title(title: &str, wanted: &str) -> bool {
    let normal = |text: &str| {
        let text = text.trim().to_lowercase().replace("...", "…");
        text.trim_end_matches('…').trim().to_string()
    };
    !wanted.trim().is_empty() && normal(title) == normal(wanted)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Granted,
    Missing,
    /// This platform doesn't ask for it.
    NotNeeded,
    /// Can't be had here (a missing service or a session type).
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Permissions {
    pub platform: String,
    /// Reading and acting through the accessibility layer.
    pub accessibility: Access,
    pub screen_recording: Access,
    /// Synthetic mouse and keyboard input.
    pub input: Access,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Accessibility,
    ScreenRecording,
}

/// A request's answer: text for people and agents, and the same as data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub text: String,
    pub data: Value,
}

/// What a request will touch, resolved before it runs: for the allowed-apps
/// check and the confirmation the person sees.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub pid: Option<u32>,
    pub target_window: Option<String>,
    /// The snapshot an element number came from: an action approved
    /// against it is refused once a newer snapshot replaced it.
    pub snapshot_id: Option<u64>,
    pub app_name: String,
    pub app_id: String,
    /// `Click button “Save”`.
    pub action: String,
}

/// Numbers snapshots, so an approval can name the one it was made against
/// (also across the helper process boundary, where instants don't travel).
static SNAPSHOT_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct Snapshot<H = platform::Handle> {
    id: u64,
    created: std::time::Instant,
    window: WindowInfo,
    elements: Vec<tree::Element<H>>,
}

struct State {
    platform: Result<platform::Platform, String>,
    scope: String,
    options: TargetOptions,
    clipboard: Option<arboard::Clipboard>,
    snapshots: HashMap<(String, u32), Snapshot>,
}

type Job = Box<dyn FnOnce(&mut State) + Send>;

static WORKER: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);

fn spawn_worker() -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let _ = std::thread::Builder::new()
        .name("dragent-computer".into())
        .spawn(move || {
            let mut state = State {
                platform: platform::Platform::new(),
                scope: String::new(),
                options: TargetOptions::default(),
                clipboard: None,
                snapshots: HashMap::new(),
            };
            for job in rx {
                job(&mut state);
            }
        });
    tx
}

/// Serialize desktop access. Queued jobs expire before starting; a slow running
/// action retains the worker so a retry cannot inject input concurrently.
fn call<T: Send + 'static>(
    job: impl FnOnce(&mut State) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    let deadline = std::time::Instant::now() + ANSWER_WITHIN;
    let job: Job = Box::new(move |state| {
        if std::time::Instant::now() >= deadline {
            return;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(state)))
            .unwrap_or_else(|_| Err("Computer use stopped unexpectedly. Try again.".into()));
        let _ = tx.send(result);
    });
    let sender = {
        let mut worker = WORKER.lock().unwrap_or_else(|p| p.into_inner());
        worker.get_or_insert_with(spawn_worker).clone()
    };
    if let Err(mpsc::SendError(job)) = sender.send(job) {
        let fresh = spawn_worker();
        *WORKER.lock().unwrap_or_else(|p| p.into_inner()) = Some(fresh.clone());
        fresh
            .send(job)
            .map_err(|_| "Computer use couldn't start.".to_string())?;
    }
    match rx.recv_timeout(ANSWER_WITHIN) {
        Ok(result) => result,
        Err(_) => {
            Err("The app did not answer in time. An action may still be finishing; inspect the app before retrying.".into())
        }
    }
}

/// Carry out a request (the app has already allowed it).
pub fn run(request: Request) -> Result<Outcome, String> {
    run_for("local", request, None)
}

/// What a request would touch, without doing it.
pub fn preview(request: Request) -> Result<Preview, String> {
    preview_for("local", request)
}

pub fn preview_for(scope: &str, request: Request) -> Result<Preview, String> {
    request.validate()?;
    if helper::active() {
        let call = helper::Call::Preview {
            scope: scope.into(),
            request,
        };
        return helper::request(call, helper::PREVIEW_WITHIN, || Ok(()));
    }
    preview_here(scope, request)
}

pub(crate) fn preview_here(scope: &str, request: Request) -> Result<Preview, String> {
    request.validate()?;
    let scope = scope.to_string();
    call(move |state| {
        state.scope = scope;
        let (request, options) = split_request(request);
        state.options = options;
        let Some(query) = request.app() else {
            return Ok(Preview {
                pid: None,
                snapshot_id: None,
                target_window: None,
                app_name: String::new(),
                app_id: String::new(),
                action: describe(&request, None),
            });
        };
        let platform = state.platform.as_mut().map_err(|e| e.clone())?;
        let apps = platform.apps()?;
        let app = usable(&apps, query)?.clone();
        let target_window = if matches!(request, Request::Windows { .. }) {
            None
        } else {
            let windows = platform.windows(&app)?;
            let legacy = if let Request::Snapshot { window, .. } = &request {
                *window
            } else {
                None
            };
            let cached = state.snapshots.get(&(state.scope.clone(), app.pid));
            let selected = if state.options.window_id.is_none()
                && state.options.window_index.is_none()
                && legacy.is_none()
            {
                cached
                    .and_then(|s| windows.iter().find(|w| w.id == s.window.id))
                    .or(windows.iter().find(|w| w.focused))
                    .or(windows.first())
                    .ok_or("window_not_found: No window is available.")?
            } else {
                select_window(&windows, legacy, &state.options)?
            };
            if let Some(cached) = cached {
                let uses_element = matches!(
                    request,
                    Request::Click {
                        element: Some(_),
                        ..
                    } | Request::Scroll {
                        element: Some(_),
                        ..
                    } | Request::ModifiedClick {
                        element: Some(_),
                        ..
                    } | Request::SetValue { .. }
                        | Request::PerformAction { .. }
                        | Request::Drag {
                            from_element: Some(_),
                            ..
                        }
                );
                if uses_element && selected.id != cached.window.id {
                    return Err("window_stale: Take a new snapshot of the selected window before choosing elements.".into());
                }
            }
            Some(selected.id.clone())
        };
        let index = match &request {
            Request::Click { element, .. } | Request::Scroll { element, .. } => *element,
            Request::SetValue { element, .. } | Request::PerformAction { element, .. } => {
                Some(*element)
            }
            Request::ModifiedClick { element, .. } => *element,
            Request::Drag { from_element, .. } => *from_element,
            _ => None,
        };
        let label = if index.is_some() {
            target(&state.snapshots, &state.scope, &app, index)?
                .0
                .map(|e| e.label.clone())
        } else {
            None
        };
        let snapshot_id = index.and_then(|_| {
            state
                .snapshots
                .get(&(state.scope.clone(), app.pid))
                .map(|s| s.id)
        });
        let mut action = describe(&request, label.as_deref());
        if state.options.restore_window {
            action.push_str(" (bring the selected window forward)");
        }
        if let Some(id) = &target_window {
            if let Some(window) = platform.windows(&app)?.iter().find(|w| &w.id == id) {
                action.push_str(&format!(" · {}", quoted(&window.title, 65)));
            }
        }
        Ok(Preview {
            pid: Some(app.pid),
            snapshot_id,
            target_window,
            action,
            app_name: app.name,
            app_id: app.id,
        })
    })
}

/// Execute in a caller's snapshot scope, optionally bound to a previously approved app.
pub fn run_for(
    scope: &str,
    request: Request,
    approved: Option<Preview>,
) -> Result<Outcome, String> {
    run_for_guarded(scope, request, approved, || Ok(()))
}

/// Recheck the app's authorization after waiting in the worker queue, before
/// resolving the target or dispatching any desktop input.
pub fn run_for_guarded(
    scope: &str,
    request: Request,
    approved: Option<Preview>,
    authorize: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<Outcome, String> {
    request.validate()?;
    if helper::active() {
        let call = helper::Call::Run {
            scope: scope.into(),
            request,
            approved,
        };
        return helper::request(call, helper::RUN_WITHIN, authorize);
    }
    run_here(scope, request, approved, authorize)
}

pub(crate) fn run_here(
    scope: &str,
    mut request: Request,
    approved: Option<Preview>,
    authorize: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<Outcome, String> {
    request.validate()?;
    let scope = scope.to_string();
    call(move |state| {
        authorize()?;
        state.scope = scope;
        let (inner, options) = split_request(request);
        request = inner;
        state.options = options;
        if let Some(approved) = approved {
            if let Some(pid) = approved.pid {
                let platform = state.platform.as_mut().map_err(|e| e.clone())?;
                let apps = platform.apps()?;
                let app = usable(&apps, &format!("pid:{pid}"))?;
                if app.id != approved.app_id || app.name != approved.app_name {
                    return Err("The approved app changed. Make a new request.".into());
                }
                if let Some(id) = approved.snapshot_id {
                    if state
                        .snapshots
                        .get(&(state.scope.clone(), pid))
                        .map(|s| s.id)
                        != Some(id)
                    {
                        return Err("The snapshot changed while approval was pending. Take a new snapshot and request the action again.".into());
                    }
                }
                // Resolve again by the approved process, never a partial name.
                request.bind_app(format!("pid:{pid}"));
                if let Some(id) = approved.target_window {
                    state.options.window_id = Some(id);
                    state.options.window_index = None;
                }
            }
        }
        let changes = request.changes_things() || state.options.restore_window;
        let affected_pid = if changes {
            let platform = state.platform.as_mut().map_err(|e| e.clone())?;
            let apps = platform.apps()?;
            Some(usable(&apps, request.app().unwrap_or_default())?.pid)
        } else {
            None
        };
        state
            .snapshots
            .retain(|_, snapshot| snapshot.created.elapsed() < Duration::from_secs(120));
        let is_action = request.changes_things();
        if !is_action && state.options.restore_window {
            if let Some(pid) = affected_pid {
                state.snapshots.retain(|(_, app_pid), _| *app_pid != pid);
            }
        }
        let mut result = execute(state, request);
        if let Some(pid) = affected_pid {
            let rejected_without_delivery = result.as_ref().err().is_some_and(|error| {
                matches!(
                    errors::details(error)["code"].as_str(),
                    Some(
                        "element_not_found"
                            | "window_stale"
                            | "window_not_found"
                            | "invalid_argument"
                            | "app_not_found"
                            | "app_blocked"
                    )
                )
            });
            if is_action && !rejected_without_delivery {
                state.snapshots.retain(|(_, app_pid), _| *app_pid != pid);
            }
            if is_action {
                if let Ok(outcome) = &mut result {
                    let metadata = outcome.data.clone();
                    // Observe the same selected window. A failed read never turns an
                    // already delivered action into a safe-to-retry error.
                    state.options.restore_window = false;
                    match execute(
                        state,
                        Request::Snapshot {
                            app: format!("pid:{pid}"),
                            window: None,
                            screenshot: state.options.screenshot.unwrap_or(true),
                        },
                    ) {
                        Ok(observed) => {
                            outcome.text.push_str("\n\n");
                            outcome.text.push_str(&observed.text);
                            outcome.data = observed.data;
                            outcome.data["action"] = metadata;
                        }
                        Err(error) => {
                            outcome.data = json!({"action":metadata,"observation_error":error});
                            outcome.text.push_str(&format!("\nFresh observation unavailable: {error}. Inspect the app before retrying."));
                        }
                    }
                }
            }
        }
        result
    })
}

/// What this platform allows now.
pub fn permissions() -> Permissions {
    if helper::active() {
        return helper::request(helper::Call::Permissions, helper::CHECK_WITHIN, || Ok(()))
            .unwrap_or_else(unavailable);
    }
    permissions_here()
}

pub(crate) fn permissions_here() -> Permissions {
    call(|state| match state.platform.as_mut() {
        Ok(platform) => Ok(platform.permissions()),
        Err(error) => Ok(unavailable(error.clone())),
    })
    .unwrap_or_else(unavailable)
}

fn unavailable(error: String) -> Permissions {
    Permissions {
        platform: std::env::consts::OS.into(),
        accessibility: Access::Unavailable,
        screen_recording: Access::Unavailable,
        input: Access::Unavailable,
        notes: vec![error],
    }
}

/// The process computer use works for: Dragent, also from its helper. Its
/// own windows are never a target.
#[cfg_attr(test, allow(dead_code))]
fn owner_pid() -> u32 {
    static OWNER: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *OWNER.get_or_init(|| {
        std::env::var(helper::OWNER_ENV)
            .ok()
            .and_then(|pid| pid.parse().ok())
            .unwrap_or_else(std::process::id)
    })
}

/// Ask the system for a permission (macOS shows its prompt once).
pub fn request_permission(kind: PermissionKind) -> bool {
    platform::request_permission(kind)
}

/// Open the system settings page where a permission is granted.
pub fn open_permission_settings(kind: PermissionKind) -> Result<(), String> {
    platform::open_permission_settings(kind)
}

fn usable<'a>(apps: &'a [AppInfo], query: &str) -> Result<&'a AppInfo, String> {
    let app = tree::resolve_app(apps, query)?;
    if tree::is_blocked(app) {
        return Err(format!(
            "{} holds passwords and other secrets; computer use never reads or works it.",
            app.name
        ));
    }
    Ok(app)
}

fn quoted(text: &str, max: usize) -> String {
    let short: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        format!("“{short}…”")
    } else {
        format!("“{short}”")
    }
}

/// `Click button “Save”`, `Type “hello”`, `Press cmd+s`.
pub fn describe(request: &Request, label: Option<&str>) -> String {
    let target = |element: &Option<usize>| match (label, element) {
        (Some(label), _) => label.to_string(),
        (None, Some(index)) => format!("element {index}"),
        (None, None) => String::new(),
    };
    match request {
        Request::Permissions => "Check computer-use permissions".into(),
        Request::Capabilities => "Check available desktop capabilities".into(),
        Request::WithOptions { request, options } => format!(
            "{}{}",
            describe(request, label),
            if options.restore_window {
                " (bring the window forward)"
            } else {
                ""
            }
        ),
        Request::Paste { text, .. } => format!("Paste {} characters", text.chars().count()),
        Request::PerformAction { element, name, .. } => {
            format!("Perform {name} on {}", target(&Some(*element)))
        }
        Request::Drag {
            from_element,
            to_element,
            ..
        } => format!("Drag {} to {}", target(from_element), target(to_element)),
        Request::ModifiedClick {
            modifiers, element, ..
        } => format!("Click {} while holding {modifiers}", target(element)),
        Request::ScrollAt {
            direction,
            amount,
            x,
            y,
            ..
        } => format!("Scroll {} by {amount} at {x:.0}, {y:.0}", direction.word()),
        Request::Apps => "List running apps".into(),
        Request::Windows { .. } => "List the app's windows".into(),
        Request::Snapshot { screenshot, .. } => {
            if *screenshot {
                "Read the window and take a screenshot".into()
            } else {
                "Read the window".into()
            }
        }
        Request::Click {
            element,
            x,
            y,
            button,
            count,
            ..
        } => {
            let verb = match (button, count) {
                (Button::Right, _) => "Right-click",
                (Button::Middle, _) => "Middle-click",
                (_, 2) => "Double-click",
                (_, n) if *n > 2 => "Click repeatedly",
                _ => "Click",
            };
            match (element, x, y) {
                (Some(_), _, _) => format!("{verb} {}", target(element)),
                (None, Some(x), Some(y)) => format!("{verb} at {:.0}, {:.0} in the window", x, y),
                _ => verb.to_string(),
            }
        }
        Request::SetValue { element, value, .. } => {
            format!(
                "Fill {} with {}",
                target(&Some(*element)),
                quoted(value, 60)
            )
        }
        Request::Type { text, .. } => format!("Type {}", quoted(text, 80)),
        Request::Key { key, .. } => format!("Press {key}"),
        Request::Scroll {
            element,
            direction,
            amount,
            ..
        } => {
            let place = if element.is_some() {
                format!(" in {}", target(element))
            } else {
                String::new()
            };
            format!("Scroll {}{place} by {amount}", direction.word())
        }
        Request::Focus { .. } => "Bring the app to the front".into(),
        Request::Menus { path, .. } => match path {
            Some(path) => format!("Read the {path} menu"),
            None => "Read the menu bar".into(),
        },
        Request::ChooseMenu { path, .. } => format!("Choose {} from the menus", menu_path(path).join(" › ")),
        Request::SetWindow {
            x,
            y,
            width,
            height,
            state,
            ..
        } => {
            let mut parts = Vec::new();
            if let (Some(x), Some(y)) = (x, y) {
                parts.push(format!("move the window to {x:.0}, {y:.0}"));
            }
            if let (Some(width), Some(height)) = (width, height) {
                parts.push(format!("resize it to {width:.0}×{height:.0}"));
            }
            if let Some(state) = state {
                parts.push(match state {
                    WindowState::Minimize => "minimize it".into(),
                    WindowState::Restore => "restore it".into(),
                    WindowState::Zoom => "zoom it".into(),
                });
            }
            let text = parts.join(", ");
            let mut chars = text.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    }
}

fn execute(state: &mut State, request: Request) -> Result<Outcome, String> {
    if matches!(request, Request::Permissions) && state.platform.is_err() {
        let permissions = Permissions {
            platform: std::env::consts::OS.into(),
            accessibility: Access::Unavailable,
            screen_recording: Access::Unavailable,
            input: Access::Unavailable,
            notes: vec![state.platform.as_ref().err().cloned().unwrap_or_default()],
        };
        return Ok(Outcome {
            text: permissions.notes.join("\n"),
            data: serde_json::to_value(permissions).unwrap_or(Value::Null),
        });
    }
    if matches!(request, Request::Capabilities) {
        return Ok(capabilities(state));
    }
    let platform = state.platform.as_mut().map_err(|e| e.clone())?;
    match request {
        Request::Permissions => {
            let permissions = platform.permissions();
            let line = |name: &str, access: Access| format!("{name}: {access:?}");
            let mut text = vec![
                format!("Platform: {}", permissions.platform),
                line("Accessibility", permissions.accessibility),
                line("Screen recording", permissions.screen_recording),
                line("Mouse and keyboard", permissions.input),
            ];
            text.extend(permissions.notes.iter().cloned());
            Ok(Outcome {
                text: text.join("\n"),
                data: serde_json::to_value(&permissions).unwrap_or(Value::Null),
            })
        }
        Request::Apps => {
            let apps = platform.apps()?;
            let text = apps
                .iter()
                .map(|app| {
                    let mut line = format!("{} — {} (pid {})", app.name, app.id, app.pid);
                    if app.active {
                        line.push_str(" · in front");
                    }
                    if tree::is_blocked(app) {
                        line.push_str(" · off limits");
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(Outcome {
                text: if text.is_empty() {
                    "No apps with windows are running.".into()
                } else {
                    text
                },
                data: json!({ "apps": apps }),
            })
        }
        Request::Windows { app } => {
            let apps = platform.apps()?;
            let app = usable(&apps, &app)?.clone();
            let windows = platform.windows(&app)?;
            let text = if windows.is_empty() {
                format!("{} has no open windows.", app.name)
            } else {
                windows
                    .iter()
                    .map(|window| {
                        format!(
                            "{}. [id {}] \"{}\" {:.0}×{:.0} at {:.0},{:.0}{}",
                            window.index + 1,
                            window.id,
                            window.title,
                            window.frame.width,
                            window.frame.height,
                            window.frame.x,
                            window.frame.y,
                            if window.focused { " · focused" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(Outcome {
                text,
                data: json!({ "app": app, "windows": windows }),
            })
        }
        Request::Snapshot {
            app,
            window,
            screenshot,
        } => {
            let apps = platform.apps()?;
            let app = usable(&apps, &app)?.clone();
            let windows = platform.windows(&app)?;
            let mut chosen = select_window(&windows, window, &state.options)?.clone();
            if state.options.restore_window {
                platform.activate(&app, Some(&chosen))?;
                let current = platform.windows(&app)?;
                chosen = current
                    .into_iter()
                    .find(|w| w.id == chosen.id)
                    .ok_or("window_stale: The selected window closed while it was restored.")?;
            }
            let raw = platform.tree(&app, &chosen, READ_ELEMENTS)?;
            let tree::Built {
                elements,
                truncated,
                offscreen,
            } = tree::build(raw, SHOWN_ELEMENTS, Some(chosen.frame));
            let shot = if state.options.screenshot.unwrap_or(screenshot) {
                Some(take_screenshot(platform, &app, &chosen))
            } else {
                None
            };
            let mut text = vec![format!(
                "{} ({}, pid {}) · window {} of {}: \"{}\" {:.0}×{:.0}",
                app.name,
                app.id,
                app.pid,
                chosen.index + 1,
                windows.len(),
                chosen.title,
                chosen.frame.width,
                chosen.frame.height
            )];
            let shot_data = match &shot {
                Some(Ok((path, scale))) => {
                    text.push(format!(
                        "Screenshot: {} ({scale}× pixels: divide pixel positions by {scale} for x/y)",
                        path.display()
                    ));
                    let size = png_size(path).unwrap_or((0, 0));
                    json!({ "path": path, "scale": scale, "format":"png", "width":size.0,"height":size.1 })
                }
                Some(Err(error)) => {
                    text.push(format!("Screenshot: not taken ({error})"));
                    Value::Null
                }
                None => Value::Null,
            };
            text.extend(elements.iter().map(|element| element.line.clone()));
            if offscreen > 0 {
                text.push("More of the page lies outside the window and isn't listed: scroll to bring it into view, then take a new snapshot.".into());
            }
            text.push(if truncated {
                format!(
                    "Showing the first {} elements. Act on one by its number; take a new snapshot after the window changes.",
                    elements.len()
                )
            } else {
                format!(
                    "{} elements. Act on one by its number; take a new snapshot after the window changes.",
                    elements.len()
                )
            });
            let data = json!({
                "snapshot": {"id":format!("{}-{}", app.pid, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()), "tree_text": text.join("\n"), "coordinate_space":"window", "focused_element":elements.iter().find(|e| e.focused).map(|e| e.index)},
                "app": app,
                "window": chosen,
                "elements": elements.iter().map(|e| json!({ "index": e.index, "line": e.line.trim_start(), "frame": e.frame, "actions": e.actions,"focused":e.focused })).collect::<Vec<_>>(),
                "screenshot_status":match &shot {Some(Ok(_))=>json!({"state":"captured"}),Some(Err(error))=>json!({"state":"failed","error":errors::details(error)}),None=>json!({"state":"skipped","reason":"no_screenshot"})},
                "screenshot": shot_data,
                "truncated": truncated,
                "offscreen": offscreen,
            });
            state.snapshots.insert(
                (state.scope.clone(), app.pid),
                Snapshot {
                    id: SNAPSHOT_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                    created: std::time::Instant::now(),
                    window: chosen,
                    elements,
                },
            );
            Ok(Outcome {
                text: text.join("\n"),
                data,
            })
        }
        Request::Focus { app } => {
            let apps = platform.apps()?;
            let app = usable(&apps, &app)?.clone();
            let window = state
                .snapshots
                .get(&(state.scope.clone(), app.pid))
                .map(|s| s.window.clone());
            platform.activate(&app, window.as_ref())?;
            Ok(done(format!("{} is in front.", app.name)))
        }
        Request::Menus { app, path } => {
            let apps = platform.apps()?;
            let app = usable(&apps, &app)?.clone();
            let path = path.as_deref().map(menu_path).unwrap_or_default();
            let entries = platform.menus(&app, &path)?;
            let mut text = String::new();
            render_menus(&entries, 0, &mut text);
            if text.is_empty() {
                text = "No menu items here.".into();
            }
            let heading = if path.is_empty() {
                format!("{}'s menus (choose one with `menu APP \"Menu > Item\"`):\n", app.name)
            } else {
                format!("{} › {}:\n", app.name, path.join(" › "))
            };
            Ok(Outcome {
                text: heading + &text,
                data: json!({"app": app.name, "path": path, "items": entries}),
            })
        }
        Request::ChooseMenu { app, path } => {
            let apps = platform.apps()?;
            let app = usable(&apps, &app)?.clone();
            // Menus act on the app in front, as they do for a person.
            platform.activate(&app, None)?;
            let chosen = platform.choose_menu(&app, &menu_path(&path))?;
            Ok(action_result(
                format!(
                    "Chose {chosen} in {}. A menu item's effect differs by app: snapshot to see what it did.",
                    app.name
                ),
                "accessibility",
                "unverified",
                "menu_item_pressed",
            ))
        }
        Request::SetWindow {
            app,
            x,
            y,
            width,
            height,
            state: window_state,
        } => {
            let apps = platform.apps()?;
            let app = usable(&apps, &app)?.clone();
            let windows = platform.windows(&app)?;
            let options = state.options.clone();
            let window = if options.window_id.is_some() || options.window_index.is_some() {
                select_window(&windows, None, &options)?.clone()
            } else {
                state
                    .snapshots
                    .get(&(state.scope.clone(), app.pid))
                    .and_then(|snapshot| windows.iter().find(|w| w.id == snapshot.window.id))
                    .or_else(|| windows.iter().find(|w| w.focused))
                    .or(windows.first())
                    .cloned()
                    .ok_or("window_not_found: This app has no windows.")?
            };
            let change = WindowChange {
                position: x.zip(y),
                size: width.zip(height),
                state: window_state,
            };
            let frame = platform.set_window(&app, &window, &change)?;
            let close = |a: f64, b: f64| (a - b).abs() <= 2.0;
            let placed = change.position.is_none_or(|(x, y)| close(frame.x, x) && close(frame.y, y));
            let sized = change.size.is_none_or(|(w, h)| close(frame.width, w) && close(frame.height, h));
            let now = format!(
                "“{}” is at {:.0}, {:.0}, {:.0}×{:.0}",
                window.title, frame.x, frame.y, frame.width, frame.height
            );
            let mut outcome = if placed && sized {
                action_result(format!("{now}."), "accessibility", "verified", "frame_read_back")
            } else {
                action_result(
                    format!("{now}: the app didn't take all of it (it may have a minimum or maximum size, or keep windows on screen)."),
                    "accessibility",
                    "partial",
                    "frame_read_back",
                )
            };
            outcome.data["frame"] = json!(frame);
            Ok(outcome)
        }
        other => act(state, other),
    }
}

/// The element and window an action on `app` refers to.
fn target<'a, H>(
    snapshots: &'a HashMap<(String, u32), Snapshot<H>>,
    scope: &str,
    app: &AppInfo,
    element: Option<usize>,
) -> Result<(Option<&'a tree::Element<H>>, Option<&'a WindowInfo>), String> {
    let snapshot = snapshots.get(&(scope.to_string(), app.pid));
    let Some(index) = element else {
        return Ok((None, snapshot.map(|s| &s.window)));
    };
    let snapshot = snapshot.filter(|s| s.created.elapsed() < Duration::from_secs(120)).ok_or_else(|| {
        format!(
            "Take a snapshot of {} first: element numbers come from it (`dragent computer snapshot \"{}\"`).",
            app.name, app.name
        )
    })?;
    let element = snapshot.elements.get(index).ok_or_else(|| {
        format!(
            "The latest snapshot of {} has no element {index} (it has {}). Take a new snapshot.",
            app.name,
            snapshot.elements.len()
        )
    })?;
    Ok((Some(element), Some(&snapshot.window)))
}

fn done(text: String) -> Outcome {
    action_result(text, "synthetic", "unverified", "synthetic_input")
}

const AFTER_CHANGE: &str = "Take a new snapshot before using element numbers again.";

fn act(state: &mut State, request: Request) -> Result<Outcome, String> {
    let platform = state.platform.as_mut().map_err(|e| e.clone())?;
    let apps = platform.apps()?;
    let app = usable(&apps, request.app().unwrap_or_default())?.clone();
    let options = state.options.clone();
    let windows = platform.windows(&app)?;
    let cached = state.snapshots.get(&(state.scope.clone(), app.pid));
    let chosen = select_window(&windows, None, &options)?.clone();
    if let Some(cached) = cached {
        let explicit = options.window_id.is_some() || options.window_index.is_some();
        if explicit && chosen.id != cached.window.id {
            return Err(
                "window_stale: Take a snapshot of the selected window before using its elements."
                    .into(),
            );
        }
        if !windows.iter().any(|w| w.id == cached.window.id) {
            return Err("window_stale: The cached window has closed. Take a new snapshot.".into());
        }
    }
    let active_window = if options.window_id.is_some() || options.window_index.is_some() {
        chosen.clone()
    } else {
        cached.map(|s| s.window.clone()).unwrap_or(chosen.clone())
    };
    state.options.window_id = Some(active_window.id.clone());
    state.options.window_index = None;
    let primary = if cfg!(target_os = "macos") {
        keys::Combo {
            key: keys::Key::Char('a'),
            meta: true,
            ctrl: false,
            alt: false,
            shift: false,
        }
    } else {
        keys::Combo {
            key: keys::Key::Char('a'),
            meta: false,
            ctrl: true,
            alt: false,
            shift: false,
        }
    };
    match request {
        Request::Click {
            element,
            x,
            y,
            button,
            count,
            ..
        } => {
            let count = count.clamp(1, 3);
            let (element, window) = target(&state.snapshots, &state.scope, &app, element)?;
            if let Some(element) = element {
                if button == Button::Left && count == 1 && platform.press(&element.handle)? {
                    return Ok(action_result(
                        format!(
                            "Dispatched an accessibility press on {} in {}.",
                            element.label, app.name
                        ),
                        "accessibility",
                        "unverified",
                        "accessibility_action_unasserted",
                    ));
                }
                let frame = platform
                    .element_frame(&element.handle)
                    .or(element.frame)
                    .filter(|frame| !frame.is_empty())
                    .ok_or_else(|| {
                        format!("{} has no position on screen to click.", element.label)
                    })?;
                platform.activate(&app, window)?;
                let (cx, cy) = frame.center();
                platform.click_at(cx, cy, button, count)?;
                return Ok(done(format!(
                    "Clicked {} in {}. {AFTER_CHANGE}",
                    element.label, app.name
                )));
            }
            let (Some(x), Some(y)) = (x, y) else {
                return Err(
                    "Click an element by its number, or give x and y in the window.".into(),
                );
            };
            let window = match window {
                Some(window) => window.clone(),
                None => active_window.clone(),
            };
            if x < 0.0 || y < 0.0 || x >= window.frame.width || y >= window.frame.height {
                return Err(format!(
                    "{x:.0}, {y:.0} is outside the window ({:.0}×{:.0}).",
                    window.frame.width, window.frame.height
                ));
            }
            platform.activate(&app, Some(&window))?;
            platform.click_at(window.frame.x + x, window.frame.y + y, button, count)?;
            Ok(done(format!(
                "Clicked at {x:.0}, {y:.0} in {}. {AFTER_CHANGE}",
                app.name
            )))
        }
        Request::SetValue { element, value, .. } => {
            let (element, window) = target(&state.snapshots, &state.scope, &app, Some(element))?;
            let element = element.expect("an index was given");
            if platform.set_value(&element.handle, &value)? {
                let actual = if element.secure {
                    None
                } else {
                    platform.read_value(&element.handle)
                };
                let (status, reason) = match &actual {
                    Some(now) if now == &value => ("verified", "value_readback"),
                    Some(_) => ("unverified", "value_mismatch"),
                    None => ("unverified", "readback_unsupported"),
                };
                let mut outcome = action_result(
                    format!(
                        "Requested a value change in {}. Verification: {status} ({reason}).",
                        element.label
                    ),
                    "accessibility",
                    status,
                    reason,
                );
                outcome.data["verification"]["property"] = json!("value");
                return Ok(outcome);
            }
            platform.activate(&app, window)?;
            platform.focus_element(&element.handle)?;
            platform.press_keys(&primary)?;
            platform.type_text(&value)?;
            Ok(done(format!(
                "Typed into {} in {}.",
                element.label, app.name
            )))
        }
        Request::Type { text, .. } => {
            keyboard_window(platform, &app, &active_window, options.restore_window, true)?;
            platform.type_text(&text)?;
            Ok(done(format!(
                "Typed {} characters into {}. {AFTER_CHANGE}",
                text.chars().count(),
                app.name
            )))
        }
        Request::Key { key, .. } => {
            let combo = keys::parse(&key)?;
            keyboard_window(
                platform,
                &app,
                &active_window,
                options.restore_window,
                false,
            )?;
            platform.press_keys(&combo)?;
            Ok(done(format!(
                "Pressed {key} in {}. {AFTER_CHANGE}",
                app.name
            )))
        }
        Request::Scroll {
            element,
            direction,
            amount,
            ..
        } => {
            let (element, window) = target(&state.snapshots, &state.scope, &app, element)?;
            let window = match window {
                Some(window) => window.clone(),
                None => active_window.clone(),
            };
            let frame = element
                .and_then(|element| platform.element_frame(&element.handle).or(element.frame))
                .filter(|frame| !frame.is_empty())
                .unwrap_or(window.frame);
            let (x, y) = frame.center();
            platform.activate(&app, Some(&window))?;
            platform.scroll_at(x, y, direction, amount.clamp(1, 50))?;
            Ok(done(format!(
                "Scrolled {} in {}. {AFTER_CHANGE}",
                direction.word(),
                app.name
            )))
        }
        Request::Paste { text, .. } => {
            keyboard_window(platform, &app, &active_window, options.restore_window, true)?;
            clipboard::paste(&mut state.clipboard, &text, || {
                keyboard_window(platform, &app, &active_window, false, true)?;
                platform.press_keys(&keys::parse("CmdOrCtrl+v")?)
            })?;
            Ok(action_result(
                format!(
                    "Dispatched a paste of {} characters into {}.",
                    text.chars().count(),
                    app.name
                ),
                "clipboard",
                "unverified",
                "clipboard_paste",
            ))
        }
        Request::PerformAction {
            element,
            name: action,
            ..
        } => {
            let (element, _) = target(&state.snapshots, &state.scope, &app, Some(element))?;
            let element = element.expect("element index supplied");
            if !element.actions.iter().any(|name| name == &action) {
                return Err("action_not_supported: Use an action advertised by this element in a fresh snapshot.".into());
            }
            platform.perform_action(&element.handle, &action)?;
            Ok(action_result(
                format!("Dispatched {action} on {}.", element.label),
                "accessibility",
                "unverified",
                "accessibility_action_unasserted",
            ))
        }
        Request::ModifiedClick {
            element,
            x,
            y,
            button,
            count,
            modifiers,
            ..
        } => {
            let (element, _) = target(&state.snapshots, &state.scope, &app, element)?;
            let point = if let Some(element) = element {
                platform
                    .element_frame(&element.handle)
                    .or(element.frame)
                    .filter(|f| !f.is_empty())
                    .map(|f| f.center())
                    .ok_or("element_not_clickable: This element has no pointer target.")?
            } else {
                in_window(&active_window, x.unwrap(), y.unwrap())?
            };
            platform.activate(&app, Some(&active_window))?;
            platform.modified_click(
                point.0,
                point.1,
                button,
                count,
                &keys::modifiers(&modifiers)?,
            )?;
            Ok(action_result(
                "Dispatched the modifier-click.".into(),
                "synthetic",
                "unverified",
                "synthetic_input",
            ))
        }
        Request::ScrollAt {
            x,
            y,
            direction,
            amount,
            ..
        } => {
            let point = in_window(&active_window, x, y)?;
            platform.activate(&app, Some(&active_window))?;
            platform.scroll_at(point.0, point.1, direction, amount)?;
            Ok(done(
                "Dispatched scrolling at the selected coordinates.".into(),
            ))
        }
        Request::Drag {
            from_element,
            to_element,
            from_x,
            from_y,
            to_x,
            to_y,
            ..
        } => {
            let point = |platform: &mut platform::Platform,
                         index: Option<usize>,
                         x: Option<f64>,
                         y: Option<f64>|
             -> Result<(f64, f64), String> {
                if let Some(index) = index {
                    let (element, _) = target(&state.snapshots, &state.scope, &app, Some(index))?;
                    let element = element.unwrap();
                    platform
                        .element_frame(&element.handle)
                        .or(element.frame)
                        .filter(|f| !f.is_empty())
                        .map(|f| f.center())
                        .ok_or_else(|| {
                            "element_not_clickable: A drag endpoint has no visible frame.".into()
                        })
                } else {
                    in_window(&active_window, x.unwrap(), y.unwrap())
                }
            };
            let from = point(platform, from_element, from_x, from_y)?;
            let to = point(platform, to_element, to_x, to_y)?;
            platform.activate(&app, Some(&active_window))?;
            platform.drag(from, to)?;
            Ok(done(
                "Dispatched the drag. Inspect the refreshed window to verify its effect.".into(),
            ))
        }
        _ => Err("Not an action.".into()),
    }
}

/// Screenshots go to a temporary folder that keeps the latest 30.
fn take_screenshot(
    platform: &mut platform::Platform,
    app: &AppInfo,
    window: &WindowInfo,
) -> Result<(PathBuf, f64), String> {
    static SHOTS: Mutex<Option<tempfile::TempDir>> = Mutex::new(None);
    let mut shots = SHOTS.lock().unwrap_or_else(|p| p.into_inner());
    if shots.is_none() {
        *shots = Some(
            tempfile::Builder::new()
                .prefix("dragent-computer-")
                .tempdir()
                .map_err(|e| e.to_string())?,
        );
    }
    let dir = shots.as_ref().unwrap().path().to_path_buf();
    prune(&dir, 30);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let path = dir.join(format!("{}-{stamp}.png", app.pid));
    platform.screenshot(app, window, &path)?;
    let width = png_size(&path).ok_or("the screenshot couldn't be read")?.0;
    let scale = if window.frame.width >= 1.0 {
        ((width as f64 / window.frame.width) * 4.0).round() / 4.0
    } else {
        1.0
    };
    Ok((path, scale.max(0.25)))
}

fn png_size(path: &std::path::Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut bytes = [0u8; 24];
    std::fs::File::open(path)
        .ok()?
        .read_exact(&mut bytes)
        .ok()?;
    (&bytes[..8] == b"\x89PNG\r\n\x1a\n").then(|| {
        (
            u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        )
    })
}

fn prune(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            Some((entry.metadata().ok()?.modified().ok()?, entry.path()))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for (_, path) in &files[..files.len() - keep] {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_menu_shortcuts_read_like_the_other_platforms() {
        assert_eq!(accelerator_text("<Control>o").as_deref(), Some("Ctrl+O"));
        assert_eq!(accelerator_text("<Primary><Shift>s").as_deref(), Some("Ctrl+Shift+S"));
        assert_eq!(accelerator_text("<Alt>F4").as_deref(), Some("Alt+F4"));
        assert_eq!(accelerator_text("Delete").as_deref(), Some("Delete"));
        assert_eq!(accelerator_text(""), None);
        assert_eq!(accelerator_text("<Hyper>x"), None);
    }

    #[test]
    fn tool_calls_become_requests() {
        assert_eq!(
            Request::from_tool("apps", &Value::Null).unwrap(),
            Request::Apps
        );
        let click = Request::from_tool("click", &json!({ "app": "Safari", "element": 4 })).unwrap();
        assert_eq!(
            click,
            Request::Click {
                app: "Safari".into(),
                element: Some(4),
                x: None,
                y: None,
                button: Button::Left,
                count: 1
            }
        );
        assert!(click.changes_things());
        let shot = Request::from_tool("snapshot", &json!({ "app": "Notes" })).unwrap();
        assert!(matches!(
            shot,
            Request::Snapshot {
                screenshot: true,
                window: None,
                ..
            }
        ));
        assert!(!shot.changes_things());
        assert_eq!(
            Request::from_tool(
                "set-value",
                &json!({ "app": "Notes", "element": 2, "value": "hi" })
            )
            .unwrap(),
            Request::SetValue {
                app: "Notes".into(),
                element: 2,
                value: "hi".into()
            }
        );
        assert!(Request::from_tool("explode", &json!({}))
            .unwrap_err()
            .contains("isn't a computer command"));
        assert!(Request::from_tool("click", &json!({})).is_err());
    }

    #[test]
    fn confirmations_say_what_will_happen() {
        let click = Request::from_tool("click", &json!({ "app": "Safari", "element": 4 })).unwrap();
        assert_eq!(
            describe(&click, Some("button “Save”")),
            "Click button “Save”"
        );
        let right = Request::from_tool(
            "click",
            &json!({ "app": "Finder", "x": 10, "y": 20, "button": "right" }),
        )
        .unwrap();
        assert_eq!(
            describe(&right, None),
            "Right-click at 10, 20 in the window"
        );
        let typing =
            Request::from_tool("type", &json!({ "app": "Notes", "text": "hello" })).unwrap();
        assert_eq!(describe(&typing, None), "Type “hello”");
        let key = Request::from_tool("key", &json!({ "app": "Notes", "key": "cmd+s" })).unwrap();
        assert_eq!(describe(&key, None), "Press cmd+s");
        let scroll = Request::from_tool(
            "scroll",
            &json!({ "app": "Notes", "direction": "up", "amount": 3 }),
        )
        .unwrap();
        assert_eq!(describe(&scroll, None), "Scroll up by 3");
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    fn app() -> AppInfo {
        AppInfo {
            name: "Sample".into(),
            id: "sample.app".into(),
            pid: 42,
            active: true,
        }
    }
    fn snapshot(created: std::time::Instant) -> Snapshot<()> {
        Snapshot {
            id: 7,
            created,
            window: WindowInfo {
                id: "test-window".into(),
                index: 0,
                title: "Draft".into(),
                frame: Rect::default(),
                focused: true,
                native: 1,
            },
            elements: vec![tree::Element {
                index: 0,
                line: "[0] button Save".into(),
                label: "button Save".into(),
                frame: None,
                secure: false,
                focused: false,
                actions: vec![],
                handle: (),
            }],
        }
    }
    #[test]
    fn element_references_require_a_fresh_snapshot_from_the_same_caller() {
        let mut snapshots = HashMap::new();
        snapshots.insert(("first".into(), 42), snapshot(std::time::Instant::now()));
        assert!(target(&snapshots, "first", &app(), Some(0)).is_ok());
        assert!(target(&snapshots, "second", &app(), Some(0))
            .err()
            .unwrap()
            .contains("Take a snapshot"));
        assert!(target(&snapshots, "first", &app(), Some(1)).is_err());
        snapshots.insert(
            ("first".into(), 42),
            snapshot(std::time::Instant::now() - Duration::from_secs(121)),
        );
        assert!(target(&snapshots, "first", &app(), Some(0))
            .err()
            .unwrap()
            .contains("Take a snapshot"));
    }
    #[test]
    fn desktop_changes_and_invalid_requests_are_checked_before_approval() {
        for (tool, args) in [
            ("focus", json!({"app":"Sample"})),
            ("scroll", json!({"app":"Sample"})),
            ("type", json!({"app":"Sample","text":"hello"})),
            ("key", json!({"app":"Sample","key":"Enter"})),
            ("click", json!({"app":"Sample","element":0})),
            (
                "set_value",
                json!({"app":"Sample","element":0,"value":"hello"}),
            ),
        ] {
            assert!(
                Request::from_tool(tool, &args).unwrap().changes_things(),
                "{tool}"
            );
        }
        for (tool, args) in [
            ("click", json!({"app":"Sample"})),
            ("click", json!({"app":"Sample", "x":1})),
            ("click", json!({"app":"Sample", "x":1, "y":1, "element":0})),
            ("click", json!({"app":"Sample", "element":0, "count":0})),
            ("snapshot", json!({"app":"Sample", "window":0})),
            ("scroll", json!({"app":"Sample", "amount":0})),
            ("key", json!({"app":"Sample", "key":"nonexistent-key"})),
        ] {
            assert!(Request::from_tool(tool, &args).is_err(), "{tool} {args}");
        }
    }
}

fn split_request(mut request: Request) -> (Request, TargetOptions) {
    let mut options = TargetOptions::default();
    while let Request::WithOptions {
        request: inner,
        options: current,
    } = request
    {
        options = current;
        request = *inner;
    }
    (request, options)
}

fn select_window<'a>(
    windows: &'a [WindowInfo],
    legacy: Option<usize>,
    options: &TargetOptions,
) -> Result<&'a WindowInfo, String> {
    if let Some(id) = &options.window_id {
        return windows.iter().find(|w| &w.id == id).ok_or_else(|| "window_not_found: The selected window is no longer available. List windows and choose a current id.".into());
    }
    if let Some(index) = options.window_index.or(legacy) {
        return windows
            .iter()
            .find(|w| w.index + 1 == index)
            .ok_or_else(|| format!("window_not_found: No window has index {index}."));
    }
    windows
        .iter()
        .find(|w| w.focused)
        .or(windows.first())
        .ok_or_else(|| "window_not_found: This app has no windows.".into())
}

/// Wayland doesn't let one app place another's windows.
fn wayland_session() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland")))
}

fn capabilities(state: &mut State) -> Outcome {
    let ready = state.platform.is_ok();
    let geometry = ready && !wayland_session();
    let data = json!({"platform":std::env::consts::OS, "provider":"dragent-native", "provider_version":env!("CARGO_PKG_VERSION"), "protocol_version":1,
        "available":ready, "supports":{
            "apps":{"list":ready,"ids":ready,"pids":ready},
            "windows":{"list":ready,"target_by_id":ready,"target_by_index":ready,"focus":ready,"move_resize":geometry,"minimize_zoom":geometry},
            "observation":{"screenshot":ready,"element_frames":ready,"annotated_screenshot":false,"ocr":false},
            "actions":{"click":ready,"type":ready,"key":ready,"hotkey":ready,"paste":ready,"scroll":ready,"drag":ready,"set_value":ready,"perform_action":ready},
            "surfaces":{"menus":ready,"dialogs":false,"dock":false,"menubar":ready}},
        "notes":["Supported operations still require OS permissions and a graphical desktop. Linux input and capture depend on the desktop's installed tools.",
            "Menus need an app with a standard menu bar (many newer apps draw their own: use their menu buttons or shortcuts). On Windows, reading the menu bar lists the menus; ask for one by path to read its items. Window geometry isn't possible on Wayland."]});
    Outcome {
        text: serde_json::to_string_pretty(&data).unwrap_or_default(),
        data,
    }
}

impl Request {
    pub fn is_app_list(&self) -> bool {
        match self {
            Self::Apps => true,
            Self::WithOptions { request, .. } => request.is_app_list(),
            _ => false,
        }
    }
    pub fn is_diagnostic(&self) -> bool {
        match self {
            Self::Permissions | Self::Capabilities => true,
            Self::WithOptions { request, .. } => request.is_diagnostic(),
            _ => false,
        }
    }
}

/// Menu items as an indented list: `Export ▸`, `Close (disabled)`.
fn render_menus(entries: &[MenuEntry], depth: usize, out: &mut String) {
    for entry in entries {
        out.push_str(&"  ".repeat(depth));
        if entry.checked {
            out.push_str("✓ ");
        }
        out.push_str(&entry.title);
        if entry.submenu {
            out.push_str(" ▸");
        }
        if let Some(shortcut) = &entry.shortcut {
            out.push_str(&format!(" ({shortcut})"));
        }
        if !entry.enabled {
            out.push_str(" (disabled)");
        }
        out.push('\n');
        render_menus(&entry.children, depth + 1, out);
    }
}

fn action_result(text: String, path: &str, state: &str, reason: &str) -> Outcome {
    Outcome {
        text,
        data: json!({"dispatched":true,"path":path,"verification":{"state":state,"reason":reason}}),
    }
}

fn keyboard_window(
    platform: &mut platform::Platform,
    app: &AppInfo,
    window: &WindowInfo,
    restore: bool,
    needs_text: bool,
) -> Result<(), String> {
    if restore {
        platform.activate(app, Some(window))?;
    }
    let apps = platform.apps()?;
    let active = apps.iter().any(|a| a.pid == app.pid && a.active);
    let windows = platform.windows(app)?;
    let focused = windows.iter().any(|w| w.id == window.id && w.focused);
    if !active || !focused {
        return Err("window_not_focused: Keyboard input needs the selected window in front. Use restore_window or bring it forward manually.".into());
    }
    if needs_text && !platform.text_receiver(app, window)? {
        return Err("window_not_focused: The selected window has no focused text receiver. Focus an editable field and inspect it first.".into());
    }
    Ok(())
}

fn in_window(window: &WindowInfo, x: f64, y: f64) -> Result<(f64, f64), String> {
    if !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= window.frame.width
        || y >= window.frame.height
    {
        return Err("invalid_argument: Coordinates must fall inside the selected window.".into());
    }
    Ok((window.frame.x + x, window.frame.y + y))
}

/// Reset grants only for the currently running, bundled Dragent application.
/// Invoked by the person's Settings confirmation, never by agent requests.
pub fn reset_permissions() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let contents = executable
            .parent()
            .and_then(|p| p.parent())
            .ok_or("The application bundle could not be located")?;
        let plist = contents.join("Info.plist");
        if !plist.is_file() {
            return Err("Reset access requires a bundled Dragent app. Grant or revoke the development binary directly in System Settings.".into());
        }
        let output = std::process::Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleIdentifier"])
            .arg(&plist)
            .output()
            .map_err(|e| e.to_string())?;
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || !(id == "com.dragent.app" || id.starts_with("com.dragent."))
        {
            return Err("The current bundle is not a recognized Dragent application.".into());
        }
        for service in ["Accessibility", "ScreenCapture"] {
            let status = std::process::Command::new("/usr/bin/tccutil")
                .args(["reset", service, &id])
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("Could not reset {service} access. Use System Settings to revoke the grant manually."));
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("This platform does not use Dragent-specific macOS privacy grants.".into())
    }
}

#[cfg(test)]
mod command_flow_tests {
    use super::*;
    /// Tests that act share the one desktop worker, and an action makes
    /// every caller's element numbers for that app stale: one at a time.
    static ACTING: Mutex<()> = Mutex::new(());
    fn request(tool: &str, arguments: Value) -> Request {
        Request::from_tool(tool, &arguments).unwrap()
    }
    fn invoke(scope: &str, tool: &str, arguments: Value) -> Result<Outcome, String> {
        run_for(scope, request(tool, arguments), None)
    }
    #[test]
    fn authorization_is_rechecked_on_the_worker_before_resolving_a_target() {
        let caller_thread = std::thread::current().id();
        let rejected = run_for_guarded(
            "guard-test",
            request("click", json!({"app":"missing.app","x":1,"y":2})),
            None,
            move || {
                assert_ne!(std::thread::current().id(), caller_thread);
                Err("Authorization was revoked while queued.".into())
            },
        )
        .unwrap_err();
        assert_eq!(rejected, "Authorization was revoked while queued.");
    }
    #[test]
    fn menus_are_read_and_chosen_by_path_and_windows_report_what_they_became() {
        let _acting = ACTING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let app = "test.app";
        let bar = invoke("menus/a", "menus", json!({"app": app})).unwrap();
        assert!(bar.text.contains("File ▸\n  New (⌘N)\n  Export ▸\n    PDF…\n  Close (disabled)"), "{}", bar.text);
        let export = invoke("menus/a", "menus", json!({"app": app, "path": "file > export"})).unwrap();
        assert_eq!(export.data["items"][0]["title"], "PDF…");
        // Case and a missing ellipsis don't matter; the answer is unverified.
        let chose = invoke("menus/a", "menu", json!({"app": app, "path": "File > Export > pdf"})).unwrap();
        assert!(chose.text.starts_with("Chose File › Export › pdf"), "{}", chose.text);
        assert_eq!(chose.data["action"]["verification"]["state"], "unverified", "{}", chose.data);
        let code = |result: Result<Outcome, String>| errors::details(&result.unwrap_err())["code"].clone();
        assert_eq!(code(invoke("menus/a", "menu", json!({"app": app, "path": "File > Close"}))), "element_not_clickable");
        assert_eq!(code(invoke("menus/a", "menu", json!({"app": app, "path": "File > Export"}))), "invalid_argument");
        assert_eq!(code(invoke("menus/a", "menu", json!({"app": app, "path": "File > Print"}))), "element_not_found");
        assert!(Request::from_tool("menu", &json!({"app": app, "path": " > "})).is_err());

        let moved = invoke("menus/a", "set_window", json!({"app": app, "x": 40, "y": 60, "width": 800, "height": 500})).unwrap();
        assert_eq!(moved.data["action"]["verification"]["state"], "verified", "{}", moved.data);
        assert_eq!(moved.data["action"]["frame"]["width"], 800.0);
        // The app keeps windows at least 300 wide: said, not hidden.
        let narrow = invoke("menus/a", "window", json!({"app": app, "width": 120, "height": 500})).unwrap();
        assert_eq!(narrow.data["action"]["verification"]["state"], "partial");
        assert!(narrow.text.contains("didn't take all of it"), "{}", narrow.text);
        for bad in [json!({"app": app}), json!({"app": app, "x": 1}), json!({"app": app, "width": 10, "height": 10})] {
            assert!(Request::from_tool("set_window", &bad).is_err(), "{bad}");
        }
        // Both change the desktop, so they go through the person's approval rules.
        assert!(request("menu", json!({"app": app, "path": "File > New"})).changes_things());
        assert!(request("window", json!({"app": app, "state": "minimize"})).changes_things());
        assert!(!request("menus", json!({"app": app})).changes_things());
        assert_eq!(
            describe(&request("menu", json!({"app": app, "path": "File > Export > PDF…"})), None),
            "Choose File › Export › PDF… from the menus"
        );
    }
    #[test]
    fn actions_return_current_state_keep_window_identity_and_reject_stale_handles() {
        let _acting = ACTING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let app = "test.app";
        let scope = "flow/first";
        let shot = invoke(scope, "snapshot", json!({"app":app,"screenshot":false})).unwrap();
        assert_eq!(shot.data["window"]["index"], 1);
        assert_eq!(shot.data["elements"][1]["actions"], json!(["press"]));
        assert!(shot.text.contains("actions:"));
        invoke(
            "flow/second",
            "snapshot",
            json!({"app":app,"screenshot":false}),
        )
        .unwrap();
        let filled = invoke(
            scope,
            "set_value",
            json!({"app":app,"element":0,"value":"verified text","screenshot":false}),
        )
        .unwrap();
        assert_eq!(filled.data["action"]["verification"]["state"], "verified");
        assert!(filled.data["snapshot"]["id"].is_string());
        assert_eq!(filled.data["screenshot_status"]["state"], "skipped");
        assert!(filled.text.contains("verified text"));
        assert!(invoke(
            "flow/second",
            "click",
            json!({"app":app,"element":1,"screenshot":false})
        )
        .is_err());
        // Fresh state returned by an action is immediately usable.
        let action = invoke(
            scope,
            "perform_action",
            json!({"app":app,"element":1,"name":"press","screenshot":false}),
        )
        .unwrap();
        assert_eq!(action.data["action"]["verification"]["state"], "unverified");
        let approved =
            preview_for(scope, request("click", json!({"app":app,"element":1}))).unwrap();
        invoke(scope, "snapshot", json!({"app":app,"screenshot":false})).unwrap();
        assert!(run_for(
            scope,
            request("click", json!({"app":app,"element":1})),
            Some(approved)
        )
        .unwrap_err()
        .contains("snapshot changed"));
        // A restored observation retains its newly numbered elements.
        let restored = invoke(
            scope,
            "snapshot",
            json!({"app":app,"window_id":"window-1","restore_window":true,"screenshot":false}),
        )
        .unwrap();
        assert_eq!(restored.data["window"]["id"], "window-1");
        assert!(preview_for(
            scope,
            request(
                "click",
                json!({"app":app,"element":1,"window_id":"window-1"})
            )
        )
        .is_ok());
        assert!(preview_for(
            scope,
            request(
                "click",
                json!({"app":app,"element":1,"window_id":"window-0"})
            )
        )
        .is_err());
        let moved = invoke(
            scope,
            "drag",
            json!({"app":app,"from_element":0,"to_element":1,"screenshot":false}),
        )
        .unwrap();
        assert_eq!(moved.data["window"]["id"], "window-1");
        assert_eq!(
            moved.data["action"]["verification"]["reason"],
            "synthetic_input"
        );
        invoke(
            scope,
            "click",
            json!({"app":app,"element":0,"screenshot":false}),
        )
        .unwrap();
        let typed = invoke(
            scope,
            "type",
            json!({"app":app,"text":"more","screenshot":false}),
        )
        .unwrap();
        assert_eq!(typed.data["action"]["verification"]["state"], "unverified");
        invoke(
            scope,
            "key",
            json!({"app":app,"key":"Escape","screenshot":false}),
        )
        .unwrap();
        assert!(invoke(
            scope,
            "type",
            json!({"app":app,"text":"must not type","screenshot":false})
        )
        .unwrap_err()
        .contains("text receiver"));
    }
    #[test]
    fn validates_complete_targets_chords_and_diagnostic_options() {
        for (tool, args) in [
            ("drag", json!({"app":"a","from_x":1,"to_x":3})),
            (
                "drag",
                json!({"app":"a","from_element":1,"to_element":2,"from_x":1}),
            ),
            ("scroll", json!({"app":"a","element":1,"x":2,"y":3})),
            (
                "snapshot",
                json!({"app":"a","window_id":"x","window_index":1}),
            ),
            ("snapshot", json!({"app":"a","window":2,"window_id":"x"})),
            ("hotkey", json!({"app":"a","key":"Return"})),
            ("press_key", json!({"app":"a","key":"Ctrl+A"})),
            ("paste", json!({"app":"a","text":"bad\u{0}text"})),
        ] {
            assert!(Request::from_tool(tool, &args).is_err(), "{tool} {args}");
        }
        assert!(request("capabilities", json!({})).is_diagnostic());
        assert!(request("snapshot", json!({"app":"a","restore_window":true})).changes_things());
        assert!(request("list-apps", json!({"screenshot":false})).is_app_list());
        assert!(Request::from_tool(
            "snapshot",
            &json!({"app":"a","restore_window":null,"screenshot":null})
        )
        .is_ok());
    }
}

fn object_keys(value: &Value) -> impl Iterator<Item = &str> {
    value.as_object().into_iter().flat_map(|object| {
        object
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, _)| key.as_str())
    })
}
