"""Declarative plugin panels: the panel file format, its validator and the
pure helpers the renderer uses. Stdlib only, no pygame, so it is tested on
its own and the dashboard only draws what this module already checked.

A panel file is plugins/<name>/panel/<name>.panel.toml (docs/PLUGINS.md):
a title, the values it reads (its own plugin's metrics only) and up to 32
elements (text, ring, bar, icon) placed inside the 480 x 256 content rect
below deskpane's own page header.

Security done-bar HIGH-1 (docs/SECURITY.md): plugin text never reaches
str.format, format_map, the % operator, an f-string or eval. A `format` is
parsed here into (prefix, field, spec, suffix) by a closed grammar, and
format_value builds the number's text from deskpane's own spec table.
Everything outside the grammar is refused when the panel loads.

Written for Python 3.7 syntax (local preview) but tomllib needs 3.11+, which
the Pi (3.13) and CI have; without it no panel loads and the reason says so.
"""

import os
import re
import stat
import unicodedata

# the one panel schema this dashboard reads
SCHEMA = "deskpane.panel/1"

# caps (docs/SECURITY.md "Load-time and content limits")
MAX_FILE_BYTES = 16384
MAX_ELEMENTS = 32
MAX_THRESHOLDS = 8
MAX_LITERAL_CHARS = 48
MAX_READS = 16
# most plugin pages turned on (PANELS), the same as the daemon's plugin cap
MAX_PANELS = 16
# longest text format_value can return (-1000000000.00 is 14)
MAX_VALUE_CHARS = 16
# the bound every plugin number obeys (the contract's MAX_METRIC_ABS)
MAX_NUMBER_ABS = 1e9

# the content rect a panel draws in (the host draws the header and dots)
CONTENT_W = 480
CONTENT_H = 256
# a built-in icon is drawn inside a box this size from its (x, y)
ICON_BOX = 24
# ring radius limits
MIN_RING_R = 8
MAX_RING_R = 128

# the closed token sets
COLORS = ("TEXT", "SUBTEXT", "ACCENT", "WARN", "CRIT")
SIZES = ("huge", "big", "mid", "small", "tiny")
ICONS = ("up", "temp", "disk", "network")
SPECS = ("d", ".0f", ".1f", ".2f", "%")

# names, the same rules as deskpane_contract (Rust and Python)
PLUGIN_NAME = re.compile(r"[a-z][a-z0-9-]{0,31}")
METRIC_KEY = re.compile(r"[a-z][a-z0-9_]{0,31}")

# Unicode categories never allowed in a literal: control, format (bidi
# overrides, zero-width), surrogates, line/paragraph separators, unassigned
# and noncharacters, private use
BANNED_CATEGORIES = ("Cc", "Cf", "Cs", "Zl", "Zp", "Cn", "Co")
# most combining marks in a row (an accent or two, never a "zalgo" stack)
MAX_COMBINING_RUN = 2
# most text elements in the big sizes, the costly ones to draw
MAX_BIG_TEXTS = 8
BIG_SIZES = ("huge", "big")
# a panel slower than this to draw, this many frames in a row, is replaced
# by a placeholder (docs/SECURITY.md "a total draw-cost budget per frame")
DRAW_BUDGET_S = 0.1
DRAW_BUDGET_FRAMES = 3

# the keys each element kind accepts (anything else is refused)
REQUIRED_KEYS = {
    "text": ("x", "y"),
    "ring": ("bind", "min", "max", "cx", "cy", "r"),
    "bar": ("bind", "min", "max", "x", "y", "w", "h"),
    "icon": ("name", "x", "y"),
}
OPTIONAL_KEYS = {
    "text": ("text", "bind", "format", "missing", "size", "color", "threshold"),
    "ring": ("label", "color", "threshold"),
    "bar": ("color", "threshold"),
    "icon": ("color", "threshold"),
}
DEFAULT_COLOR = {"text": "TEXT", "ring": "ACCENT", "bar": "ACCENT", "icon": "TEXT"}
TOP_KEYS = ("schema", "title", "reads", "element")
THRESHOLD_KEYS = ("below", "above", "color", "icon")
DEFAULT_MISSING = "--"


