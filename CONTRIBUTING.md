# Contributing

Thank you for your interest in deskpane. This file tells you how to build the
project, how to test it and which rules a change must follow.

For a security problem, do not open an issue. See [SECURITY.md](SECURITY.md).

## What is in the repository

| Folder | What it is |
|---|---|
| `daemon/` | Rust. Runs on the Pi. Receives the reports over UDP and serves them to the dashboard. Also a library with the shared report type. |
| `reporter/` | Rust. Runs on each computer. Reads the system stats and sends them to the daemon. |
| `pi/` | Python (pygame). Draws the dashboard on the touchscreen. |
| `deploy/` | The Pi installer, the uninstaller and the systemd units. |
| `scripts/` | Scripts that build the Pi bundle and install it on a Pi over SSH. |
| `docs/` | The guides (`INSTALL.md`, `HARDWARE.md`, `RECOVERY.md`), the JSON contract (`CONTRACT.md`), the plans, `diagram/` (architecture diagram source and rendered PNGs) and `media/` (photos of the screen). |

`daemon/` and `reporter/` are one Cargo workspace: one `Cargo.toml` at the repository root and
one `Cargo.lock`. `reporter` uses `daemon` as a library for the shared `Report` and
`DeviceStatus` types (`daemon/src/contract.rs`) and the `UdpClient` in `daemon::services::udp`.
It does not run any of the daemon's own server code. Inside `daemon/`, `src/device_table.rs`
holds the device table (the 32-device cap and the expiry) and `src/services/` holds the
generic plumbing: UDP server and client, broadcaster, Unix socket server.

## How it works

```
reporter (each device) --UDP JSON--> daemon (Pi) --Unix socket, one JSON line/sec--> pi/dashboard.py
```

