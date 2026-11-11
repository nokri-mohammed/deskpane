"""Tests for pi/deskpane_contract.py: the Python mirror of the wire contract.
parse_metrics reads the plugin metrics view from one socket line."""

import json
import math
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import deskpane_contract as contract  # noqa: E402

FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures", "envelope_v1.json")


def envelope(metrics):
    """A minimal envelope with the given metrics view (left out when None)."""
    views = {"stats": {"devices": []}, "agents": {"sessions": []}}
    if metrics is not None:
        views["metrics"] = metrics
    return {"v": contract.ENVELOPE_VERSION, "views": views}


class ParseMetricsTest(unittest.TestCase):
    def test_the_golden_fixture_reads_as_its_metrics(self):
        with open(FIXTURE) as f:
            line = json.load(f)
        self.assertEqual(contract.parse_metrics(line), {
            "weather": {"values": {"rain_pct": 40.0, "temp_c": 12.4}, "age_s": 42, "stale": False},
        })

    def test_no_metrics_view_is_empty(self):
        self.assertEqual(contract.parse_metrics(envelope(None)), {})

    def test_legacy_array_and_garbage_are_empty(self):
        for line in ([], [{"device_id": "x"}], None, "text", 3, {"v": 2, "views": {}}, {"v": 1}):
            with self.subTest(line=line):
                self.assertEqual(contract.parse_metrics(line), {})

    def test_a_metrics_view_that_is_not_an_object_is_empty(self):
        for metrics in ([], "x", 1, None):
            with self.subTest(metrics=metrics):
                line = envelope({})
                line["views"]["metrics"] = metrics
                self.assertEqual(contract.parse_metrics(line), {})

    def test_entries_that_are_not_objects_are_skipped(self):
        line = envelope({"bad": [1, 2], "ok": {"values": {"x": 1}, "age_s": 3, "stale": False}})
        self.assertEqual(list(contract.parse_metrics(line)), ["ok"])

    def test_values_keep_only_real_numbers(self):
        line = envelope({"p": {"values": {"a": 1, "b": 2.5, "c": True, "d": "7", "e": None,
                                          "f": float("nan"), "g": float("inf")},
                               "age_s": 0, "stale": False}})
        values = contract.parse_metrics(line)["p"]["values"]
        self.assertEqual(values, {"a": 1.0, "b": 2.5})
        for value in values.values():
            self.assertIsInstance(value, float)
            self.assertTrue(math.isfinite(value))

    def test_missing_or_odd_age_and_stale_are_safe(self):
        # no trustworthy freshness means stale, so a panel shows its missing text
        line = envelope({"p": {"values": {"a": 1}}, "q": {"values": {"a": 1}, "age_s": "x", "stale": "no"},
                         "r": {"values": {"a": 1}, "age_s": -4, "stale": 0}})
        metrics = contract.parse_metrics(line)
        for name in ("p", "q", "r"):
            with self.subTest(name=name):
                self.assertIsNone(metrics[name]["age_s"])
                self.assertTrue(metrics[name]["stale"])

    def test_values_that_are_not_an_object_read_as_none(self):
        line = envelope({"p": {"values": [1, 2], "age_s": 1, "stale": False}})
        self.assertEqual(contract.parse_metrics(line)["p"]["values"], {})

    def test_plugin_and_key_names_follow_the_contract_rules(self):
        # slice 2 builds "<plugin>.<key>" field names from these, so the mirror
        # enforces the same rules as the daemon instead of trusting the line
        good = {"values": {"temp_c": 1, "Bad": 2, "2x": 3, "a-b": 4, "k" * 33: 5, "k" * 32: 6},
                "age_s": 1, "stale": False}
        line = envelope({"weather": good, "ups-2": good, "Weather": good, "1x": good,
                         "a_b": good, "p" * 33: good, "p" * 32: good})
        metrics = contract.parse_metrics(line)
        self.assertEqual(sorted(metrics), sorted(["weather", "ups-2", "p" * 32]))
        self.assertEqual(sorted(metrics["weather"]["values"]), sorted(["temp_c", "k" * 32]))

    def test_at_most_sixteen_plugins_and_sixteen_values(self):
        values = {"k%d" % i: i for i in range(20)}
        plugins = {"p%d" % i: {"values": values, "age_s": 1, "stale": False} for i in range(20)}
        metrics = contract.parse_metrics(envelope(plugins))
        self.assertEqual(len(metrics), contract.MAX_METRIC_PLUGINS)
        for entry in metrics.values():
            self.assertEqual(len(entry["values"]), contract.MAX_METRIC_VALUES)

    def test_numbers_outside_the_contract_bound_are_dropped_without_raising(self):
        line = envelope({"p": {"values": {"ok": 1e9, "neg": -1e9, "big": 1e9 + 1, "huge": 10 ** 400,
                                          "tiny": -(10 ** 400)},
                               "age_s": 1, "stale": False}})
        self.assertEqual(contract.parse_metrics(line)["p"]["values"], {"ok": 1e9, "neg": -1e9})

    def test_parse_socket_line_is_unchanged_by_metrics(self):
        line = envelope({"p": {"values": {"a": 1}, "age_s": 1, "stale": False}})
        self.assertEqual(contract.parse_socket_line(line), ([], []))


if __name__ == "__main__":
    unittest.main()
