# Recovering the Pi if WiFi breaks

Goal: never be forced to reflash the SD card just because the Pi can't join
WiFi. There's always a local-console path back in.

## Local console access (works today, needs one extra step if using HDMI)

Updated 2026-11-06: the SPI touchscreen is now wired up and is the Pi's
*only* framebuffer when no HDMI monitor is attached (`vc4` never registers
one without a monitor present), so `getty@tty1` is active + enabled but
**deliberately kept off that framebuffer** via `fbcon=map:1` in
`/boot/firmware/cmdline.txt`. Otherwise the kernel's text console and the
dashboard app fight over the same device and both end up corrupted (this
was fully diagnosed and reproduced on 2026-11-06; see `HARDWARE.md`).

This means:
- The touchscreen never shows the console/login prompt. Only the
  dashboard app ever draws to it. That's intentional.
- **If you plug in an HDMI monitor for recovery, it won't show a console
  either**, because `fbcon=map:1` points the console at a nonexistent
  framebuffer index regardless of which device actually exists. To get a
  console on HDMI:
  1. SSH in if you can, and run `sudo con2fbmap 1 <hdmi-fb-number>`
     (check `/sys/class/graphics/fb*/name` for the one that *isn't*
     `fb_ili9486`). This takes effect immediately, no reboot, but doesn't
     persist.
  2. Or, if you can't SSH in at all, remove `fbcon=map:1` from
     `/boot/firmware/cmdline.txt` on the SD card (another machine) and
     reboot. The console will then bind to whichever fb comes up first,
     which could again be the touchscreen depending on probe order.
- No autologin is configured. Log in with the username and password you set when flashing the SD card
  once you have a console.

**So: plug in a monitor + USB keyboard, do the `con2fbmap`/cmdline step if
needed, log in, fix networking, done.** No reflash needed. SSH remains the
simplest recovery path when WiFi itself isn't the thing that's broken.

### Recovery commands (run at the local console)

```bash
nmcli device wifi list                                   # see what's in range
sudo nmcli device wifi connect "<SSID>" password "<pw>"   # join a network
nmcli connection show                                     # check known profiles / autoconnect
sudo nmcli connection up "<home-network>"                  # retry the saved home network
sudo systemctl restart NetworkManager                      # if it's just stuck
journalctl -u NetworkManager -b --no-pager | tail -50       # see why it's failing
```

## Confirmed: WiFi auto-reconnects on its own after a power cycle

Verified 2026-11-06 by rebooting the Pi and polling for it to come back:
- The saved home-network connection has `autoconnect: yes`, unlimited retries
  (`connection.autoconnect-retries: -1`), `ipv4.method: auto` (DHCP).
- After `sudo reboot`, the Pi was back on the network (ping + SSH) within
  **41 seconds**, no manual steps, no retries/hiccups in
  `journalctl -u NetworkManager`.
- Persistent journal logging is enabled (`/var/log/journal` exists), so
  boot-time network issues can always be inspected after the fact with
  `journalctl -b 0 -u NetworkManager`.

## Planned: USB Ethernet gadget mode (not set up yet)

Once a USB-OTG cable is bought, the Pi Zero 2 W's data port can present
itself as a wired network device to a laptop (`raspberrypi.local` over USB),
giving SSH access with zero dependency on WiFi at all.

**Not plug-and-play with the current image**: `/boot/firmware/config.txt`
currently has:

```
dtoverlay=dwc2,dr_mode=host
```

`dr_mode=host` forces the OTG port into USB-host mode, which blocks gadget
mode. To enable it later:

1. Change to `dtoverlay=dwc2,dr_mode=peripheral` (or `otg`) in `config.txt`.
2. Add `modules-load=dwc2,g_ether` to `/boot/firmware/cmdline.txt`.
3. Reboot, then connect via USB cable and SSH to `raspberrypi.local`
   (needs `avahi-daemon`, installed by default on Raspberry Pi OS).

## If the one-line install fails half way

The one-line install (`get.sh`) removes an old `pi-homelab-dashboard` install before it runs
`install.sh`. If the install then fails (for example `apt-get` or the display overlay download),
run the one line again; the old install is not restored.

## Not yet covered

- No backup WiFi profile (e.g. a phone hotspot) is stored. If the home
  router/SSID itself is the problem, local console access is still the only
  fallback until one is added.
- No automated self-healing watchdog. Recovery today is manual (console
  login + `nmcli`), by choice, to avoid adding unattended code before it's
  needed.
