"""Renders a pygame Surface straight onto a Linux framebuffer device (RGB565)."""
import glob
import os
os.environ.setdefault("SDL_VIDEODRIVER", "dummy")
os.environ.setdefault("SDL_AUDIODRIVER", "dummy")

import pygame

FB_PATH = "/dev/fb1"
WIDTH, HEIGHT = 480, 320  # logical/app-facing size -- dashboard.py always draws landscape
# Confirmed via `fbset -fb /dev/fb0 -i` on the real panel: the ili9486 overlay
# exposes a native-portrait device (320x480, LineLength 640), not landscape --
# kernel-level rotate=90 corrupted the GRAM addressing, so rotation is done
# here in software instead (rotate the finished landscape frame before the
# raw-byte write) rather than at the fbtft/overlay level.
PHYSICAL_SIZE = (320, 480)
# RGB565 masks matching `fbset -fb /dev/fbN -i` output (rgba 5/11,6/5,5/0)
MASKS = (0xF800, 0x07E0, 0x001F, 0)
TOUCHSCREEN_DRIVER_NAME = "fb_ili9486"


def detect_touchscreen_fb():
    """fb numbering isn't stable across boots -- it's a race between this
    panel's driver and the HDMI output for fb0 vs fb1 -- so find it by its
    driver name under /sys/class/graphics instead of trusting a fixed path.
    Returns None if it's not up yet (caller falls back to FB_PATH)."""
    for name_path in sorted(glob.glob("/sys/class/graphics/fb*/name")):
        try:
            with open(name_path) as f:
                if f.read().strip() == TOUCHSCREEN_DRIVER_NAME:
                    return "/dev/" + name_path.split("/")[-2]
        except OSError:
            continue
    return None


class FrameBuffer:
    def __init__(self, path=FB_PATH, size=(WIDTH, HEIGHT), physical_size=PHYSICAL_SIZE, rotate=-90):
        pygame.display.init()
        pygame.font.init()
        self.size = size
        self.surface = pygame.Surface(size, depth=16, masks=MASKS)
        # rotate=0 when the device's physical geometry already matches `size`
        # (e.g. dev/testing against a plain file); real panel needs rotating
        # since the device is native-portrait but the app draws landscape.
        self._rotate = rotate if physical_size != size else 0
        expected = physical_size[0] * physical_size[1] * 2
        self._fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)
        try:
            # Real fb devices have a fixed size already; this is only to
            # make a plain-file stand-in (used in dev/testing) big enough.
            os.ftruncate(self._fd, expected)
        except OSError:
            pass
        self._expected = expected

    def surface_for_drawing(self):
        return self.surface

    def flip(self):
        frame = pygame.transform.rotate(self.surface, self._rotate) if self._rotate else self.surface
        buf = frame.get_buffer().raw
        if len(buf) != self._expected:
            raise RuntimeError(
                f"surface buffer is {len(buf)} bytes, framebuffer expects {self._expected} "
                "(pitch/padding mismatch)"
            )
        # fbtft's fb_write() handler schedules the SPI flush synchronously
        # right after a write() completes -- the same path the kernel's own
        # console uses, and the console renders cleanly. mmap instead relies
        # on page-fault-triggered deferred I/O + a background timer, which
        # can fire mid-write on a single large (307KB) frame and send a
        # half-updated buffer over SPI -- that race is what produced the
        # torn/ghosted frames despite a clean console on the same bus.
        os.pwrite(self._fd, buf, 0)

    def close(self):
        os.close(self._fd)
        pygame.quit()
