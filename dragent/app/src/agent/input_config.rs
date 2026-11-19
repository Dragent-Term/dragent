use crate::console::editor_settings::EditorSettings;

use super::detected_input_type::DetectedInputType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputConfig {
    pub input_type: DetectedInputType,
    pub is_locked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabSessionKind {
    Shell,
    Agent,
}

impl InputConfig {
    pub fn new_autodetect_enabled() -> Self {
        Self {
            input_type: DetectedInputType::Shell,
            is_locked: false,
        }
    }

    pub fn for_tab_kind(kind: TabSessionKind, autodetection_enabled: bool) -> Self {
        let input_type = match kind {
            TabSessionKind::Shell => DetectedInputType::Shell,
            TabSessionKind::Agent => DetectedInputType::Agent,
        };
        Self {
            input_type,
            is_locked: !autodetection_enabled,
        }
    }

    pub fn should_run_autodetection(&self, settings: &EditorSettings) -> bool {
        settings.input_autodetection_enabled && !self.is_locked
    }

    /// Cycles: unlocked → lock-current-type → (if was Shell) lock-Agent → unlock.
    pub fn toggle_lock(&mut self) {
        if !self.is_locked {
            self.is_locked = true;
        } else if self.input_type == DetectedInputType::Shell {
            self.input_type = DetectedInputType::Agent;
        } else {
            self.is_locked = false;
            self.input_type = DetectedInputType::Shell;
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self::new_autodetect_enabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(autodetect: bool) -> EditorSettings {
        EditorSettings {
            input_autodetection_enabled: autodetect,
            ..Default::default()
        }
    }

    #[test]
    fn default_is_shell_unlocked() {
        let cfg = InputConfig::default();
        assert_eq!(cfg.input_type, DetectedInputType::Shell);
        assert!(!cfg.is_locked);
    }

    #[test]
    fn autodetection_runs_when_unlocked_and_enabled() {
        let cfg = InputConfig::default();
        assert!(cfg.should_run_autodetection(&settings(true)));
    }

    #[test]
    fn autodetection_skipped_when_locked() {
        let cfg = InputConfig {
            input_type: DetectedInputType::Shell,
            is_locked: true,
        };
        assert!(!cfg.should_run_autodetection(&settings(true)));
    }

    #[test]
    fn autodetection_skipped_when_setting_disabled() {
        let cfg = InputConfig::default();
        assert!(!cfg.should_run_autodetection(&settings(false)));
    }

    #[test]
    fn for_shell_tab_with_autodetect() {
        let cfg = InputConfig::for_tab_kind(TabSessionKind::Shell, true);
        assert_eq!(cfg.input_type, DetectedInputType::Shell);
        assert!(!cfg.is_locked);
    }

    #[test]
    fn for_agent_tab_with_autodetect() {
        let cfg = InputConfig::for_tab_kind(TabSessionKind::Agent, true);
        assert_eq!(cfg.input_type, DetectedInputType::Agent);
        assert!(!cfg.is_locked);
    }

    #[test]
    fn for_tab_without_autodetect_is_locked() {
        let cfg = InputConfig::for_tab_kind(TabSessionKind::Shell, false);
        assert!(cfg.is_locked);
    }

    #[test]
    fn toggle_lock_cycles_correctly() {
        let mut cfg = InputConfig {
            input_type: DetectedInputType::Shell,
            is_locked: false,
        };
        // unlock → lock shell
        cfg.toggle_lock();
        assert_eq!(cfg.input_type, DetectedInputType::Shell);
        assert!(cfg.is_locked);
        // lock shell → lock agent
        cfg.toggle_lock();
        assert_eq!(cfg.input_type, DetectedInputType::Agent);
        assert!(cfg.is_locked);
        // lock agent → unlock shell
        cfg.toggle_lock();
        assert!(!cfg.is_locked);
        assert_eq!(cfg.input_type, DetectedInputType::Shell);
    }

    /// The cycle must always return to the unlocked state. Every UI path that
    /// pins the mode (`/shell`, `/agent`, picking a chat mode, voice input)
    /// sets `is_locked`, so if the cycle could not reach `false` again,
    /// auto-detection would be a one-way door.
    #[test]
    fn toggle_lock_always_returns_to_auto() {
        for start in [DetectedInputType::Shell, DetectedInputType::Agent] {
            let mut cfg = InputConfig {
                input_type: start,
                is_locked: true,
            };
            let mut reached_auto = false;
            for _ in 0..4 {
                cfg.toggle_lock();
                if !cfg.is_locked {
                    reached_auto = true;
                    break;
                }
            }
            assert!(reached_auto, "locked-{start:?} never cycles back to auto");
        }
    }
}
