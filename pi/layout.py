"""Pure helpers for the dashboard screen: the installed version, where
the overview page puts things, the battery icon (T-064), and the agents page (sorting, styles, rows,
the overview line, the focus jump and the socket line shape). No pygame
import, so tests run anywhere.
"""
import os
import re

# the shared wire contract (the socket envelope and its parser); re-exported so
# `layout.ENVELOPE_VERSION`, `layout.dict_items` and `layout.parse_socket_line`
# keep working for the dashboard and the tests
from deskpane_contract import ENVELOPE_VERSION, dict_items, parse_socket_line  # noqa: F401

# the same version pattern install.sh accepts for the bundle's VERSION file
VERSION_PATTERN = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$")
MAX_VERSION_CHARS = 32
# shown when there is no usable VERSION file (running from the repo)
DEV_VERSION = "dev"

# vertical space between the clock and the date, and the date and the summary
GAP_CLOCK_DATE = 12
GAP_DATE_SUMMARY = 12
# between the device summary and the agents line under it
GAP_SUMMARY_AGENTS = 8
# the version text sits this far from the left and bottom edges
VERSION_MARGIN = 8


def read_version(dashboard_dir):
    """Returns the installed version, e.g. "0.1.0-beta.3", from the VERSION
    file one folder above the dashboard (install.sh puts it in
    /opt/deskpane/releases/<v>/ next to dashboard/). Returns "dev" when the
    file is missing, unreadable, too long or not a version."""
    path = os.path.join(dashboard_dir, os.pardir, "VERSION")
    try:
        with open(path, encoding="utf-8") as f:
            # read a little past the cap, so a huge file is never loaded whole
            text = f.read(MAX_VERSION_CHARS + 16)
    except (OSError, UnicodeDecodeError):
        return DEV_VERSION

    version = text.strip()
    if len(version) > MAX_VERSION_CHARS:
        return DEV_VERSION
    # fullmatch, so a trailing newline or second line is not accepted
    if not VERSION_PATTERN.fullmatch(version):
        return DEV_VERSION
    return version


def version_text(version):
    """The text drawn on screen: "v0.1.0-beta.3", or "dev" as it is."""
    if version == DEV_VERSION:
        return DEV_VERSION
    return "v" + version


