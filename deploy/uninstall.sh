#!/usr/bin/env bash
# Removes deskpane from this Pi. By default it keeps the settings in
# /etc/deskpane, the deskpane user, the display overlay and the
# boot config lines and the character pack in /var/lib/deskpane, so a later
# install.sh picks up where it left off.
#
# Usage: sudo ./uninstall.sh [--purge]
#   --purge  also remove the settings, the user, the overlay, our boot config
#            lines and /var/lib/deskpane (the character pack)
#
# DESKPANE_DESTDIR (empty by default) prefixes every file path, for testing in a
# fake root. A plain DESTDIR from the caller's environment is ignored on purpose.
set -euo pipefail

PREFIX=/opt/deskpane
CONFIG_DIR=/etc/deskpane
UNIT_DIR=/etc/systemd/system
BOOT_DIR=/boot/firmware
SERVICE_USER=deskpane
STATE_DIR=/var/lib/deskpane
DESTDIR="${DESKPANE_DESTDIR:-}"
UNITS=(deskpane-reporter.service deskpane-dashboard.service deskpane-unbind-fbcon.service deskpane-daemon.service)

# print a step or an error and stop
say() { echo "==> $*"; }
die() {
  echo "uninstall.sh: error: $*" >&2
  exit 1
}

# parse flags
PURGE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --purge) PURGE=1 ;;
    -h | --help)
      sed -n '2,10p' "$0"
      exit 0
      ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
done

[[ "$(id -u)" -eq 0 ]] || die "run as root: sudo ./uninstall.sh"

# stop and remove the services; a unit that was never installed is fine
say "stopping and removing the systemd units"
for unit in "${UNITS[@]}"; do
  systemctl disable --now "$unit" 2>/dev/null || true
  rm -f "$DESTDIR$UNIT_DIR/$unit"
done
systemctl daemon-reload

# remove the program files
say "removing $PREFIX"
rm -rf "$DESTDIR$PREFIX"

if [[ "$PURGE" -eq 0 ]]; then
  say "done. Kept $CONFIG_DIR, $STATE_DIR, the $SERVICE_USER user and the display setup (use --purge to remove them)"
  exit 0
fi

# settings, state (the character pack) and user
say "removing $CONFIG_DIR, $STATE_DIR and the $SERVICE_USER user"
rm -rf "$DESTDIR$CONFIG_DIR" "$DESTDIR$STATE_DIR"
if getent passwd "$SERVICE_USER" >/dev/null; then
  userdel "$SERVICE_USER"
fi

# display: the overlay file, our block in config.txt, fbcon=map:1 in cmdline.txt
REBOOT_NEEDED=0
rm -f "$DESTDIR$BOOT_DIR/overlays/tft35a.dtbo"
sync
CONFIG_TXT="$DESTDIR$BOOT_DIR/config.txt"
if [[ -f "$CONFIG_TXT" ]] && grep -q '^dtoverlay=tft35a' "$CONFIG_TXT"; then
  say "removing the tft35a overlay from config.txt"
  # drop the marked block (install.sh) and any bare tft35a line (older provisioning)
  sed -e '/^# >>> deskpane$/,/^# <<< deskpane$/d' \
    -e '/^dtoverlay=tft35a/d' "$CONFIG_TXT" >"$CONFIG_TXT.new"
  mv -f "$CONFIG_TXT.new" "$CONFIG_TXT"
  sync
  REBOOT_NEEDED=1
fi
CMDLINE_TXT="$DESTDIR$BOOT_DIR/cmdline.txt"
if [[ -f "$CMDLINE_TXT" ]] && grep -qw 'fbcon=map:1' "$CMDLINE_TXT"; then
  say "removing fbcon=map:1 from cmdline.txt"
  sed -e 's/ *fbcon=map:1//' "$CMDLINE_TXT" >"$CMDLINE_TXT.new"
  mv -f "$CMDLINE_TXT.new" "$CMDLINE_TXT"
  sync
  REBOOT_NEEDED=1
fi

say "done. Backups of the original boot files, if any, are in $BOOT_DIR/*.deskpane.bak"
if [[ "$REBOOT_NEEDED" -eq 1 ]]; then
  echo "Reboot to apply the boot config change: sudo systemctl reboot"
fi
