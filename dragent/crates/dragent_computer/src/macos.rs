//! macOS: the Accessibility API reads windows and presses or fills their
//! elements; Quartz events move the mouse and type; `screencapture` takes
//! window pictures. Needs the Accessibility permission (and Screen
//! Recording for pictures) for Dragent.

use std::ffi::c_void;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFGetTypeID, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGSize};

use crate::keys::{Combo, Key, Named};
use crate::tree::{humanize_role, AppInfo, RawNode, Reach, Rect, WindowInfo};
use crate::{Access, Button, Direction, PermissionKind, Permissions};

type AXError = i32;
const AX_OK: AXError = 0;
const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;
const AX_VALUE_ERROR: u32 = 5;
/// Deepest element a snapshot follows.
const MAX_DEPTH: usize = 40;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyMultipleAttributeValues(
        element: CFTypeRef,
        attributes: CFArrayRef,
        options: u32,
        values: *mut CFArrayRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementIsAttributeSettable(
        element: CFTypeRef,
        attribute: CFStringRef,
        settable: *mut u8,
    ) -> AXError;
    fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> AXError;
    fn AXUIElementCopyActionNames(element: CFTypeRef, names: *mut CFArrayRef) -> AXError;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> AXError;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXValueGetType(value: CFTypeRef) -> u32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

const ACCESS_NEEDED: &str =
    "Dragent needs the Accessibility permission to read and use other apps: \
System Settings → Privacy & Security → Accessibility (or Settings → Computer use in Dragent).";

/// An accessibility element, kept from a snapshot for the actions after it.
pub struct Handle(CFType);

impl Handle {
    fn ptr(&self) -> CFTypeRef {
        self.0.as_CFTypeRef()
    }
}

fn created(ptr: CFTypeRef) -> Option<CFType> {
    (!ptr.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(ptr) })
}

fn copy_attr(element: CFTypeRef, name: &str) -> Option<CFType> {
    let key = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let error =
        unsafe { AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut value) };
    if error != AX_OK {
        return None;
    }
    created(value)
}

fn is_ax_value(value: &CFType) -> bool {
    unsafe { CFGetTypeID(value.as_CFTypeRef()) == AXValueGetTypeID() }
}

fn as_string(value: &CFType) -> Option<String> {
    value.downcast::<CFString>().map(|s| s.to_string())
}

fn as_bool(value: &CFType) -> Option<bool> {
    if let Some(flag) = value.downcast::<CFBoolean>() {
        return Some(bool::from(flag));
    }
    value
        .downcast::<CFNumber>()
        .and_then(|n| n.to_i64())
        .map(|n| n != 0)
}

fn as_number(value: &CFType) -> Option<f64> {
    value.downcast::<CFNumber>().and_then(|n| n.to_f64())
}

fn as_point(value: &CFType) -> Option<CGPoint> {
    let mut point = CGPoint::new(0.0, 0.0);
    (is_ax_value(value)
        && unsafe {
            AXValueGetValue(
                value.as_CFTypeRef(),
                AX_VALUE_CGPOINT,
                &mut point as *mut _ as *mut c_void,
            )
        } != 0)
        .then_some(point)
}

fn as_size(value: &CFType) -> Option<CGSize> {
    let mut size = CGSize::new(0.0, 0.0);
    (is_ax_value(value)
        && unsafe {
            AXValueGetValue(
                value.as_CFTypeRef(),
                AX_VALUE_CGSIZE,
                &mut size as *mut _ as *mut c_void,
            )
        } != 0)
        .then_some(size)
}

fn as_array(value: &CFType) -> Vec<CFType> {
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != CFArray::<CFType>::type_id() {
        return Vec::new();
    }
    let array: CFArray<CFType> =
        unsafe { CFArray::wrap_under_get_rule(value.as_CFTypeRef() as CFArrayRef) };
    array.iter().map(|item| item.clone()).collect()
}

fn frame_of(position: Option<&CFType>, size: Option<&CFType>) -> Option<Rect> {
    let point = as_point(position?)?;
    let size = as_size(size?)?;
    Some(Rect {
        x: point.x,
        y: point.y,
        width: size.width,
        height: size.height,
    })
}

fn element_frame(element: CFTypeRef) -> Option<Rect> {
    frame_of(
        copy_attr(element, "AXPosition").as_ref(),
        copy_attr(element, "AXSize").as_ref(),
    )
}

fn set_attr(element: CFTypeRef, name: &str, value: &CFType) -> bool {
    let key = CFString::new(name);
    unsafe {
        AXUIElementSetAttributeValue(element, key.as_concrete_TypeRef(), value.as_CFTypeRef())
            == AX_OK
    }
}

fn action_names(element: CFTypeRef) -> Vec<String> {
    let mut names: CFArrayRef = std::ptr::null();
    if unsafe { AXUIElementCopyActionNames(element, &mut names) } != AX_OK || names.is_null() {
        return Vec::new();
    }
    let names: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(names) };
    names.iter().filter_map(|name| as_string(&name)).collect()
}