class PanelError(Exception):
    """A rule the panel file breaks. The message never repeats plugin text
    except names that already passed their own pattern."""


# ---- small checks ----

def _is_int(value):
    return isinstance(value, int) and not isinstance(value, bool)


def _number(value, what):
    """value as a float inside the plugin number bound, or PanelError."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise PanelError("%s: must be a number" % what)
    try:
        number = float(value)
    except (OverflowError, ValueError):
        raise PanelError("%s: number too large" % what)
    if number != number or abs(number) > MAX_NUMBER_ABS:
        raise PanelError("%s: must be finite and at most 1e9 either way" % what)
    return number


def _int_in(value, low, high, what):
    """value as an int with low <= value <= high, or PanelError."""
    if not _is_int(value):
        raise PanelError("%s: must be a whole number" % what)
    if value < low or value > high:
        raise PanelError("%s: must be from %d to %d" % (what, low, high))
    return value


def _literal(value, what):
    """a short plain text the panel draws as is, or PanelError."""
    if not isinstance(value, str):
        raise PanelError("%s: must be text" % what)
    if len(value) == 0 or len(value) > MAX_LITERAL_CHARS:
        raise PanelError("%s: must be 1 to %d characters" % (what, MAX_LITERAL_CHARS))
    combining_run = 0
    for char in value:
        category = unicodedata.category(char)
        if category in BANNED_CATEGORIES:
            raise PanelError("%s: control, invisible, unassigned or private characters are not allowed" % what)
        if category in ("Mn", "Me"):
            combining_run += 1
            if combining_run > MAX_COMBINING_RUN:
                raise PanelError("%s: at most %d combining marks in a row" % (what, MAX_COMBINING_RUN))
        else:
            combining_run = 0
    return value


def _token(value, allowed, what):
    if not isinstance(value, str) or value not in allowed:
        raise PanelError("%s: must be one of %s" % (what, ", ".join(allowed)))
    return value


def _only_keys(table, allowed, what):
    for key in table:
        if key not in allowed:
            # never echo the key: it is plugin text that matched no rule
            raise PanelError("%s: unknown key" % what)


def _is_read_name(name, plugin):
    """True for "<plugin>.<key>" of this plugin with a valid key."""
    if not isinstance(name, str):
        return False
    head, dot, key = name.partition(".")
    return dot == "." and head == plugin and METRIC_KEY.fullmatch(key) is not None


# ---- the format grammar (HIGH-1) ----

def parse_format(text, bind):
    """Parses a text element's `format` with the closed grammar: literal text
    with exactly one {field} or {field:spec}, where field is the element's
    own bind and spec is one of SPECS. No escapes, no conversions (!r), no
    attribute or index access, no nested or second field.

    returns: ((prefix, field, spec or None, suffix), None) or (None, reason)
    """
    if not isinstance(text, str):
        return None, "format: must be text"
    opens = [index for index, char in enumerate(text) if char == "{"]
    closes = [index for index, char in enumerate(text) if char == "}"]
    if len(opens) != 1 or len(closes) != 1 or closes[0] < opens[0]:
        return None, "format: needs exactly one {field} and no other braces"
    inner = text[opens[0] + 1:closes[0]]
    field, colon, spec = inner.partition(":")
    if field != bind:
        return None, "format: the field must be the element's bind"
    if colon == ":" and spec not in SPECS:
        return None, "format: the spec must be one of " + ", ".join(SPECS)
    prefix = text[:opens[0]]
    suffix = text[closes[0] + 1:]
    if colon == ":":
        return (prefix, field, spec, suffix), None
    return (prefix, field, None, suffix), None


def format_value(value, spec):
    """deskpane's own text for a plugin number (never str.format on plugin
    text). value is already inside the 1e9 bound.

    None: up to 2 decimals, trailing zeros dropped (12.4, 12, -3.25)
    "d": a whole number, halves rounded away from zero (2.5 -> 3, -2.5 -> -3)
    ".0f", ".1f", ".2f": that many decimals
    "%": the value as a whole percent, not multiplied (40 -> "40%")
    """
    if spec == "d":
        # int() drops the fraction exactly; no floor(x + 0.5) float trap
        magnitude = abs(value)
        whole = int(magnitude)
        if magnitude - whole >= 0.5:
            whole += 1
        if value < 0 and whole != 0:
            return "-" + str(whole)
        return str(whole)
    if spec in (".0f", ".1f", ".2f"):
        # spec is one of deskpane's own three constants, never plugin text
        return _no_negative_zero(format(value, spec))
    if spec == "%":
        return _no_negative_zero(format(value, ".0f")) + "%"
    text = format(value, ".2f").rstrip("0").rstrip(".")
    return _no_negative_zero(text) or "0"


def _no_negative_zero(text):
    """A rounded number that came out as -0, -0.0 or -0.00 without its sign."""
    if text.startswith("-") and text.strip("-0.") == "":
        return text[1:]
    return text


def render_format(parsed, value):
    """The full text of a parsed format for one value."""
    prefix, _field, spec, suffix = parsed
    return prefix + format_value(value, spec) + suffix


# ---- the validator ----

def _check_thresholds(raw, kind, what):
    if raw is None:
        return ()
    if not isinstance(raw, list):
        raise PanelError("%s: threshold must be a list of tables" % what)
    if len(raw) > MAX_THRESHOLDS:
        raise PanelError("%s: at most %d thresholds" % (what, MAX_THRESHOLDS))
    rules = []
    for index, table in enumerate(raw, start=1):
        label = "%s threshold %d" % (what, index)
        if not isinstance(table, dict):
            raise PanelError("%s: must be a table" % label)
        _only_keys(table, THRESHOLD_KEYS, label)
        has_below = "below" in table
        has_above = "above" in table
        if has_below == has_above:
            raise PanelError("%s: needs exactly one of below or above" % label)
        if "color" not in table and "icon" not in table:
            raise PanelError("%s: needs a color or an icon" % label)
        if "icon" in table and kind != "icon":
            raise PanelError("%s: only an icon element can switch its icon" % label)
        rule = {"below": None, "above": None, "color": None, "icon": None}
        if has_below:
            rule["below"] = _number(table["below"], label + " below")
        else:
            rule["above"] = _number(table["above"], label + " above")
        if "color" in table:
            rule["color"] = _token(table["color"], COLORS, label + " color")
        if "icon" in table:
            rule["icon"] = _token(table["icon"], ICONS, label + " icon")
        rules.append(rule)
    return tuple(rules)


def _check_bind(table, reads, what):
    bind = table["bind"]
    if bind not in reads:
        raise PanelError("%s: bind must be one of reads" % what)
    return bind


def _check_range(table, what):
    low = _number(table["min"], what + " min")
    high = _number(table["max"], what + " max")
    if not low < high:
        raise PanelError("%s: min must be below max" % what)
    return low, high


def _check_element(table, index, reads):
    what = "element %d" % index
    if not isinstance(table, dict):
        raise PanelError("%s: must be a table" % what)
    kind = _token(table.get("kind"), tuple(REQUIRED_KEYS), what + " kind")
    allowed = ("kind",) + REQUIRED_KEYS[kind] + OPTIONAL_KEYS[kind]
    _only_keys(table, allowed, what)
    for key in REQUIRED_KEYS[kind]:
        if key not in table:
            raise PanelError("%s: %s is required" % (what, key))

    element = {
        "kind": kind,
        "color": _token(table.get("color", DEFAULT_COLOR[kind]), COLORS, what + " color"),
        "thresholds": _check_thresholds(table.get("threshold"), kind, what),
    }

    if kind == "text":
        element["x"] = _int_in(table["x"], 0, CONTENT_W - 1, what + " x")
        element["y"] = _int_in(table["y"], 0, CONTENT_H - 1, what + " y")
        element["size"] = _token(table.get("size", "mid"), SIZES, what + " size")
        has_text = "text" in table
        has_bind = "bind" in table
        if has_text == has_bind:
            raise PanelError("%s: needs either text or bind + format" % what)
        if has_text:
            if "format" in table or "missing" in table:
                raise PanelError("%s: format and missing go with bind, not text" % what)
            element["text"] = _literal(table["text"], what + " text")
            element["bind"] = None
            element["format"] = None
            element["missing"] = None
        else:
            if "format" not in table:
                raise PanelError("%s: format is required with bind" % what)
            element["text"] = None
            element["bind"] = _check_bind(table, reads, what)
            fmt = _literal(table["format"], what + " format")
            parsed, reason = parse_format(fmt, element["bind"])
            if parsed is None:
                raise PanelError("%s: %s" % (what, reason))
            element["format"] = parsed
            element["missing"] = _literal(table.get("missing", DEFAULT_MISSING), what + " missing")

    elif kind == "ring":
        element["bind"] = _check_bind(table, reads, what)
        element["min"], element["max"] = _check_range(table, what)
        radius = _int_in(table["r"], MIN_RING_R, MAX_RING_R, what + " r")
        element["r"] = radius
        element["cx"] = _int_in(table["cx"], radius, CONTENT_W - radius, what + " cx")
        element["cy"] = _int_in(table["cy"], radius, CONTENT_H - radius, what + " cy")
        element["label"] = None
        if "label" in table:
            element["label"] = _literal(table["label"], what + " label")

    elif kind == "bar":
        element["bind"] = _check_bind(table, reads, what)
        element["min"], element["max"] = _check_range(table, what)
        element["x"] = _int_in(table["x"], 0, CONTENT_W - 1, what + " x")
        element["y"] = _int_in(table["y"], 0, CONTENT_H - 1, what + " y")
        element["w"] = _int_in(table["w"], 1, CONTENT_W - element["x"], what + " w")
        element["h"] = _int_in(table["h"], 1, CONTENT_H - element["y"], what + " h")

    else:
        element["name"] = _token(table["name"], ICONS, what + " name")
        element["x"] = _int_in(table["x"], 0, CONTENT_W - ICON_BOX, what + " x")
        element["y"] = _int_in(table["y"], 0, CONTENT_H - ICON_BOX, what + " y")

    return element


def _check_panel(data, plugin):
    if not isinstance(data, dict):
        raise PanelError("the file must be a TOML table")
    _only_keys(data, TOP_KEYS, "panel")
    if data.get("schema") != SCHEMA:
        raise PanelError("schema: must be " + SCHEMA)
    title = _literal(data.get("title"), "title")

    raw_reads = data.get("reads")
    if not isinstance(raw_reads, list) or len(raw_reads) == 0 or len(raw_reads) > MAX_READS:
        raise PanelError("reads: must be a list of 1 to %d names" % MAX_READS)
    for name in raw_reads:
        if not _is_read_name(name, plugin):
            raise PanelError("reads: each must be \"%s.<key>\" (its own plugin's values only)" % plugin)
    if len(set(raw_reads)) != len(raw_reads):
        raise PanelError("reads: a name is listed twice")
    reads = tuple(raw_reads)

    raw_elements = data.get("element")
    if not isinstance(raw_elements, list) or len(raw_elements) == 0:
        raise PanelError("needs at least one element")
    if len(raw_elements) > MAX_ELEMENTS:
        raise PanelError("at most %d elements" % MAX_ELEMENTS)
    elements = []
    for index, table in enumerate(raw_elements, start=1):
        elements.append(_check_element(table, index, reads))
    big_texts = [element for element in elements if element["kind"] == "text" and element["size"] in BIG_SIZES]
    if len(big_texts) > MAX_BIG_TEXTS:
        raise PanelError("at most %d text elements in the huge or big size" % MAX_BIG_TEXTS)

    return {"plugin": plugin, "title": title, "reads": reads, "elements": elements}


def load_panel(data, plugin):
    """Checks and loads one panel file.

    data: the file's bytes (the size cap applies before parsing)
    plugin: the plugin name (from PANELS, already a valid name)
    returns: (panel, None) or (None, reason). Never raises.
    """
    if len(data) > MAX_FILE_BYTES:
        return None, "the file is larger than %d bytes" % MAX_FILE_BYTES
    try:
        import tomllib
    except ImportError:
        return None, "reading panel files needs Python 3.11 or newer (tomllib)"
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return None, "the file is not UTF-8"
    try:
        parsed = tomllib.loads(text)
    except (tomllib.TOMLDecodeError, RecursionError, ValueError):
        return None, "the file is not valid TOML"
    try:
        return _check_panel(parsed, plugin), None
    except PanelError as error:
        return None, str(error)


# ---- what the renderer uses each frame ----

def pick_style(element, value):
    """(color, icon name) for an element and its bound value: the first
    threshold whose comparison holds changes them; a missing value keeps the
    element's own. Comparisons are strict (below 0 does not match 0)."""
    color = element["color"]
    icon = element.get("name")
    if value is None:
        return color, icon
    for rule in element["thresholds"]:
        if rule["below"] is not None:
            matched = value < rule["below"]
        else:
            matched = value > rule["above"]
        if matched:
            if rule["color"] is not None:
                color = rule["color"]
            if rule["icon"] is not None:
                icon = rule["icon"]
            return color, icon
    return color, icon


