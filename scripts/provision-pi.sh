#!/usr/bin/env bash
# Pushes this repo's built software onto a freshly-flashed Pi and wires it up
# to survive reboots via systemd. Run from the repo root, after:
#   1. Flashing the SD card with Raspberry Pi OS Lite (64-bit) using
#      Raspberry Pi Imager's gear-icon customization (hostname, SSH,
#      username/password, WiFi SSID/password) so the Pi comes up on the
#      network with no manual boot-partition editing.
#   2. Building the Rust binaries for the Pi's architecture:
#        cargo build --release --target aarch64-unknown-linux-gnu
#   3. Having an SSH alias for the Pi (this repo's docs assume `pi-display`
#      in ~/.ssh/config) that resolves and accepts your key.
#   4. Passwordless sudo for apt-get/tee/systemctl, since this script runs
#      sudo non-interactively over SSH. On the Pi:
#        echo 'USERNAME ALL=(root) NOPASSWD: /usr/bin/apt-get, /usr/bin/tee, /usr/bin/systemctl' \
#          | sudo tee /etc/sudoers.d/homelab-dashboard
#        sudo chmod 440 /etc/sudoers.d/homelab-dashboard
#
# Usage: scripts/provision-pi.sh [ssh-host]   (default: pi-display)
#
# NOTE: a display overlay/console change applied here only takes effect
# after a reboot -- the script will tell you at the end if one is needed.
set -euo pipefail

HOST="${1:-pi-display}"
REMOTE_DIR="pi-homelab-dashboard"
BIN_DIR="target/aarch64-unknown-linux-gnu/release"

for bin in daemon reporter; do
  if [[ ! -f "$BIN_DIR/$bin" ]]; then
    echo "error: $BIN_DIR/$bin not found — build it first:" >&2
    echo "  cargo build --release --target aarch64-unknown-linux-gnu" >&2
    exit 1
  fi
done

echo "==> checking SSH connectivity to $HOST"
ssh -o ConnectTimeout=5 "$HOST" "mkdir -p ~/$REMOTE_DIR"

echo "==> copying dashboard + binaries"
scp -q pi/dashboard.py pi/fb_display.py pi/touch.py pi/assets/tft35a.dtbo \
    "$BIN_DIR/daemon" "$BIN_DIR/reporter" \
    "$HOST:~/$REMOTE_DIR/"

REMOTE_USER="$(ssh "$HOST" 'echo $USER')"
REMOTE_HOME="$(ssh "$HOST" 'echo $HOME')"

echo "==> installing system dependencies (python3-pygame, python3-evdev)"
ssh "$HOST" "sudo apt-get update -qq && sudo apt-get install -y -qq python3-pygame python3-evdev"

echo "==> installing display overlay (Elegoo 3.5\" SPI touchscreen, ILI9486+XPT2046)"
ssh "$HOST" "cat ~/$REMOTE_DIR/tft35a.dtbo | sudo tee /boot/firmware/overlays/tft35a.dtbo > /dev/null"

REBOOT_NEEDED=0
if ssh "$HOST" "grep -q '^dtoverlay=tft35a' /boot/firmware/config.txt"; then
  echo "    config.txt already has the tft35a overlay"
else
  echo "    adding tft35a overlay to config.txt"
  ssh "$HOST" "printf '\n# Elegoo 3.5\" SPI touchscreen (ILI9486 + XPT2046 touch) -- vendor-tuned\n# gamma/init overlay from goodtft/LCD-show (see pi/assets/tft35a.dtbo).\n# rotate=0: kernel-level rotate=90 is buggy on this driver/kernel combo\n# (corrupts the frame), so rotation is done in software instead, in\n# fb_display.py.\ndtoverlay=tft35a:rotate=0\n' | sudo tee -a /boot/firmware/config.txt > /dev/null"
  REBOOT_NEEDED=1
fi

if ssh "$HOST" "grep -q 'fbcon=map:1' /boot/firmware/cmdline.txt"; then
  echo "    cmdline.txt already keeps the text console off the touchscreen"
else
  echo "    keeping the text console off the touchscreen framebuffer"
  ssh "$HOST" 'content=$(cat /boot/firmware/cmdline.txt); printf "%s fbcon=map:1" "$content" | sudo tee /boot/firmware/cmdline.txt > /dev/null'
  REBOOT_NEEDED=1
fi

# fbcon=map:1 alone isn't always enough: depending on driver probe timing a
# second VT console can still bind directly to the touchscreen fb (seen as
# a blinking text cursor on screen), bypassing the map directive. Belt and
# braces: unbind any non-dummy vtconsole at boot, before the dashboard starts.
ssh "$HOST" "sudo tee /etc/systemd/system/unbind-fbcon.service > /dev/null" << 'EOF'
[Unit]
Description=Unbind text console from the touchscreen framebuffer
After=systemd-udev-settle.service
Before=homelab-dashboard.service
Wants=systemd-udev-settle.service

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'for v in /sys/class/vtconsole/vtcon*; do grep -q dummy "$v/name" || echo 0 > "$v/bind"; done'
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF
ssh "$HOST" "sudo systemctl daemon-reload && sudo systemctl enable --now unbind-fbcon"

echo "==> writing systemd units"
ssh "$HOST" "chmod +x ~/$REMOTE_DIR/daemon ~/$REMOTE_DIR/reporter"

ssh "$HOST" "sudo tee /etc/systemd/system/homelab-daemon.service > /dev/null" << EOF
[Unit]
Description=pi-homelab-dashboard report daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$REMOTE_USER
WorkingDirectory=$REMOTE_HOME/$REMOTE_DIR
ExecStart=$REMOTE_HOME/$REMOTE_DIR/daemon
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

ssh "$HOST" "sudo tee /etc/systemd/system/homelab-dashboard.service > /dev/null" << EOF
[Unit]
Description=pi-homelab-dashboard touchscreen UI
After=homelab-daemon.service
Wants=homelab-daemon.service

[Service]
Type=simple
User=$REMOTE_USER
WorkingDirectory=$REMOTE_HOME/$REMOTE_DIR
ExecStart=/usr/bin/python3 $REMOTE_HOME/$REMOTE_DIR/dashboard.py
# on-failure, not always: a tap-to-exit (clean exit 0) should stay exited,
# only a crash (nonzero exit) should auto-restart.
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

ssh "$HOST" "sudo tee /etc/systemd/system/homelab-reporter.service > /dev/null" << EOF
[Unit]
Description=pi-homelab-dashboard reporter (reports the Pi's own stats)
After=network-online.target homelab-daemon.service
Wants=network-online.target

[Service]
Type=simple
User=$REMOTE_USER
WorkingDirectory=$REMOTE_HOME/$REMOTE_DIR
ExecStart=$REMOTE_HOME/$REMOTE_DIR/reporter run --daemon 127.0.0.1:8765 --device-id pi --label "Raspberry Pi" --interval 2
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

echo "==> enabling + starting services"
ssh "$HOST" "sudo systemctl daemon-reload && sudo systemctl enable --now homelab-daemon homelab-dashboard homelab-reporter"

echo "==> done. Checking status:"
ssh "$HOST" "systemctl --no-pager status homelab-daemon homelab-dashboard homelab-reporter | grep -E 'Loaded|Active|●'"

if [[ "${REBOOT_NEEDED:-0}" -eq 1 ]]; then
  echo
  echo "==> config.txt/cmdline.txt changed (display overlay or console mapping)."
  echo "    These only take effect after a reboot: ssh $HOST sudo systemctl reboot"
fi
