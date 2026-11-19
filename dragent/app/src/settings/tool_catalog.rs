//! What Settings → Machines knows about each tool and agent it shows: its
//! name, how to read its version, its install guide, and the ways to
//! install, update and remove it on a machine (macOS or Linux). Commands are
//! built here for the machine's own OS and run in the install sheet, a
//! terminal on that machine.

use crate::workspace::PromptAgent;
use dragent_host::development::{ToolQuery, ToolReport};
use dragent_host::host_tools::{self, PackageAction};
use dragent_host::remote::quote;

pub use host_tools::SYSTEM_TOOLS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
}

impl Os {
    /// From the runtime's `std::env::consts::OS`.
    pub fn of(name: &str) -> Os {
        if name == "macos" { Os::Mac } else { Os::Linux }
    }
}

/// One way to install a tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    pub label: &'static str,
    pub recommended: bool,
    pub install: String,
    pub update: Option<String>,
    pub uninstall: Option<String>,
    /// Whether a permission failure may be retried with `sudo` (package
    /// managers and npm; user-level installers must not run as root).
    pub may_elevate: bool,
    /// The install sources ([`dragent_host::development::ToolCopy::source`])
    /// this method manages: its update and uninstall apply to those copies.
    pub sources: &'static [&'static str],
}

/// Sources npm manages, whichever Node.js runs it.
const NPM_SOURCES: &[&str] = &["npm", "nvm", "Volta", "asdf", "mise", "fnm"];

fn agent(tool: &str) -> Option<PromptAgent> {
    PromptAgent::ALL
        .into_iter()
        .find(|agent| *agent != PromptAgent::Dragent && agent.cli() == Some(tool))
}

pub fn is_agent(tool: &str) -> bool {
    agent(tool).is_some()
}

/// The name people know a tool by.
pub fn label(tool: &str) -> String {
    match agent(tool) {
        Some(agent) => agent.label().to_string(),
        None => match tool {
            "rg" => "ripgrep".into(),
            "node" => "Node.js".into(),
            "python3" => "Python".into(),
            "go" => "Go".into(),
            "cargo" => "Rust (cargo)".into(),
            "docker" => "Docker".into(),
            "git" => "Git".into(),
            "gh" => "GitHub CLI".into(),
            "glab" => "GitLab CLI".into(),
            other => other.to_string(),
        },
    }
}

/// What prints the tool's version.
pub fn query(tool: &str) -> ToolQuery {
    let args: &[&str] = match tool {
        "go" => &["version"],
        _ => &["--version"],
    };
    ToolQuery {
        name: tool.to_string(),
        version_args: args.iter().map(|arg| arg.to_string()).collect(),
    }
}

/// The tool's own command that updates it in place, whatever installed it.
pub fn self_update(tool: &str) -> Option<&'static [&'static str]> {
    match tool {
        "claude" => Some(&["update"]),
        "cursor-agent" => Some(&["update"]),
        "goose" => Some(&["update"]),
        "opencode" => Some(&["upgrade"]),
        _ => None,
    }
}

/// The npm package an agent installs from, when it installs with npm.
fn npm_package(tool: &str) -> Option<&'static str> {
    agent(tool)?.unix_install_command()?.strip_prefix("npm install -g ")
}

/// Where to read how to install the tool by hand.
pub fn docs(tool: &str) -> Option<String> {
    let fixed = match tool {
        "git" => "https://git-scm.com/downloads",
        "rg" => "https://github.com/BurntSushi/ripgrep#installation",
        "node" | "npm" => "https://nodejs.org/en/download",
        "python3" => "https://www.python.org/downloads/",
        "go" => "https://go.dev/doc/install",
        "cargo" => "https://rustup.rs",
        "docker" => "https://docs.docker.com/engine/install/",
        "gh" => "https://cli.github.com/",
        "glab" => "https://docs.gitlab.com/cli/",
        "claude" => "https://docs.anthropic.com/en/docs/claude-code/setup",
        _ => return npm_package(tool).map(|package| format!("https://www.npmjs.com/package/{package}")),
    };
    Some(fixed.to_string())
}

