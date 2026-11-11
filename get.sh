#!/usr/bin/env bash
# deskpane one-line installer. Read it first if you like:
#   https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh
#
# On a Raspberry Pi (64-bit Raspberry Pi OS Bookworm or later), the whole install:
#   curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh | sudo bash
#
# On another Linux machine, install only the client and report to the Pi at boot:
#   curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh \
#     | sudo bash -s -- --reporter-only --daemon 192.168.1.50:8765
#
# Options (after `bash -s --`):
#   --version vX.Y.Z     install this release instead of the newest one (pre-releases included)
#   --no-reboot          do not reboot after the first install, even if the screen needs it
#   --no-display         Pi without the touchscreen (passed to install.sh)
#   --no-reporter        do not report the Pi's own stats (passed to install.sh)
#   --device-id ID       device id for this machine's reporter (default: automatic, from the machine id)
#   --label TEXT         name shown on the dashboard for this machine (default: the hostname)
#   --characters ZIP     Pi only: install a character pack zip for the agents page; copy
#                        the zip onto the Pi first (e.g. with scp), it is never downloaded
#   --reporter-only      install only the `deskpane` client to /usr/local/bin (any Linux)
#   --daemon HOST:PORT   with --reporter-only: report to this Pi at boot (systemd unit)
#
# It downloads the release files into a temporary folder, checks them against the
# release's SHA256SUMS, and stops on any mismatch. Nothing runs before that check.
#
# Test hooks (not for normal use): DESKPANE_API_URL, DESKPANE_DOWNLOAD_URL and
# DESKPANE_DESTDIR. A loud warning is printed whenever one is set.
set -euo pipefail

