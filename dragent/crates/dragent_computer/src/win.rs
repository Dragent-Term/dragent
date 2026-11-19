//! Windows: UI Automation reads windows and invokes, toggles or fills their
//! elements; SendInput moves the mouse and types; PrintWindow takes window
//! pictures. No permission to grant, but apps running as administrator
//! can't be reached from a normal Dragent.

use std::ffi::c_void;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::core::{BOOL, BSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, ExpandCollapseState_LeafNode, IUIAutomation, IUIAutomationElement,
    IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, ToggleState_On, UIA_ExpandCollapsePatternId, UIA_InvokePatternId,
    UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    IsZoomed, SetCursorPos, SetForegroundWindow, SetWindowPos, ShowWindow, GWL_EXSTYLE,
    GW_OWNER, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, WS_EX_TOOLWINDOW,
};

use crate::keys::{Combo, Key, Named};
use crate::tree::{AppInfo, RawNode, Rect, WindowInfo};
use crate::{Access, Button, Direction, PermissionKind, Permissions};

const MAX_DEPTH: usize = 40;
const WHEEL_DELTA: i32 = 120;

pub type Handle = IUIAutomationElement;

pub struct Platform {
    automation: IUIAutomation,
}

struct TopWindow {
    hwnd: HWND,
    pid: u32,
    title: String,
    frame: Rect,
}

fn rect(r: RECT) -> Rect {
    Rect {
        x: r.left as f64,
        y: r.top as f64,
        width: (r.right - r.left) as f64,
        height: (r.bottom - r.top) as f64,
    }
}

/// Visible, titled, unowned top-level windows: the ones a person switches
/// between.
fn top_windows() -> Vec<TopWindow> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = &mut *(lparam.0 as *mut Vec<HWND>);
        found.push(hwnd);
        BOOL(1)
    }
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM(&mut handles as *mut Vec<HWND> as isize),
        );
    }
    handles
        .into_iter()
        .filter_map(|hwnd| unsafe {
            if !IsWindowVisible(hwnd).as_bool() || GetWindowTextLengthW(hwnd) == 0 {
                return None;
            }
            if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.0.is_null()) {
                return None;
            }
            if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut buffer = vec![0u16; 512];
            let length = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
            let mut bounds = RECT::default();
            GetWindowRect(hwnd, &mut bounds).ok()?;
            Some(TopWindow {
                hwnd,
                pid,
                title: String::from_utf16_lossy(&buffer[..length]),
                frame: rect(bounds),
            })
        })
        .collect()
}

/// `C:\…\notepad.exe` for a process, when it can be read.
fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;
        let read = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        read.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

fn foreground_pid() -> u32 {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid
    }
}

fn hwnd(native: u64) -> HWND {
    HWND(native as usize as *mut c_void)
}

/// UI Automation control types, as words.
fn role_name(control_type: i32) -> &'static str {
    match control_type {
        50000 => "button",
        50001 => "calendar",
        50002 => "checkbox",
        50003 => "combo box",
        50004 => "text field",
        50005 => "link",
        50006 => "image",
        50007 => "list item",
        50008 => "list",
        50009 => "menu",
        50010 => "menu bar",
        50011 => "menu item",
        50012 => "progress bar",
        50013 => "radio button",
        50014 => "scroll bar",
        50015 => "slider",
        50016 => "spinner",
        50017 => "status bar",
        50018 => "tab list",
        50019 => "tab",
        50020 => "text",
        50021 => "toolbar",
        50022 => "tooltip",
        50023 => "tree",
        50024 => "tree item",
        50026 => "group",
        50027 => "thumb",
        50028 => "data grid",
        50029 => "data item",
        50030 => "document",
        50031 => "split button",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "header item",
        50036 => "table",
        50037 => "title bar",
        50038 => "separator",
        _ => "element",
    }
}

// UI Automation control types for menus.
const MENU: i32 = 50009;
const MENU_BAR: i32 = 50010;
const MENU_ITEM: i32 = 50011;

fn control_type(element: &IUIAutomationElement) -> i32 {
    unsafe { element.CurrentControlType().map(|t| t.0).unwrap_or(0) }
}

