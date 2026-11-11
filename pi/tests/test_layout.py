"""Tests for pi/layout.py: the version file and the overview page maths.
Plain unittest, no pygame needed. Run from the repo root with:
    python3 -m unittest discover -s pi/tests
"""
import json
import os
import sys
import tempfile
import unittest

# layout.py sits one folder up, next to dashboard.py
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import layout  # noqa: E402

# rendered heights of the real fonts: clock Font(None, 120), date 20, summary 16
CLOCK_H = 82
DATE_H = 13
SUMMARY_H = 11
WIDTH = 480
HEIGHT = 320


class ReadVersionTest(unittest.TestCase):
    """read_version looks for VERSION one level above the dashboard folder,
    the way install.sh lays out /opt/deskpane/releases/<v>/."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.release_dir = self._tmp.name
        self.dashboard_dir = os.path.join(self.release_dir, "dashboard")
        os.mkdir(self.dashboard_dir)

    def tearDown(self):
        self._tmp.cleanup()

    def write_version(self, text):
        with open(os.path.join(self.release_dir, "VERSION"), "w") as f:
            f.write(text)

    def test_read_version_from_file_one_level_up(self):
        self.write_version("0.1.0-beta.3")
        self.assertEqual(layout.read_version(self.dashboard_dir), "0.1.0-beta.3")

    def test_read_version_strips_newline(self):
        self.write_version("  0.1.0\n")
        self.assertEqual(layout.read_version(self.dashboard_dir), "0.1.0")

    def test_read_version_missing_file_is_dev(self):
        self.assertEqual(layout.read_version(self.dashboard_dir), "dev")

    def test_read_version_missing_folder_is_dev(self):
        missing = os.path.join(self.release_dir, "nope", "dashboard")
        self.assertEqual(layout.read_version(missing), "dev")

    def test_read_version_malformed_is_dev(self):
        for text in ["", "latest", "1.2", "v0.1.0", "0.1.0 beta", "0.1.0-\x1b[2J", "0.1.0\n0.2.0"]:
            with self.subTest(text=text):
                self.write_version(text)
                self.assertEqual(layout.read_version(self.dashboard_dir), "dev")

    def test_read_version_too_long_is_dev(self):
        # 33 characters, one past the cap; valid by the pattern otherwise
        self.write_version("0.1.0-" + "a" * 27)
        self.assertEqual(layout.read_version(self.dashboard_dir), "dev")

    def test_read_version_at_the_cap_is_kept(self):
        version = "0.1.0-" + "a" * 26
        self.assertEqual(len(version), 32)
        self.write_version(version)
        self.assertEqual(layout.read_version(self.dashboard_dir), version)

    def test_read_version_binary_junk_is_dev(self):
        with open(os.path.join(self.release_dir, "VERSION"), "wb") as f:
            f.write(b"\xff\xfe\x00\x01")
        self.assertEqual(layout.read_version(self.dashboard_dir), "dev")


class VersionTextTest(unittest.TestCase):
    def test_release_gets_a_v(self):
        self.assertEqual(layout.version_text("0.1.0-beta.3"), "v0.1.0-beta.3")

    def test_dev_stays_dev(self):
        self.assertEqual(layout.version_text("dev"), "dev")


class OverviewLayoutTest(unittest.TestCase):
    def layout_480x320(self):
        return layout.overview_layout(WIDTH, HEIGHT, CLOCK_H, DATE_H, SUMMARY_H)

    def test_overview_block_is_vertically_centred(self):
        result = self.layout_480x320()
        top = result["clock_y"]
        bottom = result["summary_y"] + SUMMARY_H
        space_above = top
        space_below = HEIGHT - bottom
        self.assertLessEqual(abs(space_above - space_below), 1)

    def test_overview_items_do_not_overlap(self):
        result = self.layout_480x320()
        self.assertLessEqual(result["clock_y"] + CLOCK_H, result["date_y"])
        self.assertLessEqual(result["date_y"] + DATE_H, result["summary_y"])

    def test_overview_items_are_in_order_top_to_bottom(self):
        result = self.layout_480x320()
        self.assertLess(result["clock_y"], result["date_y"])
        self.assertLess(result["date_y"], result["summary_y"])

    def test_overview_layout_has_no_divider(self):
        result = self.layout_480x320()
        self.assertNotIn("divider_y", result)

    def test_overview_fits_on_480x320_with_120px_clock(self):
        result = self.layout_480x320()
        self.assertGreaterEqual(result["clock_y"], 0)
        # keep clear of the page dots (y = height - 10) and the version text
        version_x, version_y = layout.version_position(HEIGHT, SUMMARY_H)
        self.assertLessEqual(result["summary_y"] + SUMMARY_H, version_y)
        self.assertLessEqual(result["summary_y"] + SUMMARY_H, HEIGHT - 20)

    def test_overview_layout_follows_the_font_heights(self):
        # a smaller clock moves the block, instead of fixed offsets for one font
        big = layout.overview_layout(WIDTH, HEIGHT, 82, DATE_H, SUMMARY_H)
        small = layout.overview_layout(WIDTH, HEIGHT, 57, DATE_H, SUMMARY_H)
        self.assertGreater(small["clock_y"], big["clock_y"])
        self.assertLess(small["date_y"], big["date_y"])

    def test_overview_on_a_tiny_screen_never_goes_negative(self):
        result = layout.overview_layout(100, 60, CLOCK_H, DATE_H, SUMMARY_H)
        self.assertGreaterEqual(result["clock_y"], 0)


class VersionPositionTest(unittest.TestCase):
    def test_version_sits_8px_from_the_bottom_left(self):
        x, y = layout.version_position(HEIGHT, SUMMARY_H)
        self.assertEqual(x, 8)
        self.assertEqual(y + SUMMARY_H, HEIGHT - 8)



def agent(sid, state="working", state_s=10, project="proj", quiet=False, **extra):
    """One session as the daemon's envelope sends it."""
    session = {
        "sid": sid, "device_id": "dp-3f9a0c41", "label": "my-pc", "project": project,
        "state": state, "doing": "Bash", "running_s": 600, "state_s": state_s,
        "quiet": quiet, "ctx": None,
    }
    session.update(extra)
    return session


