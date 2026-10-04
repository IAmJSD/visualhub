#!/usr/bin/env bash
# Build VisualHub.app as one universal binary -- Apple Silicon and Intel,
# compiled separately and joined with lipo -- signing and notarizing it
# when credentials are present. Always leaves two distributables:
#   dist/VisualHub.dmg   the app, in a window to drag into /Applications
#   dist/VisualHub.zip   the app bundle
# Only these are safe to hand to CI artifact upload, which would otherwise
# strip the bundle's permissions and symlinks and void its signature.
#
# Both architectures build on either kind of Mac; the missing target is
# added with rustup if it isn't installed.
#
# Signing is optional -- without it the app still builds, unsigned:
#   MACOS_CERT_NAME   "Developer ID Application: … (TEAMID)"
#   MACOS_KEYCHAIN    keychain holding that identity, if not the default one
#
# Notarizing needs signing to have happened, plus either
#   MACOS_NOTARY_PROFILE  a profile saved by `notarytool store-credentials`
# or the three pieces that profile would have stored:
#   APPLE_ID, APPLE_APP_SPECIFIC_PASSWORD, APPLE_TEAM_ID
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
profile="${1:-release}"
app="$root/dist/VisualHub.app"
zip="$root/dist/VisualHub.zip"
dmg="$root/dist/VisualHub.dmg"
# What the disk image is assembled in: the app plus the /Applications link,
# and nothing else -- hdiutil images the directory exactly as it finds it.
dmg_stage="$root/target/macos-dmg"
universal="$root/target/universal/$profile/visualhub"
targets=(aarch64-apple-darwin x86_64-apple-darwin)
version="$(grep -m1 '^version = ' "$root/Cargo.toml" | cut -d'"' -f2)"
# The oldest macOS the binary runs on, for both the compiler and the plist.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

# `debug` names a directory under target/, not a cargo flag: it is what
# `cargo build` does with no profile flag at all.
profile_flag=()
if [ "$profile" != "debug" ]; then
    profile_flag=(--profile "$profile")
fi

installed="$(rustup target list --installed 2>/dev/null || true)"
slices=()
for target in "${targets[@]}"; do
    if ! printf '%s\n' "$installed" | grep -qx "$target"; then
        echo "adding the $target target"
        rustup target add "$target"
    fi
    echo "building for $target"
    cargo build ${profile_flag[@]+"${profile_flag[@]}"} --target "$target" --manifest-path "$root/Cargo.toml"
    slices+=("$root/target/$target/$profile/visualhub")
done

mkdir -p "$(dirname "$universal")"
lipo -create "${slices[@]}" -output "$universal"
echo "universal: $(lipo -archs "$universal")"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
sed -e "s/@VERSION@/$version/g" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" \
    "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
cp "$universal" "$app/Contents/MacOS/visualhub"
# Keep the build outputs intact for dsymutil and local debugging; strip only
# the shipping copy, and before signing, which a later change would void.
if [ "$profile" != "debug" ]; then
    xcrun strip -S -x "$app/Contents/MacOS/visualhub"
fi
cp "$root/packaging/macos/visualhub.icns" "$app/Contents/Resources/"
cp "$root/src/ui/LICENSE-SCHIST" "$app/Contents/Resources/"
plutil -lint "$app/Contents/Info.plist" >/dev/null

signed=false
if [ -n "${MACOS_CERT_NAME:-}" ]; then
    echo "signing with $MACOS_CERT_NAME"
    # Expanded as `${keychain[@]+...}` below: bash 3.2, which is what
    # /usr/bin/env finds on a stock macOS, calls an empty array unbound
    # under `set -u`.
    keychain=()
    if [ -n "${MACOS_KEYCHAIN:-}" ]; then
        keychain=(--keychain "$MACOS_KEYCHAIN")
    fi
    codesign --force --options runtime --timestamp \
        --entitlements "$root/packaging/macos/entitlements.plist" \
        ${keychain[@]+"${keychain[@]}"} --sign "$MACOS_CERT_NAME" "$app"
    codesign --verify --strict --verbose=2 "$app"
    signed=true
else
    echo "MACOS_CERT_NAME unset: leaving the app unsigned"
fi

notary=()
if [ -n "${MACOS_NOTARY_PROFILE:-}" ]; then
    notary=(--keychain-profile "$MACOS_NOTARY_PROFILE")
    if [ -n "${MACOS_KEYCHAIN:-}" ]; then
        notary+=(--keychain "$MACOS_KEYCHAIN")
    fi
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
    notary=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID"
            --password "${APPLE_APP_SPECIFIC_PASSWORD:-}")
fi

if [ "$signed" = true ] && [ ${#notary[@]} -gt 0 ]; then
    echo "notarizing the app"
    # Notarization takes a zip, but the ticket is stapled to the bundle, so
    # this upload copy is scratch -- the shippable zip gets made afterwards.
    ditto -c -k --keepParent "$app" "$root/dist/upload.zip"
    xcrun notarytool submit "$root/dist/upload.zip" "${notary[@]}" --wait
    rm -f "$root/dist/upload.zip"

    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
    # What Gatekeeper will say on a machine that has never seen the app.
    spctl --assess --type exec --verbose=2 "$app"
elif [ "$signed" = true ]; then
    echo "no notarization credentials: signed but not notarized"
else
    echo "skipping notarization: nothing is signed"
fi

rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"

# The disk image, built last and from the finished bundle: whatever signing
# and stapling happened above is already inside it. ditto rather than cp,
# which is the only copy that carries a bundle's symlinks and extended
# attributes across intact -- a signature does not survive losing them.
rm -rf "$dmg_stage"
mkdir -p "$dmg_stage"
ditto "$app" "$dmg_stage/VisualHub.app"
# The drag-to-install target. A symlink to the real /Applications, so the
# window Finder opens has somewhere to drop the app.
ln -s /Applications "$dmg_stage/Applications"
rm -f "$dmg"
hdiutil create -volname VisualHub -srcfolder "$dmg_stage" -ov -format UDZO "$dmg"
rm -rf "$dmg_stage"

# The image is signed and notarized in its own right, not just for what it
# carries: Gatekeeper assesses the .dmg the moment it is opened, which is
# before anything inside it has been looked at.
if [ "$signed" = true ]; then
    codesign --force --timestamp \
        ${keychain[@]+"${keychain[@]}"} --sign "$MACOS_CERT_NAME" "$dmg"
    codesign --verify --strict --verbose=2 "$dmg"
fi
if [ "$signed" = true ] && [ ${#notary[@]} -gt 0 ]; then
    echo "notarizing the disk image"
    xcrun notarytool submit "$dmg" "${notary[@]}" --wait
    # A disk image does take a stapled ticket, so this is also the check on
    # the submission: stapling a rejected image fails rather than passing.
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
fi

echo "built $app ($version, $(lipo -archs "$app/Contents/MacOS/visualhub"))"
echo "built $zip"
echo "built $dmg"