fn name_of(element: &IUIAutomationElement) -> String {
    unsafe { text(element.CurrentName()) }
}

/// The direct children of `element` in the control view.
fn children_of(walker: &IUIAutomationTreeWalker, element: &IUIAutomationElement) -> Vec<IUIAutomationElement> {
    let mut found = Vec::new();
    unsafe {
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(current) = child {
            child = walker.GetNextSiblingElement(&current).ok();
            found.push(current);
            if found.len() > 300 {
                break;
            }
        }
    }
    found
}

/// The menu items under `element`, looking through a menu container.
fn menu_items(walker: &IUIAutomationTreeWalker, element: &IUIAutomationElement) -> Vec<IUIAutomationElement> {
    children_of(walker, element)
        .into_iter()
        .flat_map(|child| {
            if control_type(&child) == MENU {
                children_of(walker, &child)
            } else {
                vec![child]
            }
        })
        .filter(|child| control_type(child) == MENU_ITEM)
        .collect()
}

fn expander(element: &IUIAutomationElement) -> Option<IUIAutomationExpandCollapsePattern> {
    unsafe {
        element
            .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId)
            .ok()
    }
}

/// It opens a submenu rather than doing something.
fn opens_submenu(element: &IUIAutomationElement) -> bool {
    expander(element).is_some_and(|pattern| unsafe {
        pattern
            .CurrentExpandCollapseState()
            .is_ok_and(|state| state != ExpandCollapseState_LeafNode)
    })
}

fn menu_shortcut(element: &IUIAutomationElement) -> Option<String> {
    Some(unsafe { text(element.CurrentAcceleratorKey()) }).filter(|key| !key.trim().is_empty())
}

fn menu_enabled(element: &IUIAutomationElement) -> bool {
    unsafe { flag(element.CurrentIsEnabled()).unwrap_or(true) }
}

/// One menu item; its submenu's items are read by asking for its path
/// (opening a submenu shows it on screen).
fn menu_entry(element: &IUIAutomationElement) -> Option<crate::MenuEntry> {
    let title = name_of(element);
    if title.trim().is_empty() {
        return None;
    }
    let checked = unsafe {
        element
            .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            .ok()
            .and_then(|toggle| toggle.CurrentToggleState().ok())
            .is_some_and(|state| state == ToggleState_On)
    };
    Some(crate::MenuEntry {
        title,
        enabled: menu_enabled(element),
        checked,
        shortcut: menu_shortcut(element),
        submenu: opens_submenu(element),
        children: Vec::new(),
    })
}

