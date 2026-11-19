#!/usr/bin/env python3
"""Offline-capable runner for Dragent skill-eval scenarios.

For each selected scenario this runner:

1. creates a throwaway DRAGENT_HOME and a fixture workspace (a small git
   repository),
2. puts a `dragent` shim first on PATH that records each call's argv as
   JSON lines and forwards it to a real binary with `--dragent-cli` (or
   answers from canned fixture responses with `--canned`),
3. runs a configured agent command non-interactively with the scenario
   request (template from the config file),
4. captures its final answer, then scores: skill selected, expected
   commands present, forbidden commands absent, duplicate side-effect
   calls, unsupported commands (calls the binary rejected as unknown),
   and evidence phrases from the final answer,
5. writes a JSON + Markdown report recording agent, model, build version,
   platform and per-scenario results.

Python 3 standard library only. Everything temporary lives under one
per-run directory that is removed afterwards; only the reports in
`--out` remain.
"""

import argparse
import datetime
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
SCENARIOS_DIR = os.path.join(HERE, "scenarios")
DEFAULT_CONFIG = os.path.join(HERE, "agent.json")
DEFAULT_OUT = os.path.join(HERE, "reports")

# Family -> skill that owns it. Used to resolve which skill a run acted as
# (plurality vote over non-discovery calls) and to check must_not_use_skills.
FAMILY_SKILL = {
    "panes": "dragent-panes",
    "browser": "dragent-browser",
    "computer": "dragent-computer",
    "team": "dragent-orchestrate",
    "tasks": "dragent-orchestrate",
    "msg": "dragent-orchestrate",
    "work": "dragent-orchestrate",
    "standup": "dragent-standup",
    "automations": "dragent-automate",
    "integrations": "dragent-integrations",
    "feedback": "dragent-feedback",
    "workspaces": "dragent",
    "projects": "dragent",
    "agents": "dragent",
    "open": "dragent",
    "open-url": "dragent",
    "whoami": "dragent",
    "org": "dragent",
    "machines": "dragent",
    "settings": "dragent",
    "theme": "dragent",
    "capabilities": "dragent",
    "status": "dragent",
    "requests": "dragent",
    "mcp": "dragent-plugins",
}

# Calls that never vote for a skill: discovery and recovery reads.
VOTE_EXCLUDE = {"capabilities", "status", "requests", "version", "help"}

# (family, subcommand) pairs with outside effects. Repeating one with the
# same arguments counts as a duplicate side effect, unless the retry
# carries --idempotency-key (which the key is stripped for before
# comparing, so an idempotent retry never counts as a duplicate).
WRITE_OPS = {
    ("panes", "open"),
    ("panes", "close"),
    ("team", "create"),
    ("team", "stop"),
    ("tasks", "add"),
    ("tasks", "done"),
    ("tasks", "integrate"),
    ("tasks", "reopen"),
    ("tasks", "start"),
    ("msg", "send"),
    ("integrations", "comment"),
    ("integrations", "status"),
    ("integrations", "create"),
    ("automations", "create"),
    ("automations", "run"),
    ("automations", "update"),
    ("automations", "delete"),
    ("workspaces", "create"),
    ("workspaces", "delete"),
    ("browser", "click"),
    ("browser", "type"),
}

OUTPUT_FLAGS = {"--json", "--text", "--quiet"}

SKILL_RE = re.compile(r"Using skill:\s*([A-Za-z0-9-]+)")

