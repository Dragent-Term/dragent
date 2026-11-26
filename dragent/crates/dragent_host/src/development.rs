//! Development operations execute on the owning machine, over the host protocol.
use serde::{Deserialize, Serialize};

pub const FILE_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DevelopmentRequest {
    Environment,
    Metrics,
    HostLog,
    /// Read the current checkout's PR using this machine's GitHub login.
    PullRequest {
        root: String,
    },
    InstallCommand {
        tool: String,
    },
    OpenProject {
        path: String,
    },
    Clone {
        url: String,
        parent: String,
        name: String,
    },
    Init {
        path: String,
    },
    List {
        root: String,
        path: String,
    },
    Read {
        root: String,
        path: String,
    },
    Write {
        root: String,
        path: String,
        #[serde(with = "crate::protocol::tail_bytes")]
        data: Vec<u8>,
        /// Required revision for an existing file. None means create only.
        expected: Option<String>,
    },
    Mkdir {
        root: String,
        path: String,
    },
    Rename {
        root: String,
        from: String,
        to: String,
    },
    Delete {
        root: String,
        path: String,
        recursive: bool,
    },
    Search {
        root: String,
        query: String,
    },
    /// Plain filesystem access by absolute path on the host. Remote projects
    /// use it to back the desktop's Files tree, editor and git placement.
    Fs(FsRequest),
    /// Whether each named executable is installed, resolved with the user's
    /// shell PATH ([`crate::host_tools`]). Answered with [`DevelopmentResponse::Tools`].
    /// Runtimes older than this request close the connection instead, so
    /// callers fall back to `Environment`'s tools.
    Which {
        tools: Vec<String>,
    },
    /// Status, version and every installed copy of each tool, plus whether
    /// this machine allows administrator rights without a password.
    /// Answered with [`DevelopmentResponse::ToolsReport`].
    ProbeTools {
        tools: Vec<ToolQuery>,
    },
    /// Choose the copy of a tool Dragent uses on this machine (`None` goes
    /// back to the first one on the PATH). Agents and the runtime's own git
    /// follow the choice.
    PinTool {
        name: String,
        pin: Option<ToolPin>,
    },
    PullRequests(crate::pull_requests::Request),
    /// Machine-local agent accounts, answered by the owning host (local or
    /// remote `dragent-host`). Answers never carry a credential; a typed API
    /// key travels one way, to the machine that stores it.
    Accounts(AccountsRequest),
}

/// One account request over the host protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccountsRequest {
    List,
    Add {
        provider: String,
        label: String,
        #[serde(default)]
        kind: crate::agent_accounts::CredentialKind,
    },
    Rename {
        id: uuid::Uuid,
        label: String,
    },
    Remove {
        id: uuid::Uuid,
    },
    SetDefault {
        provider: String,
        selection: crate::agent_accounts::Selection,
    },
    SignInCommand {
        provider: String,
        target: Option<uuid::Uuid>,
    },
    LaunchEnv {
        provider: String,
        selection: crate::agent_accounts::Selection,
    },
    /// Add an API-key account together with its key.
    AddWithKey {
        provider: String,
        label: String,
        key: crate::agent_accounts::ApiKey,
    },
    /// Replace an API-key account's key.
    SetKey {
        id: uuid::Uuid,
        key: crate::agent_accounts::ApiKey,
    },
}

/// One profile as the account list shows it. No credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountProfileView {
    pub id: uuid::Uuid,
    pub provider: String,
    pub label: String,
    pub kind: crate::agent_accounts::CredentialKind,
}

/// One signed-in identity as the agent reports it, plus any probe error.
/// An error never fails the list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountIdentityView {
    pub target: Option<uuid::Uuid>,
    pub provider: String,
    pub signed_in: bool,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub kind: Option<crate::agent_accounts::CredentialKind>,
    #[serde(default)]
    pub error: Option<String>,
    /// Newest sign-in file change (unix seconds), for the Settings
    /// "Sign in again" fix. None for the system login or when none of the
    /// profile's sign-in files exist.
    #[serde(default)]
    pub credentials_changed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccountsResponse {
    List {
        machine_id: Option<uuid::Uuid>,
        profiles: Vec<AccountProfileView>,
        defaults: std::collections::BTreeMap<String, crate::agent_accounts::Selection>,
        identities: Vec<AccountIdentityView>,
    },
    Added(AccountProfileView),
    Ack,
    SignInCommand {
        command: String,
        shell: crate::agent_accounts::ShellKind,
    },
    LaunchEnv(crate::agent_accounts::LaunchEnv),
}

