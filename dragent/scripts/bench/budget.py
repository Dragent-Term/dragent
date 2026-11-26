#!/usr/bin/env python3
"""Performance budget check for the benchmark harness.

Reads the harness's measurement CSV/JSON (`run.py` / `gui_app.py` rows in
`dragent/docs/perf/data/measures.csv`: timestamp,measure,method,value,unit,note)
and exits 1 when any budget fails:

- absolute limits from `budget.toml` (or JSON): launch -> usable window
  (`cold_start_ms`), keystroke -> presented frame p95 (`input_p95_ms`), any
  single frame while streaming (`frame_max_ms`, parsed from the frame-times
  row's note maxima);
- regression: the latest numeric value per (measure, method) worse than the
  stored baseline (`budget_baseline.json`) by more than `max_worse_pct`
  percent (lower-is-better for ms; higher-is-better for rates).

Stdlib only: macOS, Windows and Linux with no installs. Never touches the
installed app; it only reads the CSV/JSON files it is given.

Usage:
    python3 budget.py [--measures PATH] [--budget PATH] [--baseline PATH]
    python3 -m unittest budget_test   (in this directory)
"""

import argparse
import csv
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_MEASURES = os.path.normpath(
    os.path.join(HERE, "..", "..", "docs", "perf", "data", "measures.csv"))
DEFAULT_BUDGET = os.path.join(HERE, "budget.toml")
DEFAULT_BASELINE = os.path.join(HERE, "budget_baseline.json")

DEFAULTS = {
    "cold_start_ms": 300.0,
    "input_p95_ms": 10.0,
    "frame_max_ms": 16.0,
    "max_worse_pct": 15.0,
}

RATE_UNITS = ("lines/s", "mb/s", "gb/s")


def _num(text):
    try:
        if text is None:
            return None
        s = str(text).strip()
        if s == "":
            return None
        return float(s)
    except (TypeError, ValueError):
        return None


def load_thresholds(path):
    """Threshold dict from a TOML ([thresholds]/[regression]) or JSON file."""
    cfg = dict(DEFAULTS)
    if not path or not os.path.exists(path):
        return cfg, ["budget file %s missing; using defaults" % path]
    warnings = []
    data = {}
    if path.endswith(".json"):
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    else:
        data = _load_toml(path)
    for section in ("thresholds", "regression", ""):
        node = data.get(section, data if section == "" else None)
        if not isinstance(node, dict):
            continue
        for key in ("cold_start_ms", "input_p95_ms", "frame_max_ms"):
            if key in node:
                cfg[key] = float(node[key])
        if "max_worse_pct" in node:
            cfg["max_worse_pct"] = float(node["max_worse_pct"])
    return cfg, warnings


def _load_toml(path):
    """Minimal TOML for our shape ([section] + key = number), via tomllib
    when available (3.11+) else a small line parser. Never executes."""
    try:
        import tomllib
        with open(path, "rb") as f:
            loaded = tomllib.load(f)
            return loaded if isinstance(loaded, dict) else {}
    except Exception:
        pass
    data = {}
    section = ""
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.split("#", 1)[0].strip()
                if not line:
                    continue
                m = re.match(r"\[(.+)\]", line)
                if m:
                    section = m.group(1).strip()
                    data.setdefault(section, {})
                    continue
                k, sep, v = line.partition("=")
                if not sep:
                    continue
                try:
                    data.setdefault(section, {})[k.strip()] = float(v.strip())
                except ValueError:
                    continue
    except OSError:
        pass
    return data


