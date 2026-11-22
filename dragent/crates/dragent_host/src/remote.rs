//! OpenSSH control plane and encrypted forwarding for the host wire protocol.
//! Runtime work always goes through HostClient, never desktop SSH shell commands.
use crate::protocol::{PROTOCOL_VERSION, ServiceEndpoint};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Authentication {
    #[default]
    Agent,
    Key,
    Password,
}

mod askpass;

/// Runs the private credential helper before app/service startup.
pub fn run_askpass() -> bool {
    askpass::run()
}

struct SshCommand {
    command: Command,
    // Own the private channel for as long as SSH can request a credential.
    _askpass: Option<askpass::Broker>,
}
impl std::ops::Deref for SshCommand {
    type Target = Command;
    fn deref(&self) -> &Command {
        &self.command
    }
}
impl std::ops::DerefMut for SshCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

#[derive(Clone)]
struct Secret(String);
impl Drop for Secret {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshHost {
    pub host: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub proxy_jump: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_file: Option<PathBuf>,
    #[serde(default)]
    pub forward_agent: bool,
    #[serde(default)]
    pub authentication: Authentication,
}
impl SshHost {
    pub fn remember_credential(&self, secret: &str) -> Result<(), String> {
        if secret.is_empty() {
            return Ok(());
        }
        keyring::Entry::new("com.dragent.remote", &self.credential_account()?)
            .map_err(|e| e.to_string())?
            .set_password(secret)
            .map_err(|e| e.to_string())
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.host.is_empty()
            || self.host.starts_with('-')
            || self
                .host
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && !"._-:".contains(c))
        {
            return Err("Invalid SSH hostname".into());
        }
        if self.user.as_ref().is_some_and(|u| {
            u.is_empty()
                || u.starts_with('-')
                || u.chars()
                    .any(|c| !c.is_ascii_alphanumeric() && !"._-".contains(c))
        }) || self.port == Some(0)
        {
            return Err("Invalid SSH username or port".into());
        }
        if self.proxy_jump.as_ref().is_some_and(|jump| {
            jump.is_empty()
                || jump.starts_with('-')
                || jump
                    .chars()
                    .any(|c| !c.is_ascii_alphanumeric() && !"._-:@[],".contains(c))
        }) {
            return Err("Invalid jump host".into());
        }
        Ok(())
    }
    fn command(&self) -> Result<SshCommand, String> {
        self.command_with_secret(None)
    }
    fn command_with_secret(&self, secret: Option<&Secret>) -> Result<SshCommand, String> {
        self.ssh_command(secret, HostKeys::Strict)
    }
    fn ssh_command(&self, secret: Option<&Secret>, host_keys: HostKeys) -> Result<SshCommand, String> {
        self.validate()?;
        let mut command = Command::new("ssh");
        command
            .env_remove("DRAGENT_SSH_SECRET")
            .env_remove("DRAGENT_SSH_ASKPASS")
            .env_remove("DRAGENT_SSH_ASKPASS_ADDRESS");
        let askpass = secret.map(askpass::Broker::new).transpose()?;
        // OpenSSH owns known_hosts, config Includes, ProxyJump, identities,
        // certificates and agent authentication. Never disable verification:
        // a machine's first key is trusted only once the person confirms its
        // fingerprint (`unknown_host_key`, then `trust_host_key`).
        command.args([
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ]);
        match host_keys {
            HostKeys::Strict => {
                command.args(["-o", "StrictHostKeyChecking=yes"]);
            }
            HostKeys::Capture { into, known } => {
                // New keys land in the first file only, never in the
                // person's own known_hosts.
                let mut files = vec![ssh_config_word(&into.to_string_lossy())];
                files.extend(known);
                command
                    .args(["-o", "StrictHostKeyChecking=accept-new"])
                    .arg("-o")
                    .arg(format!("UserKnownHostsFile={}", files.join(" ")));
            }
        }
        command.args([
            "-o",
            if secret.is_some() {
                "BatchMode=no"
            } else {
                "BatchMode=yes"
            },
        ]);
        if let Some(config) = &self.config_file {
            command.arg("-F").arg(config);
        }
        if let Some(port) = self.port {
            command.args(["-p", &port.to_string()]);
        }
        if let Some(user) = &self.user {
            command.args(["-l", user]);
        }
        if let Some(key) = &self.identity_file {
            command.arg("-i").arg(key);
        }
        command.arg(if self.forward_agent { "-A" } else { "-a" });
        if let Some(jump) = &self.proxy_jump {
            command.arg("-J").arg(jump);
        }
        if let Some(broker) = &askpass {
            command
                .env(
                    "SSH_ASKPASS",
                    std::env::current_exe().map_err(|e| e.to_string())?,
                )
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", "dragent:0")
                .env("DRAGENT_SSH_ASKPASS", "1")
                .env("DRAGENT_SSH_ASKPASS_ADDRESS", &broker.address);
        }
        match self.authentication {
            Authentication::Password => {
                command.args([
                    "-o",
                    "PreferredAuthentications=password,keyboard-interactive",
                ]);
            }
            Authentication::Key => {
                command.args([
                    "-o",
                    "IdentitiesOnly=yes",
                    "-o",
                    "PreferredAuthentications=publickey",
                ]);
            }
            Authentication::Agent => {}
        }
        Ok(SshCommand {
            command,
            _askpass: askpass,
        })
    }
    pub fn label(&self) -> String {
        let address = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let host = match &self.user {
            Some(user) => format!("{user}@{address}"),
            None => address,
        };
        match self.port {
            Some(port) => format!("{host}:{port}"),
            None => host,
        }
    }
    fn exec(&self, script: &str, secret: Option<&Secret>) -> Result<Vec<u8>, String> {
        let mut command = self.command_with_secret(secret)?;
        command
            .arg("--")
            .arg(&self.host)
            .arg(format!("sh -c {}", quote(script)));
        let output = bounded_output(command, Duration::from_secs(180))?;
        if !output.status.success() {
            return Err(format!(
                "SSH {}: {}",
                self.label(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output.stdout)
    }

    /// Bind secrets to the effective OpenSSH destination and key content,
    /// rather than to a mutable alias or a display name.
    fn credential_account(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let mut command = self.command()?;
        command.arg("-G").arg("--").arg(&self.host);
        let output = bounded_output(command, Duration::from_secs(10))?;
        if !output.status.success() {
            return Err("Could not resolve SSH configuration".into());
        }
        let config = String::from_utf8_lossy(&output.stdout);
        let mut binding = format!("{:?}\n", self.authentication);
        for line in config.lines().filter(|l| {
            l.starts_with("hostname ")
                || l.starts_with("user ")
                || l.starts_with("port ")
                || l.starts_with("identityfile ")
                || l.starts_with("proxyjump ")
                || l.starts_with("proxycommand ")
        }) {
            binding.push_str(line);
            binding.push('\n');
            if let Some(key) = line.strip_prefix("identityfile ") {
                let key = expand_key_path(key);
                if let Ok(bytes) = fs::read(key) {
                    binding.push_str(&format!("{:x}", Sha256::digest(bytes)));
                }
            }
        }
        Ok(format!("ssh/{:x}", Sha256::digest(binding.as_bytes())))
    }
    /// Effective OpenSSH settings for this destination (`ssh -G`), as shown in
    /// the machine form once an SSH config alias resolves.
    pub fn resolve(&self) -> Result<ResolvedSsh, String> {
        let mut command = self.command()?;
        command.arg("-G").arg("--").arg(&self.host);
        let output = bounded_output(command, Duration::from_secs(10))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if detail.is_empty() {
                "Could not resolve SSH configuration".into()
            } else {
                detail
            });
        }
        let config = String::from_utf8_lossy(&output.stdout);
        let value = |key: &str| {
            config.lines().find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix(' '))
                    .map(str::trim)
                    .filter(|v| !v.is_empty() && *v != "none")
                    .map(str::to_string)
            })
        };
        Ok(ResolvedSsh {
            hostname: value("hostname").unwrap_or_else(|| self.host.clone()),
            user: value("user"),
            port: value("port").and_then(|p| p.parse().ok()).unwrap_or(22),
            proxy_jump: value("proxyjump"),
            proxy_command: value("proxycommand"),
            forward_agent: value("forwardagent").is_some_and(|v| v == "yes"),
            identity_file: value("identityfile"),
        })
    }
    pub fn forget_credentials(&self) -> Result<(), String> {
        let entry = keyring::Entry::new("com.dragent.remote", &self.credential_account()?)
            .map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn test(&self, secret: Option<String>) -> Result<String, String> {
        let secret = secret
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.credential_account().ok().and_then(|a| {
                    keyring::Entry::new("com.dragent.remote", &a)
                        .ok()?
                        .get_password()
                        .ok()
                })
            })
            .map(Secret);
        let out = self.exec(
            "printf 'SSH connection successful\\n'; uname -sm",
            secret.as_ref(),
        )?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// The person's own known_hosts files, as OpenSSH resolves them for this
    /// destination (`UserKnownHostsFile`).
    fn known_hosts_files(&self) -> Result<Vec<String>, String> {
        let mut command = self.command()?;
        command.arg("-G").arg("--").arg(&self.host);
        let output = bounded_output(command, Duration::from_secs(10))?;
        if !output.status.success() {
            return Err("Could not resolve SSH configuration".into());
        }
        let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("userknownhostsfile "))
            .map(|files| files.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        if files.is_empty() {
            Ok(vec!["~/.ssh/known_hosts".into()])
        } else {
            Ok(files)
        }
    }

    /// The machine's host key when nothing trusts it yet, read without
    /// signing in and without recording it anywhere the person's SSH looks.
    /// `None` when it's already trusted.
    pub fn unknown_host_key(&self) -> Result<Option<UnknownHostKey>, String> {
        let directory = private_directory("drkey")?;
        let capture = directory.join("known_hosts");
        let result = (|| {
            fs::write(&capture, b"").map_err(|e| e.to_string())?;
            let known = self.known_hosts_files()?;
            let mut command = self.ssh_command(
                None,
                HostKeys::Capture {
                    into: capture.clone(),
                    known,
                },
            )?;
            // Stop after the key exchange: no credentials are offered.
            for option in [
                "PubkeyAuthentication=no",
                "PasswordAuthentication=no",
                "KbdInteractiveAuthentication=no",
                "GSSAPIAuthentication=no",
                "HostbasedAuthentication=no",
            ] {
                command.args(["-o", option]);
            }
            command.arg("-T").arg("--").arg(&self.host).arg("true");
            let output = bounded_output(command, Duration::from_secs(30))?;
            let entries = fs::read_to_string(&capture).unwrap_or_default();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if entries.trim().is_empty() {
                if output.status.success() || stderr.contains("Permission denied") {
                    return Ok(None);
                }
                return Err(if stderr.is_empty() {
                    format!("SSH {}: could not read the machine's host key", self.label())
                } else {
                    format!("SSH {}: {stderr}", self.label())
                });
            }
            let fingerprints: Vec<String> = entries.lines().filter_map(fingerprint).collect();
            if fingerprints.is_empty() {
                return Err("The machine sent a host key Dragent can't read".into());
            }
            Ok(Some(UnknownHostKey {
                fingerprints,
                entries,
            }))
        })();
        let _ = fs::remove_dir_all(&directory);
        result
    }

    /// Trust `key` from now on, as answering "yes" to OpenSSH's own prompt
    /// would: it's added to the person's known_hosts.
    pub fn trust_host_key(&self, key: &UnknownHostKey) -> Result<(), String> {
        use std::io::Write;
        let first = self.known_hosts_files()?.remove(0);
        let path = expand_key_path(&first);
        if let Some(parent) = path.parent() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(parent).map_err(|e| e.to_string())?;
        }
        let existing = fs::read(&path).unwrap_or_default();
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut text = String::new();
        if existing.last().is_some_and(|byte| *byte != b'\n') {
            text.push('\n');
        }
        for line in key.entries.lines().filter(|line| !line.trim().is_empty()) {
            text.push_str(line.trim_end());
            text.push('\n');
        }
        file.write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// How a command treats the machine's host key.
