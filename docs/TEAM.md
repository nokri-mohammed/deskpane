# Team

Who does what on pi-homelab-dashboard. Update the diagram when an executive or plugin is added
or removed, and add a log line every time an executive works on the project.

```mermaid
flowchart TD
    CEO["CEO (owner)<br/>decides what gets built, approves plans, commits, releases"]
    COS["Chief of staff (main Claude session)<br/>plans, routes work, integrates, reports"]
    CEO --> COS

    subgraph architecture["plugin: architecture"]
        ARCH["architect<br/>plans in docs/plans/, protocol and transport design"]
        C4["c4-context / container / component / code<br/>architecture diagrams"]
    end

    subgraph tdd["plugin: tdd"]
        TDD["tdd-guide<br/>tests first for daemon and reporter services"]
    end

    subgraph review["plugin: review"]
        CR["code-reviewer<br/>Rust diffs against rust-patterns"]
        SR["security-reviewer<br/>UDP input parsing, untrusted reports"]
    end

    subgraph essentials["plugin: essentials"]
        DOC["doc-updater<br/>README, CONTRACT.md, CLAUDE.md, backlog"]
    end

    subgraph personal["personal agents"]
        SS["security-specialist<br/>firewall, ports, systemd, Pi provisioning"]
    end

    PAT[["skill: rust-patterns (plugin: patterns)<br/>used by everyone writing or reviewing Rust"]]

    COS --> ARCH
    COS --> C4
    COS --> TDD
    COS --> CR
    COS --> SR
    COS --> DOC
    COS --> SS
    PAT -.-> TDD
    PAT -.-> CR
```

## Involvement log

| Date | Who | What |
|---|---|---|
| 2026-11-07 | chief of staff | Rust refactor: generic `services/` (UDP, Unix socket, broadcaster, system stats) with unit tests, shared UDP code in the daemon library |
| 2026-11-07 | security-specialist | Security review of the daemon: 2 HIGH, 4 MEDIUM, 2 LOW findings (backlog T-002 to T-006) |
| 2026-11-07 | chief of staff | Onboarded the team: architecture, tdd, review, patterns, essentials; wrote the CLAUDE.md briefing, backlog and this file |
| 2026-11-07 | chief of staff | Reviewed open-source readiness, drafted the v0.1.0 release plan |
| 2026-11-07 | chief of staff | Release phase 1: scrubbed private values and the unlicensed overlay from all history; phase 2: one workspace version, release-please |
