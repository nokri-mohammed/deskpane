#!/usr/bin/env bash
# Runs the Pi dashboard on this PC (T-061): the real pi/dashboard.py in a
# window (WSLg, X11 or Wayland) or offscreen, against a mock daemon or the
# real Rust daemon on loopback. Nothing here touches the Pi path.
#
# Usage: scripts/preview.sh [options]
#   (no options)              window, scenario busy, scale 2
#   --scenario NAME           busy (alias normal), single, needs-you, worst, offline, battery, empty
#   --scale N                 window size, 1 to 4 times 480 x 320
#   --headless                no window: PNG frames in ~/.cache/deskpane-preview/
#   --headless --serve PORT   and show them live at http://localhost:PORT (127.0.0.1 only)
#   --headless --frames N     write N numbered frames, then exit
#   --tap-every S             tap every S seconds (headless), to walk the pages
#   --real                    real daemon + reporter + 'deskpane agent test' on 127.0.0.1:18765
#   --no-pack                 built-in icons even if a character pack is installed
# Window keys: click, Space or Right arrow taps; N network screen; S saves a PNG; Q or Esc quits.
#
# The first run creates .venv-preview/ (git-ignored) with pygame 2.6.1.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENV="$REPO/.venv-preview"
PYGAME_VERSION="2.6.1"

die() {
  echo "preview.sh: error: $*" >&2
  exit 1
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  sed -n '2,17p' "$0"
  exit 0
fi

# Python 3.11 or newer; a pyenv shim may point at an older one, so check each
find_python() {
  local candidate
  for candidate in /usr/bin/python3 python3.13 python3.12 python3.11 python3; do
    if command -v "$candidate" >/dev/null 2>&1 &&
      "$candidate" -c 'import sys, venv; sys.exit(0 if sys.version_info >= (3, 11) else 1)' >/dev/null 2>&1; then
      command -v "$candidate"
      return 0
    fi
  done
  return 1
}

# the venv once, then again only if pygame is missing or another version
if ! "$VENV/bin/python" -c "import pygame, sys; sys.exit(0 if pygame.version.ver == '$PYGAME_VERSION' else 1)" \
  >/dev/null 2>&1; then
  PYTHON="$(find_python)" || die "Python 3.11 or newer with venv is needed (e.g. sudo apt install python3-venv)"
  echo "preview.sh: creating $VENV with pygame $PYGAME_VERSION (once)" >&2
  "$PYTHON" -m venv "$VENV"
  "$VENV/bin/python" -m pip install --quiet --disable-pip-version-check "pygame==$PYGAME_VERSION"
fi

# flat imports (import charpack) work as on the Pi
export PYTHONPATH="$REPO/pi"
export PYGAME_HIDE_SUPPORT_PROMPT=1
exec "$VENV/bin/python" "$REPO/pi/preview.py" "$@"
