#!/usr/bin/env bash
# Installs or upgrades deskpane on a Raspberry Pi from an extracted
# release bundle. Safe to run again: running a newer bundle's install.sh is the
# upgrade. Needs 64-bit Raspberry Pi OS Bookworm or later.
#
# Usage: sudo ./install.sh [options]   (see --help)
#
# DESKPANE_DESTDIR (empty by default) prefixes every file path, for testing in a
# fake root. A plain DESTDIR from the caller's environment is ignored on purpose.
set -euo pipefail

PREFIX=/opt/deskpane
CONFIG_DIR=/etc/deskpane
UNIT_DIR=/etc/systemd/system
BOOT_DIR=/boot/firmware
SERVICE_USER=deskpane
# the reporter unit's SupplementaryGroups: may read pisugar-server's socket (T-064)
PISUGAR_GROUP=pisugar
# optional character pack: state that outlives upgrades, never in a release
STATE_DIR=/var/lib/deskpane
CHARACTERS_DIR=$STATE_DIR/characters
# the real pack is about 100 KB; charpack.py has the same cap
MAX_CHARACTERS_ZIP_BYTES=$((4 * 1024 * 1024))
DESTDIR="${DESKPANE_DESTDIR:-}"
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
                       (default: automatic, derived from /etc/machine-id)
  --label TEXT         label shown on the dashboard (first install only), up to 64
                       (default: the hostname)
  --overlay-file FILE  use an already downloaded display overlay (hash still checked)
  --characters ZIP     install a character pack zip for the agents page into
                       /var/lib/deskpane/characters (checked first, never unzipped)
  --remove-characters  remove the installed character pack (built-in icons again)
  --no-summary         skip the closing summary (get.sh prints its own)
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

