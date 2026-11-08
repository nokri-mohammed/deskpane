"""Tests for pi/charpack.py: the character pack format, looks, poses and
the zip install plan. Plain unittest, no pygame, no Pillow, and never the
real pack: every PNG and zip is built in fixtures.py.
Run from the repo root with:
    python3 -m unittest discover -s pi/tests
"""
import io
import os
import sys
import unittest
import zipfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import charpack  # noqa: E402
import fixtures  # noqa: E402


def entries_of(raw_zip):
    """PackEntry list from fixture zip bytes, the way the installer reads it."""
    with zipfile.ZipFile(io.BytesIO(raw_zip)) as zf:
        return [charpack.entry_from_zipinfo(info) for info in zf.infolist()]


def plan_for(entries):
    return charpack.plan_pack_install(entries_of(fixtures.zip_bytes(entries)))


class FormatTest(unittest.TestCase):
    def test_format_constants_match_pack_v1(self):
        self.assertEqual(charpack.FRAME, 16)
        self.assertEqual((charpack.SHEET_W, charpack.SHEET_H), (128, 208))
        self.assertEqual((charpack.EMOTE_W, charpack.EMOTE_H), (64, 16))
        self.assertEqual(charpack.MAX_CHARACTERS, 15)
        self.assertEqual(charpack.DIRECTIONS, ("front", "left", "back", "right"))
        self.assertEqual(len(charpack.EMOTE_NAMES), 10)
        self.assertEqual(charpack.CHAR_FPS, 5)

    def test_frame_rect_skips_header_row(self):
        # variant 0, front, first idle frame sits one row below the header
        self.assertEqual(charpack.frame_rect(0, "front", "idle", 0), (0, 16, 16, 16))

    def test_frame_rect_variant_direction_and_column(self):
        # variant 2, back, walk frame 3: x 16 * 7, y 16 * (1 + 8 + 2)
        self.assertEqual(charpack.frame_rect(2, "back", "walk", 3), (112, 176, 16, 16))
        # variant 1, right, idle frame 2
        self.assertEqual(charpack.frame_rect(1, "right", "idle", 2), (32, 16 * 8, 16, 16))

    def test_idle_uses_columns_0_to_3_walk_uses_4_to_7(self):
        idle_x = [charpack.frame_rect(0, "front", "idle", f)[0] // 16 for f in range(4)]
        walk_x = [charpack.frame_rect(0, "front", "walk", f)[0] // 16 for f in range(4)]
        self.assertEqual(idle_x, [0, 1, 2, 3])
        self.assertEqual(walk_x, [4, 5, 6, 7])

    def test_every_frame_rect_is_inside_the_sheet(self):
        for variant in range(3):
            for direction in charpack.DIRECTIONS:
                for motion in ("idle", "walk"):
                    for frame in range(4):
                        x, y, w, h = charpack.frame_rect(variant, direction, motion, frame)
                        self.assertLessEqual(x + w, charpack.SHEET_W)
                        self.assertLessEqual(y + h, charpack.SHEET_H)

    def test_frame_rect_rejects_bad_input(self):
        with self.assertRaises(ValueError):
            charpack.frame_rect(3, "front", "idle", 0)
        with self.assertRaises(ValueError):
            charpack.frame_rect(0, "up", "idle", 0)
        with self.assertRaises(ValueError):
            charpack.frame_rect(0, "front", "run", 0)
        with self.assertRaises(ValueError):
            charpack.frame_rect(0, "front", "idle", 4)

    def test_emote_rect_per_frame(self):
        self.assertEqual(charpack.emote_rect(0), (0, 0, 16, 16))
        self.assertEqual(charpack.emote_rect(3), (48, 0, 16, 16))
        with self.assertRaises(ValueError):
            charpack.emote_rect(4)


class LooksTest(unittest.TestCase):
    def test_look_count_is_three_per_character(self):
        self.assertEqual(charpack.look_count(15), 45)
        self.assertEqual(charpack.look_count(2), 6)
        self.assertEqual(charpack.look_count(0), 0)

    def test_look_maps_to_character_and_variant(self):
        self.assertEqual(charpack.look_of(0), (1, 0))
        self.assertEqual(charpack.look_of(5), (2, 2))
        self.assertEqual(charpack.look_of(44), (15, 2))

    def test_look_for_project_is_stable(self):
        # first 8 bytes of sha256("deskpane-look-v1:deskpane") mod 45, fixed forever
        self.assertEqual(charpack.look_for_key("deskpane", 45), 12)
        self.assertEqual(charpack.look_for_key("deskpane", 45), charpack.look_for_key("deskpane", 45))

    def test_look_for_key_with_no_looks_is_zero(self):
        self.assertEqual(charpack.look_for_key("deskpane", 0), 0)

    def test_hidden_project_key_uses_device_id(self):
        self.assertEqual(charpack.look_key("deskpane", "dp-3f9a0c41"), "deskpane")
        self.assertEqual(charpack.look_key("", "dp-3f9a0c41"), "?@dp-3f9a0c41")
        self.assertNotEqual(charpack.look_key("", "dp-a"), charpack.look_key("", "dp-b"))

    def test_assign_looks_never_repeats_on_screen(self):
        keys = ["project-%d" % i for i in range(16)]
        looks = charpack.assign_looks(keys, {}, 45)
        self.assertEqual(set(looks), set(keys))
        self.assertEqual(len(set(looks.values())), 16)

    def test_assign_looks_keeps_existing_looks_when_a_project_leaves(self):
        first = charpack.assign_looks(["alpha", "beta", "gamma"], {}, 45)
        second = charpack.assign_looks(["alpha", "gamma"], first, 45)
        self.assertEqual(second, {"alpha": first["alpha"], "gamma": first["gamma"]})

    def test_assign_looks_takes_next_free_on_collision(self):
        start = charpack.look_for_key("beta", 45)
        # alpha already holds beta's start look
        looks = charpack.assign_looks(["alpha", "beta"], {"alpha": start}, 45)
        self.assertEqual(looks["alpha"], start)
        self.assertEqual(looks["beta"], (start + 1) % 45)

    def test_assign_looks_wraps_around_the_end(self):
        # find a key whose start look is the last one (5 of 6)
        key = next(k for k in ("key%d" % i for i in range(1000)) if charpack.look_for_key(k, 6) == 5)
        looks = charpack.assign_looks(["holder", key], {"holder": 5}, 6)
        self.assertEqual(looks[key], 0)

    def test_assign_looks_reuses_when_more_projects_than_looks(self):
        keys = ["p%d" % i for i in range(8)]
        looks = charpack.assign_looks(keys, {}, 6)
        self.assertEqual(set(looks), set(keys))
        self.assertEqual(len(set(looks.values())), 6)
        for key in keys:
            self.assertTrue(0 <= looks[key] < 6)

    def test_assign_looks_drops_previous_looks_out_of_range(self):
        # a smaller pack was installed: an old index past the end is reassigned
        looks = charpack.assign_looks(["alpha"], {"alpha": 40}, 6)
        self.assertTrue(0 <= looks["alpha"] < 6)

    def test_assign_looks_with_no_looks_is_empty(self):
        self.assertEqual(charpack.assign_looks(["alpha"], {}, 0), {})

    def test_assign_looks_is_independent_of_key_order(self):
        keys = ["p%d" % i for i in range(10)]
        a = charpack.assign_looks(keys, {}, 45)
        b = charpack.assign_looks(list(reversed(keys)), {}, 45)
        self.assertEqual(a, b)


class PoseTest(unittest.TestCase):
    def pose(self, state, quiet=False, state_s=100.0, dim=False, new_for_s=None, leaving_for_s=None):
        return charpack.character_pose(state, quiet, state_s, dim, new_for_s, leaving_for_s)

    def test_pose_per_state(self):
        expected = {
            "needs_you": ("front", "idle", "Exclamation"),
            "idle": ("front", "idle", "Interrogation"),
            "working": ("right", "walk", None),
            "error": ("front", "idle", "Sad"),
            "done": ("front", "idle", None),
        }
        for state, (direction, motion, emote) in expected.items():
            with self.subTest(state=state):
                pose = self.pose(state)
                self.assertEqual((pose["direction"], pose["motion"], pose["emote"]), (direction, motion, emote))
                self.assertFalse(pose["still"])
                self.assertEqual(pose["offset"], 0.0)

    def test_pose_quiet_working_stops_walking(self):
        pose = self.pose("working", quiet=True)
        self.assertEqual((pose["direction"], pose["motion"], pose["emote"]), ("right", "idle", None))

    def test_pose_dim_done_is_still_first_frame(self):
        pose = self.pose("done", dim=True, state_s=4000)
        self.assertEqual((pose["direction"], pose["motion"], pose["emote"]), ("front", "idle", None))
        self.assertTrue(pose["still"])
        self.assertTrue(pose["dim"])

    def test_happy_shows_for_5_seconds_after_done_then_none(self):
        self.assertEqual(self.pose("done", state_s=0)["emote"], "Happy")
        self.assertEqual(self.pose("done", state_s=4.9)["emote"], "Happy")
        self.assertIsNone(self.pose("done", state_s=5)["emote"])

    def test_surprised_once_for_a_new_session_not_on_first_line(self):
        self.assertEqual(self.pose("needs_you", new_for_s=0.5)["emote"], "Surprised")
        self.assertEqual(self.pose("needs_you", new_for_s=2.0)["emote"], "Exclamation")
        # not new (first line after start, or an old session): the state's emote
        self.assertEqual(self.pose("needs_you", new_for_s=None)["emote"], "Exclamation")
        self.assertEqual(self.pose("working", new_for_s=1.0)["emote"], "Surprised")

    def test_emote_shown_for_counts_from_when_the_emote_began(self):
        self.assertAlmostEqual(self.pose("idle", state_s=3.0)["emote_for_s"], 3.0)
        self.assertAlmostEqual(self.pose("idle", new_for_s=0.5, state_s=0.5)["emote_for_s"], 0.5)
        # after the 2 s surprise, the state's emote pops in fresh
        self.assertAlmostEqual(self.pose("idle", new_for_s=2.4, state_s=2.4)["emote_for_s"], 0.4)

    def test_leaving_walks_right_and_moves_off(self):
        pose = self.pose("done", leaving_for_s=1.0)
        self.assertEqual((pose["direction"], pose["motion"], pose["emote"]), ("right", "walk", None))
        self.assertAlmostEqual(pose["offset"], 0.5)
        self.assertEqual(self.pose("done", leaving_for_s=5.0)["offset"], 1.0)

    def test_unknown_state_is_front_idle_without_emote(self):
        pose = self.pose("dancing")
        self.assertEqual((pose["direction"], pose["motion"], pose["emote"]), ("front", "idle", None))

    def test_negative_times_do_not_break(self):
        pose = self.pose("done", state_s=-3)
        self.assertEqual(pose["emote"], "Happy")
        self.assertGreaterEqual(pose["emote_for_s"], 0)


class AnimationTest(unittest.TestCase):
    def test_anim_frame_at_5_fps_cycles_4_frames(self):
        frames = [charpack.anim_frame(ms, 5, 4) for ms in range(0, 1600, 200)]
        self.assertEqual(frames, [0, 1, 2, 3, 0, 1, 2, 3])
        self.assertEqual(charpack.anim_frame(199, 5, 4), 0)

    def test_anim_frame_handles_zero_and_negative(self):
        self.assertEqual(charpack.anim_frame(-500, 5, 4), 0)
        self.assertEqual(charpack.anim_frame(1000, 5, 0), 0)
        self.assertEqual(charpack.anim_frame(1000, 0, 4), 0)

    def test_emote_pop_in_plays_4_frames_then_holds_last(self):
        frames = [charpack.emote_frame(s / 10.0, 5) for s in range(0, 30, 2)]
        self.assertEqual(frames[:5], [0, 1, 2, 3, 3])
        self.assertTrue(all(f == 3 for f in frames[4:]))
        self.assertEqual(charpack.emote_frame(-1, 5), 0)

    def test_best_scale_is_integer_and_capped_at_8(self):
        self.assertEqual(charpack.best_scale(480, 200, 16), 8)
        self.assertEqual(charpack.best_scale(100, 200, 16), 6)
        self.assertEqual(charpack.best_scale(48, 48, 16, max_scale=3), 3)
        self.assertEqual(charpack.best_scale(40, 47, 16), 2)
        self.assertIsInstance(charpack.best_scale(99, 99, 16), int)

    def test_best_scale_too_small_is_zero(self):
        self.assertEqual(charpack.best_scale(15, 100, 16), 0)
        self.assertEqual(charpack.best_scale(100, 100, 0), 0)


class TrackerTest(unittest.TestCase):
    """track_sessions remembers what the last socket line held, so the
    screen knows which sessions are new and which just left."""

    def session(self, sid, state="working", project="p"):
        return {"sid": sid, "state": state, "project": project, "device_id": "dp-1", "state_s": 0}

    def test_first_line_marks_nothing_new(self):
        tracker = charpack.track_sessions(None, [self.session("a" * 12)], 100.0)
        self.assertIsNone(tracker["seen"]["a" * 12]["new_at"])
        self.assertEqual(tracker["ghosts"], {})

    def test_later_session_is_new_with_its_arrival_time(self):
        t = charpack.track_sessions(None, [self.session("a" * 12)], 100.0)
        t = charpack.track_sessions(t, [self.session("a" * 12), self.session("b" * 12)], 101.0)
        self.assertEqual(t["seen"]["b" * 12]["new_at"], 101.0)
        self.assertIsNone(t["seen"]["a" * 12]["new_at"])

    def test_state_change_records_local_time(self):
        t = charpack.track_sessions(None, [self.session("a" * 12, "working")], 100.0)
        t = charpack.track_sessions(t, [self.session("a" * 12, "working")], 101.0)
        self.assertEqual(t["seen"]["a" * 12]["state_at"], 100.0)
        t = charpack.track_sessions(t, [self.session("a" * 12, "done")], 102.5)
        self.assertEqual(t["seen"]["a" * 12]["state_at"], 102.5)

    def test_first_line_state_time_comes_from_the_daemon(self):
        s = self.session("a" * 12, "done")
        s["state_s"] = 30
        t = charpack.track_sessions(None, [s], 100.0)
        self.assertEqual(t["seen"]["a" * 12]["state_at"], 70.0)

    def test_session_gone_becomes_a_ghost_in_its_old_slot(self):
        sessions = [self.session("a" * 12), self.session("b" * 12)]
        t = charpack.track_sessions(None, sessions, 100.0)
        t = charpack.track_sessions(t, [self.session("a" * 12)], 101.0)
        ghost = t["ghosts"]["b" * 12]
        self.assertEqual(ghost["left_at"], 101.0)
        self.assertEqual(ghost["slot"], 1)
        self.assertEqual(ghost["session"]["sid"], "b" * 12)

    def test_ghost_expires_after_walk_off(self):
        t = charpack.track_sessions(None, [self.session("a" * 12)], 100.0)
        t = charpack.track_sessions(t, [], 101.0)
        t = charpack.track_sessions(t, [], 101.0 + charpack.WALK_OFF_S - 0.1)
        self.assertIn("a" * 12, t["ghosts"])
        t = charpack.track_sessions(t, [], 101.0 + charpack.WALK_OFF_S)
        self.assertEqual(t["ghosts"], {})

    def test_session_that_comes_back_is_not_a_ghost(self):
        t = charpack.track_sessions(None, [self.session("a" * 12)], 100.0)
        t = charpack.track_sessions(t, [], 101.0)
        t = charpack.track_sessions(t, [self.session("a" * 12)], 101.5)
        self.assertEqual(t["ghosts"], {})

    def test_with_ghosts_puts_each_ghost_back_in_its_slot(self):
        live = [{"sid": "a"}, {"sid": "c"}]
        ghosts = {"b": {"session": {"sid": "b"}, "left_at": 1.0, "slot": 1}}
        merged = charpack.with_ghosts(live, ghosts)
        self.assertEqual([row["sid"] for row in merged], ["a", "b", "c"])
        self.assertTrue(merged[1]["ghost"])
        self.assertFalse(merged[0].get("ghost", False))

    def test_with_ghosts_clamps_a_slot_past_the_end(self):
        ghosts = {"z": {"session": {"sid": "z"}, "left_at": 1.0, "slot": 9}}
        merged = charpack.with_ghosts([{"sid": "a"}], ghosts)
        self.assertEqual([row["sid"] for row in merged], ["a", "z"])


class PoseTimesTest(unittest.TestCase):
    """pose_times: the seconds character_pose needs, from the tracker."""

    def test_without_tracker_entry_uses_the_daemon_state_s(self):
        self.assertEqual(charpack.pose_times({"state_s": 40}, None, 100.0), (40, None, None))
        self.assertEqual(charpack.pose_times({}, None, 100.0), (0, None, None))

    def test_tracked_session_uses_local_state_time(self):
        seen = {"new_at": None, "state": "done", "state_at": 97.5}
        self.assertEqual(charpack.pose_times({"state_s": 40}, seen, 100.0), (2.5, None, None))

    def test_new_session_reports_seconds_since_it_appeared(self):
        seen = {"new_at": 99.0, "state": "idle", "state_at": 99.0}
        self.assertEqual(charpack.pose_times({"state_s": 0}, seen, 100.5), (1.5, 1.5, None))

    def test_ghost_reports_seconds_since_it_left(self):
        times = charpack.pose_times({"ghost": True, "left_at": 98.0, "state_s": 3}, None, 99.5)
        self.assertEqual(times, (3, None, 1.5))

    def test_ghost_without_left_at_has_just_left(self):
        self.assertEqual(charpack.pose_times({"ghost": True}, None, 50.0)[2], 0.0)

class PngHeaderTest(unittest.TestCase):
    def test_png_header_check_reads_size(self):
        self.assertEqual(charpack.png_size(fixtures.make_png(128, 208)), (128, 208))
        self.assertEqual(charpack.png_size(fixtures.make_png(64, 16)[:24]), (64, 16))

    def test_png_header_check_rejects_non_png(self):
        self.assertIsNone(charpack.png_size(b"GIF89a" + b"\x00" * 30))
        self.assertIsNone(charpack.png_size(b""))
        self.assertIsNone(charpack.png_size(fixtures.make_png(4, 4)[:20]))
        # signature right but the first chunk is not IHDR
        bad = fixtures.PNG_SIGNATURE + fixtures.png_chunk(b"tEXt", b"x" * 13)
        self.assertIsNone(charpack.png_size(bad))


class ZipPlanTest(unittest.TestCase):
    def assert_refused(self, entries, word):
        with self.assertRaises(charpack.PackError) as ctx:
            plan_for(entries)
        self.assertIn(word, str(ctx.exception))

    def test_zip_plan_accepts_full_pack_and_skips_extras(self):
        plan = plan_for(fixtures.full_pack_entries())
        targets = [item["target"] for item in plan["files"]]
        self.assertEqual(len([t for t in targets if t.startswith("NPC_")]), 15)
        self.assertEqual(len([t for t in targets if t.startswith("Emotes/")]), 10)
        self.assertIn("Emotes/Sad_emote_16x16.png", targets)
        # Patrons.txt, Thanks.txt, Palette/Palette.txt, Palette/rosyandblue.png, 2 folders
        self.assertEqual(plan["skipped"], 6)
        self.assertEqual(plan["characters"], 15)

    def test_zip_plan_targets_come_from_the_allowlist(self):
        plan = plan_for(fixtures.full_pack_entries(characters=2, emotes=("Happy",), extras=False))
        for item in plan["files"]:
            self.assertIn(item["target"], charpack.allowed_targets())
        self.assertEqual(plan["files"][0]["size"], (128, 208))

    def test_zip_plan_rejects_path_traversal(self):
        entries = fixtures.full_pack_entries(characters=1, extras=False) + [("../evil.txt", b"x")]
        self.assert_refused(entries, "unsafe")
        self.assert_refused([("NPC_1.png", fixtures.sheet_png()), ("Palette/../../x", b"x")], "unsafe")

    def test_zip_plan_rejects_absolute_path(self):
        self.assert_refused([("NPC_1.png", fixtures.sheet_png()), ("/etc/passwd", b"x")], "unsafe")

    def test_zip_plan_rejects_backslash_and_drive_paths(self):
        self.assert_refused([("NPC_1.png", fixtures.sheet_png()), ("Emotes\\x.png", b"x")], "unsafe")
        self.assert_refused([("NPC_1.png", fixtures.sheet_png()), ("C:/x.png", b"x")], "unsafe")
        self.assert_refused([("NPC_1.png", fixtures.sheet_png()), ("c:x.png", b"x")], "unsafe")

    def test_zip_plan_rejects_nul_in_name(self):
        entry = charpack.PackEntry("NPC_1.png\x00.txt", 10, 0, 0)
        with self.assertRaises(charpack.PackError):
            charpack.plan_pack_install([charpack.PackEntry("NPC_1.png", 10, 0, 0), entry])

    def test_zip_plan_rejects_symlink_entry(self):
        self.assert_refused(
            [("NPC_1.png", fixtures.sheet_png()), (fixtures.symlink_info("Thanks.txt"), b"/etc/shadow")],
            "symlink",
        )

    def test_zip_plan_rejects_duplicate_names(self):
        entries = [charpack.PackEntry("NPC_1.png", 100, 0, 0), charpack.PackEntry("NPC_1.png", 100, 0, 0)]
        with self.assertRaises(charpack.PackError) as ctx:
            charpack.plan_pack_install(entries)
        self.assertIn("twice", str(ctx.exception))

    def test_zip_plan_rejects_encrypted_entry(self):
        entries = [charpack.PackEntry("NPC_1.png", 100, 0, 0), charpack.PackEntry("Thanks.txt", 10, 0x1, 0)]
        with self.assertRaises(charpack.PackError) as ctx:
            charpack.plan_pack_install(entries)
        self.assertIn("encrypted", str(ctx.exception))

    def test_zip_plan_rejects_oversized_entry_and_too_many_entries(self):
        big = [charpack.PackEntry("NPC_1.png", charpack.MAX_ENTRY_BYTES + 1, 0, 0)]
        with self.assertRaises(charpack.PackError) as ctx:
            charpack.plan_pack_install(big)
        self.assertIn("too big", str(ctx.exception))

        many = [charpack.PackEntry("NPC_1.png", 100, 0, 0)]
        many += [charpack.PackEntry("extra%d.txt" % i, 1, 0, 0) for i in range(charpack.MAX_ENTRIES)]
        with self.assertRaises(charpack.PackError) as ctx:
            charpack.plan_pack_install(many)
        self.assertIn("entries", str(ctx.exception))

    def test_zip_plan_allows_exactly_max_entries(self):
        entries = [charpack.PackEntry("NPC_1.png", 100, 0, 0)]
        entries += [charpack.PackEntry("extra%d.txt" % i, 1, 0, 0) for i in range(charpack.MAX_ENTRIES - 1)]
        self.assertEqual(charpack.plan_pack_install(entries)["skipped"], charpack.MAX_ENTRIES - 1)

    def test_zip_plan_rejects_wrong_png_size(self):
        with self.assertRaises(charpack.PackError) as ctx:
            charpack.check_pack_file("NPC_1.png", fixtures.make_png(128, 192))
        self.assertIn("NPC_1.png", str(ctx.exception))
        with self.assertRaises(charpack.PackError):
            charpack.check_pack_file("Emotes/Sad_emote_16x16.png", fixtures.make_png(16, 16))
        with self.assertRaises(charpack.PackError):
            charpack.check_pack_file("NPC_1.png", b"not a png at all")
        with self.assertRaises(charpack.PackError):
            charpack.check_pack_file("NPC_1.png", b"x" * (charpack.MAX_ENTRY_BYTES + 1))
        # the right sizes pass
        charpack.check_pack_file("NPC_1.png", fixtures.sheet_png())
        charpack.check_pack_file("Emotes/Sad_emote_16x16.png", fixtures.emote_png())

    def test_zip_plan_needs_npc_1_and_contiguous_numbers(self):
        self.assert_refused([("NPC_2.png", fixtures.sheet_png())], "NPC_1.png")
        self.assert_refused(
            [("NPC_1.png", fixtures.sheet_png()), ("NPC_3.png", fixtures.sheet_png())], "contiguous"
        )
        self.assert_refused([("Thanks.txt", b"x")], "NPC_1.png")

    def test_zip_plan_skips_names_outside_the_allowlist(self):
        # NPC_16, NPC_01, nested NPC files and unknown emotes are extras, not copied
        entries = [
            ("NPC_1.png", fixtures.sheet_png()),
            ("NPC_16.png", fixtures.sheet_png()),
            ("NPC_01.png", fixtures.sheet_png()),
            ("pack/NPC_2.png", fixtures.sheet_png()),
            ("Emotes/Dance_emote_16x16.png", fixtures.emote_png()),
        ]
        plan = plan_for(entries)
        self.assertEqual([item["target"] for item in plan["files"]], ["NPC_1.png"])
        self.assertEqual(plan["skipped"], 4)

    def test_names_with_a_trailing_newline_are_not_allowlisted(self):
        # "$" in a regex also matches before a final newline; fullmatch does not
        self.assertIsNone(charpack.target_for("NPC_1.png\n"))
        self.assertIsNone(charpack.target_for("Emotes/Sad_emote_16x16.png\n"))
        self.assertEqual(charpack.target_for("NPC_1.png"), "NPC_1.png")
        with self.assertRaises(charpack.PackError) as ctx:
            charpack.plan_pack_install([charpack.PackEntry("NPC_1.png\n", 100, 0, 0)])
        self.assertIn("NPC_1.png", str(ctx.exception))

    def test_zip_plan_allows_missing_emotes(self):
        plan = plan_for(fixtures.full_pack_entries(characters=3, emotes=(), extras=False))
        self.assertEqual(plan["characters"], 3)
        self.assertEqual(plan["emotes"], 0)

    def test_zip_plan_refuses_an_empty_list(self):
        with self.assertRaises(charpack.PackError):
            charpack.plan_pack_install([])

    def test_refusal_never_repeats_the_entry_name(self):
        with self.assertRaises(charpack.PackError) as ctx:
            plan_for([("NPC_1.png", fixtures.sheet_png()), ("../\x1b[2Jevil", b"x")])
        self.assertNotIn("evil", str(ctx.exception))
        self.assertNotIn("\x1b", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