fn titles(level: &[IUIAutomationElement]) -> String {
    level
        .iter()
        .map(name_of)
        .filter(|title| !title.trim().is_empty())
        .map(|title| format!("“{title}”"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn collapse(item: Option<&IUIAutomationElement>) {
    if let Some(pattern) = item.and_then(expander) {
        let _ = unsafe { pattern.Collapse() };
    }
}

fn window_rect(target: HWND) -> Option<Rect> {
    let mut bounds = RECT::default();
    unsafe { GetWindowRect(target, &mut bounds).ok()? };
    Some(rect(bounds))
}

fn text(value: windows::core::Result<BSTR>) -> String {
    value.map(|s| s.to_string()).unwrap_or_default()
}

fn flag(value: windows::core::Result<BOOL>) -> Option<bool> {
    value.ok().map(|b| b.as_bool())
}

impl Platform {
    fn walker(&self) -> Result<IUIAutomationTreeWalker, String> {
        unsafe { self.automation.ControlViewWalker() }.map_err(|e| format!("accessibility_error: {e}"))
    }

    /// The items of the app's menu bar (not the window's system menu).
    fn menu_bar_items(&mut self, app: &AppInfo, walker: &IUIAutomationTreeWalker) -> Result<Vec<IUIAutomationElement>, String> {
        let windows = self.windows(app)?;
        let window = windows
            .iter()
            .find(|window| window.focused)
            .or(windows.first())
            .ok_or_else(|| format!("window_not_found: {} has no window.", app.name))?;
        let root = unsafe { self.automation.ElementFromHandle(hwnd(window.native)) }
            .map_err(|e| format!("accessibility_error: That window can't be read: {e}"))?;
        let mut queue = std::collections::VecDeque::from([(root, 0usize)]);
        let mut seen = 0;
        while let Some((element, depth)) = queue.pop_front() {
            seen += 1;
            if control_type(&element) == MENU_BAR && name_of(&element) != "System" {
                let items = menu_items(walker, &element);
                if !items.is_empty() {
                    return Ok(items);
                }
            }
            if depth < 6 && seen < 400 {
                queue.extend(children_of(walker, &element).into_iter().map(|child| (child, depth + 1)));
            }
        }
        Err(format!(
            "element_not_found: {} shows no menu bar to UI Automation (many newer apps draw their own): snapshot the window and use its menu buttons or keyboard shortcuts.",
            app.name
        ))
    }

    /// Open `item`'s submenu and answer its items: the item's own children,
    /// or the popup menu the app shows for it.
    fn open_submenu(&self, walker: &IUIAutomationTreeWalker, item: &IUIAutomationElement, pid: u32) -> Vec<IUIAutomationElement> {
        let Some(pattern) = expander(item) else {
            return Vec::new();
        };
        let _ = unsafe { pattern.Expand() };
        let title = name_of(item);
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(1500) {
            let own = menu_items(walker, item);
            if !own.is_empty() {
                return own;
            }
            if let Ok(desktop) = unsafe { self.automation.GetRootElement() } {
                let popups: Vec<_> = children_of(walker, &desktop)
                    .into_iter()
                    .filter(|top| {
                        control_type(top) == MENU
                            && unsafe { top.CurrentProcessId() }.is_ok_and(|id| id as u32 == pid)
                    })
                    .collect();
                let popup = popups
                    .iter()
                    .find(|popup| crate::same_menu_title(&name_of(popup), &title))
                    .or(popups.last());
                if let Some(items) = popup.map(|popup| menu_items(walker, popup)).filter(|items| !items.is_empty()) {
                    return items;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Vec::new()
    }

    /// Open the menus along `path`; answers the last level's items and the
    /// top-level item to close afterwards.
    fn menu_level(
        &mut self,
        app: &AppInfo,
        walker: &IUIAutomationTreeWalker,
        path: &[String],
    ) -> Result<(Vec<IUIAutomationElement>, Option<IUIAutomationElement>), String> {
        let mut level = self.menu_bar_items(app, walker)?;
        if !path.is_empty() {
            self.activate(app, None)?;
        }
        let mut opened: Option<IUIAutomationElement> = None;
        let mut trail: Vec<String> = Vec::new();
        for wanted in path {
            let Some(item) = level.iter().find(|item| crate::same_menu_title(&name_of(item), wanted)).cloned() else {
                collapse(opened.as_ref());
                return Err(format!(
                    "element_not_found: {} has no menu item “{wanted}”{}; there: {}.",
                    app.name,
                    if trail.is_empty() { String::new() } else { format!(" in {}", trail.join(" › ")) },
                    titles(&level)
                ));
            };
            trail.push(name_of(&item));
            if opened.is_none() {
                opened = Some(item.clone());
            }
            level = self.open_submenu(walker, &item, app.pid);
            if level.is_empty() {
                collapse(opened.as_ref());
                return Err(format!("invalid_argument: {} › {} opens no menu.", app.name, trail.join(" › ")));
            }
        }
        Ok((level, opened))
    }

    pub fn menus(&mut self, app: &AppInfo, path: &[String]) -> Result<Vec<crate::MenuEntry>, String> {
        let walker = self.walker()?;
        let (level, opened) = self.menu_level(app, &walker, path)?;
        let entries = level.iter().filter_map(menu_entry).take(60).collect();
        collapse(opened.as_ref());
        Ok(entries)
    }

    /// Choose the item at `path`; answers its full path.
    pub fn choose_menu(&mut self, app: &AppInfo, path: &[String]) -> Result<String, String> {
        let (last, parents) = path.split_last().ok_or("invalid_argument: Name a menu item.")?;
        if parents.is_empty() {
            return Err(format!(
                "invalid_argument: Name an item in a menu, like \"{last} > …\": a menu's own title only opens it (`menus APP \"{last}\"` lists its items)."
            ));
        }
        let walker = self.walker()?;
        let (level, opened) = self.menu_level(app, &walker, parents)?;
        let refuse = |message: String| {
            collapse(opened.as_ref());
            Err(message)
        };
        let Some(item) = level.iter().find(|item| crate::same_menu_title(&name_of(item), last)) else {
            return refuse(format!(
                "element_not_found: {} has no item “{last}” in {}; there: {}.",
                app.name,
                parents.join(" › "),
                titles(&level)
            ));
        };
        let title = name_of(item);
        if opens_submenu(item) {
            return refuse(format!("invalid_argument: “{title}” opens a submenu: name one of its items."));
        }
        if !menu_enabled(item) {
            return refuse(format!(
                "element_not_clickable: “{title}” is disabled right now: the app needs something first (a selection, an open document)."
            ));
        }
        let chosen = unsafe {
            match item.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
                Ok(invoke) => invoke.Invoke(),
                Err(_) => item
                    .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                    .and_then(|toggle| toggle.Toggle()),
            }
        };
        if let Err(error) = chosen {
            let hint = menu_shortcut(item)
                .map(|shortcut| format!(" Its shortcut is {shortcut}: press it with hotkey."))
                .unwrap_or_default();
            return refuse(format!("action_not_supported: {} didn't take the menu choice ({error}).{hint}", app.name));
        }
        let mut full: Vec<String> = parents.to_vec();
        full.push(title);
        Ok(full.join(" › "))
    }

    /// Move, resize or change the state of `window`; answers its frame after.
    pub fn set_window(&mut self, app: &AppInfo, window: &WindowInfo, change: &crate::WindowChange) -> Result<Rect, String> {
        let target = hwnd(window.native);
        if window_rect(target).is_none() {
            return Err(format!("window_not_found: {}'s window “{}” has closed.", app.name, window.title));
        }
        let refuse = |what: &str| {
            format!(
                "action_not_supported: {}'s window “{}” can't be {what} (an app running as administrator can only be changed by Dragent running as administrator too).",
                app.name, window.title
            )
        };
        unsafe {
            match change.state {
                Some(crate::WindowState::Minimize) => {
                    let _ = ShowWindow(target, SW_MINIMIZE);
                }
                Some(crate::WindowState::Restore) => {
                    let _ = ShowWindow(target, SW_RESTORE);
                    let _ = self.activate(app, Some(window));
                }
                Some(crate::WindowState::Zoom) => {
                    let command = if IsZoomed(target).as_bool() { SW_RESTORE } else { SW_MAXIMIZE };
                    let _ = ShowWindow(target, command);
                }
                None => {}
            }
            if change.position.is_some() || change.size.is_some() {
                // A minimized or maximized window has no place of its own.
                if IsIconic(target).as_bool() || IsZoomed(target).as_bool() {
                    let _ = ShowWindow(target, SW_RESTORE);
                }
                let mut flags = SWP_NOZORDER | SWP_NOACTIVATE;
                if change.position.is_none() {
                    flags |= SWP_NOMOVE;
                }
                if change.size.is_none() {
                    flags |= SWP_NOSIZE;
                }
                let (x, y) = change.position.unwrap_or_default();
                let (width, height) = change.size.unwrap_or_default();
                SetWindowPos(
                    target,
                    None,
                    x.round() as i32,
                    y.round() as i32,
                    width.round() as i32,
                    height.round() as i32,
                    flags,
                )
                .map_err(|_| refuse(if change.size.is_some() { "resized" } else { "moved" }))?;
            }
        }
        let wanted = |frame: &Rect| {
            let close = |a: f64, b: f64| (a - b).abs() <= 2.0;
            change.position.is_none_or(|(x, y)| close(frame.x, x) && close(frame.y, y))
                && change.size.is_none_or(|(w, h)| close(frame.width, w) && close(frame.height, h))
        };
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            if window_rect(target).is_some_and(|frame| wanted(&frame)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        window_rect(target).ok_or_else(|| "accessibility_error: The window's frame couldn't be read back.".into())
    }

    pub fn new() -> Result<Self, String> {
        unsafe {
            // Already initialised on this thread is fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| format!("UI Automation isn't available: {e}"))?;
            Ok(Self { automation })
        }
    }

    pub fn permissions(&mut self) -> Permissions {
        Permissions {
            platform: "Windows".into(),
            accessibility: Access::NotNeeded,
            screen_recording: Access::NotNeeded,
            input: Access::Granted,
            notes: vec![
                "Apps running as administrator can only be read and used while Dragent runs as administrator too."
                    .into(),
            ],
        }
    }

    pub fn apps(&mut self) -> Result<Vec<AppInfo>, String> {
        let front = foreground_pid();
        let mut apps: Vec<AppInfo> = Vec::new();
        for window in top_windows() {
            if window.pid == crate::owner_pid() || apps.iter().any(|app| app.pid == window.pid) {
                continue;
            }
            let path = image_path(window.pid).unwrap_or_default();
            let file = Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| window.title.clone());
            let name = Path::new(&file)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| file.clone());
            apps.push(AppInfo {
                name,
                id: file,
                pid: window.pid,
                active: window.pid == front,
            });
        }
        Ok(apps)
    }

    pub fn windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>, String> {
        let front = unsafe { GetForegroundWindow() };
        Ok(top_windows()
            .into_iter()
            .filter(|window| window.pid == app.pid)
            .enumerate()
            .map(|(index, window)| WindowInfo {
                id: (window.hwnd.0 as usize as u64).to_string(),
                index,
                title: window.title,
                frame: window.frame,
                focused: window.hwnd == front,
                native: window.hwnd.0 as usize as u64,
            })
            .collect())
    }

    pub fn tree(
        &mut self,
        _app: &AppInfo,
        window: &WindowInfo,
        limit: usize,
    ) -> Result<Vec<RawNode<Handle>>, String> {
        unsafe {
            let root = self
                .automation
                .ElementFromHandle(hwnd(window.native))
                .map_err(|e| format!("That window can't be read: {e}"))?;
            let walker = self
                .automation
                .ControlViewWalker()
                .map_err(|e| e.to_string())?;
            let mut reach = crate::tree::Reach::new(window.frame);
            let mut nodes = Vec::new();
            let mut stack = vec![(root, 0usize)];
            while let Some((element, depth)) = stack.pop() {
                if nodes.len() >= limit {
                    break;
                }
                let control = element.CurrentControlType().map(|t| t.0).unwrap_or(0);
                let role = role_name(control);
                let secure = flag(element.CurrentIsPassword()).unwrap_or(false);
                let toggles = matches!(control, 50002 | 50013);
                let checked = if toggles {
                    element
                        .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                        .ok()
                        .and_then(|toggle| toggle.CurrentToggleState().ok())
                        .map(|state| state == ToggleState_On)
                        .or_else(|| {
                            element
                                .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                                    UIA_SelectionItemPatternId,
                                )
                                .ok()
                                .and_then(|item| flag(item.CurrentIsSelected()))
                        })
                } else {
                    None
                };
                let value = if !secure && matches!(control, 50003 | 50004 | 50015 | 50016 | 50030) {
                    element
                        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                        .ok()
                        .map(|pattern| text(pattern.CurrentValue()))
                } else {
                    None
                };
                let frame = element
                    .CurrentBoundingRectangle()
                    .ok()
                    .map(rect)
                    .filter(|f| !f.is_empty());
                let mut children = Vec::new();
                if depth < MAX_DEPTH && reach.descend(depth, role, frame) {
                    let mut child = walker.GetFirstChildElement(&element).ok();
                    while let Some(current) = child {
                        child = walker.GetNextSiblingElement(&current).ok();
                        children.push(current);
                        if children.len() > 500 {
                            break;
                        }
                    }
                }
                nodes.push(RawNode {
                    depth,
                    role: role.to_string(),
                    name: text(element.CurrentName()),
                    value,
                    description: text(element.CurrentHelpText()),
                    placeholder: String::new(),
                    enabled: flag(element.CurrentIsEnabled()).unwrap_or(true),
                    focused: flag(element.CurrentHasKeyboardFocus()).unwrap_or(false),
                    selected: false,
                    checked,
                    secure,
                    frame,
                    actions: self.element_actions(&element),
                    has_children: !children.is_empty(),
                    handle: element,
                });
                for child in children.into_iter().rev() {
                    stack.push((child, depth + 1));
                }
            }
            Ok(nodes)
        }
    }

    pub fn screenshot(
        &mut self,
        _app: &AppInfo,
        window: &WindowInfo,
        path: &Path,
    ) -> Result<(), String> {
        let width = window.frame.width as i32;
        let height = window.frame.height as i32;
        if width <= 0 || height <= 0 || (width as u64) * (height as u64) > 64_000_000 {
            return Err("the window has no size".into());
        }
        unsafe {
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let previous = SelectObject(memory, bitmap.into());
            // 2 = PW_RENDERFULLCONTENT: windows drawn by the GPU too.
            let printed = PrintWindow(hwnd(window.native), memory, PRINT_WINDOW_FLAGS(2)).as_bool();
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            let rows = GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr() as *mut c_void),
                &mut info,
                DIB_RGB_COLORS,
            );
            SelectObject(memory, previous);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            if !printed || rows == 0 {
                return Err("the window couldn't be captured".into());
            }
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            image::save_buffer(
                path,
                &pixels,
                width as u32,
                height as u32,
                image::ColorType::Rgba8,
            )
            .map_err(|e| format!("the screenshot couldn't be saved: {e}"))
        }
    }

    pub fn activate(&mut self, app: &AppInfo, window: Option<&WindowInfo>) -> Result<(), String> {
        let target = match window {
            Some(window) => hwnd(window.native),
            None => top_windows()
                .into_iter()
                .find(|w| w.pid == app.pid)
                .map(|w| w.hwnd)
                .ok_or_else(|| format!("{} has no window to bring forward.", app.name))?,
        };
        unsafe {
            if IsIconic(target).as_bool() {
                let _ = ShowWindow(target, SW_RESTORE);
            }
            // Windows only lets the app with the last input take the
            // foreground: a released Alt key counts as that input.
            send(&[key_input(0x12, false), key_input(0x12, true)])?;
            let _ = SetForegroundWindow(target);
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            if foreground_pid() == app.pid {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "{} couldn't be brought to the front, so nothing was typed or clicked.",
            app.name
        ))
    }

    pub fn press(&mut self, handle: &Handle) -> Result<bool, String> {
        unsafe {
            if let Ok(invoke) =
                handle.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            {
                return Ok(invoke.Invoke().is_ok());
            }
            if let Ok(toggle) =
                handle.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            {
                return Ok(toggle.Toggle().is_ok());
            }
            if let Ok(item) = handle.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                UIA_SelectionItemPatternId,
            ) {
                return Ok(item.Select().is_ok());
            }
        }
        Ok(false)
    }

    pub fn set_value(&mut self, handle: &Handle, value: &str) -> Result<bool, String> {
        unsafe {
            let Ok(pattern) =
                handle.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            else {
                return Ok(false);
            };
            if flag(pattern.CurrentIsReadOnly()).unwrap_or(true) {
                return Ok(false);
            }
            Ok(pattern.SetValue(&BSTR::from(value)).is_ok())
        }
    }

    pub fn read_value(&mut self, handle: &Handle) -> Option<String> {
        unsafe {
            if handle
                .CurrentIsPassword()
                .map(|p| p.as_bool())
                .unwrap_or(true)
            {
                return None;
            }
            handle
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .ok()
                .map(|pattern| text(pattern.CurrentValue()))
        }
    }

    pub fn focus_element(&mut self, handle: &Handle) -> Result<(), String> {
        unsafe {
            handle
                .SetFocus()
                .map_err(|e| format!("That element can't take focus: {e}"))
        }
    }

    pub fn element_frame(&mut self, handle: &Handle) -> Option<Rect> {
        unsafe {
            handle
                .CurrentBoundingRectangle()
                .ok()
                .map(rect)
                .filter(|f| !f.is_empty())
        }
    }

    pub fn click_at(&mut self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
        let (down, up) = match button {
            Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        };
        unsafe { SetCursorPos(x.round() as i32, y.round() as i32).map_err(|e| e.to_string())? };
        std::thread::sleep(Duration::from_millis(30));
        for _ in 0..count {
            send(&[mouse_input(down, 0), mouse_input(up, 0)])?;
            std::thread::sleep(Duration::from_millis(40));
        }
        Ok(())
    }

    pub fn scroll_at(
        &mut self,
        x: f64,
        y: f64,
        direction: Direction,
        amount: u32,
    ) -> Result<(), String> {
        unsafe { SetCursorPos(x.round() as i32, y.round() as i32).map_err(|e| e.to_string())? };
        let delta = WHEEL_DELTA * amount as i32;
        let input = match direction {
            Direction::Down => mouse_input(MOUSEEVENTF_WHEEL, -delta),
            Direction::Up => mouse_input(MOUSEEVENTF_WHEEL, delta),
            Direction::Left => mouse_input(MOUSEEVENTF_HWHEEL, -delta),
            Direction::Right => mouse_input(MOUSEEVENTF_HWHEEL, delta),
        };
        send(&[input])
    }

    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                send(&[key_input(0x0D, false), key_input(0x0D, true)])?;
            }
            let mut inputs = Vec::new();
            for unit in line.trim_end_matches('\r').encode_utf16() {
                inputs.push(unicode_input(unit, false));
                inputs.push(unicode_input(unit, true));
            }
            for chunk in inputs.chunks(64) {
                send(chunk)?;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }

    pub fn press_keys(&mut self, combo: &Combo) -> Result<(), String> {
        let (code, needs_shift) = virtual_key(combo.key).ok_or_else(|| {
            format!(
                "{:?} has no key on this keyboard layout; type it as text instead.",
                combo.key
            )
        })?;
        let mut modifiers = Vec::new();
        if combo.ctrl {
            modifiers.push(0x11u16);
        }
        if combo.alt {
            modifiers.push(0x12);
        }
        if combo.shift || needs_shift {
            modifiers.push(0x10);
        }
        if combo.meta {
            modifiers.push(0x5B);
        }
        let mut inputs: Vec<INPUT> = modifiers.iter().map(|m| key_input(*m, false)).collect();
        inputs.push(key_input(code, false));
        inputs.push(key_input(code, true));
        inputs.extend(modifiers.iter().rev().map(|m| key_input(*m, true)));
        send(&inputs)
    }
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err("Windows refused the input (an app running as administrator may be in front).".into())
    }
}

