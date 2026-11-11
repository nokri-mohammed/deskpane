"""Tests for pi/preview_scenarios.py and pi/mock_daemon.py: the fake socket
lines the local preview (T-061) feeds the real dashboard. Plain unittest,
no pygame. Run from the repo root with:
    python3 -m unittest discover -s pi/tests
"""
import json
import os
import re
import socket
import sys
import unittest

# the modules sit one folder up, next to dashboard.py
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import layout  # noqa: E402
import mock_daemon  # noqa: E402
import preview_scenarios as scenarios  # noqa: E402

FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures", "envelope_v1.json")
TIMES = (0, 10, 30, 60)
SID = re.compile(r"^[0-9a-f]{12}$")
# keys the daemon sends as null until they are known
NULLABLE = {"ctx"}
# keys the daemon leaves out when a device has no reading (the fixture sets them)
OPTIONAL = {"battery_pct", "charging"}


def kind_of(value):
    """The JSON kind of a value: numbers are one kind, bool is its own."""
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, (int, float)):
        return "number"
    if value is None:
        return "null"
    return type(value).__name__


def load_fixture():
    with open(FIXTURE) as f:
        return json.load(f)


def sessions_by_sid(envelope):
    return {s["sid"]: s for s in envelope["views"]["agents"]["sessions"]}


def devices_by_id(envelope):
    return {d["device_id"]: d for d in envelope["views"]["stats"]["devices"]}


class ScenarioNamesTest(unittest.TestCase):
    def test_the_named_scenarios_exist(self):
        for name in ("busy", "single", "needs-you", "worst", "offline", "battery", "empty"):
            self.assertIn(name, scenarios.SCENARIOS)
        self.assertEqual(scenarios.DEFAULT_SCENARIO, "busy")

    def test_normal_is_an_alias_of_busy(self):
        self.assertEqual(scenarios.resolve_scenario("normal"), "busy")
        self.assertEqual(scenarios.resolve_scenario("worst"), "worst")
        self.assertIsNone(scenarios.resolve_scenario("nope"))

    def test_an_unknown_scenario_raises(self):
        with self.assertRaises(KeyError):
            scenarios.envelope_at("nope", 0)


