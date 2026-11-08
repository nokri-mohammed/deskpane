#!/usr/bin/env bash
# Licence guard (T-053), run by CI: fails when git tracks anything under
# assets/characters/, any file named like a character pack image
# (NPC_<n>.png, <Name>_emote_16x16.png) or any zip, in any letter case.
# Packs such as the paid one in assets/characters/ are licensed to their
# owner and must never be redistributed through this repo.
#
# It only checks the current tree. A pack that was ever committed stays in
# the git history even after `git rm`, and needs a history rewrite (as was
# done for T-004) before the repo is pushed anywhere public.
#
# Usage: scripts/check-no-pack-files.sh   (from anywhere inside the repo)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

TRACKED="$(git -C "$REPO" ls-files -- ':(icase)assets/characters/' ':(glob,icase)**/NPC_*.png' \
  ':(glob,icase)**/*_emote_16x16.png' ':(glob,icase)**/*.zip')"
if [[ -n "$TRACKED" ]]; then
  echo "$TRACKED" >&2
  echo "check-no-pack-files.sh: error: character pack files are tracked by git (above)." >&2
  echo "  Remove them with: git rm --cached <file>. The pack licence forbids redistribution." >&2
  exit 1
fi
echo "check-no-pack-files.sh: no character pack files tracked"
