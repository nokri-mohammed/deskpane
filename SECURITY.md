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

An authenticated transport (TCP with mutual TLS) is planned. It has no release date.

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

The reporter on your other computers only sends. It does not accept input from the network.

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
