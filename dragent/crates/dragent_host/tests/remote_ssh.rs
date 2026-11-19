//! Opt-in live SSH acceptance test. The fixture script uses an isolated sshd,
//! home directory, keys and known_hosts; no user's real host is modified.
use dragent_host::{
    development::{
        DevelopmentRequest as Request, DevelopmentResponse as Response, ToolPin, ToolQuery,
        ToolStatus,
    },
    protocol::{AgentDispatch, PromptTransport},
    remote::SshHost,
    HostClient,
};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

/// The fixture scripts written here run on the SSH target, which this
/// acceptance test's fixture (`scripts/test-remote-ssh.sh`) always sets up as
/// a POSIX `/bin/sh` remote regardless of the client platform running this
/// test — so the execute bit only needs setting when `root` is also a local
/// Unix path (loopback/bind-mount fixture).
#[cfg(unix)]
fn make_executable(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(not(unix))]
fn make_executable(_path: &str) {}

struct HostGuard(HostClient);
impl Drop for HostGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

#[test]
#[ignore = "requires the isolated SSH fixture; run scripts/test-remote-ssh.sh"]
fn remote_development_survives_transport_loss() {
    let config = std::env::var("DRAGENT_TEST_SSH_CONFIG").expect("fixture config");
    let host: SshHost = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
    // A first connection refuses the unknown host key until it's confirmed,
    // even though the fixture's ssh_config asks for accept-new.
    let refused = HostClient::connect_remote(host.clone()).expect_err("unknown host key");
    assert_eq!(
        dragent_host::remote::host_key_problem(&refused),
        Some(dragent_host::remote::HostKeyProblem::Unknown),
        "{refused}"
    );
    let key = host
        .unknown_host_key()
        .unwrap()
        .expect("the fixture's key is new");
    assert!(key.fingerprints[0].starts_with("ED25519 SHA256:"), "{key:?}");
    host.trust_host_key(&key).unwrap();
    assert_eq!(host.unknown_host_key().unwrap(), None);
    let client = HostClient::connect_remote(host.clone()).expect("provision and connect");
    let _guard = HostGuard(client.clone());
    let Response::Environment { home, shell, .. } =
        client.development(Request::Environment).unwrap()
    else {
        panic!("environment")
    };
    assert_ne!(
        dragent_shell_hooks::ShellKind::from_program(&shell),
        dragent_shell_hooks::ShellKind::Other
    );
    assert!(
        home.contains("dragent-ssh-test"),
        "isolated remote home: {home}"
    );
    let Response::Metrics { memory_total, .. } = client.development(Request::Metrics).unwrap()
    else {
        panic!("metrics")
    };
    assert!(memory_total > 0);
    assert!(matches!(
        client
            .development(Request::InstallCommand { tool: "git".into() })
            .unwrap(),
        Response::Command(_)
    ));
    assert!(client
        .development(Request::InstallCommand {
            tool: "git; touch injected".into()
        })
        .is_err());
    // Tools resolve through the remote user's own shell PATH.
    let Response::Tools(tools) = client
        .development(Request::Which {
            tools: vec![
                "git".into(),
                "sh".into(),
                "dragent-missing-tool".into(),
                "../bin/sh".into(),
            ],
        })
        .unwrap()
    else {
        panic!("tools")
    };
    assert_eq!(
        tools,
        vec![
            ("git".into(), true),
            ("sh".into(), true),
            ("dragent-missing-tool".into(), false),
            ("../bin/sh".into(), false),
        ]
    );
    let Response::Command(rust) = client
        .development(Request::InstallCommand { tool: "cargo".into() })
        .unwrap()
    else {
        panic!("command")
    };
    assert!(rust.starts_with("sh -c ") && rust.contains("rustup"), "{rust}");
    let root = format!("{home}/project");
    std::fs::create_dir_all(&root).unwrap(); // fixture lives on loopback
    client
        .development(Request::Init { path: root.clone() })
        .unwrap();
    client
        .development(Request::Write {
            root: root.clone(),
            path: "README.md".into(),
            data: b"remote project\n".to_vec(),
            expected: None,
        })
        .unwrap();
    let Response::Entries(entries) = client.development(Request::Fs(
        dragent_host::development::FsRequest::List { path: root.clone() },
    )).expect("runtime must support the desktop's filesystem protocol") else {
        panic!("filesystem listing");
    };
    assert!(entries.iter().any(|entry| entry.path.ends_with("README.md")));
    let Response::File { data, revision } = client
        .development(Request::Read {
            root: root.clone(),
            path: "README.md".into(),
        })
        .unwrap()
    else {
        panic!("read")
    };
    assert_eq!(data, b"remote project\n");
    client
        .development(Request::Write {
            root: root.clone(),
            path: "README.md".into(),
            data: b"edited remotely\n".to_vec(),
            expected: Some(revision),
        })
        .unwrap();
    let (code, _, _) = client.run_git(&root, &["add", "--all"]).unwrap();
    assert_eq!(code, 0);
    let (code, _, err) = client
        .run_git(
            &root,
            &[
                "-c",
                "user.name=Remote Test",
                "-c",
                "user.email=remote@example.com",
                "commit",
                "-m",
                "remote initial",
            ],
        )
        .unwrap();
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    let workspace = client.create_workspace(&root).unwrap();
    let home_state = client
        .ensure_terminal_home(&workspace, &root, &root)
        .unwrap();
    let worktree = client
        .create_worktree(&workspace, "remote-test", &root, "remote-test")
        .unwrap();
    assert!(worktree.path.starts_with(&home));
    let session = client
        .create_terminal_with_shell(
            &workspace,
            &home_state.id,
            None,
            Some(&root),
            100,
            30,
            Some("/bin/sh"),
        )
        .unwrap();
    assert_command_completion(&client, &workspace, &session.id);
    // Exercise the default selected on the host as well as Bash (the common
    // Linux fallback); the fixture's SSH environment deliberately uses /bin/sh.
    for shell in [None, Some("/bin/bash")] {
        let extra = client
            .create_terminal_with_shell(
                &workspace,
                &home_state.id,
                None,
                Some(&root),
                100,
                30,
                shell,
            )
            .unwrap();
        assert_command_completion(&client, &workspace, &extra.id);
        client.close_terminal(&workspace, &extra.id).unwrap();
    }
    assert_interactive_input(&client, &workspace, &home_state.id, &root);
    client
        .resize_terminal(&workspace, &session.id, 120, 40)
        .unwrap();
    let original_host = client.host_info().unwrap().host_id;
    let same_host = HostClient::connect_remote(host).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        client.remote_connection().unwrap(),
        same_host.remote_connection().unwrap()
    ));
    let rediscovered = same_host.create_workspace(&root).unwrap();
    assert_eq!(rediscovered, workspace);
    assert!(same_host
        .list_terminals(&rediscovered)
        .unwrap()
        .iter()
        .any(|s| s.id == session.id));
    client.remote_connection().unwrap().disconnect();
    assert_eq!(client.host_info().unwrap().host_id, original_host);
    // An intentional machine disconnect must suppress automatic transport
    // recovery; explicitly connecting again reuses the same remote runtime.
    client.remote_connection().unwrap().suspend();
    assert!(client
        .host_info()
        .unwrap_err()
        .contains("Machine disconnected"));
    let resumed =
        HostClient::connect_remote_explicit(client.remote_connection().unwrap().host.clone(), None)
            .unwrap();
    assert!(std::sync::Arc::ptr_eq(
        client.remote_connection().unwrap(),
        resumed.remote_connection().unwrap()
    ));
    assert_eq!(client.host_info().unwrap().host_id, original_host);
    assert!(
        client
            .read_terminal(&workspace, &session.id, None)
            .unwrap()
            .1
    );
    let config = client
        .add_agent_config(
            "Remote test",
            "/bin/sh",
            vec!["-c".into()],
            PromptTransport::Argv,
            vec![],
            Default::default(),
        )
        .unwrap();
    let agent = client
        .run_agent(
            &workspace,
            &home_state.id,
            &config.id,
            "printf REMOTE_AGENT_OK",
            AgentDispatch::default(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = client.agent_run(&agent.id).unwrap();
        if snapshot.output.contains("REMOTE_AGENT_OK") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "agent output: {}",
            snapshot.output
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_tool_choices(&client, &workspace, &home_state.id, &root);
    assert_remote_coordination(&client, &root);
    // The loopback fixture serves the preview from this process. A fixture on
    // another machine (a container) serves it there, on the port given in
    // DRAGENT_TEST_REMOTE_PREVIEW_PORT.
    let (remote_port, server) = match std::env::var("DRAGENT_TEST_REMOTE_PREVIEW_PORT") {
        Ok(port) => (port.parse().expect("preview port"), None),
        Err(_) => {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.write_all(b"REMOTE_PREVIEW_OK").unwrap();
            });
            (port, Some(server))
        }
    };
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let local_port = probe.local_addr().unwrap().port();
    drop(probe);
    let forwarding = client
        .remote_connection()
        .unwrap()
        .forward_port(remote_port, local_port)
        .unwrap();
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", local_port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut data = String::new();
    socket.read_to_string(&mut data).unwrap();
    assert_eq!(data, "REMOTE_PREVIEW_OK");
    if let Some(server) = server {
        server.join().unwrap();
    }
    client.remote_connection().unwrap().disconnect();
    assert!(client.ping()); // recreates the preview and wire forwards together
    assert!(std::net::TcpStream::connect(("127.0.0.1", local_port)).is_ok());
    drop(forwarding);
    assert!(std::net::TcpListener::bind(("127.0.0.1", local_port)).is_ok());
    client.close_terminal(&workspace, &session.id).unwrap();
    client.shutdown().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    client.remote_connection().unwrap().disconnect();
    assert!(
        !client.ping(),
        "transport recovery must not restart a stopped host"
    );
    client
        .remote_connection()
        .unwrap()
        .start_or_update(false)
        .expect("explicit start");
    assert!(client.ping());
    let Response::File { data, .. } = client
        .development(Request::Read {
            root,
            path: "README.md".into(),
        })
        .unwrap()
    else {
        panic!("persisted file")
    };
    assert_eq!(data, b"edited remotely\n");
    assert!(matches!(
        client.development(Request::HostLog).unwrap(),
        Response::Text(_)
    ));
    client.shutdown().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    client
        .remote_connection()
        .unwrap()
        .start_or_update(true)
        .expect("update keeps the configured development binary");
    assert!(client.ping());
    client.shutdown().unwrap();
}

