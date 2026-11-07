"""Watches the ADS7846 touchscreen for taps and exposes a toggling counter
plus the screen position of the last tap, so callers can tell a tap on a
specific icon/button apart from a tap anywhere else."""
import threading
import time


class TouchWatcher:
    def __init__(self, device_path="/dev/input/event0", debounce_s=0.35, screen_size=(480, 320)):
        self.taps = 0
        self.last_pos = (0, 0)
        self._device_path = device_path
        self._debounce_s = debounce_s
        self._screen_size = screen_size
        self._available = False
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._thread.start()

    def _loop(self):
        try:
            import evdev
            dev = evdev.InputDevice(self._device_path)
            caps = dict(dev.capabilities().get(evdev.ecodes.EV_ABS, []))
            abs_x = caps.get(evdev.ecodes.ABS_X)
            abs_y = caps.get(evdev.ecodes.ABS_Y)
        except Exception:
            self._available = False
            return

        self._available = True
        width, height = self._screen_size
        # Raw touch-controller coordinates, not screen pixels, scaled below.
        # Unverified on real hardware; recheck this mapping once the panel
        # is actually wired up (README: touch not yet confirmed on device).
        x_min, x_max = (abs_x.min, abs_x.max) if abs_x else (0, width)
        y_min, y_max = (abs_y.min, abs_y.max) if abs_y else (0, height)
        raw_x, raw_y = x_min, y_min
        last_tap = 0.0
        touching = False
        for event in dev.read_loop():
            if event.type == evdev.ecodes.EV_ABS:
                if event.code == evdev.ecodes.ABS_X:
                    raw_x = event.value
                elif event.code == evdev.ecodes.ABS_Y:
                    raw_y = event.value
            elif event.type == evdev.ecodes.EV_KEY and event.code == evdev.ecodes.BTN_TOUCH:
                down = event.value == 1
                if down and not touching:
                    now = time.time()
                    if now - last_tap >= self._debounce_s:
                        px = int((raw_x - x_min) / max(1, x_max - x_min) * width)
                        py = int((raw_y - y_min) / max(1, y_max - y_min) * height)
                        self.last_pos = (px, py)
                        self.taps += 1
                        last_tap = now
                touching = down

    @property
    def available(self):
        return self._available
