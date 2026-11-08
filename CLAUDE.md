# CLAUDE.md

Context for working in deskpane. The README is the short pitch, CONTRIBUTING.md has the
architecture and dev details, and this file covers how to work here.

## What it is

A Raspberry Pi Zero 2 W with an Elegoo 3.5" SPI touchscreen, an LCD35 (ILI9486 + XPT2046)
clone, shows live stats for every device on the homelab that reports in. Data flow:

```
reporter (each device) --UDP JSON--> daemon (Pi) --Unix socket, one JSON line/sec--> pi/dashboard.py
```

- `reporter/` (Rust, blocking): reads stats with `sysinfo`, sends one `Report` per packet every
  `--interval` seconds (or once with `--once`). The default device id is `dp-` plus 8 hex chars
  of SHA-256 over the machine id; the default label is the hostname.
- `daemon/` (Rust, tokio): validates each report, keeps the latest report per device in memory
  (at most 32 devices), owned by the source IP while online (a second IP gets a rate-limited
  `id_in_use` reply), marks a device offline after 15 s of silence, forgets it after 5 minutes
  of silence, broadcasts a `Vec<DeviceStatus>` every second to Unix socket clients. Exits 11
  when UDP 8765 is taken.
- `pi/` (Python, pygame): draws to the framebuffer (`/dev/fb1`, no X11), tap to change page.
- `docs/CONTRACT.md` is the JSON wire shape and its limits. `daemon/src/contract.rs` holds the
  Rust types, `validate` and the limit constants.

## Layout

