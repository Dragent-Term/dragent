//! Settings → Usage for the phone (Dragent companion): what the coding
//! agents on this desktop used, read from their own records, and each Claude
//! Code and Codex account's plan limits. Same data as the desktop page; the
//! records never leave this desktop except inside the encrypted channel.
//!
//! Methods:
//! - `usage.summary {days, refresh?}` -> `{range, total, agents, models,
//!   projects, days, accounts}`
//! - `usage.use_account {provider, account}` -> `{accounts}`: the account new
//!   sessions of that agent use on this desktop.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_host::agent_usage::AccountUsage;
use dragent_host::token_usage::{self, Row, Totals};
use serde_json::{json, Map, Value};

use super::companion_host::Failure;
use super::desktop_catalog::AccountKey;

pub(crate) const METHODS: &[&str] = &["usage.summary", "usage.use_account"];

/// Reading every agent's records takes a moment: a phone flicking between
/// ranges reuses the last reading this long.
const ROWS_FRESH_FOR: Duration = Duration::from_secs(60);
const MAX_DAYS: i64 = 365;
const MAX_LISTED: usize = 20;

type Job = Box<dyn FnOnce() -> Result<Value, Failure> + Send>;

static ROWS: Mutex<Option<(Instant, Arc<Vec<Row>>)>> = Mutex::new(None);

fn rows(refresh: bool) -> Arc<Vec<Row>> {
    let mut cached = ROWS.lock().unwrap_or_else(|e| e.into_inner());
    if !refresh {
        if let Some((at, rows)) = cached.as_ref() {
            if at.elapsed() < ROWS_FRESH_FOR {
                return rows.clone();
            }
        }
    }
    let rows = Arc::new(token_usage::collect());
    *cached = Some((Instant::now(), rows.clone()));
    rows
}

/// "2026-11-16" for a day counted from 0001-01-01.
fn date(day: i32) -> Option<String> {
    chrono::NaiveDate::from_num_days_from_ce_opt(day).map(|date| date.format("%Y-%m-%d").to_string())
}

fn totals_json(totals: &Totals) -> Value {
    json!({
        "tokens": totals.tokens(),
        "input": totals.input,
        "output": totals.output,
        "cache_read": totals.cache_read,
        "cache_write": totals.cache_write,
        "replies": totals.replies,
        "cost": round_cents(totals.cost),
        "unpriced_tokens": totals.unpriced_tokens,
        "cache_savings": round_cents(totals.cache_savings),
    })
}

fn round_cents(amount: f64) -> f64 {
    if amount.is_finite() { (amount * 100.0).round() / 100.0 } else { 0.0 }
}

fn project_name(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().filter(|name| !name.is_empty()).unwrap_or(path).to_string()
}

pub(crate) fn accounts_json(accounts: &[AccountUsage]) -> Value {
    Value::Array(
        accounts
            .iter()
            .map(|account| {
                let usage = account.usage.as_ref();
                json!({
                    "provider": account.provider,
                    "account": account.account,
                    "label": account.label,
                    "default": account.default,
                    "plan": usage.and_then(|usage| usage.plan.clone()),
                    "updated_at": usage.map(|usage| usage.updated_at),
                    "windows": usage.map_or_else(Vec::new, |usage| usage
                        .windows
                        .iter()
                        .map(|window| json!({
                            "label": window.label,
                            "used_percent": window.used_percent.clamp(0.0, 100.0),
                            "resets_at": window.resets_at,
                        }))
                        .collect()),
                })
            })
            .collect(),
    )
}

/// The usage page for the last `days` days (1 = today).
pub(crate) fn summary_json(rows: &[Row], days: i32, accounts: &[AccountUsage]) -> Value {
    let summary = token_usage::summarise(rows, days);
    json!({
        "range": {"from": date(summary.from), "to": date(summary.to), "days": days},
        "total": totals_json(&summary.total),
        "agents": summary.agents.iter().map(|(agent, totals)| json!({
            "agent": agent, "totals": totals_json(totals),
        })).collect::<Vec<_>>(),
        "models": summary.models.iter().take(MAX_LISTED).map(|(agent, model, totals)| json!({
            "agent": agent, "model": model, "totals": totals_json(totals),
        })).collect::<Vec<_>>(),
        "projects": summary.projects.iter().take(MAX_LISTED).map(|(path, totals)| json!({
            "name": project_name(path), "path": path, "totals": totals_json(totals),
        })).collect::<Vec<_>>(),
        "days": summary.days.iter().filter_map(|(day, agents)| {
            let by_agent: Map<String, Value> = agents
                .iter()
                .map(|(agent, totals)| (agent.clone(), json!({"tokens": totals.tokens(), "cost": round_cents(totals.cost)})))
                .collect();
            Some(json!({"date": date(*day)?, "agents": by_agent}))
        }).collect::<Vec<_>>(),
        "accounts": accounts_json(accounts),
    })
}