enum HostKeys {
    /// Only keys the person already trusts.
    Strict,
    /// Accept a new key, but record it in `into` alone; `known` are the
    /// person's own files, still consulted.
    Capture { into: PathBuf, known: Vec<String> },
}

/// A machine's host key that nothing trusts yet, waiting for the person to
/// confirm it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownHostKey {
    /// What to compare with the machine's own, e.g. `ED25519 SHA256:…`.
    pub fingerprints: Vec<String>,
    /// The known_hosts lines OpenSSH recorded, written on trust.
    entries: String,
}

/// Why OpenSSH refused the machine's host key, from its error output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyProblem {
    /// Nothing trusts the machine's key yet: ask the person.
    Unknown,
    /// The machine's key isn't the one trusted before.
    Changed,
}

pub fn host_key_problem(error: &str) -> Option<HostKeyProblem> {
    if error.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || error.contains("Host key for") && error.contains("has changed")
    {
        Some(HostKeyProblem::Changed)
    } else if error.contains("Host key verification failed") {
        Some(HostKeyProblem::Unknown)
    } else {
        None
    }
}

/// `ssh-keygen -l` style fingerprint of one known_hosts line:
/// `ED25519 SHA256:…`.
fn fingerprint(line: &str) -> Option<String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let mut fields = line.split_whitespace();
    let _hosts = fields.next()?;
    let kind = fields.next()?;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(fields.next()?)
        .ok()?;
    let digest = base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&blob));
    let name = match kind {
        "ssh-ed25519" => "ED25519",
        "ssh-rsa" => "RSA",
        "ssh-dss" => "DSA",
        kind if kind.starts_with("ecdsa-") => "ECDSA",
        kind if kind.starts_with("sk-ssh-ed25519") => "ED25519-SK",
        kind if kind.starts_with("sk-ecdsa") => "ECDSA-SK",
        kind => kind,
    };
    Some(format!("{name} SHA256:{digest}"))
}