/// The app in front, as Accessibility sees it now. The running-apps list
/// learns of a switch a moment later, too late for a check made right after
/// bringing an app forward.
fn frontmost_pid() -> Option<u32> {
    let system = created(unsafe { AXUIElementCreateSystemWide() })?;
    let app = copy_attr(system.as_CFTypeRef(), "AXFocusedApplication")?;
    let mut pid = 0;
    (unsafe { AXUIElementGetPid(app.as_CFTypeRef(), &mut pid) } == AX_OK && pid > 0)
        .then_some(pid as u32)
}

fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// What each element read for a snapshot asks for, in this order.
const NODE_ATTRIBUTES: [&str; 12] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXPlaceholderValue",
    "AXEnabled",
    "AXFocused",
    "AXSelected",
    "AXPosition",
    "AXSize",
    "AXChildren",
];

pub struct Platform {
    node_attributes: CFArray<CFString>,
    /// Apps already asked to build their web content's tree.
    revealed: std::collections::HashSet<u32>,
}

/// Chromium-based browsers, which build their pages' accessibility tree
/// only for an assistive app that asks.
fn chromium(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    [
        "com.google.chrome",
        "org.chromium.",
        "com.microsoft.edgemac",
        "com.brave.browser",
        "company.thebrowser.",
        "com.vivaldi.",
        "com.operasoftware.",
        "com.openai.atlas",
        "ai.perplexity.comet",
    ]
    .iter()
    .any(|prefix| id.starts_with(prefix))
}

impl Platform {
    pub fn new() -> Result<Self, String> {
        // A hung app must not hold a request for long: every call gives up
        // after this many seconds.
        if let Some(system) = created(unsafe { AXUIElementCreateSystemWide() }) {
            unsafe { AXUIElementSetMessagingTimeout(system.as_CFTypeRef(), 3.0) };
        }
        let names: Vec<CFString> = NODE_ATTRIBUTES
            .iter()
            .map(|name| CFString::new(name))
            .collect();
        Ok(Self {
            node_attributes: CFArray::from_CFTypes(&names),
            revealed: Default::default(),
        })
    }

    pub fn permissions(&mut self) -> Permissions {
        let accessibility = if trusted() {
            Access::Granted
        } else {
            Access::Missing
        };
        let screen = if unsafe { CGPreflightScreenCaptureAccess() } {
            Access::Granted
        } else {
            Access::Missing
        };
        let mut notes = Vec::new();
        if accessibility == Access::Missing {
            notes.push("Grant Accessibility to Dragent to read and use other apps (System Settings → Privacy & Security → Accessibility).".into());
        }
        if screen == Access::Missing {
            notes.push("Grant Screen Recording to Dragent for screenshots (System Settings → Privacy & Security → Screen & System Audio Recording). Snapshots work without it, as text.".into());
        }
        Permissions {
            platform: "macOS".into(),
            accessibility,
            screen_recording: screen,
            input: accessibility,
            notes,
        }
    }

