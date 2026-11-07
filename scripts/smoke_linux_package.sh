#!/usr/bin/env bash
# In a clean distribution container: install the nw-motion package, check it
# can run the analysis and serve, then remove it.
#
#   docker run --rm -v "$PWD:/work:ro" ubuntu:22.04 bash /work/scripts/smoke_linux_package.sh deb
set -euo pipefail
FORMAT="${1:?usage: smoke_linux_package.sh deb|rpm|arch}"
DIST="${DIST:-/work/dist/release}"

package() {
    # shellcheck disable=SC2206 # the pattern is meant to expand
    local matches=("$DIST"/$1)
    [[ ${#matches[@]} -eq 1 && -f "${matches[0]}" ]] \
        || { echo "expected one $1 in $DIST, found: ${matches[*]}" >&2; exit 1; }
    printf '%s\n' "${matches[0]}"
}

case "$FORMAT" in
    deb)
        apt-get update
        DEBIAN_FRONTEND=noninteractive apt-get install -y "$(package 'nw-motion_*.deb')"
        ;;
    rpm) dnf install -y "$(package 'nw-motion-*.rpm')" ;;
    arch) pacman -Syu --noconfirm; pacman -U --noconfirm "$(package 'nw-motion-*.pkg.tar.zst')" ;;
    *) echo "unknown format: $FORMAT" >&2; exit 1 ;;
esac

nw-motion --version
# Every element the analysis uses, from the bundled GStreamer only.
nw-motion --check

work="$(mktemp -d)"
nw-motion access-codes add "$work/codes.json" smoke --max-connections 1 >/dev/null
nw-motion access-codes list "$work/codes.json" | grep -q '^smoke'
grep -q '"hash"' "$work/codes.json"

nw-motion --listen tcp:127.0.0.1:7699 --identity "$work/identity.key" \
    --access-codes "$work/codes.json" > "$work/daemon.log" 2>&1 &
daemon=$!
for _ in $(seq 1 50); do
    grep -q 'Listening for NeuroWatch' "$work/daemon.log" && break
    kill -0 "$daemon" 2>/dev/null || break
    sleep 0.2
done
if ! grep -q 'Listening for NeuroWatch' "$work/daemon.log"; then
    cat "$work/daemon.log" >&2
    exit 1
fi
kill "$daemon"
wait "$daemon" 2>/dev/null || true

case "$FORMAT" in
    deb) apt-get remove -y nw-motion ;;
    rpm) dnf remove -y nw-motion ;;
    arch) pacman -R --noconfirm nw-motion ;;
esac
if [[ -e /opt/nw-motion || -e /usr/bin/nw-motion ]]; then
    echo 'files left behind after removal' >&2
    exit 1
fi
echo "nw-motion $FORMAT package: installed, checked, served and removed"