def step_draw_budget(streak, elapsed_s):
    """Counts slow frames in a row for one panel.

    streak: slow frames in a row so far
    elapsed_s: how long this frame's draw took
    returns: (new streak, True once the streak reaches DRAW_BUDGET_FRAMES)
    """
    if elapsed_s <= DRAW_BUDGET_S:
        return 0, False
    streak += 1
    return streak, streak >= DRAW_BUDGET_FRAMES


def fill_fraction(value, low, high):
    """How full a ring or bar is, 0.0 to 1.0 (0.0 for a missing value)."""
    if value is None:
        return 0.0
    fraction = (value - low) / (high - low)
    return min(1.0, max(0.0, fraction))


def panel_values(panel, metrics):
    """The panel's declared reads and nothing else: "<plugin>.<key>" -> the
    number, or None when it is missing or the plugin's data is stale.

    metrics: deskpane_contract.parse_metrics output
    """
    entry = metrics.get(panel["plugin"])
    values = {}
    for name in panel["reads"]:
        values[name] = None
        if entry is None or entry.get("stale", True):
            continue
        key = name.partition(".")[2]
        number = entry.get("values", {}).get(key)
        if isinstance(number, float):
            values[name] = number
    return values


# ---- loading the turned-on panels ----

def parse_panels_setting(text):
    """The PANELS setting as a list of plugin names in its own order: spaces
    and empty items ignored, a name listed twice counts once, items that are
    not plugin names skipped, at most MAX_PANELS (the daemon reads it the
    same way)."""
    names = []
    for item in text.split(","):
        name = item.strip()
        if not name or name in names or PLUGIN_NAME.fullmatch(name) is None:
            continue
        if len(names) >= MAX_PANELS:
            break
        names.append(name)
    return names


