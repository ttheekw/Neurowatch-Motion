#!/usr/bin/env bash
# Build nw-motion Linux release packages: a portable .tar.gz and native .deb,
# .rpm and Arch .pkg.tar.zst, each with its own GStreamer and FFmpeg (only
# the plugins the analysis uses), so no distribution codec packages are needed.
#
# The GStreamer SDK is NeuroWatch's own release build
# (../Neurowatch/scripts/build_release_gstreamer.sh): the same versions and
# the same LGPL FFmpeg as NeuroWatch's packages. Packages are built natively:
# --arch must match the host. Nothing is published.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG_NAME="nw-motion"
PKG_RELEASE="1"
PKG_SUMMARY="NeuroWatch Motion - motion detection plugin for NeuroWatch"
PKG_LICENSE="Apache-2.0"
PKG_URL="https://github.com/ttheekw/Neurowatch-Motion"
PKG_MAINTAINER="Neurowatch Developers <dev@neurowatch.invalid>"
INSTALL_PREFIX="/opt/nw-motion"
USER_UNITS="/usr/lib/systemd/user"
SERVICE_HOOK="$ROOT/scripts/packaging/session-service.sh"

# The GStreamer plugins the analysis pipelines use (src/worker.rs).
PLUGINS=(coreelements app playback typefindfunctions videoconvertscale
    videoparsersbad libav)
# Hardware decoding (--hardware-decoding), when the SDK has them. They may
# need host libraries that are only recommended: without them the plugin
# does not load and decoding stays in software.
OPTIONAL_PLUGINS=(va nvcodec)
# Libraries taken from the SDK; everything else comes from the host.
BUNDLED_LIBS='^lib(gst|av|sw|postproc|orc-0\.4)'
# Host libraries the bundle may need directly, and the packages that hold them.
HOST_LIBS='^(libc|libm|libdl|libpthread|librt|libgcc_s|ld-linux-[a-z0-9_-]+|libglib-2\.0|libgobject-2\.0|libgmodule-2\.0|libgio-2\.0|libz|libstdc\+\+)\.so'
OPTIONAL_HOST_LIBS='^(libva|libva-drm|libdrm|libgudev-1\.0|libudev|libEGL|libGL|libGLESv2|libX11|libX11-xcb|libxcb|libgstgl)'
DEB_DEPENDS=(libgcc-s1 libstdc++6 zlib1g)
DEB_RECOMMENDS=(libva2 libva-drm2 libdrm2 libgudev-1.0-0)
RPM_REQUIRES=(libgcc libstdc++ zlib)
RPM_RECOMMENDS=(libva libdrm libgudev)
ARCH_DEPENDS=(gcc-libs zlib)
ARCH_OPTDEPENDS=("libva: hardware decoding (--hardware-decoding)" "libgudev: hardware decoding")

usage() {
    cat <<'USAGE'
Usage: scripts/package_linux.sh --version VERSION --arch ARCH --format FORMAT[,FORMAT...]
                                [--gstreamer-sdk DIR] [--out-dir DIR] [--dry-run]

  --version        release version (v1.2.3 or 1.2.3); must match Cargo.toml
  --arch           x86_64 | arm64 (aliases: amd64, aarch64); must match the host
  --format         portable | deb | rpm | arch (x86_64 only); comma separated or repeated
  --gstreamer-sdk  a built SDK to use (default: build/gstreamer-sdk, built if missing
                   with $NEUROWATCH_DIR/scripts/build_release_gstreamer.sh;
                   NEUROWATCH_DIR defaults to ../Neurowatch)
  --out-dir        output directory (default: dist/release)
  --dry-run        validate arguments and print the artifact paths without building

NW_MOTION_LENIENT_DEPS=1 reports unknown host libraries instead of failing
(for a test bundle made from a system GStreamer, never for a release).
USAGE
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

VERSION=""
ARCH=""
FORMATS=()
OUT_DIR="$ROOT/dist/release"
SDK=""
DRY_RUN=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) VERSION="${2:-}"; shift 2 ;;
        --version=*) VERSION="${1#*=}"; shift ;;
        --arch) ARCH="${2:-}"; shift 2 ;;
        --arch=*) ARCH="${1#*=}"; shift ;;
        --format) IFS=, read -r -a parts <<< "${2:-}"; FORMATS+=("${parts[@]}"); shift 2 ;;
        --format=*) IFS=, read -r -a parts <<< "${1#*=}"; FORMATS+=("${parts[@]}"); shift ;;
        --gstreamer-sdk) SDK="${2:-}"; shift 2 ;;
        --gstreamer-sdk=*) SDK="${1#*=}"; shift ;;
        --out-dir) OUT_DIR="${2:-}"; shift 2 ;;
        --out-dir=*) OUT_DIR="${1#*=}"; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; fail "unknown argument: $1" ;;
    esac
