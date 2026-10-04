#!/usr/bin/env bash
# Build the native Linux packages from one already-built binary:
#
#   dist/visualhub_VERSION-1_ARCH.deb            dpkg-deb, or ar+tar without it
#   dist/visualhub-VERSION-1.ARCH.rpm            needs rpmbuild
#   dist/visualhub-VERSION-1-ARCH.pkg.tar.zst    tar+zstd, .MTREE needs bsdtar
#
# Each format is skipped, loudly, when the tool it needs is absent, so a
# machine with none of them still gets the ones it can build. Packages are
# for the host architecture only: cargo builds one target per invocation.
set -euo pipefail

# shellcheck source=packaging/linux/payload.sh
source "$(dirname "$0")/payload.sh"

version="$payload_version"
# The package revision, bumped only when the packaging changes under a
# version that has already shipped.
release=1
summary="A native GitHub client"
url="https://github.com/IAmJSD/visualhub"
packager="${PACKAGER:-Astrid Gealer <astrid@infrawrench.com>}"
builddate="${SOURCE_DATE_EPOCH:-$(date +%s)}"

machine="$(uname -m)"
case "$machine" in
    x86_64)  deb_arch=amd64; rpm_arch=x86_64;  pkg_arch=x86_64 ;;
    aarch64) deb_arch=arm64; rpm_arch=aarch64; pkg_arch=aarch64 ;;
    *) echo "error: no package architecture known for $machine" >&2; exit 1 ;;
esac

dist="$payload_root/dist"
# Scratch, not a deliverable: dist/ is uploaded wholesale by CI.
build="$payload_root/target/linux-packages"

cargo build --release --manifest-path "$payload_root/Cargo.toml"
mkdir -p "$dist"
rm -rf "$build"

built=()

# GPUI draws with Vulkan and talks Wayland or X11; fontconfig, wayland and
# the Vulkan loader are dlopen'd, so they're listed by hand -- nothing in
# the ELF points at them for a dependency generator to find.

build_deb() {
    local work="$build/deb" out size
    mkdir -p "$work/DEBIAN"
    stage_payload "$work"
    # Debian keeps licences in the documentation directory and nowhere else.
    install -Dm644 "$work/usr/share/licenses/visualhub/LICENSE-SCHIST" \
        "$work/usr/share/doc/visualhub/copyright"
    rm -rf "$work/usr/share/licenses"
    # dpkg applies the archive's ./ entry to / on install, so it must be 755.
    chmod 755 "$work"

    size="$(du -sk --exclude=DEBIAN "$work" | cut -f1)"
    cat > "$work/DEBIAN/control" <<EOF
Package: visualhub
Version: $version-$release
Section: devel
Priority: optional
Architecture: $deb_arch
Maintainer: $packager
Installed-Size: $size
Depends: libc6, libfontconfig1, libfreetype6, libxcb1, libxkbcommon0,
  libxkbcommon-x11-0, libwayland-client0, libvulkan1, hicolor-icon-theme
Recommends: gh
Homepage: $url
Description: $summary
 VisualHub covers what you do on github.com -- issues, pull requests,
 reviews, Actions, releases and more -- in a native window.
EOF
    ( cd "$work" && find usr -type f -exec md5sum {} + > DEBIAN/md5sums )

    cat > "$work/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -qtf /usr/share/icons/hicolor || true
fi
EOF
    cp "$work/DEBIAN/postinst" "$work/DEBIAN/postrm"
    chmod 755 "$work/DEBIAN/postinst" "$work/DEBIAN/postrm"

    out="$dist/visualhub_${version}-${release}_${deb_arch}.deb"
    rm -f "$out"
    if command -v dpkg-deb >/dev/null 2>&1; then
        dpkg-deb --root-owner-group --build "$work" "$out" >/dev/null
    else
        # No dpkg here: a .deb is an ar archive of exactly three members in
        # exactly this order, which binutils' ar and tar can write.
        local ar="$build/deb-ar"
        mkdir -p "$ar"
        echo 2.0 > "$ar/debian-binary"
        tar --owner=0 --group=0 --numeric-owner -czf "$ar/control.tar.gz" -C "$work/DEBIAN" .
        tar --owner=0 --group=0 --numeric-owner --exclude=./DEBIAN -czf "$ar/data.tar.gz" -C "$work" .
        ( cd "$ar" && ar rcD "$out" debian-binary control.tar.gz data.tar.gz )
    fi
    built+=("$out")
}

