# Plan: open-source release v0.1.0

Status: draft, waiting for CEO approval. Owner: chief of staff.

Goal: someone who has never seen this repo can buy the hardware, download a release, and have
the dashboard running on a Pi with a reporter on their PC, by following the docs only.

## Where we are

| Area | State |
|---|---|
| License | MIT, done |
| CI | builds and tests on every push to main |
| Release pipeline | tag `v*.*.*` builds daemon (aarch64) and reporter (aarch64, x86_64, Windows), attaches `.tar.gz` to a GitHub Release |
| Versioning | none: crates at 0.1.0, no CHANGELOG, commits are not Conventional Commits |
| GitHub repo | none yet, no remote |
| Install for others | none: `scripts/provision-pi.sh` assumes the author's machine, systemd units only exist inside that script |
| Docs | HARDWARE.md and RECOVERY.md are deep notes, not a guide; display name fixed 2026-11-07 (Elegoo 3.5" SPI touchscreen, an LCD35 clone, in README.md and CLAUDE.md); README status updated in T-012 |
| Private data | done: real hostname, LAN IP and username scrubbed 2026-11-07 (placeholders in `docs/CONTRACT.md` and `docs/RECOVERY.md`), git history rewritten (T-004) |
| Third-party binary | done: `pi/assets/tft35a.dtbo` removed (goodtft/LCD-show has no license); `scripts/provision-pi.sh` downloads it from the vendor at install time, pinned to one commit and checked by SHA-256 |
| Security | done: the 2 HIGH findings are fixed (T-002 report validation, commit 6c29acc; T-003 device table cap and expiry, commit 7fd7b50). MEDIUM items stay open (T-006) |

## Phase 1: clean up before anything is public

1. Scrub private data (T-004): placeholders in CONTRACT.md and RECOVERY.md, then rewrite
   history with `git filter-branch` so the values never reach GitHub. Free now, impossible after
   the first push. Owner: security-reviewer checks, chief of staff runs.
2. Check the license of `tft35a.dtbo`. If redistribution is not allowed, remove it and have the
   installer download it from the vendor. Owner: chief of staff.
3. Fix T-002 and T-003 (validation, capped and expiring device table). Owner: tdd-guide writes
   tests first, code-reviewer and security-reviewer review.
4. Fix README facts: one display name (Elegoo LCD35 clone), current status, real setup steps.
   Display name: done 2026-11-07 (CEO confirmed Elegoo; README.md and CLAUDE.md now match
   HARDWARE.md). Current status: done in T-012. Real setup steps: still open (phase 4).
   Owner: doc-updater.

## Phase 2: versioning and changelog

1. Conventional Commits from now on (the essentials hook already warns).
2. One version for the workspace: `version.workspace = true` in both crates.
3. release-please GitHub Action: on every push to main it keeps a release PR with the next
   version and `CHANGELOG.md`. Merging the PR tags `vX.Y.Z` and creates the GitHub Release.
4. The build jobs run in the same workflow, after release-please reports a release was
   created. (A tag pushed by the workflow's own token does not trigger a second workflow, so a
   separate tag-triggered workflow would never run.)

Owner: architect reviews the design, chief of staff implements.

After v0.1.0 ships, remove `"release-as": "0.1.0"` from `release-please-config.json`, or
every release PR will keep proposing 0.1.0. GitHub repo setting needed: Actions may create pull
requests (Settings, Actions, General).

## Phase 3: release artifacts people actually need

| Artifact | For |
|---|---|
| `pi-homelab-dashboard-pi-aarch64.tar.gz` | the Pi: daemon binary, `pi/` dashboard, systemd units, overlay, `install.sh` |
| `reporter-linux-x86_64.tar.gz`, `reporter-linux-aarch64.tar.gz` | Linux machines and other Pis |
| `reporter-windows-x86_64.zip` | Windows (zip, not tar.gz) |
| `SHA256SUMS` | checking downloads |

Also: move the systemd units out of `provision-pi.sh` into `deploy/systemd/*.service`, used by
both `install.sh` and provisioning. CI adds `cargo fmt --check` and clippy, with the lints
that fight the house style switched off (`redundant_field_names`, `single_match`,
`manual_unwrap_or`). Owner: architect designs, security-specialist reviews `install.sh` (it
runs with sudo), chief of staff implements.

## Phase 4: docs for a newcomer

- README: what it is, a photo of the screen, quick start in three steps, links.
- `docs/INSTALL.md`: flash Pi OS Lite 64-bit, enable SPI, install the bundle, check it runs;
  install the reporter on Linux and Windows; troubleshooting.
- `docs/HARDWARE.md`: a bill of materials table at the top (Pi Zero 2 W, Elegoo 3.5" LCD35
  clone, microSD, power supply, optional case) with links; the existing deep notes stay below.
- `CONTRIBUTING.md`: build and test commands, Conventional Commits, `rust-patterns` style.
- `SECURITY.md`: LAN-only, no auth in v1, how to report a problem.
- `CHANGELOG.md` (generated), GitHub issue and PR templates.

Owner: doc-updater, chief of staff reviews against a "fresh user" read-through.

## Phase 5: publish (CEO approval)

Create the public GitHub repo, push, set description and topics, merge the first release PR
to ship v0.1.0, then download the artifacts and follow INSTALL.md on a clean Pi as the final
check.

## Decisions for the CEO

Answered 2026-11-07: keep the commit author email as is. GitHub repo setup (T-017) happens when we are ready to publish.


1. Repo name and owner: `nokri-mohammed/pi-homelab-dashboard`?
2. Rewrite git history to remove the hostname and IP before the first push? (recommended: yes)
3. Fix the two HIGH security items before v0.1.0? (recommended: yes)
4. Keep the team files public (`CLAUDE.md`, `.claude/settings.json`, `docs/TEAM.md`,
   `docs/backlog.md`)? (recommended: yes, no secrets in them, and they show how it is built)