/// A path as one word of an OpenSSH option value.
fn ssh_config_word(text: &str) -> String {
    if text.contains(char::is_whitespace) {
        format!("\"{text}\"")
    } else {
        text.to_string()
    }
}

/// An owner-only temporary directory.
fn private_directory(prefix: &str) -> Result<PathBuf, String> {
    let base = if cfg!(target_os = "macos") {
        PathBuf::from("/private/tmp")
    } else {
        std::env::temp_dir()
    };
    let directory = base.join(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&directory).map_err(|e| e.to_string())?;
    Ok(directory)
}

/// OpenSSH's effective connection settings for one destination.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedSsh {
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub forward_agent: bool,
    pub identity_file: Option<String>,
}

pub fn expand_key_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else {
        PathBuf::from(path)
    }
}

pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

fn bounded_output(
    mut command: SshCommand,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    // Drain concurrently, so progress output cannot deadlock the child.
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = std::io::Read::take(&mut pipe, 4 * 1024 * 1024).read_to_end(&mut bytes);
            bytes
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let deadline = Instant::now() + timeout;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("SSH operation timed out".to_string());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(e.to_string());
            }
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    Ok(std::process::Output {
        status: result?,
        stdout,
        stderr,
    })
}

/// The oldest glibc the published Linux runtime runs on: it is built on
/// Debian 11 (see `.github/workflows/remote-host-release.yml`).
const MIN_GLIBC: &str = "2.31";

