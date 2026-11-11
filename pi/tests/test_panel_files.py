"""Tests for pi/panels.py: the declarative plugin panel file, its validator,
the closed format grammar (security done-bar HIGH-1) and the pure helpers the
renderer uses. Stdlib only; the TOML tests need Python 3.11+ (tomllib), which
CI and the Pi have, and are skipped on an older local Python."""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import panels  # noqa: E402

try:
    import tomllib  # noqa: F401
    HAVE_TOMLLIB = True
except ImportError:
    HAVE_TOMLLIB = False

needs_toml = unittest.skipUnless(HAVE_TOMLLIB, "tomllib needs Python 3.11+")

GOOD = b'''
schema = "deskpane.panel/1"
title = "Weather"
reads = ["weather.temp_c", "weather.rain_pct"]

[[element]]
kind = "text"
bind = "weather.temp_c"
format = "{weather.temp_c:.0f} C"
missing = "no data"
x = 24
y = 40
size = "huge"
color = "TEXT"

[[element.threshold]]
below = 0.0
color = "WARN"

[[element]]
kind = "bar"
bind = "weather.rain_pct"
min = 0.0
max = 100.0
x = 24
y = 200
w = 432
h = 14
color = "ACCENT"

[[element]]
kind = "ring"
bind = "weather.rain_pct"
min = 0
max = 100
cx = 360
cy = 100
r = 60
label = "Rain"

[[element]]
kind = "icon"
name = "temp"
x = 200
y = 40

[[element]]
kind = "text"
text = "Today"
x = 24
y = 120
'''


def load(text, plugin="weather"):
    return panels.load_panel(text.encode() if isinstance(text, str) else text, plugin)


def with_element(body, reads='["weather.temp_c"]'):
    """A one-element panel file around the given element body."""
    return ('schema = "deskpane.panel/1"\ntitle = "T"\nreads = %s\n\n[[element]]\n%s\n'
            % (reads, body))


TEXT_EL = 'kind = "text"\nbind = "weather.temp_c"\nformat = "%s"\nx = 0\ny = 0'