build_rpm() {
    if ! command -v rpmbuild >/dev/null 2>&1; then
        echo "rpmbuild not found; skipping the .rpm" >&2
        return
    fi
    local work="$build/rpm"
    mkdir -p "$work/payload"
    stage_payload "$work/payload"

    # The payload is built and staged already, so the spec only wraps it.
    cat > "$work/visualhub.spec" <<EOF
%global debug_package %{nil}
%global _build_id_links none

Name:           visualhub
Version:        $version
Release:        $release
Summary:        $summary
License:        MIT
URL:            $url
Requires:       fontconfig
Requires:       freetype
Requires:       libxcb
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       libwayland-client
Requires:       vulkan-loader
Requires:       hicolor-icon-theme
Recommends:     gh

%description
VisualHub covers what you do on github.com -- issues, pull requests,
reviews, Actions, releases and more -- in a native window.

%install
cp -a %{payload}/. %{buildroot}/

%files
%license %{_datadir}/licenses/visualhub/LICENSE-SCHIST
%{_bindir}/visualhub
%{_datadir}/applications/com.infrawrench.visualhub.desktop
%{_datadir}/icons/hicolor/256x256/apps/com.infrawrench.visualhub.png

%post
update-desktop-database -q %{_datadir}/applications &>/dev/null || :
gtk-update-icon-cache -qtf %{_datadir}/icons/hicolor &>/dev/null || :

%postun
update-desktop-database -q %{_datadir}/applications &>/dev/null || :
gtk-update-icon-cache -qtf %{_datadir}/icons/hicolor &>/dev/null || :
EOF

    rpmbuild -bb --quiet --target "$rpm_arch" \
        --define "_topdir $work" \
        --define "payload $work/payload" \
        --define "_rpmdir $dist" \
        --define "_build_name_fmt %%{NAME}-%%{VERSION}-%%{RELEASE}.%%{ARCH}.rpm" \
        "$work/visualhub.spec"
    built+=("$dist/visualhub-${version}-${release}.${rpm_arch}.rpm")
}

build_pkg() {
    local work="$build/pkg" out size dep
    mkdir -p "$work"
    stage_payload "$work"

    size="$(du -sb "$work" | cut -f1)"
    cat > "$work/.PKGINFO" <<EOF
pkgname = visualhub
pkgbase = visualhub
pkgver = $version-$release
pkgdesc = $summary
url = $url
builddate = $builddate
packager = $packager
size = $size
arch = $pkg_arch
license = MIT
EOF
    for dep in fontconfig freetype2 hicolor-icon-theme libxcb libxkbcommon \
               libxkbcommon-x11 vulkan-icd-loader wayland; do
        echo "depend = $dep" >> "$work/.PKGINFO"
    done
    echo "optdepend = github-cli: sign in with your gh login" >> "$work/.PKGINFO"
    chmod 644 "$work/.PKGINFO"

    # pacman reads the metadata from the front of the stream, so .PKGINFO
    # goes in first and the payload last.
    local entries=(.PKGINFO)
    if command -v bsdtar >/dev/null 2>&1; then
        ( cd "$work" && LC_ALL=C bsdtar -czf .MTREE --format=mtree \
            --uid 0 --gid 0 --uname root --gname root \
            --options='!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link' \
            .PKGINFO usr )
        chmod 644 "$work/.MTREE"
        entries+=(.MTREE)
    else
        echo "bsdtar not found; the .pkg.tar.zst ships without a .MTREE" \
             "(it installs, but \`pacman -Qkk\` cannot verify it)" >&2
    fi
    entries+=(usr)

    out="$dist/visualhub-${version}-${release}-${pkg_arch}.pkg.tar.zst"
    tar --owner=0 --group=0 --numeric-owner -C "$work" -cf - "${entries[@]}" \
        | zstd -q -T0 -19 -c > "$out"
    built+=("$out")
}

build_deb
build_rpm
build_pkg

for f in "${built[@]}"; do
    echo "built $f" "$(du -h "$f" | cut -f1)"
done