/// Homebrew installs Dragent offers for agents on a Mac: (package, cask).
fn brew_agent(tool: &str) -> Option<(&'static str, bool)> {
    match tool {
        "claude" => Some(("claude-code", true)),
        "codex" => Some(("codex", false)),
        "gemini" => Some(("gemini-cli", false)),
        _ => None,
    }
}

fn sh(script: &str) -> String {
    format!("sh -c {}", quote(script))
}

/// The ways to install `tool` on a machine running `os`, recommended first.
pub fn methods(tool: &str, os: Os) -> Vec<Method> {
    let mac = os == Os::Mac;
    if host_tools::uses_package_manager(tool) {
        let run = |action| host_tools::package_command(action, &[tool], mac).ok();
        // Removing git or the system Python breaks too much else.
        let removable = !matches!(tool, "git" | "python3");
        return vec![Method {
            label: if mac { "Homebrew" } else { "Package manager" },
            recommended: true,
            install: run(PackageAction::Install).unwrap_or_default(),
            update: run(PackageAction::Update),
            uninstall: removable.then(|| run(PackageAction::Remove)).flatten(),
            may_elevate: !mac,
            sources: if mac { &["Homebrew"] } else { &["System", "Homebrew"] },
        }];
    }
    if tool == "cargo" {
        return vec![Method {
            label: "rustup",
            recommended: true,
            install: host_tools::rustup_command(PackageAction::Install),
            update: Some(host_tools::rustup_command(PackageAction::Update)),
            uninstall: Some(host_tools::rustup_command(PackageAction::Remove)),
            may_elevate: false,
            sources: &["Cargo", "rustup"],
        }];
    }
    let Some(command) = agent(tool).and_then(PromptAgent::unix_install_command) else {
        return Vec::new();
    };
    let mut methods = Vec::new();
    if let Some(package) = command.strip_prefix("npm install -g ").filter(|p| !p.contains(' ')) {
        methods.push(Method {
            label: "npm",
            recommended: true,
            install: sh(&host_tools::npm_global_install(package)),
            update: Some(sh(&host_tools::npm_global_install(&format!("{package}@latest")))),
            uninstall: Some(sh(&host_tools::npm_global_uninstall(package))),
            may_elevate: true,
            sources: NPM_SOURCES,
        });
    } else if command.contains("install") || command.contains("curl") {
        // The vendor's own installer, which installs the latest version
        // again when re-run.
        methods.push(Method {
            label: "Official installer",
            recommended: true,
            install: command.to_string(),
            update: Some(command.to_string()),
            uninstall: None,
            may_elevate: false,
            sources: &["Installer", "Manual"],
        });
    }
    if mac {
        if let Some((package, cask)) = brew_agent(tool) {
            let packages = if cask { format!("--cask {package}") } else { package.to_string() };
            let brew = |action| host_tools::brew_command(action, tool, &packages);
            methods.push(Method {
                label: "Homebrew",
                recommended: methods.is_empty(),
                install: brew(PackageAction::Install),
                update: Some(brew(PackageAction::Update)),
                uninstall: Some(brew(PackageAction::Remove)),
                may_elevate: false,
                sources: &["Homebrew"],
            });
        }
    }
    methods
}

pub fn can_install(tool: &str, os: Os) -> bool {
    !methods(tool, os).is_empty()
}

/// The recommended way to install `tool`.
pub fn recommended(tool: &str, os: Os) -> Option<Method> {
    let methods = methods(tool, os);
    methods
        .iter()
        .find(|method| method.recommended)
        .or(methods.first())
        .cloned()
}

/// The copy Dragent uses, as the report lists it.
fn used_source(report: &ToolReport) -> Option<&str> {
    let resolved = report.resolved.as_deref()?;
    report
        .copies
        .iter()
        .find(|copy| copy.path == resolved || copy.real_path == resolved)
        .map(|copy| copy.source.as_str())
}

