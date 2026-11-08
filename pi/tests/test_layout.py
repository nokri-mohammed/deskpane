"""Tests for pi/layout.py: the version file and the overview page maths.
Plain unittest, no pygame needed. Run from the repo root with:
    python3 -m unittest discover -s pi/tests
"""
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


if __name__ == "__main__":
    unittest.main()