# everything sits in main(), called on the last line, so a download cut short
# by the network never runs half a script
main() {
  local repo="nokri-mohammed/deskpane"
  local api_url="${DESKPANE_API_URL:-https://api.github.com/repos/$repo/releases?per_page=10}"
  local download_url="${DESKPANE_DOWNLOAD_URL:-https://github.com/$repo/releases/download}"
  DESTDIR="${DESKPANE_DESTDIR:-}"
  # sudo -E or a root shell can carry these over by accident: say so loudly
  if [[ -n "${DESKPANE_API_URL:-}${DESKPANE_DOWNLOAD_URL:-}${DESKPANE_DESTDIR:-}" ]]; then
    echo "WARNING: test hooks set: DESKPANE_API_URL='${DESKPANE_API_URL:-}' DESKPANE_DOWNLOAD_URL='${DESKPANE_DOWNLOAD_URL:-}' DESKPANE_DESTDIR='${DESKPANE_DESTDIR:-}'" >&2
  fi
  # https only; the test hooks may also point at local file:// paths
  CURL_PROTO="=https"
  if [[ -n "${DESKPANE_API_URL:-}${DESKPANE_DOWNLOAD_URL:-}" ]]; then
    CURL_PROTO="=https,file"
  fi

  # parse flags
  local version="" reboot=1 reporter_only=0 daemon_addr="" device_id="" label="" characters=""
  local install_args=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --version)
        [[ $# -ge 2 ]] || die "--version needs a value, e.g. --version v0.1.0-beta.2"
        version="$2"
        shift
        ;;
      --no-reboot) reboot=0 ;;
      --no-display | --no-reporter) install_args+=("$1") ;;
      --device-id)
        [[ $# -ge 2 ]] || die "--device-id needs a value"
        device_id="$2"
        shift
        ;;
      --label)
        [[ $# -ge 2 ]] || die "--label needs a value"
        label="$2"
        shift
        ;;
      --characters)
        [[ $# -ge 2 ]] || die "--characters needs a value: the path of the pack zip on this Pi"
        characters="$2"
        shift
        ;;
      --reporter-only) reporter_only=1 ;;
      --daemon)
        [[ $# -ge 2 ]] || die "--daemon needs a value, e.g. --daemon 192.168.1.50:8765"
        daemon_addr="$2"
        shift
        ;;
      -h | --help)
        # piped, there is no file to print from, so point at the source
        if [[ -f "${BASH_SOURCE[0]:-}" ]]; then
          sed -n '2,28p' "${BASH_SOURCE[0]}"
        else
          echo "Options and usage: https://github.com/$repo/blob/main/get.sh"
        fi
        exit 0
        ;;
      *) die "unknown option: $1" ;;
    esac
    shift
  done
  if [[ -n "$daemon_addr" && "$reporter_only" -eq 0 ]]; then
    die "--daemon only goes with --reporter-only"
  fi
  # the pack is for the Pi's screen; it is checked once the bundle is here
  if [[ -n "$characters" ]]; then
    [[ "$reporter_only" -eq 0 ]] || die "--characters is for the Pi install; it cannot be used with --reporter-only"
    [[ -f "$characters" ]] || die "--characters: $characters not found (copy the zip onto this Pi first)"
    # absolute, because install.sh runs from the download folder; CDPATH
    # cleared so cd cannot jump elsewhere, -- so a leading dash is no option
    characters="$(CDPATH='' cd -- "$(dirname -- "$characters")" && pwd -P)/$(basename -- "$characters")"
    install_args+=(--characters "$characters")
  fi

  # common checks: root, Linux, 64-bit
  [[ "$(id -u)" -eq 0 ]] || die "run as root: curl -fsSL <url> | sudo bash"
  [[ "$(uname -s)" == Linux ]] || die "deskpane installs on Linux only (Windows: use the .zip from the releases page)"
  local arch
  arch="$(uname -m)"
  command -v curl >/dev/null || die "curl is missing: sudo apt-get install curl"
  command -v sha256sum >/dev/null || die "sha256sum is missing (package coreutils)"

  # work folder in /tmp (TMPDIR is not honoured under sudo -E), removed on exit
  WORK_DIR="$(mktemp -d /tmp/deskpane.XXXXXX)"
  trap 'rm -rf "$WORK_DIR"' EXIT

  if [[ -z "$version" ]]; then
    version="$(find_newest_release "$api_url")"
  fi
  version="v${version#v}"
  [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "not a release version: $version"

  if [[ "$reporter_only" -eq 1 ]]; then
    install_client "$download_url" "$version" "$arch" "$daemon_addr" "$device_id" "$label"
  else
    install_pi "$download_url" "$version" "$arch" "$reboot" "$device_id" "$label" "${install_args[@]}"
  fi
}

# print an error and stop
die() {
  echo "get.sh: error: $*" >&2
  exit 1
}

# print a step
say() { echo "==> $*"; }

# value of KEY=value from a plain file (never sourced as shell code); empty if missing
read_key() {
  if [[ -f "$2" ]]; then
    sed -n "s/^$1=//p" "$2" | head -n 1
  fi
}

# newest release tag, pre-releases included (the API lists newest first, drafts are hidden)
find_newest_release() {
  local api_url="$1" status tag
  # -q first: ignore ~/.curlrc; curl's own error line stays visible on stderr
  status="$(curl -q -sSL --proto "$CURL_PROTO" --proto-redir "$CURL_PROTO" --tlsv1.2 -o "$WORK_DIR/releases.json" -w '%{http_code}' \
    -H 'Accept: application/vnd.github+json' --max-time 30 "$api_url" || true)"
  case "$status" in
    200) ;;
    000 | "")
      if [[ "$api_url" != file://* || ! -s "$WORK_DIR/releases.json" ]]; then
        die "cannot reach GitHub: no network, or the clock is wrong (check \`date\`), or TLS failed"
      fi
      ;;
    403 | 429) die "GitHub API rate limit reached. Wait an hour, or pick a version: ... | sudo bash -s -- --version vX.Y.Z" ;;
    *) die "GitHub API answered HTTP $status for $api_url" ;;
  esac
  tag="$(grep -o '"tag_name": *"[^"]*"' "$WORK_DIR/releases.json" | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')"
  [[ -n "$tag" ]] || die "no deskpane release found yet"
  echo "$tag"
}

