# Backlog

## To Do

- T-005 Relay skips RecvError::Lagged instead of disconnecting: [todo 2026-11-07]
- T-008 Upper limits for totals and uptime (1e308 still accepted in mem/disk totals, uptime): [todo 2026-11-07]
- T-009 Reporter checks its own report against the contract limits before sending: [todo 2026-11-07]
- T-010 Release workflow fixes: Cargo.lock in extra-files + --locked, initial-version instead of release-as, draft release published by one final job, per-job permissions, concurrency, actions pinned to SHAs, publish = false (before first release PR merge): [todo 2026-11-07]
- T-011 Overlay download hardening: verify before first SSH, https-only curl, re-check hash on the Pi: [todo 2026-11-07]
- T-013 Version visible at runtime: reporter --version, daemon logs its version at startup: [todo 2026-11-07]
- T-014 Contract compatibility rule: new Report fields use #[serde(default)] or the commit is feat!: [todo 2026-11-07]
- T-015 Dashboard prunes its per-device view cache for devices the daemon forgot: [todo 2026-11-07]
- T-016 Rate-limited logging for bad report, rejected report and device table full: [todo 2026-11-07]
- T-006 Security MEDIUM items (HMAC, firewall, socket in /run, systemd hardening, sudoers): [todo 2026-11-07]

## In Progress


- T-007 Open-source release v0.1.0 (plans: docs/plans/2026-11-07-open-source-release.md, docs/plans/2026-11-07-release-artifacts.md): [todo 2026-11-07, in-progress 2026-11-07 chief-of-staff (phases 1+2 done), phase 1 approved 2026-11-07 security-reviewer, phase 2 reviewed + phase 3 designed 2026-11-07 architect, waiting on CEO decisions]

## Blocked

## In Review



## Done

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
