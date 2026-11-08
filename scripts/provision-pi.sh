#!/usr/bin/env bash
# Installs this repo's build onto a Raspberry Pi over SSH, the same way a user
# installs a release: build the Pi bundle, copy it over, run its install.sh.
# Run from anywhere inside the repo, after:
#   1. Flashing Raspberry Pi OS Lite (64-bit, Bookworm or later) with Raspberry
#      Pi Imager's customization (hostname, SSH, user, WiFi).
#   2. Building the Pi binaries:
#        cargo build --release --locked --target aarch64-unknown-linux-musl
#   3. Having an SSH alias for the Pi (default `pi-zero` in ~/.ssh/config).
# sudo on the Pi asks for your password; no passwordless sudo is needed.
#
# Usage: scripts/provision-pi.sh [ssh-host] [install.sh options...]
#   e.g. scripts/provision-pi.sh pi-zero --label "Pi Zero"
#   --characters ZIP is read here: the local zip is copied into the Pi's temp
#   folder with the bundle, passed to install.sh, and removed with the folder.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="${1:-pi-zero}"
if [[ $# -gt 0 ]]; then
  shift
fi
BIN_DIR="${BIN_DIR:-$REPO/target/aarch64-unknown-linux-musl/release}"

die() {
  echo "provision-pi.sh: error: $*" >&2
  exit 1
}

# an ssh host starting with "-" would be read as an ssh option
[[ "$HOST" != -* ]] || die "ssh host must not start with '-': $HOST"

# install.sh options, except --characters, whose zip is local to this machine
INSTALL_OPTS=()
CHARACTERS_ZIP=""
while [[ $# -gt 0 ]]; do
  if [[ "$1" == --characters ]]; then
    [[ $# -ge 2 ]] || die "--characters needs a value: the path of the pack zip"
    [[ -f "$2" && ! -L "$2" ]] || die "--characters: $2 is not a regular file"
    # one absolute path, fixed now, for the scp below
    CHARACTERS_ZIP="$(realpath -- "$2")"
    shift 2
    continue
  fi
  INSTALL_OPTS+=("$1")
  shift
done

# local work directory, removed on exit
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

# download and verify the display overlay here, before touching the Pi
OVERLAY_URL="$(sed -n 's/^OVERLAY_URL=//p' "$REPO/deploy/overlay.env" | head -n 1)"
OVERLAY_SHA256="$(sed -n 's/^OVERLAY_SHA256=//p' "$REPO/deploy/overlay.env" | head -n 1)"
[[ "$OVERLAY_URL" =~ ^https://raw\.githubusercontent\.com/goodtft/LCD-show/[0-9a-f]{40}/ ]] ||
  die "deploy/overlay.env: OVERLAY_URL must point at a pinned goodtft/LCD-show commit"
[[ "$OVERLAY_SHA256" =~ ^[0-9a-f]{64}$ ]] || die "deploy/overlay.env: bad OVERLAY_SHA256"
echo "==> downloading and checking the display overlay"
curl -fsSL --proto '=https' --tlsv1.2 --max-time 120 "$OVERLAY_URL" -o "$WORK_DIR/tft35a.dtbo"
echo "$OVERLAY_SHA256  $WORK_DIR/tft35a.dtbo" | sha256sum -c --quiet - ||
  die "overlay hash mismatch, nothing was copied to the Pi"

# build the same bundle a release ships
echo "==> packaging the Pi bundle"
"$REPO/scripts/package-pi-bundle.sh" --bin-dir "$BIN_DIR" --out-dir "$WORK_DIR" >/dev/null
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/Cargo.toml" | head -n 1)"
TOP="deskpane-$VERSION-pi-aarch64"

# copy bundle and overlay into a fresh temp directory on the Pi
echo "==> copying to $HOST"
REMOTE_DIR="$(ssh -o ConnectTimeout=5 "$HOST" 'mktemp -d')"
[[ "$REMOTE_DIR" =~ ^/tmp/tmp\.[A-Za-z0-9]+$ ]] || die "unexpected remote temp dir: $REMOTE_DIR"
scp -q -- "$WORK_DIR/deskpane-pi-aarch64.tar.gz" "$WORK_DIR/tft35a.dtbo" "$HOST:$REMOTE_DIR/"
# the pack goes into the same temp folder under a fixed name, removed with it
if [[ -n "$CHARACTERS_ZIP" ]]; then
  echo "==> copying the character pack to $HOST"
  scp -q -- "$CHARACTERS_ZIP" "$HOST:$REMOTE_DIR/characters.zip"
  INSTALL_OPTS+=(--characters "$REMOTE_DIR/characters.zip")
fi

# extract and install; -t so sudo can ask for the password
INSTALL_ARGS=""
if [[ ${#INSTALL_OPTS[@]} -gt 0 ]]; then
  INSTALL_ARGS="$(printf ' %q' "${INSTALL_OPTS[@]}")"
fi
echo "==> running install.sh on $HOST"
ssh -t "$HOST" "cd $REMOTE_DIR && tar xzf deskpane-pi-aarch64.tar.gz && sudo ./$TOP/install.sh --overlay-file $REMOTE_DIR/tft35a.dtbo$INSTALL_ARGS; status=\$?; rm -rf $REMOTE_DIR; exit \$status"