/// Installs a protocol-pinned, checksum-verified headless host release. The
/// path may be overridden for development/offline deployments. Existing
/// daemons are reused, and protocol mismatch is surfaced without killing
/// another client's live terminals.
fn provision(
    host: &SshHost,
    secret: Option<&Secret>,
    update: bool,
    scope: &str,
) -> Result<ServiceEndpoint, String> {
    // Each release's runtime is a file on that version's GitHub Release.
    let cdn = std::env::var("DRAGENT_REMOTE_RELEASE_URL")
        .unwrap_or_else(|_| "https://github.com/Dragent-Term/dragent/releases/download".into());
    if !cdn.starts_with("https://") {
        return Err("Remote releases require HTTPS".into());
    }
    let override_binary = std::env::var("DRAGENT_REMOTE_BINARY")
        .ok()
        .filter(|path| !path.is_empty())
        .or_else(|| loopback_development_binary(host));
    let managed = override_binary.is_none();
    let binary = override_binary
        .unwrap_or_else(|| format!("$HOME/.dragent/remote/bin/v{PROTOCOL_VERSION}/dragent-host"));
    // Only the fixed default expands HOME; custom paths are literal.
    let binary = if binary == format!("$HOME/.dragent/remote/bin/v{PROTOCOL_VERSION}/dragent-host")
    {
        format!("\"$HOME/.dragent/remote/bin/v{PROTOCOL_VERSION}/dragent-host\"")
    } else {
        quote(&binary)
    };
    let script = provision_script_scoped(&cdn, &binary, managed, update, scope);
    let bytes = host.exec(&script, secret)?;
    let endpoint_line = bytes
        .split(|b| *b == b'\n')
        .rev()
        .find(|line| line.starts_with(b"{"))
        .ok_or("Remote host did not return an endpoint")?;
    let endpoint: ServiceEndpoint = serde_json::from_slice(endpoint_line)
        .map_err(|e| format!("Invalid remote host endpoint: {e}"))?;
    if endpoint.version != PROTOCOL_VERSION {
        return Err(format!(
            "Remote host protocol {} differs from desktop {}. Upgrade the remote host before connecting.",
            endpoint.version, PROTOCOL_VERSION
        ));
    }
    Ok(endpoint)
}

/// Development desktops can be launched by Finder without inheriting a shell
/// environment. For loopback hosts, use a matching adjacent debug host binary
/// when one is present. Real remote hosts download from the GitHub Release.
/// The script that installs (when missing, or on `update`) and starts the
/// runtime on the remote machine, then prints its endpoint.
#[cfg(test)]
fn provision_script(cdn: &str, binary: &str, managed: bool, update: bool) -> String {
    provision_script_scoped(cdn, binary, managed, update, "")
}
fn runtime_scope() -> String {
    let org = dragent_paths::organisation_id();
    if org == "local" { String::new() } else { format!("/{}", dragent_paths::organisation_key(&org)) }
}
fn provision_script_scoped(cdn: &str, binary: &str, managed: bool, update: bool, scope: &str) -> String {
    format!(
        r#"set -eu
login_umask=$(umask)
umask 077
export DRAGENT_HOME="$HOME/.dragent/remote/runtime-v{protocol}{scope}"
export DRAGENT_HOST_ENDPOINT_FILE="$DRAGENT_HOME/host-v1.json"
mkdir -p "$DRAGENT_HOME" "$HOME/.dragent/remote/bin/v{protocol}"
binary={binary}
if [ {managed} = 0 ] && [ ! -x "$binary" ]; then
  echo 'The configured development host binary is not executable on the remote host' >&2
  exit 1
fi
if [ {managed} = 1 ] && {{ [ ! -x "$binary" ] || [ {update} = 1 ]; }}; then
  case "$(uname -s)" in Linux) os=linux;; Darwin) os=macos;; *) echo 'Remote hosts require Linux or macOS' >&2; exit 1;; esac
  case "$(uname -m)" in x86_64|amd64) arch=x86_64;; aarch64|arm64) arch=arm64;; *) echo 'Unsupported remote architecture' >&2; exit 1;; esac
  if [ "$os" = linux ]; then
    glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{{print $2}}')
    if [ -z "$glibc" ]; then
      echo 'Dragent needs a glibc-based Linux on this machine; musl distributions such as Alpine are not supported yet' >&2; exit 1
    fi
    if ! awk -v have="$glibc" -v need={min_glibc} 'BEGIN {{ split(have, h, "."); split(need, n, "."); exit !(h[1] > n[1] || (h[1] == n[1] && h[2] >= n[2])) }}'; then
      echo "Dragent needs glibc {min_glibc} or newer on this machine, which has $glibc (for example Ubuntu 20.04, Debian 11 or RHEL 9 and later)" >&2; exit 1
    fi
  fi
  url={cdn}/v{release}/dragent-host-$os-$arch
  if command -v curl >/dev/null 2>&1; then
    fetch() {{ curl --fail --silent --show-error --location --proto '=https' --max-time "$3" "$1" -o "$2"; }}
  elif command -v wget >/dev/null 2>&1; then
    fetch() {{ wget --quiet --https-only --timeout="$3" -O "$2" "$1"; }}
  else
    echo 'Dragent needs curl or wget on this machine to install its workspace runtime' >&2; exit 1
  fi
  staging=$(mktemp -d "$HOME/.dragent/remote/bin/install.XXXXXX")
  trap 'rm -rf "$staging"' EXIT
  if ! fetch "$url" "$staging/dragent-host" 120 || ! fetch "$url.sha256" "$staging/dragent-host.sha256" 30; then
    echo "Couldn't download Dragent's workspace runtime {release} for $os-$arch. Check the machine's internet access; if it has access, this version's runtime isn't published yet." >&2; exit 1
  fi
  (cd "$staging"; if command -v sha256sum >/dev/null 2>&1; then sha256sum -c dragent-host.sha256; else shasum -a 256 -c dragent-host.sha256; fi) >&2
  chmod 700 "$staging/dragent-host"
  mv "$staging/dragent-host" "$binary"
