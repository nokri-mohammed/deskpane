# Touchscreen hardware notes

The display is an Elegoo 3.5" SPI touchscreen (elegoo.com), a clone of the
common "LCD35"/MHS35 reference design: ILI9486 display controller (480x320,
8:5), XPT2046 touch controller (register-compatible with ADS7846, so the
in-kernel `ads7846` driver handles it fine). It plugs onto the Pi's 40-pin
GPIO header via a 26-pin (13x2) socket — normal for this size of board, not
a wiring mismatch; the 40-pin header is backward-compatible with the older
26-pin layout for the pins this board actually uses.

## GPIO pin table (from the official Elegoo manual)

| Function | Physical pin | BCM GPIO |
|---|---|---|
| TP_IRQ (touch interrupt) | 11 | 17 |
| LCD_RS / DC (data/command) | 18 | 24 |
| LCD_SI / TP_SI (SPI MOSI, shared) | 19 | 10 |
| TP_SO (SPI MISO, touch) | 21 | 9 |
| RST (reset, shared) | 22 | 25 |
| LCD_SCK / TP_SCK (SPI clock, shared) | 23 | 11 |
| LCD_CS (display chip select) | 24 | 8 (SPI0 CE0) |
| TP_CS (touch chip select) | 26 | 7 (SPI0 CE1) |

No backlight GPIO exists in this board's pinout — it's hardwired on.

## What's actually installed, and why

`/boot/firmware/config.txt`:
```
dtoverlay=tft35a:rotate=0
```

This is **not** the generic mainline `ili9486` fbtft chip definition —
it's a precompiled device-tree overlay (`tft35a.dtbo`, installed
to `/boot/firmware/overlays/`; not shipped in this repo because the vendor repo has no
license, `scripts/provision-pi.sh` downloads it pinned to one commit and checks its hash) taken from the vendor's own driver project,
[goodtft/LCD-show](https://github.com/goodtft/LCD-show) (their `LCD35-show`
script references it as `tft35a`). It uses the same in-kernel `fbtft`
framework (`compatible = "ilitek,ili9486"`, same driver we started with),
but bundles a **vendor-specific 15-parameter gamma/init sequence** (the
ILI9486 `0xE0`/`0xE1` gamma correction commands) that the generic mainline
chip definition doesn't include. Without it, the panel "works" — correct
geometry, correct pixel format, touch registers fine — but has washed-out
blacks (near-black renders as visible light grey) and a blue-ish cast, and
curved/detailed content (anti-aliased shapes, text) shows a fine-line
static-like texture that flat/striped content doesn't. The generic overlay
is still a reasonable *starting point* on a different clone panel, but if
you see this exact symptom, look for the vendor's own overlay first before
chasing it as a software bug — it isn't one.

Do **not** run `LCD35-show` (or any script from that repo) as-is on a
modern Raspberry Pi OS (Trixie/bookworm+): it targets the old `/boot/`
path (this OS uses `/boot/firmware/`), and it does a lot of unwanted stuff
for a headless setup (removes X11 config, forces HDMI hotplug/mode,
installs fbcp/cmake, installs X11 input packages, rewrites `/etc/inittab`,
auto-reboots). We only took the one file we needed
(`usr/tft35a-overlay.dtb`) and wired it in ourselves via
`scripts/provision-pi.sh`.

### `rotate=0`, not `rotate=90` — rotation is done in software

Both the generic overlay and this vendor overlay **corrupt the frame**
when asked to rotate 90° at the kernel/overlay level on this kernel
(6.18, Raspberry Pi OS Trixie) — symptoms seen: a blank 1/3 strip with
stretched content, or content repeated ~3x vertically. This reproduced
identically with two completely different overlays, so it's a kernel/fbtft
quirk for this setup, not a bad parameter choice.

Fix: leave the overlay at its native portrait orientation (`rotate=0`,
exposing a real `320x480` framebuffer, confirmed via
`fbset -fb /dev/fb0 -i`), and rotate the finished frame in software instead
— see `pi/fb_display.py`, `FrameBuffer.flip()` (`pygame.transform.rotate()`
before the raw-byte write). The app still draws landscape
(`dashboard.py`/`touch.py` are unaware of any of this).

### Console kept off the touchscreen (`fbcon=map:1`)

`/boot/firmware/cmdline.txt` has `fbcon=map:1` appended. With no HDMI
monitor attached, `vc4` never registers a framebuffer, so the touchscreen
becomes the *only* one — and the kernel's text console/login prompt
(`getty@tty1`) will bind to it by default and periodically repaint over
whatever the app writes, corrupting it. `fbcon=map:1` points the console at
a framebuffer index that doesn't exist, so it simply doesn't bind anywhere,
leaving the touchscreen exclusively for the app. See `RECOVERY.md` for what
this means if you need a console via HDMI later.

`fbcon=map:1` alone wasn't fully reliable: depending on driver probe
timing, a **second** VT console (`vtcon1`, separate from the dummy one
`fbcon=map:1` produces) could still bind directly to the real framebuffer
device regardless of the map directive — visible as a blinking text cursor
on screen, positioned wherever the console happened to think its cursor
was. Confirm with `cat /sys/class/vtconsole/vtcon*/name` and
`.../bind` — anything not `(S) dummy device` with `bind=1` is a live
console on a real fb. Fixed with a small oneshot systemd service,
`unbind-fbcon.service` (installed by `provision-pi.sh`), that unbinds any
non-dummy vtconsole at boot, before the dashboard starts.

### Frame writes use `os.pwrite()`, not `mmap`

`fb_display.py` writes each frame with a single `os.pwrite()` call, going
through the kernel's normal `fb_write()` path (the same one the console
itself uses successfully) rather than `mmap` + manual paging, which relies
on page-fault-triggered deferred I/O and a background flush timer that can
race with a single large (~300KB) write and send a half-updated frame over
SPI. This wasn't actually the root cause of the problems diagnosed on
2026-11-06 (the gamma/rotate issues above were), but it's a more
deterministic write path and there's no reason to prefer `mmap` here.

## Diagnostic method, if this ever breaks again

If the screen is blank/white/garbled, isolate layer by layer rather than
guessing at overlay parameters:
1. Write a trivial solid color straight to `/dev/fbN` with `os.pwrite()`,
   no pygame involved. Confirms wiring/overlay/pixel-format/write-path.
2. Write a structured raw pattern (bands, stripes, a diagonal line) the
   same way. Confirms geometry/stride/rotation, since a solid fill can't
   reveal row/column ordering bugs.
3. Only once (1) and (2) are clean, test actual pygame-rendered content
   through the real code path. If *that* alone breaks while (1)/(2) don't,
   suspect the panel's gamma/init tuning (vendor overlay) before suspecting
   the app's code.
