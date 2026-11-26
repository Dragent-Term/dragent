//! New terminals start whatever the length of the folders they are told
//! about, through a real host and terminal service, as the app opens them:
//! with its launchers in place, several at once, in a repository's main
//! checkout and in a worktree.
#![cfg(unix)]

use dragent_host::HostClient;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Opened at once.
const TERMINALS: usize = 6;

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dragent-host"))
}

fn shells() -> Vec<&'static str> {
    ["/bin/zsh", "/bin/bash"]
        .into_iter()
        .filter(|shell| Path::new(shell).exists())
        .collect()
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}");
}

/// A folder in `base` whose path is `length` bytes long.
fn folder(base: &Path, length: usize) -> PathBuf {
    let mut path = base.to_path_buf();
    loop {
        let left = length.saturating_sub(path.as_os_str().len() + 1);
        assert!(left > 0, "{} is too long already", base.display());
        // Leaves room for one more name after it.
        let name = if left > 120 { 100 } else { left };
        path.push("d".repeat(name));
        if left == name {
            std::fs::create_dir_all(&path).unwrap();
            assert_eq!(path.as_os_str().len(), length);
            return path;
        }
    }
}

static ONE: Mutex<()> = Mutex::new(());

struct Place {
    _one: MutexGuard<'static, ()>,
    services: dragent_host::testing::ServiceGuard,
    base: PathBuf,
    data: PathBuf,
    client: HostClient,
}

impl Place {
    fn new(length: usize) -> Self {
        let one = ONE.lock().unwrap_or_else(|p| p.into_inner());
        let id = uuid::Uuid::new_v4().simple().to_string();
        let base = Path::new("/tmp").canonicalize().unwrap().join(&id[..8]);
        std::fs::create_dir_all(base.join("home")).unwrap();
        let data = folder(&base, length);
        // What the app writes before its first terminal.
        std::fs::create_dir_all(data.join("bin")).unwrap();
        for name in ["dragent", "dragent-open"] {
            use std::os::unix::fs::PermissionsExt;
            let file = data.join("bin").join(name);
            std::fs::write(&file, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let rc = "HISTFILE=$HOME/history; HISTSIZE=100; SAVEHIST=100\n\
                  HISTCONTROL=ignorespace; export BROWSER=mine\n\
                  [ -n \"$ZSH_VERSION\" ] && setopt histignorespace\n";
        for name in [".zshrc", ".bash_profile"] {
            std::fs::write(base.join("home").join(name), rc).unwrap();
        }
        let repo = base.join("Terminal agent");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
        git(
            &repo,
            &["worktree", "add", "-q", "-b", "work", "../work tree"],
        );
        // SAFETY: tests in this executable serialize on `ONE`.
        unsafe {
            for (name, path) in [
                ("DRAGENT_HOME", data.clone()),
                ("DRAGENT_DEV_HOME", data.clone()),
                (dragent_host::ENDPOINT_FILE_ENV, data.join("host.json")),
                ("DRAGENT_WORKSPACE_DB", base.join("db.sqlite")),
                ("HOME", base.join("home")),
                ("ZDOTDIR", base.join("home")),
                ("XDG_CONFIG_HOME", base.join("home/.config")),
            ] {
                std::env::set_var(name, path);
            }
            for name in [
                "CARGO_TARGET_DIR",
                "CARGO_MAKEFLAGS",
                "MAKEFLAGS",
                "BROWSER",
                "RUSTC_WRAPPER",
            ] {
                std::env::remove_var(name);
            }
        }
        let services = dragent_host::testing::ServiceGuard::new();
        let client = HostClient::ensure_host_with(&helper()).unwrap();
        Self {
            _one: one,
            services,
            base,
            data,
            client,
        }
    }

    /// Open terminals at once in `dir`, and what each said.
    fn open(&self, dir: &Path, shell: &str, command: &str, done: &str) -> Vec<String> {
        let root = dir.to_string_lossy().into_owned();
        let workspace = self.client.create_local_workspace(&root).unwrap();
        let home = self
            .client
            .ensure_terminal_home(&workspace, &root, &root)
            .unwrap()
            .id;
        std::thread::scope(|scope| {
            let opened: Vec<_> = (0..TERMINALS)
                .map(|_| {
                    scope.spawn(|| {
                        let client = HostClient::connect_existing().unwrap();
                        let terminal = client
                            .create_terminal_with_shell(
                                &workspace,
                                &home,
                                Some(command),
                                None,
                                200,
                                50,
                                Some(shell),
                            )
                            .unwrap()
                            .id;
                        let read = || {
                            client
                                .read_terminal(&workspace, &terminal, None)
                                .map(|read| read.0)
                                .unwrap_or_default()
                        };
                        let until = Instant::now() + Duration::from_secs(30);
                        while !read().contains(done) && Instant::now() < until {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        // Anything that comes after it, too.
                        std::thread::sleep(Duration::from_millis(200));
                        let said = read();
                        let _ = client.close_terminal(&workspace, &terminal);
                        said
                    })
                })
                .collect();
            opened.into_iter().map(|t| t.join().unwrap()).collect()
        })
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.services.stop();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn terminals_start_at_any_folder_length_in_a_checkout_and_in_a_worktree() {
    for length in [30, 106, 200, 244] {
        let place = Place::new(length);
        let target = place.data.join("build-cache");
        for shell in shells() {
            for dir in ["Terminal agent", "work tree"] {
                let command = "echo \"T=${CARGO_TARGET_DIR:-none} B=$BROWSER \
                               S=${DRAGENT_SHELL_SETUP:-unset} I=${#DRAGENT_HOST_TERMINAL_ID} \
                               H=$(fc -l -20 2>/dev/null | grep -c 'builtin eva[l]') \
                               $(command -v dragent) end$((40+2))\"";
                for said in place.open(&place.base.join(dir), shell, command, "end42") {
                    let at = format!("{length} {shell} {dir}: {said}");
                    let flat = said.replace("\r\n", "\n");
                    assert!(flat.contains("end42"), "didn't start at {at}");
                    let told = format!("T={}", target.display());
                    assert_eq!(flat.contains(&told), dir == "work tree", "{at}");
                    assert_eq!(flat.contains("T=none"), dir != "work tree", "{at}");
                    // The person's own value stays; ours leaves nothing.
                    assert!(flat.contains("B=mine S=unset I=41 H=0"), "{at}");
                    let launcher = place.data.join("bin/dragent");
                    assert!(flat.contains(&*launcher.to_string_lossy()), "{at}");
                }
            }
        }
    }
}

#[test]
fn a_command_longer_than_an_unread_line_holds_still_runs_whole() {
    let place = Place::new(60);
    let command = format!(
        ": {}; echo end$((40+2))",
        "long ".repeat(if cfg!(target_os = "linux") { 1200 } else { 500 })
    );
    for shell in shells() {
        for said in place.open(&place.base.join("home"), shell, &command, "end42") {
            assert!(said.contains("end42"), "{shell}: {said}");
        }
    }
}
