//! Platforms without computer use: every request says so.

use std::path::Path;

use crate::keys::Combo;
use crate::tree::{AppInfo, RawNode, Rect, WindowInfo};
use crate::{Access, Button, Direction, PermissionKind, Permissions};

pub struct Handle;

const UNAVAILABLE: &str = "Computer use isn't available on this platform.";

pub struct Platform;

impl Platform {
    pub fn menus(&mut self, _: &AppInfo, _: &[String]) -> Result<Vec<crate::MenuEntry>, String> {
        Err(format!("unsupported_capability: Reading menus isn't available on this platform in this build; use the app's keyboard shortcuts (key, hotkey)."))
    }
    pub fn choose_menu(&mut self, _: &AppInfo, _: &[String]) -> Result<String, String> {
        Err(format!("unsupported_capability: Choosing menu items isn't available on this platform in this build; use the app's keyboard shortcut (hotkey)."))
    }
    pub fn set_window(&mut self, _: &AppInfo, _: &WindowInfo, _: &crate::WindowChange) -> Result<Rect, String> {
        Err(format!("unsupported_capability: Moving, resizing or minimizing windows isn't available on this platform in this build."))
    }

    pub fn new() -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn permissions(&mut self) -> Permissions {
        Permissions {
            platform: std::env::consts::OS.into(),
            accessibility: Access::Unavailable,
            screen_recording: Access::Unavailable,
            input: Access::Unavailable,
            notes: vec![UNAVAILABLE.into()],
        }
    }
    pub fn apps(&mut self) -> Result<Vec<AppInfo>, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn windows(&mut self, _: &AppInfo) -> Result<Vec<WindowInfo>, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn tree(
        &mut self,
        _: &AppInfo,
        _: &WindowInfo,
        _: usize,
    ) -> Result<Vec<RawNode<Handle>>, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn screenshot(&mut self, _: &AppInfo, _: &WindowInfo, _: &Path) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn activate(&mut self, _: &AppInfo, _: Option<&WindowInfo>) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn press(&mut self, _: &Handle) -> Result<bool, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn set_value(&mut self, _: &Handle, _: &str) -> Result<bool, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn read_value(&mut self, _: &Handle) -> Option<String> {
        None
    }
    pub fn focus_element(&mut self, _: &Handle) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn element_frame(&mut self, _: &Handle) -> Option<Rect> {
        None
    }
    pub fn click_at(&mut self, _: f64, _: f64, _: Button, _: u32) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn scroll_at(&mut self, _: f64, _: f64, _: Direction, _: u32) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn type_text(&mut self, _: &str) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn press_keys(&mut self, _: &Combo) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
}

pub fn request_permission(_: PermissionKind) -> bool {
    false
}

pub fn open_permission_settings(_: PermissionKind) -> Result<(), String> {
    Err(UNAVAILABLE.into())
}

impl Platform {
    pub fn perform_action(&mut self, _: &Handle, _: &str) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn text_receiver(&mut self, _: &AppInfo, _: &WindowInfo) -> Result<bool, String> {
        Ok(false)
    }
    pub fn modified_click(
        &mut self,
        _: f64,
        _: f64,
        _: Button,
        _: u32,
        _: &Combo,
    ) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    pub fn drag(&mut self, _: (f64, f64), _: (f64, f64)) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
}