    pub fn apps(&mut self) -> Result<Vec<AppInfo>, String> {
        use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
        let front = frontmost_pid();
        let apps = objc2::rc::autoreleasepool(|_| {
            let workspace = NSWorkspace::sharedWorkspace();
            let running = workspace.runningApplications();
            let mut apps = Vec::new();
            for index in 0..running.count() {
                let app = running.objectAtIndex(index);
                if app.activationPolicy() != NSApplicationActivationPolicy::Regular
                    || app.isTerminated()
                {
                    continue;
                }
                let pid = app.processIdentifier();
                if pid <= 0 || pid as u32 == crate::owner_pid() {
                    continue;
                }
                let name = app
                    .localizedName()
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                let id = app
                    .bundleIdentifier()
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| name.clone());
                apps.push(AppInfo {
                    name,
                    id,
                    pid: pid as u32,
                    active: front.map_or_else(|| app.isActive(), |front| front == pid as u32),
                });
            }
            apps
        });
        Ok(apps)
    }

    fn app_element(&self, app: &AppInfo) -> Result<CFType, String> {
        if !trusted() {
            return Err(ACCESS_NEEDED.into());
        }
        created(unsafe { AXUIElementCreateApplication(app.pid as i32) })
            .ok_or_else(|| format!("{} can't be reached.", app.name))
    }

    /// Electron apps and Chromium browsers leave their web content out of
    /// the accessibility tree until an assistive app asks for it, once per
    /// process; it then takes a moment to appear.
    fn reveal_web_content(&mut self, app: &AppInfo) {
        if !self.revealed.insert(app.pid) {
            return;
        }
        let Ok(element) = self.app_element(app) else {
            return;
        };
        let on = CFBoolean::true_value().as_CFType();
        let mut asked = set_attr(element.as_CFTypeRef(), "AXManualAccessibility", &on);
        if chromium(&app.id) {
            asked |= set_attr(element.as_CFTypeRef(), "AXEnhancedUserInterface", &on);
        }
        if asked {
            std::thread::sleep(Duration::from_millis(400));
        }
    }

    fn ax_windows(&self, app: &AppInfo) -> Result<Vec<CFType>, String> {
        let element = self.app_element(app)?;
        Ok(copy_attr(element.as_CFTypeRef(), "AXWindows")
            .map(|w| as_array(&w))
            .unwrap_or_default())
    }

    pub fn windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>, String> {
        let element = self.app_element(app)?;
        let focused = copy_attr(element.as_CFTypeRef(), "AXFocusedWindow");
        let on_screen = cg_windows(app.pid);
        let windows = self.ax_windows(app)?;
        Ok(windows
            .iter()
            .enumerate()
            .map(|(index, window)| {
                let frame = element_frame(window.as_CFTypeRef()).unwrap_or_default();
                let title = copy_attr(window.as_CFTypeRef(), "AXTitle")
                    .and_then(|t| as_string(&t))
                    .unwrap_or_default();
                let native = on_screen
                    .iter()
                    .filter(|(_, bounds)| close(bounds, &frame))
                    .map(|(id, _)| *id)
                    .next()
                    .unwrap_or(0);
                WindowInfo {
                    id: if native != 0 {
                        native.to_string()
                    } else {
                        format!("ax:{index}:{}", title)
                    },
                    index,
                    title,
                    frame,
                    focused: focused.as_ref().is_some_and(|f| f == window),
                    native,
                }
            })
            .collect())
    }

    fn window_element(&self, app: &AppInfo, window: &WindowInfo) -> Result<CFType, String> {
        let windows = self.ax_windows(app)?;
        windows
            .iter()
            .find(|w| {
                element_frame(w.as_CFTypeRef()).is_some_and(|frame| close(&frame, &window.frame))
            })
            .or_else(|| windows.get(window.index))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "That window of {} has closed. Take a new snapshot.",
                    app.name
                )
            })
    }

    pub fn tree(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        limit: usize,
    ) -> Result<Vec<RawNode<Handle>>, String> {
        self.reveal_web_content(app);
        let root = self.window_element(app, window)?;
        let mut reach = Reach::new(window.frame);
        let mut nodes = Vec::new();
        let mut stack = vec![(root, 0usize)];
        while let Some((element, depth)) = stack.pop() {
            if nodes.len() >= limit {
                break;
            }
            let mut values: CFArrayRef = std::ptr::null();
            let error = unsafe {
                AXUIElementCopyMultipleAttributeValues(
                    element.as_CFTypeRef(),
                    self.node_attributes.as_concrete_TypeRef(),
                    0,
                    &mut values,
                )
            };
            if error != AX_OK || values.is_null() {
                continue;
            }
            let values: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(values) };
            let get = |i: usize| -> Option<CFType> {
                let value = values.get(i as isize)?.clone();
                let failed = is_ax_value(&value)
                    && unsafe { AXValueGetType(value.as_CFTypeRef()) } == AX_VALUE_ERROR;
                (!failed).then_some(value)
            };
            let text = |i: usize| get(i).and_then(|v| as_string(&v)).unwrap_or_default();
            let role_name = text(0);
            let subrole = text(1);
            let secure = subrole == "AXSecureTextField";
            let role = match subrole.as_str() {
                "AXSecureTextField" => "password field".to_string(),
                "AXSearchField" => "search field".to_string(),
                "AXSwitch" => "switch".to_string(),
                _ => humanize_role(&role_name),
            };
            let raw_value = get(4);
            let toggles = matches!(role_name.as_str(), "AXCheckBox" | "AXRadioButton")
                || subrole == "AXSwitch";
            let (value, checked) = match raw_value {
                Some(v) if toggles => (None, as_bool(&v)),
                Some(v) => (
                    as_string(&v).or_else(|| as_number(&v).map(|n| format!("{n}"))),
                    None,
                ),
                None => (None, None),
            };
            let children = get(11).map(|c| as_array(&c)).unwrap_or_default();
            let frame = frame_of(get(9).as_ref(), get(10).as_ref());
            let descend = reach.descend(depth, &role, frame);
            nodes.push(RawNode {
                depth,
                role,
                name: text(2),
                value,
                description: text(3),
                placeholder: text(5),
                enabled: get(6).and_then(|v| as_bool(&v)).unwrap_or(true),
                focused: get(7).and_then(|v| as_bool(&v)).unwrap_or(false),
                selected: get(8).and_then(|v| as_bool(&v)).unwrap_or(false),
                checked,
                secure,
                frame,
                actions: action_names(element.as_CFTypeRef()),
                has_children: !children.is_empty(),
                handle: Handle(element),
            });
            if depth < MAX_DEPTH && descend {
                for child in children.into_iter().rev() {
                    stack.push((child, depth + 1));
                }
            }
        }
        Ok(nodes)
    }

    pub fn screenshot(
        &mut self,
        _app: &AppInfo,
        window: &WindowInfo,
        path: &Path,
    ) -> Result<(), String> {
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            return Err("Screen Recording isn't allowed for Dragent".into());
        }
        let path_text = path.to_string_lossy().to_string();
        let mut command = Command::new("/usr/sbin/screencapture");
        command.arg("-x");
        if window.native == 0 {
            return Err("The target window could not be identified for a screenshot. Use its accessibility tree.".into());
        }
        command.args(["-o", "-l", &window.native.to_string(), &path_text]);
        let status = command
            .status()
            .map_err(|e| format!("screencapture didn't run: {e}"))?;
        let written = std::fs::metadata(path)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if status.success() && written {
            Ok(())
        } else {
            Err("the window couldn't be captured".into())
        }
    }

    pub fn activate(&mut self, app: &AppInfo, window: Option<&WindowInfo>) -> Result<(), String> {
        let element = self.app_element(app)?;
        set_attr(
            element.as_CFTypeRef(),
            "AXFrontmost",
            &CFBoolean::true_value().as_CFType(),
        );
        if let Some(window) = window {
            if let Ok(window) = self.window_element(app, window) {
                set_attr(
                    window.as_CFTypeRef(),
                    "AXMinimized",
                    &CFBoolean::false_value().as_CFType(),
                );
                let key = CFString::new("AXRaise");
                unsafe {
                    AXUIElementPerformAction(window.as_CFTypeRef(), key.as_concrete_TypeRef())
                };
                set_attr(
                    window.as_CFTypeRef(),
                    "AXMain",
                    &CFBoolean::true_value().as_CFType(),
                );
            }
        }
        let frontmost = |element: &CFType| {
            copy_attr(element.as_CFTypeRef(), "AXFrontmost")
                .and_then(|v| as_bool(&v))
                .unwrap_or(false)
        };
        if wait_for(|| frontmost(&element), Duration::from_millis(600)) {
            return Ok(());
        }
        // Some apps ignore the attribute; Launch Services brings them forward.
        let _ = Command::new("/usr/bin/open").args(["-b", &app.id]).status();
        if wait_for(|| frontmost(&element), Duration::from_millis(1500)) {
            Ok(())
        } else {
            Err(format!(
                "{} couldn't be brought to the front, so nothing was typed or clicked.",
                app.name
            ))
        }
    }

    pub fn press(&mut self, handle: &Handle) -> Result<bool, String> {
        if !action_names(handle.ptr())
            .iter()
            .any(|name| name == "AXPress")
        {
            return Ok(false);
        }
        let key = CFString::new("AXPress");
        Ok(unsafe { AXUIElementPerformAction(handle.ptr(), key.as_concrete_TypeRef()) } == AX_OK)
    }

    pub fn set_value(&mut self, handle: &Handle, value: &str) -> Result<bool, String> {
        let key = CFString::new("AXValue");
        let mut settable = 0u8;
        let readable = unsafe {
            AXUIElementIsAttributeSettable(handle.ptr(), key.as_concrete_TypeRef(), &mut settable)
        } == AX_OK;
        if !readable || settable == 0 {
            return Ok(false);
        }
        Ok(set_attr(
            handle.ptr(),
            "AXValue",
            &CFString::new(value).as_CFType(),
        ))
    }

    pub fn read_value(&mut self, handle: &Handle) -> Option<String> {
        if copy_attr(handle.ptr(), "AXSubrole")
            .and_then(|v| as_string(&v))
            .is_some_and(|s| s == "AXSecureTextField")
        {
            return None;
        }
        copy_attr(handle.ptr(), "AXValue").and_then(|v| as_string(&v))
    }

    pub fn focus_element(&mut self, handle: &Handle) -> Result<(), String> {
        if !set_attr(
            handle.ptr(),
            "AXFocused",
            &CFBoolean::true_value().as_CFType(),
        ) {
            return Err("The target field could not take focus; nothing was typed.".into());
        }
        if !wait_for(
            || {
                copy_attr(handle.ptr(), "AXFocused")
                    .and_then(|v| as_bool(&v))
                    .unwrap_or(false)
            },
            Duration::from_millis(500),
        ) {
            return Err("The target field did not take focus; nothing was typed.".into());
        }
        Ok(())
    }

    pub fn element_frame(&mut self, handle: &Handle) -> Option<Rect> {
        element_frame(handle.ptr())
    }

    pub fn click_at(&mut self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
        let point = CGPoint::new(x, y);
        let (down, up, which) = match button {
            Button::Left => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
            Button::Right => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
            Button::Middle => (
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseUp,
                CGMouseButton::Center,
            ),
        };
        mouse(CGEventType::MouseMoved, point, CGMouseButton::Left, 0)?;
        std::thread::sleep(Duration::from_millis(30));
        for click in 1..=count as i64 {
            mouse(down, point, which, click)?;
            mouse(up, point, which, click)?;
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
        mouse(
            CGEventType::MouseMoved,
            CGPoint::new(x, y),
            CGMouseButton::Left,
            0,
        )?;
        let lines = amount as i32;
        let (vertical, horizontal) = match direction {
            Direction::Down => (-lines, 0),
            Direction::Up => (lines, 0),
            Direction::Left => (0, lines),
            Direction::Right => (0, -lines),
        };
        let event =
            CGEvent::new_scroll_event(source()?, ScrollEventUnit::LINE, 2, vertical, horizontal, 0)
                .map_err(|_| "Couldn't create a scroll event.".to_string())?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                key_event(36, CGEventFlags::CGEventFlagNull)?;
            }
            let units: Vec<u16> = line.encode_utf16().collect();
            for chunk in units.chunks(16) {
                for down in [true, false] {
                    let event = CGEvent::new_keyboard_event(source()?, 0, down)
                        .map_err(|_| "Couldn't create a key event.".to_string())?;
                    event.set_string_from_utf16_unchecked(chunk);
                    // Plain text, whatever modifier the system thinks is down.
                    event.set_flags(CGEventFlags::CGEventFlagNull);
                    event.post(CGEventTapLocation::HID);
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        }
        Ok(())
    }

    pub fn press_keys(&mut self, combo: &Combo) -> Result<(), String> {
        let code = key_code(combo.key).ok_or_else(|| {
            format!(
                "{:?} has no key on this keyboard layout; type it as text instead.",
                combo.key
            )
        })?;
        with_modifiers(combo, |flags| key_event(code, flags))
    }
}