# download one release file and SHA256SUMS, then check the file against it
download_verified() {
  local download_url="$1" version="$2" asset="$3"
  say "downloading $asset ($version)"
  curl -q -fsSL --proto "$CURL_PROTO" --proto-redir "$CURL_PROTO" --tlsv1.2 --max-time 300 -o "$WORK_DIR/$asset" "$download_url/$version/$asset" ||
    die "download failed: $download_url/$version/$asset"
  if [[ ! -f "$WORK_DIR/SHA256SUMS" ]]; then
    curl -q -fsSL --proto "$CURL_PROTO" --proto-redir "$CURL_PROTO" --tlsv1.2 --max-time 60 -o "$WORK_DIR/SHA256SUMS" "$download_url/$version/SHA256SUMS" ||
      die "download failed: $download_url/$version/SHA256SUMS"
  fi
  # the line for exactly this file must exist and match
  grep -E "^[0-9a-f]{64}  $asset\$" "$WORK_DIR/SHA256SUMS" >"$WORK_DIR/check.sha256" ||
    die "SHA256SUMS has no line for $asset; refusing to install"
  (cd "$WORK_DIR" && sha256sum -c --status check.sha256) ||
    die "checksum mismatch for $asset; refusing to install (the download is corrupt or was changed)"
  say "checksum OK"
}