class SortAgentsTest(unittest.TestCase):
    def sids(self, sessions):
        return [s["sid"] for s in layout.sort_agents(sessions)]

    def test_sort_puts_needs_you_first_longest_waiting_first(self):
        sessions = [
            agent("w1", "working"),
            agent("n-short", "needs_you", state_s=5),
            agent("n-long", "needs_you", state_s=90),
        ]
        self.assertEqual(self.sids(sessions), ["n-long", "n-short", "w1"])

    def test_sort_puts_error_after_needs_you_before_working(self):
        sessions = [
            agent("w", "working"),
            agent("e-old", "error", state_s=300),
            agent("e-new", "error", state_s=3),
            agent("n", "needs_you"),
        ]
        self.assertEqual(self.sids(sessions), ["n", "e-new", "e-old", "w"])

    def test_sort_puts_dim_done_last(self):
        sessions = [
            agent("d-dim", "done", state_s=layout.DONE_DIM_AFTER_S + 1),
            agent("d-new", "done", state_s=10),
            agent("d-older", "done", state_s=100),
            agent("i", "idle"),
            agent("w", "working"),
        ]
        self.assertEqual(self.sids(sessions), ["w", "i", "d-new", "d-older", "d-dim"])

    def test_sort_is_stable_by_project_within_a_group(self):
        sessions = [agent("b", "working", project="zeta"), agent("a", "working", project="alpha")]
        self.assertEqual(self.sids(sessions), ["a", "b"])

    def test_sort_puts_unknown_states_at_the_end(self):
        sessions = [agent("x", "dancing"), agent("d", "done")]
        self.assertEqual(self.sids(sessions), ["d", "x"])

    def test_sort_survives_missing_fields(self):
        self.assertEqual(len(layout.sort_agents([{"sid": "a"}, {"state": "done", "state_s": None}])), 2)


class AgentStyleTest(unittest.TestCase):
    def test_needs_you_is_amber_filled_with_alert_icon(self):
        style = layout.agent_style("needs_you", False, 3)
        self.assertEqual((style["color"], style["icon"], style["word"]), ("amber", "alert", "NEEDS YOU"))
        self.assertTrue(style["fill"])

    def test_working_is_blue_pulse(self):
        style = layout.agent_style("working", False, 3)
        self.assertEqual((style["color"], style["icon"], style["word"]), ("blue", "pulse", "WORKING"))
        self.assertFalse(style["fill"])

    def test_agent_style_quiet_working_is_grey_with_question_mark(self):
        style = layout.agent_style("working", True, 900)
        self.assertEqual((style["color"], style["icon"], style["word"]), ("greyblue", "question", "QUIET"))

    def test_agent_style_error_is_red_with_x(self):
        style = layout.agent_style("error", False, 3)
        self.assertEqual((style["color"], style["icon"], style["word"]), ("red", "cross", "ERROR"))
        self.assertFalse(style["fill"])

    def test_done_is_green_check_and_dims_after_30_minutes(self):
        fresh = layout.agent_style("done", False, 60)
        old = layout.agent_style("done", False, layout.DONE_DIM_AFTER_S + 1)
        self.assertEqual((fresh["color"], fresh["icon"], fresh["word"]), ("green", "check", "DONE"))
        self.assertFalse(fresh["dim"])
        self.assertTrue(old["dim"])

    def test_idle_is_grey_open_circle_ready(self):
        style = layout.agent_style("idle", False, 3)
        self.assertEqual((style["color"], style["icon"], style["word"]), ("grey", "circle", "READY"))

    def test_quiet_only_applies_to_working(self):
        self.assertEqual(layout.agent_style("needs_you", True, 3)["word"], "NEEDS YOU")
        self.assertEqual(layout.agent_style("error", True, 3)["word"], "ERROR")

    def test_unknown_state_is_grey_with_a_fixed_word(self):
        style = layout.agent_style("\x1b[2J", False, 3)
        self.assertEqual((style["color"], style["word"]), ("grey", "?"))

    def test_ended_ghost_is_grey_ended(self):
        self.assertEqual(layout.agent_style("ended", False, 0)["word"], "ENDED")