fi
export DRAGENT_HOST_LOG_FILE="$DRAGENT_HOME/host.log"
export DRAGENT_REMOTE_HOST_BINARY="$binary"
# Only the runtime's own folders above are private. Terminals, agents and
# installs it starts get the account's usual permissions.
umask "$login_umask"
if [ -n "${{SHELL:-}}" ] && [ -x "$SHELL" ]; then
  "$SHELL" -lc 'exec "$DRAGENT_REMOTE_HOST_BINARY" --dragent-print-host-endpoint'
else
  "$binary" --dragent-print-host-endpoint
fi
"#,
        protocol = PROTOCOL_VERSION,
        release = env!("CARGO_PKG_VERSION"),
        update = if update { 1 } else { 0 },
        managed = if managed { 1 } else { 0 },
        cdn = quote(cdn.trim_end_matches('/')),
        binary = binary,
        min_glibc = MIN_GLIBC,
        scope = scope,
    )
}

fn loopback_development_binary(host: &SshHost) -> Option<String> {
    if !matches!(host.host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return None;
    }
    let executable = std::env::current_exe().ok()?;
    let candidates = [
        executable.parent()?.join("dragent-host"),
        executable
            .parent()?
            .parent()?
            .join("Resources")
            .join("dragent-host"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

#[derive(Debug)]
struct Tunnel {
    _askpass: Option<askpass::Broker>,
    child: Child,
    endpoint: ServiceEndpoint,
}

#[derive(Debug)]
pub struct RemoteConnection {
    pub host: SshHost,
    secret: Option<Secret>,
    credential_binding: String,
    runtime_scope: String,
    forwards: Mutex<std::collections::BTreeMap<u16, u16>>,
    directory: PathBuf,
    tunnel: Mutex<Option<Tunnel>>,
    suspended: std::sync::atomic::AtomicBool,
}
impl RemoteConnection {
    pub fn connect(host: SshHost) -> Result<Self, String> {
        Self::connect_with_secret(host, None, false)
    }
    pub fn connect_with_secret(
        host: SshHost,
        supplied: Option<String>,
        remember: bool,
    ) -> Result<Self, String> {
        host.validate()?;
        let account = host.credential_account()?;
        let entry =
            keyring::Entry::new("com.dragent.remote", &account).map_err(|e| e.to_string())?;
        let secret = supplied
            .filter(|s| !s.is_empty())
            .or_else(|| entry.get_password().ok())
            .map(Secret);
        let directory = private_directory("drssh")?;
        let connection = Self {
            host,
            secret,
            credential_binding: account,
            runtime_scope: runtime_scope(),
            forwards: Mutex::new(std::collections::BTreeMap::new()),
            directory,
            tunnel: Mutex::new(None),
            suspended: std::sync::atomic::AtomicBool::new(false),
        };
        connection.endpoint()?;
        if remember {
            if let Some(secret) = &connection.secret {
                entry
                    .set_password(&secret.0)
                    .map_err(|e| format!("Connected, but OS credential storage failed: {e}"))?;
            }
        }
        Ok(connection)
    }
    /// Single recovery owner. Called before RPC/stream connect; never retries
    /// a mutation after an ambiguous response, and never restarts the daemon.
    pub fn endpoint(&self) -> Result<ServiceEndpoint, String> {
        if self.suspended.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Machine disconnected. Connect it from Settings → Machines.".into());
        }
        let mut guard = self.tunnel.lock().map_err(|e| e.to_string())?;
        if self.suspended.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Machine disconnected. Connect it from Settings → Machines.".into());
        }
        if let Some(tunnel) = guard.as_mut() {
            if tunnel
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Ok(tunnel.endpoint.clone());
            }
        }
        if let Some(mut old) = guard.take() {
            let _ = old.child.kill();
            let _ = old.child.wait();
        }
        if self.host.credential_account()? != self.credential_binding {
            return Err(
                "SSH destination or key changed. Reconnect using the updated connection settings."
                    .into(),
            );
        }
        let remote = if self.directory.join("connected").exists() {
            // Reconnect probes the existing endpoint only; it cannot start a
            // daemon and turn an outage into a replacement terminal session.
            let script = format!(
                "cat \"$HOME/.dragent/remote/runtime-v{}{}/host-v1.json\"",
                PROTOCOL_VERSION, self.runtime_scope
            );
            serde_json::from_slice::<ServiceEndpoint>(
                &self.host.exec(&script, self.secret.as_ref())?,
            )
            .map_err(|e| e.to_string())?
        } else {
            provision(&self.host, self.secret.as_ref(), false, &self.runtime_scope)?
        };
        if remote.version != PROTOCOL_VERSION {
            return Err("Remote protocol changed; upgrade or reconnect explicitly".into());
        }
        let local_socket = self.directory.join("wire.sock");
        #[cfg(not(unix))]
        let local_port = {
            let listener =
                std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
            listener.local_addr().map_err(|e| e.to_string())?.port()
        };
        let _ = fs::remove_file(&local_socket);
        let _ = fs::remove_file(self.directory.join("control"));
        let target = match remote.socket.as_ref() {
            Some(socket) if !socket.contains(['\n', '\0']) => socket.clone(),
            Some(_) => return Err("Invalid remote socket".into()),
            None if remote.port != 0 => format!("127.0.0.1:{}", remote.port),
            None => return Err("Remote endpoint has no address".into()),
        };
        #[cfg(unix)]
        let forwarding = format!("{}:{target}", local_socket.display());
        #[cfg(not(unix))]
        let forwarding = format!("127.0.0.1:{local_port}:{target}");
        let mut command = self.host.command_with_secret(self.secret.as_ref())?;
        command
            .arg("-M")
            .arg("-S")
            .arg(self.directory.join("control"));
        for (local, remote) in self.forwards.lock().map_err(|e| e.to_string())?.iter() {
            command
                .arg("-L")
                .arg(format!("127.0.0.1:{local}:127.0.0.1:{remote}"));
        }
        command
            .args([
                "-N",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "StreamLocalBindUnlink=yes",
                "-L",
            ])
            .arg(forwarding)
            .arg("--")
            .arg(&self.host.host)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                fs::File::create(self.directory.join("ssh.log")).map_err(|e| e.to_string())?,
            ));
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut endpoint = remote;
        #[cfg(unix)]
        {
            endpoint.socket = Some(local_socket.to_string_lossy().into_owned());
            endpoint.port = 0;
        }
        #[cfg(not(unix))]
        {
            endpoint.socket = None;
            endpoint.port = local_port;
        }
        loop {
            if crate::discovery::ping(&endpoint, "remote-probe") {
                break;
            }
            if child.try_wait().map_err(|e| e.to_string())?.is_some() || Instant::now() >= deadline
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "SSH forwarding failed: {}",
                    fs::read_to_string(self.directory.join("ssh.log")).unwrap_or_default()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        fs::write(self.directory.join("connected"), b"ready").map_err(|e| e.to_string())?;
        *guard = Some(Tunnel {
            _askpass: command._askpass.take(),
            child,
            endpoint: endpoint.clone(),
        });
        Ok(endpoint)
    }
    pub fn disconnect(&self) {
        if let Ok(mut tunnel) = self.tunnel.lock() {
            if let Some(mut tunnel) = tunnel.take() {
                let _ = tunnel.child.kill();
                let _ = tunnel.child.wait();
            }
        }
    }
    /// User intent differs from an accidental transport loss: background RPCs
    /// must not silently reconnect a machine explicitly disconnected in Settings.
    pub fn suspend(&self) {
        self.suspended
            .store(true, std::sync::atomic::Ordering::Release);
        self.disconnect();
    }
    pub fn resume(&self) {
        self.suspended
            .store(false, std::sync::atomic::Ordering::Release);
    }
    pub fn is_suspended(&self) -> bool {
        self.suspended.load(std::sync::atomic::Ordering::Acquire)
    }
    /// Explicit lifecycle action only. Existing sessions are stopped by the
    /// caller before replacing a host binary; transport recovery never calls it.
    pub fn start_or_update(&self, update: bool) -> Result<(), String> {
        self.resume();
        self.disconnect();
        let _ = fs::remove_file(self.directory.join("connected"));
        if update {
            provision(&self.host, self.secret.as_ref(), true, &self.runtime_scope)?;
        }
        self.endpoint().map(|_| ())
    }

    pub fn shared(
        host: SshHost,
        supplied: Option<String>,
        remember: bool,
    ) -> Result<Arc<Self>, String> {
        use sha2::{Digest, Sha256};
        static CONNECTIONS: OnceLock<
            Mutex<std::collections::HashMap<String, Weak<RemoteConnection>>>,
        > = OnceLock::new();
        let binding = host.credential_account()?;
        let entry =
            keyring::Entry::new("com.dragent.remote", &binding).map_err(|e| e.to_string())?;
        let secret = supplied
            .filter(|s| !s.is_empty())
            .or_else(|| entry.get_password().ok());
        let key = format!(
            "{}:{}:{}:{:x}",
            runtime_scope(),
            binding,
            serde_json::to_string(&host).map_err(|e| e.to_string())?,
            Sha256::digest(secret.as_deref().unwrap_or_default().as_bytes())
        );
        let mut registry = CONNECTIONS
            .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
            .lock()
            .map_err(|e| e.to_string())?;
        registry.retain(|_, value| value.strong_count() > 0);
        if let Some(connection) = registry.get(&key).and_then(Weak::upgrade) {
            if remember {
                if let Some(secret) = &secret {
                    entry.set_password(secret).map_err(|e| e.to_string())?;
                }
            }
            return Ok(connection);
        }
        let connection = Arc::new(Self::connect_with_secret(host, secret, remember)?);
        registry.insert(key, Arc::downgrade(&connection));
        Ok(connection)
    }

    pub fn forward_port(self: &Arc<Self>, remote: u16, local: u16) -> Result<PortForward, String> {
        if remote == 0 || local == 0 {
            return Err("Ports must be between 1 and 65535".into());
        }
        self.endpoint()?;
        let mut forwards = self.forwards.lock().map_err(|e| e.to_string())?;
        if forwards.contains_key(&local) {
            return Err("This local port is already forwarded".into());
        }
        let probe = std::net::TcpListener::bind(("127.0.0.1", local))
            .map_err(|e| format!("Local port unavailable: {e}"))?;
        drop(probe);
        let mut command = self.host.command()?;
        command
            .arg("-S")
            .arg(self.directory.join("control"))
            .args([
                "-O",
                "forward",
                "-L",
                &format!("127.0.0.1:{local}:127.0.0.1:{remote}"),
            ])
            .arg("--")
            .arg(&self.host.host);
        let output = bounded_output(command, Duration::from_secs(10))?;
        if !output.status.success() {
            return Err(format!(
                "Port forwarding failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        forwards.insert(local, remote);
        Ok(PortForward {
            connection: self.clone(),
            remote,
            local,
        })
    }
    fn cancel_forward(&self, remote: u16, local: u16) {
        if let Ok(mut forwards) = self.forwards.lock() {
            forwards.remove(&local);
        }
        if let Ok(mut command) = self.host.command() {
            command
                .arg("-S")
                .arg(self.directory.join("control"))
                .args([
                    "-O",
                    "cancel",
                    "-L",
                    &format!("127.0.0.1:{local}:127.0.0.1:{remote}"),
                ])
                .arg("--")
                .arg(&self.host.host);
            let _ = bounded_output(command, Duration::from_secs(2));
        }
    }
}
impl Drop for RemoteConnection {
    fn drop(&mut self) {
        self.disconnect();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Debug)]
pub struct PortForward {
    connection: Arc<RemoteConnection>,
    pub remote: u16,
    pub local: u16,
}
impl PortForward {
    pub fn alive(&mut self) -> bool {
        self.connection.tunnel.lock().is_ok_and(|mut t| {
            t.as_mut()
                .is_some_and(|t| t.child.try_wait().is_ok_and(|s| s.is_none()))
        })
    }
}
impl Drop for PortForward {
    fn drop(&mut self) {
        self.connection.cancel_forward(self.remote, self.local);
    }
}

#[cfg(all(test, unix))]
mod provision_tests {
    use super::*;

    fn run(script: &str, home: &std::path::Path) -> std::process::Output {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", home)
            .output()
            .unwrap()
    }

    #[test]
    fn organisation_runtimes_have_separate_storage_and_endpoints() {
        let a = provision_script_scoped("https://example.invalid", "binary", false, false, "/org-a");
        let b = provision_script_scoped("https://example.invalid", "binary", false, false, "/org-b");
        assert!(a.contains(&format!("runtime-v{PROTOCOL_VERSION}/org-a")));
        assert!(b.contains(&format!("runtime-v{PROTOCOL_VERSION}/org-b")));
        assert_ne!(a, b);
        assert!(a.contains("DRAGENT_HOST_ENDPOINT_FILE=\"$DRAGENT_HOME/host-v1.json\""));
    }

    #[test]
    fn the_install_script_parses() {
        for (managed, update) in [(true, false), (true, true), (false, false)] {
            let script = provision_script(
                "https://example.invalid",
                "\"$HOME/bin/dragent-host\"",
                managed,
                update,
            );
            let status = Command::new("sh")
                .arg("-n")
                .arg("-c")
                .arg(&script)
                .status()
                .unwrap();
            assert!(status.success(), "managed={managed} update={update}");
        }
    }

    #[test]
    fn an_unpublished_runtime_says_so() {
        let home = std::env::temp_dir().join(format!("dragent-provision-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // Nothing listens on port 9: the download fails at once.
        let script = provision_script(
            "https://127.0.0.1:9",
            &format!("\"$HOME/.dragent/remote/bin/v{PROTOCOL_VERSION}/dragent-host\""),
            true,
            false,
        );
        let output = run(&script, &home);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        if cfg!(target_os = "linux") && stderr.contains("glibc") {
            // An old or musl test machine stops at the glibc check instead.
            assert!(stderr.contains("Dragent needs"), "{stderr}");
        } else {
            assert!(
                stderr.contains("Couldn't download Dragent's workspace runtime"),
                "{stderr}"
            );
        }
        // The failed download leaves nothing half-installed.
        let bin = home.join(format!(".dragent/remote/bin/v{PROTOCOL_VERSION}"));
        assert!(!bin.join("dragent-host").exists());
        assert_eq!(
            std::fs::read_dir(&bin).unwrap().count(),
            0,
            "staging removed"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    /// The runtime's folders are private, but what it starts (terminals,
    /// installs) must not inherit that: a `sudo npm install -g` would leave a
    /// tool only root can run. A stand-in runtime prints the umask it got.
    #[test]
    fn the_runtime_starts_with_the_login_umask() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("dragent-umask-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let runtime = home.join("runtime");
        std::fs::write(&runtime, "#!/bin/sh\numask\n").unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = provision_script(
            "https://example.invalid",
            &quote(&runtime.to_string_lossy()),
            false,
            false,
        );
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("umask 022\n{script}"))
            .env("HOME", &home)
            .env_remove("SHELL")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .trim_start_matches('0'),
            "22"
        );
        let private = home.join(format!(".dragent/remote/runtime-v{PROTOCOL_VERSION}"));
        assert_eq!(
            std::fs::metadata(private).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    /// Linux machines are checked for glibc before downloading. Stand-in
    /// `uname` and `getconf` commands play the remote Linux machine.
    #[test]
    fn linux_glibc_is_checked() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("dragent-glibc-{}", std::process::id()));
        let fake = root.join("bin");
        std::fs::create_dir_all(&fake).unwrap();
        let tool = |name: &str, body: &str| {
            let path = fake.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        tool(
            "uname",
            "case \"$1\" in -s) echo Linux;; -m) echo x86_64;; esac",
        );
        let script = provision_script(
            "https://127.0.0.1:9",
            &format!("\"$HOME/.dragent/remote/bin/v{PROTOCOL_VERSION}/dragent-host\""),
            true,
            false,
        );
        let path = format!("{}:{}", fake.display(), std::env::var("PATH").unwrap());
        let attempt = |getconf: &str| {
            tool("getconf", getconf);
            let home = root.join("home");
            std::fs::create_dir_all(&home).unwrap();
            let output = Command::new("sh")
                .arg("-c")
                .arg(&script)
                .env("HOME", &home)
                .env("PATH", &path)
                .output()
                .unwrap();
            assert!(!output.status.success());
            String::from_utf8_lossy(&output.stderr).into_owned()
        };
        let old = attempt("echo glibc 2.28");
        assert!(
            old.contains("needs glibc 2.31 or newer") && old.contains("has 2.28"),
            "{old}"
        );
        let musl = attempt("exit 1");
        assert!(musl.contains("musl"), "{musl}");
        for supported in ["echo glibc 2.31", "echo glibc 2.39", "echo glibc 3.0"] {
            let stderr = attempt(supported);
            assert!(
                stderr.contains("Couldn't download"),
                "{supported}: {stderr}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fingerprints_match_ssh_keygen() {
        // `ssh-keygen -lf` on this line: SHA256:PG0gcmnoyJK2ctmEtV4geQChmwwxowNiinqbHXR7jBs (ED25519)
        let line = "example.com ssh-ed25519 \
            AAAAC3NzaC1lZDI1NTE5AAAAIB/jt+k98sKTJ7Qyu4KVWxWqHh2zaFeBOYXJSqkJUfEN";
        assert_eq!(
            fingerprint(line).as_deref(),
            Some("ED25519 SHA256:PG0gcmnoyJK2ctmEtV4geQChmwwxowNiinqbHXR7jBs")
        );
        assert_eq!(fingerprint("example.com ssh-ed25519 not-base64!"), None);
        assert_eq!(fingerprint(""), None);
    }

    #[test]
    fn host_key_failures_are_told_apart() {
        let unknown = "No ED25519 host key is known for 10.0.0.5 and you have requested strict checking.\r\nHost key verification failed.";
        assert_eq!(host_key_problem(unknown), Some(HostKeyProblem::Unknown));
        let changed = "@@@@@@@@@@@\r\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\r\nHost key verification failed.";
        assert_eq!(host_key_problem(changed), Some(HostKeyProblem::Changed));
        assert_eq!(host_key_problem("Connection refused"), None);
    }

    #[test]
    fn strict_host_keys_are_the_default() {
        let host = SshHost {
            host: "example.com".into(),
            name: String::new(),
            proxy_jump: None,
            user: None,
            port: None,
            identity_file: None,
            config_file: None,
            forward_agent: false,
            authentication: Authentication::Agent,
        };
        let command = host.command().unwrap();
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|arg| arg == "StrictHostKeyChecking=yes"));
        assert!(!args.iter().any(|arg| arg.contains("accept-new")));
        assert_eq!(ssh_config_word("/tmp/a b/known_hosts"), "\"/tmp/a b/known_hosts\"");
    }
}
