//! Isolated host/PTY harness and deterministic mock agent CLIs shared by the
//! recovery integration tests. Each test executable runs one test, because
//! the harness points process-wide environment variables at private storage.
#![allow(dead_code)]

use dragent_host::{
    agent_sessions::{self, RestoreTerminal},
    HostClient,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// The shell name `wrap_command_with_helper`/`agent_sessions::quote` use to
/// pick a quoting convention, kept in sync with the shell actually passed to
/// `create_terminal_with_shell` below.
#[cfg(unix)]
const SHELL_NAME: &str = "sh";
#[cfg(not(unix))]
const SHELL_NAME: &str = "powershell";

/// `create_terminal_with_shell`'s preferred shell. `None` lets the host
/// resolve the platform default (there is no single well-known shell path on
/// Windows the way `/bin/bash` is on Unix).
#[cfg(unix)]
const TERMINAL_SHELL: Option<&str> = Some("/bin/bash");
#[cfg(not(unix))]
const TERMINAL_SHELL: Option<&str> = None;

/// A `python3`/`python`/`py` interpreter on `PATH`, needed on Windows since
/// there is no shebang-based execute bit to run `MOCK` directly as on Unix.
#[cfg(not(unix))]
fn python_interpreter() -> &'static str {
    use std::sync::OnceLock;
    static CACHE: OnceLock<&'static str> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        ["python3", "python", "py"]
            .into_iter()
            .find(|name| dragent_host::host_tools::find_in(name, &dirs).is_some())
            .expect("mock-agent tests need a python3/python/py interpreter on PATH")
    })
}

#[cfg(unix)]
fn mock_launch_command(bin: &std::path::Path) -> String {
    format!("'{}'", bin.display())
}
#[cfg(not(unix))]
fn mock_launch_command(bin: &std::path::Path) -> String {
    format!("{} '{}'", python_interpreter(), bin.display())
}

/// Portable SIGKILL: `kill -9` on unix, `taskkill /F` on Windows.
fn kill_process(pid: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
    }
}

pub struct Harness {
    pub dir: PathBuf,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    pub client: HostClient,
    pub workspace: String,
    pub home: String,
}

