//! Machine-local CLI provider accounts. This store never contains credentials.
//! Native CLIs own authentication inside private, independent directories.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::{Path, PathBuf}, process::Command};
use uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Selection {
    #[default]
    Inherit,
    System,
    Profile(Uuid),
}
impl Selection {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "inherit" => Ok(Self::Inherit),
            "system" => Ok(Self::System),
            _ => value.parse().map(Self::Profile).map_err(|_| "Use inherit, system, or an account ID".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capability {
    pub isolated: bool,
    pub native_login: bool,
    pub native_status: bool,
    pub same_profile_resume: bool,
    pub cross_account_resume: bool,
    pub reason: String,
    pub source: String,
}
/// Unknown/custom providers are deliberately system-only, never inferred from
/// a settings directory or from another provider's environment variable.
pub fn capability(provider: &str) -> Capability {
    let (supported, reason, source) = match provider {
        "claude" => (true, "claude.ai accounts only. Keyless Console sign-ins are not isolated. Requires a compatible installed CLI; authenticate explicitly before use.", "https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts"),
        "codex" => (true, "Native CODEX_HOME isolation. Authentication and history remain in the chosen profile. Administrator authentication requirements still apply.", "https://developers.openai.com/codex/auth"),
        "opencode" => (false, "A config-directory override does not isolate auth.json. Independent credential and inherited-provider isolation has not been verified.", "https://opencode.ai/docs/providers/"),
        "gemini" => (false, "OAuth, API-key and Vertex credentials have different storage. Complete credential and policy isolation has not been verified.", "https://geminicli.com/docs/get-started/authentication/"),
        "agent" | "cursor-agent" => (false, "Native browser login and API-key authentication are documented; independent browser credential storage has not been verified.", "https://cursor.com/docs/cli/reference/authentication"),
        "muse" => (false, "Independent native credential storage has not been verified against accessible primary documentation.", "https://muse.meta.com/"),
        _ => (false, "No verified account adapter. Existing system authentication remains available.", ""),
    };
    // Owner-only directories: POSIX modes on unix, a protected DACL on Windows.
    let private = cfg!(any(unix, windows));
    Capability { isolated: supported && private, native_login: supported && private, native_status: supported && private, same_profile_resume: supported && private, cross_account_resume: false, reason: if !private && supported { "Private profile ACL support is not implemented on this platform; use system authentication.".into() } else { reason.into() }, source: source.into() }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    pub provider: String,
    pub label: String,
    pub machine_id: Uuid,
}
/// Immutable selection captured with a conversation. Missing profiles fail
/// closed; the label is a launch-time snapshot, never a claim about identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub selection: Selection,
    pub provider: String,
    pub machine_id: Option<Uuid>,
    pub label: String,
}
impl Binding {
    pub fn system(provider: &str) -> Self {
        Self { selection: Selection::System, provider: provider.into(), machine_id: None, label: "System login".into() }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    pub machine_id: Option<Uuid>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub defaults: BTreeMap<String, Selection>,
    /// Canonical project root -> provider -> selection. Local to this host.
    #[serde(default)]
    pub projects: BTreeMap<PathBuf, BTreeMap<String, Selection>>,
}
pub(crate) fn root() -> PathBuf { dragent_paths::home().join("agent-accounts") }
fn io_error(_: impl std::fmt::Display) -> String { "Could not access the private agent account store".into() }
fn private_dir(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    return windows_private::private_dir(path);
    #[cfg(not(any(unix, windows)))]
    return Err("Private account directories are not supported on this platform yet".into());
    #[cfg(unix)] {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(io_error)?;
        let meta = fs::symlink_metadata(path).map_err(io_error)?;
        if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
            return Err("Account directory ownership is invalid".into());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)
    }
}

