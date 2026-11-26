#!/usr/bin/env python3
"""Unit tests for budget.py (run: python3 -m unittest budget_test)."""

import csv
import json
import os
import tempfile
import unittest

import budget

TOML = """\
[thresholds]
cold_start_ms = 300.0
input_p95_ms = 10.0
frame_max_ms = 16.0

[regression]
max_worse_pct = 15.0
"""

BASELINE = {
    "measures": {
        "cold-start:probe-startup": {"value": 14.0, "unit": "ms"},
        "input-latency:pty-echo-proxy": {"value": 0.05, "unit": "ms"},
        "input-latency:in-app-log": {"value": 5.0, "unit": "ms", "p95": 9.0},
        "frame-times:in-app-frames": {"value": 5.0, "unit": "ms", "max": 10.0},
        "frame-times:stream-load-applied": {"value": 700.0, "unit": "lines/s"},
    }
}

PASS_ROWS = [
    {"measure": "cold-start", "method": "probe-startup",
     "value": "14.0", "unit": "ms", "note": "probe"},
    {"measure": "input-latency", "method": "pty-echo-proxy",
     "value": "0.05", "unit": "ms", "note": "proxy"},
    {"measure": "frame-times", "method": "stream-load-applied",
     "value": "700", "unit": "lines/s", "note": "headless rate"},
]


def write_files(rows, baseline=None, toml_text=TOML):
    tmp = tempfile.mkdtemp(prefix="budget-test-")
    measures = os.path.join(tmp, "measures.csv")
    with open(measures, "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=["timestamp", "measure", "method",
                                          "value", "unit", "note"])
        w.writeheader()
        for i, row in enumerate(rows):
            w.writerow({"timestamp": 1700000000 + i, **row})
    budget_path = os.path.join(tmp, "budget.toml")
    with open(budget_path, "w", encoding="utf-8") as f:
        f.write(toml_text)
    baseline_path = os.path.join(tmp, "baseline.json")
    with open(baseline_path, "w", encoding="utf-8") as f:
        json.dump(baseline if baseline is not None else BASELINE, f)
    return measures, budget_path, baseline_path


class BudgetTest(unittest.TestCase):
    def test_headless_quick_mode_passes(self):
        m, b, base = write_files(PASS_ROWS)
        failures, warnings, _ = budget.check(m, b, base)
        self.assertEqual(failures, [])

    def test_cold_start_over_budget_fails(self):
        rows = [dict(PASS_ROWS[0], value="500.0")] + PASS_ROWS[1:]
        m, b, base = write_files(rows)
        failures, _, _ = budget.check(m, b, base)
        self.assertTrue(any("cold-start" in f for f in failures))

    def test_input_p95_from_note_fails(self):
        rows = PASS_ROWS[:1] + [{
            "measure": "input-latency", "method": "in-app-log",
            "value": "5.0", "unit": "ms",
            "note": "60 samples (p50; p95=20.00 ms)"}]
        m, b, base = write_files(rows)
        failures, _, _ = budget.check(m, b, base)
        self.assertTrue(any("input-latency" in f and "p95" in f
                            for f in failures))

    def test_frame_max_from_note_fails(self):
        rows = PASS_ROWS[:2] + [{
            "measure": "frame-times", "method": "in-app-frames",
            "value": "5.0", "unit": "ms",
            "note": "10 aggregates (median p50; max frame 30.00 ms)"}]
        m, b, base = write_files(rows)
        failures, _, _ = budget.check(m, b, base)
        self.assertTrue(any("frame-times" in f for f in failures))

    def test_frame_rate_rows_skip_absolute_check(self):
        # lines/s rows carry no frame time; only the regression rule applies.
        self.assertEqual(budget.frame_max(PASS_ROWS[2]), None)

    def test_regression_within_tolerance_passes(self):
        # 10% worse than the 14.0 ms baseline: inside the 15% budget.
        rows = [dict(PASS_ROWS[0], value="15.0")] + PASS_ROWS[1:]
        m, b, base = write_files(rows)
        failures, _, _ = budget.check(m, b, base)
        self.assertEqual([f for f in failures if "regression" in f], [])

    def test_regression_beyond_tolerance_fails(self):
        # 50% worse than baseline: fails even though under the absolute budget.
        rows = [dict(PASS_ROWS[0], value="21.0")] + PASS_ROWS[1:]
        m, b, base = write_files(rows)
        failures, _, _ = budget.check(m, b, base)
        self.assertTrue(any("regression" in f and "cold-start" in f
                            for f in failures))

    def test_regression_rate_direction(self):
        # Higher-is-better: a 50% drop in lines/s is a regression.
        rows = [dict(r) for r in PASS_ROWS]
        rows[2] = dict(rows[2], value="300")
        m, b, base = write_files(rows)
        failures, _, _ = budget.check(m, b, base)
        self.assertTrue(any("regression" in f and "frame-times" in f
                            for f in failures))

    def test_skipped_rows_ignored(self):
        rows = PASS_ROWS + [{
            "measure": "cold-start", "method": "skipped",
            "value": "", "unit": "ms", "note": "no binary"}]
        m, b, base = write_files(rows)
        failures, _, _ = budget.check(m, b, base)
        self.assertEqual(failures, [])

    def test_missing_baseline_warns_but_passes(self):
        m, b, _base = write_files(PASS_ROWS)
        missing = os.path.join(tempfile.mkdtemp(), "nope.json")
        failures, warnings, _ = budget.check(m, b, missing)
        self.assertEqual(failures, [])
        self.assertTrue(any("regression check skipped" in w for w in warnings))

    def test_json_measures_input(self):
        tmp = tempfile.mkdtemp(prefix="budget-test-")
        payload = {"measures": [
            {"measure": "cold-start", "method": "probe-startup",
             "value": 14.0, "unit": "ms", "note": ""},
            {"measure": "input-latency", "method": "pty-echo-proxy",
             "value": 0.05, "unit": "ms", "note": ""},
        ]}
        measures = os.path.join(tmp, "measures.json")
        with open(measures, "w", encoding="utf-8") as f:
            json.dump(payload, f)
        budget_path = os.path.join(tmp, "budget.toml")
        with open(budget_path, "w", encoding="utf-8") as f:
            f.write(TOML)
        baseline_path = os.path.join(tmp, "baseline.json")
        with open(baseline_path, "w", encoding="utf-8") as f:
            json.dump(BASELINE, f)
        failures, _, _ = budget.check(measures, budget_path, baseline_path)
        self.assertEqual(failures, [])


if __name__ == "__main__":
    unittest.main()
