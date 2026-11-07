# Plan: release artifacts (T-007 phase 3)

Status: approved by the CEO on 2026-11-07 (see Decisions). Owner: architect designs and
implements (T-010 plus phase 3), security-specialist reviews `install.sh` and the systemd units.

Parent plan: `docs/plans/2026-11-07-open-source-release.md`.

Goal: a release page with five downloads that a newcomer can use without cloning the repo:
one bundle for the Pi, three reporter downloads, and a checksum file.

## Current state

- `release.yml` builds four bare binaries and uploads each as its own `.tar.gz` to an already
  published GitHub Release. No Python dashboard, no units, no installer, no checksums.
- The three systemd units (plus `unbind-fbcon.service`) exist only as heredocs inside
  `scripts/provision-pi.sh`, templated with the SSH user's name and home directory.
- `provision-pi.sh` downloads the display overlay from a pinned goodtft/LCD-show commit and
  checks its SHA-256, because the vendor repo has no license and we may not redistribute it.
- The daemon and dashboard share a socket in `/tmp`. Nothing runs as a dedicated user.
- `cargo fmt --check` reports 8 diffs today (line wrapping only). `cargo clippy` reports 3
  warnings, all caused by the house style: `redundant_field_names`, `needless_late_init`,
  `needless_bool_assign`.

## Artifacts

| File | Contents | Built on |
|---|---|---|
| `pi-homelab-dashboard-pi-aarch64.tar.gz` | the Pi bundle below | `ubuntu-24.04-arm` |
| `reporter-linux-x86_64.tar.gz` | `reporter`, `LICENSE`, `README.txt` | `ubuntu-latest` |
| `reporter-linux-aarch64.tar.gz` | `reporter`, `LICENSE`, `README.txt` | `ubuntu-24.04-arm` |
| `reporter-windows-x86_64.zip` | `reporter.exe`, `LICENSE`, `README.txt` | `ubuntu-latest` (mingw cross) |
| `SHA256SUMS` | `sha256sum` lines for the four files above | publish job |

File names carry no version, so docs can link
`https://github.com/<owner>/pi-homelab-dashboard/releases/latest/download/<file>` and the link
never goes stale. The version lives in the top-level directory inside each archive and in a
`VERSION` file.

Linux binaries are built for the `*-unknown-linux-musl` targets (static, no glibc
dependency). A glibc build on `ubuntu-latest` links against glibc 2.39, which can refuse to
start on Raspberry Pi OS Bookworm (glibc 2.36), Debian 11 or Ubuntu 20.04. The reporter is
meant to run on every machine in the homelab, so "runs anywhere" matters more than the small
size and speed cost of musl. The aarch64 builds run on GitHub's native arm64 runner (free for
public repos), so no cross linker is needed.

## Pi bundle layout

```
pi-homelab-dashboard-0.1.0-pi-aarch64/
  VERSION                         0.1.0
  LICENSE
  README.txt                      three lines: verify, extract, sudo ./install.sh
  install.sh
  uninstall.sh
  overlay.env                     OVERLAY_URL and OVERLAY_SHA256 (no overlay file)
  bin/
    daemon
    reporter                      the Pi reports its own stats, as today
  dashboard/
    dashboard.py
    fb_display.py
    touch.py
  systemd/
    homelab-daemon.service
    homelab-dashboard.service
    homelab-reporter.service
    unbind-fbcon.service
  config/
    env.example                   copied to /etc/pi-homelab-dashboard/env on first install
```

The parent plan lists "overlay" in the bundle. That is a mistake: the overlay cannot be
redistributed, so the bundle carries only its pinned URL and hash, and `install.sh` downloads
it on the Pi.

## Repo layout changes

```
deploy/
  systemd/
    homelab-daemon.service
    homelab-dashboard.service
    homelab-reporter.service
    unbind-fbcon.service
  overlay.env                     single source of the overlay URL and hash
  env.example
  install.sh
  uninstall.sh
scripts/
  package-pi-bundle.sh            builds the bundle tarball from a built daemon + reporter
  provision-pi.sh                 becomes: package, scp the tarball, ssh sudo install.sh
```

One source of truth for each thing. `provision-pi.sh` stops writing units and boot config
itself; it builds the same bundle CI builds and runs the same `install.sh` on the Pi. That
way the developer path and the user path cannot drift, and every provisioning run tests the
installer.

## Static systemd units

The units stop being templates. Instead of the SSH user and home directory they use a system
user and fixed paths:

