//! Linux: AT-SPI, the desktop accessibility bus, reads windows and runs
//! their actions or fills their text. Pointer and keyboard input goes
//! through xdotool on X11 or ydotool on Wayland, else AT-SPI's own event
//! generator (X11 only). Window pictures come from the X server, or grim on
//! Wayland.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::keys::{Combo, Key, Named};
use crate::tree::{AppInfo, RawNode, Rect, WindowInfo};
use crate::{Access, Button, Direction, PermissionKind, Permissions};

const MAX_DEPTH: usize = 40;
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const ACTION: &str = "org.a11y.atspi.Action";
const TEXT: &str = "org.a11y.atspi.Text";
const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";

// AT-SPI state bits.
const STATE_ACTIVE: u32 = 1;
const STATE_CHECKED: u32 = 4;
const STATE_EDITABLE: u32 = 7;
const STATE_ENABLED: u32 = 8;
const STATE_FOCUSED: u32 = 12;
const STATE_SELECTED: u32 = 23;
const STATE_SENSITIVE: u32 = 24;

const MENU_ROLES: [&str; 4] = ["menu", "menu item", "check menu item", "radio menu item"];

/// An accessible object: the app's bus name and the object's path.
#[derive(Clone)]
pub struct Handle {
    bus: String,
    path: OwnedObjectPath,
}

pub struct Platform {
    bus: Connection,
}

/// The X11 client window showing `window` of `app` (its pid and title in
/// the window manager's client list), with the connection and root window.
fn x11_client(
    app: &AppInfo,
    window: &WindowInfo,
) -> Result<(x11rb::rust_connection::RustConnection, u32, u32), String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    let (connection, screen) =
        x11rb::connect(None).map_err(|e| format!("the X server can't be reached: {e}"))?;
    let root = connection.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, String> {
        connection
            .intern_atom(false, name)
            .map_err(|e| e.to_string())?
            .reply()
            .map(|r| r.atom)
            .map_err(|e| e.to_string())
    };
    let list = atom(b"_NET_CLIENT_LIST")?;
    let pid_atom = atom(b"_NET_WM_PID")?;
    let title_atom = atom(b"_NET_WM_NAME")?;
    let property = connection
        .get_property(
            false,
            root,
            list,
            x11rb::protocol::xproto::AtomEnum::WINDOW,
            0,
            4096,
        )
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    for id in property.value32().into_iter().flatten() {
        let pid = connection
            .get_property(
                false,
                id,
                pid_atom,
                x11rb::protocol::xproto::AtomEnum::CARDINAL,
                0,
                1,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if pid.value32().and_then(|mut values| values.next()) != Some(app.pid) {
            continue;
        }
        let name = connection
            .get_property(
                false,
                id,
                title_atom,
                x11rb::protocol::xproto::AtomEnum::ANY,
                0,
                4096,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if String::from_utf8_lossy(&name.value) == window.title {
            candidates.push(id);
        }
    }
    if candidates.len() != 1 {
        return Err(format!(
            "{}'s window “{}” couldn't be told apart from its other X11 windows.",
            app.name, window.title
        ));
    }
    Ok((connection, root, candidates[0]))
}

fn has(states: &[u32], bit: u32) -> bool {
    states
        .get((bit / 32) as usize)
        .is_some_and(|word| word & (1 << (bit % 32)) != 0)
}

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"))
}

fn find_tool(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn run_tool(tool: &Path, args: &[String]) -> Result<String, String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new(tool)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} could not start: {e}", tool.display()))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(1_048_576).read_to_end(&mut bytes);
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(65_536).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    match status {
        Some(status) if status.success() => Ok(String::from_utf8_lossy(&stdout).to_string()),
        Some(_) => Err(format!(
            "{} failed: {}",
            tool.file_name().unwrap_or_default().to_string_lossy(),
            String::from_utf8_lossy(&stderr).trim()
        )),
        None => Err(format!(
            "action_timeout: {} exceeded its 15-second deadline. Inspect the app before retrying.",
            tool.file_name().unwrap_or_default().to_string_lossy()
        )),
    }
}

/// AT-SPI role names, in the words the other platforms use.
fn role_word(role: &str) -> String {
    match role {
        "push button" => "button",
        "check box" => "checkbox",
        "password text" => "password field",
        "entry" => "text field",
        "label" | "static" => "text",
        "panel" | "filler" | "section" => "group",
        "page tab" => "tab",
        "page tab list" => "tab list",
        "document web" | "document frame" => "web area",
        "check menu item" | "radio menu item" => "menu item",
        "spin button" => "spinner",
        "scroll pane" => "scroll area",
        "tool bar" => "toolbar",
        "status bar" => "status bar",
        "list box" => "list",
        other => other,
    }
    .to_string()
}

const WINDOW_ROLES: &[&str] = &["frame", "window", "dialog", "alert", "file chooser"];

impl Platform {
    /// The app's menu bar, searched breadth-first through its windows
    /// (the active one first).
    fn menu_bar(&self, app: &AppInfo) -> Result<Handle, String> {
        let mut windows = self.window_handles(app);
        windows.sort_by_key(|(.., active)| !*active);
        for (window, ..) in windows {
            let mut queue = std::collections::VecDeque::from([(window, 0usize)]);
            let mut seen = 0;
            while let Some((node, depth)) = queue.pop_front() {
                seen += 1;
                if self.role(&node) == "menu bar" {
                    return Ok(node);
                }
                if depth < 6 && seen < 400 {
                    queue.extend(self.children(&node).into_iter().map(|child| (child, depth + 1)));
                }
            }
        }
        Err(format!(
            "element_not_found: {} shows no menu bar to accessibility (GTK 4 and many newer apps use a menu button instead): snapshot the window and press its menu button, or use keyboard shortcuts.",
            app.name
        ))
    }

