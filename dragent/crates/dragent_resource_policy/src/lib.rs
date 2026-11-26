//! Central resource budgets and reusable bounded-content primitives.
//!
//! Product features should consume these policies instead of inventing local
//! limits. That gives terminals, conversations, artifacts, and future role
//! workers one predictable scaling model.

mod budgeted_queue;

pub use budgeted_queue::{BudgetedQueue, Eviction};
pub mod agent_slots;
pub mod build_slots;

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLevel {
    Foreground,
    Background,
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    Normal,
    Elevated,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    pub repaint_interval: Duration,
    pub hot_content_items: usize,
    pub hot_content_bytes: usize,
    pub raw_asset_bytes: usize,
}

impl ResourceBudget {
    pub fn for_state(activity: ActivityLevel, pressure: MemoryPressure) -> Self {
        let mut budget = match activity {
            ActivityLevel::Foreground => Self {
                repaint_interval: Duration::from_millis(16),
                hot_content_items: 512,
                hot_content_bytes: 32 * 1024 * 1024,
                raw_asset_bytes: 96 * 1024 * 1024,
            },
            ActivityLevel::Background => Self {
                repaint_interval: Duration::from_millis(250),
                hot_content_items: 128,
                hot_content_bytes: 8 * 1024 * 1024,
                raw_asset_bytes: 64 * 1024 * 1024,
            },
            ActivityLevel::Suspended => Self {
                repaint_interval: Duration::from_secs(1),
                hot_content_items: 32,
                hot_content_bytes: 2 * 1024 * 1024,
                raw_asset_bytes: 32 * 1024 * 1024,
            },
        };
        match pressure {
            MemoryPressure::Normal => {}
            MemoryPressure::Elevated => {
                budget.hot_content_items /= 2;
                budget.hot_content_bytes /= 2;
                budget.raw_asset_bytes /= 2;
            }
            MemoryPressure::Critical => {
                budget.hot_content_items = budget.hot_content_items.min(16);
                budget.hot_content_bytes = budget.hot_content_bytes.min(1024 * 1024);
                budget.raw_asset_bytes = budget.raw_asset_bytes.min(16 * 1024 * 1024);
            }
        }
        budget
    }
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self::for_state(ActivityLevel::Foreground, MemoryPressure::Normal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn less_active_states_receive_smaller_budgets() {
        let foreground =
            ResourceBudget::for_state(ActivityLevel::Foreground, MemoryPressure::Normal);
        let suspended = ResourceBudget::for_state(ActivityLevel::Suspended, MemoryPressure::Normal);
        assert!(suspended.repaint_interval > foreground.repaint_interval);
        assert!(suspended.hot_content_bytes < foreground.hot_content_bytes);
        assert!(suspended.raw_asset_bytes < foreground.raw_asset_bytes);
    }

    #[test]
    fn pressure_never_increases_a_budget() {
        let normal = ResourceBudget::for_state(ActivityLevel::Foreground, MemoryPressure::Normal);
        let critical =
            ResourceBudget::for_state(ActivityLevel::Foreground, MemoryPressure::Critical);
        assert!(critical.hot_content_items <= normal.hot_content_items);
        assert!(critical.hot_content_bytes <= normal.hot_content_bytes);
        assert!(critical.raw_asset_bytes <= normal.raw_asset_bytes);
    }
}