| Item | Value |
|---|---|
| Install root | `/opt/pi-homelab-dashboard/releases/<version>/`, with `/opt/pi-homelab-dashboard/current` a symlink to the active release |
| Service user | `pi-homelab` (system user, no login shell, groups `video` and `input` for `/dev/fb1` and `/dev/input/event*`) |
| Config | `/etc/pi-homelab-dashboard/env`, read with `EnvironmentFile=`, never overwritten by an upgrade |
| Socket | `/run/pi-homelab-dashboard/daemon.sock`, created through `RuntimeDirectory=pi-homelab-dashboard` on the daemon unit, set in both units with `Environment=SOCKET_PATH=...` |

`homelab-daemon.service`: `User=pi-homelab`, `ExecStart=/opt/pi-homelab-dashboard/current/bin/daemon`,
`EnvironmentFile=-/etc/pi-homelab-dashboard/env`, `RuntimeDirectory=pi-homelab-dashboard`,
`RuntimeDirectoryMode=0750`, `Restart=always`.

`homelab-dashboard.service`: same user, `ExecStart=/usr/bin/python3 /opt/pi-homelab-dashboard/current/dashboard/dashboard.py`,
`After=` and `Wants=homelab-daemon.service unbind-fbcon.service`, `Restart=on-failure` (tap to
exit stays exited, as today).

`homelab-reporter.service`: same user,
`ExecStart=/opt/pi-homelab-dashboard/current/bin/reporter run --daemon 127.0.0.1:8765 --device-id ${DEVICE_ID} --label ${LABEL} --interval 2`.
systemd expands `${VAR}` from the env file as one word, so a label with spaces works.

`unbind-fbcon.service`: unchanged from today, runs as root.

Hardening (T-006, security-specialist) slots into these files later: `NoNewPrivileges=yes`,
`ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`. Note that `PrivateTmp` would break
the current `/tmp` socket, which is one more reason the socket moves to `/run` in this phase.

`env.example`:

```
# device id and label the Pi's own reporter uses on the dashboard
DEVICE_ID=pi-zero
LABEL=Raspberry Pi
# where the daemon listens for reports from other machines
LISTEN_ADDR=0.0.0.0:8765
```

## What install.sh does

Run on the Pi as `sudo ./install.sh` from inside the extracted bundle. It is idempotent:
running it again with a newer bundle is the upgrade path. Written in bash with
`set -euo pipefail`, checked with shellcheck in CI.

1. Preflight. Stop with a clear message if: not root; `uname -m` is not `aarch64` (tell the
   user to flash Raspberry Pi OS Lite 64-bit); `/boot/firmware/config.txt` is missing
   (Bookworm or later is required); the bundle directory is incomplete.