done
[[ -n "$VERSION" ]] || { usage >&2; fail '--version is required'; }
[[ -n "$ARCH" ]] || { usage >&2; fail '--arch is required'; }
[[ ${#FORMATS[@]} -gt 0 ]] || { usage >&2; fail '--format is required'; }

VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$ ]] \
    || fail "version must look like 1.2.3 or 1.2.3-rc.1, got: $VERSION"
CARGO_VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
[[ "${VERSION%%-*}" == "$CARGO_VERSION" ]] \
    || fail "version $VERSION does not match Cargo.toml ($CARGO_VERSION)"

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64"; DEB_ARCH="amd64"; RPM_ARCH="x86_64"; PAC_ARCH="x86_64" ;;
    arm64|aarch64) ARCH="arm64"; DEB_ARCH="arm64"; RPM_ARCH="aarch64"; PAC_ARCH="" ;;
    *) fail "unsupported --arch: $ARCH (expected x86_64 or arm64)" ;;
esac
for format in "${FORMATS[@]}"; do
    case "$format" in
        portable|deb|rpm|arch) ;;
        *) fail "unsupported --format: $format (expected portable, deb, rpm or arch)" ;;
    esac
    [[ "$format" != arch || -n "$PAC_ARCH" ]] \
        || fail "--format arch is x86_64 only (Arch Linux ARM is not a supported target)"
done

# Pre-releases sort before the release: deb/rpm "~", pacman no "-".
DEB_VERSION="${VERSION/-/\~}-${PKG_RELEASE}"
RPM_VERSION="${VERSION/-/\~}"
PAC_VERSION="${VERSION/-/}"

artifact_path() {
    case "$1" in
        portable) printf '%s/%s-%s-linux-%s.tar.gz\n' "$OUT_DIR" "$PKG_NAME" "$VERSION" "$ARCH" ;;
        deb) printf '%s/%s_%s_%s.deb\n' "$OUT_DIR" "$PKG_NAME" "$DEB_VERSION" "$DEB_ARCH" ;;
        rpm) printf '%s/%s-%s-%s.%s.rpm\n' "$OUT_DIR" "$PKG_NAME" "$RPM_VERSION" "$PKG_RELEASE" "$RPM_ARCH" ;;
        arch) printf '%s/%s-%s-%s-%s.pkg.tar.zst\n' "$OUT_DIR" "$PKG_NAME" "$PAC_VERSION" "$PKG_RELEASE" "$PAC_ARCH" ;;
    esac
}
if [[ $DRY_RUN -eq 1 ]]; then
    for format in "${FORMATS[@]}"; do artifact_path "$format"; done
    exit 0
fi

HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
    x86_64|amd64) HOST_ARCH="x86_64" ;;
    aarch64|arm64) HOST_ARCH="arm64" ;;
