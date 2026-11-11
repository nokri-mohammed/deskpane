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
- It first reads only the `kind` of a packet. No `kind` means a stats report. `"agent"` means an
  agent status packet. Any other kind is dropped, and the log line does not repeat it. It
  then accepts only a JSON object that has every field of that kind, each with the right type,
  and drops anything else. It ignores fields that it does not know.
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

Agent status packets never get a reply, whether the daemon accepts them or not (see
[Agent status packets](#agent-status-packets)). The reply rules below are for stats reports
only.

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

## Agent status packets

From version 0.1.0-beta.4, a computer can send the status of its Claude Code sessions to the
same UDP port. A hook runs `deskpane agent` at each event and the daemon shows one row for each
session. [docs/CONTRACT.md](docs/CONTRACT.md) has the packet.

What is sent:

- A session key (`sid`): 12 hex characters of a SHA-256 hash over a fixed prefix and the Claude
  Code session id. The raw session id names the transcript file and is used to resume a
  session, so it never leaves the machine. A subagent adds an 8 character hash of its id.
- The device id of the computer, which is already in every stats report.
- The project name: the last part of the project folder, cut to 32 bytes. Never the full path,
  because a path contains your user name. If the project folder is your home folder, the name
  would be your user name, so the client sends an empty name and the screen shows `?`.
- A state word (`idle`, `working`, `done`, `needs_you`, `error`, `ended`).
- A short text of at most 32 bytes: a tool name (an MCP tool is sent as plain `MCP`), a fixed
  word such as `thinking`, or `API error:` with a word from a fixed list.
- The sender's clock in milliseconds.

What is never sent: your prompts, tool input or output, file names and paths, commands, the
assistant's answers, notification texts, the transcript path, the working folder, the model and
the session title.

You can change the project name. `--project NAME` shows an alias. `--hide-project` sends no
name at all. The screen is in the room, so use them if visitors should not see your project
names.

Limits of this design:

- Packets are plain JSON on the network, like stats. A device that can see the traffic learns
  your project names, the tools you use and when you work. It also learns the `sid` of each
  session.
- Nothing is signed. A device on the network can show a false row, including a false
  "needs you". The worst result is that you walk to your computer for nothing, or miss a row.
  UDP authentication (planned for v0.2.0) is meant to cover agent packets as well.

## Limits

These limits keep a bad or hostile sender from filling the table, the log or the network:

- The daemon keeps at most 16 sessions in total and at most 8 for each source IP. A new session
  from a full source is dropped. When the table is full, a new session replaces the oldest
  `idle`, `done` or `error` row. It never replaces a `working` or `needs_you` row. If there is
  none to replace, the new packet is dropped.
- A session belongs to the IP address that created it. A packet from another IP is dropped while
  the owner sent something in the last 120 seconds. After 120 seconds of silence, another IP
  can take the session over.
- **The weakness of that takeover:** a session that waits for you (`needs_you`) sends nothing
  while it waits, so after 120 seconds it counts as silent. A device on your network that has
  seen the `sid` can then take the row over and change it. It needs the `sid`, which only a
  device that sees the traffic has. The result is a false row, as above. It is a known limit,
  not a bug, and authentication is the real fix.
- A packet that arrives late, up to 5 seconds older than the stored one, is dropped. A packet
  that arrives after the session `ended`, inside that window, is dropped too, so a late packet
  cannot bring a closed row back. A parallel subagent has its own key and its own order.
- A session with no packet for 4 hours is forgotten, whatever its state. A `working` row with no
  packet for 10 minutes is marked as quiet on the screen.
- Refusals and bad packets go to the log through a limiter: at most one line for each source
  every 10 seconds. A stored packet is not logged. A new and a forgotten session get one line
  each. A sender cannot fill the journal.
- The daemon sends no reply to an agent packet. Nothing can be reflected or amplified.

## Claude Code hooks

`deskpane agent` runs inside Claude Code hooks, on your computer. It is made so that it cannot
get in the way:

- It always exits with code 0, even when the packet cannot be sent or the input is broken.
- It writes nothing to stdout. Claude Code adds the stdout of some hooks to the conversation,
  so even one stray line would reach the model. Messages go to stderr, and only with `--verbose`.
- It gives up after 500 milliseconds and exits. It reads the hook's input to the end, up to a
  cap of 8 MiB, and keeps only a few small fields of it. It sends one UDP packet and does not
  wait for an answer.
- It sends only the fields listed above. The rest of the hook input is thrown away.

`deskpane agent install-hooks` and `uninstall-hooks` edit your Claude Code settings file
(`~/.claude/settings.json`, or the file you give with `--settings`):

- The file is parsed first. If it is not valid JSON, or the hooks part has an unexpected shape,
  the command stops and writes nothing.
- It adds or removes only its own handler. Every other entry keeps its value and order. Running
  it again changes nothing.
- It copies the file to `<name>.deskpane-backup-<time>` before it writes. A backup never
  overwrites an older one. Backups and the temporary file are created with mode 0600, so
  other users on the computer cannot read your settings.
- It writes a temporary file and renames it over the settings file. It refuses to write if the
  file changed while it worked. If `settings.json` is a symlink, the real file behind it is
  edited and the link stays.
- The hook command is the absolute path of the `deskpane` program. The command warns when that
  path is in a build or temp folder, which may be deleted later.

## Character packs

The agents page can draw pixel characters from a pack that you provide
([docs/CHARACTERS.md](docs/CHARACTERS.md)). A pack is a zip file, so it is treated as untrusted
input:

- The installer (`install.sh --characters`) accepts only a normal file of at most 4 MiB. A
  symlink or a device is refused. The validator opens the file once and reads it by file
  descriptor, so the file cannot change between the check and the copy. `get.sh` runs the same
  check before it installs anything.
- The zip is never unzipped and nothing in it is run. A small Python program (standard
  library only, started with `python3 -I`) reads it.
- The whole zip is refused if any entry has an absolute path, a drive letter, a backslash, a
  NUL byte or a `..` part, is a symlink, is encrypted or appears twice, or if the zip has more
  than 64 entries.
- Only these names are used: `NPC_1.png` to `NPC_15.png` at the root, and ten emote files
  `Emotes/<Name>_emote_16x16.png`. Other entries are skipped and counted, never copied. A
  used file is at most 64 KiB, starts with the PNG signature and has the exact size in its
  header (128 x 208 or 64 x 16). Reading stops at the cap, so a zip bomb cannot fill memory.
  The files together are at most 2 MiB.
- Files are written under the fixed names above, never under a name from the zip. They go to a
  new folder that replaces the old one in one step. If anything fails first, the old pack stays.
- The folder is `/var/lib/deskpane/characters`, owned by root (folder 0755, files 0644). The
  dashboard only reads it, and the service user cannot change it. An upgrade does not touch it.
  If the pack cannot be installed after the check, the installer warns and the screen uses the
  built-in icons.
- The pack is optional. A missing or broken pack never stops the agents page.
- **Licence guard:** character packs are usually paid and not free to share. The repository
  never contains one, no release bundle contains one and the docs never show one. A script
  (`scripts/check-no-pack-files.sh`) fails CI if git tracks a pack file, and the bundle script
  refuses to pack one.

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
