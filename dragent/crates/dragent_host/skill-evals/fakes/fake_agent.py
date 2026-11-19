#!/usr/bin/env python3
"""Scripted fake agent that proves the eval runner without a model.

--mode good: declares the scenario's expected skill, issues one `dragent`
call per `expect` entry (with the positionals canned runs accept), then
prints every evidence phrase from `success` as its final answer. A good
run should score as passing.

--mode wrong: declares a forbidden skill, issues a forbidden write,
repeats a side-effect call without an idempotency key, and answers
without evidence. A wrong run must score as failing.
"""

import argparse
import json
import os
import subprocess
import sys

EXTRA_POSITIONALS = {
    ("team", "create"): lambda sc, ws: ["Fake team goal for " + sc["id"]],
    ("tasks", "add"): lambda sc, ws: ["Fake task", "--prompt", "Do the thing"],
    ("tasks", "show"): lambda sc, ws: ["t1"],
    ("tasks", "diff"): lambda sc, ws: ["t1"],
    ("tasks", "integrate"): lambda sc, ws: ["t1", "--check", "true"],
    ("tasks", "reopen"): lambda sc, ws: ["t1", "--feedback", "Fix it"],
    ("tasks", "wait"): lambda sc, ws: ["--timeout", "5"],
    ("tasks", "read"): lambda sc, ws: ["t1", "--lines", "5"],
    ("msg", "send"): lambda sc, ws: ["lead", "Hello from the fake agent"],
    ("requests", "show"): lambda sc, ws: ["req-canned-1"],
    ("browser", "open"): lambda sc, ws: ["localhost:3000"],
    ("browser", "click"): lambda sc, ws: ["e1"],
    ("browser", "type"): lambda sc, ws: ["e1", "hello"],
    ("computer", "snapshot"): lambda sc, ws: ["FakeApp"],
    ("computer", "click"): lambda sc, ws: ["FakeApp", "1"],
    ("integrations", "search"): lambda sc, ws: ["linear", "checkout"],
    ("integrations", "show"): lambda sc, ws: ["linear", "ENG-1"],
    ("automations", "preview"): lambda sc, ws: [
        "--schedule", "0 9 * * *", "--timezone", "UTC", "--count", "2"],
    ("automations", "create"): lambda sc, ws: [
        "--name", "Fake", "--prompt", "Do it",
        "--schedule", "0 9 * * *", "--timezone", "UTC", "--paused"],
    ("automations", "get"): lambda sc, ws: ["auto-1"],
    ("automations", "runs"): lambda sc, ws: ["auto-1", "--limit", "2"],
    ("workspaces", "create"): lambda sc, ws: ["--project", ws],
    ("feedback", None): lambda sc, ws: [
        "--draft", "--kind", "bug", "--observed", "fake observed"],
}


def find_scenario(scenarios_dir, scenario_id):
    for name in sorted(os.listdir(scenarios_dir)):
        if not name.endswith(".json"):
            continue
        with open(os.path.join(scenarios_dir, name), encoding="utf-8") as handle:
            parsed = json.load(handle)
        items = parsed if isinstance(parsed, list) else [parsed]
        for item in items:
            if item["id"] == scenario_id:
                return item
    raise SystemExit(f"fake agent: unknown scenario {scenario_id}")


def run_dragent(argv):
    proc = subprocess.run(["dragent"] + argv, capture_output=True,
                          text=True, timeout=120)
    print(f"$ dragent {' '.join(argv)} -> exit {proc.returncode}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=["good", "wrong"], required=True)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--scenarios", default=os.environ.get("EVAL_SCENARIOS", ""))
    args = parser.parse_args()
    workspace = os.environ.get("EVAL_WORKSPACE", ".")
    scenario = find_scenario(args.scenarios, args.scenario)

    if args.mode == "good":
        print(f"Using skill: {scenario['expected_skill']}")
        run_dragent(["capabilities", "--json"])
        for entry in scenario["expect"]:
            parts = entry.split()
            family = parts[0]
            sub = parts[1] if len(parts) > 1 and not parts[1].startswith("--") else None
            argv = [family] + ([sub] if sub else [])
            extra = EXTRA_POSITIONALS.get((family, sub), lambda sc, ws: [])(scenario, workspace)
            argv += extra + ["--json"]
            if family == "feedback":
                argv = [family] + extra  # --draft already; no extra --json needed
            run_dragent(argv)
        print("Final answer:")
        print("; ".join(
            p.strip() for p in scenario["success"].split(";") if p.strip()))
        return 0

    forbidden = (scenario.get("must_not_use_skills", []) or ["dragent-orchestrate"])
    print(f"Using skill: {forbidden[0]}")
    run_dragent(["team", "create", "Wrong team", "--json"])
    run_dragent(["integrations", "comment", "linear", "ENG-1",
                 "--body", "wrong", "--json"])
    run_dragent(["panes", "open", "--label", "dup", "--json"])
    run_dragent(["panes", "open", "--label", "dup", "--json"])
    print("Final answer:")
    print("Done.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