esac
[[ "$HOST_ARCH" == "$ARCH" ]] || fail "packages are built natively; host is $HOST_ARCH, --arch is $ARCH"

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}
for tool in cargo readelf objdump pkg-config; do require_command "$tool"; done
for format in "${FORMATS[@]}"; do
    case "$format" in
        deb) require_command dpkg-deb ;;
        rpm) require_command rpmbuild ;;
        arch) require_command bsdtar; require_command zstd ;;
    esac
done

WORK_DIR="$ROOT/build/release-linux-$ARCH"
BUNDLE_DIR="$WORK_DIR/bundle/$PKG_NAME"
rm -rf "$WORK_DIR"
mkdir -p "$BUNDLE_DIR/bin" "$BUNDLE_DIR/gstreamer/lib/gstreamer-1.0" "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

echo "[1/5] GStreamer SDK"
if [[ -z "$SDK" ]]; then
    SDK="$ROOT/build/gstreamer-sdk"
    if [[ ! -d "$SDK/lib/gstreamer-1.0" ]]; then
        NEUROWATCH_DIR="${NEUROWATCH_DIR:-$ROOT/../Neurowatch}"
        [[ -f "$NEUROWATCH_DIR/scripts/build_release_gstreamer.sh" ]] \
            || fail "no SDK and no NeuroWatch checkout at $NEUROWATCH_DIR (set NEUROWATCH_DIR)"
        rm -rf "$SDK"
        bash "$NEUROWATCH_DIR/scripts/build_release_gstreamer.sh" "$SDK" "$ROOT/build/media-build"
    fi
fi
SDK="$(cd "$SDK" && pwd)"
[[ -d "$SDK/lib/gstreamer-1.0" ]] || fail "not a GStreamer SDK: $SDK"

echo "[2/5] Building nw-motion (release)"
# The bundle's libraries resolve from the binary's own folder (DT_RPATH, which
# unlike DT_RUNPATH also serves the libraries and plugins it loads), so the
# program runs without a launcher script or LD_LIBRARY_PATH.
export PKG_CONFIG_PATH="$SDK/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,\$ORIGIN/../gstreamer/lib -C link-arg=-Wl,--disable-new-dtags" \
    cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked
install -m 0755 "${CARGO_TARGET_DIR:-$ROOT/target}/release/nw-motion" "$BUNDLE_DIR/bin/nw-motion"
ln -s bin/nw-motion "$BUNDLE_DIR/nw-motion"
install -m 0644 "$ROOT/LICENSE" "$ROOT/NOTICE" "$ROOT/README.md" "$BUNDLE_DIR/"

echo "[3/5] Bundling GStreamer"
GST_LIB="$BUNDLE_DIR/gstreamer/lib"
for plugin in "${PLUGINS[@]}"; do
    [[ -f "$SDK/lib/gstreamer-1.0/libgst$plugin.so" ]] || fail "SDK lacks the $plugin plugin"
    cp -L "$SDK/lib/gstreamer-1.0/libgst$plugin.so" "$GST_LIB/gstreamer-1.0/"
done
for plugin in "${OPTIONAL_PLUGINS[@]}"; do
    if [[ -f "$SDK/lib/gstreamer-1.0/libgst$plugin.so" ]]; then
        cp -L "$SDK/lib/gstreamer-1.0/libgst$plugin.so" "$GST_LIB/gstreamer-1.0/"
    else
        echo "  (SDK has no $plugin plugin: left out)"
    fi