    fn menu_items(&self, handle: &Handle) -> Vec<Handle> {
        self.children(handle)
            .into_iter()
            .filter(|child| MENU_ROLES.contains(&self.role(child).as_str()))
            .collect()
    }

    fn menu_title(&self, handle: &Handle) -> String {
        self.text_property(handle, "Name")
    }

    fn menu_enabled(&self, handle: &Handle) -> bool {
        let states = self.states(handle);
        has(&states, STATE_SENSITIVE) || has(&states, STATE_ENABLED)
    }

    /// The accelerator of an item's key binding
    /// (`mnemonic;mnemonic path;accelerator`).
    fn menu_shortcut(&self, handle: &Handle) -> Option<String> {
        let binding: String = self.call(handle, ACTION, "GetKeyBinding", &(0i32,)).ok()?;
        crate::accelerator_text(binding.split(';').nth(2)?)
    }

    fn menu_entry(&self, handle: &Handle, depth: usize) -> Option<crate::MenuEntry> {
        let title = self.menu_title(handle);
        if title.trim().is_empty() {
            return None;
        }
        let submenu = self.role(handle) == "menu";
        Some(crate::MenuEntry {
            title,
            enabled: self.menu_enabled(handle),
            checked: has(&self.states(handle), STATE_CHECKED),
            shortcut: self.menu_shortcut(handle),
            submenu,
            children: if submenu && depth > 0 {
                self.menu_items(handle)
                    .iter()
                    .filter_map(|child| self.menu_entry(child, depth - 1))
                    .collect()
            } else {
                Vec::new()
            },
        })
    }

    fn menu_titles(&self, level: &[Handle]) -> String {
        level
            .iter()
            .map(|item| self.menu_title(item))
            .filter(|title| !title.trim().is_empty())
            .map(|title| format!("“{title}”"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The items at `path`. A menu whose items only appear once it's open
    /// is opened when `open` (choosing), never just to read it.
    fn menu_level(&mut self, app: &AppInfo, path: &[String], open: bool) -> Result<Vec<Handle>, String> {
        let mut level = self.menu_items(&self.menu_bar(app)?);
        let mut trail: Vec<String> = Vec::new();
        for wanted in path {
            let item = level
                .iter()
                .find(|item| crate::same_menu_title(&self.menu_title(item), wanted))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "element_not_found: {} has no menu item “{wanted}”{}; there: {}.",
                        app.name,
                        if trail.is_empty() { String::new() } else { format!(" in {}", trail.join(" › ")) },
                        self.menu_titles(&level)
                    )
                })?;
            trail.push(self.menu_title(&item));
            level = self.menu_items(&item);
            if level.is_empty() && open && self.press(&item)? {
                let start = Instant::now();
                while level.is_empty() && start.elapsed() < Duration::from_millis(1000) {
                    std::thread::sleep(Duration::from_millis(50));
                    level = self.menu_items(&item);
                }
            }
            if level.is_empty() {
                return Err(format!("invalid_argument: {} › {} opens no menu.", app.name, trail.join(" › ")));
            }
        }
        Ok(level)
    }

    pub fn menus(&mut self, app: &AppInfo, path: &[String]) -> Result<Vec<crate::MenuEntry>, String> {
        Ok(self
            .menu_level(app, path, false)?
            .iter()
            .filter_map(|item| self.menu_entry(item, 1))
            .take(60)
            .collect())
    }

