# Backlog

## To Do

- T-002 Validate reports in the daemon before insert, try/except around drawing: [todo 2026-11-07]
- T-003 Cap device table at 32 and expire after 5 min silence, 1500-byte UDP buffer: [todo 2026-11-07]
- T-005 Relay skips RecvError::Lagged instead of disconnecting: [todo 2026-11-07]
- T-006 Security MEDIUM items (HMAC, firewall, socket in /run, systemd hardening, sudoers): [todo 2026-11-07]

## In Progress

- T-007 Open-source release v0.1.0 (plan: docs/plans/2026-11-07-open-source-release.md): [todo 2026-11-07, in-progress 2026-11-07 chief-of-staff (phase 1 cleanup and phase 2 versioning done)]

## Blocked

## In Review

## Done

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