/// One tool to report on: its executable, and the arguments that print its
/// version (`--version` for most, `version` for go).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolQuery {
    pub name: String,
    pub version_args: Vec<String>,
}

/// A copy of a tool the user chose over the first one on the PATH.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolPin {
    /// An installed executable, by absolute path.
    Path(String),
    /// A command line run instead, such as `npx -y @openai/codex`.
    Command(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolStatus {
    /// Runs.
    Ready,
    /// Found, but fails to start (a missing interpreter or library).
    Broken,
    Missing,
}

/// One installed copy of a tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCopy {
    pub path: String,
    /// The file it really is, symlinks followed.
    pub real_path: String,
    /// How it was installed, judged from where it lives ("npm", "Homebrew"…).
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolReport {
    pub name: String,
    pub status: ToolStatus,
    pub version: Option<String>,
    /// Every copy on the PATH, first (what a shell runs) first.
    pub copies: Vec<ToolCopy>,
    pub pin: Option<ToolPin>,
    /// What Dragent runs: the pinned copy or command, else the first copy.
    pub resolved: Option<String>,
    /// Why it's broken, or that the pinned copy is gone.
    pub detail: Option<String>,
}

/// How installs can get administrator rights on this machine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Elevation {
    Root,
    PasswordlessSudo,
    /// sudo asks for the user's password (typed in the install terminal).
    SudoWithPassword,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolsReport {
    pub tools: Vec<ToolReport>,
    pub elevation: Elevation,
}

/// Account-wide filesystem capability of an authenticated host client. Like
/// CreateTerminal and RunGit, this operates with the SSH account's privileges;
/// it is not a project sandbox. Root-relative requests have separate policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FsRequest {
    Stat {
        path: String,
    },
    List {
        path: String,
    },
    Read {
        path: String,
    },
    Write {
        path: String,
        #[serde(with = "crate::protocol::tail_bytes")]
        data: Vec<u8>,
    },
    CreateDir {
        path: String,
    },
    CreateFile {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Copy {
        from: String,
        to: String,
    },
    Remove {
        path: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FsStat {
    pub exists: bool,
    pub directory: bool,
    pub symlink: bool,
    pub bytes: u64,
    pub modified_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub directory: bool,
    pub symlink: bool,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DevelopmentResponse {
    Accounts(AccountsResponse),
    Environment {
        home: String,
        os: String,
        arch: String,
        shell: String,
        tools: Vec<(String, bool)>,
    },
    Project {
        root: String,
        git: bool,
    },
    Entries(Vec<FileEntry>),
    File {
        #[serde(with = "crate::protocol::tail_bytes")]
        data: Vec<u8>,
        revision: String,
    },
    Saved {
        revision: String,
    },
    Matches {
        lines: Vec<String>,
        truncated: bool,
    },
    Metrics {
        cpu_percent: f32,
        memory_used: u64,
        memory_total: u64,
        uptime_seconds: u64,
        /// Filesystem holding the host user's home. Older hosts omit these.
        #[serde(default)]
        disk_used: u64,
        #[serde(default)]
        disk_total: u64,
    },
    Text(String),
    Command(String),
    Done,
    Stat(FsStat),
    /// (executable, installed), in the order asked.
    Tools(Vec<(String, bool)>),
    ToolsReport(ToolsReport),
}

#[cfg(not(target_family = "wasm"))]
mod runtime {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::Read,
        path::{Component, Path, PathBuf},
    };
    /// (used, total) bytes of the filesystem holding the home directory.
    fn home_disk_usage() -> (u64, u64) {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let Some(home) = dirs::home_dir() else {
                return (0, 0);
            };
            let Ok(path) = std::ffi::CString::new(home.as_os_str().as_bytes()) else {
                return (0, 0);
            };
            let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
                return (0, 0);
            }
            let block = stat.f_frsize as u64;
            let total = stat.f_blocks as u64 * block;
            let free = stat.f_bfree as u64 * block;
            (total.saturating_sub(free), total)
        }
        #[cfg(not(unix))]
        {
            (0, 0)
        }
    }
    fn revision(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }
    fn project(path: &str) -> Result<PathBuf, String> {
        let home = dirs::home_dir().ok_or("Home directory unavailable")?;
        let path = if path == "~" {
            home
        } else if let Some(p) = path.strip_prefix("~/") {
            home.join(p)
        } else {
            PathBuf::from(path)
        };
        let root = fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !root.is_dir() {
            return Err("Choose a directory".into());
        }
        Ok(root)
    }
    /// Resolve existing ancestors too: symlinks cannot escape a project even
    /// when creating a file below a symlink or removing a linked directory.
    fn contained(root: &Path, relative: &str, existing: bool) -> Result<PathBuf, String> {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err("Path must stay inside the project".into());
        }
        let joined = root.join(path);
        let mut ancestor = joined.as_path();
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or("Invalid path")?;
        }
        let canonical = fs::canonicalize(ancestor).map_err(|e| e.to_string())?;
        if !canonical.starts_with(root) {
            return Err("Path resolves outside the project".into());
        }
        if existing {
            fs::canonicalize(&joined).map_err(|e| e.to_string())
        } else {
            Ok(joined)
        }
    }
    fn absolute(path: &str) -> Result<PathBuf, String> {
        let home = dirs::home_dir().ok_or("Home directory unavailable")?;
        let path = if path == "~" {
            home
        } else if let Some(rest) = path.strip_prefix("~/") {
            home.join(rest)
        } else {
            PathBuf::from(path)
        };
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err("Expected an absolute path without parent traversal".into());
        }
        Ok(path)
    }

    fn stat(path: &Path) -> FsStat {
        let Ok(link) = fs::symlink_metadata(path) else {
            return FsStat::default();
        };
        let metadata = fs::metadata(path).unwrap_or(link.clone());
        FsStat {
            exists: true,
            directory: metadata.is_dir(),
            symlink: link.file_type().is_symlink(),
            bytes: metadata.len(),
            modified_unix_ms: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or_default(),
        }
    }

    /// Resolve the parent, retaining the final symlink as an entry. Destructive
    /// operations act on links themselves, rather than their targets.
    fn mutation_entry(path: &Path) -> Result<PathBuf, String> {
        let name = path.file_name().ok_or("Cannot mutate a filesystem root")?;
        let parent =
            fs::canonicalize(path.parent().ok_or("Invalid path")?).map_err(|e| e.to_string())?;
        let entry = parent.join(name);
        if let Some(home) = dirs::home_dir().and_then(|home| home.canonicalize().ok()) {
            if home.starts_with(&entry) {
                return Err("Cannot mutate the home directory or its ancestors".into());
            }
        }
        Ok(entry)
    }

    fn copy_recursive(
        from: &Path,
        to: &Path,
        depth: usize,
        remaining: &mut usize,
    ) -> Result<(), String> {
        if depth > 128 || *remaining == 0 {
            return Err("Copy exceeds the depth or 100,000 entry limit".into());
        }
        *remaining -= 1;
        let link = fs::symlink_metadata(from).map_err(|e| e.to_string())?;
        if link.file_type().is_symlink() {
            let target = fs::read_link(from).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, to).map_err(|e| e.to_string())?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
                if link.file_type().is_symlink_dir() {
                    symlink_dir(target, to)
                } else {
                    symlink_file(target, to)
                }
                .map_err(|e| e.to_string())?;
            }
        } else if link.is_dir() {
            fs::create_dir(to).map_err(|e| e.to_string())?;
            for entry in fs::read_dir(from).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                copy_recursive(
                    &entry.path(),
                    &to.join(entry.file_name()),
                    depth + 1,
                    remaining,
                )?;
            }
        } else if link.is_file() {
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(0x00200000);
            } // FILE_FLAG_OPEN_REPARSE_POINT
            let mut source = options.open(from).map_err(|e| e.to_string())?;
            let metadata = source.metadata().map_err(|e| e.to_string())?;
            if !metadata.is_file() {
                return Err("Only regular files can be copied".into());
            }
            // Exclusive creation also prevents following an existing target link.
            let mut destination = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(to)
                .map_err(|e| e.to_string())?;
            destination
                .set_permissions(metadata.permissions())
                .map_err(|e| e.to_string())?;
            std::io::copy(&mut source, &mut destination).map_err(|e| e.to_string())?;
        } else {
            return Err("Cannot copy sockets, devices or special files".into());
        }
        Ok(())
    }

    /// No check-then-rename window that could overwrite another entry.
    fn rename_exclusive(from: &Path, to: &Path) -> Result<(), String> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            use std::os::unix::ffi::OsStrExt;
            let from =
                std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
            let to =
                std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
            #[cfg(target_os = "macos")]
            let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
            // std::fs::rename uses REPLACE_EXISTING on Windows. Explicitly
            // omit that flag so a racing destination creation cannot be lost.
            if unsafe {
                windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
            } == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = (from, to);
            Err("Exclusive rename is unsupported on this host".into())
        }
    }

    fn fs_request(request: FsRequest) -> Result<DevelopmentResponse, String> {
        match request {
            FsRequest::Stat { path } => Ok(DevelopmentResponse::Stat(stat(&absolute(&path)?))),
            FsRequest::List { path } => {
                let dir = absolute(&path)?;
                let mut entries = Vec::new();
                for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.take(10_001) {
                    if entries.len() == 10_000 {
                        return Err("Directory exceeds 10,000 entries".into());
                    }
                    let entry = entry.map_err(|e| e.to_string())?;
                    let link = entry.file_type().map_err(|e| e.to_string())?;
                    let metadata = entry.metadata().map_err(|e| e.to_string())?;
                    let target = fs::metadata(entry.path()).unwrap_or(metadata);
                    entries.push(FileEntry {
                        path: entry.file_name().to_string_lossy().into_owned(),
                        directory: target.is_dir(),
                        symlink: link.is_symlink(),
                        bytes: target.len(),
                    });
                }
                entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
                Ok(DevelopmentResponse::Entries(entries))
            }
            FsRequest::Read { path } => {
                let data = bytes(&absolute(&path)?)?;
                Ok(DevelopmentResponse::File {
                    revision: revision(&data),
                    data,
                })
            }
            FsRequest::Write { path, data } => {
                if data.len() as u64 > FILE_LIMIT {
                    return Err("File exceeds the 8 MiB transfer limit".into());
                }
                replace_file(&absolute(&path)?, &data)
            }
            FsRequest::CreateDir { path } => {
                fs::create_dir_all(absolute(&path)?).map_err(|e| e.to_string())?;
                Ok(DevelopmentResponse::Done)
            }
            FsRequest::CreateFile { path } => {
                fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(absolute(&path)?)
                    .map_err(|e| e.to_string())?;
                Ok(DevelopmentResponse::Done)
            }
            FsRequest::Rename { from, to } => {
                let from = mutation_entry(&absolute(&from)?)?;
                let to = mutation_entry(&absolute(&to)?)?;
                rename_exclusive(&from, &to)?;
                Ok(DevelopmentResponse::Done)
            }
            FsRequest::Copy { from, to } => {
                let from = mutation_entry(&absolute(&from)?)?;
                let to = mutation_entry(&absolute(&to)?)?;
                if to.starts_with(&from) {
                    return Err("Cannot copy an entry into itself or its descendants".into());
                }
                copy_recursive(&from, &to, 0, &mut 100_000)?;
                Ok(DevelopmentResponse::Done)
            }
            FsRequest::Remove { path } => {
                let target = mutation_entry(&absolute(&path)?)?;
                let link = fs::symlink_metadata(&target).map_err(|e| e.to_string())?;
                if link.is_dir() {
                    fs::remove_dir_all(&target)
                } else {
                    fs::remove_file(&target)
                }
                .map_err(|e| e.to_string())?;
                Ok(DevelopmentResponse::Done)
            }
        }
    }

    /// Atomically replace `target`'s contents. A symlink is saved through to
    /// the file it points at: renaming over the link itself would swap it
    /// for a plain file and leave the real file unchanged.
    fn replace_file(target: &Path, data: &[u8]) -> Result<DevelopmentResponse, String> {
        let target = match fs::symlink_metadata(target) {
            Ok(link) if link.file_type().is_symlink() => fs::canonicalize(target)
                .map_err(|_| "The link points to a file that doesn't exist".to_string())?,
            _ => target.to_path_buf(),
        };
        let parent = target.parent().ok_or("Invalid path")?;
        let tmp = parent.join(format!(".dragent-save-{}", uuid::Uuid::new_v4()));
        let result = (|| {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp)
                .map_err(|e| e.to_string())?;
            if let Ok(metadata) = fs::metadata(&target) {
                file.set_permissions(metadata.permissions())
                    .map_err(|e| e.to_string())?;
            }
            file.write_all(data).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
            Ok(DevelopmentResponse::Saved {
                revision: revision(data),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }

    fn bytes(path: &Path) -> Result<Vec<u8>, String> {
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Choose a file".into());
        }
        let mut data = Vec::new();
        (&mut file)
            .take(FILE_LIMIT + 1)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        if data.len() as u64 > FILE_LIMIT {
            return Err("File exceeds the 8 MiB transfer limit".into());
        }
        Ok(data)
    }
    fn project_reply(root: PathBuf) -> DevelopmentResponse {
        let git = crate::git_command()
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .is_ok_and(|o| o.status.success());
        DevelopmentResponse::Project {
            root: root.to_string_lossy().into_owned(),
            git,
        }
    }

    fn accounts_execute(request: AccountsRequest) -> Result<DevelopmentResponse, String> {
        match request {
            AccountsRequest::List => {
                let registry = crate::agent_accounts::Registry::load()?;
                let machine_id = registry.machine_id;
                let profiles: Vec<AccountProfileView> = registry
                    .profiles
                    .iter()
                    .map(|p| AccountProfileView {
                        id: p.id,
                        provider: p.provider.clone(),
                        label: p.label.clone(),
                        kind: p.kind,
                    })
                    .collect();
                let defaults = registry.defaults.clone();
                // Profiles whose binding no longer resolves still appear, with the error.
                let mut identities: Vec<AccountIdentityView> = Vec::new();
                struct Job {
                    target: Option<uuid::Uuid>,
                    provider: String,
                    kind: Option<crate::agent_accounts::CredentialKind>,
                    binding: crate::agent_accounts::Binding,
                    exe: PathBuf,
                }
                let mut jobs: Vec<Job> = Vec::new();
                for p in &registry.profiles {
                    match registry.resolve(
                        &p.provider,
                        None,
                        &crate::agent_accounts::Selection::Profile(p.id),
                    ) {
                        Ok(binding) => jobs.push(Job {
                            target: Some(p.id),
                            provider: p.provider.clone(),
                            kind: Some(p.kind),
                            binding,
                            exe: PathBuf::from(&p.provider),
                        }),
                        Err(error) => identities.push(AccountIdentityView {
                            target: Some(p.id),
                            provider: p.provider.clone(),
                            signed_in: false,
                            email: None,
                            plan: None,
                            kind: Some(p.kind),
                            error: Some(error),
                            credentials_changed_at:
                                crate::agent_accounts::credentials_changed_at_in(
                                    &crate::agent_accounts::root(),
                                    &p.provider,
                                    Some(p.id),
                                ),
                        }),
                    }
                }
                for (name, present) in
                    crate::host_tools::lookup(&["claude".to_string(), "codex".to_string()])
                {
                    if present {
                        jobs.push(Job {
                            target: None,
                            provider: name.clone(),
                            kind: None,
                            binding: crate::agent_accounts::Binding::system(&name),
                            exe: PathBuf::from(&name),
                        });
                    }
                }
                // Probes run in parallel threads; an error becomes `error`, never fails the list.
                let probed: Vec<AccountIdentityView> = std::thread::scope(|s| {
                    let handles: Vec<_> = jobs
                        .into_iter()
                        .map(|job| {
                            s.spawn(move || {
                                match crate::agent_accounts::identity(&job.binding, &job.exe) {
                                    Ok(who) => AccountIdentityView {
                                        target: job.target,
                                        provider: job.provider,
                                        signed_in: who.signed_in,
                                        email: who.email,
                                        plan: who.plan,
                                        kind: job.kind.or(who.kind),
                                        error: None,
                                        credentials_changed_at: who.credentials_changed_at,
                                    },
                                    Err(error) => AccountIdentityView {
                                        target: job.target,
                                        provider: job.provider,
                                        signed_in: false,
                                        email: None,
                                        plan: None,
                                        kind: job.kind,
                                        error: Some(error),
                                        credentials_changed_at:
                                            crate::agent_accounts::credentials_changed_at(
                                                &job.binding,
                                            ),
                                    },
                                }
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| {
                            h.join().unwrap_or(AccountIdentityView {
                                target: None,
                                provider: "unknown".into(),
                                signed_in: false,
                                email: None,
                                plan: None,
                                kind: None,
                                error: Some("account probe failed".into()),
                                credentials_changed_at: None,
                            })
                        })
                        .collect()
                });
                identities.extend(probed);
                Ok(DevelopmentResponse::Accounts(AccountsResponse::List {
                    machine_id,
                    profiles,
                    defaults,
                    identities,
                }))
            }
            AccountsRequest::Add {
                provider,
                label,
                kind,
            } => {
                let profile =
                    crate::agent_accounts::Registry::update(|r| r.add(&provider, &label, kind))?;
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Added(
                    AccountProfileView {
                        id: profile.id,
                        provider: profile.provider,
                        label: profile.label,
                        kind: profile.kind,
                    },
                )))
            }
            AccountsRequest::AddWithKey {
                provider,
                label,
                key,
            } => {
                let profile = crate::agent_accounts::add_with_key(&provider, &label, &key)?;
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Added(
                    AccountProfileView {
                        id: profile.id,
                        provider: profile.provider,
                        label: profile.label,
                        kind: profile.kind,
                    },
                )))
            }
            AccountsRequest::SetKey { id, key } => {
                crate::agent_accounts::save_key(id, &key)?;
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack))
            }
            AccountsRequest::Rename { id, label } => {
                crate::agent_accounts::Registry::update(|r| r.rename(id, &label))?;
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack))
            }
            AccountsRequest::Remove { id } => {
                crate::agent_accounts::Registry::update(|r| r.remove(id))?;
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack))
            }
            AccountsRequest::SetDefault {
                provider,
                selection,
            } => {
                crate::agent_accounts::Registry::update(|r| {
                    r.set_default(&provider, None, selection)
                })?;
                Ok(DevelopmentResponse::Accounts(AccountsResponse::Ack))
            }
            AccountsRequest::SignInCommand { provider, target } => {
                let shell = crate::agent_accounts::host_shell_kind();
                let command = crate::agent_accounts::sign_in_command(&provider, target, shell)?;
                Ok(DevelopmentResponse::Accounts(
                    AccountsResponse::SignInCommand { command, shell },
                ))
            }
            AccountsRequest::LaunchEnv {
                provider,
                selection,
            } => {
                let registry = crate::agent_accounts::Registry::load()?;
                let binding = registry.resolve(&provider, None, &selection)?;
                let env = crate::agent_accounts::launch_env(&binding)?;
                Ok(DevelopmentResponse::Accounts(AccountsResponse::LaunchEnv(
                    env,
                )))
            }
        }
    }
    pub fn execute(request: DevelopmentRequest) -> Result<DevelopmentResponse, String> {
        use DevelopmentRequest::*;
        match request {
            Accounts(request) => accounts_execute(request),
            PullRequests(request) => {
                crate::pull_requests::execute(request).map(DevelopmentResponse::Text)
            }
            PullRequest { root } => {
                let output = crate::host_tools::command("gh")
                    .args(["pr", "view", "--json", "number,title,url,state,isDraft,reviewDecision,baseRefName,headRefName,statusCheckRollup"])
                    .current_dir(absolute(&root)?)
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .env("GH_PROMPT_DISABLED", "1")
                    .env("GH_PAGER", "")
                    .output().map_err(|error| format!("Could not start gh: {error}"))?;
                if !output.status.success() {
                    return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
                }
                Ok(DevelopmentResponse::Text(
                    String::from_utf8_lossy(&output.stdout).into_owned(),
                ))
            }
            Metrics => {
                static SYSTEM: std::sync::OnceLock<std::sync::Mutex<sysinfo::System>> =
                    std::sync::OnceLock::new();
                let mut system = SYSTEM
                    .get_or_init(|| std::sync::Mutex::new(sysinfo::System::new()))
                    .lock()
                    .map_err(|e| e.to_string())?;
                system.refresh_memory();
                system.refresh_cpu_usage();
                let (disk_used, disk_total) = home_disk_usage();
                Ok(DevelopmentResponse::Metrics {
                    cpu_percent: system.global_cpu_usage(),
                    memory_used: system.used_memory(),
                    memory_total: system.total_memory(),
                    uptime_seconds: sysinfo::System::uptime(),
                    disk_used,
                    disk_total,
                })
            }
            HostLog => {
                use std::io::{Seek, SeekFrom};
                let path = dragent_paths::organisation_dir().join("host.log");
                let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
                let len = file.metadata().map_err(|e| e.to_string())?.len();
                file.seek(SeekFrom::Start(len.saturating_sub(64 * 1024)))
                    .map_err(|e| e.to_string())?;
                let mut data = Vec::new();
                file.take(64 * 1024)
                    .read_to_end(&mut data)
                    .map_err(|e| e.to_string())?;
                Ok(DevelopmentResponse::Text(
                    String::from_utf8_lossy(&data).into_owned(),
                ))
            }
            InstallCommand { tool } => Ok(DevelopmentResponse::Command(
                crate::host_tools::install_command(&tool)?,
            )),
            Which { tools } => {
                if tools.len() > crate::host_tools::MAX_LOOKUP {
                    return Err("Too many tools in one lookup".into());
                }
                Ok(DevelopmentResponse::Tools(crate::host_tools::lookup(
                    &tools,
                )))
            }
            ProbeTools { tools } => {
                if tools.len() > crate::host_tools::MAX_LOOKUP {
                    return Err("Too many tools in one lookup".into());
                }
                Ok(DevelopmentResponse::ToolsReport(crate::host_tools::report(
                    &tools,
                )))
            }
            PinTool { name, pin } => {
                crate::host_tools::set_pin(&name, pin)?;
                Ok(DevelopmentResponse::Done)
            }
            Environment => {
                // System tools plus the two agents older apps look for here.
                let names: Vec<String> = crate::host_tools::SYSTEM_TOOLS
                    .iter()
                    .chain(&["claude", "codex"])
                    .map(|tool| tool.to_string())
                    .collect();
                let tools = crate::host_tools::lookup(&names);
                Ok(DevelopmentResponse::Environment {
                    home: dirs::home_dir()
                        .ok_or("Home unavailable")?
                        .to_string_lossy()
                        .into_owned(),
                    os: std::env::consts::OS.into(),
                    arch: std::env::consts::ARCH.into(),
                    shell: crate::terminals::integrated_shell(None)
                        .unwrap_or_else(|_| dragent_pty::resolve_shell(None)),
                    tools,
                })
            }
            OpenProject { path } => Ok(project_reply(project(&path)?)),
            Clone { url, parent, name } => {
                if url.is_empty() || url.starts_with('-') || url.contains(['\n', '\0']) {
                    return Err("Invalid repository URL".into());
                }
                if name.is_empty()
                    || Path::new(&name).components().count() != 1
                    || !matches!(
                        Path::new(&name).components().next(),
                        Some(Component::Normal(_))
                    )
                {
                    return Err("Choose a project folder name".into());
                }
                let parent = project(&parent)?;
                let target = parent.join(name);
                if target.exists() {
                    return Err("Destination already exists".into());
                }
                let out = crate::git_command()
                    .current_dir(&parent)
                    .args(["clone", "--"])
                    .arg(url)
                    .arg(&target)
                    .output()
                    .map_err(|e| e.to_string())?;
                if !out.status.success() {
                    return Err(String::from_utf8_lossy(&out.stderr).into_owned());
                }
                Ok(project_reply(target))
            }
            Init { path } => {
                let root = project(&path)?;
                let out = crate::git_command()
                    .current_dir(&root)
                    .arg("init")
                    .output()
                    .map_err(|e| e.to_string())?;
                if !out.status.success() {
                    return Err(String::from_utf8_lossy(&out.stderr).into_owned());
                }
                Ok(project_reply(root))
            }
            List { root, path } => {
                let root = project(&root)?;
                let dir = contained(&root, &path, true)?;
                let mut entries = Vec::new();
                for entry in fs::read_dir(dir).map_err(|e| e.to_string())?.take(10_001) {
                    if entries.len() == 10_000 {
                        return Err("Directory exceeds 10,000 entries".into());
                    }
                    let entry = entry.map_err(|e| e.to_string())?;
                    let metadata = entry.metadata().map_err(|e| e.to_string())?;
                    entries.push(FileEntry {
                        path: entry
                            .path()
                            .strip_prefix(&root)
                            .map_err(|e| e.to_string())?
                            .to_string_lossy()
                            .into_owned(),
                        directory: metadata.is_dir(),
                        symlink: entry.file_type().map_err(|e| e.to_string())?.is_symlink(),
                        bytes: metadata.len(),
                    });
                }
                entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
                Ok(DevelopmentResponse::Entries(entries))
            }
            Read { root, path } => {
                let data = bytes(&contained(&project(&root)?, &path, true)?)?;
                Ok(DevelopmentResponse::File {
                    revision: revision(&data),
                    data,
                })
            }
            Write {
                root,
                path,
                data,
                expected,
            } => {
                if data.len() as u64 > FILE_LIMIT {
                    return Err("File exceeds the 8 MiB transfer limit".into());
                }
                let root = project(&root)?;
                let target = contained(&root, &path, false)?;
                // Serialize revision checks and atomic replaces on the host.
                static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
                let _guard = WRITE_LOCK.lock().map_err(|e| e.to_string())?;
                if target.exists() {
                    if expected.as_deref() != Some(revision(&bytes(&target)?).as_str()) {
                        return Err("File changed on the host. Reload before saving.".into());
                    }
                } else if expected.is_some() {
                    return Err("File was removed on the host".into());
                }
                // `contained` already checked where an existing link resolves.
                replace_file(&target, &data)
            }
            Mkdir { root, path } => {
                fs::create_dir_all(contained(&project(&root)?, &path, false)?)
                    .map_err(|e| e.to_string())?;
                Ok(DevelopmentResponse::Done)
            }
            Rename { root, from, to } => {
                let root = project(&root)?;
                let resolved = contained(&root, &from, true)?;
                let source = root.join(&from);
                let target = contained(&root, &to, false)?;
                if resolved == root || target.symlink_metadata().is_ok() {
                    return Err("Cannot replace an existing path or rename the project root".into());
                }
                fs::rename(source, target).map_err(|e| e.to_string())?;
                Ok(DevelopmentResponse::Done)
            }
            Delete {
                root,
                path,
                recursive,
            } => {
                let root = project(&root)?;
                let target = contained(&root, &path, true)?;
                if target == root {
                    return Err("Cannot delete the project root".into());
                }
                let link = root.join(path);
                if link
                    .symlink_metadata()
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
                {
                    fs::remove_file(link)
                } else if target.is_dir() {
                    if recursive {
                        fs::remove_dir_all(target)
                    } else {
                        fs::remove_dir(target)
                    }
                } else {
                    fs::remove_file(target)
                }
                .map_err(|e| e.to_string())?;
                Ok(DevelopmentResponse::Done)
            }
            Fs(request) => fs_request(request),
            Search { root, query } => {
                if query.is_empty() {
                    return Err("Enter search text".into());
                }
                let root = project(&root)?;
                let mut dirs = vec![root.clone()];
                let mut lines = Vec::new();
                let mut scanned = 0;
                while let Some(dir) = dirs.pop() {
                    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
                        let entry = entry.map_err(|e| e.to_string())?;
                        scanned += 1;
                        if scanned > 20_000 || lines.len() >= 1_000 {
                            return Ok(DevelopmentResponse::Matches {
                                lines,
                                truncated: true,
                            });
                        }
                        let ty = entry.file_type().map_err(|e| e.to_string())?;
                        if ty.is_symlink() {
                            continue;
                        }
                        if ty.is_dir() {
                            if ![".git", "node_modules", "target", ".dragent"]
                                .contains(&entry.file_name().to_string_lossy().as_ref())
                            {
                                dirs.push(entry.path());
                            }
                        } else if ty.is_file()
                            && entry.metadata().is_ok_and(|m| m.len() <= 1024 * 1024)
                        {
                            let Ok(data) = fs::read(entry.path()) else {
                                continue;
                            };
                            if data.contains(&0) {
                                continue;
                            }
                            let text = String::from_utf8_lossy(&data);
                            for (line, content) in text.lines().enumerate() {
                                if content.contains(&query) {
                                    lines.push(format!(
                                        "{}:{}:{}",
                                        entry.path().strip_prefix(&root).unwrap().display(),
                                        line + 1,
                                        content.chars().take(500).collect::<String>()
                                    ));
                                }
                                if lines.len() >= 1_000 {
                                    return Ok(DevelopmentResponse::Matches {
                                        lines,
                                        truncated: true,
                                    });
                                }
                            }
                        }
                    }
                }
                Ok(DevelopmentResponse::Matches {
                    lines,
                    truncated: false,
                })
            }
        }
    }
}
#[cfg(not(target_family = "wasm"))]
pub use runtime::execute;