class AgentPagesTest(unittest.TestCase):
    def test_agent_pages_split_by_five(self):
        self.assertEqual(layout.agent_pages(0), [])
        self.assertEqual(layout.agent_pages(1), [(0, 1)])
        self.assertEqual(layout.agent_pages(5), [(0, 5)])
        self.assertEqual(layout.agent_pages(6), [(0, 5), (5, 6)])
        self.assertEqual(layout.agent_pages(16), [(0, 5), (5, 10), (10, 15), (15, 16)])

    def test_page_ring_order_overview_agents_devices(self):
        ring = layout.page_ring(2, 7)
        self.assertEqual(ring, [
            ("overview", None), ("agents", (0, 5)), ("agents", (5, 7)), ("device", 0), ("device", 1),
        ])

    def test_page_ring_without_sessions_has_no_agents_page(self):
        self.assertEqual(layout.page_ring(1, 0), [("overview", None), ("device", 0)])

    def test_plugin_pages_come_after_the_devices(self):
        # no panel turned on: the ring is exactly what it was before plugins
        self.assertEqual(layout.page_ring(1, 0, 0), [("overview", None), ("device", 0)])
        self.assertEqual(layout.page_ring(1, 0, 1), [("overview", None), ("device", 0), ("plugin", 0)])
        self.assertEqual(layout.page_ring(0, 6, 3), [
            ("overview", None), ("agents", (0, 5)), ("agents", (5, 6)),
            ("plugin", 0), ("plugin", 1), ("plugin", 2),
        ])

    def test_a_plugin_page_is_keyed_by_its_plugin_name(self):
        panels = [{"plugin": "weather"}, {"plugin": "ups-2"}]
        devices = [{"device_id": "dp-1"}]
        ring = layout.page_ring(1, 0, 2)
        self.assertEqual(layout.page_keys(ring, devices, panels), [
            ("overview", None), ("device", "dp-1"), ("plugin", "weather"), ("plugin", "ups-2"),
        ])
        # without panels, page_keys reads as before
        self.assertEqual(layout.page_keys(layout.page_ring(1, 0), devices),
                         [("overview", None), ("device", "dp-1")])


class AgentRowsLayoutTest(unittest.TestCase):
    def test_single_session_gets_card_layout(self):
        rects = layout.agent_rows_layout(WIDTH, HEIGHT, 1, 40, 24)
        self.assertEqual(rects, [(0, 40, WIDTH, HEIGHT - 40 - 24)])

    def test_rows_fit_between_header_and_dots_for_1_to_5(self):
        for n in range(1, 6):
            with self.subTest(rows=n):
                rects = layout.agent_rows_layout(WIDTH, HEIGHT, n, 40, 24)
                self.assertEqual(len(rects), n)
                self.assertEqual(rects[0][1], 40)
                last = rects[-1]
                self.assertLessEqual(last[1] + last[3], HEIGHT - 24)
                for (x, y, w, h), (_, next_y, _, _) in zip(rects, rects[1:]):
                    self.assertEqual(y + h, next_y)

    def test_row_heights_are_64_up_to_4_rows_and_51_for_5(self):
        self.assertEqual({r[3] for r in layout.agent_rows_layout(WIDTH, HEIGHT, 2, 40, 24)}, {64})
        self.assertEqual({r[3] for r in layout.agent_rows_layout(WIDTH, HEIGHT, 4, 40, 24)}, {64})
        self.assertEqual({r[3] for r in layout.agent_rows_layout(WIDTH, HEIGHT, 5, 40, 24)}, {51})

    def test_no_rows_is_empty(self):
        self.assertEqual(layout.agent_rows_layout(WIDTH, HEIGHT, 0, 40, 24), [])


class FmtElapsedTest(unittest.TestCase):
    def test_fmt_elapsed_units(self):
        self.assertEqual(layout.fmt_elapsed(0), "0s")
        self.assertEqual(layout.fmt_elapsed(45), "45s")
        self.assertEqual(layout.fmt_elapsed(59), "59s")
        self.assertEqual(layout.fmt_elapsed(60), "1m")
        self.assertEqual(layout.fmt_elapsed(12 * 60 + 30), "12m")
        self.assertEqual(layout.fmt_elapsed(3 * 3600 + 59), "3h")
        self.assertEqual(layout.fmt_elapsed(2 * 86400 + 5), "2d")

    def test_fmt_elapsed_bad_input_is_0s(self):
        self.assertEqual(layout.fmt_elapsed(-5), "0s")
        self.assertEqual(layout.fmt_elapsed(None), "0s")
        self.assertEqual(layout.fmt_elapsed("12"), "0s")


class AgentSummaryTest(unittest.TestCase):
    def test_agent_summary_none_when_empty(self):
        self.assertIsNone(layout.agent_summary([]))

    def test_agent_summary_counts_error_and_drops_zero_parts(self):
        sessions = [
            agent("a", "needs_you"), agent("b", "error"), agent("c", "working"), agent("d", "working"),
        ]
        parts = layout.agent_summary(sessions)
        self.assertEqual(parts, [
            ("needs_you", "1 needs you"), ("error", "1 error"), ("working", "2 working"),
        ])

    def test_agent_summary_includes_done_and_ready(self):
        parts = layout.agent_summary([agent("a", "done"), agent("b", "idle"), agent("c", "idle")])
        self.assertEqual(parts, [("done", "1 done"), ("idle", "2 ready")])

    def test_agent_summary_ignores_ghosts(self):
        self.assertIsNone(layout.agent_summary([agent("a", "done", ghost=True)]))


class AgentLineTest(unittest.TestCase):
    def test_line_two_is_label_dot_doing(self):
        self.assertEqual(layout.agent_line2(agent("a", doing="allow Bash?")), "my-pc \u00b7 allow Bash?")

    def test_line_two_uses_device_id_without_a_label(self):
        self.assertEqual(layout.agent_line2(agent("a", label="", doing="")), "dp-3f9a0c41")

    def test_quiet_line_two_says_no_news(self):
        line = layout.agent_line2(agent("a", "working", quiet=True, state_s=3600, silent_s=720))
        self.assertEqual(line, "my-pc \u00b7 no news for 12m")

    def test_quiet_line_two_falls_back_to_state_s_without_silent_s(self):
        line = layout.agent_line2(agent("a", "working", quiet=True, state_s=720))
        self.assertEqual(line, "my-pc \u00b7 no news for 12m")

    def test_quiet_line_two_bad_silent_s_falls_back_to_state_s(self):
        for bad in (None, "720", True):
            with self.subTest(bad=bad):
                line = layout.agent_line2(agent("a", "working", quiet=True, state_s=720, silent_s=bad))
                self.assertEqual(line, "my-pc \u00b7 no news for 12m")

    def test_project_title_shows_question_mark_when_hidden(self):
        self.assertEqual(layout.agent_title(agent("a", project="")), "?")
        self.assertEqual(layout.agent_title(agent("a", project="deskpane")), "deskpane")


