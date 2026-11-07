#!/usr/bin/env bash
# Builds the Pi release bundle, pi-homelab-dashboard-pi-aarch64.tar.gz, from
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
ARCHIVE_NAME="pi-homelab-dashboard-pi-aarch64.tar.gz"

die() {
  echo "package-pi-bundle.sh: error: $*" >&2
  exit 1
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
for bin in daemon reporter; do
  [[ -f "$BIN_DIR/$bin" ]] || die "$BIN_DIR/$bin not found, build it first (see --help)"
  if command -v file >/dev/null; then
    file -b "$BIN_DIR/$bin" | grep -q 'aarch64' || die "$BIN_DIR/$bin is not an aarch64 binary"
  fi
done

# lay the bundle out in a temp directory
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
TOP="pi-homelab-dashboard-$VERSION-pi-aarch64"
DIR="$STAGE/$TOP"
mkdir -p "$DIR/bin" "$DIR/dashboard" "$DIR/systemd" "$DIR/config"

echo "$VERSION" >"$DIR/VERSION"
install -m 0644 "$REPO/LICENSE" "$REPO/deploy/README.txt" "$REPO/deploy/overlay.env" "$DIR/"
install -m 0755 "$REPO/deploy/install.sh" "$REPO/deploy/uninstall.sh" "$DIR/"
install -m 0755 "$BIN_DIR/daemon" "$BIN_DIR/reporter" "$DIR/bin/"
install -m 0644 "$REPO/pi/dashboard.py" "$REPO/pi/fb_display.py" "$REPO/pi/touch.py" "$DIR/dashboard/"
install -m 0644 "$REPO"/deploy/systemd/*.service "$DIR/systemd/"
install -m 0644 "$REPO/deploy/env.example" "$DIR/config/"

# same bytes for the same inputs: sorted names, root owner, commit time as mtime
MTIME="$(git -C "$REPO" log -1 --format=%ct 2>/dev/null || date +%s)"
mkdir -p "$OUT_DIR"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$MTIME" \
  -C "$STAGE" -cf - "$TOP" | gzip -n -9 >"$OUT_DIR/$ARCHIVE_NAME"

echo "$OUT_DIR/$ARCHIVE_NAME"