```
daemon/src/
  lib.rs              pub mod agent_table; pub mod contract; pub mod device_table; pub mod services;
  contract.rs         Report, ReportPacket (+ ack), Reply, build_reply, validate,
                      check_device_id, limit constants (MAX_NAME_BYTES, MAX_REPLY_BYTES, ...);
                      agent packets: PacketKind, parse_packet (kind probe), AgentPacket,
                      AgentState, validate_agent, AGENT_PACKET_VERSION, MAX_PROJECT_BYTES,
                      MAX_DOING_BYTES, SID_LEN; socket line: Envelope, AgentSession
  device_table.rs     DeviceTable of DeviceEntry, pure insert_report (cap, IP ownership ->
                      InsertOutcome) and forget_silent (expiry)
  agent_table.rs      AgentTable (sessions + recently ended sids) of AgentEntry, pure
                      insert_agent (caps, IP ownership, per-key reorder guard, pending
                      needs_you keys per subagent, eviction -> AgentOutcome), forget_agents,
                      agent_session (running_s, state_s, silent_s, quiet)
  main.rs             project logic: bind_error_message, store_report, store_agent,
                      udp_server (kind dispatch, replies for reports only),
                      handle_agent_packet + agent_outcome_line (refusals through the log
                      limiter, new sessions always), forget_silent_devices,
                      forget_silent_agents, broadcast_statuses (envelope + label join),
                      unix_socket_relay, main; OFFLINE_AFTER, FORGET_AFTER, MAX_DEVICES,
                      REPLY_EVERY, EXIT_PORT_IN_USE, MAX_AGENTS, AGENT_* windows
  services/
    broadcaster.rs    tokio broadcast channel that sends JSON
    reply_limiter.rs  ReplyLimiter: per-source and per-second reply limits, capped memory
    udp.rs            UdpSocketServer (async, daemon: try_new, receive, send_to) +
                      UdpClient (blocking, reporter: send, try_receive -> Received) +
                      send_datagram (one packet, never panics or prints: agent hooks)
    unix_socket.rs    UnixSocketServer + Subscriber, #[cfg(unix)] only
reporter/src/
  main.rs             clap CLI, resolve_identity, read_reply, review_replies, build_report,
                      run (back-off loop), report_once; exit codes EXIT_USAGE 2, and for
                      --once only EXIT_ID_IN_USE 10, EXIT_NO_ANSWER 12; `deskpane agent`:
                      hook_main / run_hook (500 ms deadline, exit 0, no stdout), is_hook_call,
                      drain_hook_stdin, hook_args, install_hooks, uninstall_hooks,
                      exe_location_warning, run_agent_test
  agent.rs            pure hook path: HookInput, parse_hook_input, map_event, tool_label,
                      session_sid, sub_key, resolve_project (home folder sends ""),
                      build_agent_packet
  services/
    capped_reader.rs  read_capped: read a stream to the end, keep at most N bytes
    claude_settings.rs pure merge_agent_hooks / remove_agent_hooks on a Claude Code settings
                      document, command_handler, is_agent_handler, backup_suffix
    settings_file.rs  read_json_file (+ FileStamp fingerprint: mtime, length, SHA-256),
                      backup_file (never overwrites), write_atomically (temp file + rename,
                      refuses if the file changed); files created 0600, symlinks followed
    watchdog.rs       start_watchdog: run one action after a delay on a background thread
    conflict.rs       ConflictTracker: warn once, back off 300 s when a conflict outlasts the
                      grace window (run mode never exits on a conflict)
    machine_id.rs     read_machine_id per OS + pure parse_* and derive_device_id (sha2)
    system_stats.rs   SystemStats (sysinfo wrapper) + pure pick_disk_gb / pick_cpu_temp
reporter/tests/
  agent_hook.rs       runs the real binary as a hook: stdout empty, exit 0, deadline, caps
  agent_setup.rs      runs hooks-snippet, install-hooks, uninstall-hooks, test (temp files only)
pi/
  dashboard.py        reads the socket, draws an overview page, then one device per page
  layout.py           pure helpers, no pygame: read_version, overview_layout (with the agents
                      line), version_position; agents page: sort_agents, agent_style, page_ring,
                      agent_rows_layout, fmt_elapsed, agent_summary, fit_text,
                      should_focus_agents, parse_socket_line (envelope or legacy array)
  charpack.py         pure character pack helpers: pack format v1, frame_rect, looks,
                      assign_looks, character_pose, anim_frame, best_scale, track_sessions,
                      plan_pack_install, check_pack_file
  charpack_install.py stdlib pack installer, run as python3 -I, --check mode
  sprites.py          thin pygame loader and scaled-frame cache
  fb_display.py       framebuffer drawing
  touch.py            evdev tap watcher
  tests/              stdlib unittest: test_layout, test_charpack, test_charpack_install,
                      test_deploy_characters (fake-root shell tests), fixtures.py (PNG and
                      zip fixtures built in code)
deploy/
  systemd/*.service   the four units the Pi bundle installs (static, user deskpane)
  install.sh          installs or upgrades the Pi bundle (runs as root on the Pi);
                      --characters ZIP / --remove-characters manage /var/lib/deskpane/characters
  uninstall.sh        removes it (--purge also removes settings, user, boot lines, /var/lib/deskpane)
  overlay.env         pinned display overlay URL + SHA-256 (the overlay is never shipped)
  env.example         becomes /etc/deskpane/env on first install
  README.txt          bundle readme
scripts/
  package-pi-bundle.sh  builds dist/deskpane-pi-aarch64.tar.gz
  provision-pi.sh       builds the bundle and runs its install.sh on a Pi over SSH
  check-no-pack-files.sh  licence guard: CI fails if git tracks a character pack file
docs/
  CONTRACT.md         JSON wire shape and report limits
  INSTALL.md          user install guide (Pi, Linux and Windows reporters)
  HARDWARE.md         parts, pins, display driver notes
  RECOVERY.md         recovery notes for the Pi
  diagram/            architecture diagram source + rendered PNGs
  media/              photos and GIFs of the real screen (README image: media/screen.jpg)
  plans/              plans, one file per topic
  backlog.md          task list, local only (in .gitignore, not in the public repo)
  TEAM.md             team diagram and involvement log
```

`reporter` depends on `daemon` as a library for the contract types and `UdpClient`. The crates
and directories keep the names `daemon` and `reporter`; the binaries they build are
`deskpane-daemon` (Pi) and `deskpane` (client, `deskpane report ...`), set with `[[bin]] name`.

## Commands

Use `~/.cargo/bin/cargo`. `/usr/bin/cargo` is 1.75 and too old for this workspace.
`rust-toolchain.toml` pins Rust 1.99.1 for local builds and CI; bump it together with the
`toolchain:` inputs in `.github/workflows/`.

