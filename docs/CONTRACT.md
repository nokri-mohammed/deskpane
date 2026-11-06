# Reporting contract

Any device that wants to show up on the dashboard sends **one JSON object
per UDP datagram** to the daemon's port (`8765` by default). No response is
expected back — UDP is fire-and-forget, and a datagram is already a
message boundary, so no newline terminator or connection handshake is
needed.

> **v1 has no authentication.** Transport is plain UDP, and anything that
> can reach port 8765 can post a report under any `device_id` it likes.
> `PROTOCOL` in `daemon/src/main.rs` is the single switch point for adding
> TCP and TCP+mTLS later (the private CA in `pi-homelab-ca` is already set
> up for that) — fine to run as-is on a trusted LAN in the meantime, not
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
| `device_id` | string | Stable, unique identifier. Used as the dict key on the Pi — pick something that won't change (hostname is fine). |
| `label` | string | Short display name shown on the dashboard page (e.g. `"Server"`). |
| `hostname` | string | Shown under the label. |
| `ip` | string | Primary IPv4 address. |
| `cpu_pct` | number | 0–100. |
| `mem_used_mb` / `mem_total_mb` | number | Megabytes. |
| `disk_used_gb` / `disk_total_gb` | number | Gigabytes, of `/`. |
| `temp_c` | number | 0 if unavailable — the dashboard shows "N/A" for 0. |
| `uptime_s` | number | Seconds since boot. |

The Pi stamps its own receive-time on each report and considers a device
**offline** if no report has arrived in the last `OFFLINE_AFTER_S` seconds
(default 15s in the daemon, `daemon/src/main.rs`) — it drops off the page
rotation until it reports again.

This same JSON shape is relayed locally from the daemon to the dashboard
over a Unix domain socket (see `README.md` → Architecture) — the contract
doesn't change, only the transport does for that last hop.