# remove the pre-rename install (homelab-*.service): its daemon holds UDP 8765 and
# its dashboard fights ours for the screen. Only units whose Description proves they
# are ours are touched; only binaries named in their ExecStart are deleted, never
# guessed paths. install.sh does the same, this copy also covers older bundles.
remove_old_install() {
  local unit_dir="$DESTDIR/etc/systemd/system" unit_file name description exec_path removed=0
  local old_dirs=()
  for unit_file in "$unit_dir"/homelab-*.service "$unit_dir/unbind-fbcon.service"; do
    if [[ ! -f "$unit_file" ]]; then
      continue
    fi
    name="$(basename "$unit_file")"
    description="$(sed -n 's/^Description=//p' "$unit_file" | head -n 1)"
    if [[ "$description" != pi-homelab-dashboard* && "$description" != "Unbind text console from the touchscreen framebuffer" ]]; then
      echo "get.sh: warning: leaving $name alone: it does not look like an old pi-homelab-dashboard unit" >&2
      continue
    fi
    say "removing old install unit $name"
    systemctl stop "$name" 2>/dev/null || true
    systemctl disable "$name" 2>/dev/null || true
    exec_path="$(sed -n 's/^ExecStart=//p' "$unit_file" | head -n 1 | awk '{print $1}')"
    if [[ "$exec_path" == /* && ( "$(basename "$exec_path")" == daemon || "$(basename "$exec_path")" == reporter ) && -f "$DESTDIR$exec_path" ]]; then
      say "removing old binary $exec_path"
      rm -f "$DESTDIR$exec_path"
      # list each old folder once
      if [[ " ${old_dirs[*]} " != *" $(dirname "$exec_path") "* ]]; then
        old_dirs+=("$(dirname "$exec_path")")
      fi
    fi
    rm -f "$unit_file"
    removed=1
  done
  if [[ "$removed" -eq 1 ]]; then
    systemctl daemon-reload
    systemctl reset-failed 2>/dev/null || true
  fi
  if [[ ${#old_dirs[@]} -gt 0 ]]; then
    say "the old install's files are still in: ${old_dirs[*]} (safe to delete by hand)"
  fi
}

# stop if a process other than deskpane-daemon holds UDP 8765, and name it
check_udp_port() {
  local line process
  if ! command -v ss >/dev/null; then
    return 0
  fi
  while IFS= read -r line; do
    if [[ -z "$line" ]]; then
      continue
    fi
    process="$(echo "$line" | sed -n 's/.*users:(("\([^"]*\)",pid=\([0-9]*\).*/\1 (pid \2)/p')"
    if [[ "$process" == deskpane-daemon* ]]; then
      continue
    fi
    die "UDP port 8765 is in use by ${process:-an unknown process}. Stop it, then run this again."
  done <<<"$(ss -Hulnp 'sport = :8765' 2>/dev/null || true)"
}

# true when config.txt has our overlay and cmdline.txt keeps the console off the screen
display_configured() {
  grep -q '^dtoverlay=tft35a' "$DESTDIR/boot/firmware/config.txt" 2>/dev/null &&
    grep -qw 'fbcon=map:1' "$DESTDIR/boot/firmware/cmdline.txt" 2>/dev/null
}

# full install on a Raspberry Pi
install_pi() {
  local download_url="$1" version="$2" arch="$3" reboot="$4" device_id="$5" label="$6"
  shift 6
  local install_args=("$@")

  # a Pi with 64-bit Raspberry Pi OS Bookworm (Debian 12) or later
  [[ "$arch" == aarch64 ]] || die "needs 64-bit Raspberry Pi OS (aarch64), this system is $arch. Flash Raspberry Pi OS Lite (64-bit)."
  local model=""
  if [[ -r "$DESTDIR/proc/device-tree/model" ]]; then
    model="$(tr -d '\0' <"$DESTDIR/proc/device-tree/model")"
  fi
  [[ "$model" == "Raspberry Pi"* ]] || die "this is not a Raspberry Pi. On another Linux machine use: ... | sudo bash -s -- --reporter-only"
  local os_version=""
  if [[ -r "$DESTDIR/etc/os-release" ]]; then
    os_version="$(sed -n 's/^VERSION_ID="\{0,1\}\([0-9]*\)"\{0,1\}$/\1/p' "$DESTDIR/etc/os-release")"
  fi
  [[ "$os_version" =~ ^[0-9]+$ && "$os_version" -ge 12 ]] || die "needs Raspberry Pi OS Bookworm or later (Debian 12+), found VERSION_ID '${os_version:-unknown}'"

  download_verified "$download_url" "$version" deskpane-pi-aarch64.tar.gz
  tar --no-same-owner -xzf "$WORK_DIR/deskpane-pi-aarch64.tar.gz" -C "$WORK_DIR"
  local bundle
  bundle="$(find "$WORK_DIR" -maxdepth 1 -type d -name 'deskpane-*-pi-aarch64' | head -n 1)"
  [[ -n "$bundle" && -x "$bundle/install.sh" ]] || die "the bundle has no install.sh"

  # character pack: the bundle must support it and the zip must pass the
  # bundle's own validator, before remove_old_install or anything else
  # changes the system (--check writes nothing)
  local arg characters="" take_next=0
  for arg in "${install_args[@]}"; do
    if [[ "$take_next" -eq 1 ]]; then
      characters="$arg"
      take_next=0
    elif [[ "$arg" == --characters ]]; then
      take_next=1
    fi
  done
  if [[ -n "$characters" ]]; then
    # bundles older than 0.1.0-beta.4 have no character packs
    if ! grep -q -- '--characters' "$bundle/install.sh" || [[ ! -f "$bundle/dashboard/charpack_install.py" ]]; then
      die "deskpane $version does not support --characters (character packs start with 0.1.0-beta.4)"
    fi
    command -v python3 >/dev/null || die "--characters needs python3 (sudo apt-get install python3)"
    say "checking the character pack $characters"
    python3 -I "$bundle/dashboard/charpack_install.py" --check "$characters" "$DESTDIR/var/lib/deskpane/characters" ||
      die "the character pack was refused; nothing was changed"
  fi

  # pass identity flags through; install.sh validates them
  if [[ -n "$device_id" ]]; then
    install_args+=(--device-id "$device_id")
  fi
  if [[ -n "$label" ]]; then
    install_args+=(--label "$label")
  fi

  # the screen needs a reboot only when this run adds the overlay or console setting
  local configured_before=0
  if display_configured; then
    configured_before=1
  fi

  # old install out of the way, port free, before anything new is installed
  remove_old_install
  check_udp_port

  # bundles older than 0.1.0-beta.2 print their own summary and lack --no-summary
  if grep -q -- '--no-summary' "$bundle/install.sh"; then
    install_args=(--no-summary "${install_args[@]}")
  fi
  say "running the installer"
  # DESTDIR is set too (empty in normal use) for bundles older than 0.1.0-beta.2
  local root="$DESTDIR"
  DESKPANE_DESTDIR="$root" DESTDIR="$root" "$bundle/install.sh" "${install_args[@]}" </dev/null

  # short summary
  local ip unit state
  ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  echo
  say "deskpane $version installed"
  for unit in deskpane-daemon deskpane-dashboard deskpane-reporter; do
    state="$(systemctl is-active "$unit" 2>/dev/null || true)"
    if [[ -n "$state" && "$state" != inactive ]] || systemctl is-enabled "$unit" >/dev/null 2>&1; then
      echo "    $unit: ${state:-unknown}"
    fi
  done
  echo "    Pi address: ${ip:-unknown}"
  echo
  echo "On each other Linux machine, one line installs the client and reports at boot:"
  echo "  curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh | sudo bash -s -- --reporter-only --daemon ${ip:-<pi-ip>}:8765"
  echo "Or run the client by hand (id and label are automatic, or pass --device-id and --label):"
  echo "  deskpane report --daemon ${ip:-<pi-ip>}:8765"

  local with_display=1
  for arg in "${install_args[@]}"; do
    if [[ "$arg" == --no-display ]]; then
      with_display=0
    fi
  done
  if [[ "$with_display" -eq 1 && "$configured_before" -eq 0 ]] && display_configured; then
    echo
    if [[ "$reboot" -eq 1 ]]; then
      echo "The touchscreen needs a reboot. Rebooting in 10 s, Ctrl+C to cancel."
      sleep 10
      systemctl reboot
    else
      echo "The touchscreen needs a reboot: sudo systemctl reboot"
    fi
  fi
}

# client only, on any 64-bit Linux machine
install_client() {
  local download_url="$1" version="$2" arch="$3" daemon_addr="$4" device_id="$5" label="$6"
  local asset
  case "$arch" in
    x86_64) asset=deskpane-linux-x86_64.tar.gz ;;
    aarch64) asset=deskpane-linux-aarch64.tar.gz ;;
    *) die "no deskpane client for $arch (64-bit x86_64 or aarch64 only)" ;;
  esac

  # on the Pi the bundle already runs its own reporter; ours would replace its unit
  if [[ -e "$DESTDIR/etc/deskpane/env" || -e "$DESTDIR/etc/systemd/system/deskpane-daemon.service" ]]; then
    die "this is the Pi; its reporter is already installed by the Pi install"
  fi

  # check the reporter settings before downloading anything; an empty id or
  # label is left to the client (automatic id from the machine id, hostname label)
  local env_file="$DESTDIR/etc/deskpane/reporter.env"
  if [[ -n "$daemon_addr" ]]; then
    [[ "$daemon_addr" =~ ^[A-Za-z0-9.-]+:[0-9]{1,5}$ ]] || die "--daemon must look like 192.168.1.50:8765"
    # an upgrade keeps the id and label this machine already reports with
    if [[ -z "$device_id" ]]; then
      device_id="$(read_key DEVICE_ID "$env_file")"
      if [[ -n "$device_id" ]]; then
        say "keeping the device id from /etc/deskpane/reporter.env: $device_id"
      fi
    fi
    if [[ -z "$label" ]]; then
      label="$(read_key LABEL "$env_file")"
    fi
    if [[ -n "$device_id" && ! "$device_id" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
      die "--device-id: 1 to 64 letters, digits, dot, dash or underscore, starting with a letter or digit"
    fi
    local label_pattern='^[A-Za-z0-9][A-Za-z0-9 ._()-]{0,63}$'
    if [[ -n "$label" && ! "$label" =~ $label_pattern ]]; then
      die "--label: up to 64 letters, digits, spaces and . _ ( ) -, starting with a letter or digit"
    fi
  fi

  download_verified "$download_url" "$version" "$asset"
  tar --no-same-owner -xzf "$WORK_DIR/$asset" -C "$WORK_DIR"
  local binary
  binary="$(find "$WORK_DIR" -mindepth 2 -maxdepth 2 -type f -name deskpane | head -n 1)"
  [[ -n "$binary" ]] || die "the archive has no deskpane binary"
  install -d -m 0755 "$DESTDIR/usr/local/bin"
  install -m 0755 "$binary" "$DESTDIR/usr/local/bin/deskpane"
  say "installed /usr/local/bin/deskpane ($version)"

  if [[ -z "$daemon_addr" ]]; then
    echo "Run it with: deskpane report --daemon <pi-ip>:8765 (id and label are automatic)"
    echo "Or run get.sh again with --daemon <pi-ip>:8765 to report at boot."
    return 0
  fi

  # one report that asks the Pi for an answer, before any setting or unit is
  # written, so a failed check leaves an existing working setup as it was
  local once_args=(report --once --daemon "$daemon_addr") rc=0
  if [[ -n "$device_id" ]]; then
    once_args+=(--device-id "$device_id")
  fi
  if [[ -n "$label" ]]; then
    once_args+=(--label "$label")
  fi
  # clients older than 0.1.0-beta.3 (--version) have no --once and need an id
  if ! "$DESTDIR/usr/local/bin/deskpane" report --help 2>/dev/null | grep -q -- '--once'; then
    [[ -n "$device_id" ]] || die "deskpane $version needs --device-id (automatic ids start with 0.1.0-beta.3)"
    echo "get.sh: warning: deskpane $version cannot check the connection (no --once); skipping the check" >&2
    rc=12
  else
    say "checking the connection to $daemon_addr"
    "$DESTDIR/usr/local/bin/deskpane" "${once_args[@]}" || rc=$?
  fi
  case "$rc" in
    0) ;;
    10)
      # the client printed which machine has the id
      echo "get.sh: error: pick a free id and run get.sh again with --device-id <name>." >&2
      echo "  /usr/local/bin/deskpane is installed; the reporter settings and service were not changed." >&2
      exit 1
      ;;
    12)
      # installing a client before the Pi is up is normal: keep going
      echo "get.sh: warning: could not confirm the Pi got the report. Installing the service anyway; it keeps trying." >&2
      ;;
    *)
      echo "get.sh: error: the client stopped with exit code $rc (see the line above)." >&2
      echo "  /usr/local/bin/deskpane is installed; the reporter settings and service were not changed." >&2
      exit 1
      ;;
  esac

  # settings file, root-only, read by systemd
  install -d -m 0755 "$DESTDIR/etc/deskpane"
  printf 'DAEMON=%s\nDEVICE_ID=%s\nLABEL=%s\n' "$daemon_addr" "$device_id" "$label" >"$WORK_DIR/reporter.env"
  install -m 0600 -o root -g root "$WORK_DIR/reporter.env" "$DESTDIR/etc/deskpane/reporter.env"

  # a throwaway user with no rights; the client only reads system stats and sends UDP
  install -d -m 0755 "$DESTDIR/etc/systemd/system"
  cat >"$DESTDIR/etc/systemd/system/deskpane-reporter.service" <<'EOF'
