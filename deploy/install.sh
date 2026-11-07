#!/usr/bin/env bash
# Installs or upgrades deskpane on a Raspberry Pi from an extracted
# release bundle. Safe to run again: running a newer bundle's install.sh is the
# upgrade. Needs 64-bit Raspberry Pi OS Bookworm or later.
#
# Usage: sudo ./install.sh [options]   (see --help)
#
# DESTDIR (empty by default) prefixes every file path, for testing in a fake root.
set -euo pipefail

PREFIX=/opt/deskpane
CONFIG_DIR=/etc/deskpane
UNIT_DIR=/etc/systemd/system
BOOT_DIR=/boot/firmware
SERVICE_USER=deskpane
DESTDIR="${DESTDIR:-}"
BUNDLE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# markers around the lines we add to config.txt, so uninstall.sh can find them
MARK_BEGIN="# >>> deskpane"
MARK_END="# <<< deskpane"

# print a step, a warning, or an error and stop
say() { echo "==> $*"; }
warn() { echo "install.sh: warning: $*" >&2; }
die() {
  echo "install.sh: error: $*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: sudo ./install.sh [options]

  --no-display         headless: no touchscreen overlay, boot config or dashboard
  --no-reporter        do not report the Pi's own stats
  --device-id ID       device id for the Pi's own reporter (first install only),
                       letters, digits, dot, dash, underscore, up to 64
  --label TEXT         label shown on the dashboard (first install only), up to 64
  --overlay-file FILE  use an already downloaded display overlay (hash still checked)
  -h, --help           show this help
EOF
}

# true if the file's SHA-256 matches OVERLAY_SHA256
overlay_hash_ok() {
  echo "$OVERLAY_SHA256  $1" | sha256sum -c --status - 2>/dev/null
}

# value of KEY=value from a plain file (never sourced as shell code)
read_key() {
  sed -n "s/^$1=//p" "$2" | head -n 1
}

# copy a boot file to <file>.deskpane.bak once, before our first change to it
backup_once() {
  if [[ ! -f "$1.deskpane.bak" ]]; then
    cp -p "$1" "$1.deskpane.bak"
  fi
}