/// The method that installed the copy Dragent uses.
fn managing_method(tool: &str, report: &ToolReport, os: Os) -> Option<Method> {
    let source = used_source(report)?;
    methods(tool, os)
        .into_iter()
        .find(|method| method.sources.contains(&source))
}

/// How to update the copy Dragent uses: the tool's own updater when it has
/// one, else the method that installed it. `(command, may_elevate)`.
pub fn update_command(tool: &str, report: &ToolReport, os: Os) -> Option<(String, bool)> {
    if let (Some(args), Some(resolved)) = (self_update(tool), report.resolved.as_deref()) {
        if report.pin.is_none() || resolved.starts_with('/') {
            return Some((format!("{} {}", quote(resolved), args.join(" ")), false));
        }
    }
    let method = managing_method(tool, report, os)?;
    Some((method.update?, method.may_elevate))
}

/// How to remove the copy Dragent uses, when the method that installed it
/// can. `(command, may_elevate)`.
pub fn uninstall_command(tool: &str, report: &ToolReport, os: Os) -> Option<(String, bool)> {
    let method = managing_method(tool, report, os)?;
    Some((method.uninstall?, method.may_elevate))
}

/// One command installing every tool given, the package-manager ones in a
/// single call. It keeps going past a failure and fails at the end.
pub fn install_all(tools: &[&str], os: Os) -> Option<String> {
    let managed: Vec<&str> = tools
        .iter()
        .copied()
        .filter(|tool| host_tools::uses_package_manager(tool))
        .collect();
    let mut parts = Vec::new();
    if !managed.is_empty() {
        parts.push(host_tools::package_command(PackageAction::Install, &managed, os == Os::Mac).ok()?);
    }
    for tool in tools.iter().filter(|tool| !host_tools::uses_package_manager(tool)) {
        parts.push(recommended(tool, os)?.install);
    }
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => {
            let steps: Vec<String> = parts.iter().map(|part| format!("{part} || failed=1;")).collect();
            Some(sh(&format!("failed=0; {} exit $failed", steps.join(" "))))
        }
    }
}

/// What a job on a machine does to its tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Install,
    Update,
    Uninstall,
}

/// An install, update or uninstall run in the install sheet, in a terminal
/// on the machine. Afterwards Dragent checks `tools` to see it worked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolJob {
    pub kind: JobKind,
    pub tools: Vec<String>,
    /// The sheet's heading: "Install ripgrep", "Update Claude Code"…
    pub title: String,
    pub command: String,
    /// A permission failure may be retried with sudo.
    pub may_elevate: bool,
}

impl ToolJob {
    pub fn new(kind: JobKind, tools: Vec<String>, command: String, may_elevate: bool) -> Self {
        let verb = match kind {
            JobKind::Install => "Install",
            JobKind::Update => "Update",
            JobKind::Uninstall => "Uninstall",
        };
        let title = match tools.as_slice() {
            [tool] => format!("{verb} {}", label(tool)),
            many => format!("{verb} {} tools", many.len()),
        };
        Self {
            kind,
            tools,
            title,
            command,
            may_elevate,
        }
    }
}

/// Failed-install output that a retry with administrator rights may fix.
pub fn permission_denied(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    ["eacces", "eperm", "permission denied", "operation not permitted", "are you root"]
        .iter()
        .any(|sign| lower.contains(sign))
}