def _read_panel_file(path):
    """The bytes of a regular, non-symlinked file, read with the size cap
    (one byte over, so load_panel sees the file is too big)."""
    # O_NONBLOCK: opening a named pipe returns at once (and fstat refuses it)
    # instead of blocking the dashboard's start
    flags = (os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
             | getattr(os, "O_NOCTTY", 0))
    handle = os.open(path, flags)
    try:
        if not stat.S_ISREG(os.fstat(handle).st_mode):
            raise OSError(0, "not a regular file")
        chunks = []
        size = 0
        while size <= MAX_FILE_BYTES:
            chunk = os.read(handle, MAX_FILE_BYTES + 1 - size)
            if not chunk:
                break
            chunks.append(chunk)
            size += len(chunk)
        return b"".join(chunks)
    finally:
        os.close(handle)


def load_enabled_panels(names, root, log):
    """Loads <root>/<name>/panel/<name>.panel.toml for each turned-on name.
    A missing, symlinked or invalid panel is skipped with one log line, as a
    bad character pack is; it never stops the dashboard.

    names: parse_panels_setting output
    root: the plugins folder
    log: called with one line per skipped panel
    returns: the loaded panels, in the order of names
    """
    loaded = []
    for name in names:
        path = os.path.join(root, name, "panel", name + ".panel.toml")
        try:
            data = _read_panel_file(path)
        except OSError as error:
            log("dashboard: panel %s not loaded: cannot read %s (%s)" % (name, path, error.strerror or error))
            continue
        panel, reason = load_panel(data, name)
        if panel is None:
            log("dashboard: panel %s not loaded: %s" % (name, reason))
            continue
        loaded.append(panel)
    return loaded