def load_measures(path):
    """Rows as dicts {measure, method, value|None, unit, note} from CSV/JSON."""
    if path.endswith(".json"):
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
        items = data.get("measures", data) if isinstance(data, dict) else data
        if isinstance(items, dict):
            items = [{"measure": k.split(":")[0],
                      "method": ":".join(k.split(":")[1:]),
                      "value": (v.get("value") if isinstance(v, dict) else v),
                      "unit": (v.get("unit", "") if isinstance(v, dict) else ""),
                      "note": (v.get("note", "") if isinstance(v, dict) else "")}
                     for k, v in items.items()]
        rows = []
        for item in items:
            rows.append({"measure": str(item.get("measure", "")),
                         "method": str(item.get("method", "")),
                         "value": _num(item.get("value")),
                         "unit": str(item.get("unit", "")),
                         "note": str(item.get("note", ""))})
        return rows
    rows = []
    with open(path, newline="", encoding="utf-8") as f:
        for item in csv.DictReader(f):
            rows.append({"measure": (item.get("measure") or "").strip(),
                         "method": (item.get("method") or "").strip(),
                         "value": _num(item.get("value")),
                         "unit": (item.get("unit") or "").strip(),
                         "note": item.get("note") or ""})
    return rows


def load_baseline(path):
    """Baseline map 'measure:method' -> {value, unit, p95?, max?}."""
    if not path or not os.path.exists(path):
        return {}, False
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    node = data.get("measures", data) if isinstance(data, dict) else {}
    out = {}
    if isinstance(node, dict):
        for key, entry in node.items():
            if isinstance(entry, dict):
                got = {"value": _num(entry.get("value")),
                       "unit": str(entry.get("unit", ""))}
                if _num(entry.get("p95")) is not None:
                    got["p95"] = _num(entry.get("p95"))
                if _num(entry.get("max")) is not None:
                    got["max"] = _num(entry.get("max"))
                out[key] = got
            else:
                out[key] = {"value": _num(entry), "unit": ""}
    return out, True


def parse_input_p95(row):
    """p95 for an input-latency row: the note's `p95=X ms`, else the value."""
    m = re.search(r"p95\s*=\s*([\d.]+)\s*ms", row.get("note", ""))
    if m:
        return float(m.group(1))
    return row.get("value")


def parse_frame_maxima(note):
    """All `max … X ms` numbers in a frame-times note (interval + render)."""
    return [float(m.group(1))
            for m in re.finditer(r"max[^0-9]{0,12}([\d.]+)\s*ms", note)]


def frame_max(row):
    """Effective maximum frame for a frame-times row, or None when the row
    carries no frame time (headless rates like lines/s are skipped)."""
    maxima = parse_frame_maxima(row.get("note", ""))
    if maxima:
        return max(maxima)
    if (row.get("unit") or "") == "ms":
        return row.get("value")
    return None


def check_absolute(rows, cfg):
    """Absolute budget failures (one line each). Skips skipped/proxy-rate rows."""
    failures = []
    for row in rows:
        if row.get("value") is None:
            continue
        measure, method, value = row["measure"], row["method"], row["value"]
        if measure == "cold-start" and row.get("unit") == "ms":
            if value > cfg["cold_start_ms"]:
                failures.append(
                    "cold-start %s %.1f ms > budget %.1f ms"
                    % (method, value, cfg["cold_start_ms"]))
        elif measure == "input-latency":
            p95 = parse_input_p95(row)
            if p95 is not None and p95 > cfg["input_p95_ms"]:
                failures.append(
                    "input-latency %s p95 %.2f ms > budget %.1f ms"
                    % (method, p95, cfg["input_p95_ms"]))
        elif measure == "frame-times":
            mx = frame_max(row)
            if mx is not None and mx > cfg["frame_max_ms"]:
                failures.append(
                    "frame-times %s max %.2f ms > budget %.1f ms"
                    % (method, mx, cfg["frame_max_ms"]))
    return failures


def _worse(current, expected, unit, pct):
    if expected is None or expected == 0:
        return False
    if unit.lower() in RATE_UNITS:
        return current < expected * (1.0 - pct / 100.0)
    return current > expected * (1.0 + pct / 100.0)