class FitTextTest(unittest.TestCase):
    @staticmethod
    def measure(text):
        # every character is 10 px wide
        return 10 * len(text)

    def test_fit_text_keeps_short_text(self):
        self.assertEqual(layout.fit_text("abc", 30, self.measure), "abc")

    def test_fit_text_ellipsises_with_fake_measure(self):
        result = layout.fit_text("abcdefghij", 50, self.measure)
        self.assertEqual(result, "abcd" + layout.ELLIPSIS)
        self.assertLessEqual(self.measure(result), 50)

    def test_fit_text_too_narrow_for_anything_is_empty(self):
        self.assertEqual(layout.fit_text("abcdef", 5, self.measure), "")

    def test_fit_text_measures_a_handful_of_times_not_once_per_character(self):
        # security review MEDIUM-1: a 48-character label used to cost up to 47
        # measurements a frame; a binary search needs about 7
        calls = []

        def counting(text):
            calls.append(text)
            return 10 * len(text)
        result = layout.fit_text("x" * 48, 105, counting)
        self.assertEqual(result, "x" * 9 + layout.ELLIPSIS)
        self.assertLessEqual(len(calls), 8)

    def test_fit_text_matches_the_one_by_one_answer(self):
        # the longest prefix that fits, for every width, even with odd widths
        def uneven(text):
            return sum(14 if char in "mw" else 6 for char in text)
        text = "mwa bmw cmw dmwmw e fmm"
        for max_w in range(0, uneven(text) + 20):
            want = ""
            if uneven(text) <= max_w:
                want = text
            else:
                for end in range(len(text) - 1, 0, -1):
                    candidate = text[:end].rstrip() + layout.ELLIPSIS
                    if uneven(candidate) <= max_w:
                        want = candidate
                        break
            with self.subTest(max_w=max_w):
                self.assertEqual(layout.fit_text(text, max_w, uneven), want)

    def test_fit_text_empty_and_none(self):
        self.assertEqual(layout.fit_text("", 50, self.measure), "")
        self.assertEqual(layout.fit_text(None, 50, self.measure), "")


class FocusTest(unittest.TestCase):
    def test_focus_only_on_new_transition_into_needs_you(self):
        self.assertTrue(layout.should_focus_agents({"a": "working"}, {"a": "needs_you"}, 0, 100))
        # still amber: no second jump
        self.assertFalse(layout.should_focus_agents({"a": "needs_you"}, {"a": "needs_you"}, 0, 100))
        # a new session that arrives waiting also counts
        self.assertTrue(layout.should_focus_agents({}, {"b": "needs_you"}, 0, 100))
        self.assertFalse(layout.should_focus_agents({"a": "needs_you"}, {"a": "working"}, 0, 100))

    def test_focus_not_triggered_by_error(self):
        self.assertFalse(layout.should_focus_agents({"a": "working"}, {"a": "error"}, 0, 100))

    def test_focus_suppressed_after_recent_tap(self):
        self.assertFalse(layout.should_focus_agents({"a": "working"}, {"a": "needs_you"}, 95, 100))
        self.assertTrue(layout.should_focus_agents({"a": "working"}, {"a": "needs_you"}, 90, 100))

    def test_focus_not_on_the_first_line(self):
        self.assertFalse(layout.should_focus_agents(None, {"a": "needs_you"}, 0, 100))

    def test_focus_with_no_tap_yet(self):
        self.assertTrue(layout.should_focus_agents({"a": "idle"}, {"a": "needs_you"}, None, 5))


class ParseSocketLineTest(unittest.TestCase):
    def test_parse_socket_line_envelope_and_legacy_array(self):
        devices = [{"device_id": "dp-1", "online": True}]
        sessions = [agent("3f9a0c41b2d7")]
        envelope = {"v": 1, "views": {"stats": {"devices": devices}, "agents": {"sessions": sessions}}}
        self.assertEqual(layout.parse_socket_line(envelope), (devices, sessions))
        self.assertEqual(layout.parse_socket_line(devices), (devices, []))

    def test_parse_socket_line_real_daemon_shape(self):
        # the exact line the Rust daemon sends (T-041), with silent_s
        line = (
            '{"v":1,"views":{"stats":{"devices":[{"device_id":"dp-1","online":true}]},'
            '"agents":{"sessions":[{"sid":"3f9a0c41b2d7","device_id":"dp-3f9a0c41","label":"my-pc",'
            '"project":"deskpane","state":"working","doing":"Bash","running_s":1520,"state_s":34,'
            '"silent_s":700,"quiet":true,"ctx":null}]}}}'
        )
        devices, sessions = layout.parse_socket_line(json.loads(line))
        self.assertEqual(devices, [{"device_id": "dp-1", "online": True}])
        self.assertEqual(len(sessions), 1)
        session = sessions[0]
        self.assertEqual(sorted(session), sorted([
            "sid", "device_id", "label", "project", "state", "doing",
            "running_s", "state_s", "silent_s", "quiet", "ctx",
        ]))
        self.assertEqual(session["silent_s"], 700)
        self.assertIsNone(session["ctx"])
        self.assertEqual(layout.agent_line2(session), "my-pc \u00b7 no news for 11m")

    def test_envelope_without_agents_view_has_no_sessions(self):
        envelope = {"v": 1, "views": {"stats": {"devices": []}}}
        self.assertEqual(layout.parse_socket_line(envelope), ([], []))

    def test_envelope_ignores_later_views(self):
        envelope = {"v": 1, "views": {"stats": {"devices": []}, "agents": {"sessions": []}, "tasks": {}}}
        self.assertEqual(layout.parse_socket_line(envelope), ([], []))

    def test_unknown_version_or_shape_is_none(self):
        for obj in [{"v": 2, "views": {}}, {"views": {}}, "hello", 42, None, {"v": 1, "views": []}]:
            with self.subTest(obj=obj):
                self.assertIsNone(layout.parse_socket_line(obj))

    def test_non_dict_items_are_dropped(self):
        envelope = {"v": 1, "views": {"stats": {"devices": [1, {"device_id": "a"}]},
                                      "agents": {"sessions": ["x", {"sid": "b"}]}}}
        self.assertEqual(layout.parse_socket_line(envelope), ([{"device_id": "a"}], [{"sid": "b"}]))
        self.assertEqual(layout.parse_socket_line([None, {"device_id": "a"}]), ([{"device_id": "a"}], []))

    def test_wrong_inner_types_become_empty(self):
        envelope = {"v": 1, "views": {"stats": {"devices": "x"}, "agents": {"sessions": {}}}}
        self.assertEqual(layout.parse_socket_line(envelope), ([], []))


