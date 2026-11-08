# Security policy

## Supported versions

Only the latest release gets security fixes. Upgrade to the latest release before you report a
problem.

| Version | Supported |
|---|---|
| latest release | yes |
| older releases | no |

## Design: for a trusted home network only

deskpane is made for a home network (LAN) that you trust. Do not expose it to the
internet.

In version 0.1.x there is no authentication and no encryption:

- The daemon on the Pi receives plain UDP packets on port 8765, on every network interface
  (`0.0.0.0:8765`).
- Any device that can reach that port can send a report. It can use any device id, including
  the id of one of your real devices. The dashboard then shows the false values.
- Reports travel as plain JSON. Anyone on the same network can read them. A report contains
  the device's hostname, IP address and system stats.
- A device on the network can send reports with up to 32 different ids. This fills the device
  table, and the daemon then drops reports from new devices. Your existing devices keep
  updating.
- A device on the network can send reports with the id of one of your devices before that
  device does ("squatting"). The daemon then refuses your real device while the other device
  keeps sending. See [Replies from the daemon](#replies-from-the-daemon).

UDP authentication for reports and replies is planned for v0.2.0. An authenticated transport
(TCP with mutual TLS) is also planned. It has no release date.

## Device ids

By default, the reporter makes the device id from the machine id. The id is `dp-` plus 8 hex
characters of a SHA-256 hash over a fixed deskpane prefix and the machine id. For example:
`dp-3f9a0c41`. The raw machine id never leaves the machine. You can set your own id with
`--device-id`.

## What the daemon does to limit damage

- It reads at most 1500 bytes of each UDP packet. A longer packet becomes broken JSON, and the
  daemon drops it.
- It accepts only a JSON object that has every report field, each with the right type. It
  drops anything else. It ignores fields that it does not know.
- It checks every report against fixed limits before it stores it. For example: text fields
  are at most 64 bytes and contain no control characters, and numbers are finite and in range.
  [docs/CONTRACT.md](docs/CONTRACT.md) lists every limit.
- It keeps at most 32 devices in memory. It removes a device after 5 minutes of silence, which
  frees its slot.
- When it rejects a report, its log message names the field but does not repeat the bad text.
- The dashboard catches drawing errors for each page, so one bad report cannot stop the screen.

What the installer does on the Pi:

- The daemon, the dashboard and the Pi's own deskpane report as `deskpane`, a system user that
  cannot log in.
- The program files in `/opt/deskpane` belong to root. The service user cannot
  change them.
- The local socket between the daemon and the dashboard is in `/run/deskpane/`.
  Only the `deskpane` user can open it. This hop does not use the network, so file
  permissions are its access control and it needs no TLS.
- The installer downloads the display driver file from one fixed commit of the vendor's
  project. It checks the file's SHA-256 hash before it writes anything to `/boot`.
- Each release has a `SHA256SUMS` file, so you can check your downloads.

## Replies from the daemon

From version 0.1.0-beta.3, the daemon can send a small UDP reply to a reporter. The daemon
sends a reply only after a report passes all checks. It never replies to broken or rejected
packets. There are two replies:

- `id_in_use`: another IP address already uses this device id. The reply names that owner IP.
- `ok`: the report was received. The daemon sends it only when the report asks for it with
  `"ack": true`. `deskpane report --once` asks for it.

[docs/CONTRACT.md](docs/CONTRACT.md) shows the shape of each reply.

Limits on replies:

- A reply is at most 192 bytes. It is always smaller than the report that caused it, so the
  daemon cannot amplify traffic.
- The daemon sends at most 1 reply to each source IP every 10 seconds.
- The daemon sends at most 20 replies each second in total.
- The daemon remembers at most 256 source IPs for these limits. It forgets the oldest first.

What the reporter does with replies:

- The reporter's socket is connected to the daemon's address. The reporter only accepts
  packets from that IP address and port.
- It ignores replies about other device ids.
- It never prints text from a reply, except the checked device id and a parsed IP address.

Limits of this design:

- Reports and replies are not authenticated. A device on your network can forge an
  `id_in_use` reply. It can also take the id of one of your devices first. In both cases, the
  reporter pauses its reports for 5 minutes and then tries again. It does not stop for good.
- The daemon tracks the owner of each id by IP address. Reporters behind the same NAT router
  share one IP address, so the daemon cannot tell them apart.

## Exit codes

| Program | Exit code | Meaning |
|---|---|---|
| `deskpane-daemon` | 11 | UDP port 8765 is in use by another program. systemd does not restart the daemon. |
| `deskpane report --once` | 10 | Another machine uses this device id. |
| `deskpane report --once` | 12 | No answer from the daemon. |

## Recommendations

- Run the dashboard on a home network that you trust. Do not use it on a shared or public
  network.
- Do not forward port 8765 on your router. Do not expose the Pi to the internet in any other
  way.
- If you can, use a firewall on the Pi. Allow UDP port 8765 only from the computers that
  report. For example, with `ufw`:

  ```bash
  sudo ufw allow from 192.168.1.0/24 to any port 8765 proto udp
  ```

- Keep Raspberry Pi OS up to date (`sudo apt update && sudo apt full-upgrade`).
- Upgrade to the latest release of this project.

## Report a vulnerability

Do not open a public issue for a security problem.

Report it privately on GitHub:

1. Go to the repository's **Security** tab.
2. Choose **Report a vulnerability**.
3. Describe the problem, the version, and the steps to reproduce it.

GitHub calls this "private vulnerability reporting". Only the maintainer sees your report. The
fix and a security advisory are published after the problem is fixed.

This is a hobby project with one maintainer. Replies are best effort.

The known design limits above (no authentication, no encryption) are not vulnerabilities. You
do not need to report them.