class ScenarioLinesTest(unittest.TestCase):
    """Every line is what the dashboard's parse_socket_line accepts."""

    def test_every_scenario_line_parses(self):
        for name in scenarios.SCENARIOS:
            for t in TIMES:
                line = scenarios.line_at(name, t)
                if line is None:
                    # only the empty scenario closes the socket
                    self.assertEqual(name, "empty")
                    continue
                with self.subTest(scenario=name, t=t):
                    self.assertTrue(line.endswith("\n"))
                    self.assertNotIn("\n", line[:-1])
                    parsed = layout.parse_socket_line(json.loads(line))
                    self.assertIsNotNone(parsed)

    def test_every_line_has_the_fixture_keys_and_types(self):
        fixture = load_fixture()
        want_device = fixture["views"]["stats"]["devices"][0]
        want_session = fixture["views"]["agents"]["sessions"][0]
        for name in scenarios.SCENARIOS:
            for t in TIMES:
                envelope = scenarios.envelope_at(name, t)
                if envelope is None:
                    continue
                self.assertEqual(envelope["v"], fixture["v"])
                rows = [(d, want_device) for d in envelope["views"]["stats"]["devices"]]
                rows += [(s, want_session) for s in envelope["views"]["agents"]["sessions"]]
                for row, want in rows:
                    for key, value in want.items():
                        if key in OPTIONAL and key not in row:
                            continue
                        with self.subTest(scenario=name, t=t, key=key):
                            self.assertIn(key, row)
                            if row[key] is None and key in NULLABLE:
                                continue
                            self.assertEqual(kind_of(row[key]), kind_of(value))

    def test_no_line_has_keys_the_fixture_does_not_allow(self):
        """The mock sends nothing the daemon cannot send yet (model and
        tokens wait for T-063 to add them to the contract and the fixture)."""
        fixture = load_fixture()
        allowed_top = set(fixture.keys())
        allowed_views = set(fixture["views"].keys())
        allowed_device = set(fixture["views"]["stats"]["devices"][0].keys())
        allowed_session = set(fixture["views"]["agents"]["sessions"][0].keys())
        for name in scenarios.SCENARIOS:
            for t in TIMES:
                envelope = scenarios.envelope_at(name, t)
                if envelope is None:
                    continue
                with self.subTest(scenario=name, t=t):
                    self.assertLessEqual(set(envelope.keys()), allowed_top)
                    self.assertLessEqual(set(envelope["views"].keys()), allowed_views)
                    for row in envelope["views"]["stats"]["devices"]:
                        self.assertLessEqual(set(row.keys()), allowed_device, row["device_id"])
                    for row in envelope["views"]["agents"]["sessions"]:
                        self.assertLessEqual(set(row.keys()), allowed_session, row["project"])

    def test_sids_are_twelve_hex_and_unique(self):
        for name in scenarios.SCENARIOS:
            envelope = scenarios.envelope_at(name, 10)
            if envelope is None:
                continue
            sids = [s["sid"] for s in envelope["views"]["agents"]["sessions"]]
            self.assertEqual(len(sids), len(set(sids)), name)
            for sid in sids:
                self.assertRegex(sid, SID)

    def test_text_stays_inside_the_contract_limits(self):
        for name in scenarios.SCENARIOS:
            for t in TIMES:
                envelope = scenarios.envelope_at(name, t)
                if envelope is None:
                    continue
                for d in envelope["views"]["stats"]["devices"]:
                    for key in ("device_id", "label", "hostname"):
                        self.assertLessEqual(len(d[key].encode()), 64)
                    # placeholder addresses only, never a real LAN
                    self.assertTrue(d["ip"].startswith("192.168.1."), d["ip"])
                for s in envelope["views"]["agents"]["sessions"]:
                    self.assertLessEqual(len(s["project"].encode()), 32)
                    self.assertLessEqual(len(s["doing"].encode()), 32)

    def test_durations_grow_with_time(self):
        first = sessions_by_sid(scenarios.envelope_at("busy", 0))
        later = sessions_by_sid(scenarios.envelope_at("busy", 10))
        common = set(first) & set(later)
        self.assertTrue(common)
        for sid in common:
            self.assertEqual(later[sid]["running_s"], first[sid]["running_s"] + 10)
        devices_0 = devices_by_id(scenarios.envelope_at("busy", 0))
        devices_10 = devices_by_id(scenarios.envelope_at("busy", 10))
        for device_id in devices_0:
            self.assertGreater(devices_10[device_id]["uptime_s"], devices_0[device_id]["uptime_s"])


