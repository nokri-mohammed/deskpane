# deskpane

A Raspberry Pi Zero 2 W drives a small Elegoo 3.5" SPI touchscreen (an
LCD35 clone: ILI9486 display, XPT2046 touch) showing live stats, not
just for itself but for any device on the homelab that reports in. Tap
the screen to cycle between devices.

![architecture](docs/diagram/check.png)

## How it works

- **Reporting devices** run a small Rust binary that reads local system
  stats (CPU, RAM, disk, temperature, uptime) and pushes them to the Pi
  every few seconds over **UDP**. First target: Linux (via the
  [`sysinfo`](https://docs.rs/sysinfo) crate for cross-platform stats,
  installed as a systemd unit). Windows is next: same binary, same
  contract, a Windows Service instead of a unit.
- **The Pi runs a Rust daemon** that accepts those pushes and keeps the
  latest report per device in memory, marking a device offline if it stops
  reporting. **v1 has no authentication**: plain UDP, fine on a trusted
  LAN, not fine beyond it. The daemon's `PROTOCOL` is a single switch point
  for adding TCP and then TCP+mTLS later (the private CA already exists in
  `pi-homelab-ca`, just not wired in yet) without restructuring anything
  else.
- That daemon relays the same data **locally over a Unix domain socket**.
  No TLS is needed for that hop: filesystem permissions are the access
  control. This keeps the daemon and the UI as two independent processes
  without needing a second network port.
- **The Pi's dashboard** is Python + pygame, reading that socket and
  drawing animated gauges straight to the framebuffer (no X11: this is a
  plain SPI panel, not a DRM-capable display). A background thread watches
  the touchscreen's raw evdev events and flips between pages on a tap.

See [`docs/CONTRACT.md`](docs/CONTRACT.md) for the exact JSON shape every
device sends, and the diagram above (also at
[`docs/diagram/source.html`](docs/diagram/source.html), built with
[stack-diagram](https://github.com/caru-ini/stack-diagram)) for the full
picture.

## Why this shape

- **Push, not poll**: the Pi doesn't need to know about a device in
  advance or SSH into it. Any device that starts sending reports shows up,
  and the dashboard's page rotation grows automatically.
- **Rust for anything network-facing or running unattended on someone
  else's machine** (the daemon, every reporter): a single static binary,
  no runtime to install, memory-safe for code parsing input from the
  network. **Python for the UI only**, where pygame's drawing API and fast
  iteration matter more than raw performance.
- **UDP first, deliberately minimal**: getting one working end-to-end path
  (reporter → daemon → socket → display) proven before adding auth. Once
  that's solid, swapping UDP for TCP+mTLS is a change to one function, not
  a redesign. The CA infrastructure (`pi-homelab-ca`) is already built and
  waiting for that step.

## Repo layout

```
pi/           Dashboard (Python/pygame): runs on the Pi, owns the display
daemon/       Report daemon (Rust): runs on the Pi, owns the network, the
              contract module (daemon/src/contract.rs: the Report/
              DeviceStatus types and validate), the device table
              (daemon/src/device_table.rs: cap and expiry) and the
              shared services/ (UDP server + client, broadcaster, Unix
              socket server)
reporter/     Stats reporter (Rust): runs on every reporting device,
              depends on daemon as a library for the contract types and
              the UDP client
docs/         CONTRACT.md (the JSON shape devices report in), HARDWARE.md,
              RECOVERY.md, and diagram/ (architecture diagram source +
              rendered PNGs)
deploy/       what the Pi bundle installs: systemd units, install.sh,
              uninstall.sh, overlay.env (pinned overlay URL + hash),
              env.example
scripts/      package-pi-bundle.sh: builds the Pi bundle; provision-pi.sh:
              builds the bundle and runs install.sh on a Pi over SSH
```

`daemon/` and `reporter/` are one Cargo workspace: one `Cargo.toml` at the
repo root, one `Cargo.lock`. `reporter` depends on `daemon` as a library
for the shared `Report`/`DeviceStatus` types and the `UdpClient` in
`daemon::services::udp`; it doesn't run any of the daemon's own server code.

The private CA lives in a separate sibling repo,
[`pi-homelab-ca`](../pi-homelab-ca), kept out of this repo entirely so it
never risks being pushed anywhere public.

## Status

Working today:
- Pi display pipeline: framebuffer rendering (`pi/fb_display.py`), touch
  detection (`pi/touch.py`), console autostart on boot.
- Private CA + cert issuance scripts, in the sibling `pi-homelab-ca` repo
  (generated, not wired into the daemon yet: see Not built yet).
- Reporting contract defined (`docs/CONTRACT.md`).
- `daemon/`: UDP report listener + in-memory state + Unix socket
  broadcaster, plus the shared `contract` module. It rejects a report that
  fails the checks in `docs/CONTRACT.md`, marks a device offline after
  15 s of silence, keeps at most 32 devices and forgets a device after
  5 minutes of silence. Builds and runs; tested end-to-end with a fake UDP
  report in dev.
- `reporter/`: CLI (`clap`), `deskpane report --daemon <addr> --device-id
  <id> --label <name> [--interval <secs>]` reads real stats via `sysinfo`
  (CPU/RAM/disk/CPU temperature/hostname/uptime) and loops, pushing over UDP. Tested
  end-to-end against the daemon with real stats from this dev machine.
- `pi/dashboard.py`: reads the daemon's Unix socket in a background
  thread (auto-reconnects if the daemon isn't up yet), renders an overview
  page and then one device per page onto `fb_display.FrameBuffer` (ring
  gauges for CPU/RAM, a bar for disk, temperature, label, hostname, ip,
  uptime, online/offline), cycles pages via `touch.TouchWatcher`. A
  drawing error shows an error screen for that page instead of stopping
  the display. Tested end-to-end (socket → daemon → reporter →
  rendered pixel buffer) against a scratch file standing in for
  `/dev/fb1`, not yet confirmed on the real touchscreen.

Not built yet:
- a one-shot report and `deskpane install`/`uninstall` (systemd unit on
  Linux, Windows Service on Windows): `report` (a loop) is the only
  subcommand so far.
- TCP+mTLS transport for the daemon (`PROTOCOL` currently only supports
  `udp`).
- Running any of this on the actual Pi / real touchscreen: everything so
  far has been proven on dev hardware only.

## Versioning and releases

The whole project (daemon, reporter, dashboard) shares one version, following
[Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.

| Bump | When |
|---|---|
| MAJOR | a change that breaks a running setup: the JSON contract, CLI flags, env vars, socket format |
| MINOR | a new feature that works with existing setups |
| PATCH | a bug fix |

While the version is `0.x`, breaking changes bump MINOR instead of MAJOR.

Commits follow [Conventional Commits](https://www.conventionalcommits.org)
(`feat:`, `fix:`, `docs:`, ..., `feat!:` or a `BREAKING CHANGE:` footer for breaking
changes). On every push to `main`, [release-please](https://github.com/googleapis/release-please)
keeps a release PR open with the next version and the `CHANGELOG.md` entry. Merging that PR
tags `vX.Y.Z` and creates a draft GitHub Release.
[`.github/workflows/release.yml`](.github/workflows/release.yml) then builds the downloads,
attaches them with a `SHA256SUMS` file and publishes the release:

| File | For |
|---|---|
| `deskpane-pi-aarch64.tar.gz` | the Pi: daemon, reporter, dashboard, systemd units, `install.sh` (64-bit Raspberry Pi OS Bookworm or later) |
| `deskpane-linux-x86_64.tar.gz`, `deskpane-linux-aarch64.tar.gz` | other Linux machines (static binaries) |
| `deskpane-windows-x86_64.zip` | Windows |
| `SHA256SUMS` | checking the downloads |

The display overlay is not in the bundle (its vendor repo has no license); `install.sh`
downloads it from a pinned commit and checks its SHA-256.

Every push and PR to `main` also runs [`.github/workflows/ci.yml`](.github/workflows/ci.yml):
`cargo fmt --check`, `cargo clippy -D warnings` (the lints that fight the house style are
allowed in `[workspace.lints.clippy]` in `Cargo.toml`), `cargo test --locked`, shellcheck, a
Python compile check, and a trial build of the Pi bundle.

## Setup (once the above is built)

1. Build and run `daemon/` from the workspace root:
   `SOCKET_PATH=/tmp/deskpane.sock cargo run -p daemon` (env vars:
   `LISTEN_ADDR` default `0.0.0.0:8765`, `SOCKET_PATH` default
   `/run/deskpane/daemon.sock`, which the installed systemd units create).
2. Run `pi/dashboard.py` alongside it, reading the same `SOCKET_PATH`.
3. On each reporting device: `cargo run -p reporter -- report --daemon
   <pi-ip>:8765 --device-id <id> --label <name>` (native build for Linux;
   cross-compiled with `mingw-w64` for Windows via
   `--target x86_64-pc-windows-gnu`). `install` for running it as a
   persistent service is a planned increment.
4. Once TCP+mTLS lands: in `pi-homelab-ca`, `make_ca.sh` once, then
   `issue_cert.sh pi server` and `issue_cert.sh <device-id> client` per
   device.

## License

MIT, see [`LICENSE`](LICENSE).
