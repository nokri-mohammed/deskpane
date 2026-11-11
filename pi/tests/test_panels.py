"""Tests for the panel registry: the ring half (layout.PAGE_PROVIDERS) and
the draw half (dashboard.PANEL_DRAW) must stay in lockstep, so a new panel
kind cannot be added to one without the other.

Importing dashboard pulls in pygame, so the dummy video and audio drivers are
set first; the test is skipped where pygame is not installed. Run from the repo
root with:
    python3 -m unittest discover -s pi/tests
"""
import os
import sys
import unittest

# layout.py and dashboard.py sit one folder up
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import layout  # noqa: E402

# dashboard imports pygame; render to nothing so no display or audio is opened
os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
os.environ.setdefault("PYGAME_HIDE_SUPPORT_PROMPT", "1")

try:
    import dashboard  # noqa: E402
    HAVE_DASHBOARD = True
except Exception:
    HAVE_DASHBOARD = False


@unittest.skipUnless(HAVE_DASHBOARD, "dashboard (pygame) not importable here")
class PanelRegistryTests(unittest.TestCase):
    def test_draw_and_ring_halves_cover_the_same_kinds_in_order(self):
        """Every ring kind has a draw, and in the same order; no extra draws."""
        ring_kinds = [kind for kind, _enumerate in layout.PAGE_PROVIDERS]
        draw_kinds = list(dashboard.PANEL_DRAW.keys())
        self.assertEqual(ring_kinds, draw_kinds)

    def test_every_ring_page_has_a_registered_draw(self):
        """A built ring (devices and sessions present) names only known kinds."""
        ring = layout.page_ring(3, 12)
        for kind, _value in ring:
            self.assertIn(kind, dashboard.PANEL_DRAW)

    def test_ring_still_begins_overview_then_agents_then_devices(self):
        """The registry preserves the original ring order and values."""
        ring = layout.page_ring(2, 7)
        self.assertEqual(ring[0], ("overview", None))
        # 7 sessions over 5 per page is two agents pages, then the two devices
        self.assertEqual(ring[1], ("agents", (0, 5)))
        self.assertEqual(ring[2], ("agents", (5, 7)))
        self.assertEqual(ring[3], ("device", 0))
        self.assertEqual(ring[4], ("device", 1))


if __name__ == "__main__":
    unittest.main()