/// Windows' 0o700: a protected DACL that only this account (and SYSTEM)
/// can use, inherited by everything the CLIs write inside.
#[cfg(windows)]
mod windows_private {
    use super::io_error;
    use std::{ffi::c_void, fs, os::windows::ffi::OsStrExt, path::Path};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
            },
            EqualSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetTokenInformation,
            TokenOwner, TokenUser, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
            SE_DACL_PROTECTED, TOKEN_INFORMATION_CLASS, TOKEN_QUERY,
        },
        Storage::FileSystem::CreateDirectoryW,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// Frees a `LocalAlloc`ed descriptor when dropped.
    struct Local(PSECURITY_DESCRIPTOR);
    impl Drop for Local {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { LocalFree(self.0) };
            }
        }
    }

    /// One `TOKEN_USER`/`TOKEN_OWNER` of this process; both start with a
    /// pointer to the SID, which lives further on in this same buffer.
    fn token_sid(class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>, String> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io_error("token"));
            }
            let mut len = 0u32;
            GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut len);
            // usize-aligned, as the SID pointer inside requires.
            let mut buffer = vec![0usize; (len as usize).div_ceil(size_of::<usize>())];
            let ok = GetTokenInformation(token, class, buffer.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(io_error("token"));
            }
            Ok(buffer)
        }
    }

    /// The SID a `token_sid` buffer points at. The buffer must outlive it.
    fn sid_of(buffer: &[usize]) -> *mut c_void {
        buffer[0] as *mut c_void
    }

    /// Owner-only, inheritable, protected from the parent's ACEs.
    fn descriptor(user: *mut c_void) -> Result<Local, String> {
        unsafe {
            let mut sid = std::ptr::null_mut::<u16>();
            if ConvertSidToStringSidW(user, &mut sid) == 0 {
                return Err(io_error("sid"));
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, (0..).take_while(|&i| *sid.add(i) != 0).count()));
            LocalFree(sid.cast());
            let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{text})(A;OICI;FA;;;SY)").encode_utf16().chain(Some(0)).collect();
            let mut descriptor = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()) == 0 {
                return Err(io_error("sddl"));
            }
            Ok(Local(descriptor))
        }
    }

    pub(super) fn private_dir(path: &Path) -> Result<(), String> {
        let user = token_sid(TokenUser)?;
        let owner = token_sid(TokenOwner)?;
        let descriptor = descriptor(sid_of(&user))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let name = wide(path);
        unsafe {
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: 0,
            };
            // Created private from the start; an existing one is checked below.
            CreateDirectoryW(name.as_ptr(), &attributes);
        }
        let meta = fs::symlink_metadata(path).map_err(io_error)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err("Account directory ownership is invalid".into());
        }
        unsafe {
            let mut found_owner = std::ptr::null_mut();
            let mut current = std::ptr::null_mut();
            if GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut found_owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut current,
            ) != ERROR_SUCCESS
            {
                return Err(io_error("security"));
            }
            let current = Local(current);
            // New objects are owned by the token's default owner, which is the
            // Administrators group in an elevated session.
            if EqualSid(found_owner, sid_of(&user)) == 0 && EqualSid(found_owner, sid_of(&owner)) == 0 {
                return Err("Account directory ownership is invalid".into());
            }
            let mut control = 0u16;
            let mut revision = 0u32;
            GetSecurityDescriptorControl(current.0, &mut control, &mut revision);
            if control & SE_DACL_PROTECTED != 0 {
                return Ok(());
            }
            // Made before this was private (or by hand): lock it down once,
            // which also reaches what is already inside.
            let (mut present, mut defaulted) = (0, 0);
            let mut dacl: *mut ACL = std::ptr::null_mut();
            if GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) == 0 {
                return Err(io_error("dacl"));
            }
            if SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            ) != ERROR_SUCCESS
            {
                return Err(io_error("dacl"));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    #[test]
    fn makes_an_owner_only_directory() {
        let dir = std::env::temp_dir().join(format!("dragent-private-{}", uuid::Uuid::new_v4()));
        private_dir(&dir.join("inner")).unwrap();
        // Idempotent: the second call only checks.
        private_dir(&dir.join("inner")).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut opts = fs::OpenOptions::new(); opts.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
    let result = (|| {
        let mut file = opts.open(&tmp).map_err(io_error)?;
        file.write_all(&serde_json::to_vec_pretty(value).map_err(io_error)?).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(&tmp, path).map_err(io_error)
    })();
    if result.is_err() { let _ = fs::remove_file(tmp); }
    result
}
impl Registry {
    pub fn load() -> Result<Self, String> {
        match fs::read(root().join("accounts.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "Agent account metadata is invalid; launch refused".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(io_error(e)),
        }
    }
    pub fn update<T>(change: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        private_dir(&root())?;
        let mut options = fs::OpenOptions::new(); options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600).custom_flags(libc::O_NOFOLLOW); }
        let lock = options.open(root().join("accounts.lock")).map_err(io_error)?;
        lock.lock().map_err(io_error)?;
        let mut registry = Self::load()?;
        registry.machine_id.get_or_insert_with(Uuid::new_v4);
        let result = change(&mut registry)?;
        write_json(&root().join("accounts.json"), &registry)?;
        Ok(result)
    }
    pub fn add(&mut self, provider: &str, label: &str) -> Result<Profile, String> {
        let cap = capability(provider);
        if !cap.isolated { return Err(cap.reason); }
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
            return Err("Account names must contain 1–64 characters without control characters".into());
        }
        if self.profiles.iter().any(|p| p.provider == provider && p.label.eq_ignore_ascii_case(label)) {
            return Err("An account with this name already exists for this agent".into());
        }
        let profile = Profile { id: Uuid::new_v4(), provider: provider.into(), label: label.into(), machine_id: *self.machine_id.get_or_insert_with(Uuid::new_v4) };
        private_dir(&root().join(profile.id.to_string()))?;
        self.profiles.push(profile.clone());
        Ok(profile)
    }
    pub fn remove(&mut self, id: Uuid) -> Result<(), String> {
        let index = self.profiles.iter().position(|p| p.id == id).ok_or("Account no longer exists")?;
        self.profiles.remove(index);
        // Keep dangling defaults and bindings so they fail closed, and retain
        // provider-owned history/credentials. Never sign out a running session.
        Ok(())
    }
    pub fn rename(&mut self, id: Uuid, label: &str) -> Result<(), String> {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) { return Err("Enter an account name of 1–64 characters".into()); }
        let profile = self.profiles.iter_mut().find(|p| p.id == id).ok_or("Account no longer exists")?;
        profile.label = label.into(); Ok(())
    }
    pub fn resolve(&self, provider: &str, project: Option<&Path>, choice: &Selection) -> Result<Binding, String> {
        let project_choice = project
            .and_then(|p| {
                self.projects.get(p).or_else(|| {
                    p.canonicalize().ok().and_then(|c| self.projects.get(&c))
                })
            })
            .and_then(|p| p.get(provider));
        let selection = [Some(choice), project_choice, self.defaults.get(provider)].into_iter().flatten().find(|s| **s != Selection::Inherit).cloned().unwrap_or(Selection::System);
        match selection {
            Selection::Profile(id) => {
                let p = self.profiles.iter().find(|p| p.id == id).ok_or("Selected agent account was removed. Choose an account explicitly.")?;
                if p.provider != provider || Some(p.machine_id) != self.machine_id { return Err("Account belongs to a different provider or machine".into()); }
                if !capability(provider).isolated { return Err(capability(provider).reason); }
                Ok(Binding { selection: Selection::Profile(id), provider: provider.into(), machine_id: Some(p.machine_id), label: p.label.clone() })
            }
            _ => Ok(Binding::system(provider)),
        }
    }
    pub fn validate(&self, binding: &Binding) -> Result<(), String> {
        let resolved = self.resolve(&binding.provider, None, &binding.selection)?;
        if resolved.machine_id != binding.machine_id { return Err("Account belongs to another machine; authenticate on the execution host".into()); }
        Ok(())
    }
    pub fn set_default(&mut self, provider: &str, project: Option<&Path>, selection: Selection) -> Result<(), String> {
        self.resolve(provider, None, &selection)?;
        let target = match project { Some(path) => self.projects.entry(path.canonicalize().map_err(io_error)?).or_default(), None => &mut self.defaults };
        target.insert(provider.into(), selection); Ok(())
    }
}
/// Process-local changes only. No global HOME changes and no credential copy.
pub fn configure(command: &mut Command, binding: &Binding) -> Result<(), String> {
    if binding.selection == Selection::System { return Ok(()); }
    Registry::load()?.validate(binding)?;
    let Selection::Profile(id) = binding.selection else { return Err("Unresolved account selection".into()); };
    let dir = root().join(id.to_string());
    if !dir.is_dir() { return Err("Account storage is missing; launch refused".into()); }
    private_dir(&dir)?;
    // Plain `C:\…` on Windows, not `\\?\C:\…`: the CLIs report this path
    // back and the isolation check compares it.
    let dir = dunce::canonicalize(dir).map_err(io_error)?;
    match binding.provider.as_str() {
        "claude" => {
            // Routing/provider overrides are rejected instead of bypassing
            // an administrator's cloud/gateway requirements.
            for key in ["CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY", "ANTHROPIC_BASE_URL"] {
                if std::env::var_os(key).is_some() { return Err("Selected Claude profile conflicts with inherited provider routing; use system login or a separately configured host".into()); }
            }
            for key in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR", "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR"] { command.env_remove(key); }
            command.env("CLAUDE_CONFIG_DIR", dir);
        }
        "codex" => {
            for key in ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_AUTH_JSON"] { command.env_remove(key); }
            command.env("CODEX_HOME", dir);
        }
        _ => return Err("Account adapter unavailable".into()),
    }
    Ok(())
}
/// Metadata-only lookup used by the host helper. Git worktrees map back to
/// their common repository; no tokens or histories are inspected.
pub fn project_for(cwd: &Path, registry: &Registry) -> Option<PathBuf> {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if let Some(path) = canonical.ancestors().find(|p| registry.projects.contains_key(*p)) {
        return Some(path.to_owned());
    }
    if let Some(path) = cwd.ancestors().find(|p| registry.projects.contains_key(*p)) { return Some(path.to_owned()); }
    let output = Command::new("git").args(["rev-parse", "--path-format=absolute", "--git-common-dir"]).current_dir(cwd).stderr(std::process::Stdio::null()).output().ok()?;
    if !output.status.success() { return None; }
    let common = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    common.parent().and_then(|p| p.canonicalize().ok()).filter(|p| registry.projects.contains_key(p))
}