impl Harness {
    pub fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("dragent-agent-recovery-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut previous = Vec::new();
        for (name, path) in [
            ("DRAGENT_HOME", dir.join("data")),
            (dragent_host::ENDPOINT_FILE_ENV, dir.join("endpoint.json")),
            ("DRAGENT_WORKSPACE_DB", dir.join("workspace.sqlite")),
            // Agents' user-level hook configs (Droid, Muse) are written here,
            // never into the real home.
            ("HOME", dir.join("home")),
            ("ZDOTDIR", dir.join("home")),
            ("XDG_CONFIG_HOME", dir.join("home").join(".config")),
        ] {
            if name != dragent_host::ENDPOINT_FILE_ENV && name != "DRAGENT_WORKSPACE_DB" {
                std::fs::create_dir_all(&path).unwrap();
            }
            previous.push((name, std::env::var_os(name)));
            // This integration executable contains one test and its private host.
            unsafe {
                std::env::set_var(name, path);
            }
        }
        let client = HostClient::ensure_host_with(&helper()).unwrap();
        let root = dir.to_str().unwrap();
        let workspace = client.create_local_workspace(root).unwrap();
        let home = client
            .ensure_terminal_home(&workspace, root, root)
            .unwrap()
            .id;
        Self {
            dir,
            previous,
            client,
            workspace,
            home,
        }
    }
    pub fn launch(&self, provider: &str) -> (Uuid, String) {
        let bin = self.dir.join(provider);
        // Never rewrite a mock another pane is still executing.
        if !bin.exists() {
            std::fs::write(&bin, MOCK).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let pane = Uuid::new_v4();
        let launch = agent_sessions::wrap_command_with_helper(
            pane,
            SHELL_NAME,
            &mock_launch_command(&bin),
            &helper(),
        );
        let session = self
            .client
            .create_terminal_with_shell(
                &self.workspace,
                &self.home,
                Some(&launch),
                Some(self.dir.to_str().unwrap()),
                80,
                24,
                TERMINAL_SHELL,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !agent_sessions::load(pane).is_some_and(|r| r.conversation_id.is_some()) {
            assert!(
                Instant::now() < deadline,
                "{provider} did not report a conversation: {:?}; terminal: {}",
                agent_sessions::load(pane),
                self.output(&session.id),
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        (pane, session.id)
    }
    pub fn restore(
        &self,
        client: &HostClient,
        pane: Uuid,
        terminal: &str,
    ) -> Result<(String, String), String> {
        self.restore_outcome(client, pane, terminal)
            .map(|r| (r.home_id, r.terminal_id))
    }
    pub fn restore_outcome(
        &self,
        client: &HostClient,
        pane: Uuid,
        terminal: &str,
    ) -> Result<agent_sessions::Restored, String> {
        agent_sessions::restore_terminal(
            client,
            RestoreTerminal {
                pane_uuid: pane,
                workspace_id: &self.workspace,
                home_id: &self.home,
                terminal_id: terminal,
                root: self.dir.to_str().unwrap(),
                cwd: self.dir.to_str().unwrap(),
                shell: TERMINAL_SHELL,
                allow_new_shell: false,
                helper: Some(&helper()),
            },
        )
    }
    pub fn output(&self, terminal: &str) -> String {
        self.client
            .read_terminal(&self.workspace, terminal, None)
            .unwrap()
            .0
    }
    pub fn input(&self, terminal: &str, id: &str, token: &str) {
        self.client
            .send_terminal_input(&self.workspace, terminal, format!("{token}\n").as_bytes())
            .unwrap();
        wait(|| {
            self.output(terminal)
                .contains(&format!("INPUT|{id}|{token}"))
                .then_some(())
        });
    }
    pub fn crash(&self, terminal: &str) {
        let text = wait(|| {
            let t = self.output(terminal);
            t.contains("READY|").then_some(t)
        });
        let line = text
            .lines()
            .find_map(|line| line.find("READY|").map(|i| &line[i..]))
            .unwrap();
        let parts: Vec<_> = line.trim().split('|').collect();
        // Both PIDs came from our own isolated mock child. Kill the monitor
        // first, exactly as a machine crash prevents recording normal exit.
        for pid in [parts[3], parts[2]] {
            let pid: i32 = pid.parse().unwrap();
            assert!(pid > 1 && pid != std::process::id() as i32);
            kill_process(pid);
        }
        self.client
            .close_terminal(&self.workspace, terminal)
            .unwrap();
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.client.shutdown();
        std::thread::sleep(Duration::from_millis(200));
        for (name, previous) in &self.previous {
            unsafe {
                if let Some(v) = previous {
                    std::env::set_var(name, v);
                } else {
                    std::env::remove_var(name);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
pub fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}
pub fn wait<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for isolated agent/host"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub const MOCK: &str = r#"#!/usr/bin/env python3
import os,sys,json,re,signal,subprocess,uuid,shlex
# Like real agents, exit cleanly when the system asks.
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
args=sys.argv[1:]
provider=os.path.basename(sys.argv[0])
here=os.path.dirname(sys.argv[0])
if args[:1]==['create-chat']:
    minted=str(uuid.uuid4())
    open(os.path.join(here,'minted'),'a').write(minted+'\n')
    print(minted,flush=True)
    sys.exit(0)
id=None
for flag in ['--resume','--session','-s','--session-id']:
    if flag in args: id=args[args.index(flag)+1]
if 'resume' in args: id=args[args.index('resume')+1]
# A conversation exists once it received a prompt; resuming one that never
# did fails, as it does for the real agents.
store=os.path.join(here,'used-conversations')
used=lambda: open(store).read().split() if os.path.exists(store) else []
resume_flag={'claude':'--resume','codex':'resume','opencode':'--session','mimo':'--session',
    'gemini':'--resume','qwen':'--resume','copilot':'--resume','codebuddy':'--resume',
    'droid':'--resume','muse':'resume'}.get(provider)
if id=='missing-transcript' or (resume_flag in args and id not in used()):
    print('No conversation found with id '+str(id),flush=True)
    sys.exit(2)
preassigned=('gemini','qwen','copilot','codebuddy','kimi','cursor-agent')
if provider in preassigned:
    # Identity is chosen by the launcher; the agent never reports it.
    assert id, 'launched without an exact conversation id: %r'%args
resuming=id is not None
id=id or str(uuid.uuid4())
def user_hooks(event, payload):
    # Hooks from the agent's own user-level config. Muse runs them with a
    # cleared environment: the pane must be found without it.
    home=os.environ['HOME']
    if provider=='droid':
        events=json.load(open(os.path.join(home,'.factory','hooks.json')))
        env=None
    else:
        config=os.environ.get('XDG_CONFIG_HOME') or os.path.join(home,'.config')
        events=json.load(open(os.path.join(config,'muse','settings.json')))['hooks']
        env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':home}
    for group in events.get(event,[]):
        for h in group['hooks']:
            subprocess.run(h['command'],shell=True,input=json.dumps(dict(payload,hook_event_name=event)),text=True,env=env,check=True)
hook=None
if provider in preassigned:
    pass
elif provider in ('droid','muse'):
    user_hooks('SessionStart',{'session_id':id,'source':'resume' if resuming else 'startup'})
elif provider=='claude':
    plugin=args[args.index('--plugin-dir')+1]
    data=json.load(open(os.path.join(plugin,'hooks','hooks.json')))
    hook=shlex.split(data['hooks']['SessionStart'][0]['hooks'][0]['command'])
elif provider=='codex':
    config=args[args.index('-c')+1]
    hook=shlex.split(json.loads(re.search(r'command=("(?:[^"\\]|\\.)*")',config).group(1)))
else:
    data=json.loads(os.environ['MIMOCODE_CONFIG_CONTENT' if provider=='mimo' else 'OPENCODE_CONFIG_CONTENT'])
    plugin=data['plugin'][-1].removeprefix('file://')
    script=open(plugin).read()
    # The lifecycle plugin stores its command once, then passes it to Bun.
    hook=json.loads(re.search(r'const helper\s*=\s*(\[.*?\]);',script).group(1))
if hook: subprocess.run(hook,input=json.dumps({'session_id':id}),text=True,check=True)
print('READY|'+id+'|'+str(os.getpid())+'|'+str(os.getppid()),flush=True)
for line in sys.stdin:
    token=line.strip()
    if token=='quit': break
    open(store,'a').write(id+'\n')
    if provider in ('droid','muse'):
        user_hooks('UserPromptSubmit',{'session_id':id,'prompt':token})
    print('INPUT|'+id+'|'+token,flush=True)
"#;
