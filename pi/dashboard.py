"""Dashboard: reads the daemon's device-status broadcast over a Unix
socket and renders it onto the Pi's framebuffer touchscreen, in the
animated ring-gauge style of the original single-device prototype.
Page 0 is an overview (clock, date, device count, agents line, version);
then the agents pages (one per 5 Claude Code sessions, none without
sessions), then one page per device. Tap the screen (see touch.py) to cycle
pages. With a character pack installed (CHARACTERS_DIR), sessions show a
pixel-art character instead of the built-in icon.
"""
import json
import math
import os
import socket
import subprocess
import sys
import threading
import time
import traceback

import pygame

import charpack
import display
import sprites
from layout import (
    agent_line2,
    agent_rows_layout,
    agent_style,
    agent_summary,
    agent_title,
    BATTERY_MARGIN,
    battery_fields,
    battery_geometry,
    battery_style,
    battery_widget_layout,
    bolt_points,
    fit_text,
    fmt_elapsed,
    focus_state,
    host_max_w,
    local_battery,
    overview_layout,
    page_keys,
    page_ring,
    parse_socket_line,
    read_version,
    sort_agents,
    step_focus,
    version_position,
    version_text,
)
from touch import TouchWatcher

SOCKET_PATH = os.environ.get("SOCKET_PATH", "/run/deskpane/daemon.sock")
# installed by install.sh --characters; root owned, read only for us
CHARACTERS_DIR = os.environ.get("CHARACTERS_DIR", "/var/lib/deskpane/characters")
# the interface NetworkMonitor checks; "none" (the PC preview) means always ok
NET_IFACE = os.environ.get("NET_IFACE", "wlan0")

# A full 320x480 RGB565 frame (307KB) takes ~150ms to physically transfer
# over the SPI bus at 16MHz. Redrawing faster than that interrupts the panel
# mid-transfer and blends two frames together (the "ghosted/obfuscated"
# look) -- this is a dashboard, not an animation, so a low FPS costs nothing.
FPS = 3
# 5 fps only while characters animate on the agents page (plan decision 3);
# 200 ms per frame is still above the 150 ms SPI transfer
AGENTS_FPS = charpack.CHAR_FPS
TRANSITION_SECS = 0.35
# repeat the same drawing error in the log at most once per this many seconds
DRAW_ERROR_LOG_EVERY_SECS = 30
EASE = 0.12

BG = (16, 17, 21)
CARD = (26, 28, 34)
TEXT = (235, 236, 240)
SUBTEXT = (130, 134, 145)
ACCENT = (64, 209, 171)
WARN = (235, 180, 64)
CRIT = (232, 93, 93)
RING_BG = (45, 48, 58)

# agents page geometry: header, page dots, state bar, icon and character cells
AGENT_HEADER_H = 40
AGENT_FOOTER_H = 24
AGENT_BAR_W = 8
AGENT_ICON_CELL = 36
AGENT_ICON_R = 11
# with a pack: the 48 px character sits at the left of an 80 px cell and
# its 32 px emote to the right of the head, so the bubble never covers it
AGENT_CHAR_CELL = 80
AGENT_CHAR_PAD = 4
# the emote may overlap this much of the character frame's empty right edge
AGENT_EMOTE_TUCK = 8
# gap between the icon or character and the text column
AGENT_TEXT_GAP = 10
# gap between line 1 and line 2 of a row
AGENT_LINE_GAP = 3
# single card: the art cell and the gap between the head and the bubble
AGENT_ART_W = 160
AGENT_CARD_PAD = 12
AGENT_EMOTE_GAP = 4
# agent state colours by the names layout.agent_style returns
AGENT_COLORS = {
    "amber": WARN,
    "blue": (92, 152, 236),
    "greyblue": (112, 128, 152),
    "green": (96, 200, 112),
    "red": CRIT,
    "grey": SUBTEXT,
}
# text on the amber needs-you fill
DARK_TEXT = (28, 22, 8)
# secondary text on the amber fill: tinted from the amber, still 5:1 on it
AMBER_SUBTEXT = (88, 62, 16)
# the state bar of a needs-you row, a shade of the fill instead of a black notch
AMBER_BAR = (176, 128, 32)
ROW_LINE = (40, 42, 50)


# pygame.font.Font(None, n) draws its built-in font at n * 0.6875; the
# freetype fonts below use the same factor so every size stays as it was
DEFAULT_FONT_SCALE = 0.6875
FONT_SIZES = {"clock": 120, "huge": 56, "big": 40, "mid": 26, "small": 20, "tiny": 16}


# below this pixel size, text is drawn 4x larger and shrunk with an average
# filter: no hinting at all, so letters keep their real shapes and spacing
SUPERSAMPLE_BELOW_PX = 24
SUPERSAMPLE = 4
TEXT_CACHE_MAX = 256
# fonts at or above this pixel size keep a smaller cache
BIG_TEXT_PX = 24
BIG_TEXT_CACHE_MAX = 32