# remove the pre-rename install (pi-homelab-dashboard, homelab-*.service): its
# daemon holds UDP 8765 and its dashboard fights ours for the screen. Only units
# whose Description proves they are ours are touched, and only binaries named in
# their ExecStart are deleted, never guessed paths.
remove_old_install() {
  local unit_file name description exec_path old_dir
  for unit_file in "$DESTDIR$UNIT_DIR"/homelab-*.service "$DESTDIR$UNIT_DIR/unbind-fbcon.service"; do
    if [[ ! -f "$unit_file" ]]; then
      continue
    fi
    name="$(basename "$unit_file")"
    description="$(read_key Description "$unit_file")"
    if [[ "$description" != pi-homelab-dashboard* && "$description" != "Unbind text console from the touchscreen framebuffer" ]]; then
      warn "leaving $name alone: it does not look like an old pi-homelab-dashboard unit"
      continue
    fi

    say "removing old install unit $name"
    systemctl stop "$name" 2>/dev/null || true
    systemctl disable "$name" 2>/dev/null || true

    # first word of ExecStart; delete it only if it is the old daemon or reporter binary
    exec_path="$(read_key ExecStart "$unit_file" | awk '{print $1}')"
    if [[ "$exec_path" == /* && ( "$(basename "$exec_path")" == daemon || "$(basename "$exec_path")" == reporter ) && -f "$DESTDIR$exec_path" ]]; then
      say "removing old binary $exec_path"
      rm -f "$DESTDIR$exec_path"
      old_dir="$(dirname "$exec_path")"
      # list each old folder once
      if [[ " ${OLD_DIRS[*]} " != *" $old_dir "* ]]; then
        OLD_DIRS+=("$old_dir")
      fi
    fi
    rm -f "$unit_file"
    OLD_REMOVED=1
  done
  if [[ "$OLD_REMOVED" -eq 1 ]]; then
    systemctl daemon-reload
    systemctl reset-failed 2>/dev/null || true
  fi
}

# stop if a process other than deskpane-daemon holds UDP 8765, and name it
check_udp_port() {
  local holders line process
  if ! command -v ss >/dev/null; then
    warn "ss not found, cannot check who uses UDP 8765"
    return 0
  fi
  holders="$(ss -Hulnp 'sport = :8765' 2>/dev/null || true)"
  while IFS= read -r line; do
    if [[ -z "$line" ]]; then
      continue
    fi
    process="$(echo "$line" | sed -n 's/.*users:(("\([^"]*\)",pid=\([0-9]*\).*/\1 (pid \2)/p')"
    if [[ "$process" == deskpane-daemon* ]]; then
      continue
    fi
    die "UDP port 8765 is in use by ${process:-an unknown process}. Stop it, then run the installer again."
  done <<<"$holders"
}

# parse flags
SUMMARY=1
OLD_REMOVED=0
OLD_DIRS=()
WITH_DISPLAY=1
WITH_REPORTER=1
DEVICE_ID=""
LABEL=""
OVERLAY_FILE=""
CHARACTERS_ZIP=""
REMOVE_CHARACTERS=0
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
    --characters)
      [[ $# -ge 2 ]] || die "--characters needs a value: the path of the pack zip"
      CHARACTERS_ZIP="$2"
      shift
      ;;
    --remove-characters) REMOVE_CHARACTERS=1 ;;
    --no-summary) SUMMARY=0 ;;
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
  dashboard/dashboard.py dashboard/fb_display.py dashboard/touch.py dashboard/layout.py \
  dashboard/deskpane_contract.py \
  dashboard/charpack.py dashboard/charpack_install.py dashboard/sprites.py dashboard/display.py \
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

# work directory for downloads, removed on exit (root only, from mktemp)
WORK_DIR="$(mktemp -d /tmp/deskpane.XXXXXX)"
trap 'rm -rf "$WORK_DIR"' EXIT

# character pack: check the whole zip now, before anything on the system
# changes. The zip is copied once, at most 4 MiB plus one byte, into the
# root-only work folder; that private copy is what gets checked and later
# installed, so the user's file can change under us without effect. The
# validator is stdlib Python, run isolated (-I); it never calls unzip and
# writes nothing in --check mode.
PACK_COPY=""
if [[ -n "$CHARACTERS_ZIP" ]]; then
  [[ "$REMOVE_CHARACTERS" -eq 0 ]] || die "--characters and --remove-characters cannot be used together"
  [[ "$WITH_DISPLAY" -eq 1 ]] || die "--characters needs the dashboard; it cannot be used with --no-display"
  [[ -f "$CHARACTERS_ZIP" && ! -L "$CHARACTERS_ZIP" ]] || die "--characters: $CHARACTERS_ZIP is not a regular file (no symlinks or devices)"
  command -v python3 >/dev/null || die "--characters needs python3 (sudo apt-get install python3)"
  PACK_COPY="$WORK_DIR/characters.zip"
  head -c "$((MAX_CHARACTERS_ZIP_BYTES + 1))" -- "$CHARACTERS_ZIP" >"$PACK_COPY" ||
    die "--characters: cannot read $CHARACTERS_ZIP"
  ZIP_BYTES="$(wc -c <"$PACK_COPY")"
  [[ "$ZIP_BYTES" -le "$MAX_CHARACTERS_ZIP_BYTES" ]] || die "--characters: the zip is bigger than 4 MiB"
  say "checking the character pack (copied to $PACK_COPY)"
  python3 -I "$BUNDLE_DIR/dashboard/charpack_install.py" --check "$PACK_COPY" "$DESTDIR$CHARACTERS_DIR" ||
    die "the character pack was refused; nothing was changed"
fi

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
    curl -q -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 --max-time 120 "$OVERLAY_URL" -o "$OVERLAY_TMP"
    overlay_hash_ok "$OVERLAY_TMP" || die "downloaded overlay hash mismatch; nothing was changed"
    OVERLAY_NEEDED=1
  fi
fi

# an old pre-rename install would hold the port and the screen: remove it first
remove_old_install
check_udp_port

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

# group pisugar for the reporter unit (SupplementaryGroups=pisugar, T-064).
# systemd refuses to start a unit whose group does not exist, so it is made
# here when missing. It grants nothing until pisugar-server is set up with
# its socket in group pisugar (docs/INSTALL.md, "PiSugar battery").
if [[ "$WITH_REPORTER" -eq 1 ]]; then
  if getent group "$PISUGAR_GROUP" >/dev/null; then
    say "group $PISUGAR_GROUP already exists"
  else
    say "creating system group $PISUGAR_GROUP (battery reading, optional)"
    groupadd --system "$PISUGAR_GROUP"
  fi
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

# character pack: written only with --characters, removed only with
# --remove-characters; otherwise the folder is left exactly as it is.
# Root owns it; the dashboard only reads it (ProtectSystem=strict keeps
# /var/lib readable). Fixed file names only, swapped in as a whole. The pack
# is optional, so a failure here warns and the install carries on; the
# previous pack (if any) stays as it was.
if [[ -n "$PACK_COPY" ]]; then
  say "installing the character pack into $CHARACTERS_DIR"
  if install -d -m 0755 -o root -g root "$DESTDIR$STATE_DIR" &&
    python3 -I "$RELEASE_DIR/dashboard/charpack_install.py" "$PACK_COPY" "$DESTDIR$CHARACTERS_DIR"; then
    chown -R root:root "$DESTDIR$CHARACTERS_DIR"
  else
    warn "the character pack could not be installed; continuing with the built-in icons"
  fi
elif [[ "$REMOVE_CHARACTERS" -eq 1 ]]; then
  say "removing the character pack from $CHARACTERS_DIR"
  rm -rf "$DESTDIR$CHARACTERS_DIR" "$DESTDIR$CHARACTERS_DIR.new" "$DESTDIR$CHARACTERS_DIR.old"
fi

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

# ask the daemon, as the Pi's own reporter, whether its id is free; it may
# still be binding, so retry "no answer" (exit 12) for about 5 s. Never fatal.
REPORTED_AS=""
if [[ "$WITH_REPORTER" -eq 1 ]]; then
  ONCE_ARGS=(report --once --daemon 127.0.0.1:8765)
  ENV_DEVICE_ID="$(read_key DEVICE_ID "$ENV_FILE")"
  ENV_LABEL="$(read_key LABEL "$ENV_FILE")"
  if [[ -n "$ENV_DEVICE_ID" ]]; then
    ONCE_ARGS+=(--device-id "$ENV_DEVICE_ID")
  fi
  if [[ -n "$ENV_LABEL" ]]; then
    ONCE_ARGS+=(--label "$ENV_LABEL")
  fi
  say "checking the Pi's own report"
  ONCE_RC=12
  ONCE_DEADLINE=$((SECONDS + 5))
  while [[ "$ONCE_RC" -eq 12 && "$SECONDS" -lt "$ONCE_DEADLINE" ]]; do
    ONCE_RC=0
    ONCE_OUT="$("$DESTDIR$PREFIX/current/bin/deskpane" "${ONCE_ARGS[@]}" 2>"$WORK_DIR/once.err")" || ONCE_RC=$?
    if [[ "$ONCE_RC" -eq 12 ]]; then
      sleep 1
    fi
  done
  case "$ONCE_RC" in
    0)
      say "$ONCE_OUT"
      # "deskpane: reported to 127.0.0.1:8765 as <id> (<label>)": keep the id
      REPORTED_AS="$(echo "$ONCE_OUT" | sed -n 's/^deskpane: reported to [^ ]* as \([^ ]*\) .*/\1/p')"
      ;;
    10)
      cat "$WORK_DIR/once.err" >&2
      warn "a machine on the LAN already uses the Pi's device id. Set DEVICE_ID in $CONFIG_DIR/env, then: sudo systemctl restart deskpane-reporter"
      ;;
    *)
      cat "$WORK_DIR/once.err" >&2
      warn "could not confirm the Pi's own report (exit $ONCE_RC); check: journalctl -u deskpane-daemon -u deskpane-reporter"
      ;;
  esac
