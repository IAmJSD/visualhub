#!/usr/bin/env bash
# Rebuild packaging/macos/visualhub.icns from assets/icon.svg: every size
# macOS asks for, at 1x and 2x. Needs rsvg-convert (`brew install librsvg`).
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
iconset="$root/target/VisualHub.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" "$root/assets/icon.svg" -o "$iconset/icon_${size}x${size}.png"
    rsvg-convert -w $((size * 2)) -h $((size * 2)) "$root/assets/icon.svg" -o "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o "$root/packaging/macos/visualhub.icns"
rm -rf "$iconset"
echo "built packaging/macos/visualhub.icns"