class OverviewAgentsLineTest(unittest.TestCase):
    def test_agents_line_sits_below_the_summary(self):
        result = layout.overview_layout(WIDTH, HEIGHT, CLOCK_H, DATE_H, SUMMARY_H, agents_h=SUMMARY_H)
        self.assertLessEqual(result["summary_y"] + SUMMARY_H, result["agents_y"])
        version_x, version_y = layout.version_position(HEIGHT, SUMMARY_H)
        self.assertLessEqual(result["agents_y"] + SUMMARY_H, version_y)

    def test_block_stays_centred_with_the_agents_line(self):
        result = layout.overview_layout(WIDTH, HEIGHT, CLOCK_H, DATE_H, SUMMARY_H, agents_h=SUMMARY_H)
        bottom = result["agents_y"] + SUMMARY_H
        self.assertLessEqual(abs(result["clock_y"] - (HEIGHT - bottom)), 1)

    def test_no_agents_line_keeps_the_old_layout(self):
        old = layout.overview_layout(WIDTH, HEIGHT, CLOCK_H, DATE_H, SUMMARY_H)
        self.assertNotIn("agents_y", old)



class PageKeysTest(unittest.TestCase):
    def test_page_keys_are_stable_names(self):
        ring = layout.page_ring(2, 7)
        devices = [{"device_id": "dp-a"}, {"device_id": "dp-b"}]
        self.assertEqual(layout.page_keys(ring, devices), [
            ("overview", None), ("agents", 0), ("agents", 1), ("device", "dp-a"), ("device", "dp-b"),
        ])

    def test_fallback_for_a_vanished_agents_page_is_the_last_agents_page(self):
        keys = [("overview", None), ("agents", 0), ("device", "dp-a")]
        self.assertEqual(layout.fallback_key(("agents", 2), keys), ("agents", 0))

    def test_fallback_otherwise_is_the_overview(self):
        keys = [("overview", None), ("device", "dp-a")]
        self.assertEqual(layout.fallback_key(("agents", 0), keys), ("overview", None))
        self.assertEqual(layout.fallback_key(("device", "dp-gone"), keys), ("overview", None))