fi

# keep this release and the previous one (the rollback), delete older ones
for dir in "$RELEASES"/*/; do
  name="$(basename "$dir")"
  if [[ "$name" != "$VERSION" && "$name" != "$PREVIOUS" ]]; then
    say "removing old release $name"
    rm -rf "$dir"
  fi
done

# summary (get.sh passes --no-summary and prints its own)
if [[ ${#OLD_DIRS[@]} -gt 0 ]]; then
  say "the old install's files are still in: ${OLD_DIRS[*]} (safe to delete by hand)"
fi
if [[ "$SUMMARY" -eq 0 ]]; then
  exit 0
fi
echo
say "deskpane $VERSION installed"
systemctl --no-pager --lines=0 status "${UNITS[@]}" || true
PI_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
if [[ -z "$PI_IP" ]]; then
  PI_IP="<this Pi's IP>"
fi
echo
if [[ -n "$REPORTED_AS" ]]; then
  echo "This Pi reports as: $REPORTED_AS"
fi
echo "On every other machine, run the reporter pointed at this Pi (id and label are automatic):"
echo "  deskpane report --daemon $PI_IP:8765"
if [[ -n "$PREVIOUS" && "$PREVIOUS" != "$VERSION" ]]; then
  echo "Rollback: sudo ln -sfn releases/$PREVIOUS $PREFIX/current && sudo systemctl restart ${UNITS[*]}"
fi
if [[ "$REBOOT_NEEDED" -eq 1 ]]; then
  echo
  echo "The display overlay or console setting changed. Reboot to start the dashboard:"
  echo "  sudo systemctl reboot"
fi
