//! What holds new agents back: critically low memory, or a cap the person
//! set. There is no limit of our own. Also the per-agent memory limit.
//!
//! The cap and the memory limit live in `<org>/resource-limits.json`
//! (Settings → Machines); both are empty by default.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent_sleep::Pressure;
pub use crate::protocol::WaitReason;

pub const RESOURCE_LIMITS_FILENAME: &str = "resource-limits.json";
/// Smallest memory limit that makes sense for an agent.
pub const MIN_AGENT_MEMORY_LIMIT: u64 = 256 << 20;
/// Without a limit of the person's own, an agent is a runaway only above
/// this, and above a quarter of the machine's memory.
const DEFAULT_AGENT_MEMORY_LIMIT: u64 = 4 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_agents: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_memory_limit_bytes: Option<u64>,
}

impl ResourceLimits {
    /// The limits set for `org_dir`; none when absent or unreadable.
    pub fn load(org_dir: &Path) -> Self {
        std::fs::read(org_dir.join(RESOURCE_LIMITS_FILENAME))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, org_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(org_dir).map_err(|e| e.to_string())?;
        crate::agent_sessions::write_atomic(org_dir, RESOURCE_LIMITS_FILENAME, &bytes)
    }

    pub fn max_agents(&self) -> Option<usize> {
        dragent_resource_policy::agent_slots::clamp_max_agents(self.max_agents)
    }

    /// The limit the person set, if any.
    pub fn agent_memory_limit(&self) -> Option<u64> {
        self.agent_memory_limit_bytes
            .map(|limit| limit.max(MIN_AGENT_MEMORY_LIMIT))
    }
}

/// Above this an agent counts as a runaway when no limit is set.
pub fn default_agent_memory_limit(total_memory: u64) -> u64 {
    DEFAULT_AGENT_MEMORY_LIMIT.max(total_memory / 4)
}

/// Whether an agent holding `bytes` is paused. A limit the person set always
/// counts; our own only while the machine is close to stalling, since a big
/// build is no runaway on a machine that has the memory for it. `allowed` is
/// what the person let this agent have by resuming it.
pub fn should_pause(
    bytes: u64,
    limits: &ResourceLimits,
    allowed: Option<u64>,
    total_memory: u64,
    pressure: Option<Pressure>,
) -> bool {
    let limit = match limits.agent_memory_limit() {
        Some(limit) => limit,
        None if pressure == Some(Pressure::Critical) => default_agent_memory_limit(total_memory),
        None => return false,
    };
    bytes > limit.max(allowed.unwrap_or(0))
}

/// Whether new starts wait. An unreadable pressure (`None`) never holds one.
pub fn held(cap: Option<usize>, live: usize, pressure: Option<Pressure>) -> Option<WaitReason> {
    if let Some(max) = cap.filter(|max| live >= *max) {
        return Some(WaitReason::AtCap { live, max });
    }
    (pressure == Some(Pressure::Critical)).then_some(WaitReason::MemoryCritical)
}

/// Whether new starts wait on this machine right now. The processes are
/// only looked at when a cap is set and enough agents have records to
/// reach it.
pub fn held_now(org_dir: &Path) -> Option<WaitReason> {
    let cap = ResourceLimits::load(org_dir).max_agents();
    let live = cap.map_or(0, |max| {
        let panes = awake_panes(&org_dir.join("agent-sessions"), &|pane| {
            crate::agent_sessions::is_asleep(pane) || crate::agent_sessions::is_waiting(pane)
        });
        if panes.len() < max {
            panes.len()
        } else {
            running_count(&panes)
        }
    });
    held(cap, live, crate::agent_sleep::gate_pressure())
}

/// Panes whose agent is recorded as running and isn't asleep or waiting.
fn awake_panes(sessions_dir: &Path, idle: &dyn Fn(uuid::Uuid) -> bool) -> Vec<uuid::Uuid> {
    crate::agent_memory::active_sessions(sessions_dir)
        .into_iter()
        .map(|record| record.pane_uuid)
        .filter(|pane| !idle(*pane))
        .collect()
}

