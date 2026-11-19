use std::collections::HashSet;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SshDestination {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl SshDestination {
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();
        if input.is_empty() || input.chars().any(char::is_whitespace) {
            return Err("Enter an SSH host such as user@example.com.".to_string());
        }

        let (user, host) = match input.rsplit_once('@') {
            Some((user, host)) if !user.is_empty() && !host.is_empty() && !user.contains('@') => {
                (Some(user.to_string()), host)
            }
            Some(_) => return Err("Use one username and host, such as user@example.com.".into()),
            None => (None, input),
        };

        if user.as_deref().is_some_and(|user| !valid_name(user)) {
            return Err("The SSH username contains unsupported characters.".into());
        }

        let (host, port) = parse_host_and_port(host)?;
        Ok(Self { user, host, port })
    }

    pub fn display(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let destination = match &self.user {
            Some(user) => format!("{user}@{host}"),
            None => host,
        };
        match self.port {
            Some(port) => format!("{destination}:{port}"),
            None => destination,
        }
    }

    pub fn shell_command(&self) -> String {
        let destination = shell_quote(&match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        });
        match self.port {
            Some(port) => format!("ssh -p {port} -- {destination}"),
            None => format!("ssh -- {destination}"),
        }
    }
}

pub fn ssh_config_hosts() -> Vec<SshDestination> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut visited = HashSet::new();
    let mut config = String::new();
    read_config_includes(&home.join(".ssh/config"), &home.join(".ssh"), &mut visited, &mut config);
    parse_ssh_config_hosts(&config)
}

fn read_config_includes(path: &std::path::Path, base: &std::path::Path, visited: &mut HashSet<std::path::PathBuf>, output: &mut String) {
    if visited.len() >= 64 || output.len() > 1024 * 1024 { return; }
    let Ok(path) = path.canonicalize() else { return; };
    if !visited.insert(path.clone()) { return; }
    let Ok(config) = std::fs::read_to_string(path) else { return; };
    for line in config.lines() {
        output.push_str(line); output.push('\n');
        let line = line.split('#').next().unwrap_or_default().trim();
        if let Some((key,patterns)) = line.split_once(char::is_whitespace).or_else(|| line.split_once('=')) {
            if key.eq_ignore_ascii_case("include") {
                for pattern in patterns.split_whitespace() {
                    let pattern = pattern.trim_matches(['\"','\'']);
                    let path = if let Some(rest) = pattern.strip_prefix("~/") { dirs::home_dir().unwrap_or_default().join(rest) } else if std::path::Path::new(pattern).is_absolute() { std::path::PathBuf::from(pattern) } else { base.join(pattern) };
                    if let Ok(paths) = glob::glob(&path.to_string_lossy()) { for path in paths.flatten() { read_config_includes(&path,base,visited,output); } }
                }
            }
        }
    }
}

fn parse_ssh_config_hosts(config: &str) -> Vec<SshDestination> {
    let mut seen = HashSet::new();
    let mut hosts = Vec::new();
    for line in config.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let fields = line.split_once(char::is_whitespace);
        let assignment = line
            .split_once('=')
            .filter(|(key, _)| !key.chars().any(char::is_whitespace));
        let Some((key, values)) = assignment.or(fields) else {
            continue;
        };
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }
        for alias in values.split_whitespace() {
            if valid_name(alias)
                && !alias.contains(['*', '?', '!'])
                && seen.insert(alias.to_string())
            {
                hosts.push(SshDestination {
                    user: None,
                    host: alias.to_string(),
                    port: None,
                });
            }
        }
    }
    hosts
}

fn parse_host_and_port(input: &str) -> Result<(String, Option<u16>), String> {
    if let Some(rest) = input.strip_prefix('[') {
        if let Some((host, port)) = rest.rsplit_once("]:") {
            if !host.ends_with(']') && valid_ipv6(host) {
                return Ok((host.to_string(), Some(parse_port(port)?)));
            }
        }
        return parse_bracketed_host(rest);
    }

    if let Some((host, port)) = input.rsplit_once(':') {
        if !host.contains(':') {
            return Ok((valid_host(host)?.to_string(), Some(parse_port(port)?)));
        }
    }
    Ok((valid_host(input)?.to_string(), None))
}

fn parse_bracketed_host(input: &str) -> Result<(String, Option<u16>), String> {
    let host = input
        .strip_suffix(']')
        .ok_or_else(|| "Bracketed IPv6 addresses must end with `]`.".to_string())?;
    if valid_ipv6(host) {
        Ok((host.to_string(), None))
    } else {
        Err("Enter a valid bracketed IPv6 address.".to_string())
    }
}

fn valid_host(host: &str) -> Result<&str, String> {
    if !host.is_empty() && valid_name(host) {
        Ok(host)
    } else {
        Err("The SSH host contains unsupported characters.".to_string())
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

fn valid_ipv6(value: &str) -> bool {
    value.parse::<std::net::Ipv6Addr>().is_ok()
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| "Use an SSH port between 1 and 65535.".to_string())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_ports_and_ipv6() {
        let direct = SshDestination::parse("engineer@example.com:2222").unwrap();
        assert_eq!(
            direct.shell_command(),
            "ssh -p 2222 -- 'engineer@example.com'"
        );
        assert_eq!(direct.display(), "engineer@example.com:2222");

        let ipv6 = SshDestination::parse("engineer@[2001:db8::1]:2200").unwrap();
        assert_eq!(
            ipv6.shell_command(),
            "ssh -p 2200 -- 'engineer@2001:db8::1'"
        );
        assert_eq!(ipv6.display(), "engineer@[2001:db8::1]:2200");
    }

    #[test]
    fn rejects_shell_syntax_and_malformed_destinations() {
        for input in [
            "host;rm",
            "ssh host",
            "-oProxyCommand=x",
            "user@@host",
            "host:0",
        ] {
            assert!(SshDestination::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn reads_only_literal_ssh_config_hosts() {
        let config = "Host work staging\nHost * !ignored wildcard\nHost=not-supported";
        let aliases: Vec<_> = parse_ssh_config_hosts(config)
            .into_iter()
            .map(|destination| destination.host)
            .collect();
        // `*` and `!ignored` are patterns and correctly dropped; `wildcard`
        // is a literal alias and is kept, like OpenSSH itself would match it.
        assert_eq!(aliases, ["work", "staging", "wildcard", "not-supported"]);
    }
}