```bash
~/.cargo/bin/cargo test --workspace                                   # 443 tests: daemon 238, reporter 205
python3 -m unittest discover -s pi/tests                             # 214 tests: layout 87, charpack 68, pack install 27, fake-root deploy 32 (CI runs it too)
~/.cargo/bin/cargo build --release --locked --target aarch64-unknown-linux-musl # Pi build, static (linker in .cargo/config.toml)
scripts/package-pi-bundle.sh                                          # Pi bundle from that build, into dist/
scripts/provision-pi.sh pi-zero                                       # bundle + install.sh on a Pi over SSH
~/.cargo/bin/cargo fmt --all --check                                  # CI runs these two as well
~/.cargo/bin/cargo clippy --workspace --all-targets --locked -- -D warnings
SOCKET_PATH=/tmp/deskpane.sock ~/.cargo/bin/cargo run -p daemon      # LISTEN_ADDR, SOCKET_PATH (default /run/deskpane/daemon.sock), PROTOCOL
~/.cargo/bin/cargo run -p reporter -- report --daemon 127.0.0.1:8765   # automatic id and label; --device-id, --label, --once
```

The Windows target (`x86_64-pc-windows-gnu`) builds in CI (`windows-check` job: clippy) and in the
release workflow. Locally: `rustup target add x86_64-pc-windows-gnu`, then
`~/.cargo/bin/cargo clippy --locked --target x86_64-pc-windows-gnu -p reporter --all-targets -- -D warnings`.

`.github/workflows/release.yml` runs on every push to `main`: release-please keeps a release PR open, and merging it tags `vX.Y.Z`, creates a draft release, builds the
downloads and publishes the release with `SHA256SUMS`. Plan: `docs/plans/2026-11-07-release-artifacts.md`.

## Code style

All Rust here follows the `rust-patterns` skill from the claude-team hub
(`hub/plugins/patterns/skills/rust-patterns/SKILL.md`, installed with
`claude plugin install patterns@claude-team`). Read it before writing Rust. In short:

- No shorthand: every fallible call is `let x = call(); match x { ... }`. No `if let`,
  `while let`, `?`, `.expect`, `unwrap_or_else`, iterator chains or field init shorthand.
  Exception: destructuring a pattern (`Command::Report { daemon, label }`) keeps the short
  form, because rustc warns on the long one.
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
- Character packs (`assets/characters/`, git-ignored) are licensed to the owner. Never commit
  them, put them in a bundle, or show them in docs or `docs/media/`. `scripts/check-no-pack-files.sh`
  (CI) and `package-pi-bundle.sh` enforce this.

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
  and delete them. Pure logic takes plain data so tests feed fake readings (times are passed
  in as `Instant`s). `pi/` tests are stdlib `unittest` in `pi/tests/`, for pure code only (no
  pygame): `python3 -m unittest discover -s pi/tests`. Shell scripts are tested in
  `pi/tests/test_deploy_characters.py` (real scripts under bash, `DESKPANE_DESTDIR` temp root,
  stub commands first on PATH). Pack PNGs and zips are built in code by `pi/tests/fixtures.py`;
  never use the real pack in tests.
- **code-reviewer**: review Rust against `rust-patterns` (no shorthand, comments above, generic
  code in `services/`, project logic in `main.rs`). Check new services have edge-case tests.
- **security-reviewer**: untrusted input enters in one place: UDP packets on `0.0.0.0:8765`,
  parsed in `udp_server` in `daemon/src/main.rs`, checked by `validate` in
  `daemon/src/contract.rs`, then stored through `daemon/src/device_table.rs`. There is no auth in
  v1. The Unix socket is local. Open findings are in `docs/backlog.md` (T-006 MEDIUM items and
  others).
- **security-specialist**: owns firewall, ports, systemd hardening and the Pi provisioning in
  `scripts/provision-pi.sh`, `deploy/install.sh` and `deploy/systemd/`.
- **doc-updater**: keep `README.md` (short pitch, under ~80 lines), `CONTRIBUTING.md` (layout, how it works, status), `docs/CONTRACT.md` (wire shape) and this
  file in sync with the code. Task tracking is in `docs/backlog.md`. Keep `docs/TEAM.md`
  current: diagram on team changes, a log line after each executive task.

Gap: there is no Python style skill yet for `pi/`.

## Open work

The live list is `docs/backlog.md`. Do not copy items here.

Done: the two HIGH security findings (T-002 report validation, commit 6c29acc; T-003 device table
cap and expiry, commit 7fd7b50) and the private-data scrub with history rewritten (T-004).

Not built yet: `deskpane install` (systemd / Windows Service), TCP+mTLS transport, UDP
authentication (T-006, scheduled for v0.2.0).

v0.1.0-beta.1 was installed and run on the real Pi Zero 2 W and touchscreen on 2026-11-07.