fn assert_command_completion(client: &HostClient, workspace: &str, session: &str) {
    // PTY echo alone does not prove execution or command completion.
    let mut offset = client
        .read_terminal_chunk(&workspace, session, 0, 1 << 20)
        .unwrap()
        .total;
    for (command, status, expected) in [
        ("ls", 0, "README.md"),
        ("false", 1, ""),
        ("cd ..", 0, ""),
        ("printf 'REMOTE_BUILD_OK\\n'", 0, "REMOTE_BUILD_OK"),
    ] {
        client
            .send_terminal_input(&workspace, session, format!("{command}\r").as_bytes())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut bytes = Vec::new();
        loop {
            let chunk = client
                .read_terminal_chunk(&workspace, session, offset, 1 << 20)
                .unwrap();
            offset = chunk.base + chunk.data.len() as u64;
            assert!(
                chunk.exit_code.is_none(),
                "interactive shell must remain alive"
            );
            bytes.extend(chunk.data);
            let output = String::from_utf8_lossy(&bytes);
            let started = format!("\x1b]7979;preexec;{command}");
            let completed = format!("\x1b]7979;precmd;{status};");
            if let Some(start) = output.find(&started) {
                if let Some(end) = output[start..].find(&completed) {
                    assert!(
                        output[start..start + end].contains(expected),
                        "missing output: {output:?}"
                    );
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "command {command:?} never completed: {output:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn assert_interactive_input(client: &HostClient, workspace: &str, home: &str, root: &str) {
    let path = format!("{root}/interactive_input.py");
    // A real raw-mode primary-screen program, like an agent's setup menu.
    // No provider account, network call, or model usage is involved.
    std::fs::write(
        &path,
        r#"import os, sys, termios, tty
fd = sys.stdin.fileno()
saved = termios.tcgetattr(fd)
try:
    tty.setraw(fd)
    os.write(1, b'REMOTE_INPUT_READY\r\n')
    keys = b''
    while len(keys) < 4:
        keys += os.read(fd, 4 - len(keys))
    os.write(1, b'RECEIVED_' + keys.hex().encode() + b'\r\n')
finally:
    termios.tcsetattr(fd, termios.TCSANOW, saved)
"#,
    )
    .unwrap();
    let session = client
        .create_terminal_with_shell(workspace, home, None, Some(root), 80, 24, None)
        .unwrap();
    client
        .send_terminal_input(workspace, &session.id, b"python3 interactive_input.py\r")
        .unwrap();
    let mut offset = 0;
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let chunk = client
            .read_terminal_chunk(workspace, &session.id, offset, 1 << 20)
            .unwrap();
        offset = chunk.base + chunk.data.len() as u64;
        output.extend(chunk.data);
        if String::from_utf8_lossy(&output).contains("REMOTE_INPUT_READY") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "interactive program never became ready"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut resumed = false;
    client
        .stream_terminal_operations(workspace, &session.id, 0, |chunk| {
            if let Some(resume) = chunk.resume {
                assert!(resume.shell_ready);
                assert_eq!(
                    resume.command.as_deref(),
                    Some("python3 interactive_input.py")
                );
                resumed = true;
            }
            false
        })
        .unwrap();
    assert!(
        resumed,
        "GUI attach must receive the running command, not an idle checkpoint"
    );
    // Down arrow and Enter must reach the remote PTY byte for byte.
    client
        .send_terminal_input(workspace, &session.id, b"\x1b[B\r")
        .unwrap();
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let chunk = client
            .read_terminal_chunk(workspace, &session.id, offset, 1 << 20)
            .unwrap();
        offset = chunk.base + chunk.data.len() as u64;
        output.extend(chunk.data);
        let text = String::from_utf8_lossy(&output);
        if text.contains("RECEIVED_1b5b420d") && text.contains("\x1b]7979;precmd;0;") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "interactive input did not complete: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    client.close_terminal(workspace, &session.id).unwrap();
}

/// Tool reports over SSH, and a chosen copy that the runtime's own git and
/// agent launches follow. The project folder is shared with the remote side
/// (loopback, or a bind mount for a container), so wrappers written here run
/// there.
fn assert_tool_choices(client: &HostClient, workspace: &str, home: &str, root: &str) {
    let query = |name: &str| ToolQuery {
        name: name.into(),
        version_args: vec!["--version".into()],
    };
    let probe = |names: &[&str]| {
        let Response::ToolsReport(report) = client
            .development(Request::ProbeTools {
                tools: names.iter().map(|name| query(name)).collect(),
            })
            .unwrap()
        else {
            panic!("tools report")
        };
        report
    };
    let report = probe(&["git", "dragent-missing-tool"]);
    let git = &report.tools[0];
    assert_eq!(git.status, ToolStatus::Ready, "{git:?}");
    assert!(git.version.is_some() && !git.copies.is_empty(), "{git:?}");
    assert_eq!(report.tools[1].status, ToolStatus::Missing);
    let real_git = git.resolved.clone().unwrap();

    let script = |name: &str, body: String| {
        let path = format!("{root}/{name}");
        std::fs::write(&path, body).unwrap();
        make_executable(&path);
        path
    };
    let marker = format!("{root}/chosen-git-ran");
    let git_wrapper = script(
        "git-wrapper",
        format!("#!/bin/sh\necho ran >> '{marker}'\nexec '{real_git}' \"$@\"\n"),
    );
    client
        .development(Request::PinTool {
            name: "git".into(),
            pin: Some(ToolPin::Path(git_wrapper.clone())),
        })
        .unwrap();
    // Opening a project runs git through the runtime.
    client
        .development(Request::OpenProject { path: root.to_string() })
        .unwrap();
    assert!(
        std::fs::read_to_string(&marker).is_ok_and(|ran| ran.contains("ran")),
        "the runtime's git ran the chosen copy"
    );
    let chosen = probe(&["git"]);
    assert_eq!(chosen.tools[0].pin, Some(ToolPin::Path(git_wrapper.clone())));
    assert_eq!(chosen.tools[0].resolved.as_deref(), Some(git_wrapper.as_str()));

    // An agent launches with the copy chosen for its command.
    let sh_wrapper = script("sh-wrapper", "#!/bin/sh\nprintf CHOSEN_\nexec /bin/sh \"$@\"\n".into());
    client
        .development(Request::PinTool {
            name: "sh".into(),
            pin: Some(ToolPin::Path(sh_wrapper)),
        })
        .unwrap();
    let config = client
        .add_agent_config(
            "Chosen copy",
            "sh",
            vec!["-c".into()],
            PromptTransport::Argv,
            vec![],
            Default::default(),
        )
        .unwrap();
    let run = client
        .run_agent(workspace, home, &config.id, "printf AGENT_OK", AgentDispatch::default())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let output = client.agent_run(&run.id).unwrap().output;
        if output.contains("CHOSEN_AGENT_OK") {
            break;
        }
        assert!(Instant::now() < deadline, "agent output: {output}");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A copy that isn't there is refused; forgetting the choice goes back
    // to the PATH.
    assert!(client
        .development(Request::PinTool {
            name: "git".into(),
            pin: Some(ToolPin::Path(format!("{root}/no-such-git"))),
        })
        .is_err());
    for name in ["git", "sh"] {
        client
            .development(Request::PinTool { name: name.into(), pin: None })
            .unwrap();
    }
    let restored = probe(&["git"]);
    assert_eq!(restored.tools[0].pin, None);
    assert_eq!(restored.tools[0].resolved.as_deref(), Some(real_git.as_str()));
}

/// Start a worker and use its remote CLI without a desktop inbox. Reattaching
/// the SSH transport must preserve the exact launch generation and live PTY.
fn assert_remote_coordination(client: &HostClient, root: &str) {
    use dragent_host::team::{Party, TeamRequest, TeamResponse};
    use serde_json::json;
    let bin = format!("{root}/mock-agents");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = format!("{bin}/claude");
    let remote_home=std::path::Path::new(root).parent().unwrap();
    let config_path=remote_home.join(".claude.json");
    std::fs::write(&config_path,json!({"mcpServers":{"keep-test":{"command":"user-owned-server"}},"keepPreference":true}).to_string()).unwrap();
    let owner="fixture-desktop";
    let service_catalog=dragent_host::service_mcp::catalog(&["linear"]);
    let desktop=serde_json::to_value(dragent_host::team_commands::Caller::person()).unwrap();
    client.coordination("host_services_poll",json!({"owner":owner,"catalog":service_catalog,"limit":0}),desktop.clone()).unwrap();
    let stop=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let calls=std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (peer_client,peer_stop,peer_calls)=(client.clone(),stop.clone(),calls.clone());
    let peer=std::thread::spawn(move || {
        while !peer_stop.load(std::sync::atomic::Ordering::Acquire) {
            let result=peer_client.coordination("host_services_poll",json!({"owner":owner,"catalog":dragent_host::service_mcp::catalog(&["linear"]),"limit":4}),desktop.clone());
            let Ok(value)=result else {break};
            for call in value["calls"].as_array().into_iter().flatten() {
                let name=call["request"]["name"].as_str().unwrap();
                assert!(call["request"]["cwd"].as_str().unwrap().starts_with('/'));
                peer_calls.lock().unwrap().push(name.into());
                // An isolated mock desktop: no external service is contacted.
                let text=match name {"services"=>json!([{"service":"linear","connected":true}]).to_string(),"comment"=>json!({"done":"mock comment only"}).to_string(), _=>json!({"mock":true}).to_string()};
                peer_client.coordination("host_services_finish",json!({"owner":owner,"id":call["id"],"text":text,"error":false}),desktop.clone()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    let mcp_requests=[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"msg_send","arguments":{"to":"lead","text":"SSH MCP worker mailbox verified"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"task_add","arguments":{"title":"Forbidden worker task","prompt":"Must be refused","agent":"claude"}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"unknown_tool","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"services","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"comment","arguments":{"service":"linear","id":"FIXTURE-1","text":"mock only"}}}),
    ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")+"\n";
    let transcript=format!("{root}/remote-mcp.jsonl");
    let registered_server = "import json,os,subprocess,sys; config=json.load(open(os.path.join(os.environ['HOME'],'.claude.json'))); server=config['mcpServers']['dragent']; sys.exit(subprocess.run([server['command']]+server.get('args',[]),input=sys.stdin.buffer.read()).returncode)";
    let script=format!("#!/bin/sh\nprintf 'COORD_WORKER_READY\\n'\ndragent msg send lead 'SSH worker mailbox verified' --json\nprintf '%s' {} | python3 -c {} > {}\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done\n",
        dragent_host::remote::quote(&mcp_requests),dragent_host::remote::quote(registered_server),dragent_host::remote::quote(&transcript));
    std::fs::write(&executable,script).unwrap();
    make_executable(&executable);
    client
        .development(Request::PinTool {
            name: "claude".into(),
            pin: Some(ToolPin::Path(executable)),
        })
        .unwrap();
    let person = serde_json::to_value(dragent_host::team_commands::Caller::person()).unwrap();
    client
        .coordination(
            "host_preflight",
            json!({"agents":["claude"]}),
            person.clone(),
        )
        .unwrap();
    let lead_pane = uuid::Uuid::new_v4();
    let leader_executable = format!("{bin}/pinned-claude-launcher");
    std::fs::write(
        &leader_executable,
        "#!/bin/sh\nprintf 'COORD_LEAD_READY\\n'\nwhile IFS= read -r line; do :; done\n",
    )
    .unwrap();
    make_executable(&leader_executable);
    let TeamResponse::Team(team) = client
        .team(TeamRequest::Create {
            title: "SSH coordination".into(),
            project_root: root.into(),
            roster: vec![],
            cost: Default::default(),
            max_parallel_per_agent: None,
            lead: Some(dragent_host::team::Member {
                pane: Some(lead_pane.to_string()),
                terminal: None,
                agent: Some("claude".into()),
            }),
        })
        .unwrap()
    else {
        panic!("team")
    };
    let lead_workspace = client.create_workspace(root).unwrap();
    let lead_home = client
        .ensure_terminal_home(&lead_workspace, root, root)
        .unwrap();
    let lead_command = dragent_host::agent_sessions::wrap_remote_provider_command(
        lead_pane,
        "/bin/bash",
        &format!(
            "{} 'Lead startup'",
            dragent_host::remote::quote(&leader_executable)
        ),
        "claude",
    );
    let lead_terminal = client
        .create_terminal_with_shell(
            &lead_workspace,
            &lead_home.id,
            Some(&lead_command),
            Some(root),
            100,
            30,
            Some("/bin/bash"),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshots = client
            .coordination(
                "host_sessions",
                json!({"panes":[lead_pane.to_string()]}),
                person.clone(),
            )
            .unwrap();
        if snapshots[0]["record"]["active"] == true {
            assert_eq!(snapshots[0]["record"]["provider"], "claude");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Remote lead wrapper didn't start: {snapshots}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        client
            .coordination(
                "host_start_lead",
                json!({"team":team.id,"agent":"claude"}),
                person.clone()
            )
            .unwrap_err()
            .contains("still running")
    );
    client.coordination("task_add",json!({"team":team.id,"title":"Mock remote task","prompt":"Report startup only","agent":"claude"}),person.clone()).unwrap();
    let started = client
        .coordination("task_start", json!({"team":team.id}), person.clone())
        .unwrap();
    assert_eq!(
        started["started"].as_array().map(Vec::len),
        Some(1),
        "{started}"
    );
    let TeamResponse::Team(running) = client
        .team(TeamRequest::Get {
            team: team.id.clone(),
        })
        .unwrap()
    else {
        panic!("running")
    };
    let task = &running.tasks[0];
    assert_ne!(task.workspace.as_deref(), Some(root));
    let pane = task.worker.as_ref().unwrap().pane.clone().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let record = loop {
        let rows = client
            .coordination("host_sessions", json!({"panes":[pane]}), person.clone())
            .unwrap();
        if rows[0]["record"]["active"] == true {
            break rows[0]["record"].clone();
        }
        assert!(Instant::now() < deadline, "Worker didn't start: {rows}");
        std::thread::sleep(Duration::from_millis(50));
    };
    loop {
        let TeamResponse::Messages(messages) = client
            .team(TeamRequest::Inbox {
                team: team.id.clone(),
                party: Party::Lead,
                all: true,
                peek: true,
            })
            .unwrap()
        else {
            panic!("inbox")
        };
        if messages.iter().any(|m|m.body == "SSH worker mailbox verified")
            && messages.iter().any(|m|m.body == "SSH MCP worker mailbox verified" && m.from == Party::Task("t1".into())) {
            break;
        }
        let screen = client
            .coordination(
                "task_read",
                json!({"team":team.id,"task":"t1"}),
                person.clone(),
            )
            .unwrap();
        assert!(
            Instant::now() < deadline,
            "Remote helper mailbox failed: {screen}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let replies=loop {
        let Response::File {data,..} = client.development(Request::Read {root:root.into(),path:"remote-mcp.jsonl".into()}).unwrap() else {panic!("MCP transcript")};
        let rows:Vec<serde_json::Value>=String::from_utf8(data).unwrap().lines().filter_map(|line|serde_json::from_str(line).ok()).collect();
        if rows.len() == 7 {break rows;}
        assert!(Instant::now()<deadline,"MCP process did not complete: {rows:?}");
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(replies[0]["result"]["serverInfo"]["name"],"dragent");
    assert_eq!(replies[1]["result"]["tools"],json!(dragent_host::service_mcp::catalog(&["linear"]).into_iter().chain(dragent_host::mcp::coordination_tools()).collect::<Vec<_>>()));
    assert_eq!(replies[2]["result"]["isError"],false);
    assert_eq!(replies[3]["result"]["isError"],true,"Workers must not plan tasks over MCP");
    assert_eq!(replies[4]["error"]["code"],-32602);
    assert_eq!(replies[5]["result"]["isError"],false,"services: {}",replies[5]);
    assert_eq!(replies[6]["result"]["isError"],false,"comment: {}",replies[6]);
    assert_eq!(*calls.lock().unwrap(),["services","comment"]);
    let registered:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(registered["keepPreference"],true);
    assert_eq!(registered["mcpServers"]["keep-test"]["command"],"user-owned-server");
    assert!(registered["mcpServers"]["dragent"].get("env").is_none());
    let TeamResponse::Team(after_mcp)=client.team(TeamRequest::Get {team:team.id.clone()}).unwrap() else {panic!("team after MCP")};
    assert_eq!(after_mcp.tasks.len(),1,"MCP must enforce the same worker permissions as CLI");
    client.remote_connection().unwrap().disconnect();
    assert!(client.ping());
    let snapshots = client
        .coordination("host_sessions", json!({"panes":[pane]}), person.clone())
        .unwrap();
    assert_eq!(
        snapshots[0]["record"]["generation"], record["generation"],
        "Reconnect must not restart the worker"
    );
    assert_eq!(
        client
            .terminal(
                record["workspace_id"].as_str().unwrap(),
                record["terminal_id"].as_str().unwrap()
            )
            .unwrap()
            .state,
        dragent_host::protocol::TerminalState::Running
    );
    client
        .close_terminal(
            record["workspace_id"].as_str().unwrap(),
            record["terminal_id"].as_str().unwrap(),
        )
        .unwrap();
    client
        .close_terminal(&lead_workspace, &lead_terminal.id)
        .unwrap();
    // A closed PTY may retain an active recovery record so it can resume
    // after shutdown. Liveness is the host terminal state, not that flag.
    assert!(!client.terminal(&lead_workspace, &lead_terminal.id).is_ok_and(|terminal| terminal.state == dragent_host::protocol::TerminalState::Running));
    let replacement = client
        .coordination(
            "host_start_lead",
            json!({"team":team.id,"agent":"claude"}),
            person.clone(),
        )
        .unwrap();
    let replacement_pane = replacement["agent_id"].as_str().unwrap();
    assert_ne!(replacement_pane, lead_pane.to_string());
    let snapshots = client
        .coordination(
            "host_sessions",
            json!({"panes":[replacement_pane]}),
            person.clone(),
        )
        .unwrap();
    assert_eq!(snapshots[0]["record"]["active"], true);
    client
        .close_terminal(
            snapshots[0]["record"]["workspace_id"].as_str().unwrap(),
            snapshots[0]["record"]["terminal_id"].as_str().unwrap(),
        )
        .unwrap();
    client.team(TeamRequest::Stop { team: team.id }).unwrap();
    stop.store(true,std::sync::atomic::Ordering::Release);peer.join().unwrap();
    client.coordination("host_mcp_configure",json!({"enabled":false}),person.clone()).unwrap();
    let disabled:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert!(disabled["mcpServers"].get("dragent").is_none());
    assert_eq!(disabled["mcpServers"]["keep-test"]["command"],"user-owned-server");
    client.coordination("host_mcp_configure",json!({"enabled":true}),person.clone()).unwrap();
    let enabled:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert!(enabled["mcpServers"]["dragent"].is_object());
    client
        .development(Request::PinTool {
            name: "claude".into(),
            pin: None,
        })
        .unwrap();
}