fn wait_for(condition: impl Fn() -> bool, within: Duration) -> bool {
    let start = Instant::now();
    loop {
        if condition() {
            return true;
        }
        if start.elapsed() >= within {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn close(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() < 3.0
        && (a.y - b.y).abs() < 3.0
        && (a.width - b.width).abs() < 3.0
        && (a.height - b.height).abs() < 3.0
}

/// The app's normal on-screen windows: (window number, bounds).
fn cg_windows(pid: u32) -> Vec<(u64, Rect)> {
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionOnScreenOnly,
    };
    let Some(list) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let list: CFArray<CFType> = unsafe { CFArray::wrap_under_get_rule(list.as_concrete_TypeRef()) };
    let number = |dict: &CFDictionary<CFString, CFType>, key: &'static str| {
        dict.find(CFString::from_static_string(key))
            .and_then(|v| as_number(&v))
    };
    list.iter()
        .filter_map(|item| {
            let dict: CFDictionary<CFString, CFType> = unsafe {
                CFDictionary::wrap_under_get_rule(item.as_CFTypeRef() as CFDictionaryRef)
            };
            if number(&dict, "kCGWindowOwnerPID")? as u32 != pid
                || number(&dict, "kCGWindowLayer")? != 0.0
            {
                return None;
            }
            let id = number(&dict, "kCGWindowNumber")? as u64;
            let bounds = dict.find(CFString::from_static_string("kCGWindowBounds"))?;
            let bounds: CFDictionary<CFString, CFType> = unsafe {
                CFDictionary::wrap_under_get_rule(bounds.as_CFTypeRef() as CFDictionaryRef)
            };
            Some((
                id,
                Rect {
                    x: number(&bounds, "X")?,
                    y: number(&bounds, "Y")?,
                    width: number(&bounds, "Width")?,
                    height: number(&bounds, "Height")?,
                },
            ))
        })
        .collect()
}

fn source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Couldn't create input events.".to_string())
}

