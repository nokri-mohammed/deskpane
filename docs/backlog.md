# Backlog

## To Do

- T-005 Relay skips RecvError::Lagged instead of disconnecting: [todo 2026-11-07]
- T-008 Upper limits for totals and uptime (1e308 still accepted in mem/disk totals, uptime): [todo 2026-11-07]
- T-009 Reporter checks its own report against the contract limits before sending: [todo 2026-11-07]
- T-013 Version visible at runtime: reporter --version, daemon logs its version at startup: [todo 2026-11-07]
- T-014 Contract compatibility rule: new Report fields use #[serde(default)] or the commit is feat!: [todo 2026-11-07]
- T-015 Dashboard prunes its per-device view cache for devices the daemon forgot: [todo 2026-11-07]
- T-016 Rate-limited logging for bad report, rejected report and device table full: [todo 2026-11-07]
- T-017 GitHub repo setup at first push: protect main (PR + CI checks, no force push), allow Actions to open PRs, draft-release dry run before v0.1.0: [todo 2026-11-07]
- T-018 Pi's own reporter target follows LISTEN_ADDR port instead of fixed 127.0.0.1:8765: [todo 2026-11-07]
- T-019 install.sh remembers --no-display/--no-reporter from the last install so upgrades keep them: [todo 2026-11-07]
- T-020 touch.py finds the touchscreen by name instead of always /dev/input/event0: [todo 2026-11-07]
- T-021 install.sh copies uninstall.sh into /opt so uninstall works without the bundle: [todo 2026-11-07]
- T-022 Vision and name: display with views (stats, tasks via MCP/AI agent, canvas from a web portal); pick a name that fits, rename before first publish: [todo 2026-11-07, in-progress 2026-11-07 architect]
- T-006 Security MEDIUM items (HMAC, firewall, sudoers, separate daemon user, touchscreen-only udev rule, unbind-fbcon sandbox, precise uninstall, bundle signing): [todo 2026-11-07]

## In Progress

- T-007 Open-source release v0.1.0 (plans: docs/plans/2026-11-07-open-source-release.md, docs/plans/2026-11-07-release-artifacts.md): [todo 2026-11-07, in-progress 2026-11-07 chief-of-staff (phases 1+2 done), phase 1 approved 2026-11-07 security-reviewer, phase 2 reviewed + phase 3 designed 2026-11-07 architect, CEO approved phase 3 decisions 2026-11-07, phase 3 in-progress architect, phase 4 first pass done 2026-11-07, second pass waits on the project name (T-022), phase 3 done 2026-11-07]

## Blocked

## In Review

## Done

- T-011 Overlay download hardening: verify before first SSH, https-only curl, re-check hash on the Pi: [todo 2026-11-07, in-progress 2026-11-07 architect, in-review 2026-11-07 security-specialist + security-reviewer, fixes architect, fixes done 2026-11-07, done 2026-11-07 CEO approved]
- T-010 Release workflow fixes: Cargo.lock in extra-files + --locked, initial-version instead of release-as, draft release published by one final job, per-job permissions, concurrency, actions pinned to SHAs, publish = false (before first release PR merge): [todo 2026-11-07, in-progress 2026-11-07 architect, in-review 2026-11-07 security-specialist + security-reviewer, fixes architect, fixes done 2026-11-07, done 2026-11-07 CEO approved]
- T-012 Docs pass: stale private-data notes in CLAUDE.md and plan, em dashes, test count, device_table.rs in layout, CONTRACT.md offline sentence: [todo 2026-11-07, in-progress 2026-11-07 doc-updater, in-review 2026-11-07 CEO, done 2026-11-07 CEO approved]
- T-003 Cap device table at 32 and expire after 5 min silence, 1500-byte UDP buffer: [todo 2026-11-07, in-progress 2026-11-07 tdd-guide, in-review 2026-11-07 code-reviewer, approved 2026-11-07 code-reviewer, polish tdd-guide, done 2026-11-07 CEO approved]
- T-002 Validate reports in the daemon before insert, try/except around drawing: [todo 2026-11-07, in-progress 2026-11-07 tdd-guide, in-review 2026-11-07 code-reviewer, in-progress 2026-11-07 tdd-guide (review fixes), in-review 2026-11-07 code-reviewer (round 2), approved 2026-11-07 code-reviewer, done 2026-11-07 CEO approved]
- T-004 Replace real hostname/IP/username with placeholders, scrubbed from history: [todo 2026-11-07, done 2026-11-07 chief-of-staff]
- T-001 Onboard team (architecture, tdd, review, patterns, essentials): [todo 2026-11-07, done 2026-11-07 chief-of-staff]

<!--
Rule: one line per task, living under its current status heading.
On pickup/handoff/status-change: cut the line, move it under the new heading,
append ONE new entry to the trailing history. No paragraphs, no long notes.

- T-001 Short task title: [todo 11-01]
- T-001 Short task title: [todo 11-01, in-progress 11-02 backend-eng]
- T-001 Short task title: [todo 11-01, in-progress 11-02 backend-eng, blocked 11-03 backend-eng (waiting on X)]
-->