    /// Choose the item at `path`; answers its full path.
    pub fn choose_menu(&mut self, app: &AppInfo, path: &[String]) -> Result<String, String> {
        let (last, parents) = path.split_last().ok_or("invalid_argument: Name a menu item.")?;
        if parents.is_empty() {
            return Err(format!(
                "invalid_argument: Name an item in a menu, like \"{last} > …\": a menu's own title only opens it (`menus APP \"{last}\"` lists its items)."
            ));
        }
        self.activate(app, None)?;
        let level = self.menu_level(app, parents, true)?;
        let item = level
            .iter()
            .find(|item| crate::same_menu_title(&self.menu_title(item), last))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "element_not_found: {} has no item “{last}” in {}; there: {}.",
                    app.name,
                    parents.join(" › "),
                    self.menu_titles(&level)
                )
            })?;
        let title = self.menu_title(&item);
        if self.role(&item) == "menu" {
            return Err(format!("invalid_argument: “{title}” opens a submenu: name one of its items."));
        }
        if !self.menu_enabled(&item) {
            return Err(format!(
                "element_not_clickable: “{title}” is disabled right now: the app needs something first (a selection, an open document)."
            ));
        }
        if !self.press(&item)? {
            let hint = self
                .menu_shortcut(&item)
                .map(|shortcut| format!(" Its shortcut is {shortcut}: press it with hotkey."))
                .unwrap_or_default();
            return Err(format!("action_not_supported: {} didn't take the menu choice.{hint}", app.name));
        }
        let mut full: Vec<String> = parents.to_vec();
        full.push(title);
        Ok(full.join(" › "))
    }

    /// Move, resize or change the state of `window` through the window
    /// manager (X11); answers its frame after.
    pub fn set_window(&mut self, app: &AppInfo, window: &WindowInfo, change: &crate::WindowChange) -> Result<Rect, String> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};
        if wayland() {
            return Err(
                "unsupported_capability: Wayland doesn't let one app move, resize, minimize or zoom another's windows: ask the person, or use the app's own shortcuts."
                    .into(),
            );
        }
        let handle = self.window_handle(app, window)?;
        let (connection, root, id) = x11_client(app, window)?;
        let atom = |name: &[u8]| -> Result<u32, String> {
            connection
                .intern_atom(false, name)
                .map_err(|e| e.to_string())?
                .reply()
                .map(|reply| reply.atom)
                .map_err(|e| e.to_string())
        };
        let message = |kind: u32, data: [u32; 5]| -> Result<(), String> {
            connection
                .send_event(
                    false,
                    root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    ClientMessageEvent::new(32, id, kind, data),
                )
                .map(|_| ())
                .map_err(|e| format!("action_not_supported: The window manager didn't take the change: {e}"))
        };
        // _NET_WM_STATE actions; 2 says a tool, not the app, asks.
        const REMOVE: u32 = 0;
        const TOGGLE: u32 = 2;
        const FROM_TOOL: u32 = 2;
        match change.state {
            Some(crate::WindowState::Minimize) => message(atom(b"WM_CHANGE_STATE")?, [3, 0, 0, 0, 0])?,
            Some(crate::WindowState::Restore) => {
                message(atom(b"_NET_WM_STATE")?, [REMOVE, atom(b"_NET_WM_STATE_FULLSCREEN")?, 0, FROM_TOOL, 0])?;
                message(atom(b"_NET_ACTIVE_WINDOW")?, [FROM_TOOL, 0, 0, 0, 0])?;
            }
            Some(crate::WindowState::Zoom) => message(
                atom(b"_NET_WM_STATE")?,
                [TOGGLE, atom(b"_NET_WM_STATE_MAXIMIZED_VERT")?, atom(b"_NET_WM_STATE_MAXIMIZED_HORZ")?, FROM_TOOL, 0],
            )?,
            None => {}
        }
        if change.position.is_some() || change.size.is_some() {
            let (x, y) = change.position.unwrap_or_default();
            let (width, height) = change.size.unwrap_or_default();
            // North-west gravity; bits 8–11 say which values are given.
            let mut flags = 1 | (FROM_TOOL << 12);
            if change.position.is_some() {
                flags |= (1 << 8) | (1 << 9);
            }
            if change.size.is_some() {
                flags |= (1 << 10) | (1 << 11);
            }
            message(
                atom(b"_NET_MOVERESIZE_WINDOW")?,
                [flags, x.round().max(0.0) as u32, y.round().max(0.0) as u32, width.round().max(1.0) as u32, height.round().max(1.0) as u32],
            )?;
        }
        connection.flush().map_err(|e| e.to_string())?;
        let wanted = |frame: &Rect| {
            let close = |a: f64, b: f64| (a - b).abs() <= 2.0;
            change.position.is_none_or(|(x, y)| close(frame.x, x) && close(frame.y, y))
                && change.size.is_none_or(|(w, h)| close(frame.width, w) && close(frame.height, h))
        };
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            if self.extents(&handle).is_some_and(|frame| wanted(&frame)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.extents(&handle)
            .ok_or_else(|| "accessibility_error: The window's frame couldn't be read back.".into())
    }

    pub fn new() -> Result<Self, String> {
        let unavailable = |e: zbus::Error| {
            format!("The desktop accessibility bus (AT-SPI) isn't reachable: {e}. Install and start at-spi2-core.")
        };
        let session = Connection::session().map_err(unavailable)?;
        let reply = session
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.a11y.Bus"),
                "GetAddress",
                &(),
            )
            .map_err(unavailable)?;
        let address: String = reply.body().deserialize().map_err(unavailable)?;
        let bus = zbus::blocking::connection::Builder::address(address.as_str())
            .and_then(|builder| builder.build())
            .map_err(unavailable)?;
        Ok(Self { bus })
    }

    fn call<B, R>(&self, handle: &Handle, iface: &str, method: &str, body: &B) -> Result<R, String>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'de> zbus::zvariant::DynamicDeserialize<'de>,
    {
        let reply = self
            .bus
            .call_method(
                Some(handle.bus.as_str()),
                handle.path.as_str(),
                Some(iface),
                method,
                body,
            )
            .map_err(|e| e.to_string())?;
        reply.body().deserialize::<R>().map_err(|e| e.to_string())
    }

    fn property(&self, handle: &Handle, iface: &str, name: &str) -> Option<OwnedValue> {
        self.call(
            handle,
            "org.freedesktop.DBus.Properties",
            "Get",
            &(iface, name),
        )
        .ok()
    }

    fn text_property(&self, handle: &Handle, name: &str) -> String {
        self.property(handle, ACCESSIBLE, name)
            .and_then(|value| String::try_from(value).ok())
            .unwrap_or_default()
    }

    fn children(&self, handle: &Handle) -> Vec<Handle> {
        self.call::<_, Vec<(String, OwnedObjectPath)>>(handle, ACCESSIBLE, "GetChildren", &())
            .unwrap_or_default()
            .into_iter()
            .map(|(bus, path)| Handle { bus, path })
            .collect()
    }

    fn role(&self, handle: &Handle) -> String {
        self.call::<_, String>(handle, ACCESSIBLE, "GetRoleName", &())
            .unwrap_or_default()
    }

    fn states(&self, handle: &Handle) -> Vec<u32> {
        self.call::<_, Vec<u32>>(handle, ACCESSIBLE, "GetState", &())
            .unwrap_or_default()
    }

    fn extents(&self, handle: &Handle) -> Option<Rect> {
        let (x, y, width, height): (i32, i32, i32, i32) =
            self.call(handle, COMPONENT, "GetExtents", &(0u32,)).ok()?;
        let frame = Rect {
            x: x as f64,
            y: y as f64,
            width: width as f64,
            height: height as f64,
        };
        (!frame.is_empty()).then_some(frame)
    }

    fn pid(&self, bus: &str) -> Option<u32> {
        let reply = self
            .bus
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetConnectionUnixProcessID",
                &(bus,),
            )
            .ok()?;
        reply.body().deserialize::<u32>().ok()
    }

    fn app_roots(&self) -> Vec<Handle> {
        let desktop = Handle {
            bus: REGISTRY.into(),
            path: OwnedObjectPath::try_from(ROOT).expect("a valid object path"),
        };
        self.children(&desktop)
    }

    fn window_handles(&self, app: &AppInfo) -> Vec<(Handle, String, Rect, bool)> {
        let Some(root) = self
            .app_roots()
            .into_iter()
            .find(|root| self.pid(&root.bus) == Some(app.pid))
        else {
            return Vec::new();
        };
        self.children(&root)
            .into_iter()
            .filter(|child| WINDOW_ROLES.contains(&self.role(child).as_str()))
            .map(|child| {
                let title = self.text_property(&child, "Name");
                let frame = self.extents(&child).unwrap_or_default();
                let active = has(&self.states(&child), STATE_ACTIVE);
                (child, title, frame, active)
            })
            .collect()
    }

    fn window_handle(&self, app: &AppInfo, window: &WindowInfo) -> Result<Handle, String> {
        let windows = self.window_handles(app);
        windows
            .iter()
            .find(|(_, title, frame, _)| {
                *title == window.title && (frame.x - window.frame.x).abs() < 3.0
            })
            .or_else(|| windows.get(window.index))
            .map(|(handle, ..)| handle.clone())
            .ok_or_else(|| {
                format!(
                    "That window of {} has closed. Take a new snapshot.",
                    app.name
                )
            })
    }

    pub fn permissions(&mut self) -> Permissions {
        let mut notes = vec![
            "Apps only show their elements while desktop accessibility is on (GNOME: gsettings set org.gnome.desktop.interface toolkit-accessibility true; Qt apps also need QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1).".to_string(),
        ];
        let (input, screen) = if wayland() {
            let input = if find_tool("ydotool").is_some() {
                Access::Granted
            } else {
                Access::Missing
            };
            if input == Access::Missing {
                notes.push("For clicks and typing on Wayland, install ydotool and start ydotoold. Pressing buttons and filling fields work without it.".into());
            }
            let screen = if find_tool("grim").is_some() {
                Access::Granted
            } else {
                Access::Missing
            };
            if screen == Access::Missing {
                notes.push("For screenshots on Wayland, install grim (wlroots desktops).".into());
            }
            (input, screen)
        } else {
            if find_tool("xdotool").is_none() {
                notes.push(
                    "Installing xdotool makes clicks and typing more reliable on X11.".into(),
                );
            }
            (Access::Granted, Access::Granted)
        };
        Permissions {
            platform: "Linux".into(),
            accessibility: Access::Granted,
            screen_recording: screen,
            input,
            notes,
        }
    }

    pub fn apps(&mut self) -> Result<Vec<AppInfo>, String> {
        let mut apps: Vec<AppInfo> = Vec::new();
        for root in self.app_roots() {
            let Some(pid) = self.pid(&root.bus) else {
                continue;
            };
            if pid == crate::owner_pid() || apps.iter().any(|app| app.pid == pid) {
                continue;
            }
            let windows: Vec<Handle> = self
                .children(&root)
                .into_iter()
                .filter(|child| WINDOW_ROLES.contains(&self.role(child).as_str()))
                .collect();
            if windows.is_empty() {
                continue;
            }
            let active = windows
                .iter()
                .any(|window| has(&self.states(window), STATE_ACTIVE));
            let id = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .map(|comm| comm.trim().to_string())
                .unwrap_or_default();
            let mut name = self.text_property(&root, "Name");
            if name.is_empty() {
                name = id.clone();
            }
            apps.push(AppInfo {
                name,
                id,
                pid,
                active,
            });
        }
        Ok(apps)
    }

    pub fn windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>, String> {
        Ok(self
            .window_handles(app)
            .into_iter()
            .enumerate()
            .map(|(index, (handle, title, frame, focused))| WindowInfo {
                id: format!("{}{}", handle.bus, handle.path),
                index,
                title,
                frame,
                focused,
                native: 0,
            })
            .collect())
    }

    pub fn tree(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        limit: usize,
    ) -> Result<Vec<RawNode<Handle>>, String> {
        let root = self.window_handle(app, window)?;
        let mut reach = crate::tree::Reach::new(window.frame);
        let mut nodes = Vec::new();
        let mut stack = vec![(root, 0usize)];
        while let Some((handle, depth)) = stack.pop() {
            if nodes.len() >= limit {
                break;
            }
            let atspi_role = self.role(&handle);
            let states = self.states(&handle);
            let frame = self.extents(&handle);
            let children =
                if depth < MAX_DEPTH && reach.descend(depth, &role_word(&atspi_role), frame) {
                    self.children(&handle)
                } else {
                    Vec::new()
                };
            let secure = atspi_role == "password text";
            let editable = has(&states, STATE_EDITABLE);
            let value = if editable && !secure {
                self.call::<_, String>(&handle, TEXT, "GetText", &(0i32, 500i32))
                    .ok()
            } else {
                None
            };
            let toggles = matches!(
                atspi_role.as_str(),
                "check box"
                    | "radio button"
                    | "toggle button"
                    | "check menu item"
                    | "radio menu item"
            );
            nodes.push(RawNode {
                depth,
                // AT-SPI calls editors and static text both "text".
                role: if editable && atspi_role == "text" {
                    "text area".into()
                } else {
                    role_word(&atspi_role)
                },
                name: self.text_property(&handle, "Name"),
                value,
                description: self.text_property(&handle, "Description"),
                placeholder: String::new(),
                enabled: has(&states, STATE_ENABLED) || has(&states, STATE_SENSITIVE),
                focused: has(&states, STATE_FOCUSED),
                selected: has(&states, STATE_SELECTED),
                checked: toggles.then(|| has(&states, STATE_CHECKED)),
                secure,
                frame,
                actions: self.element_actions(&handle),
                has_children: !children.is_empty(),
                handle,
            });
            for child in children.into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
        Ok(nodes)
    }

    pub fn screenshot(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        path: &Path,
    ) -> Result<(), String> {
        let f = window.frame;
        if f.is_empty() {
            return Err("the window reports no position".into());
        }
        if wayland() {
            if !has(
                &self.states(&self.window_handle(app, window)?),
                STATE_ACTIVE,
            ) {
                return Err(
                    "Bring the target window forward before taking a screenshot on Wayland.".into(),
                );
            }
            let grim = find_tool("grim").ok_or("install grim for screenshots on Wayland")?;
            run_tool(
                &grim,
                &[
                    "-g".into(),
                    format!("{:.0},{:.0} {:.0}x{:.0}", f.x, f.y, f.width, f.height),
                    path.to_string_lossy().to_string(),
                ],
            )?;
            return Ok(());
        }
        use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
        let (connection, root, drawable) = x11_client(app, window).map_err(|_| {
            "The target X11 window could not be identified uniquely for a screenshot.".to_string()
        })?;
        let geometry = connection
            .get_geometry(drawable)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let (width, height) = (geometry.width as u32, geometry.height as u32);
        if width == 0 || height == 0 || width as u64 * height as u64 > 64_000_000 {
            return Err("The window has an unsupported size.".into());
        }
        let image = connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                drawable,
                0,
                0,
                geometry.width,
                geometry.height,
                !0,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| format!("the window couldn't be captured: {e}"))?;
        let pixels = image.data.len() / (width * height).max(1) as usize;
        if pixels != 4 {
            return Err("this display's pixel format isn't supported for screenshots".into());
        }
        let mut rgba = image.data;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        // Keep pixel coordinates aligned with the AT-SPI window frame even
        // when the window manager adds borders around the X11 client.
        let origin = connection
            .translate_coordinates(drawable, root, 0, 0)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let offset_x = (origin.dst_x as f64 - f.x).round() as i64;
        let offset_y = (origin.dst_y as f64 - f.y).round() as i64;
        let (out_width, out_height) = (f.width.round() as u32, f.height.round() as u32);
        if out_width == 0 || out_height == 0 || out_width as u64 * out_height as u64 > 64_000_000 {
            return Err("The window has an unsupported size.".into());
        }
        let client = image::RgbaImage::from_raw(width, height, rgba)
            .ok_or("Unsupported screenshot layout")?;
        let mut window =
            image::RgbaImage::from_pixel(out_width, out_height, image::Rgba([0, 0, 0, 255]));
        image::imageops::overlay(&mut window, &client, offset_x, offset_y);
        window
            .save(path)
            .map_err(|e| format!("the screenshot couldn't be saved: {e}"))
    }

    pub fn activate(&mut self, app: &AppInfo, window: Option<&WindowInfo>) -> Result<(), String> {
        let windows = self.window_handles(app);
        let target = window
            .and_then(|w| {
                windows
                    .iter()
                    .find(|(_, title, ..)| *title == w.title)
                    .or_else(|| windows.get(w.index))
            })
            .or(windows.first())
            .cloned()
            .ok_or_else(|| format!("{} has no window to bring forward.", app.name))?;
        if !wayland() {
            if let Some(xdotool) = find_tool("xdotool") {
                let ids = run_tool(
                    &xdotool,
                    &["search".into(), "--pid".into(), app.pid.to_string()],
                )
                .unwrap_or_default();
                let ids: Vec<&str> = ids
                    .lines()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .collect();
                let chosen = ids
                    .iter()
                    .find(|id| {
                        run_tool(&xdotool, &["getwindowname".into(), id.to_string()])
                            .is_ok_and(|name| name.trim() == target.1)
                    })
                    .or(ids.first());
                if let Some(id) = chosen {
                    let _ = run_tool(&xdotool, &["windowmap".into(), id.to_string()]);
                    let _ = run_tool(
                        &xdotool,
                        &["windowactivate".into(), "--sync".into(), id.to_string()],
                    );
                }
            }
        }
        if !has(&self.states(&target.0), STATE_ACTIVE) {
            let _: Result<bool, String> = self.call(&target.0, COMPONENT, "GrabFocus", &());
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            if has(&self.states(&target.0), STATE_ACTIVE) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(if wayland() {
            format!("{} isn't in front, and Wayland doesn't let one app bring another forward. Click its window first.", app.name)
        } else {
            format!(
                "{} couldn't be brought to the front, so nothing was typed or clicked.",
                app.name
            )
        })
    }

    pub fn press(&mut self, handle: &Handle) -> Result<bool, String> {
        let count = self
            .property(handle, ACTION, "NActions")
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0);
        for index in 0..count {
            let name: String = self
                .call(handle, ACTION, "GetName", &(index,))
                .unwrap_or_default();
            if matches!(
                name.as_str(),
                "click" | "press" | "activate" | "jump" | "toggle" | "open"
            ) {
                return Ok(self
                    .call::<_, bool>(handle, ACTION, "DoAction", &(index,))
                    .unwrap_or(false));
            }
        }
        Ok(false)
    }

    pub fn set_value(&mut self, handle: &Handle, value: &str) -> Result<bool, String> {
        Ok(self
            .call::<_, bool>(handle, EDITABLE_TEXT, "SetTextContents", &(value,))
            .unwrap_or(false))
    }

    pub fn read_value(&mut self, handle: &Handle) -> Option<String> {
        if self.role(handle) == "password text" {
            return None;
        }
        self.call(handle, TEXT, "GetText", &(0i32, -1i32)).ok()
    }

    pub fn focus_element(&mut self, handle: &Handle) -> Result<(), String> {
        if self.call::<_, bool>(handle, COMPONENT, "GrabFocus", &())? {
            Ok(())
        } else {
            Err("The target field could not take focus; nothing was typed.".into())
        }
    }

    pub fn element_frame(&mut self, handle: &Handle) -> Option<Rect> {
        self.extents(handle)
    }

    fn generator(&self) -> Handle {
        Handle {
            bus: REGISTRY.into(),
            path: OwnedObjectPath::try_from("/org/a11y/atspi/registry/deviceeventcontroller")
                .expect("a valid object path"),
        }
    }

    pub fn click_at(&mut self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
        let (x, y) = (x.round() as i32, y.round() as i32);
        if wayland() {
            let ydotool = find_tool("ydotool").ok_or(WAYLAND_INPUT)?;
            run_tool(
                &ydotool,
                &[
                    "mousemove".into(),
                    "--absolute".into(),
                    "-x".into(),
                    x.to_string(),
                    "-y".into(),
                    y.to_string(),
                ],
            )?;
            let code = match button {
                Button::Left => "0xC0",
                Button::Right => "0xC1",
                Button::Middle => "0xC2",
            };
            run_tool(
                &ydotool,
                &[
                    "click".into(),
                    "--repeat".into(),
                    count.to_string(),
                    code.into(),
                ],
            )?;
            return Ok(());
        }
        let number = match button {
            Button::Left => 1,
            Button::Middle => 2,
            Button::Right => 3,
        };
        if let Some(xdotool) = find_tool("xdotool") {
            run_tool(
                &xdotool,
                &[
                    "mousemove".into(),
                    "--sync".into(),
                    x.to_string(),
                    y.to_string(),
                    "click".into(),
                    "--repeat".into(),
                    count.to_string(),
                    number.to_string(),
                ],
            )?;
            return Ok(());
        }
        let event = if count >= 2 {
            format!("b{number}d")
        } else {
            format!("b{number}c")
        };
        self.call::<_, ()>(
            &self.generator(),
            "org.a11y.atspi.DeviceEventController",
            "GenerateMouseEvent",
            &(x, y, event.as_str()),
        )
    }

    pub fn scroll_at(
        &mut self,
        x: f64,
        y: f64,
        direction: Direction,
        amount: u32,
    ) -> Result<(), String> {
        if wayland() {
            let ydotool = find_tool("ydotool").ok_or(WAYLAND_INPUT)?;
            run_tool(
                &ydotool,
                &[
                    "mousemove".into(),
                    "--absolute".into(),
                    "-x".into(),
                    (x as i32).to_string(),
                    "-y".into(),
                    (y as i32).to_string(),
                ],
            )?;
            let (dx, dy) = match direction {
                Direction::Down => (0, -(amount as i32)),
                Direction::Up => (0, amount as i32),
                Direction::Left => (-(amount as i32), 0),
                Direction::Right => (amount as i32, 0),
            };
            run_tool(
                &ydotool,
                &[
                    "mousemove".into(),
                    "--wheel".into(),
                    "-x".into(),
                    dx.to_string(),
                    "-y".into(),
                    dy.to_string(),
                ],
            )?;
            return Ok(());
        }
        let xdotool = find_tool("xdotool").ok_or("Scrolling needs xdotool on X11; install it.")?;
        let button = match direction {
            Direction::Up => "4",
            Direction::Down => "5",
            Direction::Left => "6",
            Direction::Right => "7",
        };
        run_tool(
            &xdotool,
            &[
                "mousemove".into(),
                "--sync".into(),
                (x as i32).to_string(),
                (y as i32).to_string(),
                "click".into(),
                "--repeat".into(),
                amount.to_string(),
                button.into(),
            ],
        )
        .map(|_| ())
    }

    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        if wayland() {
            let ydotool = find_tool("ydotool").ok_or(WAYLAND_INPUT)?;
            return run_tool(&ydotool, &["type".into(), "--".into(), text.into()]).map(|_| ());
        }
        if let Some(xdotool) = find_tool("xdotool") {
            return run_tool(
                &xdotool,
                &[
                    "type".into(),
                    "--delay".into(),
                    "8".into(),
                    "--".into(),
                    text.into(),
                ],
            )
            .map(|_| ());
        }
        // 4: a string to type.
        self.call::<_, ()>(
            &self.generator(),
            "org.a11y.atspi.DeviceEventController",
            "GenerateKeyboardEvent",
            &(0i32, text, 4u32),
        )
    }

    pub fn press_keys(&mut self, combo: &Combo) -> Result<(), String> {
        if wayland() {
            let ydotool = find_tool("ydotool").ok_or(WAYLAND_INPUT)?;
            let key = evdev_code(combo.key).ok_or_else(|| {
                format!(
                    "{:?} can't be pressed with ydotool; type it as text.",
                    combo.key
                )
            })?;
            let mut modifiers = Vec::new();
            if combo.ctrl {
                modifiers.push(29);
            }
            if combo.alt {
                modifiers.push(56);
            }
            if combo.shift {
                modifiers.push(42);
            }
            if combo.meta {
                modifiers.push(125);
            }
            let mut args = vec!["key".to_string()];
            args.extend(modifiers.iter().map(|m| format!("{m}:1")));
            args.push(format!("{key}:1"));
            args.push(format!("{key}:0"));
            args.extend(modifiers.iter().rev().map(|m| format!("{m}:0")));
            return run_tool(&ydotool, &args).map(|_| ());
        }
        let xdotool =
            find_tool("xdotool").ok_or("Pressing keys needs xdotool on X11; install it.")?;
        let mut parts = Vec::new();
        if combo.ctrl {
            parts.push("ctrl".to_string());
        }
        if combo.alt {
            parts.push("alt".into());
        }
        if combo.shift {
            parts.push("shift".into());
        }
        if combo.meta {
            parts.push("super".into());
        }
        parts.push(keysym(combo.key));
        run_tool(
            &xdotool,
            &["key".into(), "--clearmodifiers".into(), parts.join("+")],
        )
        .map(|_| ())
    }
}