@needs_toml
class LoadPanelTest(unittest.TestCase):
    def test_the_example_loads(self):
        panel, reason = load(GOOD)
        self.assertIsNone(reason)
        self.assertEqual(panel["title"], "Weather")
        self.assertEqual(panel["reads"], ("weather.temp_c", "weather.rain_pct"))
        kinds = [element["kind"] for element in panel["elements"]]
        self.assertEqual(kinds, ["text", "bar", "ring", "icon", "text"])

    def test_defaults_are_filled_in(self):
        panel, _ = load(GOOD)
        icon = panel["elements"][3]
        self.assertEqual(icon["color"], "TEXT")
        self.assertEqual(icon["thresholds"], ())
        literal = panel["elements"][4]
        self.assertEqual(literal["size"], "mid")
        text = panel["elements"][0]
        self.assertEqual(text["format"], ("", "weather.temp_c", ".0f", " C"))
        self.assertEqual(text["thresholds"], ({"below": 0.0, "above": None, "color": "WARN", "icon": None},))

    def test_a_file_over_16_kb_is_refused_before_parsing(self):
        big = GOOD + b"#" * (panels.MAX_FILE_BYTES + 1 - len(GOOD))
        panel, reason = load(big)
        self.assertIsNone(panel)
        self.assertIn("16384", reason)

    def test_bad_bytes_bad_toml_and_deep_nesting_are_reasons_not_crashes(self):
        for text in (b"\xff\xfe", b"title = ", b"x = " + b"[" * 5000 + b"]" * 5000, b""):
            with self.subTest(text=text[:10]):
                panel, reason = load(text)
                self.assertIsNone(panel)
                self.assertIsInstance(reason, str)

    def test_the_schema_must_match(self):
        panel, reason = load(GOOD.replace(b"deskpane.panel/1", b"deskpane.panel/2"))
        self.assertIsNone(panel)
        self.assertIn("schema", reason)

    def test_unknown_keys_are_refused_without_echoing_them(self):
        for text in (GOOD + b'\nevil_key = 1\n',
                     with_element('kind = "icon"\nname = "up"\nx = 0\ny = 0\nonclick = "rm"')):
            with self.subTest(text=text[-20:]):
                panel, reason = load(text)
                self.assertIsNone(panel)
                self.assertIn("unknown key", reason)
                self.assertNotIn("evil", reason)
                self.assertNotIn("onclick", reason)

    def test_reads_must_be_this_plugins_own_values(self):
        # D4: a panel reads only its own plugin's metrics
        for reads in ('["other.temp_c"]', '["weather"]', '["weather.Temp"]', '[]', '"weather.temp_c"',
                      '["weather.temp_c", "weather.temp_c"]', '[1]'):
            with self.subTest(reads=reads):
                panel, reason = load(with_element('kind = "icon"\nname = "up"\nx = 0\ny = 0', reads))
                self.assertIsNone(panel)
                self.assertIn("reads", reason)

    def test_a_plugin_name_with_a_dash_reads_its_values(self):
        text = with_element(TEXT_EL.replace("weather.temp_c", "ups-2.load_pct") % "{ups-2.load_pct:%}",
                            '["ups-2.load_pct"]')
        panel, reason = load(text, plugin="ups-2")
        self.assertIsNone(reason)
        self.assertEqual(panel["elements"][0]["format"], ("", "ups-2.load_pct", "%", ""))

    def test_a_bind_must_be_a_declared_read(self):
        text = with_element('kind = "bar"\nbind = "weather.rain_pct"\nmin = 0\nmax = 1\nx = 0\ny = 0\nw = 9\nh = 9')
        panel, reason = load(text)
        self.assertIsNone(panel)
        self.assertIn("reads", reason)

    def test_at_most_32_elements_and_8_thresholds(self):
        icon = '[[element]]\nkind = "icon"\nname = "up"\nx = 0\ny = 0\n'
        head = 'schema = "deskpane.panel/1"\ntitle = "T"\nreads = ["weather.temp_c"]\n'
        panel, reason = load(head + icon * 33)
        self.assertIsNone(panel)
        self.assertIn("32", reason)
        self.assertIsNotNone(load(head + icon * 32)[0])

        threshold = '[[element.threshold]]\nabove = 1.0\ncolor = "CRIT"\n'
        panel, reason = load(head + icon + threshold * 9)
        self.assertIsNone(panel)
        self.assertIn("8", reason)
        self.assertIsNotNone(load(head + icon + threshold * 8)[0])

    def test_no_elements_is_refused(self):
        panel, reason = load('schema = "deskpane.panel/1"\ntitle = "T"\nreads = ["weather.temp_c"]\n')
        self.assertIsNone(panel)
        self.assertIn("element", reason)

    def test_literals_are_capped_and_have_no_control_or_format_characters(self):
        for title in ("", "x" * 49, "a\\u0007b", "a\\u202eb", "a\\nb", "a\\u200bb"):
            with self.subTest(title=title):
                panel, reason = load(GOOD.replace(b'title = "Weather"', ('title = "%s"' % title).encode()))
                self.assertIsNone(panel)
                self.assertIn("title", reason)
        self.assertIsNotNone(load(GOOD.replace(b'title = "Weather"', b'title = "' + b"x" * 48 + b'"'))[0])

    def test_unassigned_private_and_stacked_combining_characters_are_refused(self):
        # security review LOW-4: no noncharacters, private use or "zalgo" stacks
        for title in ("a\\uffffb", "a\\U0010ffffb", "a\\ue000b", "a" + "\\u0301" * 3):
            with self.subTest(title=title):
                panel, reason = load(GOOD.replace(b'title = "Weather"', ('title = "%s"' % title).encode()))
                self.assertIsNone(panel)
                self.assertIn("title", reason)
        # one or two accents on a letter are ordinary text
        self.assertIsNotNone(load(GOOD.replace(b'title = "Weather"', 'title = "Caf\\u00e9 e\\u0301\\u0302"'.encode()))[0])

    def test_at_most_8_big_or_huge_texts(self):
        # security review MEDIUM-1: big text is the costly element to draw
        head = 'schema = "deskpane.panel/1"\ntitle = "T"\nreads = ["weather.temp_c"]\n'
        big = '[[element]]\nkind = "text"\ntext = "a"\nx = 0\ny = 0\nsize = "%s"\n'
        panel, reason = load(head + (big % "huge") * 5 + (big % "big") * 4)
        self.assertIsNone(panel)
        self.assertIn("8", reason)
        self.assertIsNotNone(load(head + (big % "huge") * 4 + (big % "big") * 4 + (big % "mid") * 20)[0])

    def test_tokens_must_come_from_the_closed_sets(self):
        cases = [
            ('kind = "html"\nx = 0\ny = 0', "kind"),
            ('kind = "icon"\nname = "skull"\nx = 0\ny = 0', "name"),
            ('kind = "icon"\nname = "up"\nx = 0\ny = 0\ncolor = "#ff0000"', "color"),
            ('kind = "text"\ntext = "a"\nx = 0\ny = 0\nsize = "giant"', "size"),
        ]
        for body, word in cases:
            with self.subTest(body=body):
                panel, reason = load(with_element(body))
                self.assertIsNone(panel)
                self.assertIn(word, reason)

    def test_types_are_strict(self):
        for body in ('kind = "icon"\nname = "up"\nx = true\ny = 0',
                     'kind = "icon"\nname = "up"\nx = 1.5\ny = 0',
                     'kind = "icon"\nname = "up"\nx = "1"\ny = 0',
                     'kind = "bar"\nbind = "weather.temp_c"\nmin = "0"\nmax = 1\nx = 0\ny = 0\nw = 9\nh = 9',
                     'kind = "bar"\nbind = "weather.temp_c"\nmin = nan\nmax = 1\nx = 0\ny = 0\nw = 9\nh = 9',
                     'kind = "bar"\nbind = "weather.temp_c"\nmin = 0\nmax = inf\nx = 0\ny = 0\nw = 9\nh = 9',
                     'kind = "bar"\nbind = "weather.temp_c"\nmin = 0\nmax = 2e9\nx = 0\ny = 0\nw = 9\nh = 9',
                     'kind = "bar"\nbind = "weather.temp_c"\nmin = 5\nmax = 5\nx = 0\ny = 0\nw = 9\nh = 9'):
            with self.subTest(body=body):
                panel, reason = load(with_element(body))
                self.assertIsNone(panel, body)

    def test_required_keys_are_required(self):
        for body in ('kind = "icon"\nx = 0\ny = 0', 'kind = "text"\nx = 0\ny = 0',
                     'kind = "text"\ntext = "a"\nbind = "weather.temp_c"\nformat = "{weather.temp_c}"\nx = 0\ny = 0',
                     'kind = "text"\nbind = "weather.temp_c"\nx = 0\ny = 0',
                     'kind = "ring"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\ncx = 50\ncy = 50',
                     'kind = "icon"\nname = "up"\nx = 0'):
            with self.subTest(body=body):
                self.assertIsNone(load(with_element(body))[0], body)

    def test_geometry_stays_inside_the_480_by_256_content_rect(self):
        bad = ['kind = "text"\ntext = "a"\nx = -1\ny = 0',
               'kind = "text"\ntext = "a"\nx = 480\ny = 0',
               'kind = "text"\ntext = "a"\nx = 0\ny = 256',
               'kind = "icon"\nname = "up"\nx = 457\ny = 0',
               'kind = "icon"\nname = "up"\nx = 0\ny = 233',
               'kind = "bar"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\nx = 10\ny = 0\nw = 471\nh = 9',
               'kind = "bar"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\nx = 0\ny = 250\nw = 9\nh = 7',
               'kind = "bar"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\nx = 0\ny = 0\nw = 0\nh = 7',
               'kind = "ring"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\ncx = 50\ncy = 50\nr = 51',
               'kind = "ring"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\ncx = 440\ncy = 128\nr = 41',
               'kind = "ring"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\ncx = 100\ncy = 100\nr = 7']
        for body in bad:
            with self.subTest(body=body):
                panel, reason = load(with_element(body))
                self.assertIsNone(panel, body)
        good = ['kind = "icon"\nname = "up"\nx = 456\ny = 232',
                'kind = "bar"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\nx = 0\ny = 0\nw = 480\nh = 256',
                'kind = "ring"\nbind = "weather.temp_c"\nmin = 0\nmax = 1\ncx = 128\ncy = 128\nr = 128']
        for body in good:
            with self.subTest(body=body):
                self.assertIsNotNone(load(with_element(body))[0], body)

    def test_a_threshold_needs_one_comparison_and_a_change(self):
        icon = 'kind = "icon"\nname = "up"\nx = 0\ny = 0\n'
        for threshold in ('below = 1.0\nabove = 2.0\ncolor = "WARN"', 'color = "WARN"', 'below = 1.0',
                          'below = nan\ncolor = "WARN"', 'below = 1.0\ncolor = "PINK"',
                          'below = 1.0\nicon = "skull"', 'below = 1.0\ncolor = "WARN"\nextra = 1'):
            with self.subTest(threshold=threshold):
                text = with_element(icon + '\n[[element.threshold]]\n' + threshold)
                self.assertIsNone(load(text)[0], threshold)

    def test_only_an_icon_element_can_switch_icons(self):
        body = (TEXT_EL % "{weather.temp_c}") + '\n[[element.threshold]]\nbelow = 1.0\nicon = "up"'
        self.assertIsNone(load(with_element(body))[0])


