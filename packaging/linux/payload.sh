# Sourced, not run: the file layout every Linux package installs.
#
# The AppImage and the three native packages (packages.sh) ship the same
# files under the same prefix; only the metadata wrapped around them
# differs. Keeping the layout here means a new file lands in all four at
# once rather than in whichever script was edited.

payload_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
payload_root="$(cd "$payload_dir/../.." && pwd)"
payload_version="$(sed -n '0,/^version = /s/^version = "\(.*\)"/\1/p' "$payload_root/Cargo.toml")"

# stage_payload DIR -- fill DIR with the /usr tree a package installs.
stage_payload() {
    local dest="$1"

    install -Dm755 "$payload_root/target/release/visualhub" "$dest/usr/bin/visualhub"
    # Strip only the shipping copy; the original stays for debugging.
    strip --strip-unneeded "$dest/usr/bin/visualhub"
    install -Dm644 "$payload_dir/visualhub.desktop" \
        "$dest/usr/share/applications/com.infrawrench.visualhub.desktop"
    # The desktop entry looks its icon up by the Icon= key, so the file
    # carries the app ID as its name.
    install -Dm644 "$payload_dir/visualhub.png" \
        "$dest/usr/share/icons/hicolor/256x256/apps/com.infrawrench.visualhub.png"
    install -Dm644 "$payload_root/src/ui/LICENSE-SCHIST" \
        "$dest/usr/share/licenses/visualhub/LICENSE-SCHIST"
}