fn account_id(value: Option<&Value>) -> Result<String, Failure> {
    match value {
        Some(Value::String(id))
            if !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
        {
            Ok(id.clone())
        }
        _ => Err(Failure::Invalid),
    }
}

/// Check a request on the socket thread and return the work to run off it.
pub(crate) fn job(_expected: &AccountKey, method: &str, params: &Value) -> Result<Job, Failure> {
    let object = params.as_object().ok_or(Failure::Invalid)?;
    match method {
        "usage.summary" => {
            let days = match object.get("days") {
                Some(Value::Number(n)) => n.as_i64().filter(|days| (1..=MAX_DAYS).contains(days)).ok_or(Failure::Invalid)?,
                None => 7,
                _ => return Err(Failure::Invalid),
            } as i32;
            let refresh = matches!(object.get("refresh"), Some(Value::Bool(true)));
            Ok(Box::new(move || {
                let rows = rows(refresh);
                let accounts = dragent_host::agent_usage::accounts();
                Ok(summary_json(&rows, days, &accounts))
            }))
        }
        "usage.use_account" => {
            let provider = match object.get("provider").and_then(Value::as_str) {
                Some(provider @ ("claude" | "codex")) => provider.to_string(),
                _ => return Err(Failure::Invalid),
            };
            let account = account_id(object.get("account"))?;
            Ok(Box::new(move || {
                let known = dragent_host::agent_usage::accounts()
                    .into_iter()
                    .any(|row| row.provider == provider && row.account == account);
                if !known {
                    return Err(Failure::Gone);
                }
                dragent_host::agent_usage::set_default(&provider, &account).map_err(|_| Failure::Unavailable)?;
                Ok(json!({"accounts": accounts_json(&dragent_host::agent_usage::accounts())}))
            }))
        }
        _ => Err(Failure::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_host::agent_usage::{Usage, Window};

    fn row(agent: &str, day: i32, project: Option<&str>, input: u64) -> Row {
        Row {
            agent: agent.into(),
            model: "claude-sonnet-4-5".into(),
            day,
            project: project.map(Into::into),
            input,
            output: 10,
            replies: 1,
            ..Row::default()
        }
    }

    #[test]
    fn the_summary_has_dates_agents_projects_and_accounts() {
        let today = token_usage::today();
        let rows = vec![
            row("claude", today, Some("/Users/me/dev/app/"), 100),
            row("codex", today - 1, None, 50),
            row("claude", today - 30, None, 999),
        ];
        let accounts = vec![AccountUsage {
            provider: "claude".into(),
            account: "system".into(),
            label: "System login".into(),
            default: true,
            usage: Some(Usage {
                provider: "claude".into(),
                account: "system".into(),
                windows: vec![Window { label: "5-hour".into(), used_percent: 140.0, resets_at: Some(1) }],
                plan: Some("max".into()),
                updated_at: 2,
            }),
        }];
        let value = summary_json(&rows, 7, &accounts);
        assert_eq!(value["range"]["days"], 7);
        assert_eq!(value["days"].as_array().unwrap().len(), 7);
        assert_eq!(value["total"]["input"], 150, "the 30-day-old row is outside the range");
        assert_eq!(value["agents"][0]["agent"], "claude");
        assert_eq!(value["projects"][0]["name"], "app");
        assert_eq!(value["accounts"][0]["windows"][0]["used_percent"], 100.0, "clamped");
        assert_eq!(value["accounts"][0]["plan"], "max");
    }

    #[test]
    fn requests_are_checked() {
        let key = AccountKey::new("u", "o");
        assert!(job(&key, "usage.summary", &json!({"days": 0})).is_err());
        assert!(job(&key, "usage.summary", &json!({"days": 400})).is_err());
        assert!(job(&key, "usage.summary", &json!({"days": 30})).is_ok());
        assert!(job(&key, "usage.use_account", &json!({"provider": "gemini", "account": "system"})).is_err());
        assert!(job(&key, "usage.use_account", &json!({"provider": "codex", "account": "../x"})).is_err());
        assert!(job(&key, "usage.use_account", &json!({"provider": "codex", "account": "system"})).is_ok());
    }
}