const WAYLAND_INPUT: &str =
    "Clicking and typing on Wayland needs ydotool (with ydotoold running); install it, or use element actions.";

fn keysym(key: Key) -> String {
    match key {
        Key::Named(named) => match named {
            Named::Return => "Return".into(),
            Named::Tab => "Tab".into(),
            Named::Space => "space".into(),
            Named::Backspace => "BackSpace".into(),
            Named::Delete => "Delete".into(),
            Named::Escape => "Escape".into(),
            Named::Left => "Left".into(),
            Named::Right => "Right".into(),
            Named::Up => "Up".into(),
            Named::Down => "Down".into(),
            Named::Home => "Home".into(),
            Named::End => "End".into(),
            Named::PageUp => "Prior".into(),
            Named::PageDown => "Next".into(),
            Named::F(n) => format!("F{n}"),
        },
        Key::Char(c) => match c {
            '+' => "plus".into(),
            '-' => "minus".into(),
            '=' => "equal".into(),
            ',' => "comma".into(),
            '.' => "period".into(),
            '/' => "slash".into(),
            '\\' => "backslash".into(),
            ';' => "semicolon".into(),
            '\'' => "apostrophe".into(),
            '[' => "bracketleft".into(),
            ']' => "bracketright".into(),
            '`' => "grave".into(),
            ' ' => "space".into(),
            c => c.to_string(),
        },
    }
}

