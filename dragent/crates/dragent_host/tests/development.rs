use dragent_host::development::{
    DevelopmentRequest as Request, DevelopmentResponse as Response, execute,
};
use std::path::PathBuf;

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("dragent-development-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn root(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
    fn read(&self, path: &str) -> (Vec<u8>, String) {
        match execute(Request::Read {
            root: self.root(),
            path: path.into(),
        })
        .unwrap()
        {
            Response::File { data, revision } => (data, revision),
            other => panic!("{other:?}"),
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn editing_transferring_and_searching_preserve_host_data() {
    let project = Project::new();
    execute(Request::Mkdir {
        root: project.root(),
        path: "src".into(),
    })
    .unwrap();
    execute(Request::Write {
        root: project.root(),
        path: "src/main.rs".into(),
        data: b"fn main() { println!(\"hello\"); }\n".to_vec(),
        expected: None,
    })
    .unwrap();
    let (data, revision) = project.read("src/main.rs");
    assert!(data.starts_with(b"fn main"));
    assert!(
        execute(Request::Write {
            root: project.root(),
            path: "src/main.rs".into(),
            data: b"lost".to_vec(),
            expected: None
        })
        .is_err()
    );
    execute(Request::Write {
        root: project.root(),
        path: "src/main.rs".into(),
        data: b"changed\n".to_vec(),
        expected: Some(revision.clone()),
    })
    .unwrap();
    assert!(
        execute(Request::Write {
            root: project.root(),
            path: "src/main.rs".into(),
            data: b"stale".to_vec(),
            expected: Some(revision)
        })
        .is_err()
    );
    assert_eq!(project.read("src/main.rs").0, b"changed\n");
    execute(Request::Write {
        root: project.root(),
        path: "binary".into(),
        data: vec![0, 255, 127, 1],
        expected: None,
    })
    .unwrap();
    assert_eq!(project.read("binary").0, vec![0, 255, 127, 1]);
    match execute(Request::Search {
        root: project.root(),
        query: "changed".into(),
    })
    .unwrap()
    {
        Response::Matches { lines, truncated } => {
            assert_eq!(lines, ["src/main.rs:1:changed"]);
            assert!(!truncated);
        }
        other => panic!("{other:?}"),
    }
    execute(Request::Rename {
        root: project.root(),
        from: "src/main.rs".into(),
        to: "src/lib.rs".into(),
    })
    .unwrap();
    assert!(!project.0.join("src/main.rs").exists());
    assert_eq!(project.read("src/lib.rs").0, b"changed\n");
    execute(Request::Delete {
        root: project.root(),
        path: "src/lib.rs".into(),
        recursive: false,
    })
    .unwrap();
    assert!(!project.0.join("src/lib.rs").exists());
}

#[test]
fn paths_and_symlinks_cannot_escape_the_project() {
    let project = Project::new();
    let outside = Project::new();
    std::fs::write(outside.0.join("secret"), b"private").unwrap();
    for path in ["../secret", "/etc/passwd", "src/../../secret"] {
        assert!(
            execute(Request::Read {
                root: project.root(),
                path: path.into()
            })
            .is_err()
        );
        assert!(
            execute(Request::Write {
                root: project.root(),
                path: path.into(),
                data: b"bad".to_vec(),
                expected: None
            })
            .is_err()
        );
        assert!(
            execute(Request::Delete {
                root: project.root(),
                path: path.into(),
                recursive: true
            })
            .is_err()
        );
    }
    assert!(
        execute(Request::Delete {
            root: project.root(),
            path: String::new(),
            recursive: true
        })
        .is_err()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside.0, project.0.join("escape")).unwrap();
        assert!(
            execute(Request::Read {
                root: project.root(),
                path: "escape/secret".into()
            })
            .is_err()
        );
        assert!(
            execute(Request::Write {
                root: project.root(),
                path: "escape/new".into(),
                data: b"bad".to_vec(),
                expected: None
            })
            .is_err()
        );
        assert!(
            execute(Request::Mkdir {
                root: project.root(),
                path: "escape/newdir".into()
            })
            .is_err()
        );
        assert!(
            execute(Request::Delete {
                root: project.root(),
                path: "escape".into(),
                recursive: true
            })
            .is_err()
        );
        assert_eq!(std::fs::read(outside.0.join("secret")).unwrap(), b"private");
    }
}

#[test]
fn concurrent_revision_saves_have_one_winner() {
    let project = Project::new();
    std::fs::write(project.0.join("shared"), b"original").unwrap();
    let (_, revision) = project.read("shared");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = [b"one".to_vec(), b"two".to_vec()]
        .into_iter()
        .map(|data| {
            let barrier = barrier.clone();
            let root = project.root();
            let revision = revision.clone();
            std::thread::spawn(move || {
                barrier.wait();
                execute(Request::Write {
                    root,
                    path: "shared".into(),
                    data,
                    expected: Some(revision),
                })
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
}

/// Saving a symlinked file writes the file it points at and keeps the link,
/// both inside a project and through the machine-wide file requests.
#[cfg(unix)]
#[test]
fn saving_a_symlink_writes_its_target_and_keeps_the_link() {
    use dragent_host::development::FsRequest;
    let project = Project::new();
    std::fs::write(project.0.join("real.yml"), b"one").unwrap();
    std::os::unix::fs::symlink("real.yml", project.0.join("link.yml")).unwrap();
    let (_, revision) = project.read("link.yml");
    execute(Request::Write {
        root: project.root(),
        path: "link.yml".into(),
        data: b"two".to_vec(),
        expected: Some(revision),
    })
    .unwrap();
    assert_eq!(std::fs::read(project.0.join("real.yml")).unwrap(), b"two");
    let link = project.0.join("link.yml");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());

    execute(Request::Fs(FsRequest::Write {
        path: link.to_string_lossy().into_owned(),
        data: b"three".to_vec(),
    }))
    .unwrap();
    assert_eq!(std::fs::read(project.0.join("real.yml")).unwrap(), b"three");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());

    std::os::unix::fs::symlink("missing.yml", project.0.join("dangling.yml")).unwrap();
    assert!(
        execute(Request::Fs(FsRequest::Write {
            path: project.0.join("dangling.yml").to_string_lossy().into_owned(),
            data: b"lost".to_vec(),
        }))
        .is_err()
    );
    // A link that leaves the project still can't be saved through.
    let outside = Project::new();
    std::fs::write(outside.0.join("secret"), b"private").unwrap();
    std::os::unix::fs::symlink(outside.0.join("secret"), project.0.join("out")).unwrap();
    assert!(
        execute(Request::Write {
            root: project.root(),
            path: "out".into(),
            data: b"bad".to_vec(),
            expected: None,
        })
        .is_err()
    );
    assert_eq!(std::fs::read(outside.0.join("secret")).unwrap(), b"private");
}

#[test]
fn git_initialization_and_clone_execute_on_the_host() {
    let project = Project::new();
    let destination = Project::new();
    execute(Request::Init {
        path: project.root(),
    })
    .unwrap();
    assert!(project.0.join(".git").is_dir());
    let response = execute(Request::Clone {
        url: project.root(),
        parent: destination.root(),
        name: "checkout".into(),
    })
    .unwrap();
    assert!(matches!(response, Response::Project { git: true, .. }));
    assert!(destination.0.join("checkout/.git").is_dir());
    for name in ["../outside", "/absolute", "."] {
        assert!(
            execute(Request::Clone {
                url: project.root(),
                parent: destination.root(),
                name: name.into()
            })
            .is_err()
        );
    }
}

#[test]
fn remote_binary_file_messages_use_the_existing_binary_tail_protocol() {
    let response = Response::File {
        data: vec![0, 255, 1, 128],
        revision: "revision".into(),
    };
    let bytes = dragent_host::protocol::encode_message(&response).unwrap();
    assert!(
        matches!(dragent_host::protocol::decode_message::<Response>(&bytes).unwrap(),Response::File { data,.. } if data == vec![0,255,1,128])
    );
}
