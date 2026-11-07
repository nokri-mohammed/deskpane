# CLAUDE.md

Context for working in pi-homelab-dashboard. The README covers the "why"; this file covers how to
work here.

## What it is

A Raspberry Pi Zero 2 W with a Waveshare 3.5" SPI touchscreen shows live stats for every device
on the homelab that reports in. Data flow:

```
reporter (each device) --UDP JSON--> daemon (Pi) --Unix socket, one JSON line/sec--> pi/dashboard.py
```

- `reporter/` (Rust, blocking): reads stats with `sysinfo`, sends one `Report` per packet every
  `--interval` seconds.
- `daemon/` (Rust, tokio): keeps the latest report per device in memory, marks a device offline
  after 15 s of silence, broadcasts a `Vec<DeviceStatus>` every second to Unix socket clients.
- `pi/` (Python, pygame): draws to the framebuffer (`/dev/fb1`, no X11), tap to change page.
- `docs/CONTRACT.md` is the JSON wire shape. `daemon/src/contract.rs` holds the Rust types.

## Layout

```
daemon/src/
  lib.rs              pub mod contract; pub mod services;
  contract.rs         Report, DeviceStatus
  main.rs             project logic: udp_server, broadcast_statuses, unix_socket_relay, main
  services/
    broadcaster.rs    tokio broadcast channel that sends JSON
    udp.rs            UdpSocketServer (async, daemon) + UdpClient (blocking, reporter)
    unix_socket.rs    UnixSocketServer + Subscriber, #[cfg(unix)] only
reporter/src/
  main.rs             clap CLI, build_report, send_report, run
  services/
    system_stats.rs   SystemStats (sysinfo wrapper) + pure pick_disk_gb / pick_cpu_temp
pi/
  dashboard.py        reads the socket, draws one device per page
  fb_display.py       framebuffer drawing
  touch.py            evdev tap watcher
scripts/
  provision-pi.sh     pushes this repo onto a freshly flashed Pi
docs/
  CONTRACT.md         JSON wire shape
  plans/              plans, one file per topic
  backlog.md          task list
  TEAM.md             team diagram and involvement log
```

`reporter` depends on `daemon` as a library for the contract types and `UdpClient`.

## Commands

Use `~/.cargo/bin/cargo`. `/usr/bin/cargo` is 1.75 and too old for this workspace.

```bash
~/.cargo/bin/cargo test --workspace                                   # 39 tests: daemon 25, reporter 14
~/.cargo/bin/cargo build --release --target aarch64-unknown-linux-gnu # Pi build (linker in .cargo/config.toml)
~/.cargo/bin/cargo run -p daemon                                      # LISTEN_ADDR, SOCKET_PATH, PROTOCOL env vars
~/.cargo/bin/cargo run -p reporter -- run --daemon 127.0.0.1:8765 --device-id dev --label Dev
```

The Windows target (`x86_64-pc-windows-gnu`) is not installed on this machine. CI and the release
workflow build it. Pushing a `v*` tag runs `.github/workflows/release.yml`.

## Code style

All Rust here follows the `rust-patterns` skill from the claude-team hub
(`hub/plugins/patterns/skills/rust-patterns/SKILL.md`, installed with
`claude plugin install patterns@claude-team`). Read it before writing Rust. In short:

- No shorthand: every fallible call is `let x = call(); match x { ... }`. No `if let`,
  `while let`, `?`, `.expect`, `unwrap_or_else`, iterator chains or field init shorthand.
  `.unwrap()` in tests only.
- Comments above the code, short, one per block. Multi-line comments use `/* */`.
- Generic plumbing goes in `services/` as small structs with unit tests. Project logic stays in
  `main.rs`.
- Python in `pi/` follows the same spirit: explicit, commented above.

## Rules for this repo

- The repo is prepared for open source. Never write real WiFi SSIDs, passwords, usernames,
  hostnames or LAN IPs into tracked files. Use placeholders (`192.168.1.50`, `pi-zero`).
- The private CA lives in the sibling repo `../pi-homelab-ca`. Never copy keys or certs here.
- Commit only when the owner asks. Branch first if on `main`.
- Commit messages follow Conventional Commits (`feat:`, `fix:`, `docs:`, `feat!:` for
  breaking). release-please derives the next semantic version and CHANGELOG.md from them, so a
  wrong type means a wrong release. One version covers the whole project (`[workspace.package]`
  in `Cargo.toml`); never edit it by hand.
- Never use em dashes in docs or comments.

## Team

Executives on this project (hub plugins `architecture`, `tdd`, `review`, `patterns`,
`essentials`, plus the personal `security-specialist`). Each one reads its brief before working.

- **architect**: three components talk over two links: reporter to daemon over UDP (one JSON
  `Report` per packet, `docs/CONTRACT.md`), daemon to dashboard over a Unix socket (one JSON
  array of `DeviceStatus` per line, every second). `daemon/src/contract.rs` is the shared type.
  The daemon's `Protocol` enum is the switch point for TCP and TCP+mTLS later. Plans go in
  `docs/plans/YYYY-MM-DD-<topic>.md`.
- **tdd-guide**: run `~/.cargo/bin/cargo test --workspace`. Tests are a `#[cfg(test)] mod tests`
  at the bottom of each service file, `#[tokio::test]` for async code, `#[should_panic(expected
  = "...")]` for setup failures. Socket tests use short `/tmp/phd-test-{pid}-{name}.sock` paths
  and delete them. Pure logic takes plain data so tests feed fake readings. `pi/` has no tests
  yet.
- **code-reviewer**: review Rust against `rust-patterns` (no shorthand, comments above, generic
  code in `services/`, project logic in `main.rs`). Check new services have edge-case tests.
- **security-reviewer**: untrusted input enters in one place: UDP packets on `0.0.0.0:8765`,
  parsed in `udp_server` in `daemon/src/main.rs`. There is no auth in v1. The Unix socket is local.
  The known findings are under Open work.
- **security-specialist**: owns firewall, ports, systemd hardening and the Pi provisioning in
  `scripts/provision-pi.sh`.
- **doc-updater**: keep `README.md` (layout, status), `docs/CONTRACT.md` (wire shape) and this
  file in sync with the code. Task tracking is in `docs/backlog.md`. Keep `docs/TEAM.md`
  current: diagram on team changes, a log line after each executive task.

Gap: there is no Python style skill yet for `pi/`.

## Open work

From the security review (verdict: fine on a trusted LAN, not beyond it):

- HIGH: validate reports in the daemon before insert (string length caps, no control
  characters, finite sane floats), and wrap drawing in `pi/dashboard.py` in try/except. A bad
  report can crash the dashboard today.
- HIGH: the device table has no cap and no expiry. Cap at about 32 devices, expire after 5
  minutes of silence, shrink the UDP buffer to 1500 bytes.
- MEDIUM: HMAC on reports, firewall udp/8765, move the socket to
  `/run/pi-homelab-dashboard/daemon.sock` with 0600, systemd hardening, tighten the sudoers line
  in `scripts/provision-pi.sh`.
- LOW: replace the real hostname/IP in `docs/CONTRACT.md` and the username in
  `docs/RECOVERY.md` with placeholders before publishing.
- `unix_socket_relay` disconnects a client on `RecvError::Lagged`; it could skip and continue
  instead.

Not built yet: `reporter report` (one-shot), `reporter install` (systemd / Windows Service),
TCP+mTLS transport, running on the real Pi and touchscreen.
