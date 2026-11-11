#!/usr/bin/env bash
# Builds the Pi release bundle, deskpane-pi-aarch64.tar.gz, from
# already built aarch64 binaries. Used by the release workflow, the CI trial
# build and scripts/provision-pi.sh. Run from anywhere inside the repo.
#
# Usage: scripts/package-pi-bundle.sh [--version X.Y.Z] [--bin-dir DIR] [--out-dir DIR]
#   --version  defaults to [workspace.package] version in Cargo.toml
#   --bin-dir  defaults to target/aarch64-unknown-linux-musl/release
#   --out-dir  defaults to dist
#
# e.g. cargo build --release --locked --target aarch64-unknown-linux-musl
#      scripts/package-pi-bundle.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCHIVE_NAME="deskpane-pi-aarch64.tar.gz"

die() {
  echo "package-pi-bundle.sh: error: $*" >&2
  exit 1
}

# licence guard (T-053): character packs are licensed to their owner and must
# never be redistributed, so no pack-shaped file may sit in a folder the
# bundle is built from, and the finished bundle may hold no image or zip
# (no `find | head` here: under pipefail a closed pipe would end the
# script silently, before the message)
refuse_pack_files() {
  local found
  found="$(find "$@" \( -iname 'NPC_*.png' -o -iname '*_emote_16x16.png' -o -iname '*.zip' \
    -o \( -type d \( -name characters -o -name Emotes \) \) \) -print 2>/dev/null || true)"
  if [[ -n "$found" ]]; then
    sed -n '1,5p' <<<"$found" >&2
    die "character pack files found where the bundle is built from; a pack must never enter a bundle (see above)"
  fi
}

# parse flags
VERSION=""
BIN_DIR="$REPO/target/aarch64-unknown-linux-musl/release"
OUT_DIR="$REPO/dist"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version | --bin-dir | --out-dir)
      [[ $# -ge 2 ]] || die "$1 needs a value"
      case "$1" in
        --version) VERSION="$2" ;;
        --bin-dir) BIN_DIR="$2" ;;
        --out-dir) OUT_DIR="$2" ;;
      esac
      shift
      ;;
    -h | --help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
done

# the first version = line in Cargo.toml is [workspace.package] version
if [[ -z "$VERSION" ]]; then
  VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/Cargo.toml" | head -n 1)"
fi
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || die "bad version: '$VERSION'"

# the binaries must exist and be aarch64
for bin in deskpane-daemon deskpane; do
  [[ -f "$BIN_DIR/$bin" ]] || die "$BIN_DIR/$bin not found, build it first (see --help)"
  if command -v file >/dev/null; then
    file -b "$BIN_DIR/$bin" | grep -q 'aarch64' || die "$BIN_DIR/$bin is not an aarch64 binary"
  fi
done

refuse_pack_files "$REPO/pi" "$REPO/deploy"

# lay the bundle out in a temp directory
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
TOP="deskpane-$VERSION-pi-aarch64"
DIR="$STAGE/$TOP"
mkdir -p "$DIR/bin" "$DIR/dashboard" "$DIR/systemd" "$DIR/config"

echo "$VERSION" >"$DIR/VERSION"
install -m 0644 "$REPO/LICENSE" "$REPO/deploy/README.txt" "$REPO/deploy/overlay.env" "$DIR/"
install -m 0755 "$REPO/deploy/install.sh" "$REPO/deploy/uninstall.sh" "$DIR/"
install -m 0755 "$BIN_DIR/deskpane-daemon" "$BIN_DIR/deskpane" "$DIR/bin/"
install -m 0644 "$REPO/pi/dashboard.py" "$REPO/pi/fb_display.py" "$REPO/pi/touch.py" "$REPO/pi/layout.py" \
  "$REPO/pi/deskpane_contract.py" "$REPO/pi/panels.py" \
  "$REPO/pi/charpack.py" "$REPO/pi/charpack_install.py" "$REPO/pi/sprites.py" "$REPO/pi/display.py" \
  "$DIR/dashboard/"
install -m 0644 "$REPO"/deploy/systemd/*.service "$DIR/systemd/"
install -m 0644 "$REPO/deploy/env.example" "$DIR/config/"

# plugin panel files only (never plugin source): plugins/<name>/panel/<name>.panel.toml,
# a regular file whose folder is a plugin name; PANELS on the Pi turns them on
for panel in "$REPO"/plugins/*/panel/*.panel.toml; do
  [[ -f "$panel" && ! -L "$panel" ]] || continue
  name="$(basename "$(dirname "$(dirname "$panel")")")"
  [[ "$name" =~ ^[a-z][a-z0-9-]{0,31}$ && "$(basename "$panel")" == "$name.panel.toml" ]] || continue
  # never through a symlinked plugin or panel folder
  [[ ! -L "$REPO/plugins/$name" && ! -L "$REPO/plugins/$name/panel" ]] || continue
  install -D -m 0644 "$panel" "$DIR/plugins/$name/panel/$name.panel.toml"
done

# the staged bundle itself: no images or zips at all, whatever their name
STAGED_MEDIA="$(find "$DIR" \( -iname '*.png' -o -iname '*.zip' \) -print || true)"
if [[ -n "$STAGED_MEDIA" ]]; then
  sed -n '1,5p' <<<"$STAGED_MEDIA" >&2
  die "the bundle would contain image or zip files (character pack?); refusing to build"
fi

# same bytes for the same inputs: sorted names, root owner, commit time as mtime
MTIME="$(git -C "$REPO" log -1 --format=%ct 2>/dev/null || date +%s)"
mkdir -p "$OUT_DIR"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$MTIME" \
  -C "$STAGE" -cf - "$TOP" | gzip -n -9 >"$OUT_DIR/$ARCHIVE_NAME"

echo "$OUT_DIR/$ARCHIVE_NAME"
