#!/usr/bin/env bash
# Rebuild the toolbar button's icons from assets/icon.svg: the app icon,
# lit, for a page VisualHub can open, and a grey copy for everywhere else.
# Needs rsvg-convert (`brew install librsvg`).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
svg="$here/../assets/icon.svg"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The tile fills the button: the app icon's margin is room for a shadow
# the toolbar doesn't draw.
sed -e 's/viewBox="0 0 1024 1024"/viewBox="92 92 840 840"/' "$svg" > "$work/on.svg"
sed -e 's/#8B5CF6/#9CA3AF/; s/#6D28D9/#6B7280/g; s/#3B1A8F/#4B5563/; s/#1B0B4A/#1F2937/g' \
    "$work/on.svg" > "$work/off.svg"

for state in on off; do
    for size in 16 32 48 128; do
        rsvg-convert -w "$size" -h "$size" "$work/$state.svg" -o "$here/icons/$state-$size.png"
    done
done
echo "built extension/icons"
