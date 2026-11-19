//! Concurrency capacity for agent runs (replaces the placeholder).
//!
//! Two-level gate: per-workspace (one noisy project cannot starve the
//! machine) and per-host (fork bombs and billing runaway stop here).
//! Counts are LIVE runs only — exited sessions release their slots
//! (the caller supplies liveness; this module only decides).
//! Rejections name the limit and the remedy (close/stop runs).

/// Default caps: 4 live runs per workspace, 16 per host.
pub const DEFAULT_MAX_RUNS_PER_WORKSPACE: usize = 4;
pub const DEFAULT_MAX_RUNS_PER_HOST: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct CapacityLimits {
    pub max_runs_per_workspace: usize,
    pub max_runs_per_host: usize,
}

impl Default for CapacityLimits {
    fn default() -> Self {
        Self {
            max_runs_per_workspace: DEFAULT_MAX_RUNS_PER_WORKSPACE,
            max_runs_per_host: DEFAULT_MAX_RUNS_PER_HOST,
        }
    }
}

/// Admit one more run, or refuse with an actionable error.
pub fn check_capacity(
    workspace_live_runs: usize,
    host_live_runs: usize,
    limits: CapacityLimits,
) -> Result<(), String> {
    if workspace_live_runs >= limits.max_runs_per_workspace {
        return Err(format!(
            "workspace run capacity reached ({}/{}) — stop or close a run first",
            workspace_live_runs, limits.max_runs_per_workspace
        ));
    }
    if host_live_runs >= limits.max_runs_per_host {
        return Err(format!(
            "host run capacity reached ({}/{}) — stop or close a run first",
            host_live_runs, limits.max_runs_per_host
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_below_both_caps() {
        assert!(check_capacity(0, 0, CapacityLimits::default()).is_ok());
        assert!(check_capacity(3, 15, CapacityLimits::default()).is_ok());
    }

    #[test]
    fn refuses_at_either_cap_with_actionable_errors() {
        let err = check_capacity(4, 0, CapacityLimits::default()).unwrap_err();
        assert!(err.contains("workspace") && err.contains("stop"), "{err}");
        let err = check_capacity(0, 16, CapacityLimits::default()).unwrap_err();
        assert!(err.contains("host") && err.contains("stop"), "{err}");
    }

    #[test]
    fn custom_limits_apply() {
        let limits = CapacityLimits {
            max_runs_per_workspace: 1,
            max_runs_per_host: 100,
        };
        assert!(check_capacity(0, 0, limits).is_ok());
        assert!(check_capacity(1, 0, limits).is_err());
    }
}