@needs_toml
class FormatGrammarTest(unittest.TestCase):
    """HIGH-1: plugin text never reaches str.format; anything outside the
    closed grammar is refused at load."""

    def test_accepted_formats(self):
        cases = {
            "{weather.temp_c}": ("", "weather.temp_c", None, ""),
            "{weather.temp_c:d} C": ("", "weather.temp_c", "d", " C"),
            "now {weather.temp_c:.1f}": ("now ", "weather.temp_c", ".1f", ""),
            "{weather.temp_c:.2f}": ("", "weather.temp_c", ".2f", ""),
            "{weather.temp_c:.0f}": ("", "weather.temp_c", ".0f", ""),
            "rain {weather.temp_c:%}": ("rain ", "weather.temp_c", "%", ""),
        }
        for fmt, want in cases.items():
            with self.subTest(fmt=fmt):
                panel, reason = load(with_element(TEXT_EL % fmt))
                self.assertIsNone(reason, fmt)
                self.assertEqual(panel["elements"][0]["format"], want)

    def test_adversarial_formats_are_refused(self):
        adversarial = [
            "{weather.temp_c.__class__}",
            "{weather.temp_c.__class__.__init__.__globals__}",
            "{0}",
            "{}",
            "{{weather.temp_c}}",
            "{weather.temp_c!r}",
            "{weather.temp_c!s:.1f}",
            "{weather.temp_c:>{n}}",
            "{weather.temp_c:>10}",
            "{weather.temp_c:,}",
            "{weather.temp_c:.100f}",
            "{weather.temp_c[0]}",
            "{weather.temp_c} {weather.temp_c}",
            "{weather.rain_pct}",
            "{other.temp_c}",
            "{weather.temp_c",
            "weather.temp_c}",
            "}{",
            "{ weather.temp_c }",
            "%s",
            "no field at all",
            "{weather.temp_c:}",
        ]
        for fmt in adversarial:
            with self.subTest(fmt=fmt):
                panel, reason = load(with_element(TEXT_EL % fmt))
                self.assertIsNone(panel, fmt)
                self.assertIn("format", reason)

    def test_the_parser_alone_refuses_without_a_toml_file(self):
        self.assertEqual(panels.parse_format("{a.b}", "a.b"), (("", "a.b", None, ""), None))
        parsed, reason = panels.parse_format("{a.b.c}", "a.b")
        self.assertIsNone(parsed)
        self.assertIn("format", reason)