SHIM = """#!/usr/bin/env python3
import json, os, subprocess, sys

log_path = os.environ["DRAGENT_EVAL_LOG"]
real = os.environ.get("DRAGENT_EVAL_REAL", "")
canned = os.environ.get("DRAGENT_EVAL_CANNED", "") == "1"
version = os.environ.get("DRAGENT_EVAL_VERSION", "unknown")
known = set(os.environ.get("DRAGENT_EVAL_FAMILIES", "").split(","))

args = sys.argv[1:]
if args and args[0] == "--dragent-cli":
    args = args[1:]

record = {"argv": args}
if canned:
    family = args[0] if args else "help"
    if family not in known:
        record["exit"] = 2
        record["output"] = (
            '{"ok": false, "status": "failed", "code": "unknown_command", '
            f'"message": "unknown command \\u201c{{family}}\\u201d"}}'
        )
    elif family == "version":
        record["exit"] = 0
        record["output"] = f"dragent {version}"
    elif family == "requests" and len(args) > 2 and args[1] == "show":
        record["exit"] = 0
        record["output"] = json.dumps({
            "request_id": args[2], "state": "done", "ok": True, "canned": True,
        })
    else:
        record["exit"] = 0
        record["output"] = json.dumps({"ok": True, "canned": True, "family": family})
else:
    try:
        proc = subprocess.run(
            [real, "--dragent-cli"] + args,
            capture_output=True, text=True, timeout=120,
        )
    except Exception as error:
        record["exit"] = 3
        record["output"] = f"canned shim could not run the binary: {error}"
    else:
        record["exit"] = proc.returncode
        record["output"] = (proc.stdout or "") + (proc.stderr or "")
        record["output"] = record["output"][:4000]

with open(log_path, "a", encoding="utf-8") as log:
    log.write(json.dumps(record) + "\\n")

if not canned:
    sys.stdout.write(record["output"])
else:
    sys.stdout.write(record["output"])
sys.exit(record["exit"])
"""

FIXTURE_FILES = {
    "empty": {"README.md": "# Eval fixture\n"},
    "no-app": {"README.md": "# Eval fixture\n"},
    "service-results": {"README.md": "# Eval fixture\n"},
    "page": {
        "README.md": "# Eval fixture\n",
        "index.html": "<!doctype html><title>Fixture</title><p>fixture</p>\n",
    },
    "server": {
        "README.md": "# Eval fixture\n",
        "package.json": '{"name": "fixture", "scripts": {"start": "node server.js"}}\n',
        "server.js": 'require("http").createServer((q, s) => s.end("ok")).listen(3000);\n',
    },
    "team-context": {
        "README.md": "# Eval fixture\n",
        "TEAM.md": "# Fixture team context\n",
    },
}


def canonical_family(family):
    aliases = {
        "pane": "panes",
        "teams": "team",
        "task": "tasks",
        "message": "msg",
        "integration": "integrations",
        "automation": "automations",
        "workspace": "workspaces",
        "project": "projects",
        "agent": "agents",
        "request": "requests",
    }
    return aliases.get(family, family)


def load_scenarios(directory, select):
    scenarios = []
    for name in sorted(os.listdir(directory)):
        if not name.endswith(".json"):
            continue
        with open(os.path.join(directory, name), encoding="utf-8") as handle:
            parsed = json.load(handle)
        items = parsed if isinstance(parsed, list) else [parsed]
        for item in items:
            item["_file"] = name
            scenarios.append(item)
    if select:
        wanted = set(select)
        scenarios = [item for item in scenarios if item["id"] in wanted]
        missing = wanted - {item["id"] for item in scenarios}
        if missing:
            raise SystemExit(f"unknown scenario ids: {sorted(missing)}")
    return scenarios


def binary_info(binary):
    """Version string and known families from the real binary, if any."""
    if not binary or not os.path.exists(binary):
        return "unknown", []
    try:
        proc = subprocess.run(
            [binary, "--dragent-cli", "version"],
            capture_output=True, text=True, timeout=30,
        )
        version = proc.stdout.strip().split()[-1] if proc.stdout.strip() else "unknown"
    except Exception:
        version = "unknown"
    try:
        proc = subprocess.run(
            [binary, "--dragent-cli", "capabilities", "--json"],
            capture_output=True, text=True, timeout=30,
        )
        report = json.loads(proc.stdout)
        families = [c["name"] for c in report.get("commands", [])]
    except Exception:
        families = []
    families += ["help", "version"]
    return version, families


def make_workspace(path, fixture):
    os.makedirs(path, exist_ok=True)
    for name, content in FIXTURE_FILES.get(fixture, FIXTURE_FILES["empty"]).items():
        with open(os.path.join(path, name), "w", encoding="utf-8") as handle:
            handle.write(content)
    git = shutil.which("git")
    if git is None:
        return
    env = dict(os.environ,
               GIT_AUTHOR_NAME="eval", GIT_AUTHOR_EMAIL="eval@example.invalid",
               GIT_COMMITTER_NAME="eval", GIT_COMMITTER_EMAIL="eval@example.invalid")
    subprocess.run([git, "init", "-q"], cwd=path, env=env,
                   capture_output=True, timeout=60)
    subprocess.run([git, "add", "-A"], cwd=path, env=env,
                   capture_output=True, timeout=60)
    subprocess.run([git, "-c", "user.email=eval@example.invalid",
                    "-c", "user.name=eval", "commit", "-qm", "fixture"],
                   cwd=path, env=env, capture_output=True, timeout=60)


