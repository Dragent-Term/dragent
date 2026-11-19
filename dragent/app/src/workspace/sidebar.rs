//! Sidebar types, tab colors, and display helpers.

use warpui::color::ColorU;

use super::PaneTab;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarViewMode {
    List,
    Compact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarFocus {
    None,
    Search,
    Rename(usize),
}

pub type TabColorOption = Option<ColorU>;

fn rgb(r: u8, g: u8, b: u8) -> ColorU {
    ColorU::new(r, g, b, 255)
}

pub fn tab_color_options() -> [TabColorOption; 7] {
    [
        None,
        Some(rgb(55, 128, 233)),
        Some(rgb(28, 160, 90)),
        Some(rgb(188, 161, 246)),
        Some(rgb(229, 160, 26)),
        Some(rgb(191, 64, 157)),
        Some(rgb(121, 156, 146)),
    ]
}

impl PaneTab {
    pub fn display_label(&self) -> &str {
        match self.custom_label.as_deref().unwrap_or(&self.title) {
            // Dragent's agent tabs were called "Agent" before.
            "Agent" => "Dragent",
            label => label,
        }
    }
}

pub fn color_to_hex(color: ColorU) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

pub fn color_from_hex(hex: &str) -> Option<ColorU> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(ColorU::new(r, g, b, 255))
}
