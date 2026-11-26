//! Machine-local CLI provider accounts. This store never contains credentials.
//! Native CLIs own authentication inside private, independent directories.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};
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
            _ => value
                .parse()
                .map(Self::Profile)
                .map_err(|_| "Use inherit, system, or an account ID".into()),
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    #[default]
    Subscription,
    ApiKey,
}

impl CredentialKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "subscription" => Ok(Self::Subscription),
            "api-key" | "api_key" | "apikey" => Ok(Self::ApiKey),
            _ => Err("Use subscription or api-key".into()),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::ApiKey => "api-key",
        }
    }
}

/// Unknown/custom providers are deliberately system-only, never inferred from
/// a settings directory or from another provider's environment variable.
pub fn capability(provider: &str) -> Capability {
    let (supported, reason, source) = match provider {
        "claude" => (
            true,
            "Claude supports claude.ai subscriptions and API keys in isolated profiles. Requires a compatible installed CLI; authenticate explicitly before use.",
            "https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts",
        ),
        "codex" => (
            true,
            "Native CODEX_HOME isolation. Authentication and history remain in the chosen profile. Administrator authentication requirements still apply.",
            "https://developers.openai.com/codex/auth",
        ),
        "opencode" => (
            false,
            "A config-directory override does not isolate auth.json. Independent credential and inherited-provider isolation has not been verified.",
            "https://opencode.ai/docs/providers/",
        ),
        "gemini" => (
            false,
            "OAuth, API-key and Vertex credentials have different storage. Complete credential and policy isolation has not been verified.",
            "https://geminicli.com/docs/get-started/authentication/",
        ),
        "agent" | "cursor-agent" => (
            false,
            "Native browser login and API-key authentication are documented; independent browser credential storage has not been verified.",
            "https://cursor.com/docs/cli/reference/authentication",
        ),
        "muse" => (
            false,
            "Independent native credential storage has not been verified against accessible primary documentation.",
            "https://muse.meta.com/",
        ),
        _ => (
            false,
            "No verified account adapter. Existing system authentication remains available.",
            "",
        ),
    };
    // Owner-only directories: POSIX modes on unix, a protected DACL on Windows.
    let private = cfg!(any(unix, windows));
    Capability {
        isolated: supported && private,
        native_login: supported && private,
        native_status: supported && private,
        same_profile_resume: supported && private,
        cross_account_resume: false,
        reason: if !private && supported {
            "Private profile ACL support is not implemented on this platform; use system authentication.".into()
        } else {
            reason.into()
        },
        source: source.into(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: Uuid,
    pub provider: String,
    pub label: String,
    pub machine_id: Uuid,
    #[serde(default)]
    pub kind: CredentialKind,
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
        Self {
            selection: Selection::System,
            provider: provider.into(),
            machine_id: None,
            label: "System login".into(),
        }
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
pub(crate) fn root() -> PathBuf {
    dragent_paths::home().join("agent-accounts")
}
fn io_error(_: impl std::fmt::Display) -> String {
    "Could not access the private agent account store".into()
}
fn private_dir(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    return windows_private::private_dir(path);
    #[cfg(not(any(unix, windows)))]
    return Err("Private account directories are not supported on this platform yet".into());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(io_error)?;
        let meta = fs::symlink_metadata(path).map_err(io_error)?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
        {
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
        Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree},
        Security::{
            ACL,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
            },
            DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, GetTokenInformation, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
            SECURITY_ATTRIBUTES, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TokenOwner, TokenUser,
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
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(
                sid,
                (0..).take_while(|&i| *sid.add(i) != 0).count(),
            ));
            LocalFree(sid.cast());
            let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{text})(A;OICI;FA;;;SY)")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut descriptor = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
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
            if EqualSid(found_owner, sid_of(&user)) == 0
                && EqualSid(found_owner, sid_of(&owner)) == 0
            {
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
            if GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) == 0
            {
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
    write_private(path, &serde_json::to_vec_pretty(value).map_err(io_error)?)
}

/// Replace a file whole, readable by this account only.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut file = opts.open(&tmp).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(&tmp, path).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

/// POSIX single-quote: `'` becomes `'\''`, everything else stays literal.
pub(crate) fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// PowerShell single-quote: `'` becomes `''`, everything else stays literal.
pub(crate) fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn claude_api_key_helper(dir: &Path) -> String {
    #[cfg(windows)]
    {
        let dir = dir.to_string_lossy();
        format!("type \"{dir}\\api-key\"")
    }
    #[cfg(not(windows))]
    {
        let full = dir.join("api-key").to_string_lossy().into_owned();
        format!("cat {}", posix_quote(&full))
    }
}

fn write_claude_api_helper(dir: &Path) -> Result<(), String> {
    let path = dir.join("settings.json");
    let mut value = match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(serde_json::Value::Null)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
        Err(e) => return Err(io_error(e)),
    };
    if !value.is_object() {
        value = serde_json::json!({});
    }
    value.as_object_mut().expect("object").insert(
        "apiKeyHelper".into(),
        serde_json::Value::String(claude_api_key_helper(dir)),
    );
    write_json(&path, &value)
}
impl Registry {
    pub fn load() -> Result<Self, String> {
        match fs::read(root().join("accounts.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Agent account metadata is invalid; launch refused".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(io_error(e)),
        }
    }
    pub fn update<T>(change: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        private_dir(&root())?;
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options
            .open(root().join("accounts.lock"))
            .map_err(io_error)?;
        lock.lock().map_err(io_error)?;
        let mut registry = Self::load()?;
        registry.machine_id.get_or_insert_with(Uuid::new_v4);
        let result = change(&mut registry)?;
        write_json(&root().join("accounts.json"), &registry)?;
        Ok(result)
    }
    pub fn add(
        &mut self,
        provider: &str,
        label: &str,
        kind: CredentialKind,
    ) -> Result<Profile, String> {
        let cap = capability(provider);
        if !cap.isolated {
            return Err(cap.reason);
        }
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
            return Err(
                "Account names must contain 1–64 characters without control characters".into(),
            );
        }
        if self
            .profiles
            .iter()
            .any(|p| p.provider == provider && p.label.eq_ignore_ascii_case(label))
        {
            return Err("An account with this name already exists for this agent".into());
        }
        let profile = Profile {
            id: Uuid::new_v4(),
            provider: provider.into(),
            label: label.into(),
            machine_id: *self.machine_id.get_or_insert_with(Uuid::new_v4),
            kind,
        };
        let dir = root().join(profile.id.to_string());
        private_dir(&dir)?;
        if provider == "claude" && kind == CredentialKind::ApiKey {
            let canonical = dunce::canonicalize(&dir).map_err(io_error)?;
            write_claude_api_helper(&canonical)?;
        }
        self.profiles.push(profile.clone());
        Ok(profile)
    }
    pub fn remove(&mut self, id: Uuid) -> Result<(), String> {
        let index = self
            .profiles
            .iter()
            .position(|p| p.id == id)
            .ok_or("Account no longer exists")?;
        self.profiles.remove(index);
        // Keep dangling defaults and bindings so they fail closed, and retain
        // provider-owned history/credentials. Never sign out a running session.
        Ok(())
    }
    pub fn rename(&mut self, id: Uuid, label: &str) -> Result<(), String> {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
            return Err("Enter an account name of 1–64 characters".into());
        }
        let index = self
            .profiles
            .iter()
            .position(|p| p.id == id)
            .ok_or("Account no longer exists")?;
        // Same rule as `add`: no two accounts of one agent share a name
        // (case-insensitive), excluding the account being renamed.
        let provider = self.profiles[index].provider.clone();
        if self
            .profiles
            .iter()
            .any(|p| p.id != id && p.provider == provider && p.label.eq_ignore_ascii_case(label))
        {
            return Err("An account with this name already exists for this agent".into());
        }
        self.profiles[index].label = label.into();
        Ok(())
    }
    pub fn resolve(
        &self,
        provider: &str,
        project: Option<&Path>,
        choice: &Selection,
    ) -> Result<Binding, String> {
        let project_choice = project
            .and_then(|p| {
                self.projects
                    .get(p)
                    .or_else(|| p.canonicalize().ok().and_then(|c| self.projects.get(&c)))
            })
            .and_then(|p| p.get(provider));
        let selection = [Some(choice), project_choice, self.defaults.get(provider)]
            .into_iter()
            .flatten()
            .find(|s| **s != Selection::Inherit)
            .cloned()
            .unwrap_or(Selection::System);
        match selection {
            Selection::Profile(id) => {
                let p =
                    self.profiles.iter().find(|p| p.id == id).ok_or(
                        "Selected agent account was removed. Choose an account explicitly.",
                    )?;
                if p.provider != provider || Some(p.machine_id) != self.machine_id {
                    return Err("Account belongs to a different provider or machine".into());
                }
                if !capability(provider).isolated {
                    return Err(capability(provider).reason);
                }
                Ok(Binding {
                    selection: Selection::Profile(id),
                    provider: provider.into(),
                    machine_id: Some(p.machine_id),
                    label: p.label.clone(),
                })
            }
            _ => Ok(Binding::system(provider)),
        }
    }
    pub fn validate(&self, binding: &Binding) -> Result<(), String> {
        let resolved = self.resolve(&binding.provider, None, &binding.selection)?;
        if resolved.machine_id != binding.machine_id {
            return Err(
                "Account belongs to another machine; authenticate on the execution host".into(),
            );
        }
        Ok(())
    }
    pub fn set_default(
        &mut self,
        provider: &str,
        project: Option<&Path>,
        selection: Selection,
    ) -> Result<(), String> {
        self.resolve(provider, None, &selection)?;
        let target = match project {
            Some(path) => self
                .projects
                .entry(path.canonicalize().map_err(io_error)?)
                .or_default(),
            None => &mut self.defaults,
        };
        target.insert(provider.into(), selection);
        Ok(())
    }
}
/// The environment a remote launch needs for an account: variables to set
/// and variables to remove. No credential ever appears here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchEnv {
    pub set: Vec<(String, String)>,
    pub unset: Vec<String>,
}

