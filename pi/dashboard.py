"""Dashboard: reads the daemon's device-status broadcast over a Unix
socket and renders it onto the Pi's framebuffer touchscreen, in the
animated ring-gauge style of the original single-device prototype.
Page 0 is an overview (clock, date, device count); pages after that are
one per device. Tap the screen (see touch.py) to cycle pages.
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

from fb_display import FrameBuffer, detect_touchscreen_fb
from touch import TouchWatcher

SOCKET_PATH = os.environ.get("SOCKET_PATH", "/run/deskpane/daemon.sock")
FB_PATH = os.environ.get("FB_PATH") or detect_touchscreen_fb() or "/dev/fb1"

# A full 320x480 RGB565 frame (307KB) takes ~150ms to physically transfer
# over the SPI bus at 16MHz. Redrawing faster than that interrupts the panel
# mid-transfer and blends two frames together (the "ghosted/obfuscated"
# look) -- this is a dashboard, not an animation, so a low FPS costs nothing.
FPS = 3
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
                        devices = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    with self._lock:
                        self._devices = devices
            except OSError:
                pass
            self._connected = False
            time.sleep(1)

    @property
    def devices(self):
        with self._lock:
            return list(self._devices)

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
        threading.Thread(target=self._loop, daemon=True).start()

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
            return self._ok

    @property
    def reason(self):
        with self._lock:
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


def overview_layout(width, height):
    """Fixed screen positions for the overview page."""
    clock_y = height // 2 - 76
    date_y = clock_y + 72
    summary_y = date_y + 26
    divider_y = summary_y + 26
    return {
        "clock_y": clock_y,
        "date_y": date_y,
        "summary_y": summary_y,
        "divider_y": divider_y,
    }


def draw_overview(surface, fonts, devices, connected, t0, now, width, height):
    surface.fill(BG)
    layout = overview_layout(width, height)

    clock_surf = fonts["clock"].render(time.strftime("%H:%M"), True, TEXT)
    clock_y = layout["clock_y"]
    surface.blit(clock_surf, (width // 2 - clock_surf.get_width() // 2, clock_y))

    date_surf = fonts["small"].render(time.strftime("%A, %d %B %Y"), True, SUBTEXT)
    date_y = layout["date_y"]
    surface.blit(date_surf, (width // 2 - date_surf.get_width() // 2, date_y))

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
    summary_y = layout["summary_y"]
    summary_x = width // 2 - summary_surf.get_width() // 2
    surface.blit(summary_surf, (summary_x, summary_y))
    if connected and total_count:
        draw_pulse_dot(surface, (summary_x - 12, summary_y + summary_surf.get_height() // 2), t0, now)

    divider_y = layout["divider_y"]
    pygame.draw.line(surface, (40, 42, 50), (40, divider_y), (width - 40, divider_y), 1)


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

        host_surf = font_mid.render(device.get("hostname", ""), True, TEXT)
        surface.blit(host_surf, (14, 10))
        tag_surf = font_tiny.render(device.get("label", ""), True, ACCENT)
        surface.blit(tag_surf, (14, 36))
        ip_surf = font_small.render(device.get("ip", ""), True, SUBTEXT)
        surface.blit(ip_surf, (14 + tag_surf.get_width() + 8, 35))

        clock_str = time.strftime("%H:%M:%S")
        clk_surf = font_small.render(clock_str, True, SUBTEXT)
        surface.blit(clk_surf, (width - clk_surf.get_width() - 14, 14))

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


def main():
    fb = FrameBuffer(path=FB_PATH)
    surface = fb.surface_for_drawing()
    width, height = surface.get_size()

    fonts = {
        "clock": pygame.font.Font(None, 84),
        "big": pygame.font.Font(None, 40),
        "mid": pygame.font.Font(None, 26),
        "small": pygame.font.Font(None, 20),
        "tiny": pygame.font.Font(None, 16),
    }

    reader = SocketReader()
    touch = TouchWatcher(screen_size=(width, height))
    net_monitor = NetworkMonitor()

    views = {}  # device_id -> DeviceView, built lazily on first sight
    page = 0
    last_taps = 0
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
            for event in pygame.event.get():
                if event.type == pygame.QUIT:
                    return

            devices = sorted(reader.devices, key=lambda d: d.get("device_id", ""))
            total_pages = 1 + len(devices)  # page 0 is always the overview

            if touch.taps != last_taps:
                if showing_problem:
                    return  # any tap here exits the loop, back to the terminal
                elif not net_monitor.ok and point_in_circle(touch.last_pos, net_icon_center, net_icon_tap_radius):
                    showing_problem = True
                else:
                    prev_frame = pygame.Surface((width, height), depth=16, masks=surface.get_masks())
                    prev_frame.blit(surface, (0, 0))
                    page = (page + 1) % total_pages
                    transition_until = now + TRANSITION_SECS
            last_taps = touch.taps

            # Draw this frame. One bad device (odd text, huge numbers) must not
            # kill the display loop, so any drawing error is caught, logged
            # and replaced by a plain error screen; a tap still moves on.
            try:
                if showing_problem:
                    draw_network_problem(surface, fonts, net_monitor.reason, width, height)
                else:
                    page %= total_pages
                    if page == 0:
                        draw_overview(surface, fonts, devices, reader.connected, t0, now, width, height)
                    else:
                        device = devices[page - 1]
                        view = views.setdefault(device["device_id"], DeviceView())
                        view.ease(device)
                        view.render(surface, device, fonts, t0, now, width, height)
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

            fb.flip()
            clock.tick(FPS)
    except KeyboardInterrupt:
        pass
    finally:
        fb.close()


if __name__ == "__main__":
    main()
