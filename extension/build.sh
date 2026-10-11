#!/usr/bin/env bash
# Assemble the extension for one browser family into a directory:
#   extension/build.sh chromium|firefox OUT_DIR
# The app writes the same files itself when it installs the extension
# (src/browsers.rs); this is for the release, which signs Firefox's.
set -euo pipefail

family="${1:?chromium or firefox}"
out="${2:?output directory}"
here="$(cd "$(dirname "$0")" && pwd)"
version="$(grep -m1 '^version = ' "$here/../Cargo.toml" | cut -d'"' -f2)"

rm -rf "$out"
mkdir -p "$out/icons"
cp "$here/background.js" "$here/supported.js" "$here/hosts.js" "$out/"
cp "$here"/icons/*.png "$out/icons/"
sed "s/@VERSION@/$version/" "$here/manifest.$family.json" > "$out/manifest.json"
echo "extension for $family $version in $out"
