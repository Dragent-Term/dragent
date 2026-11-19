//! Build output left behind in Dragent's managed worktrees.
//!
//! Every agent works in a worktree of its own, and an agent that builds Rust
//! there gets a private Cargo `target` folder: several GB each, and 20 agents
//! mean 20 of them. Nothing ever removed them, so they outlived the work.
//!
//! This sweeps them. A folder is removed only when all of these hold:
//!
//! - it is a Cargo `target` folder (it carries Cargo's own `CACHEDIR.TAG`
//!   marker), directly inside a worktree under the managed worktrees root;
//! - no build has written to it for [`STALE_AFTER`] (so none is running);
//! - nothing has a file open inside it (`lsof`, when available).
//!
//! It is all regenerable: the next `cargo build` in that worktree recreates
//! it. Source, git state and everything else in the worktree is untouched.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long a `target` folder must be untouched before it is removed.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the sweep runs while the app is open.
pub const SWEEP_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How deep inside a worktree a `target` folder is looked for
/// (`dragent/target` is one level down in a workspace with subfolders).
const SEARCH_DEPTH: usize = 2;

/// What one sweep removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub folders: usize,
    pub bytes: u64,
}

/// Whether `dir` is a Cargo `target` folder: Cargo writes `CACHEDIR.TAG`
/// (with this signature) at its top, and nothing else does.
fn is_cargo_target(dir: &Path) -> bool {
    std::fs::read(dir.join("CACHEDIR.TAG"))
        .map(|tag| tag.starts_with(b"Signature: 8a477f597d28d172789f06886806bc55"))
        .unwrap_or(false)
}

/// The newest write inside a `target` folder, looked for where a build
/// leaves marks: the folder itself and each profile's top and `deps`.
fn last_build(target: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut note = |path: &Path| {
        if let Ok(modified) = std::fs::metadata(path).and_then(|meta| meta.modified()) {
            newest = Some(newest.map_or(modified, |current| current.max(modified)));
        }
    };
    note(target);
    let Ok(profiles) = std::fs::read_dir(target) else { return newest };
    for profile in profiles.flatten() {
        let path = profile.path();
        if !path.is_dir() {
            continue;
        }
        note(&path);
        note(&path.join("deps"));
        note(&path.join("incremental"));
        note(&path.join(".fingerprint"));
    }
    newest
}

/// Bytes under `dir`, not following symlinks.
fn size_of(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Whether any process has a file open under `dir`. Without `lsof` this
/// can't tell, and the answer is "yes" (nothing is removed on a guess).
fn in_use(dir: &Path) -> bool {
    match std::process::Command::new("lsof").arg("-t").arg("+D").arg(dir).output() {
        Ok(output) => !output.stdout.is_empty(),
        Err(_) => true,
    }
}

/// Every `target` folder to consider under `root`: `root/<project>/<worktree>`
/// is a worktree, and its `target` folders are looked for [`SEARCH_DEPTH`]
/// levels down.
fn candidates(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(root) else { return out };
    for project in projects.flatten() {
        let Ok(worktrees) = std::fs::read_dir(project.path()) else { continue };
        for worktree in worktrees.flatten() {
            let mut level = vec![worktree.path()];
            for _ in 0..SEARCH_DEPTH {
                let mut next = Vec::new();
                for dir in level {
                    let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        // Symlinks are never followed out of the worktree.
                        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                            continue;
                        }
                        match path.file_name().and_then(|name| name.to_str()) {
                            Some("target") if is_cargo_target(&path) => out.push(path),
                            Some(".git") | Some("node_modules") => {}
                            _ => next.push(path),
                        }
                    }
                }
                level = next;
            }
        }
    }
    out
}

/// Remove the stale `target` folders under `root`. `now` and `check_in_use`
/// are parameters so the rules can be tested.
pub fn sweep(root: &Path, now: SystemTime, check_in_use: impl Fn(&Path) -> bool) -> Swept {
    let mut swept = Swept::default();
    for target in candidates(root) {
        let Some(built) = last_build(&target) else { continue };
        let idle = now.duration_since(built).unwrap_or_default();
        if idle < STALE_AFTER || check_in_use(&target) {
            continue;
        }
        let bytes = size_of(&target);
        match std::fs::remove_dir_all(&target) {
            Ok(()) => {
                log::info!(
                    "[sweep] removed idle build output {} ({} MB, untouched {} h)",
                    target.display(),
                    bytes / (1024 * 1024),
                    idle.as_secs() / 3600
                );
                swept.folders += 1;
                swept.bytes += bytes;
            }
            Err(error) => log::warn!("[sweep] couldn't remove {}: {error}", target.display()),
        }
    }
    swept
}