done
needed() {
    readelf -d "$1" | sed -n 's/.*(NEEDED).*Shared library: \[\(.*\)\]/\1/p'
}
# The SDK libraries the binary and plugins need, following their own needs.
queue=("$BUNDLE_DIR/bin/nw-motion" "$GST_LIB/gstreamer-1.0/"*.so)
while [[ ${#queue[@]} -gt 0 ]]; do
    file="${queue[0]}"
    queue=("${queue[@]:1}")
    while read -r soname; do
        [[ -n "$soname" && "$soname" =~ $BUNDLED_LIBS && ! -e "$GST_LIB/$soname" ]] || continue
        [[ -e "$SDK/lib/$soname" ]] || continue
        cp -L "$SDK/lib/$soname" "$GST_LIB/$soname"
        queue+=("$GST_LIB/$soname")
    done < <(needed "$file")
done
# Every direct need is bundled or a known host library. What the required
# plugins load must run everywhere; only the hardware plugins (and libraries
# nothing else needs) may need the recommended host libraries.
required=("$BUNDLE_DIR/bin/nw-motion")
for plugin in "${PLUGINS[@]}"; do required+=("$GST_LIB/gstreamer-1.0/libgst$plugin.so"); done
queue=("${required[@]}")
while [[ ${#queue[@]} -gt 0 ]]; do
    file="${queue[0]}"
    queue=("${queue[@]:1}")
    while read -r soname; do
        [[ -e "$GST_LIB/$soname" ]] || continue
        [[ " ${required[*]} " == *" $GST_LIB/$soname "* ]] && continue
        required+=("$GST_LIB/$soname")
        queue+=("$GST_LIB/$soname")
    done < <(needed "$file")
done
unknown=()
while IFS= read -r -d '' file; do
    while read -r soname; do
        [[ -z "$soname" || -e "$GST_LIB/$soname" || "$soname" =~ $HOST_LIBS ]] && continue
        if [[ "$soname" =~ $OPTIONAL_HOST_LIBS && " ${required[*]} " != *" $file "* ]]; then
            continue
        fi
        unknown+=("$(basename "$file") needs $soname")
    done < <(needed "$file")
done < <(find "$BUNDLE_DIR/bin" "$GST_LIB" -type f -print0)
if [[ ${#unknown[@]} -gt 0 ]]; then
    printf '  unexpected host library: %s\n' "${unknown[@]}" >&2
    [[ "${NW_MOTION_LENIENT_DEPS:-}" == 1 ]] || fail "add the libraries above to HOST_LIBS and the package dependencies"
fi
mkdir -p "$BUNDLE_DIR/gstreamer/licenses"
if [[ -d "$SDK/share/licenses/neurowatch-media" ]]; then
    cp -a "$SDK/share/licenses/neurowatch-media/." "$BUNDLE_DIR/gstreamer/licenses/"
    cp "$SDK/VERSIONS.txt" "$BUNDLE_DIR/gstreamer/VERSIONS.txt"
else
    [[ "${NW_MOTION_LENIENT_DEPS:-}" == 1 ]] || fail "SDK has no media licences (share/licenses/neurowatch-media)"
fi
"$BUNDLE_DIR/bin/nw-motion" --check

# The oldest system the bundle runs on: the newest glibc symbol it uses, and
# the GLib it was built against.
MIN_GLIBC="$(find "$BUNDLE_DIR/bin" "$GST_LIB" -type f -exec objdump -T {} + 2>/dev/null \
    | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n 1)"
MIN_GLIB="$(pkg-config --modversion glib-2.0 | cut -d. -f1,2)"
echo "  needs glibc >= $MIN_GLIBC, GLib >= $MIN_GLIB"
DEB_DEPENDS+=("libc6 (>= $MIN_GLIBC)" "libglib2.0-0 (>= $MIN_GLIB)")
RPM_REQUIRES+=("glibc >= $MIN_GLIBC" "glib2 >= $MIN_GLIB")
ARCH_DEPENDS+=("glibc>=$MIN_GLIBC" "glib2>=$MIN_GLIB")

echo "[4/5] Staging"
STAGE="$WORK_DIR/stage"
LICENSE_DIR="/usr/share/licenses/$PKG_NAME"
mkdir -p "$STAGE$INSTALL_PREFIX" "$STAGE/usr/bin" "$STAGE$LICENSE_DIR"
cp -a "$BUNDLE_DIR/." "$STAGE$INSTALL_PREFIX/"
rm -f "$STAGE$INSTALL_PREFIX/LICENSE" "$STAGE$INSTALL_PREFIX/NOTICE"
ln -s "$INSTALL_PREFIX/bin/nw-motion" "$STAGE/usr/bin/nw-motion"
install -m 0644 "$ROOT/LICENSE" "$ROOT/NOTICE" "$STAGE$LICENSE_DIR/"
mkdir -p "$STAGE$USER_UNITS/default.target.wants"
install -m 0644 "$ROOT/scripts/packaging/nw-motion.service" "$STAGE$USER_UNITS/"
ln -s ../nw-motion.service "$STAGE$USER_UNITS/default.target.wants/nw-motion.service"
INSTALLED_SIZE_KB="$(du -sk "$STAGE" | cut -f1)"
LONG_DESCRIPTION="The motion detection analytics plugin for NeuroWatch. Run it beside
NeuroWatch or on another computer; NeuroWatch sends it the cameras' video and
shows the motion it reports. Includes its own GStreamer and FFmpeg decoders."
LONG_DESCRIPTION_DEB="$(printf '%s\n' "$LONG_DESCRIPTION" | sed '2,$s/^/ /')"

join_by() {
    local separator="$1" first="$2"
    shift 2
    printf '%s' "$first" "${@/#/$separator}"
}

build_portable() {
    local output
    output="$(artifact_path portable)"
    tar -C "$WORK_DIR/bundle" --owner=0 --group=0 -czf "$output" "$PKG_NAME"
    echo "  portable: $output"
}

build_deb() {
    local output root
    output="$(artifact_path deb)"
    root="$WORK_DIR/deb-root"
    rm -rf "$root"
    cp -a "$STAGE" "$root"
    mkdir -p "$root/usr/share/doc/$PKG_NAME" "$root/DEBIAN"
    mv "$root$LICENSE_DIR/LICENSE" "$root$LICENSE_DIR/NOTICE" "$root/usr/share/doc/$PKG_NAME/"
    rmdir "$root$LICENSE_DIR" "$root/usr/share/licenses"
    cat > "$root/DEBIAN/control" <<CONTROL
Package: $PKG_NAME
Version: $DEB_VERSION
Architecture: $DEB_ARCH
Maintainer: $PKG_MAINTAINER
Installed-Size: $INSTALLED_SIZE_KB
Depends: $(join_by ', ' "${DEB_DEPENDS[@]}")
Recommends: $(join_by ', ' "${DEB_RECOMMENDS[@]}")
Section: video
Priority: optional
Homepage: $PKG_URL
Description: $PKG_SUMMARY
 $LONG_DESCRIPTION_DEB
CONTROL
    {
        printf '#!/bin/sh\nset -e\n'
        cat "$SERVICE_HOOK"
        printf '\n[ "$1" != configure ] || nw_motion_sessions install\n'
    } > "$root/DEBIAN/postinst"
    {
        printf '#!/bin/sh\nset -e\n'
        cat "$SERVICE_HOOK"
        printf '\n[ "$1" != remove ] || nw_motion_sessions stop\n'
    } > "$root/DEBIAN/prerm"
    {
        printf '#!/bin/sh\nset -e\n'
        cat "$SERVICE_HOOK"
        printf '\ncase "$1" in remove|purge) nw_motion_sessions reload ;; esac\n'
    } > "$root/DEBIAN/postrm"
    chmod 0755 "$root/DEBIAN/postinst" "$root/DEBIAN/prerm" "$root/DEBIAN/postrm"
    dpkg-deb --root-owner-group --build "$root" "$output" >/dev/null
    echo "  deb: $output"
}

build_rpm() {
    local output topdir spec
    output="$(artifact_path rpm)"
    topdir="$WORK_DIR/rpmbuild"
    spec="$topdir/SPECS/$PKG_NAME.spec"
    rm -rf "$topdir"
    mkdir -p "$topdir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
    cat > "$spec" <<SPEC
Name:           $PKG_NAME
Version:        $RPM_VERSION
Release:        $PKG_RELEASE
Summary:        $PKG_SUMMARY
License:        $PKG_LICENSE
URL:            $PKG_URL
BuildArch:      $RPM_ARCH
AutoReqProv:    no
Requires:       $(join_by ' ' "${RPM_REQUIRES[@]}")
Recommends:     $(join_by ' ' "${RPM_RECOMMENDS[@]}")

# Prebuilt: do not strip or rewrite it.
%global __os_install_post %{nil}
%global debug_package %{nil}
%global _build_id_links none

%description
$LONG_DESCRIPTION

%install
mkdir -p %{buildroot}
cp -a "$STAGE/." %{buildroot}/

%post
$(sed 's/%/%%/g' "$SERVICE_HOOK")
nw_motion_sessions install

%preun
$(sed 's/%/%%/g' "$SERVICE_HOOK")
if [ "\$1" -eq 0 ]; then nw_motion_sessions stop; fi

%postun
$(sed 's/%/%%/g' "$SERVICE_HOOK")
nw_motion_sessions reload

%files
%defattr(-,root,root,-)
$INSTALL_PREFIX
/usr/bin/nw-motion
$USER_UNITS/nw-motion.service
$USER_UNITS/default.target.wants/nw-motion.service
%dir $LICENSE_DIR
%license $LICENSE_DIR/LICENSE
%license $LICENSE_DIR/NOTICE
SPEC
    rpmbuild -bb --define "_topdir $topdir" --target "$RPM_ARCH" "$spec" >/dev/null
    cp "$topdir/RPMS/$RPM_ARCH/$PKG_NAME-$RPM_VERSION-$PKG_RELEASE.$RPM_ARCH.rpm" "$output"
    echo "  rpm: $output"
}

build_arch() {
    local output root dep
    output="$(artifact_path arch)"
    root="$WORK_DIR/arch-root"
    rm -rf "$root"
    cp -a "$STAGE" "$root"
    {
        cat "$SERVICE_HOOK"
        cat <<'HOOKS'

post_install() { nw_motion_sessions install; }
post_upgrade() { nw_motion_sessions install; }
pre_remove() { nw_motion_sessions stop; }
post_remove() { nw_motion_sessions reload; }
HOOKS
    } > "$root/.INSTALL"
    {
        echo "# Generated by scripts/package_linux.sh"
        echo "pkgname = $PKG_NAME"
        echo "pkgbase = $PKG_NAME"
        echo "pkgver = $PAC_VERSION-$PKG_RELEASE"
        echo "pkgdesc = $PKG_SUMMARY"
        echo "url = $PKG_URL"
        echo "builddate = ${SOURCE_DATE_EPOCH:-$(date +%s)}"
        echo "packager = $PKG_MAINTAINER"
        echo "size = $((INSTALLED_SIZE_KB * 1024))"
        echo "arch = $PAC_ARCH"
        echo "license = $PKG_LICENSE"
        for dep in "${ARCH_DEPENDS[@]}"; do echo "depend = $dep"; done
        for dep in "${ARCH_OPTDEPENDS[@]}"; do echo "optdepend = $dep"; done
    } > "$root/.PKGINFO"
    (
        cd "$root"
        LANG=C bsdtar -czf .MTREE --format=mtree --uid 0 --gid 0 --uname root --gname root \
            --options='!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link' \
            .PKGINFO .INSTALL opt usr
        LANG=C bsdtar -cf - --uid 0 --gid 0 --uname root --gname root \
            .PKGINFO .MTREE .INSTALL opt usr | zstd -q -19 -T0 -o "$output" -f
    )
    echo "  arch: $output"
}

echo "[5/5] Packages into $OUT_DIR"
for format in "${FORMATS[@]}"; do
    "build_$format"
done