fn mouse(
    kind: CGEventType,
    point: CGPoint,
    button: CGMouseButton,
    clicks: i64,
) -> Result<(), String> {
    let event = CGEvent::new_mouse_event(source()?, kind, point, button)
        .map_err(|_| "Couldn't create a mouse event.".to_string())?;
    if clicks > 0 {
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, clicks);
    }
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn key_event(code: CGKeyCode, flags: CGEventFlags) -> Result<(), String> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(source()?, code, down)
            .map_err(|_| "Couldn't create a key event.".to_string())?;
        event.set_flags(flags);
        event.post(CGEventTapLocation::HID);
    }
    std::thread::sleep(Duration::from_millis(20));
    Ok(())
}

/// Virtual key codes of the ANSI layout.
fn key_code(key: Key) -> Option<CGKeyCode> {
    Some(match key {
        Key::Named(named) => match named {
            Named::Return => 36,
            Named::Tab => 48,
            Named::Space => 49,
            Named::Backspace => 51,
            Named::Delete => 117,
            Named::Escape => 53,
            Named::Left => 123,
            Named::Right => 124,
            Named::Down => 125,
            Named::Up => 126,
            Named::Home => 115,
            Named::End => 119,
            Named::PageUp => 116,
            Named::PageDown => 121,
            Named::F(n) => *[
                122u16, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64,
                79, 80, 90,
            ]
            .get(n as usize - 1)?,
        },
        Key::Char(c) => match c {
            'a' => 0,
            's' => 1,
            'd' => 2,
            'f' => 3,
            'h' => 4,
            'g' => 5,
            'z' => 6,
            'x' => 7,
            'c' => 8,
            'v' => 9,
            'b' => 11,
            'q' => 12,
            'w' => 13,
            'e' => 14,
            'r' => 15,
            'y' => 16,
            't' => 17,
            '1' => 18,
            '2' => 19,
            '3' => 20,
            '4' => 21,
            '6' => 22,
            '5' => 23,
            '=' => 24,
            '9' => 25,
            '7' => 26,
            '-' => 27,
            '8' => 28,
            '0' => 29,
            ']' => 30,
            'o' => 31,
            'u' => 32,
            '[' => 33,
            'i' => 34,
            'p' => 35,
            'l' => 37,
            'j' => 38,
            '\'' => 39,
            'k' => 40,
            ';' => 41,
            '\\' => 42,
            ',' => 43,
            '/' => 44,
            'n' => 45,
            'm' => 46,
            '.' => 47,
            '`' => 50,
            ' ' => 49,
            _ => return None,
        },
    })
}