/// How many of `panes` have a live launch monitor.
fn running_count(panes: &[uuid::Uuid]) -> usize {
    let system = crate::agent_memory::processes();
    let wrappers = crate::agent_memory::wrappers(&system);
    panes.iter().filter(|p| wrappers.contains_key(p)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dragent-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nothing_is_limited_unless_the_person_says_so() {
        let org = scratch("limits");
        assert_eq!(ResourceLimits::load(&org), ResourceLimits::default());
        assert_eq!(ResourceLimits::load(&org).max_agents(), None);
        let limits = ResourceLimits {
            max_agents: Some(6),
            agent_memory_limit_bytes: Some(2 * GB),
        };
        limits.save(&org).unwrap();
        assert_eq!(ResourceLimits::load(&org), limits);
        ResourceLimits {
            max_agents: Some(0),
            agent_memory_limit_bytes: Some(1),
        }
        .save(&org)
        .unwrap();
        let clamped = ResourceLimits::load(&org);
        assert_eq!(clamped.max_agents(), Some(1));
        assert_eq!(clamped.agent_memory_limit(), Some(MIN_AGENT_MEMORY_LIMIT));
        std::fs::write(org.join(RESOURCE_LIMITS_FILENAME), "not json").unwrap();
        assert_eq!(ResourceLimits::load(&org), ResourceLimits::default());
        let _ = std::fs::remove_dir_all(&org);
    }

    #[test]
    fn only_critical_memory_or_a_set_cap_holds_starts() {
        assert_eq!(held(None, 500, Some(Pressure::Normal)), None);
        assert_eq!(held(None, 500, Some(Pressure::Elevated)), None);
        assert_eq!(held(None, 500, None), None, "unreadable never holds");
        assert_eq!(
            held(None, 0, Some(Pressure::Critical)),
            Some(WaitReason::MemoryCritical)
        );
        assert_eq!(held(Some(3), 2, Some(Pressure::Normal)), None);
        assert_eq!(
            held(Some(3), 3, None),
            Some(WaitReason::AtCap { live: 3, max: 3 })
        );
    }

    #[test]
    fn our_own_limit_needs_a_machine_close_to_stalling_and_a_set_one_does_not() {
        let ours = ResourceLimits::default();
        let critical = Some(Pressure::Critical);
        assert!(!should_pause(
            9 * GB,
            &ours,
            None,
            16 * GB,
            Some(Pressure::Normal)
        ));
        assert!(!should_pause(9 * GB, &ours, None, 16 * GB, None));
        assert!(should_pause(9 * GB, &ours, None, 16 * GB, critical));
        assert!(
            !should_pause(9 * GB, &ours, None, 64 * GB, critical),
            "a quarter of 64 GB"
        );
        let set = ResourceLimits {
            agent_memory_limit_bytes: Some(2 * GB),
            ..ours
        };
        assert!(should_pause(
            3 * GB,
            &set,
            None,
            64 * GB,
            Some(Pressure::Normal)
        ));
        // Resumed by the person: only once it outgrows what they allowed.
        assert!(!should_pause(3 * GB, &set, Some(4 * GB), 64 * GB, critical));
        assert!(should_pause(5 * GB, &set, Some(4 * GB), 64 * GB, critical));
    }

    #[test]
    fn sleeping_and_waiting_agents_do_not_count_towards_a_cap() {
        let dir = scratch("census");
        let panes: Vec<uuid::Uuid> = (0..3).map(|_| uuid::Uuid::new_v4()).collect();
        for (n, pane) in panes.iter().enumerate() {
            crate::agent_memory::tests::write_session(&dir, *pane, n != 2);
        }
        let asleep = panes[1];
        let counted = awake_panes(&dir, &|pane| pane == asleep);
        assert_eq!(counted, [panes[0]]);
        // Recorded as running, but nothing runs it.
        assert_eq!(running_count(&counted), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
