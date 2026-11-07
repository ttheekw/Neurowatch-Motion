# NeuroWatch Motion (nw-motion)

[![CI](https://github.com/ttheekw/Neurowatch-Motion/actions/workflows/ci.yml/badge.svg)](https://github.com/ttheekw/Neurowatch-Motion/actions/workflows/ci.yml)
[![Release](https://github.com/ttheekw/Neurowatch-Motion/actions/workflows/release.yml/badge.svg)](https://github.com/ttheekw/Neurowatch-Motion/releases/latest)

The motion detection plugin for [NeuroWatch](https://github.com/ttheekw/Neurowatch). NeuroWatch
sends it the cameras' compressed H.264/H.265 video; it decodes and analyses that video itself and
answers with motion events and drawings (the boxes on the live wall). The recorder never decodes
video for it. ONVIF camera events stay in NeuroWatch.

NeuroWatch's own packages already include this plugin and start it automatically. For normal
local motion detection, install and open NeuroWatch: there is no daemon command or socket to
configure. The standalone installers here also start automatically in **local mode**. Use them
for a separately installed version; remote hosting and [access codes](#access-codes) are optional.

## Download

The [release workflow](.github/workflows/release.yml) builds packages with their own GStreamer
and FFmpeg decoders. The links below become available after the first successful release;
until then, [build from source](#run-from-source).

| System | Download |
|---|---|
| Windows 10/11 (64-bit), automatic startup | [nw-motion-windows-x86_64-setup.exe](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion-windows-x86_64-setup.exe) |
| Windows 10/11 (64-bit), portable/manual | [nw-motion-windows-x86_64.zip](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion-windows-x86_64.zip) |
| Ubuntu 22.04+, Debian 12+ (64-bit PC) | [nw-motion_amd64.deb](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion_amd64.deb) |
| Ubuntu 22.04+, Debian 12+ (ARM 64-bit) | [nw-motion_arm64.deb](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion_arm64.deb) |
| Fedora and other RPM systems (64-bit PC) | [nw-motion.x86_64.rpm](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion.x86_64.rpm) |
| Fedora and other RPM systems (ARM 64-bit) | [nw-motion.aarch64.rpm](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion.aarch64.rpm) |
| Arch Linux | [nw-motion-x86_64.pkg.tar.zst](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion-x86_64.pkg.tar.zst) |
| Any other Linux (64-bit PC), portable/manual | [nw-motion-linux-x86_64.tar.gz](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion-linux-x86_64.tar.gz) |
| Any other Linux (ARM 64-bit), portable/manual | [nw-motion-linux-arm64.tar.gz](https://github.com/ttheekw/Neurowatch-Motion/releases/latest/download/nw-motion-linux-arm64.tar.gz) |

Older versions, release notes and `SHA256SUMS`: [all releases](https://github.com/ttheekw/Neurowatch-Motion/releases).
Linux packages target glibc 2.35 or newer and require the host's GLib and standard runtime
libraries (installed automatically by native packages). The release workflow checks ARM builds
in Ubuntu 22.04, Debian 12/13 and Fedora containers; 64-bit Raspberry Pi OS (bookworm) meets the requirement but has not been
tried on a Pi.

### Install

```sh
sudo apt install ./nw-motion_amd64.deb          # Ubuntu, Debian
sudo dnf install ./nw-motion.x86_64.rpm         # Fedora
sudo pacman -U nw-motion-x86_64.pkg.tar.zst     # Arch Linux
sha256sum -c SHA256SUMS --ignore-missing        # optional: check the download
```

**Linux packages:** install to `/opt/nw-motion`, put `nw-motion` on the PATH, and install a
systemd user service. It starts in existing user sessions at installation and at subsequent
sign-ins, and restarts if it crashes. It runs as your account, using the default private local
socket that NeuroWatch's local discovery scans. No `--listen` command or `systemctl enable`
step is required. A systemd user session is required for automatic startup.

**Windows installer:** run the setup executable. It installs for your account, starts the
daemon without a console window, and adds a sign-in shortcut. The default local named pipe
is available to NeuroWatch's local discovery. Uninstalling removes the shortcut and stops
that installed copy; upgrades preserve its identity.

**Portable downloads:** extract the tarball or ZIP and run `nw-motion/nw-motion` on Linux or
`nw-motion\bin\nw-motion.exe` on Windows. These are the manual option and do not install startup
entries. `nw-motion --check` checks the bundled decoders.

### Local mode (default)

NeuroWatch's included plugin is started and approved by the main app automatically. The
standalone daemon listens locally by default and is detected in **Plugins → Add Plugin → Local**;
no address needs entering. Approve a separately installed plugin and choose its cameras there.
Local discovery does not bypass the app's approval or camera-binding controls. It can coexist
with the bundled daemon, which has its own socket. No network port is opened by the installers.

On Linux, `systemctl --user status nw-motion` and `journalctl --user -u nw-motion` show the
standalone service's status and logs. To opt out of automatic startup, use
`systemctl --user mask --now nw-motion.service`; `systemctl --user unmask nw-motion.service`
followed by `systemctl --user start nw-motion.service` restores it. On Windows, disable
**NeuroWatch Motion** in Startup Apps to opt out.

### Remote mode (optional)

On the remote computer, run a separate daemon with an explicit network listener (open UDP
port 7690, or TCP for `tls:`), and give it its own persistent identity:

```sh
nw-motion --listen quic:0.0.0.0:7690 --identity ~/.local/state/nw-motion/remote.key
```

In NeuroWatch open **Plugins → Add Plugin → URL**, enter `quic://that-computer:7690`, check that
the fingerprint shown matches the one the daemon logs, and approve it. Then bind cameras on the
plugin's page. Keep the identity file: a new key needs approving again. `tls:` works where UDP is
blocked.

## Run from source

Install Rust 1.88 or newer, a C/C++ build toolchain, pkg-config, and GStreamer development
libraries. On Ubuntu 24.04:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly gstreamer1.0-libav
```

Source builds run in the foreground unless you arrange startup yourself; the native installers
provide automatic startup. On Windows, use an MSVC developer shell and GStreamer's MSVC runtime and development
installation; set `GSTREAMER_1_0_ROOT_MSVC_X86_64`, add its `bin` directory to `PATH`, and
set `PKG_CONFIG_PATH` to its `lib/pkgconfig` directory.

```sh
cargo build --release --locked
target/release/nw-motion --check
# Default: private Unix socket, or local Windows named pipe.
target/release/nw-motion
# Remote encrypted transport with a persistent identity:
target/release/nw-motion --listen quic:0.0.0.0:7690 --identity /private/path/motion.key
# Encrypted fallback when UDP is blocked:
target/release/nw-motion --listen tls:0.0.0.0:7690 --identity /private/path/motion.key
# Loopback development:
target/release/nw-motion --listen tcp:127.0.0.1:7690
```

The two required analytics libraries are included under [`vendor/`](vendor/README.md), so
building from source does not require a NeuroWatch checkout. Use GStreamer >=1.24 with
h264parse/h265parse and gst-libav (the release packages bring their own). When a sibling
NeuroWatch checkout is present, a binary started by hand can use that checkout's release GStreamer
(`.cache/gstreamer-sdk-<newest>`, from `scripts/install_gstreamer_dev_sdk.sh`) when it is there,
as NeuroWatch runs its backend: Ubuntu 24.04's GStreamer 1.24.2 logs a
`gst_mini_object_unref` critical for every picture the analysis skips. `NW_GSTREAMER_SDK=0`
keeps the system's, a directory names another SDK; a set `GST_PLUGIN_SYSTEM_PATH_1_0` wins. Each camera's **Video decoder**
setting (Automatic, VA-API, NVIDIA NVDEC, Intel Quick Sync, V4L2 or CPU; Direct3D on Windows)
names the decoder used. Automatic tries VA-API, NVDEC, Direct3D 11, then V4L2, as NeuroWatch
does; a choice that cannot be used, or a hardware decoder with no picture after 10 s, moves on
and ends on the CPU, with the reason in the camera's status. VA-API is refused on GStreamer
1.24.0–1.24.5 (it leaks). `--hardware-decoding` is ignored. `--check` lists the hardware decoders
found. Preserve the identity file across restarts. Unix
socket directories are private (0700), sockets and secret files owner-only (0600); Windows uses
local named pipes and DPAPI-protected keys. QUIC binds its TLS certificate to the same plugin key.
The host must approve that key and permissions before sending media or credentials.

Packaged NeuroWatch starts its own included daemon and preapproves its local key. It manages
only that child. An external daemon is never killed by the recorder. A managed child also exits
when its parent dies. Up to 1024 camera bindings may be attached per session, subject to actual
CPU/decoder capacity; callbacks and media are bounded.

## Access codes

Without a code file, every recorder you approve may use the daemon. To admit only recorders
that know a code, and only so many at a time, write a code file and start the daemon with it:

```sh
target/release/nw-motion access-codes add /private/path/codes.json site-a --max-connections 10
# prints a new random code once; or bring your own:
echo "$CODE" | target/release/nw-motion access-codes add /private/path/codes.json site-b \
    --max-connections 2 --code-stdin
target/release/nw-motion --listen quic:0.0.0.0:7690 --identity /private/path/motion.key \
    --access-codes /private/path/codes.json
```

The file never holds a code: only a random salt and, per code, its PBKDF2-HMAC-SHA256 hash
(600,000 rounds), its name and its limit. It is written owner-only (0600). `access-codes list`
shows names and limits, `limit PATH NAME N` changes a limit and `remove PATH NAME` removes a
code. Giving an existing name a new code with `add` replaces the old one.

A recorder sends its code as the plugin's vendor credential (NeuroWatch: Plugins → Vendor
Credential…). Each connected recorder holds one place under its code from sign-in until it
disconnects; probes from the Add Plugin dialog take none. A missing or unknown code is refused
with `ACCESS_CODE`, one over the limit with `CONNECTION_LIMIT`; NeuroWatch shows the message on
the plugin's page and tries again every 2 s, so it gets in once a place is free.

- **Edits apply while running.** The daemon reads the file again every 2 s. A removed code, or
  a name given a new code, disconnects the recorders using it; a lowered limit disconnects the
  newest connections above it. A file that is broken, missing or unreadable keeps the list in
  force (one warning per version of the file); at start it stops the daemon instead of serving
  everyone.
- **Lost recorders give their place back.** NeuroWatch pings every 5 s; a connection silent for
  15 s (a lost network can leave one open for minutes) is closed and its place freed.
- **Restarts start from zero.** Places are kept in memory only: after a restart every recorder
  reconnects within seconds and is counted again. If more recorders share a code than its
  limit, which of them get in after a restart is first come, first served.

## Updating from 0.1.0

Version 0.2.0 includes mandatory signed session identities. Older 0.1.0 binaries built before
that feature connect and disconnect before any camera is bound. The recorder reports that the
daemon needs updating; adding a new server entry does not upgrade the executable.

Stop the older daemon, rebuild it (`cargo build --release` in this checkout), and start
`target/release/nw-motion` again. Keep the new persistent identity file across later restarts.
For a separately launched daemon, approve its displayed key and permissions in NeuroWatch's
Plugins tab before it receives cameras. The included managed daemon is preapproved locally.

## Cameras and dashboard

Open NeuroWatch's Plugins tab to bind cameras, select Main/Sub, tune sensitivity, minimum
duration, cooldown, low CPU and watched/ignored zones. Live shows all drawing layers; Output
lists actual events. The motion browser dashboard uses the host SDK's camera picker, live view,
zone editor and event list. Plugin pages are authenticated and sandboxed by NeuroWatch.

Two layers: `detections` shows the moving area/score (wall default); `debug` shows changed blocks,
zones and analysis metrics. Drawings expire after 1.5 seconds and clear on detach/loss. Events
close when a session is lost; NeuroWatch reconnects with 2/5/10/30 second backoff.

A verified local session can reuse existing stable desktop RGB/NV12 pictures at <=5 fps through
a read-only capability ring. A suitable larger Main tile can supply pictures while compressed
Sub is cached. All conversion/scaling happens here. The SDK resumes from the cached compressed
GOP after one second without desktop pictures. Low-CPU mode always uses compressed keyframes
and can miss short movement. Source switches reset detector background without ending a healthy
open event unnecessarily.

## Develop

`nw-analytics-protocol` and `nw-analytics-sdk` are included library snapshots with explicit
dependencies; see [`vendor/README.md`](vendor/README.md) for their origin and update procedure.
`src/detector.rs` is canonical plugin logic: the recorder's builtin detector has
been removed. `src/worker.rs` owns decoding/sharing, `activity.rs` the event lifecycle,
`drawing.rs` overlays, `server.rs` authenticated protocol, and `web/index.html` the dashboard.

```sh
cargo fmt --all -- --check
env -u GIO_MODULE_DIR cargo test --workspace --locked
cargo clippy --all-targets --locked -- -D warnings
python3 -m unittest discover -s tools/motion-eval -v
python3 -m unittest discover -s tests -p 'test_*.py' -v
```

Tests include real x264 moving-ball video over the daemon protocol, settings, detector, event
lifecycle and drawing behavior. Loopback motion qualification fixture remains available in
NeuroWatch's `tests/fixtures/motion_rtsp.py` for isolated recorder integration.

NeuroWatch keeps a separate release source snapshot at `crates/nw-motion`. When updating that
snapshot, map this repository's `vendor/` dependency paths to the host workspace's analytics
crates and omit the standalone `[workspace]` and profile declarations. Older versions of
NeuroWatch's `sync_motion_plugin.py` only handle the former sibling-checkout paths and need
that mapping updated before use.

The separate [CPU motion evaluator](tools/motion-eval/README.md) compares the current
detector with pinned SuBSENSE/PAWCS reference code and OpenCV MOG2/KNN on labeled
video. [Initial results](docs/research/cpu-motion-comparison/README.md) include mask
quality, CPU/memory measurements, seed repeats, and regression fixtures. This
evaluation tool does not change the production detector or its build dependencies.

## Releasing

1. Set the new version in `Cargo.toml` and run `cargo build` (updates `Cargo.lock`).
2. Pin `neurowatch.ref` to a public NeuroWatch commit containing the GStreamer build
   (`scripts/build_release_gstreamer.sh`). Analytics libraries come from this repo's `vendor/`.
3. Push, wait for CI, then tag: `git tag v0.2.1 && git push origin v0.2.1`.

Pushing `main` or opening a pull request runs source checks and tests. Downloadable executables,
installers and Linux packages are built and published **only when a version tag is pushed**
(for example, `v0.2.1`). Normal branch pushes do not produce downloadable packages.

The [Release workflow](.github/workflows/release.yml) builds the Windows installer/ZIP and Linux
packages (x86_64 and arm64), installs each one in clean Ubuntu, Debian, Fedora and Arch
containers, runs `--check`, serves with an access code, and only then publishes the GitHub
Release with `SHA256SUMS` and the version-free names the download table links to. A tag with
a suffix (`v0.3.0-rc.1`) is published as a pre-release. The pinned NeuroWatch revision is public;
no cross-repository token is required.

The [Windows workflow](.github/workflows/windows.yml) is called only by the tagged release
workflow. It checks installer startup at the default local pipe, the sign-in shortcut, identity preservation
on upgrade, and shutdown/shortcut removal on uninstall. Linux native packages include the
automatically enabled user service; package smoke tests also check the default local socket.

Locally: `scripts/package_linux.sh --version v0.2.1 --arch x86_64 --format portable,deb`
(builds NeuroWatch's GStreamer once into `build/gstreamer-sdk`; use a sibling checkout at the
commit in `neurowatch.ref`, or set `NEUROWATCH_DIR`), and on Windows, from an MSVC
developer shell with GStreamer's MSVC packages and Inno Setup 6,
`scripts/package_windows.ps1 -Version v0.2.1` (use `-InnoSetupCompiler` for a custom compiler path).