def normalize_call(argv):
    """(family, sub-or-None, key-tokens) for scoring."""
    words = list(argv)
    if words and words[0] == "--dragent-cli":
        words = words[1:]
    if not words:
        return None, None, []
    family = canonical_family(words[0])
    rest = words[1:]
    sub = None
    if rest and not rest[0].startswith("--"):
        sub = rest[0].replace("_", "-")
        rest = rest[1:]
    return family, sub, rest


def entry_matches(entry, family, sub):
    parts = entry.split()
    if not parts:
        return False
    if canonical_family(parts[0]) != family:
        return False
    if len(parts) == 1:
        return True
    if parts[1].startswith("--"):
        return True
    return sub == parts[1].replace("_", "-")


def is_write(family, sub, argv):
    if (family, sub) in WRITE_OPS:
        return True
    if family == "feedback" and "--draft" not in argv:
        return True
    return False


def duplicate_key(argv):
    """Identity of a side-effect call: args minus output switches and any
    idempotency key, so an idempotent retry never counts as a duplicate."""
    tokens = []
    skip_next = False
    for token in argv:
        if skip_next:
            skip_next = False
            continue
        if token in OUTPUT_FLAGS:
            continue
        if token == "--idempotency-key" or token.startswith("--idempotency-key="):
            if token == "--idempotency-key":
                skip_next = True
            continue
        tokens.append(token)
    return tuple(tokens)