class StepFocusTest(unittest.TestCase):
    """step_focus: the focus jump to the agents page as a pure state machine."""

    KEYS = [("overview", None), ("agents", 0), ("device", "dp-a"), ("device", "dp-b")]

    def start(self, page_key=("device", "dp-a")):
        return layout.focus_state(page_key)

    def line(self, state, prev, new, now, last_tap=None, blocked=False, went_offline=None):
        event = {"kind": "line", "prev_states": prev, "new_states": new,
                 "last_tap": last_tap, "blocked": blocked}
        if went_offline is not None:
            event["went_offline"] = went_offline
        return layout.step_focus(state, event, now)

    def test_current_device_going_offline_goes_to_the_overview(self):
        state = self.line(self.start(("device", "dp-a")), {}, {}, 100, went_offline={"dp-a"})
        self.assertEqual(state["page_key"], ("overview", None))

    def test_another_device_going_offline_keeps_the_page(self):
        state = self.line(self.start(("device", "dp-a")), {}, {}, 100, went_offline={"dp-b"})
        self.assertEqual(state["page_key"], ("device", "dp-a"))

    def test_device_going_offline_does_not_move_other_pages(self):
        for page_key in (("overview", None), ("agents", 0)):
            state = self.line(self.start(page_key), {}, {}, 100, went_offline={"dp-a"})
            self.assertEqual(state["page_key"], page_key)

    def test_offline_return_page_returns_to_the_overview(self):
        # an agent focus jump from dp-a, then dp-a goes offline during the focus
        state = self.line(self.start(("device", "dp-a")), {"a": "working"}, {"a": "needs_you"}, 100)
        state = self.line(state, {"a": "needs_you"}, {"a": "needs_you"}, 101, went_offline={"dp-a"})
        self.assertEqual(state["page_key"], ("agents", 0))
        self.assertEqual(state["focus_return"], ("overview", None))
        state = self.tick(state, 100 + layout.FOCUS_FOR_S)
        self.assertEqual(state["page_key"], ("overview", None))

    def test_needs_you_and_offline_in_one_line_still_jumps_to_agents(self):
        state = self.line(self.start(("device", "dp-a")), {"a": "working"}, {"a": "needs_you"}, 100,
                          went_offline={"dp-a"})
        self.assertEqual(state["page_key"], ("agents", 0))
        self.assertEqual(state["focus_return"], ("overview", None))

    def test_line_without_went_offline_is_unchanged(self):
        state = self.line(self.start(("device", "dp-a")), {}, {}, 100)
        self.assertEqual(state["page_key"], ("device", "dp-a"))


    def tick(self, state, now, keys=None):
        return layout.step_focus(state, {"kind": "tick", "keys": keys or self.KEYS}, now)

    def tap(self, state, now, keys=None, advance=True):
        return layout.step_focus(state, {"kind": "tap", "keys": keys or self.KEYS, "advance": advance}, now)

    def test_needs_you_jumps_to_the_first_agents_page(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100)
        self.assertEqual(state["page_key"], ("agents", 0))
        self.assertEqual(state["focus_until"], 100 + layout.FOCUS_FOR_S)
        self.assertEqual(state["focus_return"], ("device", "dp-a"))

    def test_focus_ends_and_returns_to_the_previous_page(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100)
        state = self.tick(state, 100 + layout.FOCUS_FOR_S - 1)
        self.assertEqual(state["page_key"], ("agents", 0))
        state = self.tick(state, 100 + layout.FOCUS_FOR_S)
        self.assertEqual(state["page_key"], ("device", "dp-a"))
        self.assertEqual(state["focus_until"], 0.0)
        self.assertIsNone(state["focus_return"])

    def test_a_tap_ends_focus(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100)
        state = self.tap(state, 105)
        self.assertEqual(state["focus_until"], 0.0)
        self.assertIsNone(state["focus_return"])
        # the tap moves on from the agents page as any tap does
        self.assertEqual(state["page_key"], ("device", "dp-a"))
        # and no return jump later
        self.assertEqual(self.tick(state, 200)["page_key"], ("device", "dp-a"))

    def test_a_tap_that_does_not_advance_still_ends_focus(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100)
        state = self.tap(state, 105, advance=False)
        self.assertEqual(state["page_key"], ("agents", 0))
        self.assertEqual(state["focus_until"], 0.0)

    def test_tap_wraps_around_the_ring(self):
        state = self.tap(self.start(("device", "dp-b")), 10)
        self.assertEqual(state["page_key"], ("overview", None))

    def test_second_needs_you_during_focus_keeps_the_original_return_page(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100)
        state = self.line(state, {"a": "needs_you", "b": "working"}, {"a": "needs_you", "b": "needs_you"}, 110)
        self.assertEqual(state["focus_return"], ("device", "dp-a"))
        self.assertEqual(state["focus_until"], 110 + layout.FOCUS_FOR_S)
        state = self.tick(state, 110 + layout.FOCUS_FOR_S)
        self.assertEqual(state["page_key"], ("device", "dp-a"))

    def test_line_on_the_frame_after_expiry_keeps_the_original_return_page(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100)
        # the line arrives after focus_until passed, before any tick has run
        late = 100 + layout.FOCUS_FOR_S + 0.5
        state = self.line(state, {"a": "needs_you", "b": "working"}, {"a": "needs_you", "b": "needs_you"}, late)
        self.assertEqual(state["focus_return"], ("device", "dp-a"))
        self.assertEqual(state["focus_until"], late + layout.FOCUS_FOR_S)
        state = self.tick(state, late + layout.FOCUS_FOR_S)
        self.assertEqual(state["page_key"], ("device", "dp-a"))

    def test_return_page_gone_falls_back(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100)
        keys = [("overview", None), ("agents", 0), ("device", "dp-b")]
        state = self.tick(state, 100 + layout.FOCUS_FOR_S, keys=keys)
        self.assertEqual(state["page_key"], ("overview", None))
        self.assertIsNone(state["focus_return"])

    def test_current_page_gone_falls_back_without_focus(self):
        keys = [("overview", None), ("agents", 0)]
        self.assertEqual(self.tick(self.start(("agents", 3)), 5, keys=keys)["page_key"], ("agents", 0))

    def test_no_focus_on_error(self):
        state = self.line(self.start(), {"a": "working"}, {"a": "error"}, 100)
        self.assertEqual(state["page_key"], ("device", "dp-a"))
        self.assertEqual(state["focus_until"], 0.0)

    def test_no_focus_on_the_first_socket_line(self):
        state = self.line(self.start(), None, {"a": "needs_you"}, 100)
        self.assertEqual(state["page_key"], ("device", "dp-a"))
        self.assertEqual(state["focus_until"], 0.0)

    def test_no_focus_right_after_a_tap_or_while_blocked(self):
        recent = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100, last_tap=95)
        self.assertEqual(recent["focus_until"], 0.0)
        blocked = self.line(self.start(), {"a": "working"}, {"a": "needs_you"}, 100, blocked=True)
        self.assertEqual(blocked["focus_until"], 0.0)

    def test_step_focus_does_not_change_its_input(self):
        state = self.start()
        before = dict(state)
        self.line(state, {"a": "working"}, {"a": "needs_you"}, 100)
        self.assertEqual(state, before)