/// A bounded native probe. Output is never logged, persisted or returned to UI.
fn probe(mut command: Command) -> Result<(bool, Vec<u8>), String> {
    use std::{io::Read, process::Stdio, time::{Duration, Instant}};
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| "Agent CLI is unavailable on this host")?;
    let stdout = child.stdout.take().ok_or("Agent status output unavailable")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // Drain after the cap so an unexpectedly verbose CLI cannot deadlock.
        let mut stdout = stdout; let mut chunk = [0; 4096];
        while let Ok(n) = stdout.read(&mut chunk) { if n == 0 { break; } if bytes.len() < 65536 { bytes.extend_from_slice(&chunk[..n]); } }
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok((status.success(), reader.join().unwrap_or_default())),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            _ => { let _ = child.kill(); let _ = child.wait(); return Err("Agent status check timed out or failed".into()); }
        }
    }
}
fn native_command(binding: &Binding, executable: &Path) -> Result<Command, String> {
    // npm installs agents as `.cmd` shims on Windows, which a bare name misses.
    let mut command = Command::new(crate::agent_sessions::program(&executable.to_string_lossy()));
    configure(&mut command, binding)?;
    Ok(command)
}
pub fn status(binding: &Binding, executable: &Path) -> Result<&'static str, String> {
    if binding.selection == Selection::System { return Ok("System login — managed by the CLI"); }
    let mut command = native_command(binding, executable)?;
    match binding.provider.as_str() {
        "claude" => { command.args(["auth", "status"]); }
        "codex" => { command.args(["login", "status"]); }
        _ => return Err("Native status unavailable".into()),
    }
    let (success, bytes) = probe(command)?;
    if !success { return Ok("Sign-in required"); }
    if binding.provider == "claude" {
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "Installed Claude CLI cannot verify profile isolation; update Claude Code")?;
        let Selection::Profile(id) = binding.selection else { return Err("Account selection is unresolved".into()); };
        let expected = dunce::canonicalize(root().join(id.to_string())).map_err(io_error)?;
        if value.get("configDirectory").and_then(|v| v.as_str()).map(PathBuf::from).as_ref() != Some(&expected) {
            return Err("Claude profile isolation could not be verified; Claude Code 2.1.268 or later is required".into());
        }
        if value.get("authMethod").and_then(|v| v.as_str()) != Some("claude.ai") {
            return Err("This profile requires a claude.ai login. Console, external tokens and API helpers are not supported".into());
        }
    }
    Ok("Signed in")
}
/// Who an account is signed in as, as the agent itself reports it: shown on
/// the person's own accounts page, never logged or sent anywhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub signed_in: bool,
    pub email: Option<String>,
    /// The subscription the agent reports ("max", "pro", "plus"…).
    pub plan: Option<String>,
}