class TextFont:
    """The built-in font (freesansbold.ttf, shipped inside pygame) drawn
    with pygame.freetype instead of pygame.font. pygame.font hints the bold
    glyphs so hard at small sizes that letters drift apart ("homel ab") and
    an "i" looks like an "l"; freetype's own hinter closes the "c" into an
    "o" at the same sizes. So small sizes are drawn unhinted (4x, then
    averaged down) with kerning, and big sizes, where hinting is harmless,
    directly. It keeps the calls the pages use: render(text, antialias,
    color) and size(text); every render is one line tall, so text lines up
    the same way it did with pygame.font."""

    def __init__(self, size):
        px = size * DEFAULT_FONT_SCALE
        self._ss = SUPERSAMPLE if px < SUPERSAMPLE_BELOW_PX else 1
        self._font = pygame.freetype.Font(None, px * self._ss)
        self._font.kerning = True
        self._font.pad = False
        self._ascent = self._font.get_sized_ascender()
        span = self._ascent - self._font.get_sized_descender()
        self._height = -(-span // self._ss)
        self._cache = {}
        self._boxes = {}
        # big fonts (the clock) make large surfaces, so they keep fewer
        self._cache_max = BIG_TEXT_CACHE_MAX if px >= BIG_TEXT_PX else TEXT_CACHE_MAX

    def _box(self, text):
        # fit_text measures once per dropped character, so boxes are cached
        box = self._boxes.get(text)
        if box is not None:
            return box
        rect = self._font.get_rect(text)
        left = max(0, rect.x)
        box = left, rect.y, max(1, -(-(left + rect.width) // self._ss))
        if len(self._boxes) >= self._cache_max:
            self._boxes.clear()
        self._boxes[text] = box
        return box

    def size(self, text):
        if not text:
            return 1, self._height
        return self._box(text)[2], self._height

    def get_height(self):
        return self._height

    def render(self, text, antialias, color):
        if not text:
            return pygame.Surface((1, self._height), pygame.SRCALPHA)
        key = (text, tuple(color), bool(antialias))
        cached = self._cache.get(key)
        if cached is not None:
            return cached
        left, top, width = self._box(text)
        ss = self._ss
        surface = pygame.Surface((width * ss, self._height * ss), pygame.SRCALPHA)
        self._font.antialiased = bool(antialias)
        self._font.render_to(surface, (left, self._ascent - top), text, color)
        if ss > 1:
            surface = pygame.transform.smoothscale(surface, (width, self._height))
        # the clock and elapsed times change, so the cache is simply reset when full
        if len(self._cache) >= self._cache_max:
            self._cache.clear()
        self._cache[key] = surface
        return surface


def load_fonts():
    """One font per role. Falls back to pygame.font (the old look) if
    pygame.freetype is missing, so the dashboard always starts."""
    try:
        import pygame.freetype
        pygame.freetype.init()
        return {name: TextFont(size) for name, size in FONT_SIZES.items()}
    except Exception as error:
        print(f"dashboard: freetype fonts unavailable, pygame.font instead: {error!r}", file=sys.stderr, flush=True)
        return {name: pygame.font.Font(None, size) for name, size in FONT_SIZES.items()}


def level_color(pct):
    if pct < 60:
        return ACCENT
    if pct < 85:
        return WARN
    return CRIT


class SocketReader:
    """Background thread: keeps the latest device-status list from the
    daemon. Reconnects on its own if the daemon isn't up yet or drops."""

    def __init__(self, path=SOCKET_PATH):
        self._path = path
        self._lock = threading.Lock()
        self._devices = []
        self._sessions = []
        # counts good lines, so the main loop sees when a new one arrived
        self._line_no = 0
        self._connected = False
        threading.Thread(target=self._loop, daemon=True).start()

    def _loop(self):
        while True:
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(self._path)
                f = s.makefile()
                self._connected = True
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        decoded = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    # the envelope, or a legacy bare array; anything else is skipped
                    parsed = parse_socket_line(decoded)
                    if parsed is None:
                        continue
                    with self._lock:
                        self._devices, self._sessions = parsed
                        self._line_no += 1
            except OSError:
                pass
            self._connected = False
            time.sleep(1)

    @property
    def devices(self):
        with self._lock:
            return list(self._devices)

    @property
    def sessions(self):
        with self._lock:
            return list(self._sessions)

    @property
    def line_no(self):
        with self._lock:
            return self._line_no

    @property
    def connected(self):
        return self._connected


class NetworkMonitor:
    """Background thread: checks wlan0's own link/address/route state purely
    locally, no network traffic sent. This has to work precisely when the
    network itself is down, and it's independent of the daemon/reporter
    pipeline: the Pi's own stats keep flowing to the daemon over loopback
    regardless of WiFi state, so a dead link here doesn't blank the
    dashboard, it just flags itself."""

    def __init__(self, iface="wlan0", interval_s=5):
        self._iface = iface
        self._interval_s = interval_s
        self._lock = threading.Lock()
        self._ok = True
        self._reason = "ok"
        # the preview's N key forces a problem, whatever the interface says
        self._forced = False
        # NET_IFACE=none: nothing to check, the link counts as up
        if iface != "none":
            threading.Thread(target=self._loop, daemon=True).start()

    def toggle_forced(self):
        """Turns the forced problem on or off; returns the new setting."""
        with self._lock:
            self._forced = not self._forced
            return self._forced

    def _check(self):
        try:
            operstate = open(f"/sys/class/net/{self._iface}/operstate").read().strip()
        except FileNotFoundError:
            return False, f"{self._iface} not found"
        if operstate != "up":
            return False, f"{self._iface} is {operstate}"
        try:
            addr_out = subprocess.run(
                ["ip", "-4", "-o", "addr", "show", self._iface],
                capture_output=True, text=True, timeout=2,
            ).stdout
        except Exception as e:
            return False, f"ip addr failed: {e}"
        if "inet " not in addr_out:
            return False, "no IP address assigned"
        try:
            route_out = subprocess.run(
                ["ip", "route", "show", "default"],
                capture_output=True, text=True, timeout=2,
            ).stdout
        except Exception as e:
            return False, f"ip route failed: {e}"
        if "default" not in route_out:
            return False, "no default route"
        return True, "ok"

    def _loop(self):
        while True:
            ok, reason = self._check()
            with self._lock:
                self._ok = ok
                self._reason = reason
            time.sleep(self._interval_s)

    @property
    def ok(self):
        with self._lock:
            return self._ok and not self._forced

    @property
    def reason(self):
        with self._lock:
            if self._forced:
                return "forced by the preview (N)"
            return self._reason


def point_in_circle(point, center, radius):
    dx = point[0] - center[0]
    dy = point[1] - center[1]
    return dx * dx + dy * dy <= radius * radius


def draw_network_icon(surface, center, color):
    cx, cy = center
    pygame.draw.circle(surface, color, center, 14, 2)
    pygame.draw.line(surface, color, (cx - 7, cy - 7), (cx + 7, cy + 7), 2)
    pygame.draw.line(surface, color, (cx - 7, cy + 7), (cx + 7, cy - 7), 2)


def draw_network_problem(surface, fonts, reason, width, height):
    surface.fill(BG)
    title_surf = fonts["mid"].render("Network problem", True, CRIT)
    surface.blit(title_surf, (width // 2 - title_surf.get_width() // 2, 70))

    reason_surf = fonts["small"].render(reason, True, SUBTEXT)
    surface.blit(reason_surf, (width // 2 - reason_surf.get_width() // 2, 70 + title_surf.get_height() + 10))

    note_surf = fonts["tiny"].render("This Pi's own stats keep reporting locally.", True, SUBTEXT)
    surface.blit(note_surf, (width // 2 - note_surf.get_width() // 2, 70 + title_surf.get_height() + 40))

    exit_surf = fonts["small"].render("Tap anywhere to exit to the terminal", True, ACCENT)
    surface.blit(exit_surf, (width // 2 - exit_surf.get_width() // 2, height - 50))


def fmt_uptime(seconds):
    seconds = int(seconds)
    days, seconds = divmod(seconds, 86400)
    hours, seconds = divmod(seconds, 3600)
    minutes, _ = divmod(seconds, 60)
    if days:
        return f"{days}d {hours}h"
    if hours:
        return f"{hours}h {minutes}m"
    return f"{minutes}m"


def draw_ring_gauge(surface, center, radius, pct, color, thickness, label, value_text, font_big, font_small):
    cx, cy = center
    rect = pygame.Rect(cx - radius, cy - radius, radius * 2, radius * 2)
    pygame.draw.circle(surface, RING_BG, center, radius, thickness)

    start = -math.pi / 2
    end = start + 2 * math.pi * max(0.004, pct / 100.0)
    pygame.draw.arc(surface, color, rect, -end, -start, thickness)

    val_surf = font_big.render(value_text, True, TEXT)
    surface.blit(val_surf, (cx - val_surf.get_width() // 2, cy - val_surf.get_height() // 2 - 6))
    lab_surf = font_small.render(label, True, SUBTEXT)
    surface.blit(lab_surf, (cx - lab_surf.get_width() // 2, cy + val_surf.get_height() // 2 - 2))


def draw_up_icon(surface, pos, color):
    x, y = pos
    pygame.draw.line(surface, color, (x, y + 9), (x, y - 6), 3)
    pygame.draw.polygon(surface, color, [(x - 6, y - 2), (x, y - 11), (x + 6, y - 2)])


def draw_temp_icon(surface, pos, color, bg):
    x, y = pos
    pygame.draw.rect(surface, color, (x - 3, y - 12, 6, 16), border_radius=3)
    pygame.draw.circle(surface, color, (x, y + 6), 6)
    pygame.draw.circle(surface, bg, (x, y + 6), 3)


def draw_disk_icon(surface, pos, color):
    x, y = pos
    pygame.draw.circle(surface, color, (x, y), 10, 2)
    pygame.draw.circle(surface, color, (x, y), 2)
    pygame.draw.line(surface, color, (x, y), (x + 8, y - 6), 2)


def draw_battery(surface, font, right, top, pct, charging):
    """The battery icon with its percent on the left (and a bolt left of
    that while charging), ending at x = right, on one line starting at
    y = top. Shapes and colours come from layout.py. Returns the x where
    the widget starts, so callers can keep clear."""
    style = battery_style(pct, charging)
    color = CRIT if style["low"] else TEXT
    text_surf = font.render(style["text"], True, color)
    place = battery_widget_layout(right, top, text_surf.get_width(), text_surf.get_height(),
                                  charging=style["charging"])
    surface.blit(text_surf, place["text"])

    x, y = place["icon"]
    shapes = battery_geometry(x, y, pct)
    pygame.draw.rect(surface, SUBTEXT, shapes["body"], 1)
    pygame.draw.rect(surface, SUBTEXT, shapes["nub"])
    if shapes["fill"] is not None:
        pygame.draw.rect(surface, color, shapes["fill"])
    if place["bolt"] is not None:
        pygame.draw.polygon(surface, WARN, bolt_points(place["bolt"][0], place["bolt"][1]))
    return place["left"]


def draw_page_dots(surface, index, count, width, height):
    spacing = 16
    start_x = width // 2 - (count - 1) * spacing // 2
    for i in range(count):
        color = ACCENT if i == index else RING_BG
        pygame.draw.circle(surface, color, (start_x + i * spacing, height - 10), 3)


def draw_pulse_dot(surface, pos, t0, now):
    pulse = (math.sin((now - t0) * 2.5) + 1) / 2
    color = tuple(int(ACCENT[i] * (0.4 + 0.6 * pulse)) for i in range(3))
    pygame.draw.circle(surface, color, pos, 4)


def render_agents_line(fonts, parts):
    """The overview's agents line, "Agents: 1 needs you · 1 error · 2
    working", as one surface: needs you in amber, error in red."""
    font = fonts["tiny"]
    pieces = [("Agents: ", SUBTEXT)]
    for index, (state, text) in enumerate(parts):
        if index > 0:
            pieces.append(("  \u00b7  ", SUBTEXT))
        if state == "needs_you":
            color = WARN
        elif state == "error":
            color = CRIT
        else:
            color = SUBTEXT
        pieces.append((text, color))
    rendered = [font.render(text, True, color) for text, color in pieces]
    width = sum(r.get_width() for r in rendered)
    height = max(r.get_height() for r in rendered)
    line = pygame.Surface((width, height), pygame.SRCALPHA)
    x = 0
    for r in rendered:
        line.blit(r, (x, 0))
        x += r.get_width()
    return line


def draw_overview(surface, fonts, devices, connected, t0, now, width, height, version, sessions=()):
    surface.fill(BG)

    # render first: the layout centres the block from the real text heights
    clock_surf = fonts["clock"].render(time.strftime("%H:%M"), True, TEXT)
    date_surf = fonts["small"].render(time.strftime("%A, %d %B %Y"), True, SUBTEXT)

    online_count = sum(1 for d in devices if d.get("online"))
    total_count = len(devices)

    if not connected:
        summary_text, summary_color = "daemon not reachable", CRIT
    elif total_count == 0:
        summary_text, summary_color = "no devices reporting yet", SUBTEXT
    else:
        plural = "s" if total_count != 1 else ""
        summary_text = f"{online_count}/{total_count} device{plural} online"
        summary_color = ACCENT if online_count == total_count else WARN

    summary_surf = fonts["tiny"].render(summary_text, True, summary_color)

    # one more line when Claude Code sessions report in, hidden otherwise
    parts = agent_summary(sessions)
    agents_surf = render_agents_line(fonts, parts) if parts else None
    agents_h = agents_surf.get_height() if agents_surf else 0

    layout = overview_layout(width, height, clock_surf.get_height(), date_surf.get_height(),
                             summary_surf.get_height(), agents_h=agents_h)
    surface.blit(clock_surf, (width // 2 - clock_surf.get_width() // 2, layout["clock_y"]))
    surface.blit(date_surf, (width // 2 - date_surf.get_width() // 2, layout["date_y"]))

    summary_y = layout["summary_y"]
    summary_x = width // 2 - summary_surf.get_width() // 2
    surface.blit(summary_surf, (summary_x, summary_y))
    if connected and total_count:
        draw_pulse_dot(surface, (summary_x - 12, summary_y + summary_surf.get_height() // 2), t0, now)
    if agents_surf is not None:
        surface.blit(agents_surf, (width // 2 - agents_surf.get_width() // 2, layout["agents_y"]))

    # version in the free bottom-left corner (dots bottom centre, network icon bottom right)
    version_surf = fonts["tiny"].render(version_text(version), True, SUBTEXT)
    surface.blit(version_surf, version_position(height, version_surf.get_height()))

    # this Pi's own battery in the free top-right corner, nothing without one
    battery = local_battery(devices)
    if battery is not None:
        draw_battery(surface, fonts["tiny"], width - BATTERY_MARGIN, BATTERY_MARGIN, battery[0], battery[1])


def dim_color(color, factor=0.55):
    return tuple(int(c * factor) for c in color)


def draw_agent_icon(surface, icon, center, color, t0, now, size=12):
    """Built-in state icon, used when no character pack is installed."""
    cx, cy = center
    if icon == "pulse":
        pulse = (math.sin((now - t0) * 2.5) + 1) / 2
        pulse_color = tuple(int(color[i] * (0.4 + 0.6 * pulse)) for i in range(3))
        pygame.draw.circle(surface, pulse_color, center, max(4, size // 2))
        return
    if icon == "circle":
        pygame.draw.circle(surface, color, center, size, max(2, size // 6))
        return
    if icon == "check":
        points = [(cx - size * 0.7, cy), (cx - size * 0.2, cy + size * 0.55), (cx + size * 0.75, cy - size * 0.6)]
        pygame.draw.lines(surface, color, False, points, max(3, size // 4))
        return
    # alert, cross and question sit in a ring
    pygame.draw.circle(surface, color, center, size, max(2, size // 6))
    stroke = max(2, size // 5)
    if icon == "alert":
        pygame.draw.line(surface, color, (cx, cy - size * 0.55), (cx, cy + size * 0.15), stroke)
        pygame.draw.circle(surface, color, (cx, int(cy + size * 0.5)), max(1, stroke // 2 + 1))
    elif icon == "cross":
        d = size * 0.45
        pygame.draw.line(surface, color, (cx - d, cy - d), (cx + d, cy + d), stroke)
        pygame.draw.line(surface, color, (cx - d, cy + d), (cx + d, cy - d), stroke)
    elif icon == "question":
        rect = pygame.Rect(0, 0, int(size * 0.9), int(size * 0.8))
        rect.center = (cx, int(cy - size * 0.2))
        pygame.draw.arc(surface, color, rect, -math.pi / 2, math.pi, stroke)
        pygame.draw.line(surface, color, (cx, cy + size * 0.2), (cx, cy + size * 0.3), stroke)
        pygame.draw.circle(surface, color, (cx, int(cy + size * 0.6)), max(1, stroke // 2 + 1))


class AgentScene:
    """What the agents page needs between frames: the pack (or None), the
    look per project, and the session tracker (new and leaving sessions)."""

    def __init__(self, pack):
        self.pack = pack
        self.looks = {}
        self.tracker = None

    def update(self, sorted_sessions, now):
        """Called once per new socket line."""
        self.tracker = charpack.track_sessions(self.tracker, sorted_sessions, now)
        if self.pack is None:
            return
        keys = [charpack.look_key(s.get("project") or "", s.get("device_id") or "") for s in sorted_sessions]
        # ghosts keep their look while they walk off
        for ghost in self.tracker["ghosts"].values():
            g = ghost["session"]
            keys.append(charpack.look_key(g.get("project") or "", g.get("device_id") or ""))
        self.looks = charpack.assign_looks(keys, self.looks, self.pack.looks)
        self.pack.keep_only(self.looks.values())

    def rows(self, sorted_sessions, now):
        """Sessions to draw: with a pack, ghosts walk off in their old slot;
        without one a session that left just disappears."""
        if self.pack is None or self.tracker is None:
            return list(sorted_sessions)
        ghosts = {}
        for sid, ghost in self.tracker["ghosts"].items():
            if now - ghost["left_at"] < charpack.WALK_OFF_S:
                ghosts[sid] = ghost
        return charpack.with_ghosts(sorted_sessions, ghosts)

    def pose(self, session, style, now):
        """character_pose for a row, from the tracker's local times."""
        seen = None
        if self.tracker is not None:
            seen = self.tracker["seen"].get(session.get("sid"))
        state_s, new_for_s, leaving_for_s = charpack.pose_times(session, seen, now)
        return charpack.character_pose(session.get("state"), session.get("quiet") is True, state_s,
                                       style["dim"], new_for_s, leaving_for_s)

    def look_for(self, session):
        return self.looks.get(charpack.look_key(session.get("project") or "", session.get("device_id") or ""), 0)


def draw_character(surface, scene, session, style, cell, char_scale, emote_scale, emote_pos, now,
                   clip=None, align_left=False):
    """The session's character and emote in `cell` (a Rect), centred, or
    at its left edge with `align_left`. The walk-off is clipped to `clip`
    (default the cell), so a leaving character never draws over text."""
    pack = scene.pack
    pose = scene.pose(session, style, now)
    now_ms = int(now * 1000)
    frame = 0 if pose["still"] else charpack.anim_frame(now_ms, charpack.CHAR_FPS, charpack.FRAMES_PER_MOTION)
    sprite = pack.frame(scene.look_for(session), pose["direction"], pose["motion"], frame, char_scale, pose["dim"])
    size = sprite.get_width()
    area = clip if clip is not None else cell
    x = cell.x if align_left else cell.x + (cell.width - size) // 2
    x += int(pose["offset"] * area.width)
    y = cell.y + (cell.height - size) // 2
    old_clip = surface.get_clip()
    # the clip is always put back, even if the blit raises
    try:
        surface.set_clip(area.clip(old_clip))
        surface.blit(sprite, (x, y))
    finally:
        surface.set_clip(old_clip)
    if pose["emote"] is not None and emote_scale > 0:
        bubble = pack.emote(pose["emote"], charpack.emote_frame(pose["emote_for_s"], charpack.CHAR_FPS), emote_scale)
        if bubble is not None:
            surface.blit(bubble, emote_pos(x, y, size, bubble.get_width()))


def session_style(session):
    """agent_style for a session; a ghost is grey and says ENDED."""
    if session.get("ghost"):
        return agent_style("ended", False, 0)
    return agent_style(session.get("state"), session.get("quiet") is True, session.get("state_s"))


def session_line2(session):
    """Line 2 for a row or card; a ghost shows only the machine."""
    if session.get("ghost"):
        return agent_line2({"label": session.get("label"), "device_id": session.get("device_id")})
    return agent_line2(session)


def agent_colors(style):
    """Bar, text, secondary text and word colours for one row or card.
    Needs you is the amber fill with dark text; dimmed rows fade all four."""
    color = AGENT_COLORS.get(style["color"], SUBTEXT)
    if style["fill"]:
        return AMBER_BAR, DARK_TEXT, AMBER_SUBTEXT, DARK_TEXT
    if style["dim"]:
        return dim_color(color), dim_color(TEXT), dim_color(SUBTEXT), dim_color(color)
    return color, TEXT, SUBTEXT, color


# border_radius needs pygame 2.0 or later (Raspberry Pi OS ships 2.1).
def draw_word_badge(surface, font, word, right, center_y):
    """NEEDS YOU on a row: amber letters on a dark pill, the one inverted
    element on the page, so it is the first thing seen from across the room.
    Returns the badge width."""
    word_surf = font.render(word, True, WARN)
    pad_x, pad_y = 7, 2
    badge = pygame.Rect(0, 0, word_surf.get_width() + 2 * pad_x, word_surf.get_height() + 2 * pad_y)
    badge.midright = (right, center_y)
    pygame.draw.rect(surface, DARK_TEXT, badge, border_radius=4)
    surface.blit(word_surf, (badge.x + pad_x, badge.y + pad_y))
    return badge.width


def draw_agent_row(surface, fonts, scene, session, rect, t0, now):
    """One row: state bar, icon or character, project and time in state,
    then label, doing and the state word."""
    x, y, w, h = rect
    style = session_style(session)
    bar_color, text_color, sub_color, word_color = agent_colors(style)

    # needs you: the whole row filled amber with dark text
    if style["fill"]:
        pygame.draw.rect(surface, WARN, rect)
    pygame.draw.rect(surface, bar_color, (x, y, AGENT_BAR_W, h))

    # character cell with a pack, else the small built-in icon
    drawn_character = False
    if scene.pack is not None:
        cell = pygame.Rect(x + AGENT_BAR_W + AGENT_CHAR_PAD, y, AGENT_CHAR_CELL - AGENT_CHAR_PAD, h)
        try:
            # emote right of the head, its tail tucked into the frame's empty edge
            draw_character(surface, scene, session, style, cell, charpack.ROW_CHAR_SCALE, charpack.ROW_EMOTE_SCALE,
                           lambda cx, cy, size, bw: (cx + size - AGENT_EMOTE_TUCK, max(y + 1, cy - 2)), now,
                           align_left=True)
            drawn_character = True
        except Exception as error:
            # a broken sprite falls back to the icon for this row only
            print(f"dashboard: character draw failed, icon instead: {error!r}", file=sys.stderr, flush=True)
    if drawn_character:
        text_x = x + AGENT_BAR_W + AGENT_CHAR_CELL + AGENT_TEXT_GAP
    else:
        icon_color = DARK_TEXT if style["fill"] else bar_color
        draw_agent_icon(surface, style["icon"], (x + AGENT_BAR_W + AGENT_ICON_CELL // 2 + 4, y + h // 2),
                        icon_color, t0, now, size=AGENT_ICON_R)
        text_x = x + AGENT_BAR_W + AGENT_ICON_CELL + AGENT_TEXT_GAP

    # the two lines are centred as one block in the row
    right = x + w - 12
    line1_h = fonts["mid"].get_height()
    line2_h = fonts["small"].get_height()
    line1_y = y + (h - line1_h - AGENT_LINE_GAP - line2_h) // 2
    line2_y = line1_y + line1_h + AGENT_LINE_GAP

    # line 1: project and time in state, on the same baseline
    elapsed_surf = fonts["small"].render(fmt_elapsed(session.get("state_s")), True, sub_color)
    title_w = right - text_x - elapsed_surf.get_width() - 10
    title = fit_text(agent_title(session), title_w, lambda t: fonts["mid"].size(t)[0])
    title_surf = fonts["mid"].render(title, True, text_color)
    surface.blit(title_surf, (text_x, line1_y))
    surface.blit(elapsed_surf, (right - elapsed_surf.get_width(), line1_y + line1_h - line2_h - 1))

    # line 2: label and doing, then the state word (a badge for needs you)
    if style["fill"]:
        word_w = draw_word_badge(surface, fonts["small"], style["word"], right + 4, line2_y + line2_h // 2)
    else:
        word_surf = fonts["small"].render(style["word"], True, word_color)
        word_w = word_surf.get_width()
        surface.blit(word_surf, (right - word_w, line2_y))
    line2_w = right - text_x - word_w - 10
    line2 = fit_text(session_line2(session), line2_w, lambda t: fonts["small"].size(t)[0])
    surface.blit(fonts["small"].render(line2, True, sub_color), (text_x, line2_y))

    pygame.draw.line(surface, ROW_LINE, (x, y + h - 1), (x + w, y + h - 1), 1)


def card_scales(art):
    """Character and emote scale for the card: the largest whole-number
    scale where the character plus its emote above the head fit the art
    cell, so the bubble gets its own space."""
    scale = charpack.best_scale(art.width, art.height, charpack.FRAME)
    while scale > 1:
        emote = max(1, scale // 2)
        if (scale + emote) * charpack.FRAME + AGENT_EMOTE_GAP <= art.height:
            return scale, emote
        scale -= 1
    return scale, 0


def draw_agent_card(surface, fonts, scene, session, rect, t0, now):
    """A single session: a big character (or icon) on the left; project,
    the state word very large, then doing and the times on the right. Both
    sides are centred as groups, so the card reads as one block."""
    x, y, w, h = rect
    style = session_style(session)
    bar_color, text_color, sub_color, word_color = agent_colors(style)
    if style["fill"]:
        pygame.draw.rect(surface, WARN, rect)
    pygame.draw.rect(surface, bar_color, (x, y, AGENT_BAR_W, h))

    art = pygame.Rect(x + AGENT_BAR_W + AGENT_CARD_PAD, y + AGENT_CARD_PAD, AGENT_ART_W, h - 2 * AGENT_CARD_PAD)
    drawn_character = False
    if scene.pack is not None:
        scale, emote_scale = card_scales(art)
        if scale > 0:
            size = scale * charpack.FRAME
            # room for the bubble only while the pose has one (working never
            # does), so a character with no emote sits centred
            has_emote = scene.pose(session, style, now)["emote"] is not None
            bubble = emote_scale * charpack.FRAME if has_emote else 0
            group_w = max(size, size * 9 // 16 + bubble)
            group_h = size + (bubble + AGENT_EMOTE_GAP if bubble else 0)
            cell = pygame.Rect(art.x + (art.width - group_w) // 2, art.y + (art.height - group_h) // 2 + group_h - size,
                               size, size)
            try:
                # bubble above the head and to its right, never over it
                draw_character(surface, scene, session, style, cell, scale, emote_scale,
                               lambda cx, cy, sz, bw: (cx + sz * 9 // 16, cy - bw - AGENT_EMOTE_GAP + scale), now,
                               clip=art)
                drawn_character = True
            except Exception as error:
                print(f"dashboard: character draw failed, icon instead: {error!r}", file=sys.stderr, flush=True)
    if not drawn_character:
        icon_color = DARK_TEXT if style["fill"] else bar_color
        draw_agent_icon(surface, style["icon"], art.center, icon_color, t0, now, size=44)

    text_x = art.right + 16
    text_w = x + w - 14 - text_x
    title = fit_text(agent_title(session), text_w, lambda t: fonts["big"].size(t)[0])
    title_surf = fonts["big"].render(title, True, text_color)
    # the state word is the largest thing on the card; a long word steps down
    word_font = fonts["huge"] if fonts["huge"].size(style["word"])[0] <= text_w else fonts["big"]
    word_surf = word_font.render(style["word"], True, word_color)
    doing = fit_text(session_line2(session), text_w, lambda t: fonts["small"].size(t)[0])
    doing_surf = fonts["small"].render(doing, True, sub_color)
    times = "for %s  \u00b7  running %s" % (fmt_elapsed(session.get("state_s")), fmt_elapsed(session.get("running_s")))
    times_surf = fonts["small"].render(fit_text(times, text_w, lambda t: fonts["small"].size(t)[0]), True, sub_color)

    lines = [(title_surf, 0), (word_surf, 2), (doing_surf, 12), (times_surf, 4)]
    block_h = sum(surf.get_height() + gap for surf, gap in lines)
    line_y = y + (h - block_h) // 2
    for surf, gap in lines:
        line_y += gap
        surface.blit(surf, (text_x, line_y))
        line_y += surf.get_height()


def draw_agents(surface, fonts, scene, rows, page_number, page_count, t0, now, width, height):
    """One agents page: header, then one card or up to 5 rows."""
    surface.fill(BG)
    title_surf = fonts["mid"].render("Agents", True, TEXT)
    surface.blit(title_surf, (14, (AGENT_HEADER_H - title_surf.get_height()) // 2))
    if page_count > 1:
        page_surf = fonts["small"].render(f"{page_number + 1}/{page_count}", True, SUBTEXT)
        surface.blit(page_surf, (14 + title_surf.get_width() + 10, (AGENT_HEADER_H - page_surf.get_height()) // 2 + 1))
    clock_surf = fonts["small"].render(time.strftime("%H:%M"), True, SUBTEXT)
    surface.blit(clock_surf, (width - clock_surf.get_width() - 14, (AGENT_HEADER_H - clock_surf.get_height()) // 2))
    pygame.draw.line(surface, ROW_LINE, (0, AGENT_HEADER_H - 1), (width, AGENT_HEADER_H - 1), 1)

    rects = agent_rows_layout(width, height, len(rows), AGENT_HEADER_H, AGENT_FOOTER_H)
    if len(rows) == 1:
        draw_agent_card(surface, fonts, scene, rows[0], rects[0], t0, now)
        return
    for session, rect in zip(rows, rects):
        draw_agent_row(surface, fonts, scene, session, rect, t0, now)


class DeviceView:
    """Eased display values for one device, so rings/bars glide toward a
    fresh reading each time a new report arrives instead of jumping."""

    def __init__(self):
        self.disp_cpu = 0.0
        self.disp_mem = 0.0
        self.disp_disk = 0.0

    def ease(self, device):
        mem_total = device.get("mem_total_mb", 0) or 1
        mem_pct = device.get("mem_used_mb", 0) / mem_total * 100
        disk_total = device.get("disk_total_gb", 0) or 1
        disk_pct = device.get("disk_used_gb", 0) / disk_total * 100
        self.disp_cpu += (device.get("cpu_pct", 0) - self.disp_cpu) * EASE
        self.disp_mem += (mem_pct - self.disp_mem) * EASE
        self.disp_disk += (disk_pct - self.disp_disk) * EASE

    def render(self, surface, device, fonts, t0, now, width, height):
        font_big, font_mid, font_small, font_tiny = fonts["big"], fonts["mid"], fonts["small"], fonts["tiny"]
        surface.fill(BG)

        if not device.get("online", False):
            msg = font_mid.render(f"{device.get('label', '?')}: offline", True, CRIT)
            surface.blit(msg, (width // 2 - msg.get_width() // 2, height // 2 - 10))
            return

        clock_str = time.strftime("%H:%M:%S")
        clk_surf = font_small.render(clock_str, True, SUBTEXT)
        clk_x = width - clk_surf.get_width() - 14
        surface.blit(clk_surf, (clk_x, 14))

        # the device's battery left of the clock, nothing without one
        clear_x = clk_x
        battery = battery_fields(device)
        if battery is not None:
            clear_x = draw_battery(surface, font_tiny, clk_x - 10, 14, battery[0], battery[1])

        # the hostname, cut so it ends clear of the battery (or the clock)
        host = fit_text(device.get("hostname", ""), host_max_w(14, clear_x),
                        lambda text: font_mid.size(text)[0])
        host_surf = font_mid.render(host, True, TEXT)
        surface.blit(host_surf, (14, 10))
        tag_surf = font_tiny.render(device.get("label", ""), True, ACCENT)
        surface.blit(tag_surf, (14, 36))
        ip_surf = font_small.render(device.get("ip", ""), True, SUBTEXT)
        surface.blit(ip_surf, (14 + tag_surf.get_width() + 8, 35))

        draw_pulse_dot(surface, (width - 14, 40), t0, now)

        pygame.draw.line(surface, (40, 42, 50), (0, 58), (width, 58), 1)

        draw_ring_gauge(surface, (120, 160), 62, self.disp_cpu, level_color(self.disp_cpu),
                         10, "CPU", f"{self.disp_cpu:.0f}%", font_big, font_small)
        draw_ring_gauge(surface, (280, 160), 62, self.disp_mem, level_color(self.disp_mem),
                         10, "RAM", f"{self.disp_mem:.0f}%", font_big, font_small)

        temp_c = device.get("temp_c", 0)
        draw_temp_icon(surface, (400, 120), level_color(min(100, temp_c)) if temp_c else SUBTEXT, BG)
        temp_surf = font_mid.render(f"{temp_c:.0f}°C" if temp_c else "N/A", True, TEXT)
        surface.blit(temp_surf, (420, 108))

        draw_up_icon(surface, (400, 175), ACCENT)
        up_surf = font_small.render(fmt_uptime(device.get("uptime_s", 0)), True, SUBTEXT)
        surface.blit(up_surf, (418, 168))

        bar_x, bar_y, bar_w, bar_h = 20, 258, width - 40, 14
        pygame.draw.rect(surface, CARD, (bar_x, bar_y, bar_w, bar_h), border_radius=7)
        fill_w = int(bar_w * max(0.01, self.disp_disk / 100.0))
        pygame.draw.rect(surface, level_color(self.disp_disk), (bar_x, bar_y, fill_w, bar_h), border_radius=7)
        draw_disk_icon(surface, (bar_x + 12, bar_y - 14), SUBTEXT)
        disk_label = f"Disk  {device.get('disk_used_gb', 0):.1f}/{device.get('disk_total_gb', 0):.0f} GB"
        dl_surf = font_tiny.render(disk_label, True, SUBTEXT)
        surface.blit(dl_surf, (bar_x + 28, bar_y - 18))

        mem_label = f"{device.get('mem_used_mb', 0):.0f}/{device.get('mem_total_mb', 0):.0f} MB"
        ml_surf = font_tiny.render(mem_label, True, SUBTEXT)
        surface.blit(ml_surf, (280 - ml_surf.get_width() // 2, 228))


def load_pack():
    """The installed character pack, or None. Never fatal."""
    try:
        return sprites.CharacterPack.load(CHARACTERS_DIR)
    except Exception as error:
        print(f"dashboard: character pack not loaded, built-in icons: {error!r}", file=sys.stderr, flush=True)
        return None


# ---- the panel registry: the draw half ----
#
# Each page on screen is drawn by a registered panel, keyed by the same kind
# strings and in the same order as layout.PAGE_PROVIDERS (the ring half). A
# panel draw takes (surface, value, frame): value is the ring value for that
# page (None for the overview, an (start, end) slice for an agents page, a
# device index for a device page), and frame carries this frame's data and
# fonts. This is a pure indirection: the draw calls and their arguments are
# exactly what the loop ran before, so the screen stays pixel-identical. Later
# phases swap these draws for the declarative renderer without touching the loop.


class PanelFrame:
    """Everything a panel draw needs for one frame, built once per frame."""

    def __init__(self, fonts, devices, connected, sorted_sessions, scene,
                 agent_rows_all, keys, page_key, views, version, t0, now, width, height):
        self.fonts = fonts
        self.devices = devices
        self.connected = connected
        self.sorted_sessions = sorted_sessions
        self.scene = scene
        self.agent_rows_all = agent_rows_all
        self.keys = keys
        self.page_key = page_key
        self.views = views
        self.version = version
        self.t0 = t0
        self.now = now
        self.width = width
        self.height = height


def draw_overview_panel(surface, value, frame):
    """The overview page (value is None)."""
    draw_overview(surface, frame.fonts, frame.devices, frame.connected, frame.t0, frame.now,
                  frame.width, frame.height, frame.version, sessions=frame.sorted_sessions)


def draw_agents_panel(surface, value, frame):
    """One agents page; value is the (start, end) slice of the sorted rows."""
    start, end = value
    agent_page_count = len([key for key in frame.keys if key[0] == "agents"])
    draw_agents(surface, frame.fonts, frame.scene, frame.agent_rows_all[start:end], frame.page_key[1],
                agent_page_count, frame.t0, frame.now, frame.width, frame.height)


def draw_device_panel(surface, value, frame):
    """One device page; value is the device index."""
    device = frame.devices[value]
    view = frame.views.setdefault(device["device_id"], DeviceView())
    view.ease(device)
    view.render(surface, device, frame.fonts, frame.t0, frame.now, frame.width, frame.height)


# kind -> draw, one entry per layout.PAGE_PROVIDERS kind, in the same order
PANEL_DRAW = {
    "overview": draw_overview_panel,
    "agents": draw_agents_panel,
    "device": draw_device_panel,
}


def main(screen=None):
    """The dashboard loop. screen is a display from display.py; by default
    DESKPANE_DISPLAY picks one, and the Pi (which sets nothing) gets the
    framebuffer and evdev taps exactly as before."""
    if screen is None:
        screen = display.open_display()
    surface = screen.surface_for_drawing()
    width, height = surface.get_size()

    fonts = load_fonts()

    # read once: the VERSION file sits one folder above this dashboard folder
    version = read_version(os.path.dirname(os.path.abspath(__file__)))

    reader = SocketReader()
    # the window and png backends bring their own taps (mouse, keys, timer)
    poll_events = getattr(screen, "poll_events", None)
    if poll_events is None:
        touch = TouchWatcher(screen_size=(width, height))
    else:
        touch = display.PointerTaps()
    net_monitor = NetworkMonitor(iface=NET_IFACE)

    views = {}  # device_id -> DeviceView, built lazily on first sight
    page = 0
    last_taps = 0
    last_tap_at = None

    # agents: pack, looks and tracker; states of the last line for the focus jump
    scene = AgentScene(load_pack())
    sorted_sessions = []
    last_line_no = 0
    prev_states = None
    # current page key and the focus jump (layout.step_focus decides)
    focus = focus_state(("overview", None))
    transition_until = 0.0
    prev_frame = None
    frame_b = pygame.Surface((width, height), depth=16, masks=surface.get_masks())

    showing_problem = False
    net_icon_center = (width - 28, height - 28)
    net_icon_tap_radius = 22

    clock = pygame.time.Clock()
    t0 = time.time()

    # last drawing error logged and when, so the same error is not logged every frame
    last_draw_error = None
    last_draw_error_logged_at = 0.0

    try:
        while True:
            now = time.time()

            # SDL turns SIGINT/SIGTERM into a QUIT event rather than killing
            # the process outright -- drain it here so `systemctl stop` (and
            # Ctrl+C) actually exit the loop instead of hanging until SIGKILL.
            events = pygame.event.get()
            for event in events:
                if event.type == pygame.QUIT:
                    return
            # preview backends: clicks and timer taps, N for the network screen, quit
            if poll_events is not None:
                for action in poll_events(events):
                    if action[0] == "quit":
                        return
                    if action[0] == "tap":
                        touch.tap(action[1])
                    elif action[0] == "net":
                        showing_problem = net_monitor.toggle_forced()

            devices = sorted(reader.devices, key=lambda d: d.get("device_id", ""))

            # a new socket line: re-sort sessions, track new and leaving ones,
            # and jump to the agents page when one newly needs you
            line_no = reader.line_no
            if line_no != last_line_no:
                last_line_no = line_no
                sorted_sessions = sort_agents(reader.sessions)
                scene.update(sorted_sessions, now)
                new_states = {s.get("sid"): s.get("state") for s in sorted_sessions}
                focus = step_focus(focus, {"kind": "line", "prev_states": prev_states, "new_states": new_states,
                                           "last_tap": last_tap_at, "blocked": showing_problem}, now)
                prev_states = new_states

            # the page ring: overview, agents pages, devices
            agent_rows_all = scene.rows(sorted_sessions, now)
            ring = page_ring(len(devices), len(agent_rows_all))
            keys = page_keys(ring, devices)
            # an expired focus returns; a page that has gone falls back
            focus = step_focus(focus, {"kind": "tick", "keys": keys}, now)

            if touch.taps != last_taps:
                last_tap_at = now
                if showing_problem:
                    return  # any tap here exits the loop, back to the terminal
                # a tap on the network icon opens the problem screen, any other tap turns the page
                advance = net_monitor.ok or not point_in_circle(touch.last_pos, net_icon_center, net_icon_tap_radius)
                if advance:
                    prev_frame = pygame.Surface((width, height), depth=16, masks=surface.get_masks())
                    prev_frame.blit(surface, (0, 0))
                    transition_until = now + TRANSITION_SECS
                else:
                    showing_problem = True
                # any tap ends the focus jump
                focus = step_focus(focus, {"kind": "tap", "keys": keys, "advance": advance}, now)
            last_taps = touch.taps
            page_key = focus["page_key"]
            page = keys.index(page_key)
            total_pages = len(ring)
            kind, value = ring[page]

            # Draw this frame. One bad device (odd text, huge numbers) must not
            # kill the display loop, so any drawing error is caught, logged
            # and replaced by a plain error screen; a tap still moves on.
            try:
                if showing_problem:
                    draw_network_problem(surface, fonts, net_monitor.reason, width, height)
                else:
                    # dispatch to the registered panel for this page kind; the
                    # frame carries the data and fonts the draw needs
                    frame = PanelFrame(fonts, devices, reader.connected, sorted_sessions, scene,
                                       agent_rows_all, keys, page_key, views, version, t0, now, width, height)
                    PANEL_DRAW[kind](surface, value, frame)
                    draw_page_dots(surface, page, total_pages, width, height)

                    if not net_monitor.ok:
                        draw_network_icon(surface, net_icon_center, CRIT)

                    if now < transition_until and prev_frame is not None:
                        progress = 1 - max(0.0, (transition_until - now) / TRANSITION_SECS)
                        frame_b.blit(surface, (0, 0))
                        prev_frame.set_alpha(int(255 * (1 - progress)))
                        frame_b.set_alpha(int(255 * progress))
                        surface.fill(BG)
                        surface.blit(prev_frame, (0, 0))
                        surface.blit(frame_b, (0, 0))
            except Exception as error:
                # log every 30 s, or sooner for a new error but at most once a
                # second, so even an error text that changes every frame
                # cannot flood the journal
                error_text = repr(error)
                is_new_error = error_text != last_draw_error
                seconds_since_log = now - last_draw_error_logged_at
                is_log_due = seconds_since_log >= DRAW_ERROR_LOG_EVERY_SECS
                is_new_and_allowed = is_new_error and seconds_since_log >= 1.0
                if is_log_due or is_new_and_allowed:
                    print(f"dashboard: draw error on page {page}: {error_text}", file=sys.stderr, flush=True)
                    traceback.print_exc(file=sys.stderr)
                    last_draw_error = error_text
                    last_draw_error_logged_at = now

                # a tap on the network-problem screen exits, so do not offer "skip" there
                if showing_problem:
                    fallback_text = "Cannot draw this page"
                else:
                    fallback_text = "Cannot draw this page (tap to skip)"

                # fixed text only here, so bad device data cannot break the error screen
                surface.fill(BG)
                msg = fonts["mid"].render(fallback_text, True, CRIT)
                surface.blit(msg, (width // 2 - msg.get_width() // 2, height // 2 - 10))

            screen.flip()
            # 5 fps only while characters animate on the agents page
            if kind == "agents" and scene.pack is not None and not showing_problem:
                clock.tick(AGENTS_FPS)
            else:
                clock.tick(FPS)
    except KeyboardInterrupt:
        pass
    finally:
        screen.close()


if __name__ == "__main__":
    main()