- **Reporters.** Each computer runs `deskpane report`. It reads CPU, memory, disk, CPU
  temperature, hostname and uptime with the [`sysinfo`](https://docs.rs/sysinfo) crate and
  sends one report over UDP every few seconds (default 4). The same binary builds for Linux
  and Windows. A systemd unit (Linux) and a Windows Service are planned as `deskpane install`.
- **The daemon on the Pi** receives the reports, checks each one (`validate`), keeps the
  latest report per device in memory and marks a device offline after 15 seconds of silence.
  The `PROTOCOL` setting is the single switch point for adding TCP and then TCP with mutual
  TLS later. Only `udp` works today.
- **The local hop.** The daemon sends the device list to the dashboard over a Unix domain
  socket, one JSON line every second. This hop needs no TLS: file permissions control who can
  open the socket. The daemon and the UI stay two separate processes without a second network
  port.
- **The dashboard** is Python and pygame. It reads the socket in a background thread and
  reconnects if the daemon is not up yet. It draws straight to the framebuffer (`/dev/fb1`, no
  X11, because the SPI panel is not a DRM display): an overview page, then one page per device
  with ring gauges for CPU and memory, a bar for disk, temperature, label, hostname, IP
  address, uptime and online or offline. A background thread reads the touchscreen's raw evdev
  events and moves to the next page on a tap. A drawing error shows an error screen for that
  page instead of stopping the display.

The architecture diagram is [docs/diagram/check.png](docs/diagram/check.png). Its source is
[docs/diagram/source.html](docs/diagram/source.html), built with
[stack-diagram](https://github.com/caru-ini/stack-diagram).

### Why this shape

- **Push, not poll.** The Pi does not need to know a device in advance or log in to it. Any
  device that starts to send reports shows up, and the page rotation grows by itself.
- **Rust for the network and for unattended code.** The daemon and every reporter are single
  static binaries with no runtime to install, and Rust is memory-safe for code that parses
  input from the network. **Python for the UI only**, where pygame's drawing API and fast
  changes matter more than raw speed.
- **UDP first, on purpose.** One working path (reporter, daemon, socket, display) came first.
  Authentication comes next: swapping UDP for TCP with mutual TLS changes one function, not
  the design. The maintainer keeps the private certificate authority for that step in a
  separate private repository (`pi-homelab-ca`), never in this one. Once TCP with mutual TLS
  lands, its scripts run `make_ca.sh` once, then `issue_cert.sh pi server` and
  `issue_cert.sh <device-id> client` for each device.

## Toolchain

- Install Rust with [rustup](https://rustup.rs). The file `rust-toolchain.toml` pins the exact
  Rust version that CI and the releases use. rustup reads it and installs that version, with
  `rustfmt` and `clippy`, the first time you run `cargo` in the repository.
- Do not use the Rust package from your Linux distribution. It is often too old for this
  workspace (for example, 1.75 is too old).
- Python 3 for the dashboard.
- `shellcheck` for the shell scripts.

To build the Pi binaries on an x86_64 computer, you also need:

- The Rust target: `rustup target add aarch64-unknown-linux-musl`.
- The cross linker `aarch64-linux-gnu-gcc`. On Debian and Ubuntu it is in the package
  `gcc-aarch64-linux-gnu`. `.cargo/config.toml` names it.

To build the Windows reporter on Linux, cross-compile with `mingw-w64` and
`--target x86_64-pc-windows-gnu`. CI and the release workflow do this for you.

## Build and test

Run all of these before you open a pull request. CI runs the same checks, and a pull request
must pass all of them.

```bash
cargo build --workspace --all-targets --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
shellcheck deploy/install.sh deploy/uninstall.sh scripts/*.sh
python3 -m py_compile pi/*.py
```

To build the Pi bundle:

```bash
cargo build --release --locked --target aarch64-unknown-linux-musl -p daemon -p reporter
scripts/package-pi-bundle.sh
```

The bundle goes to `dist/deskpane-pi-aarch64.tar.gz`.

To run the daemon and a reporter on your own computer:

```bash
SOCKET_PATH=/tmp/deskpane.sock cargo run -p daemon
cargo run -p reporter -- report --daemon 127.0.0.1:8765 --device-id dev --label Dev
```

The daemon reads three environment variables: `LISTEN_ADDR` (default `0.0.0.0:8765`),
`SOCKET_PATH` (default `/run/deskpane/daemon.sock`, which only exists on an installed Pi, so set
it to a path in `/tmp` on your own computer) and `PROTOCOL` (only `udp` works).

## Commit messages and versions

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org). The type at
the start sets the next version, so pick it with care:

| Type | Use it for | Version change |
|---|---|---|
| `fix:` | a bug fix | PATCH |
| `feat:` | a new feature | MINOR |
| `feat!:`, `fix!:` or a `BREAKING CHANGE:` footer | a change that breaks a running setup | MAJOR (MINOR while the version is 0.x) |
| `docs:`, `test:`, `refactor:`, `style:`, `ci:`, `chore:` | everything else | none |

Examples:

```text
fix: keep the label when the reporter restarts
feat: show network traffic on the device page
docs: explain the firewall rule in INSTALL.md
```

The pull request title must also be a Conventional Commit message. The maintainer can squash
your commits into one with that title.

[release-please](https://github.com/googleapis/release-please) reads these messages on every
push to `main`. It keeps a release pull request open with the next version and the
`CHANGELOG.md` entry. When the maintainer merges that pull request, the release is built and
published. Do not change the version in `Cargo.toml` by hand. Do not edit `CHANGELOG.md` by
hand. See [Versions and releases](#versions-and-releases).

## Versions and releases

The whole project (daemon, reporter, dashboard) shares one version and follows
[Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.

| Bump | When |
|---|---|
| MAJOR | a change that breaks a running setup: the JSON contract, CLI flags, environment variables, socket format |
| MINOR | a new feature that works with existing setups |
| PATCH | a bug fix |

While the version is `0.x`, breaking changes bump MINOR instead of MAJOR. Pre-releases look
like `0.1.0-beta.N` and are marked as pre-release on GitHub.

On every push to `main`, release-please keeps the release pull request open. Merging it tags
`vX.Y.Z` and creates a draft GitHub Release.
[`.github/workflows/release.yml`](.github/workflows/release.yml) then builds the downloads,
attaches them with a `SHA256SUMS` file and publishes the release:

| File | For |
|---|---|
| `deskpane-pi-aarch64.tar.gz` | the Pi: daemon, reporter, dashboard, systemd units, `install.sh` (64-bit Raspberry Pi OS Bookworm or later) |
| `deskpane-linux-x86_64.tar.gz`, `deskpane-linux-aarch64.tar.gz` | other Linux computers (static binaries) |
| `deskpane-windows-x86_64.zip` | Windows |
| `SHA256SUMS` | checking the downloads |

The display overlay is not in the bundle, because its vendor repository has no license.
`install.sh` downloads it from a pinned commit and checks its SHA-256.

Every push and pull request to `main` also runs
[`.github/workflows/ci.yml`](.github/workflows/ci.yml): `cargo fmt --check`,
`cargo clippy -D warnings` (the lints that fight the house style are allowed in
`[workspace.lints.clippy]` in `Cargo.toml`), `cargo test --locked`, shellcheck, a Python
compile check and a trial build of the Pi bundle.

## Rust style

The Rust code follows a house style, written for readers who are new to Rust. The full rules
are the project's `rust-patterns` style guide. You cannot install that guide outside the
project's own tools, so this section gives a summary. Read the existing code in `daemon/src/`
for examples.

1. **No shorthand.** Store the result of every call that can fail, then `match` on it. Do not
   use `if let`, `while let`, `?`, `.expect()`, `unwrap_or`, `unwrap_or_else`, `map_or` or
   struct field init shorthand (write `socket: socket`). Use `.unwrap()` in tests only.
   One exception: when unpacking a struct or enum in a `match` arm, keep the short form
   (`Command::Report { daemon, label }`), because rustc warns on the long one.

   ```rust
   // bind the socket; fatal if it fails
   let bound = UdpSocket::bind(addr).await;
   let socket = match bound {
       Ok(s) => s,
       Err(e) => panic!("bind udp {addr}: {e}"),
   };
   ```

2. **Loops, not iterator chains.** Write a `for` loop with `if` instead of
   `.iter().map().filter().collect()`. Use a loop instead of a closure where a loop works.
3. **Comments go above the code.** Write one short comment above each block. Do not put a
   comment at the end of a line. A comment of more than one line uses `/* */`.
4. **Generic code goes in `services/`.** A service is a small struct that wraps one piece of
   plumbing, such as a socket or a channel. Name it after what it wraps (`UnixSocketServer`),
   not after this project. The project logic (loops, timers, what the bytes mean) stays in
   `main.rs`.
5. **Errors in services.** A setup failure (bind, connect, bad address) panics with a message
   that names the input. A failure inside a running loop does not panic. The service logs it
   with a short prefix (`udp:`, `daemon:`) and returns an `Option` or a `bool`.
6. **Names say what one call does.** Use a verb phrase, such as `broadcast_statuses`. Keep a
   loop that runs forever where the reader can see it, not inside a helper.
7. **Small explicit details.** Write `.clone()` where ownership needs it. Release a lock early
   with `drop(guard);` and a comment that says why. Put platform-only modules behind
   `#[cfg(unix)]`, so the Windows build still compiles.

Clippy runs with `-D warnings`. Some lints conflict with this style. `Cargo.toml` turns those
lints off in `[workspace.lints.clippy]`. Do not add `#[allow]` attributes in the code to get
around a lint. Ask in the pull request instead.

Python in `pi/` follows the same idea: write it out in full, and put comments above the code.

## Tests

New code needs tests. A pull request without tests for new behavior is not merged.

- Put the tests in a `#[cfg(test)] mod tests` block at the bottom of the file.
- Use `#[tokio::test]` for async code.
- Use `#[should_panic(expected = "...")]` for setup failures.
- Test the edge cases, not only the normal case: empty input, input that is too big, an
  invalid address, a peer that disconnects.
- Put pure logic in a plain function that takes plain data. Then a test can give it fake
  values without real hardware.
- Tests clean up after themselves. Socket tests use short paths such as
  `/tmp/phd-test-<pid>-<name>.sock` and delete them.

The Python code in `pi/` has no tests yet. CI only checks that it compiles.

## The JSON contract

[docs/CONTRACT.md](docs/CONTRACT.md) describes the report that every reporter sends to the
daemon. `daemon/src/contract.rs` holds the Rust type and the limits. People run reporters and
daemons of different versions at the same time, so this contract must stay compatible.

- A new field in `Report` must be optional (`Option<T>` with `#[serde(default)]`). Then a new
  daemon still accepts reports from an old reporter.
- To remove a field, rename a field, change its type or make a limit stricter is a breaking
  change. Mark the commit with `!` (for example `feat!:`).
- Update `docs/CONTRACT.md` and `validate` in `daemon/src/contract.rs` in the same pull
  request as the change.

## Private data

The repository is public. Do not put these in any file, commit message, issue or log that you
paste:

- passwords, keys, tokens or certificates
- real WiFi network names
- real IP addresses, hostnames or user names from your network

Use these placeholders instead: `192.168.1.50` for the Pi, `pi-zero` for its hostname,
`my-pc` for another computer.

## Writing style for docs and comments

- Do not use em dashes. Use a comma, a colon or a new sentence.
- Write short sentences in plain words. Many readers are not experts and some do not read
  English as a first language.
- Write steps as a numbered list, one action per step.

## Project status

Working today:

- The daemon: UDP listener, report checks, in-memory device table (at most 32 devices, offline
  after 15 seconds, forgotten after 5 minutes of silence) and the Unix socket broadcaster.
- The reporter: `deskpane report --daemon <addr> --device-id <id> --label <name>
  [--interval <secs>]`, tested end to end against the daemon with real stats.
- The dashboard: framebuffer drawing (`pi/fb_display.py`), touch (`pi/touch.py`), start on
  boot. Its end-to-end test (reporter, daemon, socket, rendered pixels) used a scratch file in
  place of `/dev/fb1`.

Not built yet:

- A one-shot report, and `deskpane install` and `uninstall` (a systemd unit on Linux, a
  Windows Service on Windows). `report` (a loop) is the only subcommand.
- TCP with mutual TLS for the daemon. `PROTOCOL` supports only `udp`.

## Pull requests

1. Fork the repository and make a branch.
2. Make your change, with tests.
3. Run the checks in [Build and test](#build-and-test).
4. Open a pull request. Fill in the template.

By contributing, you agree that your contribution is licensed under the MIT License (see
[LICENSE](LICENSE)). Everyone in this project follows the [Code of Conduct](CODE_OF_CONDUCT.md).