def score_scenario(scenario, calls, answer):
    pairs = [(call["family"], call["subcommand"]) for call in calls]
    expect_results = []
    for entry in scenario["expect"]:
        found = any(entry_matches(entry, family, sub)
                    for family, sub in pairs)
        expect_results.append({"command": entry, "found": found})
    forbid_results = []
    for entry in scenario["forbid"]:
        found = any(entry_matches(entry, family, sub)
                    for family, sub in pairs)
        forbid_results.append({"command": entry, "found": found})

    skill_of = {}
    for family in FAMILY_SKILL:
        skill_of.setdefault(FAMILY_SKILL[family], []).append(family)
    forbidden_hits = []
    for skill in scenario.get("must_not_use_skills", []):
        hit = [f"{family} {sub or ''}".strip()
               for family, sub in pairs
               if FAMILY_SKILL.get(family) == skill]
        if hit:
            forbidden_hits.append({"skill": skill, "calls": hit})

    votes = {}
    for family, _ in pairs:
        if family in VOTE_EXCLUDE:
            continue
        skill = FAMILY_SKILL.get(family)
        if skill:
            votes[skill] = votes.get(skill, 0) + 1
    voted = max(votes, key=votes.get) if votes else None
    declared = SKILL_RE.search(answer)
    declared = declared.group(1) if declared else None
    if declared:
        skill_ok = declared == scenario["expected_skill"]
    else:
        skill_ok = voted == scenario["expected_skill"]

    seen = {}
    duplicates = []
    for argv in [call["argv"] for call in calls]:
        family, sub, _ = normalize_call(argv)
        if family is None or not is_write(family, sub, argv):
            continue
        key = duplicate_key(argv)
        if key in seen:
            duplicates.append({"command": " ".join(argv)})
        else:
            seen[key] = True

    unsupported = []
    for call in calls:
        output = call.get("output", "")
        if call.get("exit", 0) != 0 and (
                "unknown_command" in output or "unknown command" in output):
            unsupported.append({"command": " ".join(call["argv"])})

    phrases = [p.strip() for p in scenario.get("success", "").split(";") if p.strip()]
    lowered = answer.lower()
    matched = [p for p in phrases if p.lower() in lowered]
    needed = max(1, (len(phrases) + 1) // 2)
    success_ok = len(matched) >= needed

    passed = (
        skill_ok
        and all(item["found"] for item in expect_results)
        and not any(item["found"] for item in forbid_results)
        and not forbidden_hits
        and not duplicates
        and not unsupported
        and success_ok
    )
    return {
        "skill_ok": skill_ok,
        "declared_skill": declared,
        "voted_skill": voted,
        "expect": expect_results,
        "forbid": forbid_results,
        "forbidden_skill_hits": forbidden_hits,
        "duplicates": duplicates,
        "unsupported": unsupported,
        "success_ok": success_ok,
        "evidence_matched": len(matched),
        "evidence_needed": needed,
        "passed": passed,
    }


def run_one(scenario, config, args, version, families, run_root):
    base = tempfile.mkdtemp(prefix="dragent-eval-", dir=run_root)
    home = os.path.join(base, "home")
    workspace = os.path.join(base, "workspace")
    bindir = os.path.join(base, "bin")
    os.makedirs(home)
    os.makedirs(bindir)
    make_workspace(workspace, scenario.get("fixture", "empty"))

    shim_path = os.path.join(bindir, "dragent")
    with open(shim_path, "w", encoding="utf-8") as handle:
        handle.write(SHIM)
    os.chmod(shim_path, 0o755)
    log_path = os.path.join(base, "calls.jsonl")

    template = config["agents"].get(args.agent) or config["agents"][config["default"]]
    fields = {
        "request": scenario["request"],
        "scenario": scenario["id"],
        "workspace": workspace,
        "home": home,
        "fakes": os.path.join(HERE, "fakes"),
    }
    command = shlex.split(template.format(
        **{key: shlex.quote(str(value)) for key, value in fields.items()}))

    env = dict(os.environ)
    env["PATH"] = bindir + os.pathsep + env.get("PATH", "")
    env["DRAGENT_HOME"] = home
    env["DRAGENT_EVAL_LOG"] = log_path
    env["DRAGENT_EVAL_REAL"] = args.binary or ""
    env["DRAGENT_EVAL_CANNED"] = "1" if args.canned else "0"
    env["DRAGENT_EVAL_VERSION"] = version
    env["DRAGENT_EVAL_FAMILIES"] = ",".join(families)
    env["EVAL_WORKSPACE"] = workspace
    env["EVAL_SCENARIO"] = scenario["id"]
    env["EVAL_SCENARIOS"] = SCENARIOS_DIR

    try:
        proc = subprocess.run(command, cwd=workspace, env=env,
                              capture_output=True, text=True,
                              timeout=args.timeout)
        answer = (proc.stdout or "") + (proc.stderr or "")
        timed_out = False
    except subprocess.TimeoutExpired as expired:
        answer = ((expired.stdout or b"").decode("utf-8", "replace")
                  + (expired.stderr or b"").decode("utf-8", "replace"))
        timed_out = True

    calls = []
    if os.path.exists(log_path):
        with open(log_path, encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                try:
                    record = json.loads(line)
                except json.JSONDecodeError:
                    continue
                family, sub, _ = normalize_call(record.get("argv", []))
                if family is None:
                    continue
                record["family"] = family
                record["subcommand"] = sub
                calls.append(record)

    scored_calls = [(call["family"], call["subcommand"]) for call in calls]
    result = score_scenario(scenario, calls, answer)
    result.update({
        "id": scenario["id"],
        "file": scenario["_file"],
        "expected_skill": scenario["expected_skill"],
        "timed_out": timed_out,
        "calls": [{"argv": call["argv"], "exit": call.get("exit", 0)}
                  for call in calls],
        "answer": answer[:4000],
    })
    if not args.keep:
        shutil.rmtree(base, ignore_errors=True)
    return result


def write_reports(results, meta, outdir):
    os.makedirs(outdir, exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    json_path = os.path.join(outdir, f"eval-{stamp}.json")
    md_path = os.path.join(outdir, f"eval-{stamp}.md")
    payload = dict(meta)
    payload["scenarios"] = results
    with open(json_path, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2)
        handle.write("\n")
    total = len(results)
    passed = sum(1 for item in results if item["passed"])
    lines = [
        "# Skill-eval report",
        "",
        f"Agent: `{meta['agent']}` · model: `{meta['model']}` · "
        f"build: `{meta['build']}` · platform: `{meta['platform']}`",
        f"Scenarios: {passed}/{total} passed · "
        f"canned: `{meta['canned']}` · {meta['created']}",
        "",
        "| Scenario | Skill | Result | Skill | Expect | Forbid | Duplicates "
        "| Unsupported | Evidence |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for item in results:
        mark = "pass" if item["passed"] else "FAIL"
        missing = [e["command"] for e in item["expect"] if not e["found"]]
        hit = [f["command"] for f in item["forbid"] if f["found"]]
        lines.append(
            f"| {item['id']} | {item['expected_skill']} | {mark} | "
            f"{'ok' if item['skill_ok'] else 'WRONG'} "
            f"({item['declared_skill'] or item['voted_skill'] or 'none'}) | "
            f"{'ok' if not missing else 'missing ' + ', '.join(missing)} | "
            f"{'ok' if not hit and not item['forbidden_skill_hits'] else 'HIT'} | "
            f"{len(item['duplicates'])} | {len(item['unsupported'])} | "
            f"{item['evidence_matched']}/{item['evidence_needed']} |")
    lines.append("")
    for item in results:
        lines += [
            f"## {item['id']} ({'pass' if item['passed'] else 'FAIL'})",
            "",
            f"Skill: declared `{item['declared_skill']}`, voted "
            f"`{item['voted_skill']}`, expected `{item['expected_skill']}`.",
            "",
            "Calls:",
            "",
        ]
        if item["calls"]:
            for call in item["calls"]:
                lines.append(f"- `dragent {' '.join(call['argv'])}` "
                             f"(exit {call['exit']})")
        else:
            lines.append("- (no dragent calls recorded)")
        lines.append("")
        if not item["passed"]:
            lines.append("Failures:")
            lines.append("")
            if not item["skill_ok"]:
                lines.append(f"- wrong skill (expected `{item['expected_skill']}`)")
            for entry in item["expect"]:
                if not entry["found"]:
                    lines.append(f"- missing expected command `{entry['command']}`")
            for entry in item["forbid"]:
                if entry["found"]:
                    lines.append(f"- forbidden command ran `{entry['command']}`")
            for hit in item["forbidden_skill_hits"]:
                lines.append(f"- forbidden skill `{hit['skill']}` used: "
                             + ", ".join(hit["calls"]))
            for dup in item["duplicates"]:
                lines.append(f"- duplicate side effect `{dup['command']}`")
            for unsupported in item["unsupported"]:
                lines.append(f"- unsupported command `{unsupported['command']}`")
            if not item["success_ok"]:
                lines.append(f"- final answer lacks evidence "
                             f"({item['evidence_matched']}/{item['evidence_needed']})")
            if item["timed_out"]:
                lines.append("- agent timed out")
            lines.append("")
    with open(md_path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines))
    return json_path, md_path


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenarios", default=SCENARIOS_DIR)
    parser.add_argument("--select", nargs="*", default=[],
                        help="Scenario ids to run (default: all).")
    parser.add_argument("--agent", default=None,
                        help="Agent key from the config file (default: its default).")
    parser.add_argument("--config", default=DEFAULT_CONFIG)
    parser.add_argument("--canned", action="store_true",
                        help="Answer dragent calls from canned fixture responses.")
    parser.add_argument("--binary", default=os.environ.get("DRAGENT_BINARY", ""),
                        help="Real dragent binary the shim forwards to.")
    parser.add_argument("--out", default=DEFAULT_OUT)
    parser.add_argument("--keep", action="store_true",
                        help="Keep per-scenario temp dirs for debugging.")
    parser.add_argument("--timeout", type=int, default=300,
                        help="Seconds per agent run.")
    parser.add_argument("--run-root", default=None,
                        help="Where per-scenario temp dirs go (default: system temp).")
    args = parser.parse_args(argv)
    with open(args.config, encoding="utf-8") as handle:
        config = json.load(handle)
    if not args.agent:
        args.agent = config.get("default", "fake")
    if not args.binary:
        # Canned runs still need it: it says which commands this build has.
        found = shutil.which("dragent")
        if not found:
            raise SystemExit("no --binary given and no dragent on PATH: pass the build to evaluate")
        args.binary = found
    version, families = binary_info(args.binary)
    scenarios = load_scenarios(args.scenarios, args.select)
    if not scenarios:
        raise SystemExit("no scenarios selected")
    results = []
    for scenario in scenarios:
        results.append(run_one(scenario, config, args, version, families,
                               args.run_root))
    meta = {
        "agent": args.agent,
        "model": config.get("model", args.agent),
        "build": version,
        "platform": f"{platform.system()} {platform.machine()}",
        "created": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "canned": args.canned,
    }
    json_path, md_path = write_reports(results, meta, args.out)
    passed = sum(1 for item in results if item["passed"])
    print(f"{passed}/{len(results)} passed")
    print(f"JSON: {json_path}")
    print(f"Markdown: {md_path}")
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())