pub fn request_permission(kind: PermissionKind) -> bool {
    match kind {
        PermissionKind::Accessibility => {
            let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
            let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
            unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0 }
        }
        PermissionKind::ScreenRecording => unsafe { CGRequestScreenCaptureAccess() },
    }
}

pub fn open_permission_settings(kind: PermissionKind) -> Result<(), String> {
    let pane = match kind {
        PermissionKind::Accessibility => "Privacy_Accessibility",
        PermissionKind::ScreenRecording => "Privacy_ScreenCapture",
    };
    Command::new("/usr/bin/open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{pane}"
        ))
        .status()
        .map_err(|e| e.to_string())
        .and_then(|status| {
            status
                .success()
                .then_some(())
                .ok_or_else(|| "System Settings didn't open.".into())
        })
}

// ── Menus and window geometry ───────────────────────────────────────────────

fn children_of(element: &CFType) -> Vec<CFType> {
    copy_attr(element.as_CFTypeRef(), "AXChildren")
        .map(|value| as_array(&value))
        .unwrap_or_default()
}

fn text_attr(element: &CFType, name: &str) -> String {
    copy_attr(element.as_CFTypeRef(), name)
        .and_then(|value| as_string(&value))
        .unwrap_or_default()
}

/// The items of the menu an item (or menu bar item) opens; none for a leaf.
fn submenu_items(item: &CFType) -> Vec<CFType> {
    children_of(item)
        .into_iter()
        .find(|child| text_attr(child, "AXRole") == "AXMenu")
        .map(|menu| children_of(&menu))
        .unwrap_or_default()
}

fn menu_enabled(item: &CFType) -> bool {
    copy_attr(item.as_CFTypeRef(), "AXEnabled")
        .and_then(|value| as_bool(&value))
        .unwrap_or(true)
}

/// `⌘⇧N`, from the item's key and modifier mask (shift 1, option 2,
/// control 4, no command 8).
fn menu_shortcut(item: &CFType) -> Option<String> {
    let key = text_attr(item, "AXMenuItemCmdChar");
    if key.trim().is_empty() {
        return None;
    }
    let mask = copy_attr(item.as_CFTypeRef(), "AXMenuItemCmdModifiers")
        .and_then(|value| as_number(&value))
        .unwrap_or(0.0) as u32;
    let mut text = String::new();
    for (bit, glyph) in [(4, '⌃'), (2, '⌥'), (1, '⇧')] {
        if mask & bit != 0 {
            text.push(glyph);
        }
    }
    if mask & 8 == 0 {
        text.push('⌘');
    }
    text.push_str(&key);
    Some(text)
}

/// One menu item, with `depth` levels of its submenu; separators are none.
fn menu_entry(item: &CFType, depth: usize) -> Option<crate::MenuEntry> {
    let title = text_attr(item, "AXTitle");
    if title.trim().is_empty() {
        return None;
    }
    let submenu = submenu_items(item);
    Some(crate::MenuEntry {
        title,
        enabled: menu_enabled(item),
        checked: !text_attr(item, "AXMenuItemMarkChar").trim().is_empty(),
        shortcut: menu_shortcut(item),
        submenu: !submenu.is_empty(),
        children: if depth > 0 {
            submenu
                .iter()
                .filter_map(|child| menu_entry(child, depth - 1))
                .collect()
        } else {
            Vec::new()
        },
    })
}

