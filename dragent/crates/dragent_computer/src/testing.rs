//! Deterministic desktop used only by protocol tests; production backends still compile.
use crate::{
    keys::Combo,
    tree::{AppInfo, RawNode, Rect, WindowInfo},
    Access, Button, Direction, PermissionKind, Permissions,
};
use std::path::Path;
pub type Handle = usize;
pub struct Platform {
    text: String,
    active: usize,
    text_focused: bool,
}
impl Platform {
    /// File › New (⌘N), File › Export › PDF…, File › Close (disabled).
    pub fn menus(&mut self, _: &AppInfo, path: &[String]) -> Result<Vec<crate::MenuEntry>, String> {
        let item = |title: &str, enabled: bool| crate::MenuEntry {
            title: title.into(),
            enabled,
            ..Default::default()
        };
        let export = crate::MenuEntry {
            submenu: true,
            children: vec![item("PDF…", true)],
            ..item("Export", true)
        };
        let file = crate::MenuEntry {
            submenu: true,
            children: vec![
                crate::MenuEntry { shortcut: Some("⌘N".into()), ..item("New", true) },
                export,
                item("Close", false),
            ],
            ..item("File", true)
        };
        let mut level = vec![file];
        for wanted in path {
            let found = level
                .iter()
                .find(|entry| crate::same_menu_title(&entry.title, wanted))
                .ok_or_else(|| format!("element_not_found: No menu item “{wanted}”."))?;
            level = found.children.clone();
        }
        Ok(level)
    }
    pub fn choose_menu(&mut self, app: &AppInfo, path: &[String]) -> Result<String, String> {
        let (last, parents) = path.split_last().ok_or("invalid_argument: Name a menu item.")?;
        let item = self
            .menus(app, parents)?
            .into_iter()
            .find(|entry| crate::same_menu_title(&entry.title, last))
            .ok_or_else(|| format!("element_not_found: No menu item “{last}”."))?;
        if item.submenu {
            return Err(format!("invalid_argument: “{}” opens a submenu: name an item in it.", item.title));
        }
        if !item.enabled {
            return Err(format!("element_not_clickable: “{}” is disabled right now.", item.title));
        }
        self.text = format!("chose {}", item.title);
        Ok(path.join(" › "))
    }
    pub fn set_window(&mut self, _: &AppInfo, window: &WindowInfo, change: &crate::WindowChange) -> Result<Rect, String> {
        let (x, y) = change.position.unwrap_or((window.frame.x, window.frame.y));
        // Like many apps, this one keeps windows at least 300 wide.
        let (width, height) = change.size.map(|(w, h)| (w.max(300.0), h)).unwrap_or((window.frame.width, window.frame.height));
        Ok(Rect { x, y, width, height })
    }

    pub fn new() -> Result<Self, String> {
        Ok(Self {
            text: String::new(),
            active: 0,
            text_focused: true,
        })
    }
    pub fn permissions(&mut self) -> Permissions {
        Permissions {
            platform: "test".into(),
            accessibility: Access::Granted,
            screen_recording: Access::Missing,
            input: Access::Granted,
            notes: vec![],
        }
    }
    pub fn apps(&mut self) -> Result<Vec<AppInfo>, String> {
        Ok(vec![AppInfo {
            name: "Test App".into(),
            id: "test.app".into(),
            pid: 42,
            active: true,
        }])
    }
    pub fn windows(&mut self, _: &AppInfo) -> Result<Vec<WindowInfo>, String> {
        Ok((0..2)
            .map(|index| WindowInfo {
                id: format!("window-{index}"),
                index,
                title: format!("Document {index}"),
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 600.0,
                    height: 400.0,
                },
                focused: index == self.active,
                native: index as u64 + 1,
            })
            .collect())
    }
    pub fn tree(
        &mut self,
        _: &AppInfo,
        _: &WindowInfo,
        _: usize,
    ) -> Result<Vec<RawNode<Handle>>, String> {
        Ok((0..2)
            .map(|i| RawNode {
                depth: 0,
                role: if i == 0 { "text field" } else { "button" }.into(),
                name: if i == 0 { "Editor" } else { "Save" }.into(),
                value: if i == 0 {
                    Some(self.text.clone())
                } else {
                    None
                },
                description: String::new(),
                placeholder: String::new(),
                enabled: true,
                focused: i == 0 && self.text_focused,
                selected: false,
                checked: None,
                secure: false,
                frame: Some(Rect {
                    x: (i * 100) as f64,
                    y: 10.0,
                    width: 80.0,
                    height: 40.0,
                }),
                actions: if i == 1 { vec!["press".into()] } else { vec![] },
                has_children: false,
                handle: i,
            })
            .collect())
    }
    pub fn screenshot(&mut self, _: &AppInfo, _: &WindowInfo, _: &Path) -> Result<(), String> {
        Err("Screen capture disabled in the protocol test desktop".into())
    }
    pub fn activate(&mut self, _: &AppInfo, window: Option<&WindowInfo>) -> Result<(), String> {
        if let Some(window) = window {
            self.active = window.index;
        }
        Ok(())
    }
    pub fn press(&mut self, handle: &Handle) -> Result<bool, String> {
        if *handle == 0 {
            self.text_focused = true;
        }
        Ok(true)
    }
    pub fn set_value(&mut self, _: &Handle, value: &str) -> Result<bool, String> {
        self.text = value.into();
        Ok(true)
    }
    pub fn read_value(&mut self, _: &Handle) -> Option<String> {
        Some(self.text.clone())
    }
    pub fn focus_element(&mut self, _: &Handle) -> Result<(), String> {
        self.text_focused = true;
        Ok(())
    }
    pub fn element_frame(&mut self, handle: &Handle) -> Option<Rect> {
        Some(Rect {
            x: (handle * 100) as f64,
            y: 10.0,
            width: 80.0,
            height: 40.0,
        })
    }
    pub fn click_at(&mut self, _: f64, _: f64, _: Button, _: u32) -> Result<(), String> {
        Ok(())
    }
    pub fn scroll_at(&mut self, _: f64, _: f64, _: Direction, _: u32) -> Result<(), String> {
        Ok(())
    }
    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.text.push_str(text);
        Ok(())
    }
    pub fn press_keys(&mut self, _: &Combo) -> Result<(), String> {
        self.text_focused = false;
        Ok(())
    }
    pub fn perform_action(&mut self, _: &Handle, _: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn text_receiver(&mut self, _: &AppInfo, _: &WindowInfo) -> Result<bool, String> {
        Ok(self.text_focused)
    }
    pub fn modified_click(
        &mut self,
        _: f64,
        _: f64,
        _: Button,
        _: u32,
        _: &Combo,
    ) -> Result<(), String> {
        Ok(())
    }
    pub fn drag(&mut self, _: (f64, f64), _: (f64, f64)) -> Result<(), String> {
        Ok(())
    }
}
pub fn request_permission(_: PermissionKind) -> bool {
    false
}
pub fn open_permission_settings(_: PermissionKind) -> Result<(), String> {
    Err("No OS settings in protocol tests".into())
}