class DisplayBackendTest(unittest.TestCase):
    """pick_backend and pick_scale read DESKPANE_DISPLAY and DESKPANE_SCALE
    (T-061); the Pi sets neither, so it keeps the framebuffer."""

    def test_unset_or_empty_is_the_framebuffer(self):
        self.assertEqual(layout.pick_backend(None), ("fb", None))
        self.assertEqual(layout.pick_backend(""), ("fb", None))

    def test_known_values(self):
        for value in ("fb", "window", "png"):
            self.assertEqual(layout.pick_backend(value), (value, None))
        self.assertEqual(layout.pick_backend(" Window "), ("window", None))

    def test_unknown_value_falls_back_to_fb_with_a_note(self):
        backend, note = layout.pick_backend("x11")
        self.assertEqual(backend, "fb")
        self.assertIn("x11", note)
        self.assertIn("fb", note)

    def test_scale_default_range_and_bad_values(self):
        self.assertEqual(layout.pick_scale(None), 2)
        self.assertEqual(layout.pick_scale(""), 2)
        self.assertEqual(layout.pick_scale("3"), 3)
        self.assertEqual(layout.pick_scale("0"), 1)
        self.assertEqual(layout.pick_scale("9"), 4)
        self.assertEqual(layout.pick_scale("big"), 2)


class ScaleTapTest(unittest.TestCase):
    """A click in the scaled preview window becomes a tap in panel pixels."""

    def test_scales_one_to_four(self):
        for scale in (1, 2, 3, 4):
            self.assertEqual(layout.scale_tap((100 * scale, 50 * scale), scale), (100, 50))
            self.assertEqual(layout.scale_tap((100 * scale + scale - 1, 50 * scale), scale), (100, 50))

    def test_clicks_outside_clamp_to_the_screen(self):
        self.assertEqual(layout.scale_tap((-5, -9), 2), (0, 0))
        self.assertEqual(layout.scale_tap((5000, 5000), 2), (WIDTH - 1, HEIGHT - 1))
        self.assertEqual(layout.scale_tap((960, 640), 2, (WIDTH, HEIGHT)), (WIDTH - 1, HEIGHT - 1))

    def test_a_bad_scale_counts_as_one(self):
        self.assertEqual(layout.scale_tap((10, 20), 0), (10, 20))


class BatteryFieldsTest(unittest.TestCase):
    """battery_fields reads the optional T-064 fields of one device."""

    def test_absent_fields_are_none(self):
        self.assertIsNone(layout.battery_fields({"device_id": "pi-zero"}))
        self.assertIsNone(layout.battery_fields({"charging": True}))

    def test_valid_fields(self):
        self.assertEqual(layout.battery_fields({"battery_pct": 87.5, "charging": True}), (87.5, True))
        self.assertEqual(layout.battery_fields({"battery_pct": 0}), (0.0, False))
        self.assertEqual(layout.battery_fields({"battery_pct": 100, "charging": False}), (100.0, False))

    def test_bad_values_are_none(self):
        for pct in (-1, 100.5, float("nan"), float("inf"), "87", True, None, [50]):
            with self.subTest(pct=pct):
                self.assertIsNone(layout.battery_fields({"battery_pct": pct, "charging": False}))

    def test_charging_must_be_exactly_true(self):
        self.assertEqual(layout.battery_fields({"battery_pct": 50, "charging": "yes"}), (50.0, False))
        self.assertEqual(layout.battery_fields({"battery_pct": 50, "charging": 1}), (50.0, False))

    def test_not_a_dict_is_none(self):
        self.assertIsNone(layout.battery_fields(None))
        self.assertIsNone(layout.battery_fields([1, 2]))


class LocalBatteryTest(unittest.TestCase):
    """local_battery picks the battery of the Pi itself (local: true) for the
    overview; nothing when there is none or the choice is not clear."""

    def pi(self, pct=87, charging=True, **extra):
        device = {"device_id": "pi-zero", "local": True, "online": True, "battery_pct": pct, "charging": charging}
        device.update(extra)
        return device

    def test_none_without_devices_or_without_a_local_one(self):
        self.assertIsNone(layout.local_battery([]))
        laptop = {"device_id": "laptop", "online": True, "battery_pct": 50, "charging": False}
        self.assertIsNone(layout.local_battery([laptop]))

    def test_one_local_device(self):
        desk = {"device_id": "desk-pc", "online": True}
        self.assertEqual(layout.local_battery([desk, self.pi()]), (87.0, True))

    def test_local_without_battery_is_none(self):
        self.assertIsNone(layout.local_battery([{"device_id": "pi-zero", "local": True, "online": True}]))

    def test_two_local_devices_is_none(self):
        other = self.pi(pct=20, device_id="pi-two")
        self.assertIsNone(layout.local_battery([self.pi(), other]))

    def test_offline_or_not_exactly_true_local_is_skipped(self):
        self.assertIsNone(layout.local_battery([self.pi(online=False)]))
        self.assertIsNone(layout.local_battery([self.pi(local="true")]))


class BatteryStyleTest(unittest.TestCase):
    """battery_style: the text, the low flag (red under 20 %) and the colour
    name; colour is never the only signal, the number says the same."""

    def test_boundaries(self):
        self.assertEqual(layout.battery_style(0, False), {"text": "0%", "low": True, "charging": False, "color": "red"})
        self.assertEqual(layout.battery_style(19.9, False)["text"], "19%")
        self.assertTrue(layout.battery_style(19.9, False)["low"])
        self.assertEqual(layout.battery_style(20, False), {"text": "20%", "low": False, "charging": False, "color": "text"})
        self.assertEqual(layout.battery_style(100, True), {"text": "100%", "low": False, "charging": True, "color": "text"})

    def test_charging_low_is_still_red(self):
        style = layout.battery_style(15, True)
        self.assertTrue(style["charging"])
        self.assertEqual(style["color"], "red")