class FormatValueTest(unittest.TestCase):
    def test_the_spec_table(self):
        cases = [
            (12.4, None, "12.4"), (12.0, None, "12"), (-3.25, None, "-3.25"),
            (2.5, "d", "3"), (-2.5, "d", "-3"), (2.4, "d", "2"), (1e9, "d", "1000000000"),
            (12.46, ".0f", "12"), (12.46, ".1f", "12.5"), (12.456, ".2f", "12.46"),
            (40.0, "%", "40%"), (99.6, "%", "100%"), (-1e9, "%", "-1000000000%"),
        ]
        for value, spec, want in cases:
            with self.subTest(value=value, spec=spec):
                self.assertEqual(panels.format_value(value, spec), want)

    def test_output_is_bounded_for_every_spec_and_extreme(self):
        for spec in (None, "d", ".0f", ".1f", ".2f", "%"):
            for value in (1e9, -1e9, 1e-9, -0.0, 123456789.123456):
                with self.subTest(spec=spec, value=value):
                    self.assertLessEqual(len(panels.format_value(value, spec)), panels.MAX_VALUE_CHARS)

    def test_halves_and_negative_zero(self):
        # security review LOW-3: no float trap at 0.5, never a "-0"
        cases = [
            (0.49999999999999994, "d", "0"), (-0.49999999999999994, "d", "0"), (0.5, "d", "1"),
            (-0.3, "d", "0"), (-0.3, ".0f", "0"), (-0.001, ".2f", "0.00"), (-0.04, ".1f", "0.0"),
            (-0.4, "%", "0%"), (-0.6, "%", "-1%"), (-0.6, ".0f", "-1"), (-0.0, "d", "0"),
        ]
        for value, spec, want in cases:
            with self.subTest(value=value, spec=spec):
                self.assertEqual(panels.format_value(value, spec), want)

    def test_render_text_puts_the_value_between_the_literals(self):
        fmt = ("now ", "weather.temp_c", ".1f", " C")
        self.assertEqual(panels.render_format(fmt, 12.44), "now 12.4 C")


