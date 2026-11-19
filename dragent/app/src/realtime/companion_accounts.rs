//! The desktop's agent accounts for the phone (Dragent companion): each
//! Claude Code and Codex account on this desktop, whether it's signed in,
//! its plan limits, and which one new sessions use; add, rename, remove and
//! choose one. Credentials stay with each agent's own CLI on this desktop;
//! signing in runs the agent's own login in a desktop terminal the phone
//! opens (`dragent agents accounts login <id>`).
//!
//! Methods:
//! - `accounts.list {}` -> `{agents:[{provider, name, supported, reason, accounts:[…]}]}`
//! - `accounts.add {provider, label}` -> `{account, agents}`
//! - `accounts.rename {account, label}` -> `{agents}`
//! - `accounts.remove {account}` -> `{agents}`
//! - `accounts.use {provider, account}` -> `{agents}`

use dragent_host::agent_accounts::{self, Registry, Selection};
use dragent_host::agent_usage::{self, AccountUsage};
use serde_json::{json, Map, Value};

use super::companion_host::Failure;
use super::desktop_catalog::AccountKey;

pub(crate) const METHODS: &[&str] = &["accounts.list", "accounts.add", "accounts.rename", "accounts.remove", "accounts.use"];

/// The agents whose accounts can be kept apart on one desktop.
const PROVIDERS: &[(&str, &str)] = &[("claude", "Claude Code"), ("codex", "Codex")];

type Job = Box<dyn FnOnce() -> Result<Value, Failure> + Send>;

fn text(object: &Map<String, Value>, key: &str, max: usize) -> Result<String, Failure> {
    match object.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control) => {
            Ok(value.trim().to_string())
        }
        _ => Err(Failure::Invalid),
    }
}

fn only(object: &Map<String, Value>, keys: &[&str]) -> Result<(), Failure> {
    if object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(Failure::Invalid);
    }
    Ok(())
}

fn provider(object: &Map<String, Value>) -> Result<String, Failure> {
    let key = text(object, "provider", 16)?;
    PROVIDERS.iter().any(|(id, _)| *id == key).then_some(key).ok_or(Failure::Invalid)
}

fn profile_id(object: &Map<String, Value>) -> Result<uuid::Uuid, Failure> {
    text(object, "account", 64)?.parse().map_err(|_| Failure::Invalid)
}

/// "Signed in", "Sign-in required", or why it couldn't be checked.
fn status(registry: &Registry, provider: &str, selection: &Selection) -> String {
    let program = dragent_host::host_tools::command(provider).get_program().to_owned();
    match registry.resolve(provider, None, selection) {
        Ok(binding) => agent_accounts::status(&binding, std::path::Path::new(&program)).map_or_else(|error| error, str::to_string),
        Err(error) => error,
    }
}

fn windows(usage: Option<&AccountUsage>) -> (Option<String>, Vec<Value>) {
    let Some(reading) = usage.and_then(|row| row.usage.as_ref()) else { return (None, Vec::new()) };
    let windows = reading
        .windows
        .iter()
        .map(|window| json!({"label": window.label, "used_percent": window.used_percent.clamp(0.0, 100.0), "resets_at": window.resets_at}))
        .collect();
    (reading.plan.clone(), windows)
}