/// Linux input event codes, for ydotool.
fn evdev_code(key: Key) -> Option<u16> {
    Some(match key {
        Key::Named(named) => match named {
            Named::Return => 28,
            Named::Tab => 15,
            Named::Space => 57,
            Named::Backspace => 14,
            Named::Delete => 111,
            Named::Escape => 1,
            Named::Left => 105,
            Named::Right => 106,
            Named::Up => 103,
            Named::Down => 108,
            Named::Home => 102,
            Named::End => 107,
            Named::PageUp => 104,
            Named::PageDown => 109,
            Named::F(n @ 1..=10) => 58 + n as u16,
            Named::F(11) => 87,
            Named::F(12) => 88,
            Named::F(_) => return None,
        },
        Key::Char(c) => {
            let row = |keys: &str, first: u16| keys.find(c).map(|i| first + i as u16);
            row("1234567890", 2)
                .or_else(|| row("qwertyuiop", 16))
                .or_else(|| row("asdfghjkl", 30))
                .or_else(|| row("zxcvbnm", 44))
                .or(match c {
                    '-' => Some(12),
                    '=' => Some(13),
                    '[' => Some(26),
                    ']' => Some(27),
                    ';' => Some(39),
                    '\'' => Some(40),
                    '`' => Some(41),
                    '\\' => Some(43),
                    ',' => Some(51),
                    '.' => Some(52),
                    '/' => Some(53),
                    ' ' => Some(57),
                    _ => None,
                })?
        }
    })
}