fn titles(level: &[CFType]) -> String {
    level
        .iter()
        .map(|item| text_attr(item, "AXTitle"))
        .filter(|title| !title.trim().is_empty())
        .map(|title| format!("“{title}”"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn settable(element: CFTypeRef, name: &str) -> bool {
    let key = CFString::new(name);
    let mut flag = 0u8;
    let checked =
        unsafe { AXUIElementIsAttributeSettable(element, key.as_concrete_TypeRef(), &mut flag) };
    checked == AX_OK && flag != 0
}

fn press_element(element: CFTypeRef) -> AXError {
    let key = CFString::new("AXPress");
    unsafe { AXUIElementPerformAction(element, key.as_concrete_TypeRef()) }
}

impl Platform {
    /// The items at `path` in the app's menus (the menu bar's own items for
    /// an empty path), with one level below each.
    fn menu_level(&mut self, app: &AppInfo, path: &[String]) -> Result<Vec<CFType>, String> {
        let element = self.app_element(app)?;
        let bar = copy_attr(element.as_CFTypeRef(), "AXMenuBar").ok_or_else(|| {
            format!("element_not_found: {} shows no menu bar to Accessibility.", app.name)
        })?;
        let mut level = children_of(&bar);
        let mut trail: Vec<String> = Vec::new();
        for wanted in path {
            let item = level
                .iter()
                .find(|item| crate::same_menu_title(&text_attr(item, "AXTitle"), wanted))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "element_not_found: {} has no menu item “{wanted}”{}; there: {}.",
                        app.name,
                        if trail.is_empty() { String::new() } else { format!(" in {}", trail.join(" › ")) },
                        titles(&level)
                    )
                })?;
            trail.push(text_attr(&item, "AXTitle"));
            level = submenu_items(&item);
            if level.is_empty() {
                return Err(format!(
                    "invalid_argument: {} › {} opens no menu.",
                    app.name,
                    trail.join(" › ")
                ));
            }
        }
        Ok(level)
    }

    pub fn menus(&mut self, app: &AppInfo, path: &[String]) -> Result<Vec<crate::MenuEntry>, String> {
        Ok(self
            .menu_level(app, path)?
            .iter()
            .filter_map(|item| menu_entry(item, 1))
            .take(60)
            .collect())
    }

    /// Choose the item at `path`; answers its full path.
    pub fn choose_menu(&mut self, app: &AppInfo, path: &[String]) -> Result<String, String> {
        let (last, parents) = path
            .split_last()
            .ok_or("invalid_argument: Name a menu item.")?;
        if parents.is_empty() {
            return Err(format!(
                "invalid_argument: Name an item in a menu, like \"{last} > …\": a menu's own title only opens it (`menus APP \"{last}\"` lists its items)."
            ));
        }
        let level = self.menu_level(app, parents)?;
        let item = level
            .iter()
            .find(|item| crate::same_menu_title(&text_attr(item, "AXTitle"), last))
            .ok_or_else(|| {
                format!(
                    "element_not_found: {} has no item “{last}” in {}; there: {}.",
                    app.name,
                    parents.join(" › "),
                    titles(&level)
                )
            })?;
        let title = text_attr(item, "AXTitle");
        let submenu = submenu_items(item);
        if !submenu.is_empty() {
            return Err(format!(
                "invalid_argument: “{title}” opens a submenu: name one of its items ({}).",
                titles(&submenu)
            ));
        }
        if !menu_enabled(item) {
            return Err(format!(
                "element_not_clickable: “{title}” is disabled right now: the app needs something first (a selection, an open document)."
            ));
        }
        let error = press_element(item.as_CFTypeRef());
        if error != AX_OK {
            let hint = menu_shortcut(item)
                .map(|shortcut| format!(" Its shortcut is {shortcut}: press it with hotkey."))
                .unwrap_or_default();
            return Err(format!(
                "action_not_supported: {} didn't take the menu choice (Accessibility error {error}).{hint}",
                app.name
            ));
        }
        let mut full: Vec<String> = parents.to_vec();
        full.push(title);
        Ok(full.join(" › "))
    }

    /// Move, resize or change the state of `window`; answers its frame after.
    pub fn set_window(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        change: &crate::WindowChange,
    ) -> Result<Rect, String> {
        let element = self.window_element(app, window)?;
        let ptr = element.as_CFTypeRef();
        let refuse = |what: &str| {
            format!(
                "action_not_supported: {}'s window “{}” can't be {what} (full screen, or the app doesn't allow it).",
                app.name, window.title
            )
        };
        if let Some(state) = change.state {
            match state {
                crate::WindowState::Minimize => {
                    if !settable(ptr, "AXMinimized") {
                        return Err(refuse("minimized"));
                    }
                    set_attr(ptr, "AXMinimized", &CFBoolean::true_value().as_CFType());
                }
                crate::WindowState::Restore => {
                    if copy_attr(ptr, "AXFullScreen").and_then(|value| as_bool(&value)) == Some(true) {
                        set_attr(ptr, "AXFullScreen", &CFBoolean::false_value().as_CFType());
                    }
                    set_attr(ptr, "AXMinimized", &CFBoolean::false_value().as_CFType());
                    let raise = CFString::new("AXRaise");
                    unsafe { AXUIElementPerformAction(ptr, raise.as_concrete_TypeRef()) };
                }
                crate::WindowState::Zoom => {
                    let button = copy_attr(ptr, "AXZoomButton").ok_or_else(|| refuse("zoomed"))?;
                    if press_element(button.as_CFTypeRef()) != AX_OK {
                        return Err(refuse("zoomed"));
                    }
                }
            }
        }
        let place = |x: f64, y: f64| -> Result<(), String> {
            let point = CGPoint::new(x, y);
            let value = created(unsafe { AXValueCreate(AX_VALUE_CGPOINT, &point as *const _ as *const c_void) })
                .ok_or("accessibility_error: Couldn't describe the position.")?;
            set_attr(ptr, "AXPosition", &value);
            Ok(())
        };
        if change.position.is_some() && !settable(ptr, "AXPosition") {
            return Err(refuse("moved"));
        }
        if change.size.is_some() && !settable(ptr, "AXSize") {
            return Err(refuse("resized"));
        }
        if let Some((x, y)) = change.position {
            place(x, y)?;
        }
        if let Some((width, height)) = change.size {
            let size = CGSize::new(width, height);
            let value = created(unsafe { AXValueCreate(AX_VALUE_CGSIZE, &size as *const _ as *const c_void) })
                .ok_or("accessibility_error: Couldn't describe the size.")?;
            set_attr(ptr, "AXSize", &value);
            // A size can push the window over a screen edge, which moves it.
            if let Some((x, y)) = change.position {
                place(x, y)?;
            }
        }
        let wanted = |frame: &Rect| {
            let close = |a: f64, b: f64| (a - b).abs() <= 2.0;
            change.position.is_none_or(|(x, y)| close(frame.x, x) && close(frame.y, y))
                && change.size.is_none_or(|(w, h)| close(frame.width, w) && close(frame.height, h))
        };
        wait_for(
            || element_frame(ptr).is_some_and(|frame| wanted(&frame)),
            Duration::from_millis(800),
        );
        element_frame(ptr).ok_or_else(|| "accessibility_error: The window's frame couldn't be read back.".into())
    }
}