/// Every account of the agents that support several, as the phone shows them.
pub(crate) fn agents_json() -> Value {
    let registry = Registry::load().unwrap_or_default();
    let usage = agent_usage::accounts();
    let agents: Vec<Value> = PROVIDERS
        .iter()
        .map(|(provider, name)| {
            let capability = agent_accounts::capability(provider);
            let in_use = usage.iter().find(|row| row.provider == *provider && row.default).map(|row| row.account.clone());
            let mut accounts = vec![("system".to_string(), "System login".to_string(), Selection::System)];
            accounts.extend(
                registry
                    .profiles
                    .iter()
                    .filter(|profile| profile.provider == *provider)
                    .map(|profile| (profile.id.to_string(), profile.label.clone(), Selection::Profile(profile.id))),
            );
            let rows: Vec<Value> = accounts
                .into_iter()
                .map(|(account, label, selection)| {
                    let reading = usage.iter().find(|row| row.provider == *provider && row.account == account);
                    let (plan, windows) = windows(reading);
                    let system = account == "system";
                    json!({
                        "account": account,
                        "label": label,
                        "system": system,
                        "in_use": in_use.as_deref() == Some(account.as_str()) || (in_use.is_none() && system),
                        "status": status(&registry, provider, &selection),
                        "plan": plan,
                        "windows": windows,
                    })
                })
                .collect();
            json!({
                "provider": provider,
                "name": name,
                "supported": capability.isolated,
                "reason": capability.reason,
                "accounts": rows,
            })
        })
        .collect();
    json!({"agents": agents})
}

fn changed(result: Result<(), String>) -> Result<Value, Failure> {
    result.map_err(|error| {
        log::info!("[companion] account change refused: {error}");
        Failure::Unavailable
    })?;
    Ok(agents_json())
}

/// Check a request on the socket thread and return the work to run off it.
pub(crate) fn job(_expected: &AccountKey, method: &str, params: &Value) -> Result<Job, Failure> {
    let object = params.as_object().ok_or(Failure::Invalid)?;
    match method {
        "accounts.list" => {
            only(object, &[])?;
            Ok(Box::new(|| Ok(agents_json())))
        }
        "accounts.add" => {
            only(object, &["provider", "label"])?;
            let provider = provider(object)?;
            let label = text(object, "label", 64)?;
            Ok(Box::new(move || {
                let profile = Registry::update(|registry| registry.add(&provider, &label)).map_err(|_| Failure::Unavailable)?;
                let mut value = agents_json();
                value["account"] = json!(profile.id.to_string());
                Ok(value)
            }))
        }
        "accounts.rename" => {
            only(object, &["account", "label"])?;
            let id = profile_id(object)?;
            let label = text(object, "label", 64)?;
            Ok(Box::new(move || changed(Registry::update(|registry| registry.rename(id, &label)))))
        }
        "accounts.remove" => {
            only(object, &["account"])?;
            let id = profile_id(object)?;
            Ok(Box::new(move || {
                changed(Registry::update(|registry| {
                    let provider = registry.profiles.iter().find(|p| p.id == id).map(|p| p.provider.clone()).ok_or("Account no longer exists")?;
                    // New sessions mustn't be left pointing at a removed
                    // account: they go back to the system login.
                    if registry.defaults.get(&provider) == Some(&Selection::Profile(id)) {
                        registry.set_default(&provider, None, Selection::System)?;
                    }
                    registry.remove(id)
                }))
            }))
        }
        "accounts.use" => {
            only(object, &["provider", "account"])?;
            let provider = provider(object)?;
            let account = text(object, "account", 64)?;
            if account != "system" && account.parse::<uuid::Uuid>().is_err() {
                return Err(Failure::Invalid);
            }
            Ok(Box::new(move || changed(agent_usage::set_default(&provider, &account))))
        }
        _ => Err(Failure::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_checked() {
        let key = AccountKey::new("u", "o");
        assert!(job(&key, "accounts.list", &json!({})).is_ok());
        assert!(job(&key, "accounts.list", &json!({"x": 1})).is_err());
        assert!(job(&key, "accounts.add", &json!({"provider": "gemini", "label": "Work"})).is_err(), "only agents with separate accounts");
        assert!(job(&key, "accounts.add", &json!({"provider": "claude", "label": " "})).is_err());
        assert!(job(&key, "accounts.add", &json!({"provider": "claude", "label": "Work"})).is_ok());
        assert!(job(&key, "accounts.rename", &json!({"account": "system", "label": "x"})).is_err(), "the system login can't be renamed");
        assert!(job(&key, "accounts.use", &json!({"provider": "codex", "account": "../x"})).is_err());
        assert!(job(&key, "accounts.use", &json!({"provider": "codex", "account": "system"})).is_ok());
    }
}