class ScenarioContentTest(unittest.TestCase):
    def test_busy_has_every_state_and_a_late_needs_you(self):
        envelope = scenarios.envelope_at("busy", 0)
        self.assertEqual(len(envelope["views"]["stats"]["devices"]), 3)
        sessions = envelope["views"]["agents"]["sessions"]
        self.assertEqual(len(sessions), 6)
        self.assertEqual(len({s["device_id"] for s in sessions}), 2)
        states = {s["state"] for s in sessions}
        self.assertLessEqual({"working", "idle", "done", "error"}, states)
        self.assertNotIn("needs_you", states)
        later = {s["state"] for s in scenarios.envelope_at("busy", 25)["views"]["agents"]["sessions"]}
        self.assertIn("needs_you", later)
        # model and tokens wait for T-063 (contract and fixture first)
        self.assertFalse(any("model" in s or "tokens" in s for s in sessions))

    def test_single_is_one_session(self):
        envelope = scenarios.envelope_at("single", 0)
        self.assertEqual(len(envelope["views"]["agents"]["sessions"]), 1)

    def test_needs_you_starts_with_one_and_adds_another(self):
        def waiting(t):
            sessions = scenarios.envelope_at("needs-you", t)["views"]["agents"]["sessions"]
            return [s for s in sessions if s["state"] == "needs_you"]
        self.assertEqual(len(waiting(0)), 1)
        self.assertEqual(len(waiting(15)), 2)

    def test_worst_is_full_and_long(self):
        envelope = scenarios.envelope_at("worst", 0)
        devices = envelope["views"]["stats"]["devices"]
        sessions = envelope["views"]["agents"]["sessions"]
        self.assertEqual(len(devices), 32)
        self.assertEqual(len(sessions), 16)
        self.assertTrue(any(len(s["project"].encode()) == 32 for s in sessions))
        self.assertTrue(any(len(d["label"].encode()) == 64 for d in devices))
        self.assertTrue(any(s["quiet"] for s in sessions))
        known = set(layout.SORT_GROUP) | {"ended"}
        self.assertTrue(any(s["state"] not in known for s in sessions))
        # a session leaves (the ghost walk-off with a pack)
        later = scenarios.envelope_at("worst", 30)["views"]["agents"]["sessions"]
        self.assertEqual(len(later), 15)

    def test_offline_device_goes_offline_and_silence_grows(self):
        before = devices_by_id(scenarios.envelope_at("offline", 0))
        after = devices_by_id(scenarios.envelope_at("offline", 15))
        went = [d for d in before if before[d]["online"] and not after[d]["online"]]
        self.assertEqual(len(went), 1)
        silent_0 = [s["silent_s"] for s in scenarios.envelope_at("offline", 0)["views"]["agents"]["sessions"]]
        silent_30 = [s["silent_s"] for s in scenarios.envelope_at("offline", 30)["views"]["agents"]["sessions"]]
        self.assertGreater(max(silent_30), max(silent_0))

    def test_battery_cycles_three_levels_and_no_fields(self):
        def local_pi(t):
            devices = scenarios.envelope_at("battery", t)["views"]["stats"]["devices"]
            return [d for d in devices if d.get("local") is True][0]
        seen = []
        for t in range(4):
            pi = local_pi(t)
            seen.append((pi.get("battery_pct"), pi.get("charging")))
        self.assertEqual(seen, [(87, True), (45, False), (15, False), (None, None)])
        # the cycle repeats, and the step without a battery has no keys at all
        self.assertEqual(local_pi(4)["battery_pct"], 87)
        self.assertNotIn("battery_pct", local_pi(3))
        self.assertNotIn("charging", local_pi(3))

    def test_only_the_pi_is_local(self):
        for name in scenarios.SCENARIOS:
            envelope = scenarios.envelope_at(name, 0)
            if envelope is None:
                continue
            for d in envelope["views"]["stats"]["devices"]:
                self.assertIsInstance(d["local"], bool)
                if d["local"]:
                    self.assertEqual(d["device_id"], "pi-zero")

    def test_empty_has_nothing_then_disconnects(self):
        envelope = scenarios.envelope_at("empty", 0)
        self.assertEqual(envelope["views"]["stats"]["devices"], [])
        self.assertEqual(envelope["views"]["agents"]["sessions"], [])
        self.assertIsNone(scenarios.envelope_at("empty", 60))
        self.assertIsNone(scenarios.line_at("empty", 60))


class MockDaemonTest(unittest.TestCase):
    """The mock is a real Unix socket server that writes scenario lines."""

    def setUp(self):
        self.path = "/tmp/phd-test-%d-mock.sock" % os.getpid()
        self.clock = [100.0]
        self.mock = None

    def tearDown(self):
        if self.mock is not None:
            self.mock.stop()
        self.assertFalse(os.path.exists(self.path))

    def start(self, name):
        self.mock = mock_daemon.MockDaemon(self.path, name, interval_s=0.05, clock=lambda: self.clock[0])
        self.mock.start()

    def test_a_client_reads_scenario_lines(self):
        self.start("busy")
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(5)
        client.connect(self.path)
        reader = client.makefile()
        line = reader.readline()
        reader.close()
        client.close()
        parsed = layout.parse_socket_line(json.loads(line))
        self.assertIsNotNone(parsed)
        self.assertEqual(len(parsed[1]), 6)

    def test_a_closed_scenario_refuses_clients(self):
        self.start("empty")
        # past the disconnect second of the empty scenario
        self.clock[0] += 120
        self.assertTrue(self.mock.wait_closed(5))
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(1)
        with self.assertRaises(OSError):
            client.connect(self.path)
        client.close()


if __name__ == "__main__":
    unittest.main()