impl Platform {
    pub fn perform_action(&mut self, handle: &Handle, action: &str) -> Result<(), String> {
        if !action_names(handle.ptr()).iter().any(|name| name == action) {
            return Err(
                "action_not_supported: The element no longer advertises this action.".into(),
            );
        }
        let name = CFString::new(action);
        if unsafe { AXUIElementPerformAction(handle.ptr(), name.as_concrete_TypeRef()) } != AX_OK {
            return Err("accessibility_error: The element refused the action.".into());
        }
        Ok(())
    }
    pub fn text_receiver(&mut self, app: &AppInfo, _window: &WindowInfo) -> Result<bool, String> {
        let root = self.app_element(app)?;
        let Some(focused) = copy_attr(root.as_CFTypeRef(), "AXFocusedUIElement") else {
            return Ok(false);
        };
        let role = copy_attr(focused.as_CFTypeRef(), "AXRole")
            .and_then(|v| as_string(&v))
            .unwrap_or_default();
        Ok(matches!(
            role.as_str(),
            "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSecureTextField"
        ))
    }
    pub fn modified_click(
        &mut self,
        x: f64,
        y: f64,
        button: Button,
        count: u32,
        combo: &Combo,
    ) -> Result<(), String> {
        let (down, up, which) = match button {
            Button::Left => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
            Button::Right => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
            Button::Middle => (
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseUp,
                CGMouseButton::Center,
            ),
        };
        mouse(CGEventType::MouseMoved, CGPoint::new(x, y), which, 0)?;
        with_modifiers(combo, |flags| {
            for click in 1..=count {
                for kind in [down, up] {
                    let event =
                        CGEvent::new_mouse_event(source()?, kind, CGPoint::new(x, y), which)
                            .map_err(|_| "Mouse event unavailable")?;
                    event.set_flags(flags);
                    event
                        .set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, click as i64);
                    event.post(CGEventTapLocation::HID);
                }
            }
            Ok(())
        })
    }
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Result<(), String> {
        mouse(
            CGEventType::MouseMoved,
            CGPoint::new(from.0, from.1),
            CGMouseButton::Left,
            0,
        )?;
        mouse(
            CGEventType::LeftMouseDown,
            CGPoint::new(from.0, from.1),
            CGMouseButton::Left,
            1,
        )?;
        let moved = (|| {
            for step in 1..=24 {
                let t = step as f64 / 24.0;
                mouse(
                    CGEventType::LeftMouseDragged,
                    CGPoint::new(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t),
                    CGMouseButton::Left,
                    1,
                )?;
                std::thread::sleep(Duration::from_millis(12));
            }
            Ok::<(), String>(())
        })();
        let released = mouse(
            CGEventType::LeftMouseUp,
            CGPoint::new(to.0, to.1),
            CGMouseButton::Left,
            1,
        );
        moved.and(released)
    }
}
/// Hold `combo`'s modifiers around `act` the way a person does: each key
/// pressed, then released. Flags on the event alone leave the system
/// thinking the modifier is still down, so text typed next arrives as
/// shortcuts (a "z" as Command-Z).
fn with_modifiers(
    combo: &Combo,
    act: impl FnOnce(CGEventFlags) -> Result<(), String>,
) -> Result<(), String> {
    let held: Vec<(CGKeyCode, CGEventFlags)> = [
        (combo.meta, 55, CGEventFlags::CGEventFlagCommand),
        (combo.ctrl, 59, CGEventFlags::CGEventFlagControl),
        (combo.alt, 58, CGEventFlags::CGEventFlagAlternate),
        (combo.shift, 56, CGEventFlags::CGEventFlagShift),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, code, flag)| (code, flag))
    .collect();
    let mut flags = CGEventFlags::CGEventFlagNull;
    let mut pressed = Vec::new();
    let mut result = Ok(());
    for (code, flag) in &held {
        flags |= *flag;
        if let Err(error) = modifier_key(*code, true, flags) {
            result = Err(error);
            break;
        }
        pressed.push((*code, *flag));
    }
    if result.is_ok() {
        result = act(flags);
    }
    // Released even when something failed, never left held.
    for (code, flag) in pressed.into_iter().rev() {
        flags.remove(flag);
        let _ = modifier_key(code, false, flags);
    }
    result
}

fn modifier_key(code: CGKeyCode, down: bool, flags: CGEventFlags) -> Result<(), String> {
    let event = CGEvent::new_keyboard_event(source()?, code, down)
        .map_err(|_| "Couldn't create a key event.".to_string())?;
    event.set_flags(flags);
    event.post(CGEventTapLocation::HID);
    Ok(())
}
