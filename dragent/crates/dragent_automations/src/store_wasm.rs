//! The web viewer runs no automations: reads are empty, writes refuse.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Result};

use crate::model::{
    Automation, PromptSource, PromptVersion, Run, RunFilter, RunProgress, RunStatus, RunWorkspace,
};

pub struct Store;

impl Store {
    pub fn open(_path: &Path) -> Result<Self> {
        bail!("automations are unavailable in the web viewer")
    }
    pub fn open_in_memory() -> Result<Self> {
        bail!("automations are unavailable in the web viewer")
    }
    pub fn list_automations(&self) -> Result<Vec<Automation>> {
        Ok(Vec::new())
    }
    pub fn get_automation(&self, _id: &str) -> Result<Option<Automation>> {
        Ok(None)
    }
    pub fn save_automation(&self, _automation: &Automation) -> Result<()> {
        bail!("automations are unavailable in the web viewer")
    }
    pub fn delete_automation(&self, _id: &str) -> Result<()> {
        Ok(())
    }
    pub fn insert_run(&self, _run: Run) -> Result<Option<Run>> {
        Ok(None)
    }
    pub fn delete_run(&self, _id: &str) -> Result<()> {
        Ok(())
    }
    pub fn get_run(&self, _id: &str) -> Result<Option<Run>> {
        Ok(None)
    }
    pub fn transition(
        &self,
        _id: &str,
        _from: &[RunStatus],
        _change: impl FnOnce(&mut Run),
    ) -> Result<Option<Run>> {
        Ok(None)
    }
    pub fn runs_in_status(&self, _statuses: &[RunStatus]) -> Result<Vec<Run>> {
        Ok(Vec::new())
    }
    pub fn scheduled_run(&self, _automation_id: &str) -> Result<Option<Run>> {
        Ok(None)
    }
    pub fn due_runs(&self, _now_ms: i64, _limit: usize) -> Result<Vec<Run>> {
        Ok(Vec::new())
    }
    pub fn last_planned_at(&self, _automation_id: &str) -> Result<Option<i64>> {
        Ok(None)
    }
    pub fn latest_run(&self, _automation_id: &str) -> Result<Option<Run>> {
        Ok(None)
    }
    pub fn list_runs(
        &self,
        _automation_id: &str,
        _filter: RunFilter,
        _before_seq: Option<i64>,
        _limit: usize,
    ) -> Result<Vec<Run>> {
        Ok(Vec::new())
    }
    pub fn run_counts(&self, _automation_id: &str) -> Result<HashMap<RunFilter, usize>> {
        Ok(HashMap::new())
    }
    pub fn record_run_workspace(&self, _workspace: &RunWorkspace) -> Result<()> {
        Ok(())
    }
    pub fn run_workspaces(&self) -> Result<Vec<RunWorkspace>> {
        Ok(Vec::new())
    }
    pub fn data_version(&self) -> i64 {
        0
    }
    pub fn add_prompt_version(
        &self,
        _automation_id: &str,
        _prompt: &str,
        _source: PromptSource,
        _now_ms: i64,
    ) -> Result<Option<PromptVersion>> {
        Ok(None)
    }
    pub fn prompt_versions(&self, _automation_id: &str) -> Result<Vec<PromptVersion>> {
        Ok(Vec::new())
    }
    pub fn prompt_version(&self, _id: i64) -> Result<Option<PromptVersion>> {
        Ok(None)
    }
    pub fn copy_patterns(&self, _project_root: &Path) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    pub fn set_copy_patterns(&self, _project_root: &Path, _patterns: &[String]) -> Result<()> {
        Ok(())
    }
    pub fn meta(&self, _key: &str) -> Result<Option<String>> {
        Ok(None)
    }
    pub fn set_meta(&self, _key: &str, _value: &str) -> Result<()> {
        Ok(())
    }
    pub fn latest_runs(&self) -> Result<HashMap<String, Run>> {
        Ok(HashMap::new())
    }
    pub fn finished_since(&self, _since_ms: i64) -> Result<Vec<Run>> {
        Ok(Vec::new())
    }
    pub fn started_runs(&self, _automation_id: &str, _limit: usize) -> Result<Vec<Run>> {
        Ok(Vec::new())
    }
    pub fn followed_runs(&self, _since_ms: i64) -> Result<Vec<Run>> {
        Ok(Vec::new())
    }
    pub fn run_in_pane(&self, _pane: &str) -> Result<Option<Run>> {
        Ok(None)
    }
    pub fn run_in_terminal(&self, _session_id: &str) -> Result<Option<Run>> {
        Ok(None)
    }
    pub fn set_progress(&self, _id: &str, _progress: RunProgress) -> Result<Option<Run>> {
        bail!("automations are unavailable in the web viewer")
    }
    pub fn set_pane(&self, _id: &str, _pane: &str) -> Result<Option<Run>> {
        bail!("automations are unavailable in the web viewer")
    }
    pub fn prune_runs(&self, _automation_id: &str, _keep: usize) -> Result<usize> {
        Ok(0)
    }
    pub fn runs_matching(&self, _id: &str) -> Result<Vec<Run>> {
        Ok(Vec::new())
    }
}