# Sends this machine's stats to the deskpane Pi. Installed by get.sh --reporter-only.
# The sandbox below passes systemd-analyze verify; confirm it on a real machine
# (journalctl -u deskpane-reporter) before relying on it.
[Unit]
Description=deskpane reporter
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
DynamicUser=yes
EnvironmentFile=/etc/deskpane/reporter.env
ExecStart=/usr/local/bin/deskpane report --daemon ${DAEMON} --device-id ${DEVICE_ID} --label ${LABEL}
Restart=always
RestartSec=5
UMask=0077
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
RestrictNamespaces=yes
RestrictRealtime=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
CapabilityBoundingSet=
# UDP to the Pi only; stats come from /proc and /sys, no local sockets needed
RestrictAddressFamilies=AF_INET AF_INET6

[Install]
WantedBy=multi-user.target
EOF
  chmod 0644 "$DESTDIR/etc/systemd/system/deskpane-reporter.service"
  systemctl daemon-reload

  systemctl enable deskpane-reporter.service
  systemctl restart deskpane-reporter.service
  if [[ -n "$device_id" ]]; then
    say "reporting to $daemon_addr as '$device_id', at every boot"
  else
    say "reporting to $daemon_addr with an automatic id, at every boot"
  fi
  echo "Settings: /etc/deskpane/reporter.env, then: sudo systemctl restart deskpane-reporter"
}

main "$@"