/// The validated launch environment for a binding: the same checks and the
/// same refusals [`configure`] applies.
pub fn launch_env(binding: &Binding) -> Result<LaunchEnv, String> {
    if binding.selection == Selection::System {
        return Ok(LaunchEnv::default());
    }
    Registry::load()?.validate(binding)?;
    let Selection::Profile(id) = binding.selection else {
        return Err("Unresolved account selection".into());
    };
    let dir = root().join(id.to_string());
    if !dir.is_dir() {
        return Err("Account storage is missing; launch refused".into());
    }
    private_dir(&dir)?;
    // Plain `C:\…` on Windows, not `\\?\C:\…`: the CLIs report this path
    // back and the isolation check compares it.
    let dir = dunce::canonicalize(dir).map_err(io_error)?;
    let dir = dir.to_string_lossy().into_owned();
    match binding.provider.as_str() {
        "claude" => {
            // Routing/provider overrides are rejected instead of bypassing
            // an administrator's cloud/gateway requirements.
            for key in [
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
                "ANTHROPIC_BASE_URL",
            ] {
                if std::env::var_os(key).is_some() {
                    return Err("Selected Claude profile conflicts with inherited provider routing; use system login or a separately configured host".into());
                }
            }
            Ok(LaunchEnv {
                set: vec![("CLAUDE_CONFIG_DIR".into(), dir)],
                unset: [
                    "ANTHROPIC_API_KEY",
                    "ANTHROPIC_AUTH_TOKEN",
                    "CLAUDE_CODE_OAUTH_TOKEN",
                    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
                    "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            })
        }
        "codex" => Ok(LaunchEnv {
            set: vec![("CODEX_HOME".into(), dir)],
            unset: ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_AUTH_JSON"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }),
        _ => Err("Account adapter unavailable".into()),
    }
}

/// Prefix a POSIX remote launch line, e.g.
/// `env -u ANTHROPIC_API_KEY CLAUDE_CONFIG_DIR='…' ` (trailing space).
/// Empty when there is nothing to set or unset.
pub fn posix_prefix(env: &LaunchEnv) -> String {
    if env.set.is_empty() && env.unset.is_empty() {
        return String::new();
    }
    let mut out = String::from("env");
    for key in &env.unset {
        out.push_str(" -u ");
        out.push_str(key);
    }
    for (key, value) in &env.set {
        out.push(' ');
        out.push_str(key);
        out.push('=');
        out.push_str(&posix_quote(value));
    }
    out.push(' ');
    out
}

/// Process-local changes only. No global HOME changes and no credential copy.
pub fn configure(command: &mut Command, binding: &Binding) -> Result<(), String> {
    let env = launch_env(binding)?;
    for (key, value) in env.set {
        command.env(key, value);
    }
    for key in env.unset {
        command.env_remove(key);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    #[default]
    Posix,
    #[serde(rename = "powershell")]
    PowerShell,
}

/// This host's own shell kind: PowerShell on Windows, POSIX elsewhere.
pub fn host_shell_kind() -> ShellKind {
    #[cfg(windows)]
    return ShellKind::PowerShell;
    #[cfg(not(windows))]
    return ShellKind::Posix;
}

fn profile_kind_for_status(id: Uuid) -> CredentialKind {
    Registry::load()
        .ok()
        .and_then(|r| r.profiles.into_iter().find(|p| p.id == id))
        .map(|p| p.kind)
        .unwrap_or_default()
}

/// Whether a Claude API-key profile's `api-key` file exists and is
/// non-empty. Metadata only: the key itself is never read, logged, or
/// returned.
pub(crate) fn api_key_file_present(profile_dir: &Path) -> bool {
    fs::metadata(profile_dir.join("api-key"))
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// True when `binding` is a Claude API-key profile whose key file is
/// missing or empty, i.e. the person hasn't typed the key yet. The store
/// root is explicit so tests never touch the real account store.
pub(crate) fn claude_key_pending(
    store_root: &Path,
    binding: &Binding,
    registry: &Registry,
) -> bool {
    if binding.provider != "claude" {
        return false;
    }
    let Selection::Profile(id) = binding.selection else {
        return false;
    };
    let kind = registry
        .profiles
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.kind)
        .unwrap_or_default();
    kind == CredentialKind::ApiKey && !api_key_file_present(&store_root.join(id.to_string()))
}

/// An API key on its way to the private account store. Never shown in debug
/// output, and wiped when dropped.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApiKey(String);

impl ApiKey {
    /// A key as the person typed or pasted it. Surrounding whitespace is
    /// dropped; anything that can't be part of a key is refused.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let key = raw.trim();
        if key.is_empty() {
            return Err("Enter an API key".into());
        }
        if key.len() > 1024 || !key.chars().all(|c| c.is_ascii_graphic()) {
            return Err("That doesn't look like an API key. Check for spaces or missing parts.".into());
        }
        Ok(Self(key.to_string()))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(not(target_family = "wasm"))]
impl Drop for ApiKey {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Add an API-key account with its key in one step: when the key can't be
/// stored, no half-made account is left behind.
pub fn add_with_key(provider: &str, label: &str, key: &ApiKey) -> Result<Profile, String> {
    // Keys arrive over the host protocol too, so check them here as well.
    let key = ApiKey::parse(key.expose())?;
    let profile = Registry::update(|r| r.add(provider, label, CredentialKind::ApiKey))?;
    if let Err(error) = save_key(profile.id, &key) {
        let _ = Registry::update(|r| r.remove(profile.id));
        let _ = fs::remove_dir_all(root().join(profile.id.to_string()));
        return Err(error);
    }
    Ok(profile)
}

/// Store (or replace) an API-key account's key.
pub fn save_key(id: Uuid, key: &ApiKey) -> Result<(), String> {
    let key = ApiKey::parse(key.expose())?;
    let registry = Registry::load()?;
    let profile = registry
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or("Account no longer exists")?;
    if profile.kind != CredentialKind::ApiKey {
        return Err("This account signs in with a subscription, not an API key".into());
    }
    match profile.provider.as_str() {
        "claude" => write_private(&root().join(id.to_string()).join("api-key"), key.expose().as_bytes()),
        "codex" => {
            let binding = registry.resolve("codex", None, &Selection::Profile(id))?;
            let mut command = native_command(&binding, Path::new("codex"))?;
            command.args(["login", "--with-api-key"]);
            if feed(command, key.expose().as_bytes())? {
                Ok(())
            } else {
                Err("Codex didn't accept the API key".into())
            }
        }
        _ => Err("API keys aren't available for this agent".into()),
    }
}

/// Run a command with `input` on its stdin; whether it succeeded.
fn feed(mut command: Command, input: &[u8]) -> Result<bool, String> {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Agent CLI is unavailable on this host")?;
    if let Some(mut stdin) = child.stdin.take() {
        // A command that exits without reading reports through its status.
        let _ = stdin.write_all(input);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Saving the API key timed out".into());
            }
        }
    }
}

/// One-line sign-in command the person runs in a terminal on the machine
/// that owns the account. Dragent never sees a key or token: keys are typed
/// into the terminal with echo off. `target` None is the machine's own
/// system login (API keys are not offered there).
pub fn sign_in_command(
    provider: &str,
    target: Option<Uuid>,
    shell: ShellKind,
) -> Result<String, String> {
    match target {
        None => match (provider, shell) {
            ("claude", _) => Ok("claude auth login".into()),
            ("codex", _) => Ok("codex login".into()),
            _ => Err("Native sign-in unavailable".into()),
        },
        Some(id) => {
            let registry = Registry::load()?;
            let profile = registry
                .profiles
                .iter()
                .find(|p| p.id == id)
                .ok_or("Account no longer exists")?;
            if profile.provider != provider {
                return Err("Account belongs to a different provider".into());
            }
            let dir = dunce::canonicalize(root().join(id.to_string())).map_err(io_error)?;
            sign_in_command_for_profile(provider, profile.kind, &dir, shell)
        }
    }
}

/// POSIX API-key sign-in scripts, run as `sh -c '<script>' sh <dir>`: the
/// profile dir arrives as `"$1"`, so these stay constant. They contain no
/// single quotes and no backslashes — only double-quoted prompts — so the
/// outer shell (bash, zsh, dash or fish) only ever quotes them once.
const CLAUDE_API_KEY_POSIX_SCRIPT: &str = "echo \"Type or paste your key, then press Enter. It stays hidden.\"; printf %s \"Anthropic API key (hidden): \"; stty -echo 2>/dev/null; trap \"stty echo 2>/dev/null\" EXIT INT TERM; read -r K; stty echo 2>/dev/null; trap - EXIT INT TERM; echo; [ -n \"$K\" ] || { echo \"API key is empty; sign-in cancelled\" >&2; unset K; exit 1; }; echo \"Received ${#K} characters.\"; umask 077; printf \"%s\" \"$K\" > \"$1/api-key\"; unset K; CLAUDE_CONFIG_DIR=\"$1\" claude auth status >/dev/null && echo Signed in";
const CODEX_API_KEY_POSIX_SCRIPT: &str = "echo \"Type or paste your key, then press Enter. It stays hidden.\"; printf %s \"OpenAI API key (hidden): \"; stty -echo 2>/dev/null; trap \"stty echo 2>/dev/null\" EXIT INT TERM; read -r K; stty echo 2>/dev/null; trap - EXIT INT TERM; echo; [ -n \"$K\" ] || { echo \"API key is empty; sign-in cancelled\" >&2; unset K; exit 1; }; echo \"Received ${#K} characters.\"; printf \"%s\" \"$K\" | CODEX_HOME=\"$1\" codex login --with-api-key; rc=$?; unset K; exit $rc";

pub(crate) fn sign_in_command_for_profile(
    provider: &str,
    kind: CredentialKind,
    dir: &Path,
    shell: ShellKind,
) -> Result<String, String> {
    let dir_str = dir.to_string_lossy().into_owned();
    match (provider, kind, shell) {
        ("claude", CredentialKind::Subscription, ShellKind::Posix) => Ok(format!(
            "CLAUDE_CONFIG_DIR={} claude auth login",
            posix_quote(&dir_str)
        )),
        ("codex", CredentialKind::Subscription, ShellKind::Posix) => {
            Ok(format!("CODEX_HOME={} codex login", posix_quote(&dir_str)))
        }
        ("claude", CredentialKind::Subscription, ShellKind::PowerShell) => Ok(format!(
            "$prevClaudeConfigDir = $env:CLAUDE_CONFIG_DIR; $env:CLAUDE_CONFIG_DIR={dir_q}; claude auth login; if ($null -eq $prevClaudeConfigDir) {{ Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue }} else {{ $env:CLAUDE_CONFIG_DIR = $prevClaudeConfigDir }}",
            dir_q = powershell_quote(&dir_str),
        )),
        ("codex", CredentialKind::Subscription, ShellKind::PowerShell) => Ok(format!(
            "$prevCodexHome = $env:CODEX_HOME; $env:CODEX_HOME={dir_q}; codex login; if ($null -eq $prevCodexHome) {{ Remove-Item Env:CODEX_HOME -ErrorAction SilentlyContinue }} else {{ $env:CODEX_HOME = $prevCodexHome }}",
            dir_q = powershell_quote(&dir_str),
        )),
        // POSIX API-key commands pass the profile dir as an argument —
        // `sh -c '<script>' sh <dir>` — and the script uses `"$1"` for it.
        // The script itself contains no single quotes and no backslashes at
        // all, so the only quoting is one plain `'...'` span around the
        // script plus one `posix_quote` of the dir, which bash, zsh, dash
        // and fish all read the same way. (Quoting the dir inside the
        // script instead broke fish on dirs containing an apostrophe,
        // because the payload ended up quoted twice.) Echo is turned off
        // with `stty -echo` (restored on exit via trap) instead of the
        // non-portable `read -s`; `read -r` reads the key from stdin.
        ("claude", CredentialKind::ApiKey, ShellKind::Posix) => Ok(format!(
            "sh -c {} sh {}",
            posix_quote(CLAUDE_API_KEY_POSIX_SCRIPT),
            posix_quote(&dir_str),
        )),
        ("codex", CredentialKind::ApiKey, ShellKind::Posix) => Ok(format!(
            "sh -c {} sh {}",
            posix_quote(CODEX_API_KEY_POSIX_SCRIPT),
            posix_quote(&dir_str),
        )),
        // PowerShell API-key commands run inside `& { … }` and never `exit`,
        // so a failed sign-in can't close the person's window. The env var
        // is saved first and restored after: a bare `Remove-Item` would
        // delete a value the person already had. The Claude status JSON is
        // piped to `Out-Null`, and neither command returns `$c` — a bare
        // `return $c` would print the number as a stray line — they just
        // print Signed in or an error. `-LiteralPath` treats brackets in
        // profile dirs literally.
        ("claude", CredentialKind::ApiKey, ShellKind::PowerShell) => {
            let key_path = powershell_quote(&dir.join("api-key").to_string_lossy());
            Ok(format!(
                "& {{ $k = Read-Host 'Anthropic API key (type or paste, then press Enter)' -AsSecureString; $p = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($k)); if ([string]::IsNullOrEmpty($p)) {{ Write-Error 'API key is empty; sign-in cancelled'; return }}; Set-Content -NoNewline -LiteralPath {key_path} -Value $p; $p = $null; $k = $null; $prevClaudeConfigDir = $env:CLAUDE_CONFIG_DIR; $env:CLAUDE_CONFIG_DIR={dir_q}; claude auth status | Out-Null; $c = $LASTEXITCODE; if ($null -eq $prevClaudeConfigDir) {{ Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue }} else {{ $env:CLAUDE_CONFIG_DIR = $prevClaudeConfigDir }}; if ($c -eq 0) {{ echo Signed in }} else {{ Write-Error 'Sign-in failed' }} }}",
                dir_q = powershell_quote(&dir_str),
            ))
        }
        ("codex", CredentialKind::ApiKey, ShellKind::PowerShell) => Ok(format!(
            "& {{ $k = Read-Host 'OpenAI API key (type or paste, then press Enter)' -AsSecureString; $p = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($k)); if ([string]::IsNullOrEmpty($p)) {{ Write-Error 'API key is empty; sign-in cancelled'; return }}; $prevCodexHome = $env:CODEX_HOME; $env:CODEX_HOME={dir_q}; $p | codex login --with-api-key; $c = $LASTEXITCODE; if ($null -eq $prevCodexHome) {{ Remove-Item Env:CODEX_HOME -ErrorAction SilentlyContinue }} else {{ $env:CODEX_HOME = $prevCodexHome }}; $p = $null; $k = $null; if ($c -eq 0) {{ echo Signed in }} else {{ Write-Error 'Sign-in failed' }} }}",
            dir_q = powershell_quote(&dir_str),
        )),
        _ => Err("Native sign-in unavailable".into()),
    }
}
/// Metadata-only lookup used by the host helper. Git worktrees map back to
/// their common repository; no tokens or histories are inspected.
pub fn project_for(cwd: &Path, registry: &Registry) -> Option<PathBuf> {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if let Some(path) = canonical
        .ancestors()
        .find(|p| registry.projects.contains_key(*p))
    {
        return Some(path.to_owned());
    }
    if let Some(path) = cwd.ancestors().find(|p| registry.projects.contains_key(*p)) {
        return Some(path.to_owned());
    }
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(cwd)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    common
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .filter(|p| registry.projects.contains_key(p))
}

/// A bounded native probe. Output is never logged, persisted or returned to UI.
fn probe(mut command: Command) -> Result<(bool, Vec<u8>), String> {
    use std::{
        io::Read,
        process::Stdio,
        time::{Duration, Instant},
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Agent CLI is unavailable on this host")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Agent status output unavailable")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // Drain after the cap so an unexpectedly verbose CLI cannot deadlock.
        let mut stdout = stdout;
        let mut chunk = [0; 4096];
        while let Ok(n) = stdout.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if bytes.len() < 65536 {
                bytes.extend_from_slice(&chunk[..n]);
            }
        }
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok((status.success(), reader.join().unwrap_or_default())),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Agent status check timed out or failed".into());
            }
        }
    }
}
fn native_command(binding: &Binding, executable: &Path) -> Result<Command, String> {
    // npm installs agents as `.cmd` shims on Windows, which a bare name misses.
    let mut command = Command::new(crate::agent_sessions::program(
        &executable.to_string_lossy(),
    ));
    configure(&mut command, binding)?;
    Ok(command)
}
pub fn status(binding: &Binding, executable: &Path) -> Result<&'static str, String> {
    if binding.selection == Selection::System {
        return Ok("System login — managed by the CLI");
    }
    let mut command = native_command(binding, executable)?;
    match binding.provider.as_str() {
        "claude" => {
            // No key typed yet: the profile's apiKeyHelper is configured, so
            // `auth status` would wrongly report signed in. Report "not
            // signed in" without calling the CLI at all.
            if claude_key_pending(&root(), binding, &Registry::load().unwrap_or_default()) {
                return Ok("Sign-in required");
            }
            command.args(["auth", "status"]);
        }
        "codex" => {
            command.args(["login", "status"]);
        }
        _ => return Err("Native status unavailable".into()),
    }
    let (success, bytes) = probe(command)?;
    if !success {
        return Ok("Sign-in required");
    }
    if binding.provider == "claude" {
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(
            |_| "Installed Claude CLI cannot verify profile isolation; update Claude Code",
        )?;
        let Selection::Profile(id) = binding.selection else {
            return Err("Account selection is unresolved".into());
        };
        let expected = dunce::canonicalize(root().join(id.to_string())).map_err(io_error)?;
        if value
            .get("configDirectory")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .as_ref()
            != Some(&expected)
        {
            return Err("Claude profile isolation could not be verified; Claude Code 2.1.268 or later is required".into());
        }
        let kind = profile_kind_for_status(id);
        let auth_method = value.get("authMethod").and_then(|v| v.as_str());
        match kind {
            CredentialKind::Subscription if auth_method != Some("claude.ai") => {
                return Err(
                    "This account is set up for a subscription; sign in with claude.ai".into(),
                );
            }
            CredentialKind::ApiKey if auth_method != Some("api_key_helper") => {
                return Err(
                    "This account is set up for an API key; run its sign-in again to enter the key"
                        .into(),
                );
            }
            _ => {}
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
    pub kind: Option<CredentialKind>,
    /// Newest sign-in file change (unix seconds): Claude's `api-key`,
    /// `.credentials.json` and `.claude.json`, Codex's `auth.json`. Metadata
    /// only — files are never opened — so a change means fresh credentials.
    /// None for the system login or when none of the files exist.
    pub credentials_changed_at: Option<u64>,
}

/// Newest sign-in file change (unix seconds) for a profile. Metadata only:
/// modification times are read, files are never opened or read. None for the
/// system login or when none of the files exist.
pub fn credentials_changed_at(binding: &Binding) -> Option<u64> {
    let Selection::Profile(id) = binding.selection else {
        return None;
    };
    credentials_changed_at_in(&root(), &binding.provider, Some(id))
}

pub(crate) fn credentials_changed_at_in(
    store_root: &Path,
    provider: &str,
    target: Option<Uuid>,
) -> Option<u64> {
    let id = target?;
    let names: &[&str] = match provider {
        "claude" => &["api-key", ".credentials.json", ".claude.json"],
        "codex" => &["auth.json"],
        _ => return None,
    };
    let dir = store_root.join(id.to_string());
    names
        .iter()
        .filter_map(|name| fs::metadata(dir.join(name)).ok())
        .filter(|meta| meta.is_file())
        .filter_map(|meta| meta.modified().ok())
        .filter_map(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs())
        .max()
}

/// Check an account, the system login included: whether it's signed in and
/// as whom. For a profile this applies the same isolation checks as
/// [`status`].
pub fn identity(binding: &Binding, executable: &Path) -> Result<Identity, String> {
    match binding.provider.as_str() {
        "claude" => {
            // Same gate as `status`: no key file means not signed in, and
            // the CLI is never consulted (it would claim otherwise).
            if claude_key_pending(&root(), binding, &Registry::load().unwrap_or_default()) {
                return Ok(Identity {
                    signed_in: false,
                    email: None,
                    plan: None,
                    kind: Some(CredentialKind::ApiKey),
                    credentials_changed_at: credentials_changed_at(binding),
                });
            }
            let mut command = native_command(binding, executable)?;
            command.args(["auth", "status"]);
            let (success, bytes) = probe(command)?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            let kind = match binding.selection {
                Selection::Profile(id) => {
                    // The isolation checks, on the same answer.
                    status(binding, executable)?;
                    Some(profile_kind_for_status(id))
                }
                _ => None,
            };
            let text = |key: &str| {
                value
                    .get(key)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
            };
            let signed_in = success
                && value
                    .get("loggedIn")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(success);
            Ok(Identity {
                signed_in,
                email: text("email"),
                plan: text("subscriptionType").or_else(|| text("planTier")),
                kind,
                credentials_changed_at: credentials_changed_at(binding),
            })
        }
        "codex" => {
            let mut command = native_command(binding, executable)?;
            command.args(["login", "status"]);
            let (success, _) = probe(command)?;
            let (account, kind) = match binding.selection {
                Selection::Profile(id) => (id.to_string(), Some(profile_kind_for_status(id))),
                _ => ("system".into(), None),
            };
            let claims = crate::agent_usage::codex_home(&account)
                .and_then(|home| fs::read(home.join("auth.json")).ok())
                .and_then(|bytes| codex_claims(&bytes))
                .unwrap_or_default();
            Ok(Identity {
                signed_in: success,
                kind,
                credentials_changed_at: credentials_changed_at(binding),
                ..claims
            })
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
    let claims: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .ok()?,
    )
    .ok()?;
    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let plan = claims
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    Some(Identity {
        signed_in: false,
        email,
        plan,
        kind: None,
        // The claims alone don't say which profile was read; the caller
        // fills this in from the binding it probed.
        credentials_changed_at: None,
    })
}

pub fn login(id: Uuid) -> Result<i32, String> {
    let registry = Registry::load()?;
    let profile = registry
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or("Account no longer exists")?;
    let binding = registry.resolve(&profile.provider, None, &Selection::Profile(id))?;
    let mut command = native_command(&binding, Path::new(&profile.provider))?;
    match profile.provider.as_str() {
        "claude" => {
            command.args(["auth", "login"]);
        }
        "codex" => {
            command.arg("login");
        }
        _ => return Err("Native sign-in unavailable".into()),
    }
    command
        .status()
        .map(|s| s.code().unwrap_or(1))
        .map_err(|_| "Could not start native sign-in".into())
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn codex_claims_read_email_and_plan_only() {
        use base64::Engine as _;
        let claims = serde_json::json!({"email": "me@example.com", "https://api.openai.com/auth": {"chatgpt_plan_type": "plus"}});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        let auth = serde_json::json!({"tokens": {"id_token": format!("h.{payload}.s"), "access_token": "secret"}});
        let found = codex_claims(&serde_json::to_vec(&auth).unwrap()).unwrap();
        assert_eq!(found.email.as_deref(), Some("me@example.com"));
        assert_eq!(found.plan.as_deref(), Some("plus"));
        assert!(codex_claims(b"{}").is_none());
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    #[test]
    fn old_accounts_json_without_kind_still_loads() {
        let raw = serde_json::json!({
            "machine_id": "11111111-1111-1111-1111-111111111111",
            "profiles": [
                {"id": "22222222-2222-2222-2222-222222222222", "provider": "claude", "label": "Work", "machine_id": "11111111-1111-1111-1111-111111111111"}
            ],
            "defaults": {},
            "projects": {}
        });
        let registry: Registry = serde_json::from_value(raw).expect("old file loads");
        assert_eq!(registry.profiles.len(), 1);
        assert_eq!(registry.profiles[0].kind, CredentialKind::Subscription);
        // New files round-trip the kind.
        let raw = serde_json::json!({
            "machine_id": "11111111-1111-1111-1111-111111111111",
            "profiles": [
                {"id": "33333333-3333-3333-3333-333333333333", "provider": "claude", "label": "Keys", "machine_id": "11111111-1111-1111-1111-111111111111", "kind": "api_key"}
            ],
            "defaults": {},
            "projects": {}
        });
        let registry: Registry = serde_json::from_value(raw).expect("new file loads");
        assert_eq!(registry.profiles[0].kind, CredentialKind::ApiKey);
    }

    #[test]
    fn posix_quote_keeps_everything_literal() {
        assert_eq!(posix_quote("/tmp/dir"), "'/tmp/dir'");
        assert_eq!(posix_quote("/tmp/a'b"), "'/tmp/a'\\''b'");
        // `$()`, backticks, double quotes and spaces stay inside single quotes.
        let tricky = "/tmp/a'b \"c\" $(rm -rf /) `evil` space dir";
        let quoted = posix_quote(tricky);
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert!(quoted.contains("'\\''"));
        assert!(quoted.contains("$(rm -rf /)"));
        assert!(quoted.contains("`evil`"));
    }

    #[test]
    fn powershell_quote_doubles_single_quotes() {
        assert_eq!(powershell_quote("C:\\dir"), "'C:\\dir'");
        assert_eq!(powershell_quote("a'b"), "'a''b'");
    }

    #[test]
    fn subscription_sign_in_commands_are_exact() {
        let dir = Path::new("/tmp/profiles/abc");
        let claude = sign_in_command_for_profile(
            "claude",
            CredentialKind::Subscription,
            dir,
            ShellKind::Posix,
        )
        .unwrap();
        assert_eq!(
            claude,
            "CLAUDE_CONFIG_DIR='/tmp/profiles/abc' claude auth login"
        );
        let codex = sign_in_command_for_profile(
            "codex",
            CredentialKind::Subscription,
            dir,
            ShellKind::Posix,
        )
        .unwrap();
        assert_eq!(codex, "CODEX_HOME='/tmp/profiles/abc' codex login");
        let claude_ps = sign_in_command_for_profile(
            "claude",
            CredentialKind::Subscription,
            Path::new("C:\\p\\abc"),
            ShellKind::PowerShell,
        )
        .unwrap();
        assert_eq!(
            claude_ps,
            "$prevClaudeConfigDir = $env:CLAUDE_CONFIG_DIR; $env:CLAUDE_CONFIG_DIR='C:\\p\\abc'; claude auth login; if ($null -eq $prevClaudeConfigDir) { Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue } else { $env:CLAUDE_CONFIG_DIR = $prevClaudeConfigDir }"
        );
        let codex_ps = sign_in_command_for_profile(
            "codex",
            CredentialKind::Subscription,
            Path::new("C:\\p\\abc"),
            ShellKind::PowerShell,
        )
        .unwrap();
        assert_eq!(
            codex_ps,
            "$prevCodexHome = $env:CODEX_HOME; $env:CODEX_HOME='C:\\p\\abc'; codex login; if ($null -eq $prevCodexHome) { Remove-Item Env:CODEX_HOME -ErrorAction SilentlyContinue } else { $env:CODEX_HOME = $prevCodexHome }"
        );
        for cmd in [&claude, &codex, &claude_ps, &codex_ps] {
            assert!(!cmd.contains('\n'), "one line: {cmd}");
        }
    }

    #[test]
    fn system_sign_in_commands_choose_shell_login() {
        // Target None never touches the filesystem.
        assert_eq!(
            sign_in_command("claude", None, ShellKind::Posix).unwrap(),
            "claude auth login"
        );
        assert_eq!(
            sign_in_command("codex", None, ShellKind::PowerShell).unwrap(),
            "codex login"
        );
        assert!(sign_in_command("unknown", None, ShellKind::Posix).is_err());
    }

    /// Split `sh -c '<script>' sh <dir>`: the script holds no single quotes
    /// at all, so it is everything between the first two `'`. Returns the
    /// script and everything after it (` sh <quoted dir>`).
    fn sh_c_script_and_dir(cmd: &str) -> (String, String) {
        let rest = cmd.strip_prefix("sh -c '").expect("sh -c call: {cmd}");
        let (script, rest) = rest.split_once('\'').expect("script span: {cmd}");
        (script.to_owned(), rest.to_owned())
    }

    #[test]
    fn api_key_posix_commands_pass_the_dir_as_an_argument() {
        let dir = Path::new("/tmp/profiles/abc");
        let claude =
            sign_in_command_for_profile("claude", CredentialKind::ApiKey, dir, ShellKind::Posix)
                .unwrap();
        assert_eq!(
            claude,
            format!(
                "sh -c {} sh {}",
                posix_quote(CLAUDE_API_KEY_POSIX_SCRIPT),
                posix_quote("/tmp/profiles/abc"),
            )
        );
        assert!(!claude.contains('\n'), "one line");
        let codex =
            sign_in_command_for_profile("codex", CredentialKind::ApiKey, dir, ShellKind::Posix)
                .unwrap();
        assert_eq!(
            codex,
            format!(
                "sh -c {} sh {}",
                posix_quote(CODEX_API_KEY_POSIX_SCRIPT),
                posix_quote("/tmp/profiles/abc"),
            )
        );
        assert!(!codex.contains('\n'), "one line");
        // Portable input: no `read -s` (dash rejects it), echo off via stty.
        for cmd in [&claude, &codex] {
            let (script, rest) = sh_c_script_and_dir(cmd);
            assert!(!cmd.contains("read -s"), "no read -s: {cmd}");
            assert!(script.contains("read -r K"), "reads key: {cmd}");
            assert!(script.contains("stty -echo"), "echo off: {cmd}");
            assert!(
                script.contains("trap \"stty echo 2>/dev/null\" EXIT INT TERM"),
                "trap restores echo: {cmd}"
            );
            assert!(script.contains("unset K"), "key cleared: {cmd}");
            assert_eq!(rest, format!(" sh {}", posix_quote("/tmp/profiles/abc")));
        }
        assert!(
            sh_c_script_and_dir(&claude).0.contains("umask 077"),
            "claude key file is owner-only"
        );
        assert!(sh_c_script_and_dir(&claude)
            .0
            .contains("claude auth status >/dev/null && echo Signed in"));
        assert!(sh_c_script_and_dir(&codex)
            .0
            .contains("codex login --with-api-key"));
        assert!(sh_c_script_and_dir(&codex)
            .0
            .contains("rc=$?; unset K; exit $rc"));
    }

    #[test]
    fn posix_api_key_scripts_hold_no_quoting_hazards() {
        for script in [CLAUDE_API_KEY_POSIX_SCRIPT, CODEX_API_KEY_POSIX_SCRIPT] {
            assert!(!script.contains('\''), "no single quotes: {script}");
            assert!(!script.contains('\\'), "no backslashes: {script}");
            assert!(script.contains("\"$1\""), "dir arrives as $1: {script}");
        }
        assert!(CLAUDE_API_KEY_POSIX_SCRIPT.contains("\"$1/api-key\""));
    }

    #[test]
    fn api_key_powershell_commands_use_secure_string_and_child_env() {
        let dir = Path::new("C:\\p\\abc");
        let claude = sign_in_command_for_profile(
            "claude",
            CredentialKind::ApiKey,
            dir,
            ShellKind::PowerShell,
        )
        .unwrap();
        assert!(!claude.contains('\n'));
        // A script block with `return`: never `exit`, so a failed sign-in
        // can't close the person's window.
        assert!(claude.starts_with("& { "), "script block: {claude}");
        assert!(claude.ends_with(" }"), "script block: {claude}");
        assert!(!claude.contains("exit"), "no exit: {claude}");
        assert!(!claude.contains("return $c"), "no stray number: {claude}");
        assert!(claude.contains("$k = Read-Host 'Anthropic API key (type or paste, then press Enter)' -AsSecureString"));
        assert!(claude.contains("[Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($k))"));
        assert!(claude.contains("Write-Error 'API key is empty; sign-in cancelled'; return"));
        assert!(claude.contains("Set-Content -NoNewline -LiteralPath"));
        // The status JSON is discarded; success prints one line, failure an error.
        assert!(claude.contains("claude auth status | Out-Null; $c = $LASTEXITCODE"));
        assert!(claude
            .contains("if ($c -eq 0) { echo Signed in } else { Write-Error 'Sign-in failed' }"));
        // The env var is saved first and restored after, never deleted
        // when the person already had a value.
        assert!(claude.contains(
            "$prevClaudeConfigDir = $env:CLAUDE_CONFIG_DIR; $env:CLAUDE_CONFIG_DIR='C:\\p\\abc'"
        ));
        assert!(claude.contains("Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue"));
        assert!(claude.contains("} else { $env:CLAUDE_CONFIG_DIR = $prevClaudeConfigDir }"));
        let codex = sign_in_command_for_profile(
            "codex",
            CredentialKind::ApiKey,
            dir,
            ShellKind::PowerShell,
        )
        .unwrap();
        assert!(!codex.contains('\n'));
        assert!(codex.starts_with("& { "), "script block: {codex}");
        assert!(codex.ends_with(" }"), "script block: {codex}");
        assert!(!codex.contains("exit"), "no exit: {codex}");
        assert!(!codex.contains("return $c"), "no stray number: {codex}");
        assert!(codex.contains("$k = Read-Host 'OpenAI API key (type or paste, then press Enter)' -AsSecureString"));
        assert!(codex.contains("$prevCodexHome = $env:CODEX_HOME; $env:CODEX_HOME='C:\\p\\abc'"));
        assert!(codex.contains("codex login --with-api-key"));
        assert!(codex.contains("Remove-Item Env:CODEX_HOME -ErrorAction SilentlyContinue"));
        assert!(codex.contains("} else { $env:CODEX_HOME = $prevCodexHome }"));
        assert!(codex
            .contains("if ($c -eq 0) { echo Signed in } else { Write-Error 'Sign-in failed' }"));
    }

    #[test]
    fn tricky_dirs_stay_literal_in_every_command() {
        let cases = [
            "/tmp/a'b",
            "/tmp/a b \"c\"",
            "/tmp/$(rm -rf /)",
            "/tmp/`evil`",
            "/tmp/$HOME",
        ];
        for raw in cases {
            let dir = Path::new(raw);
            for (provider, kind) in [
                ("claude", CredentialKind::Subscription),
                ("codex", CredentialKind::Subscription),
                ("claude", CredentialKind::ApiKey),
                ("codex", CredentialKind::ApiKey),
            ] {
                let cmd =
                    sign_in_command_for_profile(provider, kind, dir, ShellKind::Posix).unwrap();
                assert!(!cmd.contains('\n'), "one line for {raw}");
                if kind == CredentialKind::ApiKey {
                    // `sh -c '<script>' sh <quoted dir>`: the script is
                    // quote-free, the dir is quoted exactly once.
                    assert!(cmd.starts_with("sh -c '"), "wrapped: {cmd}");
                    let (script, rest) = sh_c_script_and_dir(&cmd);
                    assert!(!script.contains('\''), "script quote-free: {cmd}");
                    assert!(!script.contains('\\'), "script backslash-free: {cmd}");
                    assert_eq!(rest, format!(" sh {}", posix_quote(raw)));
                } else {
                    assert!(
                        cmd.contains(&posix_quote(raw)),
                        "quoted dir stays literal for {raw}: {cmd}"
                    );
                }
            }
            // PowerShell: single quotes with `''` doubling.
            let ps = sign_in_command_for_profile(
                "claude",
                CredentialKind::Subscription,
                dir,
                ShellKind::PowerShell,
            )
            .unwrap();
            assert!(ps.contains(&powershell_quote(raw)), "ps quoted: {ps}");
        }
    }

    #[test]
    fn api_keys_are_cleaned_checked_and_never_shown() {
        let key = ApiKey::parse("  sk-ant-test \n").unwrap();
        assert_eq!(key.expose(), "sk-ant-test");
        for bad in ["", "   ", "two words", "line\nbreak", "k\u{e9}y"] {
            assert!(ApiKey::parse(bad).is_err(), "{bad:?}");
        }
        let request = crate::development::AccountsRequest::AddWithKey {
            provider: "claude".into(),
            label: "Work".into(),
            key,
        };
        assert!(!format!("{request:?}").contains("sk-ant-test"));
    }

    #[cfg(unix)]
    #[test]
    fn a_key_reaches_the_command_and_its_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("dragent-key-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("api-key");
        write_private(&path, b"sk-ant-old").unwrap();
        write_private(&path, b"sk-ant-new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"sk-ant-new");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");

        let mut matches = Command::new("sh");
        matches.args(["-c", "[ \"$(cat)\" = sk-test ]"]);
        assert!(feed(matches, b"sk-test").unwrap());
        let mut refuses = Command::new("sh");
        refuses.args(["-c", "cat >/dev/null; exit 1"]);
        assert!(!feed(refuses, b"sk-test").unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_or_empty_api_key_file_counts_as_no_key() {
        let base = std::env::temp_dir().join(format!("dragent-key-gate-{}", Uuid::new_v4()));
        let dir = base.join("profile");
        std::fs::create_dir_all(&dir).unwrap();
        // Missing file: no key yet.
        assert!(!api_key_file_present(&dir));
        // Empty file: still no key.
        std::fs::write(dir.join("api-key"), b"").unwrap();
        assert!(!api_key_file_present(&dir));
        // One byte is a key (content is never read here).
        std::fs::write(dir.join("api-key"), b"k").unwrap();
        assert!(api_key_file_present(&dir));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn claude_key_pending_needs_a_typed_key() {
        let store = std::env::temp_dir().join(format!("dragent-key-gate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&store).unwrap();
        let mut registry = Registry {
            machine_id: Some(Uuid::new_v4()),
            ..Registry::default()
        };
        let api_key = Profile {
            id: Uuid::new_v4(),
            provider: "claude".into(),
            label: "keys".into(),
            machine_id: registry.machine_id.unwrap(),
            kind: CredentialKind::ApiKey,
        };
        let subscription = Profile {
            id: Uuid::new_v4(),
            provider: "claude".into(),
            label: "sub".into(),
            machine_id: registry.machine_id.unwrap(),
            kind: CredentialKind::Subscription,
        };
        registry.profiles.push(api_key.clone());
        registry.profiles.push(subscription.clone());
        let binding = |p: &Profile| Binding {
            selection: Selection::Profile(p.id),
            provider: p.provider.clone(),
            machine_id: Some(p.machine_id),
            label: p.label.clone(),
        };
        // No key file yet: pending, without the CLI ever running.
        assert!(claude_key_pending(&store, &binding(&api_key), &registry));
        // Other providers, subscriptions, the system login and unknown ids
        // are never gated by this file.
        assert!(!claude_key_pending(
            &store,
            &binding(&subscription),
            &registry
        ));
        assert!(!claude_key_pending(
            &store,
            &Binding::system("claude"),
            &registry
        ));
        let codex_binding = Binding {
            selection: Selection::Profile(api_key.id),
            provider: "codex".into(),
            machine_id: Some(api_key.machine_id),
            label: "keys".into(),
        };
        assert!(!claude_key_pending(&store, &codex_binding, &registry));
        // An empty file is still pending; any content clears it.
        let profile_dir = store.join(api_key.id.to_string());
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(profile_dir.join("api-key"), b"").unwrap();
        assert!(claude_key_pending(&store, &binding(&api_key), &registry));
        std::fs::write(profile_dir.join("api-key"), b"sk-ant-test").unwrap();
        assert!(!claude_key_pending(&store, &binding(&api_key), &registry));
        let _ = std::fs::remove_dir_all(&store);
    }

    /// Run the actual generated POSIX API-key commands under every POSIX
    /// shell present (`dash` included) against a temp profile dir with fake
    /// `claude`/`codex` binaries on PATH, feeding the key through stdin.
    #[cfg(unix)]
    #[test]
    fn posix_api_key_commands_run_under_posix_shells() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};

        fn shells() -> Vec<String> {
            ["dash", "bash", "sh"]
                .into_iter()
                .filter(|shell| {
                    Command::new(shell)
                        .arg("-c")
                        .arg("exit 0")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .is_ok_and(|s| s.success())
                })
                .map(str::to_owned)
                .collect()
        }
        let shells = shells();
        assert!(!shells.is_empty(), "no POSIX shell found to test under");

        for provider in ["claude", "codex"] {
            for shell in &shells {
                let base = std::env::temp_dir().join(format!("dragent-sh-run-{}", Uuid::new_v4()));
                let bin = base.join("bin");
                std::fs::create_dir_all(&bin).unwrap();
                let sentinel = base.join("sentinel");
                // Fake CLIs: claude reports a signed-in API-key helper and
                // records the config dir it saw; codex captures what the
                // command piped into `login --with-api-key`.
                let fake = if provider == "claude" {
                    "#!/bin/sh\nprintf '%s\\n' \"$CLAUDE_CONFIG_DIR\" >> \"$SENTINEL\"\nprintf '{\"loggedIn\":true,\"authMethod\":\"api_key_helper\"}\\n'\nexit 0\n"
                } else {
                    "#!/bin/sh\ncat > \"$SENTINEL\"\nexit 0\n"
                };
                let exe = bin.join(provider);
                std::fs::write(&exe, fake).unwrap();
                std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
                let path = format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var_os("PATH")
                        .unwrap_or_default()
                        .to_string_lossy()
                );

                let run = |profile: &std::path::Path, input: &[u8]| {
                    let cmd = sign_in_command_for_profile(
                        provider,
                        CredentialKind::ApiKey,
                        profile,
                        ShellKind::Posix,
                    )
                    .unwrap();
                    // The outer shell parses `sh -c '…'`; the inner `sh`
                    // runs the script. Both layers are exercised here.
                    let mut child = Command::new(shell)
                        .arg("-c")
                        .arg(&cmd)
                        .env("PATH", &path)
                        .env("SENTINEL", &sentinel)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .expect("spawn shell");
                    child
                        .stdin
                        .as_mut()
                        .unwrap()
                        .write_all(input)
                        .expect("feed key");
                    child.wait_with_output().expect("wait")
                };

                // A typed key signs in: exit 0, key stored (claude) or piped
                // (codex), no trailing newline, mode 0600. The profile dir
                // holds an apostrophe, which broke the old doubly-quoted
                // payload under fish.
                let _ = std::fs::remove_file(&sentinel);
                let profile = base.join("pro'file");
                std::fs::create_dir_all(&profile).unwrap();
                let out = run(&profile, b"test-key-123\n");
                assert!(
                    out.status.success(),
                    "{provider} under {shell}: {} {}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                if provider == "claude" {
                    assert_eq!(
                        std::fs::read(profile.join("api-key")).unwrap(),
                        b"test-key-123"
                    );
                    #[cfg(unix)]
                    assert_eq!(
                        std::fs::metadata(profile.join("api-key"))
                            .unwrap()
                            .permissions()
                            .mode()
                            & 0o777,
                        0o600,
                        "key file is owner-only"
                    );
                    assert!(
                        String::from_utf8_lossy(&out.stdout).contains("Signed in"),
                        "stdout: {}",
                        String::from_utf8_lossy(&out.stdout)
                    );
                    assert_eq!(
                        std::fs::read_to_string(&sentinel).unwrap().trim(),
                        profile.to_string_lossy(),
                        "config dir reached the CLI"
                    );
                } else {
                    assert_eq!(
                        std::fs::read(&sentinel).unwrap(),
                        b"test-key-123",
                        "key piped to codex"
                    );
                }

                // An empty key refuses without touching anything: exit
                // non-zero, and no key file appears for claude.
                let _ = std::fs::remove_file(&sentinel);
                let empty_profile = base.join("empty 'profile");
                std::fs::create_dir_all(&empty_profile).unwrap();
                let out = run(&empty_profile, b"\n");
                assert!(
                    !out.status.success(),
                    "{provider} under {shell} accepted an empty key"
                );
                assert!(
                    String::from_utf8_lossy(&out.stderr).contains("API key is empty"),
                    "stderr: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                if provider == "claude" {
                    assert!(
                        !empty_profile.join("api-key").exists(),
                        "no key file on refusal"
                    );
                }
                let _ = std::fs::remove_dir_all(&base);
            }
        }
    }

    #[test]
    fn rename_refuses_a_name_another_account_already_uses() {
        let mut registry = Registry {
            machine_id: Some(Uuid::new_v4()),
            ..Registry::default()
        };
        let machine = registry.machine_id.unwrap();
        let named = |provider: &str, label: &str| Profile {
            id: Uuid::new_v4(),
            provider: provider.into(),
            label: label.into(),
            machine_id: machine,
            kind: CredentialKind::Subscription,
        };
        let work = named("claude", "Work");
        let play = named("claude", "Play");
        let other_agent = named("codex", "Work");
        registry
            .profiles
            .extend([work.clone(), play.clone(), other_agent.clone()]);
        // Same rule as `add`: case-insensitive, trimmed, per agent.
        assert!(registry.rename(play.id, "work").is_err());
        assert!(registry.rename(play.id, "  WORK  ").is_err());
        assert!(registry.rename(Uuid::new_v4(), "Work").is_err());
        // Its own name (any case) and fresh names are fine; the same name
        // on another agent is untouched.
        registry.rename(play.id, "Play").unwrap();
        registry.rename(play.id, "play").unwrap();
        registry.rename(play.id, "Extra").unwrap();
        assert_eq!(
            registry
                .profiles
                .iter()
                .find(|p| p.id == play.id)
                .unwrap()
                .label,
            "Extra"
        );
        registry.rename(other_agent.id, "Work").unwrap();
    }

    #[test]
    fn credentials_changed_at_is_the_newest_sign_in_file_mtime() {
        let store = std::env::temp_dir().join(format!("dragent-changed-at-{}", Uuid::new_v4()));
        let id = Uuid::new_v4();
        let dir = store.join(id.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        // No files, the system login, and unknown providers: all None.
        assert_eq!(credentials_changed_at_in(&store, "claude", Some(id)), None);
        assert_eq!(credentials_changed_at_in(&store, "claude", None), None);
        assert_eq!(credentials_changed_at_in(&store, "other", Some(id)), None);
        // Unknown ids (no dir at all) are None, not an error.
        assert_eq!(
            credentials_changed_at_in(&store, "claude", Some(Uuid::new_v4())),
            None
        );
        let mtime = |name: &str| {
            fs::metadata(dir.join(name))
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        std::fs::write(dir.join("api-key"), b"k").unwrap();
        assert_eq!(
            credentials_changed_at_in(&store, "claude", Some(id)),
            Some(mtime("api-key"))
        );
        // Codex only looks at auth.json, so the Claude key is invisible.
        assert_eq!(credentials_changed_at_in(&store, "codex", Some(id)), None);
        std::fs::write(dir.join("auth.json"), b"{}").unwrap();
        assert_eq!(
            credentials_changed_at_in(&store, "codex", Some(id)),
            Some(mtime("auth.json"))
        );
        // The newest of Claude's three files wins.
        std::fs::write(dir.join(".credentials.json"), b"{}").unwrap();
        let newest = mtime("api-key").max(mtime(".credentials.json"));
        assert_eq!(
            credentials_changed_at_in(&store, "claude", Some(id)),
            Some(newest)
        );
        let _ = std::fs::remove_dir_all(&store);
    }

    #[test]
    fn old_list_identity_json_without_new_fields_still_loads() {
        // Older peers send identities without `kind`, `error` or
        // `credentials_changed_at`; they must still load.
        let raw = serde_json::json!({"target": null, "provider": "claude", "signed_in": true});
        let view: crate::development::AccountIdentityView =
            serde_json::from_value(raw).expect("old list entry loads");
        assert_eq!(view.credentials_changed_at, None);
        assert_eq!(view.kind, None);
        assert_eq!(view.error, None);
    }

    #[test]
    fn posix_prefix_formats_launch_env_with_trailing_space() {
        let env = LaunchEnv {
            set: vec![("CLAUDE_CONFIG_DIR".into(), "/tmp/a'b".into())],
            unset: vec!["ANTHROPIC_API_KEY".into(), "ANTHROPIC_AUTH_TOKEN".into()],
        };
        assert_eq!(
            posix_prefix(&env),
            "env -u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN CLAUDE_CONFIG_DIR='/tmp/a'\\''b' "
        );
        assert_eq!(posix_prefix(&LaunchEnv::default()), "");
    }
}

#[cfg(test)]
#[test]
#[ignore]
fn identity_probe() {
    let registry = Registry::load().unwrap_or_default();
    for provider in ["claude", "codex"] {
        let r = identity(&Binding::system(provider), Path::new(provider));
        println!(
            "{provider} system: {:?}",
            r.map(|i| (i.signed_in, i.email.is_some(), i.plan))
        );
        for p in registry.profiles.iter().filter(|p| p.provider == provider) {
            let b = registry
                .resolve(provider, None, &Selection::Profile(p.id))
                .unwrap();
            println!(
                "{provider} {}: {:?}",
                p.label,
                identity(&b, Path::new(provider)).map(|i| (i.signed_in, i.email.is_some(), i.plan))
            );
        }
    }
}