class ThresholdTest(unittest.TestCase):
    def element(self, thresholds):
        return {"color": "TEXT", "name": "up", "thresholds": tuple(thresholds)}

    def rule(self, below=None, above=None, color=None, icon=None):
        return {"below": below, "above": above, "color": color, "icon": icon}

    def test_first_matching_threshold_wins(self):
        element = self.element([self.rule(below=0.0, color="WARN"), self.rule(below=10.0, color="CRIT"),
                                self.rule(above=30.0, color="CRIT", icon="temp")])
        self.assertEqual(panels.pick_style(element, -5.0), ("WARN", "up"))
        self.assertEqual(panels.pick_style(element, 5.0), ("CRIT", "up"))
        self.assertEqual(panels.pick_style(element, 20.0), ("TEXT", "up"))
        self.assertEqual(panels.pick_style(element, 31.0), ("CRIT", "temp"))

    def test_edges_are_strict(self):
        element = self.element([self.rule(below=0.0, color="WARN"), self.rule(above=1.0, color="CRIT")])
        self.assertEqual(panels.pick_style(element, 0.0), ("TEXT", "up"))
        self.assertEqual(panels.pick_style(element, 1.0), ("TEXT", "up"))

    def test_a_missing_value_keeps_the_base_style(self):
        element = self.element([self.rule(below=0.0, color="WARN")])
        self.assertEqual(panels.pick_style(element, None), ("TEXT", "up"))

    def test_a_threshold_that_changes_only_the_icon_keeps_the_color(self):
        element = self.element([self.rule(above=1.0, icon="disk")])
        self.assertEqual(panels.pick_style(element, 2.0), ("TEXT", "disk"))


class FillFractionTest(unittest.TestCase):
    def test_clamped_to_zero_and_one(self):
        self.assertEqual(panels.fill_fraction(50.0, 0.0, 100.0), 0.5)
        self.assertEqual(panels.fill_fraction(-10.0, 0.0, 100.0), 0.0)
        self.assertEqual(panels.fill_fraction(1e9, 0.0, 100.0), 1.0)
        self.assertEqual(panels.fill_fraction(15.0, 10.0, 20.0), 0.5)
        self.assertEqual(panels.fill_fraction(None, 0.0, 100.0), 0.0)


class DrawBudgetTest(unittest.TestCase):
    """security review MEDIUM-1: a panel that is slow to draw three frames in
    a row is replaced by a placeholder (docs/SECURITY.md draw-cost budget)."""

    def test_three_slow_frames_in_a_row_trip_the_budget(self):
        slow = panels.DRAW_BUDGET_S + 0.01
        streak, tripped = panels.step_draw_budget(0, slow)
        self.assertEqual((streak, tripped), (1, False))
        streak, tripped = panels.step_draw_budget(streak, slow)
        self.assertEqual((streak, tripped), (2, False))
        streak, tripped = panels.step_draw_budget(streak, slow)
        self.assertEqual((streak, tripped), (3, True))

    def test_a_fast_frame_resets_the_streak(self):
        slow = panels.DRAW_BUDGET_S + 0.01
        streak, _ = panels.step_draw_budget(2, slow)
        streak, tripped = panels.step_draw_budget(0, 0.001)
        self.assertEqual((streak, tripped), (0, False))
        self.assertEqual(panels.step_draw_budget(2, 0.001), (0, False))


