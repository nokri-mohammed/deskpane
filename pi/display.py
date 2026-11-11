"""Display backends for the dashboard (T-061). DESKPANE_DISPLAY picks one:

    fb (default)  fb_display.FrameBuffer, the Pi's SPI panel, taps from evdev
    window        a desktop window (WSLg, X11 or Wayland), 480 x 320 times
                  DESKPANE_SCALE; a mouse click is a tap
    png           offscreen; every frame goes to latest.png, and numbered
                  frames when a frame count is given

Every backend has surface_for_drawing(), flip() and close(); window and png
also have poll_events(events), which turns pygame events into actions for
the dashboard loop: ("tap", (x, y)), ("net",) or ("quit",).

All of them draw on a 16-bit surface with the panel's RGB565 masks, so the
colours and pixels match the panel. fb_display is imported only for the fb
backend: it sets SDL_VIDEODRIVER=dummy at import, which would stop a window.
"""
import os
import sys
import time

import pygame

from layout import pick_backend, pick_scale, scale_tap

WIDTH, HEIGHT = 480, 320
# same as fb_display.MASKS (rgba 5/11, 6/5, 5/0); repeated here so the
# window backend does not import fb_display before SDL picks a video driver
MASKS = (0xF800, 0x07E0, 0x001F, 0)
# a tap from the keyboard or the timer lands in the middle, away from the
# network icon in the bottom right corner
CENTER_TAP = (WIDTH // 2, HEIGHT // 2)


def log(message):
    print("dashboard: " + message, file=sys.stderr, flush=True)


def panel_surface():
    return pygame.Surface((WIDTH, HEIGHT), depth=16, masks=MASKS)


def save_png(surface, path):
    """Writes a PNG through a temp file and a rename, so a reader never
    sees half a file."""
    temp = path + ".part.png"
    pygame.image.save(surface, temp)
    os.replace(temp, path)


class PointerTaps:
    """The same taps and last_pos as touch.TouchWatcher, fed by the window
    or the timer instead of evdev."""

    def __init__(self):
        self.taps = 0
        self.last_pos = (0, 0)

    def tap(self, pos):
        self.last_pos = pos
        self.taps += 1

    @property
    def available(self):
        return True


class WindowDisplay:
    """A desktop window. Click, Space or Right arrow taps; N toggles the
    network problem screen; S saves a PNG; Q or Esc quits."""

    def __init__(self, scale, shots_dir):
        pygame.display.init()
        pygame.font.init()
        self.scale = scale
        self._shots_dir = shots_dir
        self._window = pygame.display.set_mode((WIDTH * scale, HEIGHT * scale))
        pygame.display.set_caption("deskpane preview (click: tap, N: network, S: save, Q: quit)")
        self.surface = panel_surface()

    def surface_for_drawing(self):
        return self.surface

    def flip(self):
        # nearest neighbour, so each panel pixel is a clean square
        scaled = pygame.transform.scale(self.surface, self._window.get_size())
        self._window.blit(scaled, (0, 0))
        pygame.display.flip()

    def save_shot(self):
        os.makedirs(self._shots_dir, exist_ok=True)
        path = os.path.join(self._shots_dir, time.strftime("shot-%Y%m%d-%H%M%S.png"))
        save_png(self.surface, path)
        log("saved " + path)

    def poll_events(self, events):
        actions = []
        for event in events:
            if event.type == pygame.MOUSEBUTTONDOWN and event.button == 1:
                actions.append(("tap", scale_tap(event.pos, self.scale, (WIDTH, HEIGHT))))
            elif event.type == pygame.KEYDOWN:
                if event.key in (pygame.K_SPACE, pygame.K_RIGHT):
                    actions.append(("tap", CENTER_TAP))
                elif event.key == pygame.K_n:
                    actions.append(("net",))
                elif event.key == pygame.K_s:
                    self.save_shot()
                elif event.key in (pygame.K_q, pygame.K_ESCAPE):
                    actions.append(("quit",))
        return actions

    def close(self):
        pygame.quit()


class PngDisplay:
    """Offscreen frames: latest.png on every flip, plus frame-0001.png and
    on when frames > 0 (then it quits after that many). tap_every > 0 taps
    on a timer, so a headless run walks through the pages."""

    def __init__(self, out_dir, frames=0, tap_every=0.0, clock=time.monotonic):
        os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
        os.environ.setdefault("SDL_AUDIODRIVER", "dummy")
        pygame.display.init()
        pygame.font.init()
        os.makedirs(out_dir, exist_ok=True)
        self.surface = panel_surface()
        self._out_dir = out_dir
        self._frames = frames
        self._written = 0
        self._tap_every = tap_every
        self._clock = clock
        self._next_tap = clock() + tap_every

    def surface_for_drawing(self):
        return self.surface

    def flip(self):
        save_png(self.surface, os.path.join(self._out_dir, "latest.png"))
        if self._frames > 0 and self._written < self._frames:
            self._written += 1
            save_png(self.surface, os.path.join(self._out_dir, "frame-%04d.png" % self._written))

    def poll_events(self, events):
        if self._frames > 0 and self._written >= self._frames:
            return [("quit",)]
        actions = []
        now = self._clock()
        if self._tap_every > 0 and now >= self._next_tap:
            self._next_tap = now + self._tap_every
            actions.append(("tap", CENTER_TAP))
        return actions

    def close(self):
        pygame.quit()


def open_framebuffer():
    """The Pi panel, found by its driver name as before (FB_PATH wins)."""
    import fb_display
    path = os.environ.get("FB_PATH") or fb_display.detect_touchscreen_fb() or fb_display.FB_PATH
    return fb_display.FrameBuffer(path=path)


def default_out_dir():
    cache = os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache")
    return os.path.join(cache, "deskpane-preview")


def open_display(backend=None, scale=None, out_dir=None, frames=0, tap_every=0.0):
    """The display for a backend name, or for DESKPANE_DISPLAY and
    DESKPANE_SCALE when they are not given. The Pi sets neither: fb."""
    if backend is None:
        backend, note = pick_backend(os.environ.get("DESKPANE_DISPLAY"))
        if note is not None:
            log(note)
    if scale is None:
        scale = pick_scale(os.environ.get("DESKPANE_SCALE"))
    if out_dir is None:
        out_dir = default_out_dir()
    if backend == "window":
        return WindowDisplay(scale, out_dir)
    if backend == "png":
        return PngDisplay(out_dir, frames, tap_every)
    return open_framebuffer()
