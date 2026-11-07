# Reporting contract

Any device that wants to show up on the dashboard sends **one JSON object
per UDP datagram** to the daemon's port (`8765` by default). No response is
expected back. UDP is fire-and-forget, and a datagram is already a
message boundary, so no newline terminator or connection handshake is
needed.

A datagram may carry at most **1500 bytes of JSON**. The daemon
reads only the first 1500 bytes of a larger one, which leaves broken JSON,
so the report is dropped. A normal report is a few hundred bytes.

> **v1 has no authentication.** Transport is plain UDP, and anything that
> can reach port 8765 can post a report under any `device_id` it likes.
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
| `device_id` | string | Stable, unique identifier. Used as the dict key on the Pi, so pick something that won't change (hostname is fine). |
| `label` | string | Short display name shown on the dashboard page (e.g. `"Server"`). |
| `hostname` | string | Shown under the label. |
| `ip` | string | IPv4 or IPv6 address as text, not checked for format. |
| `cpu_pct` | number | 0 to 100. |
| `mem_used_mb` / `mem_total_mb` | number | Megabytes. |
| `disk_used_gb` / `disk_total_gb` | number | Gigabytes, of `/`. |
| `temp_c` | number | 0 if unavailable; the dashboard shows "N/A" for 0. |
| `uptime_s` | number | Seconds since boot. |

## Limits a valid report must respect

The daemon checks every report before storing it (`validate` in
`daemon/src/contract.rs`). A report that breaks any rule is dropped as a
whole and logged on the daemon as
`daemon: rejected report from <peer>: <reason>`. The reason names the field
and never repeats the bad text.

| Field | Rule |
|---|---|
| `device_id`, `label` | Not empty. At most 64 bytes (UTF-8 bytes, not characters). |
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
**offline** if no report has arrived in the last `OFFLINE_AFTER_S` seconds
(default 15s in the daemon, `daemon/src/main.rs`). It drops off the page
rotation until it reports again.

The daemon keeps at most **32 devices** (`MAX_DEVICES`). When the table is
full, a report from a new `device_id` is dropped and logged as
`daemon: device table full, dropping report from <peer>`; devices already in
the table keep updating. A device silent for more than **5 minutes**
(`FORGET_AFTER`) is removed from the table entirely, which frees its slot.
If it reports again later, it comes back as a new device.

This same JSON shape is relayed locally from the daemon to the dashboard
over a Unix domain socket (see `README.md` → Architecture). The contract
doesn't change, only the transport does for that last hop.