def overview_layout(width, height, clock_h, date_h, summary_h, agents_h=0):
    """Top y of the clock, date and summary lines on the overview page, as a
    dict. The lines form one block centred vertically, so a different
    clock font size moves the block instead of breaking it.

    width, height: screen size in pixels (width is unused today, kept so
                   callers pass the full screen)
    clock_h, date_h, summary_h: rendered heights of the three lines
    agents_h: height of the agents line under the summary, 0 when there are
              no sessions (then "agents_y" is left out)
    """
    block_h = clock_h + GAP_CLOCK_DATE + date_h + GAP_DATE_SUMMARY + summary_h
    if agents_h > 0:
        block_h += GAP_SUMMARY_AGENTS + agents_h
    clock_y = max(0, (height - block_h) // 2)
    date_y = clock_y + clock_h + GAP_CLOCK_DATE
    summary_y = date_y + date_h + GAP_DATE_SUMMARY
    result = {
        "clock_y": clock_y,
        "date_y": date_y,
        "summary_y": summary_y,
    }
    if agents_h > 0:
        result["agents_y"] = summary_y + summary_h + GAP_SUMMARY_AGENTS
    return result


def version_position(height, version_h):
    """Top-left (x, y) of the version text: bottom left corner, 8 px in. The
    bottom centre has the page dots and the bottom right the network icon."""
    return VERSION_MARGIN, height - VERSION_MARGIN - version_h


# ---------------------------------------------------------------------------
# Battery icon (T-064, docs/plans/2026-11-10-battery-agent-details-preview.md)
# ---------------------------------------------------------------------------

# under this percent the icon and its text turn red
BATTERY_LOW_PCT = 20
# the outline, the nub on its right, and the whole icon width
BATTERY_BODY_W = 22
BATTERY_BODY_H = 11
BATTERY_NUB_W = 2
BATTERY_NUB_H = 5
BATTERY_W = BATTERY_BODY_W + BATTERY_NUB_W
# the fill sits this far inside the outline
BATTERY_INSET = 2
# between the percent text and the icon, and from the screen edges
BATTERY_TEXT_GAP = 4
BATTERY_MARGIN = 8
# the charging bolt left of the percent text
BOLT_W = 7
BOLT_GAP = 2


def is_percent(value):
    """True for an int or float (not a bool) that is finite and 0 to 100."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    # NaN fails both comparisons, infinity the range
    return 0 <= value <= 100


def battery_fields(device):
    """(pct, charging) from one device of the socket line, or None when it
    has no usable battery_pct. charging is True only for a real true."""
    if not isinstance(device, dict):
        return None
    pct = device.get("battery_pct")
    if not is_percent(pct):
        return None
    return float(pct), device.get("charging") is True


def local_battery(devices):
    """The battery of the Pi itself for the overview: the one online device
    with local: true and battery fields. None when there is none, or more
    than one (then it is not clear which is this Pi)."""
    found = []
    for device in devices:
        if not isinstance(device, dict):
            continue
        if device.get("local") is not True or device.get("online") is not True:
            continue
        fields = battery_fields(device)
        if fields is not None:
            found.append(fields)
    if len(found) != 1:
        return None
    return found[0]


def battery_style(pct, charging):
    """Text, low flag and colour name for the icon. The text rounds down, so
    19.9 shows "19%" in red and never a red "20%"."""
    low = pct < BATTERY_LOW_PCT
    return {
        "text": "%d%%" % int(pct),
        "low": low,
        "charging": bool(charging),
        "color": "red" if low else "text",
    }


def battery_geometry(x, y, pct):
    """Shapes of the icon with its top-left corner at (x, y), as rects
    (x, y, w, h). fill is None at 0 % and at least 1 px wide above it."""
    inner_w = BATTERY_BODY_W - 2 * BATTERY_INSET
    inner_h = BATTERY_BODY_H - 2 * BATTERY_INSET
    fill_w = int(round(inner_w * pct / 100.0))
    if pct > 0:
        fill_w = max(1, fill_w)
    fill = None
    if fill_w > 0:
        fill = (x + BATTERY_INSET, y + BATTERY_INSET, fill_w, inner_h)
    return {
        "body": (x, y, BATTERY_BODY_W, BATTERY_BODY_H),
        "nub": (x + BATTERY_BODY_W, y + (BATTERY_BODY_H - BATTERY_NUB_H) // 2, BATTERY_NUB_W, BATTERY_NUB_H),
        "fill": fill,
    }


def bolt_points(x, y):
    """The charging mark: a 6-point lightning polygon in a BOLT_W x
    BATTERY_BODY_H box with its top-left corner at (x, y), drawn as a shape
    because the font has no lightning glyph. The two shoulders overlap by
    two rows, so the middle never breaks at this size."""
    return [
        (x + 5, y),
        (x, y + 6),
        (x + 3, y + 6),
        (x + 1, y + 10),
        (x + 6, y + 4),
        (x + 3, y + 4),
    ]


def battery_widget_layout(right, top, text_w, text_h, charging=False):
    """Where the parts go, right to left: the icon ends at right, the
    percent text sits left of it and, when charging, the bolt left of the
    text; all centred on one line that starts at top. Returns {"icon":
    (x, y), "text": (x, y), "bolt": (x, y) or None, "left": x of the whole
    widget}."""
    line_h = max(text_h, BATTERY_BODY_H)
    icon_x = right - BATTERY_W
    icon_y = top + (line_h - BATTERY_BODY_H) // 2
    text_x = icon_x - BATTERY_TEXT_GAP - text_w
    text_y = top + (line_h - text_h) // 2
    bolt = None
    left = text_x
    if charging:
        bolt = (text_x - BOLT_GAP - BOLT_W, icon_y)
        left = bolt[0]
    return {"icon": (icon_x, icon_y), "text": (text_x, text_y), "bolt": bolt, "left": left}


# the device page hostname ends at least this far left of the battery (or clock)
HOST_CLEAR_GAP = 8


def host_max_w(host_x, clear_x):
    """The widest the device page hostname may be: it starts at host_x and
    ends HOST_CLEAR_GAP left of clear_x (the battery widget's left, or the
    clock's without a battery). Never negative; fit_text cuts to it."""
    return max(0, clear_x - HOST_CLEAR_GAP - host_x)


# ---------------------------------------------------------------------------
# Agents page (section 6 of docs/plans/2026-11-08-agent-status-view.md)
# ---------------------------------------------------------------------------

# a done session dims and sorts last after 30 minutes
DONE_DIM_AFTER_S = 30 * 60
# rows per agents page, and the tallest a row gets
AGENTS_PER_PAGE = 5
MAX_ROW_H = 64
# the focus jump: how long it lasts, and how recent a tap blocks it
FOCUS_FOR_S = 30
FOCUS_TAP_QUIET_S = 10
ELLIPSIS = "\u2026"
DOT = "\u00b7"

# sort groups, top to bottom; unknown states go after all of them
SORT_GROUP = {"needs_you": 0, "error": 1, "working": 2, "idle": 3, "done": 4}
UNKNOWN_GROUP = 6
DIM_DONE_GROUP = 5

# colour name, icon name, word per state; dashboard.py maps names to RGB
STATE_STYLE = {
    "needs_you": ("amber", "alert", "NEEDS YOU"),
    "working": ("blue", "pulse", "WORKING"),
    "done": ("green", "check", "DONE"),
    "error": ("red", "cross", "ERROR"),
    "idle": ("grey", "circle", "READY"),
    "ended": ("grey", "circle", "ENDED"),
}

# overview line parts, in order: state, word after the count
SUMMARY_PARTS = (
    ("needs_you", "needs you"),
    ("error", "error"),
    ("working", "working"),
    ("done", "done"),
    ("idle", "ready"),
)


def seconds_of(value):
    """A non-negative number of seconds from a session field, else 0."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return 0
    return max(0, value)


def text_of(value):
    """A session text field as a string; anything else is empty."""
    if isinstance(value, str):
        return value
    return ""


def is_dim_done(session):
    """True for a done session older than DONE_DIM_AFTER_S."""
    return session.get("state") == "done" and seconds_of(session.get("state_s")) > DONE_DIM_AFTER_S


def sort_agents(sessions):
    """Sessions in screen order: needs_you (longest waiting first), error
    (most recent first), working, idle, done (most recent first), dimmed
    done last. Ties go by project, then sid, so rows do not jump."""

    def key(session):
        state = session.get("state")
        state_s = seconds_of(session.get("state_s"))
        group = SORT_GROUP.get(state, UNKNOWN_GROUP)
        if is_dim_done(session):
            group = DIM_DONE_GROUP
        # needs_you: longest waiting first; error and done: most recent first
        if state == "needs_you":
            age = -state_s
        elif state in ("error", "done"):
            age = state_s
        else:
            age = 0
        return group, age, text_of(session.get("project")), text_of(session.get("sid"))

    return sorted(sessions, key=key)


def agent_style(state, quiet, state_s):
    """How a row looks: {"color", "icon", "word", "dim", "fill"}. Colour is
    never the only signal; every state also has an icon and a word."""
    color, icon, word = STATE_STYLE.get(state, ("grey", "circle", "?"))
    dim = False
    if state == "working" and quiet:
        color, icon, word = "greyblue", "question", "QUIET"
    if state == "done" and seconds_of(state_s) > DONE_DIM_AFTER_S:
        dim = True
    return {"color": color, "icon": icon, "word": word, "dim": dim, "fill": state == "needs_you"}


def agent_pages(count, per_page=AGENTS_PER_PAGE):
    """(start, end) slices of the sorted sessions, one per agents page."""
    return [(start, min(start + per_page, count)) for start in range(0, max(0, count), per_page)]


# The page providers, in ring order. Each is (kind, enumerate) where enumerate
# (device_count, session_count) returns that panel's page values: the overview
# is always one page (value None); the agents panel is one page per slice of
# sessions (none without sessions); the device panel is one page per device.
# This is the pure half of the panel registry: the ring "comes from" this list,
# and dashboard.py registers a draw function per kind in the same order. Adding
# a panel kind later is one entry here plus its draw, not an edit to page_ring.
PAGE_PROVIDERS = (
    ("overview", lambda device_count, session_count: [None]),
    ("agents", lambda device_count, session_count: agent_pages(session_count)),
    ("device", lambda device_count, session_count: list(range(device_count))),
)


def page_ring(device_count, session_count):
    """The pages a tap cycles through: the overview, then the agents pages
    (none without sessions), then one page per device. Built from
    PAGE_PROVIDERS, in registration order."""
    ring = []
    for kind, enumerate_pages in PAGE_PROVIDERS:
        for value in enumerate_pages(device_count, session_count):
            ring.append((kind, value))
    return ring


def agent_rows_layout(width, height, n_rows, header_h, footer_h):
    """(x, y, w, h) per row between the header and the page dots. One
    session gets one card over the whole space; 2 to 5 get rows of at most
    MAX_ROW_H px (64 px for up to 4 rows, 51 px for 5 on 480 x 320)."""
    if n_rows <= 0:
        return []
    avail = max(0, height - header_h - footer_h)
    if n_rows == 1:
        return [(0, header_h, width, avail)]
    row_h = min(MAX_ROW_H, avail // n_rows)
    return [(0, header_h + i * row_h, width, row_h) for i in range(n_rows)]


def fmt_elapsed(seconds):
    """Short time for a row: 45s, 12m, 3h, 2d."""
    seconds = int(seconds_of(seconds))
    if seconds < 60:
        return "%ds" % seconds
    if seconds < 3600:
        return "%dm" % (seconds // 60)
    if seconds < 86400:
        return "%dh" % (seconds // 3600)
    return "%dd" % (seconds // 86400)


def agent_title(session):
    """Line 1 of a row: the project, or "?" when it is hidden."""
    return text_of(session.get("project")) or "?"


def silent_seconds(session):
    """Seconds since the daemon last heard from a session: silent_s, or
    state_s from a daemon that does not send silent_s (or sends junk)."""
    silent = session.get("silent_s")
    if isinstance(silent, bool) or not isinstance(silent, (int, float)):
        return seconds_of(session.get("state_s"))
    return seconds_of(silent)


def agent_line2(session):
    """Line 2 of a row: machine label (or device id), a dot, then what it
    is doing; a quiet session says how long there has been no news
    (silent_s, else state_s)."""
    who = text_of(session.get("label")) or text_of(session.get("device_id"))
    if session.get("state") == "working" and session.get("quiet") is True:
        doing = "no news for " + fmt_elapsed(silent_seconds(session))
    else:
        doing = text_of(session.get("doing"))
    if who and doing:
        return "%s %s %s" % (who, DOT, doing)
    return who or doing


def agent_summary(sessions):
    """The overview line parts as (state, text), zero counts left out, or
    None when there are no sessions. Ghosts (walking off) do not count."""
    counts = {}
    for session in sessions:
        if session.get("ghost"):
            continue
        state = session.get("state")
        counts[state] = counts.get(state, 0) + 1
    parts = []
    for state, word in SUMMARY_PARTS:
        if counts.get(state, 0) > 0:
            parts.append((state, "%d %s" % (counts[state], word)))
    if not parts:
        return None
    return parts


def fit_text(text, max_w, measure):
    """The text, cut with an ellipsis so measure(result) <= max_w.
    measure is injected (a font's width in the dashboard, a fake in tests)."""
    if not text:
        return ""
    if measure(text) <= max_w:
        return text
    # drop characters from the end until the text plus the ellipsis fits
    for end in range(len(text) - 1, 0, -1):
        candidate = text[:end].rstrip() + ELLIPSIS
        if measure(candidate) <= max_w:
            return candidate
    return ""


def should_focus_agents(prev_states, new_states, last_tap, now):
    """True when a session newly enters needs_you (also a new session that
    arrives waiting) and the screen was not tapped in the last 10 s.
    error never jumps: nothing waits on a click. prev_states is None on the
    first line after start, which never jumps."""
    if prev_states is None:
        return False
    if last_tap is not None and now - last_tap < FOCUS_TAP_QUIET_S:
        return False
    for sid, state in new_states.items():
        if state == "needs_you" and prev_states.get(sid) != "needs_you":
            return True
    return False


# the socket envelope shape and its parser now live in the shared Python
# contract module (the mirror of the deskpane_contract Rust crate). Re-exported
# here so every `layout.parse_socket_line` / `layout.ENVELOPE_VERSION` caller is
# unchanged.


def page_keys(ring, devices):
    """A stable key per page, so the current page survives sessions and
    devices coming and going: ("overview", None), ("agents", n) or
    ("device", device_id)."""
    keys = []
    agents_index = 0
    for kind, value in ring:
        if kind == "agents":
            keys.append(("agents", agents_index))
            agents_index += 1
        elif kind == "device":
            keys.append(("device", devices[value].get("device_id")))
        else:
            keys.append(("overview", None))
    return keys


def fallback_key(page_key, keys):
    """Where to go when the current page is gone: the last agents page for
    a vanished agents page, else the overview."""
    if page_key[0] == "agents":
        agents = [key for key in keys if key[0] == "agents"]
        if agents:
            return agents[-1]
    return ("overview", None)


def focus_state(page_key):
    """The starting state for step_focus: on page_key, no focus jump."""
    return {"page_key": page_key, "focus_until": 0.0, "focus_return": None}


def step_focus(state, event, now):
    """The focus jump as a pure state machine. Returns a new state dict
    {"page_key", "focus_until", "focus_return"}; the input is not changed.

    Events:
      {"kind": "line", "prev_states", "new_states", "last_tap", "blocked"}
          a new socket line; a new needs_you (should_focus_agents) jumps to
          the first agents page for FOCUS_FOR_S. A second jump during a
          focus keeps the page to return to. blocked: the network-problem
          screen is up, so never jump.
      {"kind": "tick", "keys"}
          every frame, with the current page keys: an expired focus returns
          to the saved page, and a page that has gone falls back.
      {"kind": "tap", "keys", "advance"}
          any tap ends the focus; advance moves to the next page in keys.
    """
    page_key = state["page_key"]
    focus_until = state["focus_until"]
    focus_return = state["focus_return"]
    kind = event.get("kind")

    if kind == "line":
        jump = not event.get("blocked") and should_focus_agents(
            event.get("prev_states"), event.get("new_states") or {}, event.get("last_tap"), now)
        if jump:
            # only the first jump of a focus remembers where to go back to; an
            # expired focus whose tick has not run yet still holds its return
            # page, so it is kept, and the tick no longer sees stale state
            if focus_return is None:
                focus_return = page_key
            page_key = ("agents", 0)
            focus_until = now + FOCUS_FOR_S

    elif kind == "tick":
        keys = event.get("keys") or [("overview", None)]
        if focus_until and now >= focus_until:
            # back to the saved page, or where that page would fall back to
            if focus_return is not None:
                page_key = focus_return
            focus_until = 0.0
            focus_return = None
        if page_key not in keys:
            page_key = fallback_key(page_key, keys)

    elif kind == "tap":
        focus_until = 0.0
        focus_return = None
        keys = event.get("keys") or [("overview", None)]
        if event.get("advance"):
            if page_key not in keys:
                page_key = fallback_key(page_key, keys)
            page_key = keys[(keys.index(page_key) + 1) % len(keys)]

    return {"page_key": page_key, "focus_until": focus_until, "focus_return": focus_return}


# ---- display backend switch (T-061) ----

# DESKPANE_DISPLAY values: the panel, a desktop window, offscreen PNG frames
BACKENDS = ("fb", "window", "png")
DEFAULT_BACKEND = "fb"
# DESKPANE_SCALE: window size in multiples of the 480 x 320 panel
MIN_SCALE = 1
MAX_SCALE = 4
DEFAULT_SCALE = 2


def pick_backend(value):
    """(backend, note) for a DESKPANE_DISPLAY value. Unset or empty is the
    framebuffer, as on the Pi. An unknown value also falls back to the
    framebuffer, with a note for the log; note is None otherwise."""
    if value is None:
        return DEFAULT_BACKEND, None
    name = value.strip().lower()
    if name == "":
        return DEFAULT_BACKEND, None
    if name in BACKENDS:
        return name, None
    return DEFAULT_BACKEND, "unknown DESKPANE_DISPLAY %r, using %s" % (value, DEFAULT_BACKEND)


def pick_scale(value):
    """The window scale from a DESKPANE_SCALE value: 1 to 4, default 2.
    Out of range clamps; text that is not a number is the default."""
    if value is None:
        return DEFAULT_SCALE
    try:
        scale = int(value.strip())
    except ValueError:
        return DEFAULT_SCALE
    return max(MIN_SCALE, min(MAX_SCALE, scale))


def scale_tap(pos, scale, size=(480, 320)):
    """A mouse position in the scaled preview window as a tap in panel
    pixels, clamped to the screen. A scale below 1 counts as 1."""
    factor = max(1, int(scale))
    width, height = size
    x = max(0, min(width - 1, int(pos[0]) // factor))
    y = max(0, min(height - 1, int(pos[1]) // factor))
    return x, y