class PanelValuesTest(unittest.TestCase):
    PANEL = {"plugin": "weather", "reads": ("weather.temp_c", "weather.rain_pct")}

    def test_only_declared_reads_fresh_values(self):
        metrics = {"weather": {"values": {"temp_c": 12.4, "rain_pct": 40.0, "secret": 1.0},
                               "age_s": 5, "stale": False},
                   "other": {"values": {"temp_c": 99.0}, "age_s": 1, "stale": False}}
        self.assertEqual(panels.panel_values(self.PANEL, metrics),
                         {"weather.temp_c": 12.4, "weather.rain_pct": 40.0})

    def test_stale_or_absent_values_are_none(self):
        stale = {"weather": {"values": {"temp_c": 12.4, "rain_pct": 1.0}, "age_s": 9999, "stale": True}}
        self.assertEqual(panels.panel_values(self.PANEL, stale),
                         {"weather.temp_c": None, "weather.rain_pct": None})
        partial = {"weather": {"values": {"temp_c": 12.4}, "age_s": 1, "stale": False}}
        self.assertEqual(panels.panel_values(self.PANEL, partial),
                         {"weather.temp_c": 12.4, "weather.rain_pct": None})
        self.assertEqual(panels.panel_values(self.PANEL, {}),
                         {"weather.temp_c": None, "weather.rain_pct": None})


class LoadEnabledPanelsTest(unittest.TestCase):
    def setUp(self):
        import tempfile
        self.root = tempfile.mkdtemp()
        self.lines = []

    def write(self, name, data):
        folder = os.path.join(self.root, name, "panel")
        os.makedirs(folder)
        with open(os.path.join(folder, name + ".panel.toml"), "wb") as f:
            f.write(data)

    def test_panels_setting_is_read_like_the_daemon_does(self):
        self.assertEqual(panels.parse_panels_setting(" weather, ups-2,,weather , Bad, 1x"),
                         ["weather", "ups-2"])
        self.assertEqual(panels.parse_panels_setting(""), [])
        many = ",".join("p%d" % index for index in range(20))
        self.assertEqual(len(panels.parse_panels_setting(many)), panels.MAX_PANELS)

    @needs_toml
    def test_missing_and_bad_panels_are_skipped_with_one_line_each(self):
        self.write("weather", GOOD)
        self.write("broken", b"not toml [")
        loaded = panels.load_enabled_panels(["weather", "broken", "absent"], self.root, self.lines.append)
        self.assertEqual([panel["plugin"] for panel in loaded], ["weather"])
        self.assertEqual(len(self.lines), 2)
        self.assertTrue(any("broken" in line for line in self.lines))
        self.assertTrue(any("absent" in line for line in self.lines))

    def test_a_fifo_is_refused_without_hanging(self):
        # security review LOW-1: opening a named pipe must not block start-up
        folder = os.path.join(self.root, "weather", "panel")
        os.makedirs(folder)
        os.mkfifo(os.path.join(folder, "weather.panel.toml"))
        self.assertEqual(panels.load_enabled_panels(["weather"], self.root, self.lines.append), [])
        self.assertEqual(len(self.lines), 1)

    def test_a_symlinked_panel_file_is_refused(self):
        folder = os.path.join(self.root, "weather", "panel")
        os.makedirs(folder)
        target = os.path.join(self.root, "elsewhere.toml")
        with open(target, "wb") as f:
            f.write(GOOD)
        os.symlink(target, os.path.join(folder, "weather.panel.toml"))
        self.assertEqual(panels.load_enabled_panels(["weather"], self.root, self.lines.append), [])
        self.assertEqual(len(self.lines), 1)


if __name__ == "__main__":
    unittest.main()