pub fn request_permission(_kind: PermissionKind) -> bool {
    true
}

pub fn open_permission_settings(_kind: PermissionKind) -> Result<(), String> {
    Err("Linux desktops don't ask for a permission for this; turn on desktop accessibility instead.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_x_names_and_input_codes() {
        assert_eq!(keysym(Key::Named(Named::PageUp)), "Prior");
        assert_eq!(keysym(Key::Char('+')), "plus");
        assert_eq!(evdev_code(Key::Char('a')), Some(30));
        assert_eq!(evdev_code(Key::Char('m')), Some(50));
        assert_eq!(evdev_code(Key::Char('1')), Some(2));
        assert_eq!(evdev_code(Key::Named(Named::F(12))), Some(88));
        assert!(has(&[1 << STATE_FOCUSED, 0], STATE_FOCUSED));
        assert!(!has(&[0, 0], STATE_FOCUSED));
    }
}

impl Platform {
    fn element_actions(&self, element: &Handle) -> Vec<String> {
        let count = self
            .property(element, ACTION, "NActions")
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(0)
            .clamp(0, 32);
        (0..count)
            .filter_map(|i| self.call(element, ACTION, "GetName", &(i,)).ok())
            .collect()
    }
    pub fn perform_action(&mut self, element: &Handle, action: &str) -> Result<(), String> {
        let actions = self.element_actions(element);
        let index = actions
            .iter()
            .position(|name| name == action)
            .ok_or("action_not_supported: The element no longer advertises this action.")?
            as i32;
        if self.call::<_, bool>(element, ACTION, "DoAction", &(index,))? {
            Ok(())
        } else {
            Err("accessibility_error: The element refused the action.".into())
        }
    }
    pub fn text_receiver(&mut self, app: &AppInfo, window: &WindowInfo) -> Result<bool, String> {
        Ok(self.tree(app, window, 2000)?.iter().any(|node| {
            node.focused
                && (has(&self.states(&node.handle), STATE_EDITABLE)
                    || matches!(
                        node.role.as_str(),
                        "text field" | "text area" | "password field"
                    ))
        }))
    }
    pub fn modified_click(
        &mut self,
        x: f64,
        y: f64,
        button: Button,
        count: u32,
        combo: &Combo,
    ) -> Result<(), String> {
        let mut modifiers = Vec::new();
        if combo.ctrl {
            modifiers.push((29, "ctrl"));
        }
        if combo.alt {
            modifiers.push((56, "alt"));
        }
        if combo.shift {
            modifiers.push((42, "shift"));
        }
        if combo.meta {
            modifiers.push((125, "super"));
        }
        if wayland() {
            let tool = find_tool("ydotool").ok_or(WAYLAND_INPUT)?;
            let mut down = vec!["key".into()];
            down.extend(modifiers.iter().map(|(code, _)| format!("{code}:1")));
            let mut up = vec!["key".into()];
            up.extend(modifiers.iter().rev().map(|(code, _)| format!("{code}:0")));
            let result = (|| {
                if !modifiers.is_empty() {
                    run_tool(&tool, &down)?;
                }
                self.click_at(x, y, button, count)
            })();
            if !modifiers.is_empty() {
                let _ = run_tool(&tool, &up);
            }
            result
        } else {
            let tool = find_tool("xdotool").ok_or("Modifier-click needs xdotool on X11.")?;
            let mut down = vec!["keydown".into()];
            down.extend(modifiers.iter().map(|(_, name)| name.to_string()));
            let mut up = vec!["keyup".into()];
            up.extend(modifiers.iter().rev().map(|(_, name)| name.to_string()));
            let result = (|| {
                if !modifiers.is_empty() {
                    run_tool(&tool, &down)?;
                }
                self.click_at(x, y, button, count)
            })();
            if !modifiers.is_empty() {
                let _ = run_tool(&tool, &up);
            }
            result
        }
    }
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Result<(), String> {
        if wayland() {
            let tool = find_tool("ydotool").ok_or(WAYLAND_INPUT)?;
            let move_to = |x: f64, y: f64| {
                run_tool(
                    &tool,
                    &[
                        "mousemove".into(),
                        "--absolute".into(),
                        "-x".into(),
                        (x.round() as i32).to_string(),
                        "-y".into(),
                        (y.round() as i32).to_string(),
                    ],
                )
            };
            move_to(from.0, from.1)?;
            run_tool(&tool, &["click".into(), "0x40".into()])?;
            let result = (|| {
                for step in 1..=20 {
                    let t = step as f64 / 20.0;
                    move_to(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)?;
                    std::thread::sleep(Duration::from_millis(12));
                }
                Ok(())
            })();
            let released = run_tool(&tool, &["click".into(), "0x80".into()]).map(|_| ());
            result.and(released)
        } else {
            let tool = find_tool("xdotool").ok_or("Dragging needs xdotool on X11.")?;
            let mut args = vec![
                "mousemove".into(),
                (from.0.round() as i32).to_string(),
                (from.1.round() as i32).to_string(),
                "mousedown".into(),
                "1".into(),
            ];
            for step in 1..=24 {
                let t = step as f64 / 24.0;
                args.extend([
                    "mousemove".into(),
                    (from.0 + (to.0 - from.0) * t).round().to_string(),
                    (from.1 + (to.1 - from.1) * t).round().to_string(),
                    "sleep".into(),
                    "0.012".into(),
                ]);
            }
            args.extend(["mouseup".into(), "1".into()]);
            let result = run_tool(&tool, &args).map(|_| ());
            let _ = run_tool(&tool, &["mouseup".into(), "1".into()]);
            result
        }
    }
}