/// Check an account, the system login included: whether it's signed in and
/// as whom. For a profile this applies the same isolation checks as
/// [`status`].
pub fn identity(binding: &Binding, executable: &Path) -> Result<Identity, String> {
    match binding.provider.as_str() {
        "claude" => {
            let mut command = native_command(binding, executable)?;
            command.args(["auth", "status"]);
            let (success, bytes) = probe(command)?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            if let Selection::Profile(_) = binding.selection {
                // The isolation checks, on the same answer.
                status(binding, executable)?;
            }
            let text = |key: &str| value.get(key).and_then(|v| v.as_str()).filter(|v| !v.is_empty()).map(str::to_owned);
            let signed_in = success && value.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(success);
            Ok(Identity { signed_in, email: text("email"), plan: text("subscriptionType").or_else(|| text("planTier")) })
        }
        "codex" => {
            let mut command = native_command(binding, executable)?;
            command.args(["login", "status"]);
            let (success, _) = probe(command)?;
            let account = match binding.selection { Selection::Profile(id) => id.to_string(), _ => "system".into() };
            let claims = crate::agent_usage::codex_home(&account)
                .and_then(|home| fs::read(home.join("auth.json")).ok())
                .and_then(|bytes| codex_claims(&bytes))
                .unwrap_or_default();
            Ok(Identity { signed_in: success, ..claims })
        }
        _ => Err("Native status unavailable".into()),
    }
}

