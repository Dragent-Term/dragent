//! Which copy button is showing "copied", and for how much longer.

use crate::icons::AppIcon;
use crate::theme::Theme;
use std::time::Duration;
use warpui_core::color::ColorU;

/// How long a copy button shows its checkmark.
pub(crate) const COPY_CONFIRMATION: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(crate) struct CopyConfirmation {
    key: Option<String>,
    generation: u64,
}

impl CopyConfirmation {
    pub(crate) fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Starts confirming `key`, replacing any earlier confirmation. Pass the
    /// returned generation to `expire` when the time is up.
    pub(crate) fn show(&mut self, key: String) -> u64 {
        self.key = Some(key);
        self.generation += 1;
        self.generation
    }

    /// Ends the confirmation started as `generation`. A later copy has its own
    /// full time, so an older timer leaves it alone. Returns whether anything
    /// changed.
    pub(crate) fn expire(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.key.take().is_some()
    }
}

/// The icon and color of a copy icon button.
pub(crate) fn copy_glyph(copied: bool, idle_color: ColorU, theme: Theme) -> (AppIcon, ColorU) {
    if copied {
        (AppIcon::Check, theme.ui_green())
    } else {
        (AppIcon::Copy, idle_color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiring_ends_the_confirmation() {
        let mut confirmation = CopyConfirmation::default();
        let generation = confirmation.show("block:1".into());
        assert_eq!(confirmation.key(), Some("block:1"));
        assert!(confirmation.expire(generation));
        assert_eq!(confirmation.key(), None);
        assert!(!confirmation.expire(generation));
    }

    #[test]
    fn an_older_timer_leaves_a_newer_copy_showing() {
        let mut confirmation = CopyConfirmation::default();
        let first = confirmation.show("block:1".into());
        let second = confirmation.show("block:1".into());
        assert!(!confirmation.expire(first));
        assert_eq!(confirmation.key(), Some("block:1"));
        assert!(confirmation.expire(second));
        assert_eq!(confirmation.key(), None);
    }
}
