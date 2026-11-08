# Reporting contract

Any device that wants to show up on the dashboard sends **one JSON object
per UDP datagram** to the daemon's port (`8765` by default). A datagram is
already a message boundary, so no newline terminator or connection
handshake is needed. The daemon may answer with **one small reply** (see
[Replies](#replies-daemon-to-reporter)); a reporter that never reads its
socket keeps working, it just misses the reply.

A datagram may carry at most **1500 bytes of JSON**. The daemon
reads only the first 1500 bytes of a larger one, which leaves broken JSON,
so the report is dropped. A normal report is a few hundred bytes.

> **v1 has no authentication.** Transport is plain UDP, and anything that
> can reach port 8765 can post a report under any `device_id` it likes, or
> hold an id first and lock the real device out (see [Id ownership](#id-ownership)).
> `PROTOCOL` in `daemon/src/main.rs` is the single switch point for adding
> TCP and TCP+mTLS later (the private CA in `pi-homelab-ca` is already set
> up for that). It is fine to run as-is on a trusted LAN in the meantime, not
> fine to expose beyond it.

```json
{
  "device_id": "my-server",
  "label": "Server",
  "hostname": "my-server",
  "ip": "192.168.1.60",
  "cpu_pct": 12.3,
  "mem_used_mb": 1234.0,
  "mem_total_mb": 8000.0,
  "disk_used_gb": 45.2,
  "disk_total_gb": 500.0,
  "temp_c": 45.0,
  "uptime_s": 123456.0
}
```

| Field | Type | Notes |
|---|---|---|
| `device_id` | string | Stable, unique identifier, the key the Pi tracks the device by. The reference client derives it from the machine id by default (`dp-` plus 8 hex chars, e.g. `dp-3f9a0c41`); `--device-id` overrides it. Must match the id rule below. |
| `label` | string | Short display name shown on the dashboard page (e.g. `"Server"`). |
| `hostname` | string | Shown under the label. |
| `ip` | string | IPv4 or IPv6 address as text, not checked for format. |
| `cpu_pct` | number | 0 to 100. |
| `mem_used_mb` / `mem_total_mb` | number | Megabytes. |
| `disk_used_gb` / `disk_total_gb` | number | Gigabytes, of `/`. |
| `temp_c` | number | 0 if unavailable; the dashboard shows "N/A" for 0. |
| `uptime_s` | number | Seconds since boot. |
| `ack` | bool, optional | `true` asks the daemon for an `ok` reply (used by `deskpane report --once`). Missing means `false`. Not part of `Report`, so it never reaches the dashboard; daemons older than 0.1.0-beta.3 ignore it. |

### Default device id (reference client)

```
input  = "deskpane-device-id-v1:" + machine_id (trimmed, lowercase)
id     = "dp-" + first 8 hex chars of SHA-256(input)
```

The machine id comes from `/etc/machine-id` (then `/var/lib/dbus/machine-id`)
on Linux and `MachineGuid` under `HKLM\SOFTWARE\Microsoft\Cryptography` on
Windows. Only the hash leaves the machine. With no machine id the client
stops with `no machine id found ... Pass a name: --device-id <name>` (exit 2).
The default label is the hostname, cut to 64 bytes (the id when the hostname
is empty).

## Limits a valid report must respect

The daemon checks every report before storing it (`validate` in
`daemon/src/contract.rs`). A report that breaks any rule is dropped as a
whole and logged on the daemon as
`daemon: rejected report from <peer>: <reason>`. The reason names the field
and never repeats the bad text.

| Field | Rule |
|---|---|
| `device_id` | `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`: 1 to 64 ASCII letters, digits, dot, dash or underscore, starting with a letter or digit. |
| `label` | Not empty. At most 64 bytes (UTF-8 bytes, not characters). |
| `hostname` | May be empty. At most 64 bytes. |
| `ip` | May be empty. At most 45 bytes (fits any IPv4 or IPv6 address without a zone id). |
| every string | No control characters (NUL, newline, tab, escape, DEL and the other Unicode control characters). |
| every number | Finite: `NaN`, `Infinity` and `-Infinity` are rejected (JSON cannot carry these; serde_json refuses them as a bad report). |
| `cpu_pct` | `0` to `100`, both included. |
| `mem_used_mb`, `mem_total_mb` | `>= 0`, and `mem_used_mb <= mem_total_mb * 1.01` (1% tolerance for rounding). |
| `disk_used_gb`, `disk_total_gb` | `>= 0`, and `disk_used_gb <= disk_total_gb * 1.01`. |
| `temp_c` | `-50` to `150`, both included. |
| `uptime_s` | `>= 0`. |

The dashboard also catches drawing errors per frame, so a report that passes
these checks but still cannot be drawn shows an error screen for that page
instead of stopping the display.

The Pi stamps its own receive-time on each report and considers a device
**offline** if no report has arrived in the last 15 seconds (`OFFLINE_AFTER`
in `daemon/src/main.rs`). The dashboard keeps the device in the page
rotation and shows an offline page for it. The device stays there until it
reports again, or until the daemon forgets it after 5 minutes of silence
(see below).

The daemon keeps at most **32 devices** (`MAX_DEVICES`). When the table is
full, a report from a new `device_id` is dropped and logged as
`daemon: device table full, dropping report from <peer>`; devices already in
the table keep updating. A device silent for more than **5 minutes**
(`FORGET_AFTER`) is removed from the table entirely, which frees its slot.
If it reports again later, it comes back as a new device.

## Id ownership

The daemon remembers the **source IP** of the report that created or last
refreshed each entry. The port is ignored on purpose: a restarted reporter
gets a new random port and must keep its id.

| Table state when a valid report for `device_id` arrives from IP `src` | Result |
|---|---|
| id not in the table | stored, owned by `src` (subject to the 32-device cap) |
| id owned by `src` | stored |
| id owned by another IP that has been silent for 15 s or more | stored, now owned by `src` |
| id owned by another IP that is still online | dropped; logged and answered with `id_in_use` (rate limited) |

The duplicate check runs before the table-full check. Two reporters behind
the same NAT share one source IP, so a duplicate id between them is not
detected.

## Replies (daemon to reporter)

The daemon sends a reply from its listening port to the source address of
the report, only after the report has parsed and passed every limit above
(never for junk). Two replies exist:

```json
{"v":1,"error":"id_in_use","device_id":"dev-server","owner_ip":"192.168.1.60"}
{"v":1,"ok":true,"device_id":"dp-3f9a0c41"}
```

| Field | Rule |
|---|---|
| `v` | Reply format version, `1`. A client ignores a reply with any other `v`. |
| `error` | Present on refusal. Only `"id_in_use"` in version 1. A client treats an unknown code as a warning, never as a reason to exit. |
| `ok` | `true` on an acknowledged report; only sent when the report had `"ack": true`. |
| `device_id` | The id the reply is about (already checked by the id rule). |
| `owner_ip` | With `id_in_use`: the IP that owns the id, as text, at most 45 bytes. |

Limits (`daemon/src/contract.rs` and `daemon/src/main.rs`):

- A reply is at most **192 bytes** (`MAX_REPLY_BYTES`) **and** smaller than
  the report that caused it, otherwise it is not sent. The largest
  `id_in_use` reply is 165 bytes (64-byte id, 45-byte IP); the smallest valid
  report is 155 bytes plus its id length beyond 1, so the daemon never
  amplifies.
- At most one reply per source IP per **10 s** (`REPLY_EVERY`) and at most
  **20 replies per second** in total (`REPLIES_PER_SECOND`). The limiter
  remembers at most **256** source IPs (`MAX_REPLY_SOURCES`) and drops the
  oldest. The duplicate log line goes through the same limiter.

## Reference client behaviour and exit codes

In the loop, the client reads waiting replies after each send without
blocking. On `id_in_use` it prints
`deskpane: error: device id "dev-server" is already used by 192.168.1.60. Pick another one: --device-id <name>`
(with ` (the Pi itself)` after a loopback owner) once per conflict. It reads
at most 8 replies per report cycle and prints at most one conflict line per
cycle. When the conflict lasts (a second `id_in_use` at least **16 s** after
the first, with no gap longer than 25 s, or one report interval plus 5 s if
longer), it prints
`deskpane: error: device id "dev-server" is still used by 192.168.1.60. Pausing reports for 300 s, then trying again. Pick another one: --device-id <name>`,
sends nothing for **300 s**, then tries again, and repeats while the
conflict lasts, with one log line per pause. The loop **never exits** for a
conflict, so a forged or passing conflict cannot stop a reporter for good.
A reporter whose IP just changed is refused for up to 15 s and never pauses.

`deskpane report --once` sends one report with `"ack": true`, waits up to 3 s
and exits (exit 10 and 12 come only from `--once`):

| Binary | Exit | Meaning | systemd |
|---|---|---|---|
| `deskpane` | 0 | `--once`: acknowledged | |
| `deskpane` | 2 | usage error, including "no machine id, pass --device-id" | restarts (fix the settings) |
| `deskpane` | 10 | `--once`: device id in use by another machine (no grace window: at install time a clash is real) | not used by units |
| `deskpane` | 12 | `--once`: no answer (Pi off, firewall, a daemon older than 0.1.0-beta.3, or another check from the same IP in the last 10 s, which the reply rate limit swallows) | not used by units |
| `deskpane-daemon` | 11 | UDP port in use by another program | `RestartPreventExitStatus=11` |

> **Spoofing.** Replies are not authenticated in v1. A LAN host that sees a
> reporter's traffic can forge `id_in_use` replies and make that reporter
> pause for 300 s at a time while the forgery goes on (a denial of service
> of one reporter that ends when the forgery stops). Squatting an id is possible
> too. Authenticating UDP reports and replies (T-006) is scheduled for
> v0.2.0, reusing the token store planned for the control API.

## Agent status packets

The same port also takes a second kind of packet: the status of one AI agent session (Claude
Code, sent by its hooks through `deskpane agent`). A packet with no `kind` is a stats report, as
above, so every old reporter keeps working. A packet with `"kind": "agent"` is an agent packet.
Any other `kind` is dropped, and the log line does not repeat the value.

```json
{"kind":"agent","v":1,"sid":"3f9a0c41b2d7","device_id":"dp-3f9a0c41","project":"my-project","state":"needs_you","doing":"allow Bash?","t":1794152400123}
```

| Field | Type | Rule |
|---|---|---|
| `kind` | string | Exactly `"agent"`. |
| `v` | integer | Agent packet version, `1`. Any other value drops the packet. |
| `sid` | string | The session key: exactly 12 lowercase hex characters. The client makes it from the Claude Code session id with SHA-256, so the raw session id never leaves the machine. |
| `device_id` | string | The same id rule as for reports. The screen uses it to show the label of the machine. |
| `project` | string | 0 to 32 bytes (UTF-8 bytes, not characters). May be empty: the screen then shows `?`. |
| `state` | string | One of `idle`, `working`, `done`, `needs_you`, `error`, `ended`. |
| `doing` | string | 0 to 32 bytes. A short fixed word or a tool name, for example `allow Bash?`, `thinking`, `API error: rate_limit`. May be empty. |
| `t` | integer | The sender's clock, in milliseconds since the Unix epoch. Zero or more. The daemon uses it only to put packets of one session in order. |
| `sub` | string, optional | Set only by a subagent: exactly 8 lowercase hex characters, a hash of the subagent id. A missing `sub` means the main agent. |
| `ctx` | integer, optional | 0 to 100, context used in percent. Reserved: the client does not send it yet, and the screen does not show it. |

The states mean:

| `state` | Meaning |
|---|---|
| `idle` | The session started and waits for a prompt. |
| `working` | A prompt or a tool is running. |
| `done` | The turn is over. |
| `needs_you` | The agent waits for you (a permission, a question or input). |
| `error` | The turn ended with an API error. `doing` names it. |
| `ended` | The session closed. The daemon removes the row. |

Limits (`validate_agent` in `daemon/src/contract.rs`):

- `project` and `doing` have no control characters and no Unicode format characters (right to
  left overrides, zero width spaces and similar). They draw nothing but could hide or reorder
  text on the screen.
- An unknown field is ignored, so a later optional field does not need a new `v`.
- A packet that breaks a rule is dropped as a whole. The log line names the field and never
  repeats the bad text.
- The largest valid packet is well under the 1500-byte read limit.

### Subagents and `sub`

Claude Code can run several subagents at once. Each subagent packet carries its own `sub`. The
daemon keeps a short list of the `sub` keys that wait for you, at most 8 for each session
(`MAX_PENDING_PER_SESSION`). The row stays `needs_you` while any of them waits, even when
another subagent of the same session keeps working. An answer from one `sub` clears only that
`sub`. Packets of different `sub` keys are never compared by `t`, because parallel subagents
send at their own pace.

### Daemon rules for agent packets

| Situation | Result |
|---|---|
| `ended` for a session in the table, from its owner IP | the row is removed |
| `ended` for a session the daemon does not know | ignored |
| A packet for a session that ended a moment ago | dropped, when it arrives inside the 5 s reorder window |
| The session belongs to another IP that sent something in the last 120 s | dropped |
| The session belongs to another IP that has been silent for 120 s or more | stored, the sender is now the owner |
| A packet older than the stored one of the same key by less than 5 s | dropped (a late packet) |
| A packet older by 5 s or more | stored (the sender's clock jumped back) |
| A new session, and that IP already owns 8 sessions | dropped |
| A new session, and the table has 16 sessions | the oldest `idle`, `done` or `error` row is replaced; with none, the packet is dropped |

The numbers are `MAX_AGENTS` (16), `MAX_AGENTS_PER_SOURCE` (8), `AGENT_TAKEOVER_AFTER` (120 s) and
`AGENT_REORDER_WINDOW` (5 s) in `daemon/src/main.rs`. A session with no packet for 4 hours
(`AGENT_FORGET_AFTER`) is forgotten, whatever its state. A `working` session with no packet for
10 minutes (`AGENT_QUIET_AFTER`) is marked `quiet`.

**Agent packets get no reply.** The daemon never answers one, not for an accepted packet, not for
a refused one. A client that waits for an answer waits for nothing. `deskpane agent test` shows
a fake session on the screen instead. Refusals and bad packets go to the daemon log through a
limiter: at most one line per source per 10 s. A new session and a forgotten session get one
line each.

An old daemon (before the agent view) reads an agent packet as a broken report and drops it with
one log line. Upgrade the Pi first.

## The socket line

The daemon relays what it knows to the dashboard over a Unix domain socket (see "How it works"
in [CONTRIBUTING.md](../CONTRIBUTING.md#how-it-works)), one JSON line a second. A line is an
envelope with one key for each view:

```json
{"v":1,"views":{"stats":{"devices":[]},"agents":{"sessions":[]}}}
```

| Field | Rule |
|---|---|
| `v` | Envelope version, `1`. |
| `views.stats.devices` | The list of `DeviceStatus`: the report fields plus `"online": true` or `false`. Same content as the bare array of v0.1.x lines. |
| `views.agents.sessions` | The list of agent sessions, below. Empty when none. |

Each session:

| Field | Type | Meaning |
|---|---|---|
| `sid` | string | The 12 hex session key. |
| `device_id` | string | The id from the packet. |
| `label` | string | The label of the device with that id, or empty when no reporter runs with it. The screen then shows the id. |
| `project` | string | As sent. Empty means `?`. |
| `state` | string | One of the six states. |
| `doing` | string | As sent. While a subagent waits, the question of the oldest waiting one. |
| `running_s` | integer | Seconds since the daemon first saw the session. Resets when the Pi restarts. |
| `state_s` | integer | Seconds since the state last changed. |
| `silent_s` | integer | Seconds since the last packet. |
| `quiet` | bool | `true` for a `working` session with no packet for 10 minutes. Never set for other states. |
| `ctx` | integer or null | Always present. `null` until the client sends context use. |

The dashboard also still reads the old bare array of `DeviceStatus`, so a new dashboard works
with an old daemon. The report contract does not change, only the last hop does.
