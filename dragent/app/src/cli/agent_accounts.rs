//! Machine-local provider account management; unrelated to `dragent whoami`.
use dragent_host::agent_accounts::{self, Registry, Selection};
use super::{Flags, absolute, listing, pretty, text};

pub(super) fn run(flags: &Flags) -> Result<String, String> {
    let operation = flags.argument(0).unwrap_or("list");
    let provider = flags.value("agent").map(super::agent_key).transpose()?;
    let id = || flags.argument(1).ok_or("Specify an account ID")?.parse::<uuid::Uuid>().map_err(|_| "Invalid account ID".to_string());
    let result = match operation {
        "list" => {
            let registry = Registry::load()?;
            let rows = registry.profiles.iter().filter(|p| provider.as_ref().is_none_or(|key| &p.provider == key)).map(|p| serde_json::json!({"id":p.id,"agent":p.provider,"name":p.label,"machine_id":p.machine_id,"status":"Not checked"})).collect();
            return Ok(listing(flags, rows, |r| text(r,"id"), |r| format!("{}\t{}\t{}", text(r,"id"),text(r,"agent"),text(r,"name")), "No provider accounts yet. Use agents accounts add --agent KEY --name NAME."));
        }
        "capabilities" => {
            let settings = crate::setup::UiSettings::load();
            let rows = crate::workspace::all_definitions(&settings.custom_agents, false).into_iter().filter(|d| provider.as_ref().is_none_or(|p| p == &d.id)).map(|d| serde_json::json!({"agent":d.id,"name":d.name,"capabilities":agent_accounts::capability(&d.id)})).collect();
            return Ok(listing(flags, rows, |r| text(r,"agent"), |r| format!("{}\t{}",text(r,"agent"),r["capabilities"]["reason"].as_str().unwrap_or_default()), "No agents available."));
        }
        "add" => {
            let provider = provider.as_deref().ok_or("Specify --agent KEY")?;
            let name = flags.value("name").ok_or("Specify --name NAME")?;
            serde_json::to_value(Registry::update(|r| r.add(provider, name))?).map_err(|_| "Could not serialize account")?
        }
        "rename" => { Registry::update(|r| r.rename(id()?, flags.value("name").ok_or("Specify --name NAME")?))?; serde_json::json!({"renamed":true}) }
        "remove" => { Registry::update(|r| r.remove(id()?))?; serde_json::json!({"removed":true,"note":"Native credentials and history retained. Existing bindings and defaults will refuse future launches."}) }
        "default" => {
            let provider = provider.as_deref().ok_or("Specify --agent KEY")?;
            let selection = Selection::parse(flags.value("account").ok_or("Specify --account ID|system|inherit")?)?;
            let project = flags.value("project").map(absolute);
            Registry::update(|r| r.set_default(provider, project.as_deref(), selection))?;
            serde_json::json!({"saved":true,"scope":if project.is_some(){"project"}else{"app"}})
        }
        "status" | "login" => {
            let id = id()?;
            let registry = Registry::load()?;
            let profile = registry.profiles.iter().find(|p| p.id == id).ok_or("Account no longer exists")?;
            let binding = registry.resolve(&profile.provider,None,&Selection::Profile(id))?;
            if operation == "login" {
                if flags.json { return Err("Native login is interactive; rerun with --text".into()); }
                let code = agent_accounts::login(id)?;
                if code != 0 { return Err("Native sign-in did not complete".into()); }
            }
            serde_json::json!({"id":id,"name":profile.label,"status":agent_accounts::status(&binding,std::path::Path::new(&profile.provider))?})
        }
        _ => return Err("Use agents accounts list, capabilities, add, rename, remove, default, login or status".into()),
    };
    if flags.json { Ok(pretty(result)) } else if flags.quiet { Ok(result.get("id").and_then(|v|v.as_str()).unwrap_or("ok").into()) } else { Ok(pretty(result)) }
}
