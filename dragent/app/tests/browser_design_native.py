"""macOS native browser integration check using an isolated profile and a mock agent.

Run with DRAGENT_BINARY pointing to a freshly built desktop binary.
The sandbox blocks external network, the user's Dragent data, and credential writes.
"""
import http.server
import json
import os
import re
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def until(run, timeout=60):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            result = run()
            if result:
                return result
        except (OSError, ValueError, KeyError, IndexError, AssertionError):
            pass
        time.sleep(0.25)
    raise AssertionError("Native design check timed out")


def main():
    assert sys.platform == "darwin", "This native runner uses the macOS sandbox"
    source = Path(os.environ.get("DRAGENT_BINARY", Path(__file__).resolve().parents[2] / "target/debug/dragent")).resolve()
    scratch = Path(tempfile.mkdtemp(prefix="dragent-design-native-", dir="/tmp"))
    profile, project, mockbin = [scratch / name for name in ("profile", "project", "bin")]
    for folder in (profile, project, mockbin, scratch / "tmp", scratch / "shell"):
        folder.mkdir()
    binary = scratch / "dragent"
    subprocess.run(["/bin/cp", "-c", str(source), str(binary)], check=True)
    subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", str(binary)], check=True, capture_output=True)
    prompt_file = scratch / "agent-prompt.json"
    mock = mockbin / "codex"
    mock.write_text(f"#!{sys.executable}\nimport json,sys\nfrom pathlib import Path\n"
                    f"if any('## Design feedback' in a for a in sys.argv[1:]): Path({str(prompt_file)!r}).write_text(json.dumps(sys.argv[1:]))\nprint('DESIGN MOCK AGENT OK')\n")
    mock.chmod(0o755)
    (profile / "ui_settings.json").write_text(json.dumps({
        "onboarding_completed": True, "theme_id": "github_dark",
        "agent_command_configs": {"codex": {"command": str(mock), "prompt_command": str(mock)}},
    }))
    (profile / "workspace-trust.json").write_text(json.dumps({
        "trusted_roots": [str(project.resolve())], "recent_roots": [],
    }))
    subprocess.run(["git", "init", "-q", str(project)], check=True)
    subprocess.run(["git", "-C", str(project), "-c", "user.name=Design Test", "-c",
                    "user.email=design-test@example.invalid", "-c", "commit.gpgsign=false",
                    "commit", "--allow-empty", "-qm", "Native design fixture"], check=True)
    user_home = Path.home()
    support = user_home / "Library/Application Support/dragent"
    sandbox = scratch / "test.sb"
    sandbox.write_text(f'''(version 1)
(allow default)
(deny network-outbound)
(allow network-outbound (remote ip "localhost:*"))
(allow network-outbound (subpath "{scratch.resolve()}"))
(deny file-read* file-write* (subpath "{user_home / '.dragent'}"))
(deny file-read* (literal "{support / 'credentials'}"))
(deny file-write* (subpath "{support}"))
(deny file-write* (subpath "{user_home / '.codex'}") (subpath "{user_home / '.claude'}")
    (subpath "{user_home / '.agents'}") (subpath "{user_home / '.config'}")
    (literal "{user_home / '.claude.json'}") (literal "{user_home / '.gitconfig'}"))
(deny mach-lookup (global-name "com.apple.securityd"))
''')

    class Page(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            data = b'''<!doctype html><html><head><title>Native design fixture</title></head>
<body style="margin:0;background:#f3f4f6;font:16px sans-serif">
<main data-source="src/Card.tsx:12" style="margin:140px 70px;padding:32px;background:white;width:400px">
<h1>Design check</h1><button id="save" style="padding:16px 25px;background:#2563eb;color:white;border:0">Save changes</button>
<p>Match this card's spacing.</p></main></body></html>'''
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *_):
            pass

    site = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Page)
    threading.Thread(target=site.serve_forever, daemon=True).start()
    port = free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("DRAGENT_")}
    env.update(DRAGENT_HOME=str(profile), DRAGENT_AUTOMATION_PORT=str(port), DRAGENT_REFRESH_TOKEN="design-test-offline",
               PATH=str(mockbin) + os.pathsep + env.get("PATH", ""),
               ZDOTDIR=str(scratch / "shell"), HISTFILE=str(scratch / "shell/history"), TMPDIR=str(scratch / "tmp"))
    log = (scratch / "native.log").open("wb")
    process = None

    def command(cmd, **args):
        with socket.create_connection(("127.0.0.1", port), timeout=50) as sock:
            sock.sendall((json.dumps({"cmd": cmd, **args}) + "\n").encode())
            result = json.loads(sock.makefile("rb").readline())
            assert result.get("ok"), result
            return result

    def start():
        nonlocal process
        process = subprocess.Popen(["/usr/bin/sandbox-exec", "-f", str(sandbox), str(binary)],
                                   cwd=project, env=env, stdout=log, stderr=log, start_new_session=True)
        def ping():
            if process.poll() is not None:
                raise RuntimeError("Test app exited: " + (scratch / "native.log").read_text()[-2000:])
            return command("ping")
        until(ping)

    def browsers():
        result = command("workspace_browsers")
        return result.get("browsers", result.get("state", []))

    def page_state():
        return next(iter(browsers()), None)

    def actions_at_right():
        state = page_state()
        bar, actions = state.get("toolbar_bounds"), state.get("controls_bounds")
        return bar and actions and abs((bar[0] + bar[2] - 6) - (actions[0] + actions[2])) < 0.5

    def evaluate(script):
        return command("workspace_browser_tool", tool="eval", arguments={"script": script}, pane=None)

    def select():
        evaluate("""(() => { const b=document.querySelector('#save'),r=b.getBoundingClientRect();
          b.dispatchEvent(new MouseEvent('click',{bubbles:true,composed:true,clientX:r.x+r.width/2,clientY:r.y+r.height/2})); })()""")
        return until(lambda: (s := page_state()) and s["design_notes"] and s["design_notes"][-1]["screenshot"] and s)

    def comment(value):
        evaluate("""(() => { const root=document.querySelector('[data-dragent-design]').shadowRoot;
          const t=root.querySelector('textarea'); t.value=""" + json.dumps(value) + ";t.dispatchEvent(new InputEvent('input',{bubbles:true,composed:true})); })()")

    def stop():
        if process and process.poll() is None:
            try:
                command("quit")
                process.wait(timeout=10)
            except (OSError, AssertionError, subprocess.TimeoutExpired):
                process.terminate()
        for line in subprocess.check_output(["ps", "-axo", "pid=,command="], text=True).splitlines():
            parts = line.strip().split(None, 1)
            if len(parts) == 2 and parts[1].startswith(str(binary)):
                try:
                    os.kill(int(parts[0]), signal.SIGTERM)
                except ProcessLookupError:
                    pass

    try:
        start()
        print("Isolated app started:", scratch, flush=True)
        command("workspace_open_project", path=str(project))
        until(lambda: command("workspace_git_panel")["panel"].get("cwd") == str(project.resolve()))
        command("workspace_browser_open", target="tab", url=f"http://127.0.0.1:{site.server_port}")
        until(lambda: (s := page_state()) and s["engine"] and not s["loading"] and s)
        until(actions_at_right)
        evaluate("""(() => {const original=Element.prototype.attachShadow;
          Element.prototype.attachShadow=function(options){return original.call(this,{...options,mode:'open'})};})()""")
        command("workspace_browser_action", action="design")
        until(lambda: page_state()["design_enabled"])
        first = select()
        note = first["design_notes"][0]
        assert note["preview"].startswith("data:image/jpeg;base64,")
        until(actions_at_right)
        print("Native capture and preview passed", flush=True)
        comment("Increase this button's padding to match the card.")
        # The agent picker lives in the app's toolbar, beside Send.
        command("workspace_browser_action", action="design-agents")
        until(lambda: any(t["key"] == "new:codex" for t in page_state()["design_targets"]))
        command("workspace_browser_action", action="design-target:new:codex")
        until(lambda: page_state()["design_target"] == "new:codex")
        command("workspace_browser_action", action="design-send")
        until(prompt_file.exists)
        prompt = "\n".join(json.loads(prompt_file.read_text()))
        assert "Increase this button's padding" in prompt and "#save" in prompt
        assert "src/Card.tsx:12" in prompt and "Screenshot:" in prompt
        # The agent gets a short brief; the DOM and CSS are in a file it can read.
        context = Path(re.search(r"(/\S+?\.md)\b", prompt).group(1))
        assert "**Computed CSS:**" in context.read_text() and "Computed CSS" not in prompt
        print("Mock agent received the brief and its context file", flush=True)
        until(lambda: not page_state()["design_notes"])
        until(actions_at_right)
        # Picking resumes after sending.
        saved = select()
        comment("Keep this draft through restart.")
        until(lambda: page_state()["design_notes"][0]["comment"] == "Keep this draft through restart.")
        uuid = saved["pane_uuid"]
        draft = next(profile.rglob("browser-design/" + uuid + "/draft.json"))
        assert draft.parent.stat().st_mode & 0o777 == 0o700
        until(lambda: json.loads(draft.read_text())[0]["comment"] == "Keep this draft through restart.")
        stop()
        port = free_port()
        env["DRAGENT_AUTOMATION_PORT"] = str(port)
        start()
        restored = until(lambda: (s := page_state()) and s["pane_uuid"] == uuid and s["design_notes"] and s)
        assert restored["design_notes"][0]["comment"] == "Keep this draft through restart."
        print("Draft restored after restart", flush=True)
        report = {"native_capture": True, "mock_agent_delivery": True, "restart_restores_draft": True,
                  "browser_actions_at_right": True,
                  "artifact_directory": str(scratch), "draft": str(draft)}
        (scratch / "result.json").write_text(json.dumps(report, indent=2))
        print(json.dumps(report))
    finally:
        stop()
        site.shutdown()
        log.close()


if __name__ == "__main__":
    main()