2. Flags: `--no-display` (headless: skip overlay, boot config, `unbind-fbcon` and the
   dashboard unit), `--no-reporter` (skip the Pi's own reporter), `--device-id ID` and
   `--label TEXT` (written to the env file on first install only). No flags means the full
   install.
3. Packages: `apt-get update` and `apt-get install -y python3-pygame python3-evdev curl
   ca-certificates`.
4. User: create `pi-homelab` with `useradd --system --no-create-home --shell
   /usr/sbin/nologin`, then add it to `video` and `input`. Skip if it exists.
5. Files: copy the bundle to `/opt/pi-homelab-dashboard/releases/<version>/`, owned by root,
   binaries 0755, Python 0644. The service user can read and run but not modify them. Switch
   `current` atomically (`ln -sfn` to a temp name, then `mv -T`).
6. Config: create `/etc/pi-homelab-dashboard/env` from `env.example` (0644, root) only if it
   does not exist, applying `--device-id` and `--label`.
7. Overlay (skipped with `--no-display`):
   1. Source `overlay.env`.
   2. If `/boot/firmware/overlays/tft35a.dtbo` already matches `OVERLAY_SHA256`, skip the
      download.
   3. Otherwise download `OVERLAY_URL` to a `mktemp` file with `curl -fsSL --proto '=https'
      --tlsv1.2`, check it with `sha256sum -c`, and abort on a mismatch without touching
      `/boot`.
   4. `install -m 0644` it into `/boot/firmware/overlays/tft35a.dtbo`.
8. Boot config (skipped with `--no-display`): before the first change, copy `config.txt`
   and `cmdline.txt` to `*.pi-homelab.bak`. Append the `dtoverlay=tft35a:rotate=0` block if
   no `dtoverlay=tft35a` line exists. Append ` fbcon=map:1` to the single line of
   `cmdline.txt` if missing, keeping it on one line. Remember whether a reboot is needed.
9. Units: `install -m 0644` the unit files into `/etc/systemd/system/`, `systemctl
   daemon-reload`, `systemctl enable` the units, then `restart` them. A restart, not
   `start`, so an upgrade picks up the new binaries.
10. Clean up: keep the current and previous release under `releases/`, delete older ones.
    The previous release is the rollback: point `current` back at it and restart.
11. Report: print the version, the `systemctl status` summary, the reboot hint if step 8
    changed anything, and the command to run on other machines:
    `reporter run --daemon <this Pi's IP>:8765 --device-id <id> --label <name>`.

`uninstall.sh` stops and disables the units, removes the unit files and
`/opt/pi-homelab-dashboard`. By default it keeps `/etc/pi-homelab-dashboard`, the
`pi-homelab` user, the overlay and the boot config edits. `--purge` removes the config and
the user and restores the `.pi-homelab.bak` files.

The documented install never pipes curl into bash: download the bundle and `SHA256SUMS`, run
`sha256sum -c --ignore-missing SHA256SUMS`, extract, then run `sudo ./install.sh`.

## Workflow changes

### ci.yml (every push to main and every PR)

| Job | Steps |
|---|---|
| `fmt` | `cargo fmt --all --check` |
| `clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| `test` | `cargo build --workspace --all-targets --locked`, `cargo test --workspace --locked` |
| `scripts` | `shellcheck deploy/install.sh deploy/uninstall.sh scripts/*.sh`, `python3 -m py_compile pi/*.py` |
| `package` | build the daemon and reporter for `aarch64-unknown-linux-musl` on `ubuntu-24.04-arm`, run `scripts/package-pi-bundle.sh`, list the tarball contents. Catches packaging breakage on the PR, not on release day |

Before turning on `fmt`, run `cargo fmt --all` once as a separate `style:` commit (it does
not bump the version). The current diffs are only line wrapping and do not fight the house
style.

The clippy allow list goes in `Cargo.toml`, not on the CI command line, so a local `cargo
clippy` and CI agree:

```toml
# lints that fight the rust-patterns house style (explicit match, no shorthand)
[workspace.lints.clippy]
redundant_field_names = "allow"
needless_late_init = "allow"
needless_bool_assign = "allow"
single_match = "allow"
manual_unwrap_or = "allow"
manual_unwrap_or_default = "allow"
question_mark = "allow"
manual_map = "allow"
match_like_matches_macro = "allow"
```

Both crates get `[lints] workspace = true`. The first three fire today (measured with clippy
0.1.99). The others are in the parent plan or fire on patterns the house style requires
(`match` that returns early instead of `?`, `match` on an `Option` instead of `.map`). Every
other default lint stays on and is an error in CI.

### release.yml (push to main)

```
release-please --(release_created)--> build-pi-bundle    --\
                                      build-reporter (x3) ----> publish
```

- Top level: `permissions: contents: read` and
  `concurrency: { group: release-please, cancel-in-progress: false }`, so two quick pushes to
  main cannot race on the release PR.
- `release-please`: job-level `contents: write` and `pull-requests: write`. Outputs
  `release_created`, `tag_name` and `version`.
- `build-pi-bundle` (`ubuntu-24.04-arm`, `contents: read`): checkout at `tag_name`, install
  `musl-tools`, build daemon and reporter with `--release --locked --target
  aarch64-unknown-linux-musl`, run `scripts/package-pi-bundle.sh <version>`,
  `actions/upload-artifact`.
- `build-reporter` matrix (`contents: read`):

  | Target | Runner | Archive |
  |---|---|---|
  | `x86_64-unknown-linux-musl` | `ubuntu-latest` + `musl-tools` | `reporter-linux-x86_64.tar.gz` |
  | `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` + `musl-tools` | `reporter-linux-aarch64.tar.gz` |
  | `x86_64-pc-windows-gnu` | `ubuntu-latest` + `gcc-mingw-w64-x86-64` | `reporter-windows-x86_64.zip` (made with `zip`) |

  Each archive has one top-level directory `reporter-<version>-<os>-<arch>/`. Uploaded with
  `actions/upload-artifact`, not to the release.
- `publish` (`needs` both build jobs, `contents: write`): `actions/download-artifact` with
  `merge-multiple: true`, then `sha256sum *.tar.gz *.zip > SHA256SUMS`, then one
  `gh release upload <tag> * --clobber`, then `gh release edit <tag> --draft=false`.

The release is created as a draft (see review finding 3), so nobody sees a release with
missing files, and a failed build leaves an unpublished draft instead of a half-filled public
release. Optional later: `actions/attest-build-provenance` in `publish` (needs `id-token:
write` and `attestations: write`) so users can check where a binary was built.

All third-party actions with write access (`release-please-action`, `rust-toolchain`,
`rust-cache`) are pinned to a full commit SHA with the tag in a comment.

## Small code changes that go with this

- `reporter`: `#[command(version)]` on the clap struct, so `reporter --version` prints the
  workspace version.
- `daemon`: print `daemon <version> listening on ...` at startup using
  `env!("CARGO_PKG_VERSION")`, so `journalctl` shows what is running.
- `pi/dashboard.py`: read `SOCKET_PATH` from the environment (already does). No change.

## Trade-offs

| Decision | Chosen | Alternative | Why |
|---|---|---|---|
| Linux libc | musl static | glibc on `ubuntu-22.04` with a max-GLIBC check | runs on any distro, no runner pinning; cost is a slightly slower allocator, irrelevant here |
| aarch64 builds | native arm64 runner | cross linker on x86 | no linker config, same steps as x86; needs a public repo (it will be one) |
| Windows | `x86_64-pc-windows-gnu` cross | `x86_64-pc-windows-msvc` on `windows-latest` | keeps every job on Linux and matches today; MSVC is the more common Windows target (open question 3) |
| Install location | `/opt` with versioned releases and a `current` link | copy into a home directory (today) | upgrade and rollback are a symlink switch; no dependency on a login user |
| Service user | dedicated `pi-homelab` | the login user | least privilege, units become static files |
| Packaging logic | `scripts/package-pi-bundle.sh` used by CI and provisioning | inline in the workflow | one definition, testable locally |
| Asset names | no version in the name | version in the name | stable "latest" download links for the docs |

## Order of work

1. `deploy/` files (units, overlay.env, env.example) and the socket move to `/run`; switch
   `provision-pi.sh` to them. security-specialist reviews.
2. `install.sh`, `uninstall.sh`, `scripts/package-pi-bundle.sh`; test with `provision-pi.sh`
   on the real Pi, including a second run (upgrade) and `uninstall.sh`.
3. `cargo fmt --all` commit, workspace lints, new CI jobs.
4. `release.yml` rewrite (musl, artifacts, publish job, draft releases, checksums).
5. Version output in daemon and reporter.

## Decisions (CEO, 2026-11-07)

All seven recommendations approved:

1. Service user: a dedicated `pi-homelab` system user.
2. Install path: `/opt/pi-homelab-dashboard/releases/<version>/` with a `current` symlink.
3. Windows build: keep the mingw cross build (`x86_64-pc-windows-gnu`) for v0.1.0.
4. 32-bit: no `armv7` reporter in v0.1.0; `install.sh` refuses 32-bit Pi OS.
5. Pi OS: Bookworm or later only (`/boot/firmware`).
6. Releases: created as drafts, published by one final `publish` job once every file is
   attached (release-please `draft` plus `force-tag-creation`).
7. Build provenance attestations: later, not in v0.1.0.

## Implementation notes (T-010 plus phase 3, 2026-11-07)

Built as designed, with these changes found while implementing:

- Clippy allow list: two more lints fire on house-style code and are allowed:
  `redundant_pattern_matching` (`match wrote { Ok(_) => true, Err(_) => false }`) and
  `manual_range_contains` (`cpu >= 0.0 && cpu <= 100.0`).
- Cargo.lock extra-files: release-please's TOML parser wraps every value as
  `{start, end, value}`, so the filter must be `$.package[?(@.name.value==='daemon')].version`.
  Checked by running release-please 17.11.2's own `GenericToml` updater on the real files: it
  bumps `Cargo.toml` and both `Cargo.lock` entries and nothing else.
- release-please-action is pinned to v5.0.0 (only breaking change: Node 24; bundles
  release-please 17.6.0, which reads `draft`, `force-tag-creation` and `initial-version`).
  GitHub's own actions are pinned to full SHAs too, not only third-party ones.
- No `musl-tools` on the runners: the crates have no C code, and Rust's self-contained musl
  linking worked locally with plain gcc. `.cargo/config.toml` names `aarch64-linux-gnu-gcc` as
  the aarch64 musl linker, which is the cross gcc on x86 and the native gcc on an arm64 runner.
- `[profile.release] strip = true`: the static aarch64 binaries are about 1 MB each.
- `provision-pi.sh` downloads and checks the overlay on the developer machine before it touches
  the Pi (T-011), then passes it with `install.sh --overlay-file`, which checks the hash again
  and once more right before writing into `/boot/firmware/overlays`. It runs `sudo` over
  `ssh -t`, so the NOPASSWD sudoers line is no longer needed. Default SSH host is `pi-zero`.
- `install.sh` wraps its `config.txt` lines in `# >>> pi-homelab-dashboard` / `# <<<` markers;
  `uninstall.sh --purge` deletes that block (and a bare `dtoverlay=tft35a` line from older
  provisioning) instead of restoring the `.bak` copy, so later user edits survive.
- `--label` accepts letters, digits, spaces and `. _ ( ) -` only, because the value lands in a
  file systemd parses.
- When a reboot is needed (first install), `install.sh` enables but does not start the dashboard
  and `unbind-fbcon`, since `/dev/fb1` only appears after the reboot.
- The version output in daemon and reporter is ticket T-013, not part of this change.

### Security review fixes (2026-11-07)

From security-specialist (install package) and security-reviewer (workflows):

- The service user gets no `video`/`input` groups; only the dashboard unit has them, through
  `SupplementaryGroups=`.
- `/etc/pi-homelab-dashboard/env` is `0600 root:root`, enforced on every run without changing
  its content (systemd reads it as root).
- The `config.txt` block starts with `[all]`, so it never lands under a trailing `[cm4]`-style
  section.
- Daemon and reporter units carry a full sandbox (the reporter with `ProtectHome=read-only`);
  the dashboard a minimal one, without `PrivateDevices` because it needs `/dev/fb1` and
  `/dev/input`. All three must be confirmed on the real Pi. Offline
  `systemd-analyze security`: daemon 1.7, reporter 1.8, dashboard 5.2.
- Re-running the same version keeps the newest other release as the rollback.
- `--device-id` and `--label` must start with a letter or digit.
- On a re-run, known units not selected this time are `disable --now`'d.
- `sync` after every boot file change in `install.sh` and `uninstall.sh`.
- `apt-get` runs only when `dpkg-query` says the packages are missing, with
  `--no-install-recommends`.
- `provision-pi.sh` checks `OVERLAY_URL` against the pinned-commit pattern and rejects an SSH
  host starting with `-`.
- Workflows: `persist-credentials: false` on every checkout, `publish` gated on
  `release_created`, `matrix.target` passed through `env`, and `rust-toolchain.toml` pins Rust
  1.99.1 with the same version as `toolchain:` on every `dtolnay/rust-toolchain` step.

Deferred to T-006: a separate daemon user, a udev rule for the touchscreen only, a sandbox for
`unbind-fbcon`, more precise uninstall with a state file, and signing.

### Beta releases (2026-11-07)

The first release is `0.1.0-beta.1`, published as a GitHub pre-release. Config in
`release-please-config.json` (keys checked in release-please's source, `manifest.js` and
`versioning-strategies/prerelease.js`, and present in the bundle of release-please-action
v5.0.0):

- `"versioning": "prerelease"`, `"prerelease-type": "beta"`, `"prerelease": true` at the top
  level; `"initial-version": "0.1.0-beta.1"` on the package. `draft`, `force-tag-creation` and
  the Cargo.toml / Cargo.lock extra-files stay.
- With `prerelease: true`, every release after a beta bumps only the beta number, whatever the
  commit type: `0.1.0-beta.1` then `0.1.0-beta.2`, and so on.
- `Cargo.toml` stays `0.1.0` until the first release PR. release-please writes
  `0.1.0-beta.1` into `Cargo.toml` and both `Cargo.lock` entries in that PR. Cargo accepts it,
  and `cargo build --locked` passes on the result.

From beta to `0.1.0` final, change one key:

```json
"prerelease": false
```

The next release PR then proposes `0.1.0` (the beta number is dropped for fix, feat and
breaking commits alike), and the GitHub release is not marked pre-release. Leave it `false`
after that. `"prerelease"` must not stay `true` after the final: release-please also marks
every `0.x` release as pre-release when it is `true`, and its next bump would be
`0.1.1-beta` (no number). `versioning` and `prerelease-type` can stay; with `prerelease: false`
they bump like the default strategy. To start a new beta series later, set `prerelease` back to
`true` and land a commit with a `Release-As: 0.2.0-beta.1` footer.