# parse flags
WITH_DISPLAY=1
WITH_REPORTER=1
DEVICE_ID=""
LABEL=""
OVERLAY_FILE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-display) WITH_DISPLAY=0 ;;
    --no-reporter) WITH_REPORTER=0 ;;
    --device-id)
      [[ $# -ge 2 ]] || die "--device-id needs a value"
      DEVICE_ID="$2"
      shift
      ;;
    --label)
      [[ $# -ge 2 ]] || die "--label needs a value"
      LABEL="$2"
      shift
      ;;
    --overlay-file)
      [[ $# -ge 2 ]] || die "--overlay-file needs a value"
      OVERLAY_FILE="$2"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
done

# preflight: root, 64-bit, Bookworm boot layout, complete bundle
[[ "$(id -u)" -eq 0 ]] || die "run as root: sudo ./install.sh"
ARCH="$(uname -m)"
if [[ "$ARCH" != "aarch64" ]]; then
  die "this bundle is for 64-bit Raspberry Pi OS (aarch64), this system is $ARCH. Flash Raspberry Pi OS Lite (64-bit)."
fi
if [[ "$WITH_DISPLAY" -eq 1 && ! -f "$DESTDIR$BOOT_DIR/config.txt" ]]; then
  die "$BOOT_DIR/config.txt not found: Raspberry Pi OS Bookworm or later is required (or use --no-display)"
fi
for f in VERSION LICENSE overlay.env config/env.example bin/deskpane-daemon bin/deskpane \
  dashboard/dashboard.py dashboard/fb_display.py dashboard/touch.py \
  systemd/deskpane-daemon.service systemd/deskpane-dashboard.service \
  systemd/deskpane-reporter.service systemd/deskpane-unbind-fbcon.service; do
  [[ -f "$BUNDLE_DIR/$f" ]] || die "bundle is incomplete: $f is missing"
done
VERSION="$(tr -d '[:space:]' <"$BUNDLE_DIR/VERSION")"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || die "bad VERSION file: '$VERSION'"

# the env file is read by systemd, so keep values to safe characters; a leading
# letter or digit means the reporter's argument parser never sees them as flags
if [[ -n "$DEVICE_ID" && ! "$DEVICE_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
  die "--device-id: 1 to 64 letters, digits, dot, dash or underscore, starting with a letter or digit"
fi
LABEL_PATTERN='^[A-Za-z0-9][A-Za-z0-9 ._()-]{0,63}$'
if [[ -n "$LABEL" && ! "$LABEL" =~ $LABEL_PATTERN ]]; then
  die "--label: up to 64 letters, digits, spaces and . _ ( ) -, starting with a letter or digit"
fi

# work directory for downloads, removed on exit
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

# display overlay: get it and check its hash before changing anything on the system
OVERLAY_TMP="$WORK_DIR/tft35a.dtbo"
OVERLAY_TARGET="$DESTDIR$BOOT_DIR/overlays/tft35a.dtbo"
OVERLAY_NEEDED=0
if [[ "$WITH_DISPLAY" -eq 1 ]]; then
  OVERLAY_URL="$(read_key OVERLAY_URL "$BUNDLE_DIR/overlay.env")"
  OVERLAY_SHA256="$(read_key OVERLAY_SHA256 "$BUNDLE_DIR/overlay.env")"
  [[ "$OVERLAY_URL" =~ ^https://raw\.githubusercontent\.com/goodtft/LCD-show/[0-9a-f]{40}/ ]] ||
    die "overlay.env: OVERLAY_URL must point at a pinned goodtft/LCD-show commit"
  [[ "$OVERLAY_SHA256" =~ ^[0-9a-f]{64}$ ]] || die "overlay.env: OVERLAY_SHA256 is not a SHA-256"

  if [[ -f "$OVERLAY_TARGET" ]] && overlay_hash_ok "$OVERLAY_TARGET"; then
    say "display overlay already installed and verified"
  elif [[ -n "$OVERLAY_FILE" ]]; then
    say "checking the display overlay from $OVERLAY_FILE"
    cp "$OVERLAY_FILE" "$OVERLAY_TMP"
    overlay_hash_ok "$OVERLAY_TMP" || die "overlay hash mismatch in $OVERLAY_FILE; nothing was changed"
    OVERLAY_NEEDED=1
  else
    say "downloading the display overlay from goodtft/LCD-show"
    command -v curl >/dev/null || die "curl is missing: sudo apt-get install curl"
    curl -fsSL --proto '=https' --tlsv1.2 --max-time 120 "$OVERLAY_URL" -o "$OVERLAY_TMP"
    overlay_hash_ok "$OVERLAY_TMP" || die "downloaded overlay hash mismatch; nothing was changed"
    OVERLAY_NEEDED=1
  fi
fi

# system packages for the dashboard, skipped when already installed
if [[ "$WITH_DISPLAY" -eq 1 ]]; then
  if dpkg-query -W python3-pygame python3-evdev >/dev/null 2>&1; then
    say "python3-pygame and python3-evdev already installed"
  else
    say "installing python3-pygame and python3-evdev"
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      python3-pygame python3-evdev
  fi
fi

# service user: no login, no home
if getent passwd "$SERVICE_USER" >/dev/null; then
  say "user $SERVICE_USER already exists"
else
  say "creating system user $SERVICE_USER"
  useradd --system --user-group --no-create-home --home-dir /nonexistent \
    --shell /usr/sbin/nologin "$SERVICE_USER"
fi

# copy this release to /opt, owned by root so the service user cannot change it
RELEASES="$DESTDIR$PREFIX/releases"
RELEASE_DIR="$RELEASES/$VERSION"
say "installing version $VERSION to $PREFIX/releases/$VERSION"
rm -rf "$RELEASE_DIR.new"
install -d -m 0755 "$RELEASE_DIR.new/bin" "$RELEASE_DIR.new/dashboard"
install -m 0755 "$BUNDLE_DIR/bin/deskpane-daemon" "$BUNDLE_DIR/bin/deskpane" "$RELEASE_DIR.new/bin/"
install -m 0644 "$BUNDLE_DIR"/dashboard/*.py "$RELEASE_DIR.new/dashboard/"
install -m 0644 "$BUNDLE_DIR/VERSION" "$BUNDLE_DIR/LICENSE" "$RELEASE_DIR.new/"
rm -rf "$RELEASE_DIR"
mv "$RELEASE_DIR.new" "$RELEASE_DIR"

# point current at this release in one step; remember the previous one for rollback
PREVIOUS=""
if [[ -L "$DESTDIR$PREFIX/current" ]]; then
  PREVIOUS="$(basename "$(readlink "$DESTDIR$PREFIX/current")")"
fi
# re-running the same version: the rollback is the newest other release still on disk
if [[ -z "$PREVIOUS" || "$PREVIOUS" == "$VERSION" ]]; then
  PREVIOUS=""
  for dir in "$RELEASES"/*/; do
    name="$(basename "$dir")"
    if [[ -d "$dir" && "$name" != "$VERSION" ]]; then
      PREVIOUS="$(printf '%s\n%s\n' "$PREVIOUS" "$name" | sed '/^$/d' | sort -V | tail -n 1)"
    fi
  done
fi
ln -sfn "releases/$VERSION" "$DESTDIR$PREFIX/current.new"
mv -Tf "$DESTDIR$PREFIX/current.new" "$DESTDIR$PREFIX/current"

# settings: written on the first install only, never overwritten; root-only
# (systemd reads EnvironmentFile as root before it drops to the service user)
ENV_FILE="$DESTDIR$CONFIG_DIR/env"
install -d -m 0755 "$DESTDIR$CONFIG_DIR"
if [[ -f "$ENV_FILE" ]]; then
  say "keeping existing $CONFIG_DIR/env"
  if [[ -n "$DEVICE_ID" || -n "$LABEL" ]]; then
    warn "--device-id/--label ignored: edit $CONFIG_DIR/env to change them"
  fi
else
  say "writing $CONFIG_DIR/env"
  while IFS= read -r line; do
    if [[ -n "$DEVICE_ID" && "$line" == DEVICE_ID=* ]]; then
      line="DEVICE_ID=$DEVICE_ID"
    fi
    if [[ -n "$LABEL" && "$line" == LABEL=* ]]; then
      line="LABEL=$LABEL"
    fi
    printf '%s\n' "$line"
  done <"$BUNDLE_DIR/config/env.example" >"$WORK_DIR/env"
  install -m 0600 -o root -g root "$WORK_DIR/env" "$ENV_FILE"
fi
# enforce owner and mode on every run, without touching the content
chown root:root "$ENV_FILE"
chmod 0600 "$ENV_FILE"

# touchscreen: overlay file, config.txt overlay line, console off the screen
REBOOT_NEEDED=0
if [[ "$WITH_DISPLAY" -eq 1 ]]; then
  # check the hash once more right before writing into /boot
  if [[ "$OVERLAY_NEEDED" -eq 1 ]]; then
    overlay_hash_ok "$OVERLAY_TMP" || die "overlay changed after it was checked; /boot was not touched"
    say "installing the display overlay into $BOOT_DIR/overlays"
    install -d -m 0755 "$DESTDIR$BOOT_DIR/overlays"
    install -m 0644 "$OVERLAY_TMP" "$OVERLAY_TARGET"
    sync
  fi

  CONFIG_TXT="$DESTDIR$BOOT_DIR/config.txt"
  if grep -q '^dtoverlay=tft35a' "$CONFIG_TXT"; then
    say "config.txt already has the tft35a overlay"
  else
    say "adding the tft35a overlay to config.txt"
    backup_once "$CONFIG_TXT"
    {
      echo ""
      echo "$MARK_BEGIN"
      # reset any conditional section above (e.g. [cm4]) so the overlay applies to every board
      echo "[all]"
      echo "# Elegoo 3.5\" SPI touchscreen (ILI9486 + XPT2046 touch), vendor-tuned"
      echo "# overlay from goodtft/LCD-show. rotate=0: kernel-level rotate=90 is buggy"
      echo "# on this driver, so fb_display.py rotates in software instead."
      echo "dtoverlay=tft35a:rotate=0"
      echo "$MARK_END"
    } >>"$CONFIG_TXT"
    sync
    REBOOT_NEEDED=1
  fi

  # cmdline.txt must stay a single line
  CMDLINE_TXT="$DESTDIR$BOOT_DIR/cmdline.txt"
  if [[ -f "$CMDLINE_TXT" ]] && ! grep -qw 'fbcon=map:1' "$CMDLINE_TXT"; then
    say "keeping the text console off the touchscreen (cmdline.txt)"
    backup_once "$CMDLINE_TXT"
    CMDLINE="$(head -n 1 "$CMDLINE_TXT" | sed 's/[[:space:]]*$//')"
    printf '%s fbcon=map:1\n' "$CMDLINE" >"$CMDLINE_TXT.new"
    mv -f "$CMDLINE_TXT.new" "$CMDLINE_TXT"
    sync
    REBOOT_NEEDED=1
  fi
fi

# which units this install runs
UNITS=(deskpane-daemon.service)
if [[ "$WITH_DISPLAY" -eq 1 ]]; then
  UNITS+=(deskpane-unbind-fbcon.service deskpane-dashboard.service)
fi
if [[ "$WITH_REPORTER" -eq 1 ]]; then
  UNITS+=(deskpane-reporter.service)
fi

# install, enable, and restart (restart so an upgrade runs the new binaries)
say "installing systemd units: ${UNITS[*]}"
install -d -m 0755 "$DESTDIR$UNIT_DIR"
for unit in "${UNITS[@]}"; do
  install -m 0644 "$BUNDLE_DIR/systemd/$unit" "$DESTDIR$UNIT_DIR/$unit"
done
systemctl daemon-reload

# a unit from an earlier install that this run did not select (e.g. --no-reporter) is stopped
for unit in deskpane-daemon.service deskpane-unbind-fbcon.service deskpane-dashboard.service deskpane-reporter.service; do
  selected=0
  for wanted in "${UNITS[@]}"; do
    if [[ "$unit" == "$wanted" ]]; then
      selected=1
    fi
  done
  if [[ "$selected" -eq 0 && -f "$DESTDIR$UNIT_DIR/$unit" ]]; then
    say "stopping $unit (not selected this time)"
    systemctl disable --now "$unit" || true
  fi
done

systemctl enable "${UNITS[@]}"
for unit in "${UNITS[@]}"; do
  # the touchscreen only exists after the reboot, so do not start the dashboard yet
  if [[ "$REBOOT_NEEDED" -eq 1 && ("$unit" == deskpane-dashboard.service || "$unit" == deskpane-unbind-fbcon.service) ]]; then
    continue
  fi
  systemctl restart "$unit"
done

# keep this release and the previous one (the rollback), delete older ones
for dir in "$RELEASES"/*/; do
  name="$(basename "$dir")"
  if [[ "$name" != "$VERSION" && "$name" != "$PREVIOUS" ]]; then
    say "removing old release $name"
    rm -rf "$dir"
  fi
done

# summary
echo
say "deskpane $VERSION installed"
systemctl --no-pager --lines=0 status "${UNITS[@]}" || true
PI_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
if [[ -z "$PI_IP" ]]; then
  PI_IP="<this Pi's IP>"
fi
echo
echo "On every other machine, run the reporter pointed at this Pi, for example:"
echo "  deskpane report --daemon $PI_IP:8765 --device-id my-pc --label \"My PC\""
if [[ -n "$PREVIOUS" && "$PREVIOUS" != "$VERSION" ]]; then
  echo "Rollback: sudo ln -sfn releases/$PREVIOUS $PREFIX/current && sudo systemctl restart ${UNITS[*]}"
fi
if [[ "$REBOOT_NEEDED" -eq 1 ]]; then
  echo
  echo "The display overlay or console setting changed. Reboot to start the dashboard:"
  echo "  sudo systemctl reboot"
fi
