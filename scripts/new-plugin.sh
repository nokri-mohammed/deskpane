#!/usr/bin/env bash
# Scaffolds a new deskpane plugin (T-067) by copying the example-report
# template and rewriting its name. The new plugin is a data source that sends a
# device report; edit source/main.rs to send your own numbers, add a panel
# later. Nothing here touches the Pi.
#
# Usage: scripts/new-plugin.sh <name>
#   <name>   lowercase letters, digits and hyphens, e.g. weather or disk-health
#
# After it runs:
#   cd plugins/<name>/source && cargo test
#   scripts/preview.sh --plugin plugins/<name>
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMPLATE="example-report"
TEMPLATE_DIR="$REPO/plugins/$TEMPLATE"

die() {
  echo "new-plugin.sh: error: $*" >&2
  exit 1
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 0
fi

NAME="${1:-}"
if [[ -z "$NAME" ]]; then
  die "a plugin name is required (e.g. weather). See --help."
fi

# a plugin name becomes a folder, a crate name and a binary name, so keep it to
# the safe shape: lowercase, digits, hyphens, starting with a letter
if [[ ! "$NAME" =~ ^[a-z][a-z0-9-]*$ ]]; then
  die "name must be lowercase letters, digits and hyphens, starting with a letter: got '$NAME'"
fi

if [[ "$NAME" == "$TEMPLATE" ]]; then
  die "'$TEMPLATE' is the template itself; pick another name"
fi

if [[ ! -d "$TEMPLATE_DIR" ]]; then
  die "template not found at plugins/$TEMPLATE"
fi

DEST="$REPO/plugins/$NAME"
if [[ -e "$DEST" ]]; then
  die "plugins/$NAME already exists"
fi

# copy the template, then drop build output and the lockfile; the lockfile is
# regenerated for the new crate name so it is correct from the first build
cp -r "$TEMPLATE_DIR" "$DEST"
rm -rf "$DEST/source/target"
rm -f "$DEST/source/Cargo.lock"

# rewrite the name everywhere it appears: the crate and binary name
# (deskpane-<template> to deskpane-<name>) and the bare name (<template> to
# <name>) in the manifest, Cargo.toml and README
for file in "$DEST/manifest.toml" "$DEST/source/Cargo.toml" "$DEST/source/main.rs" "$DEST/README.md"; do
  if [[ -f "$file" ]]; then
    sed -i "s/deskpane-$TEMPLATE/deskpane-$NAME/g; s/\\b$TEMPLATE\\b/$NAME/g" "$file"
  fi
done

# generate the lockfile for the new crate so cargo build --locked works in CI
if command -v cargo >/dev/null 2>&1; then
  ( cd "$DEST/source" && cargo generate-lockfile >/dev/null 2>&1 ) || true
fi

echo "new-plugin.sh: created plugins/$NAME from the $TEMPLATE template"
echo
echo "next:"
echo "  1. edit plugins/$NAME/manifest.toml (author, what it declares)"
echo "  2. edit plugins/$NAME/source/main.rs (send your own numbers)"
echo "  3. cd plugins/$NAME/source && cargo test"
echo "  4. scripts/preview.sh --plugin plugins/$NAME"
