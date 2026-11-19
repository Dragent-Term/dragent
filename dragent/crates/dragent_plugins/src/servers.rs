//! A plugin's servers as MCP server entries, one set per account.
//!
//! Each account gets its own entry for every server, named
//! `<plugin>-<server>` for `default` and `<plugin>-<server>-<account>` for
//! the others, so a tool call's name says which account it uses and none is
//! chosen silently. Entries start switched off: the person turns a server on
//! in Settings, and approves its tool calls there.

use std::path::Path;

use crate::manifest::{resolve_command, substitute, Manifest, Transport};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// The MCP entry's name.
    pub entry: String,
    pub server: String,
    pub account: String,
    pub transport: Transport,
    /// stdio: the program (resolved into the package when it ships one);
    /// http: the url.
    pub command: String,
    pub args: Vec<String>,
    /// Setting name, its default, and whether it's a secret.
    pub env: Vec<(String, Option<String>, bool)>,
}

/// The MCP entry name for `server` of `plugin` under `account`.
pub fn entry_name(plugin: &str, server: &str, account: &str) -> String {
    if account == "default" {
        format!("{plugin}-{server}")
    } else {
        format!("{plugin}-{server}-{account}")
    }
}

/// An account name: 1–20 lowercase letters, digits and hyphens.
pub fn valid_account(account: &str) -> bool {
    !account.is_empty()
        && account.len() <= 20
        && account
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Every entry the plugin installed at `package` needs on this system, for
/// `accounts`.
pub fn plan(manifest: &Manifest, package: &Path, accounts: &[String]) -> Vec<Planned> {
    plan_on(manifest, package, accounts, std::env::consts::OS)
}

/// [`plan`] for `system`: a server with no program there is left out.
pub fn plan_on(manifest: &Manifest, package: &Path, accounts: &[String], system: &str) -> Vec<Planned> {
    let mut planned = Vec::new();
    for account in accounts {
        for (name, server) in &manifest.servers {
            let (command, args) = match server.transport {
                Transport::Stdio => {
                    let Some((command, args)) = server.program_on(system) else {
                        continue;
                    };
                    (
                        resolve_command(command, package),
                        args.iter().map(|arg| substitute(arg, package)).collect(),
                    )
                }
                Transport::Http => (server.url.clone().unwrap_or_default(), Vec::new()),
            };
            planned.push(Planned {
                entry: entry_name(&manifest.id, name, account),
                server: name.clone(),
                account: account.clone(),
                transport: server.transport,
                command,
                args,
                env: server
                    .env
                    .iter()
                    .map(|(key, spec)| (key.clone(), spec.default.clone(), spec.secret))
                    .collect(),
            });
        }
    }
    planned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::tests::sample;

    #[test]
    fn every_account_gets_its_own_entries_naming_it() {
        let package = tempfile::tempdir().unwrap();
        sample(package.path(), "1.0.0", "x");
        let manifest = Manifest::read(package.path()).unwrap();
        let planned = plan(&manifest, package.path(), &["default".into(), "work".into()]);
        let names: Vec<&str> = planned.iter().map(|p| p.entry.as_str()).collect();
        assert_eq!(names, vec!["acme-tickets-tickets", "acme-tickets-tickets-work"]);
        assert_eq!(planned[0].command, package.path().join("bin/server").display().to_string());
        assert_eq!(planned[0].args, vec!["--stdio"]);
        assert!(planned[0].env.contains(&("ACME_TOKEN".into(), None, true)));
        assert!(planned[0].env.contains(&("ACME_SITE".into(), Some("acme.example".into()), false)));
        assert!(valid_account("work-2") && !valid_account("Work") && !valid_account(""));
    }

    #[test]
    fn each_system_runs_its_own_build_and_one_without_a_build_gets_no_entry() {
        let package = tempfile::tempdir().unwrap();
        sample(package.path(), "1.0.0", "x");
        let mut manifest = Manifest::read(package.path()).unwrap();
        let server = manifest.servers.get_mut("tickets").unwrap();
        server.command = None;
        server.platforms.insert(
            "windows".into(),
            crate::manifest::PlatformCommand { command: "./bin/server.exe".into(), args: None },
        );
        server.platforms.insert(
            "linux".into(),
            crate::manifest::PlatformCommand { command: "./bin/server-linux".into(), args: Some(vec!["--serve".into()]) },
        );
        let accounts = ["default".to_string()];
        let windows = plan_on(&manifest, package.path(), &accounts, "windows");
        assert_eq!(windows[0].command, package.path().join("bin/server.exe").display().to_string());
        assert_eq!(windows[0].args, vec!["--stdio"], "the server's own args when the system names none");
        let linux = plan_on(&manifest, package.path(), &accounts, "linux");
        assert_eq!(linux[0].args, vec!["--serve"]);
        assert!(plan_on(&manifest, package.path(), &accounts, "macos").is_empty());
        assert_eq!(manifest.servers["tickets"].systems(), vec!["windows", "linux"]);
    }
}