def _pct_worse(current, expected, unit):
    if not expected:
        return 0.0
    if unit.lower() in RATE_UNITS:
        return (expected - current) / expected * 100.0
    return (current - expected) / expected * 100.0


def latest_by_key(rows):
    """Latest numeric row per 'measure:method' (measures.csv appends)."""
    latest = {}
    for row in rows:
        if row.get("value") is None:
            continue
        latest["%s:%s" % (row["measure"], row["method"])] = row
    return latest


def check_regression(rows, baseline, pct):
    """Failures where the latest row is > pct worse than the baseline file."""
    failures = []
    latest = latest_by_key(rows)
    for key, entry in sorted(baseline.items()):
        row = latest.get(key)
        if row is None:
            continue
        expected, unit = entry.get("value"), entry.get("unit", "")
        if expected is None:
            continue
        if _worse(row["value"], expected, unit, pct):
            failures.append(
                "regression %s %.3g %s vs baseline %.3g %s (%+.1f%%, budget %.1f%%)"
                % (key, row["value"], unit or row.get("unit", ""),
                   expected, unit,
                   _pct_worse(row["value"], expected, unit), pct))
        # p95 / max sub-checks when both sides carry them.
        if row["measure"] == "input-latency" and entry.get("p95") is not None:
            p95 = parse_input_p95(row)
            if p95 is not None and _worse(p95, entry["p95"], "ms", pct):
                failures.append(
                    "regression %s p95 %.2f ms vs baseline %.2f ms (%+.1f%%)"
                    % (key, p95, entry["p95"],
                       _pct_worse(p95, entry["p95"], "ms")))
        if row["measure"] == "frame-times" and entry.get("max") is not None:
            mx = frame_max(row)
            if mx is not None and _worse(mx, entry["max"], "ms", pct):
                failures.append(
                    "regression %s max %.2f ms vs baseline %.2f ms (%+.1f%%)"
                    % (key, mx, entry["max"],
                       _pct_worse(mx, entry["max"], "ms")))
    return failures


def check(measures_path, budget_path=DEFAULT_BUDGET,
          baseline_path=DEFAULT_BASELINE):
    """Run all checks; returns (failures, warnings, context)."""
    cfg, warnings = load_thresholds(budget_path)
    rows = load_measures(measures_path)
    baseline, have_baseline = load_baseline(baseline_path)
    if not have_baseline:
        warnings.append("baseline %s missing; regression check skipped"
                        % baseline_path)
    failures = check_absolute(rows, cfg)
    if have_baseline:
        failures += check_regression(rows, baseline, cfg["max_worse_pct"])
    context = {"rows": len(rows), "cfg": cfg,
               "baseline_entries": len(baseline)}
    return failures, warnings, context


def main(argv=None):
    ap = argparse.ArgumentParser(description="Performance budget check.")
    ap.add_argument("--measures", default=DEFAULT_MEASURES,
                    help="measures.csv or JSON from the harness")
    ap.add_argument("--budget", default=DEFAULT_BUDGET,
                    help="budget.toml (or JSON) thresholds")
    ap.add_argument("--baseline", default=DEFAULT_BASELINE,
                    help="stored baseline JSON for the regression rule")
    args = ap.parse_args(argv)
    failures, warnings, context = check(args.measures, args.budget,
                                        args.baseline)
    print("[budget] %d rows vs %s (baseline: %d entries)"
          % (context["rows"], args.measures, context["baseline_entries"]))
    print("[budget] thresholds: cold-start %.0f ms, input p95 %.0f ms, "
          "frame max %.0f ms, regression %.0f%%"
          % (context["cfg"]["cold_start_ms"], context["cfg"]["input_p95_ms"],
             context["cfg"]["frame_max_ms"], context["cfg"]["max_worse_pct"]))
    for warning in warnings:
        print("[budget] warn: %s" % warning)
    if failures:
        for failure in failures:
            print("[budget] FAIL %s" % failure)
        return 1
    print("[budget] PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
