#!/usr/bin/env bash
# Build an AppImage. Requires `appimagetool` on PATH (or set APPIMAGETOOL).
# The native packages -- .deb, .rpm, .pkg.tar.zst -- are packages.sh's job;
# both ship the same tree, staged by payload.sh.
set -euo pipefail

# shellcheck source=packaging/linux/payload.sh
source "$(dirname "$0")/payload.sh"
root="$payload_root"
# Scratch, not a deliverable: dist/ is uploaded wholesale by CI, and an
# AppDir in there would collide with the real artifacts on the release.
appdir="$root/target/VisualHub.AppDir"
tool="${APPIMAGETOOL:-appimagetool}"

cargo build --release --manifest-path "$root/Cargo.toml"

rm -rf "$appdir"
stage_payload "$appdir"

# Carry every shared library the binary needs except glibc's own, which
# must come from the host it runs on.
copy_elf_closure() {
    local queue=("$1") seen="" current source soname
    while ((${#queue[@]})); do
        current="${queue[0]}"
        queue=("${queue[@]:1}")
        [[ -f "$current" ]] || continue
        while read -r first second third _; do
            if [[ "$second" == "=>" ]]; then
                source="$third"
            elif [[ "$first" == /* ]]; then
                source="$first"
            else
                continue
            fi
            [[ "$source" == /* && -f "$source" ]] || continue
            soname="$(basename "$source")"
            case "$soname" in
                libc.so.6|libm.so.6|libdl.so.2|libpthread.so.0|librt.so.1|ld-linux-*) continue ;;
            esac
            [[ "$seen" == *"|$soname|"* ]] && continue
            seen+="|$soname|"
            install -Dm755 "$source" "$appdir/usr/lib/$soname"
            queue+=("$source")
        done < <(ldd "$current" 2>/dev/null || true)
    done
}

mkdir -p "$appdir/usr/lib"
copy_elf_closure "$appdir/usr/bin/visualhub"

# appimagetool reads the desktop entry from the AppDir root and looks the
# icon up there by its Icon= key, so both are duplicated out of usr/.
cp "$appdir/usr/share/applications/com.infrawrench.visualhub.desktop" "$appdir/visualhub.desktop"
cp "$appdir/usr/share/icons/hicolor/256x256/apps/com.infrawrench.visualhub.png" \
   "$appdir/com.infrawrench.visualhub.png"

cat > "$appdir/AppRun" <<'RUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$HERE/usr/bin/visualhub" "$@"
RUN
chmod +x "$appdir/AppRun"

mkdir -p "$root/dist"
out="$root/dist/VisualHub-$(uname -m).AppImage"
if command -v "$tool" >/dev/null 2>&1; then
    "$tool" "$appdir" "$out"
    echo "built $out"
else
    echo "appimagetool not found; the AppDir is ready at $appdir" >&2
fi