/// The email and plan in a Codex `auth.json`'s sign-in token (its public
/// claims; nothing secret is read out).
fn codex_claims(auth: &[u8]) -> Option<Identity> {
    use base64::Engine as _;
    let value: serde_json::Value = serde_json::from_slice(auth).ok()?;
    let token = value.pointer("/tokens/id_token")?.as_str()?;
    let payload = token.split('.').nth(1)?;
    let claims: serde_json::Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()?;
    let email = claims.get("email").and_then(|v| v.as_str()).map(str::to_owned);
    let plan = claims.pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type").and_then(|v| v.as_str()).map(str::to_owned);
    Some(Identity { signed_in: false, email, plan })
}

pub fn login(id: Uuid) -> Result<i32, String> {
    let registry = Registry::load()?;
    let profile = registry.profiles.iter().find(|p| p.id == id).ok_or("Account no longer exists")?;
    let binding = registry.resolve(&profile.provider, None, &Selection::Profile(id))?;
    let mut command = native_command(&binding, Path::new(&profile.provider))?;
    match profile.provider.as_str() {
        "claude" => { command.args(["auth", "login"]); }
        "codex" => { command.arg("login"); }
        _ => return Err("Native sign-in unavailable".into()),
    }
    command.status().map(|s| s.code().unwrap_or(1)).map_err(|_| "Could not start native sign-in".into())
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn codex_claims_read_email_and_plan_only() {
        use base64::Engine as _;
        let claims = serde_json::json!({"email": "me@example.com", "https://api.openai.com/auth": {"chatgpt_plan_type": "plus"}});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let auth = serde_json::json!({"tokens": {"id_token": format!("h.{payload}.s"), "access_token": "secret"}});
        let found = codex_claims(&serde_json::to_vec(&auth).unwrap()).unwrap();
        assert_eq!(found.email.as_deref(), Some("me@example.com"));
        assert_eq!(found.plan.as_deref(), Some("plus"));
        assert!(codex_claims(b"{}").is_none());
    }
}

#[cfg(test)]
#[test]
#[ignore]
fn identity_probe() {
    let registry = Registry::load().unwrap_or_default();
    for provider in ["claude", "codex"] {
        let r = identity(&Binding::system(provider), Path::new(provider));
        println!("{provider} system: {:?}", r.map(|i| (i.signed_in, i.email.is_some(), i.plan)));
        for p in registry.profiles.iter().filter(|p| p.provider == provider) {
            let b = registry.resolve(provider, None, &Selection::Profile(p.id)).unwrap();
            println!("{provider} {}: {:?}", p.label, identity(&b, Path::new(provider)).map(|i| (i.signed_in, i.email.is_some(), i.plan)));
        }
    }
}