/// Sweep this organisation's managed worktrees now and every
/// [`SWEEP_EVERY`], on a thread of its own (never the UI thread).
#[cfg(not(target_family = "wasm"))]
pub fn start() {
    let _ = std::thread::Builder::new().name("worktree-sweep".into()).spawn(|| loop {
        // A little after launch, so the first sweep doesn't compete with start-up.
        std::thread::sleep(Duration::from_secs(90));
        // Read each time: the organisation can change while the app runs.
        let root = dragent_paths::organisation_dir().join("worktrees");
        let swept = sweep(&root, SystemTime::now(), in_use);
        if swept.folders > 0 {
            log::info!("[sweep] freed {} MB from {} idle build folders", swept.bytes / (1024 * 1024), swept.folders);
        }
        std::thread::sleep(SWEEP_EVERY);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dragent-sweep-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// A worktree at `<root>/<project>/<name>` with a Cargo `target` under
    /// `sub`, last built `age` ago.
    fn worktree(root: &Path, name: &str, sub: &str, age: Duration, tagged: bool) -> PathBuf {
        let worktree = root.join("proj").join(name);
        let target = worktree.join(sub).join("target");
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        fs::write(worktree.join("src.rs"), "fn main() {}").unwrap();
        fs::write(target.join("debug/deps/blob"), vec![0u8; 4096]).unwrap();
        if tagged {
            fs::write(target.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# Cargo").unwrap();
        }
        let then = SystemTime::now() - age;
        for dir in [&target, &target.join("debug"), &target.join("debug/deps")] {
            fs::File::open(dir).unwrap().set_modified(then).unwrap();
        }
        worktree
    }

    #[test]
    fn only_idle_cargo_build_output_goes_and_the_worktree_stays() {
        let root = tree("rules");
        let day = Duration::from_secs(24 * 3600);
        let old = worktree(&root, "old", "dragent", day * 3, true);
        let fresh = worktree(&root, "fresh", "dragent", Duration::from_secs(600), true);
        let untagged = worktree(&root, "untagged", "", day * 5, false);
        let busy = worktree(&root, "busy", "", day * 5, true);
        let swept = sweep(&root, SystemTime::now(), |target| target.starts_with(&busy));
        assert_eq!(swept.folders, 1);
        assert!(swept.bytes >= 4096);
        assert!(!old.join("dragent/target").exists(), "idle build output is removed");
        assert!(old.join("src.rs").exists(), "the worktree itself is untouched");
        assert!(fresh.join("dragent/target").exists(), "a recent build is kept");
        assert!(untagged.join("target").exists(), "a folder named target that isn't Cargo's is kept");
        assert!(busy.join("target").exists(), "a folder something has open is kept");
        let again = sweep(&root, SystemTime::now(), |_| false);
        assert_eq!(again.folders, 1, "the busy one goes once it is free");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_target_outside_the_managed_root_is_never_looked_at() {
        let root = tree("outside");
        let managed = root.join("managed");
        fs::create_dir_all(&managed).unwrap();
        let day = Duration::from_secs(24 * 3600);
        let kept = worktree(&root.join("projects"), "app", "", day * 9, true);
        let swept = sweep(&managed, SystemTime::now(), |_| false);
        assert_eq!(swept, Swept::default());
        assert!(kept.join("target").exists(), "a build folder outside the managed root is left alone");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symlink_out_of_a_worktree_is_not_followed() {
        let root = tree("link");
        let outside = root.join("outside-target");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55").unwrap();
        fs::File::open(&outside).unwrap().set_modified(SystemTime::now() - Duration::from_secs(9 * 24 * 3600)).unwrap();
        let worktree = root.join("proj/wt");
        fs::create_dir_all(&worktree).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, worktree.join("target")).unwrap();
        let swept = sweep(&root, SystemTime::now(), |_| false);
        assert_eq!(swept.folders, 0);
        assert!(outside.join("CACHEDIR.TAG").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
