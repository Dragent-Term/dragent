//! The organisation's automation registry on the server.
//!
//! Automations run on their owner's machine; each one is mirrored to the
//! server so teammates see it in the Team tab and so the free plan's
//! allowance counts every machine the account uses. Syncing is
//! incremental: a payload is only sent when it differs from the last one
//! the server accepted, and deletions queue until they go through.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use dragent_automations::{Automation, Schedule, Store};
use serde::Deserialize;

/// An automation as the registry lists it (any member's).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TeamAutomation {
    pub id: String,
    pub owner_user_id: String,
    #[serde(default)]
    pub owner_name: String,
    #[serde(default)]
    pub owner_email: String,
    pub name: String,
    #[serde(default)]
    pub prompt: String,
    pub schedule: String,
    #[serde(default)]
    pub schedule_label: String,
    #[serde(default)]
    pub timezone: String,
    pub agent: String,
    #[serde(default)]
    pub project_label: Option<String>,
    pub enabled: bool,
    #[serde(default)]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub last_run_status: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub created_at: i64,
}

/// The signed-in account's standing against its plan.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Allowance {
    /// `None` on a paid plan: unlimited.
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub owned: usize,
    #[serde(default)]
    pub tier: String,
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    automations: Vec<TeamAutomation>,
    #[serde(default)]
    allowance: Allowance,
}

fn base(org: &str) -> String {
    format!("/api/v1/organizations/{org}/automations")
}

/// Every automation in the organisation, plus this account's allowance.
pub(crate) fn fetch(org: &str) -> Result<(Vec<TeamAutomation>, Allowance), String> {
    let value = crate::api::API_CLIENT
        .get(&base(org), 20)
        .map_err(|error| error.user_message())?;
    let listed: ListResponse = serde_json::from_value(value)
        .map_err(|error| format!("Unexpected automations response: {error}"))?;
    Ok((listed.automations, listed.allowance))
}

/// Label shown to teammates for where an automation works.
fn project_label(automation: &Automation) -> Option<String> {
    (!automation.is_session()).then(|| {
        automation
            .project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| automation.project_root.display().to_string())
    })
}

/// What the registry stores for one of this account's automations.
pub(crate) fn payload(automation: &Automation, store: &Store) -> serde_json::Value {
    let next_run = store
        .scheduled_run(&automation.id)
        .ok()
        .flatten()
        .and_then(|run| run.scheduled_at_ms);
    let last = store.latest_run(&automation.id).ok().flatten();
    serde_json::json!({
        "name": automation.name,
        "prompt": automation.prompt,
        "schedule": automation.schedule,
        "schedule_label": match &automation.watch {
            Some(watch) => super::watch::describe(watch, &automation.schedule),
            None => Schedule::parse(&automation.schedule)
                .map(|schedule| schedule.describe())
                .unwrap_or_default(),
        },
        "timezone": automation.timezone,
        "agent": automation.agent,
        "project_label": project_label(automation),
        "enabled": automation.enabled,
        "next_run_at": next_run,
        "last_run_status": last.as_ref().map(|run| run.status.as_str()),
        "last_run_at": last.as_ref().and_then(|run| run.display_time_ms()),
        "created_at": automation.created_at_ms,
    })
}

fn fingerprint(value: &serde_json::Value) -> String {
    let mut hasher = DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

const PENDING_DELETES: &str = "sync.pending_deletes";

fn sync_key(id: &str) -> String {
    format!("sync.{id}")
}

/// Queue a deleted automation's removal from the registry.
pub(crate) fn queue_delete(store: &Store, id: &str) {
    let mut pending: Vec<String> = store
        .meta(PENDING_DELETES)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    if !pending.iter().any(|queued| queued == id) {
        pending.push(id.to_string());
    }
    let _ = store.set_meta(PENDING_DELETES, &serde_json::to_string(&pending).unwrap_or_default());
}

/// Outcome of one sync pass.
#[derive(Debug, Default)]
pub(crate) struct SyncReport {
    pub sent: usize,
    /// Refused because the free plan's allowance is used up.
    pub over_limit: Vec<String>,
    pub error: Option<String>,
}

/// Send what changed since the last pass. Stops at the first network
/// failure and tries again next time.
pub(crate) fn sync(org: &str, store: &Store, owned: &[Automation]) -> SyncReport {
    let api = &crate::api::API_CLIENT;
    let mut report = SyncReport::default();

    let pending: Vec<String> = store
        .meta(PENDING_DELETES)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let mut remaining = Vec::new();
    for id in pending {
        if report.error.is_some() {
            remaining.push(id);
            continue;
        }
        match api.delete(&format!("{}/{id}", base(org)), 20) {
            Ok(_) | Err(crate::api::ApiError::Server { status: 404 | 403, .. }) => {
                let _ = store.set_meta(&sync_key(&id), "");
            }
            Err(error) => {
                report.error = Some(error.to_string());
                remaining.push(id);
            }
        }
    }
    let _ = store.set_meta(PENDING_DELETES, &serde_json::to_string(&remaining).unwrap_or_default());

    for automation in owned {
        if report.error.is_some() {
            break;
        }
        let body = payload(automation, store);
        let print = fingerprint(&body);
        if store.meta(&sync_key(&automation.id)).ok().flatten().as_deref() == Some(print.as_str()) {
            continue;
        }
        match api.put(&format!("{}/{}", base(org), automation.id), &body, 20) {
            Ok(_) => {
                let _ = store.set_meta(&sync_key(&automation.id), &print);
                report.sent += 1;
            }
            Err(crate::api::ApiError::Server { status: 402, .. }) => {
                // Remembered so the same payload isn't re-sent every pass.
                let _ = store.set_meta(&sync_key(&automation.id), &print);
                report.over_limit.push(automation.id.clone());
            }
            Err(crate::api::ApiError::Server { status, body }) if (400..500).contains(&status) => {
                log::warn!("[automations] registry refused {}: {status} {body}", automation.id);
                let _ = store.set_meta(&sync_key(&automation.id), &print);
            }
            Err(error) => report.error = Some(error.to_string()),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_automations::WorkspaceMode;
    use std::path::PathBuf;

    #[test]
    fn payload_labels_projects_and_sessions() {
        let store = Store::open_in_memory().unwrap();
        let mut automation = Automation::new(
            "Digest",
            PathBuf::from("/code/api"),
            "0 9 * * *",
            "claude",
            "Summarise",
            WorkspaceMode::default(),
            7,
        );
        let body = payload(&automation, &store);
        assert_eq!(body["project_label"], "api");
        assert_eq!(body["created_at"], 7);
        assert!(body["schedule_label"].as_str().unwrap().contains("9:00"));
        automation.project_root = PathBuf::new();
        assert!(payload(&automation, &store)["project_label"].is_null());
        // The fingerprint only changes with the payload.
        let body = payload(&automation, &store);
        assert_eq!(fingerprint(&body), fingerprint(&payload(&automation, &store)));
        automation.enabled = false;
        assert_ne!(fingerprint(&body), fingerprint(&payload(&automation, &store)));
    }

    #[test]
    fn deletions_queue_once() {
        let store = Store::open_in_memory().unwrap();
        queue_delete(&store, "a");
        queue_delete(&store, "a");
        queue_delete(&store, "b");
        let pending: Vec<String> =
            serde_json::from_str(&store.meta(PENDING_DELETES).unwrap().unwrap()).unwrap();
        assert_eq!(pending, vec!["a".to_string(), "b".to_string()]);
    }
}