fn key_input(code: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(code),
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: if up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// The virtual key for a key, and whether the layout needs Shift for it.
fn virtual_key(key: Key) -> Option<(u16, bool)> {
    Some(match key {
        Key::Named(named) => (
            match named {
                Named::Return => 0x0D,
                Named::Tab => 0x09,
                Named::Space => 0x20,
                Named::Backspace => 0x08,
                Named::Delete => 0x2E,
                Named::Escape => 0x1B,
                Named::Left => 0x25,
                Named::Up => 0x26,
                Named::Right => 0x27,
                Named::Down => 0x28,
                Named::Home => 0x24,
                Named::End => 0x23,
                Named::PageUp => 0x21,
                Named::PageDown => 0x22,
                Named::F(n) => 0x6F + n as u16,
            },
            false,
        ),
        Key::Char(c) if c.is_ascii_lowercase() => (c.to_ascii_uppercase() as u16, false),
        Key::Char(c) if c.is_ascii_digit() => (c as u16, false),
        Key::Char(c) => {
            let mut units = [0u16; 2];
            let units = c.encode_utf16(&mut units);
            if units.len() != 1 {
                return None;
            }
            let scan = unsafe { VkKeyScanW(units[0]) };
            if scan == -1 {
                return None;
            }
            ((scan & 0xFF) as u16, scan & 0x100 != 0)
        }
    })
}

pub fn request_permission(_kind: PermissionKind) -> bool {
    true
}

pub fn open_permission_settings(_kind: PermissionKind) -> Result<(), String> {
    Err("Windows doesn't ask for a permission for this.".into())
}

impl Platform {
    fn element_actions(&self, element: &Handle) -> Vec<String> {
        use windows::Win32::UI::Accessibility::*;
        let mut actions = Vec::new();
        unsafe {
            if element
                .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                .is_ok()
            {
                actions.push("invoke".into());
            }
            if element
                .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                .is_ok()
            {
                actions.push("toggle".into());
            }
            if element
                .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
                .is_ok()
            {
                actions.push("select".into());
            }
            if element
                .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
                .is_ok()
            {
                actions.extend(["expand".into(), "collapse".into()]);
            }
            if element
                .GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId)
                .is_ok()
            {
                actions.push("scroll_into_view".into());
            }
        }
        actions
    }
    pub fn perform_action(&mut self, element: &Handle, action: &str) -> Result<(), String> {
        use windows::Win32::UI::Accessibility::*;
        let result: windows::core::Result<()> = unsafe {
            (|| match action {
                "invoke" => element
                    .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?
                    .Invoke(),
                "toggle" => element
                    .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)?
                    .Toggle(),
                "select" => element
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )?
                    .Select(),
                "expand" => element
                    .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    )?
                    .Expand(),
                "collapse" => element
                    .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    )?
                    .Collapse(),
                "scroll_into_view" => element
                    .GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId)?
                    .ScrollIntoView(),
                _ => Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80004001u32 as i32,
                ))),
            })()
        };
        result.map_err(|e| format!("action_not_supported: {e}"))
    }
    pub fn text_receiver(&mut self, app: &AppInfo, _window: &WindowInfo) -> Result<bool, String> {
        unsafe {
            let element = self
                .automation
                .GetFocusedElement()
                .map_err(|e| e.to_string())?;
            Ok(element.CurrentProcessId().unwrap_or(0) as u32 == app.pid
                && matches!(
                    element.CurrentControlType().map(|c| c.0).unwrap_or(0),
                    50003 | 50004 | 50030
                ))
        }
    }
    pub fn modified_click(
        &mut self,
        x: f64,
        y: f64,
        button: Button,
        count: u32,
        combo: &Combo,
    ) -> Result<(), String> {
        unsafe {
            SetCursorPos(x.round() as i32, y.round() as i32).map_err(|e| e.to_string())?;
        }
        let mut modifiers = Vec::new();
        if combo.ctrl {
            modifiers.push(0x11);
        }
        if combo.alt {
            modifiers.push(0x12);
        }
        if combo.shift {
            modifiers.push(0x10);
        }
        if combo.meta {
            modifiers.push(0x5b);
        }
        let (down, up) = match button {
            Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        };
        let mut inputs: Vec<INPUT> = modifiers.iter().map(|m| key_input(*m, false)).collect();
        for _ in 0..count {
            inputs.extend([mouse_input(down, 0), mouse_input(up, 0)]);
        }
        let release: Vec<INPUT> = modifiers
            .iter()
            .rev()
            .map(|m| key_input(*m, true))
            .collect();
        inputs.extend(release.iter().copied());
        let result = send(&inputs);
        let _ = send(&release);
        result
    }
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Result<(), String> {
        unsafe {
            SetCursorPos(from.0.round() as i32, from.1.round() as i32)
                .map_err(|e| e.to_string())?;
        }
        send(&[mouse_input(MOUSEEVENTF_LEFTDOWN, 0)])?;
        let moved = (|| {
            for step in 1..=24 {
                let t = step as f64 / 24.0;
                unsafe {
                    SetCursorPos(
                        (from.0 + (to.0 - from.0) * t).round() as i32,
                        (from.1 + (to.1 - from.1) * t).round() as i32,
                    )
                    .map_err(|e| e.to_string())?;
                }
                std::thread::sleep(Duration::from_millis(12));
            }
            Ok::<(), String>(())
        })();
        let released = send(&[mouse_input(MOUSEEVENTF_LEFTUP, 0)]);
        moved.and(released)
    }
}