class BatteryGeometryTest(unittest.TestCase):
    """battery_geometry: an outline 22 x 11 with a 2 x 5 nub on the right
    and the fill in proportion; bolt_points: the charging mark."""

    def test_full_and_empty(self):
        full = layout.battery_geometry(100, 10, 100)
        self.assertEqual(full["body"], (100, 10, 22, 11))
        self.assertEqual(full["nub"], (122, 13, 2, 5))
        self.assertEqual(full["fill"], (102, 12, 18, 7))
        self.assertIsNone(layout.battery_geometry(100, 10, 0)["fill"])

    def test_fill_is_proportional_and_never_vanishes_above_zero(self):
        self.assertEqual(layout.battery_geometry(0, 0, 50)["fill"][2], 9)
        self.assertEqual(layout.battery_geometry(0, 0, 1)["fill"][2], 1)
        self.assertEqual(layout.battery_geometry(0, 0, 87)["fill"][2], 16)

    def test_bolt_is_six_points_inside_its_box(self):
        bolt = layout.bolt_points(40, 20)
        self.assertEqual(len(bolt), 6)
        for x, y in bolt:
            self.assertTrue(40 <= x < 40 + layout.BOLT_W, (x, y))
            self.assertTrue(20 <= y < 20 + layout.BATTERY_BODY_H, (x, y))

    def test_widget_layout_right_aligned_text_left_of_icon(self):
        place = layout.battery_widget_layout(472, 8, 30, 11)
        # nub ends at the right edge
        self.assertEqual(place["icon"][0] + layout.BATTERY_W, 472)
        self.assertEqual(place["icon"][1], 8)
        self.assertEqual(place["text"][0] + 30 + layout.BATTERY_TEXT_GAP, place["icon"][0])
        self.assertEqual(place["text"][1], 8)
        self.assertIsNone(place["bolt"])
        self.assertEqual(place["left"], place["text"][0])
        # a tall text moves the icon down to its middle instead
        tall = layout.battery_widget_layout(472, 8, 30, 21)
        self.assertEqual(tall["text"][1], 8)
        self.assertEqual(tall["icon"][1], 13)

    def test_charging_puts_the_bolt_left_of_the_text(self):
        place = layout.battery_widget_layout(472, 8, 30, 11, charging=True)
        bolt_x, bolt_y = place["bolt"]
        self.assertEqual(bolt_x + layout.BOLT_W + layout.BOLT_GAP, place["text"][0])
        self.assertEqual(bolt_y, place["icon"][1])
        self.assertEqual(place["left"], bolt_x)

    def test_device_page_hostname_ends_clear_of_the_battery(self):
        # every character is 10 px wide; the hostname starts at x = 14
        def measure(text):
            return 10 * len(text)

        place = layout.battery_widget_layout(400, 14, 30, 11, charging=True)
        max_w = layout.host_max_w(14, place["left"])
        self.assertEqual(max_w, place["left"] - layout.HOST_CLEAR_GAP - 14)
        self.assertGreaterEqual(layout.HOST_CLEAR_GAP, 8)
        long_host = "a-very-long-hostname-that-fills-the-whole-field-" + "z" * 16
        shown = layout.fit_text(long_host, max_w, measure)
        self.assertTrue(shown.endswith(layout.ELLIPSIS))
        self.assertLessEqual(14 + measure(shown), place["left"] - 8)
        # a short hostname is kept as is
        self.assertEqual(layout.fit_text("pi-zero", max_w, measure), "pi-zero")
        # no room at all is a width of 0, never negative
        self.assertEqual(layout.host_max_w(14, 10), 0)

    def test_overview_corner_stays_clear_of_the_clock(self):
        # top right, 8 px in; the clock block starts lower (overview_layout)
        place = layout.battery_widget_layout(WIDTH - layout.BATTERY_MARGIN, layout.BATTERY_MARGIN, 34, 11)
        clock_y = layout.overview_layout(WIDTH, HEIGHT, CLOCK_H, DATE_H, SUMMARY_H, agents_h=11)["clock_y"]
        self.assertLess(layout.BATTERY_MARGIN + 11, clock_y)
        self.assertGreater(place["left"], WIDTH // 2)


class GoneOfflineTest(unittest.TestCase):
    """online_ids and gone_offline: which devices just went from online to offline."""

    def dev(self, device_id, online):
        return {"device_id": device_id, "online": online}

    def test_online_ids_lists_only_online_devices(self):
        devices = [self.dev("dp-a", True), self.dev("dp-b", False), {"device_id": "dp-c"}]
        self.assertEqual(layout.online_ids(devices), {"dp-a"})

    def test_first_line_never_reports_anything(self):
        self.assertEqual(layout.gone_offline(None, [self.dev("dp-a", False)]), set())

    def test_online_to_offline_is_reported(self):
        self.assertEqual(layout.gone_offline({"dp-a", "dp-b"}, [self.dev("dp-a", False), self.dev("dp-b", True)]),
                         {"dp-a"})

    def test_already_offline_device_is_not_reported(self):
        # viewing a device that was offline before is a deliberate choice: no jump
        self.assertEqual(layout.gone_offline(set(), [self.dev("dp-a", False)]), set())

    def test_new_device_arriving_offline_is_not_reported(self):
        self.assertEqual(layout.gone_offline({"dp-a"}, [self.dev("dp-a", True), self.dev("dp-new", False)]), set())

    def test_forgotten_device_is_not_reported(self):
        # a device the daemon dropped is gone from the ring; fallback_key handles it
        self.assertEqual(layout.gone_offline({"dp-a"}, []), set())


if __name__ == "__main__":
    unittest.main()