/// `command` run with administrator rights.
pub fn elevated(command: &str) -> String {
    format!("sudo {command}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_host::development::{ToolCopy, ToolStatus};

    fn report(tool: &str, path: &str, source: &str) -> ToolReport {
        ToolReport {
            name: tool.into(),
            status: ToolStatus::Ready,
            version: Some("1.0.0".into()),
            copies: vec![ToolCopy {
                path: path.into(),
                real_path: path.into(),
                source: source.into(),
            }],
            pin: None,
            resolved: Some(path.into()),
            detail: None,
        }
    }

    #[test]
    fn every_command_parses_as_sh() {
        let mut commands = Vec::new();
        for os in [Os::Mac, Os::Linux] {
            let tools = SYSTEM_TOOLS
                .iter()
                .copied()
                .chain(PromptAgent::ALL.iter().filter_map(|agent| agent.cli()));
            for tool in tools {
                for method in methods(tool, os) {
                    commands.push(method.install.clone());
                    commands.extend(method.update.clone());
                    commands.extend(method.uninstall.clone());
                }
            }
            commands.push(install_all(&["git", "rg", "node", "cargo"], os).unwrap());
        }
        for command in commands {
            let status = std::process::Command::new("sh")
                .arg("-n")
                .arg("-c")
                .arg(&command)
                .status()
                .unwrap();
            assert!(status.success(), "{command}");
        }
    }

    #[test]
    fn methods_match_how_tools_install() {
        let linux = methods("rg", Os::Linux);
        assert_eq!(linux.len(), 1);
        assert!(linux[0].install.contains("apt-get") && linux[0].uninstall.is_some());
        assert!(methods("git", Os::Linux)[0].uninstall.is_none(), "never remove git");
        assert!(methods("python3", Os::Mac)[0].uninstall.is_none());
        assert_eq!(methods("cargo", Os::Linux)[0].label, "rustup");
        assert!(methods("docker", Os::Linux).is_empty());
        assert!(docs("docker").is_some());

        // Official installer first, Homebrew offered on a Mac.
        let claude = methods("claude", Os::Mac);
        assert_eq!(claude[0].label, "Official installer");
        assert!(claude[0].recommended);
        assert!(claude[1].install.contains("--cask claude-code"));
        assert_eq!(methods("claude", Os::Linux).len(), 1);
        let codex = methods("codex", Os::Linux);
        assert_eq!(codex[0].label, "npm");
        assert!(codex[0].update.as_ref().unwrap().contains("@openai/codex@latest"));
        assert!(docs("codex").unwrap().contains("npmjs.com/package/@openai/codex"));
        // A login command isn't an installer.
        assert!(!can_install("rovodev", Os::Linux));
    }

    #[test]
    fn updates_use_the_tools_own_updater_or_its_installer() {
        let claude = report("claude", "/home/me/.local/bin/claude", "Installer");
        assert_eq!(update_command("claude", &claude, Os::Linux).unwrap().0, "'/home/me/.local/bin/claude' update");
        let codex = report("codex", "/usr/lib/node_modules/@openai/codex/bin/codex.js", "npm");
        let (update, elevate) = update_command("codex", &codex, Os::Linux).unwrap();
        assert!(update.contains("@openai/codex@latest") && elevate);
        assert!(uninstall_command("codex", &codex, Os::Linux).unwrap().0.contains("uninstall -g @openai/codex"));
        // Installed some way Dragent didn't: no guessing.
        let manual = report("rg", "/opt/tools/rg", "Manual");
        assert!(update_command("rg", &manual, Os::Linux).is_none());
        assert!(uninstall_command("rg", &manual, Os::Linux).is_none());
        let system = report("rg", "/usr/bin/rg", "System");
        assert!(update_command("rg", &system, Os::Linux).unwrap().0.contains("--only-upgrade ripgrep"));
    }

    #[test]
    fn install_all_merges_package_manager_tools() {
        let command = install_all(&["git", "rg", "cargo"], Os::Linux).unwrap();
        assert!(command.contains("install -y git ripgrep"), "{command}");
        assert!(command.contains("rustup"), "{command}");
        assert!(command.contains("exit $failed"), "{command}");
        assert_eq!(install_all(&["git"], Os::Mac).unwrap(), host_tools::package_command(PackageAction::Install, &["git"], true).unwrap());
    }

    #[test]
    fn permission_failures_are_recognised() {
        assert!(permission_denied("npm ERR! code EACCES"));
        assert!(permission_denied("E: Could not open lock file - open (13: Permission denied)"));
        assert!(!permission_denied("E: Unable to locate package nope"));
        assert_eq!(elevated("sh -c 'x'"), "sudo sh -c 'x'");
    }
}
