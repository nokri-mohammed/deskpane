# beta.3: unique device ids, duplicate rejection, port-in-use exit, screen tweaks

Date: 2026-11-07. Author: architect. Status: design, waiting for the CEO to read it. tdd-guide
builds it test first afterwards.

Tickets: T-032 (automatic unique ids), T-033 (reject duplicate ids, error on the device),
T-027 (port in use: one clear error, no restart loop), T-034 (screen: version, bigger clock, no
divider).

## Why

In the real-Pi test two machines were installed with the same `--device-id dev-server` (labels
"test" and "Dev server"). The daemon keys its table by `device_id` (`DeviceTable` in
`daemon/src/device_table.rs`, `insert_report` overwrites), so the two machines overwrote each
other every few seconds and the label on the screen flipped. Nobody got an error. Separately,
an old process on UDP 8765 made `deskpane-daemon` panic in `UdpSocketServer::new` and systemd
restarted it 51 times.

Goals:

1. A user never has to invent a device id. The default is unique per machine and stable.
2. If two machines still end up with the same id, the second one is refused, and the error
   shows on that machine (terminal, journal, installer), not only in the Pi's log.
3. A daemon that cannot bind its port, and a reporter whose id is taken, each print one clear
   line and stay stopped instead of looping.
4. The overview screen shows the version, a bigger clock and no divider line.

## Summary of the design

| Item | Decision |
|---|---|
| Default id | `dp-` plus 8 hex chars of SHA-256 over a fixed deskpane prefix and the machine id. Example: `dp-3f9a0c41`. Computed by the client at runtime. |
| Machine id source | Linux: `/etc/machine-id`, then `/var/lib/dbus/machine-id`. Windows: `MachineGuid` under `HKLM\SOFTWARE\Microsoft\Cryptography`. |
| No machine id | The client stops with a clear error asking for `--device-id`. No silent hostname fallback. |
| Default label | The hostname, cut to 64 bytes. Empty hostname: the id. |
| Id ownership | The first source IP that reports an id owns it while the id is online (15 s, `OFFLINE_AFTER`). |
| Duplicate | Dropped. The daemon replies with a small JSON packet, rate limited, never bigger than the request. |
| Reporter on "id in use" | Prints the error. Exits with code 10 only if the conflict lasts past a 16 s grace window, so an IP change does not kill it. |
| Install-time check | `deskpane report --once` sends one report and asks for an answer. get.sh and install.sh run it and show the result. |
| Port in use | `deskpane-daemon` prints one line and exits with code 11. |
| systemd | `RestartPreventExitStatus=10` (reporter units) and `=11` (daemon unit). |
| Screen | Version bottom left, clock font 84 to 120, divider removed, layout maths moved to a pure module with tests. |

## T-032: automatic unique device ids

### Where the id is computed: the client, at runtime

The default id is computed by `deskpane` when it starts, if `--device-id` is missing or empty.
The installers do not write a derived id into any file.

Why the client and not the installer:

- One code path for every way the client runs: by hand, the systemd unit from get.sh, the Pi's
  own reporter, and Windows (which has no installer yet). An installer-only default would leave
  manual runs and Windows without one.
- The client unit from get.sh runs with `DynamicUser=yes` and `ProtectSystem=strict`, so the
  client cannot store a generated id. A value derived from the machine id needs no storage.
- An id derived at runtime cannot drift away from the machine: an SD card cloned onto a second
  Pi gets the same id until its machine id is regenerated, and T-033 then reports the clash
  with a clear message instead of hiding it.
- The installers stay simple. They pass `--device-id` only when the user gave one.

The cost: if the machine id changes (OS reinstall), the id changes and the device shows up as a
new device, while the old entry is forgotten after 5 minutes. That is the correct outcome for a
reinstalled machine.

### Id format and derivation

```
input  = "deskpane-device-id-v1:" + machine_id (trimmed, lowercase)
digest = SHA-256(input)
id     = "dp-" + first 8 hex chars of digest     (11 chars, e.g. dp-3f9a0c41)
```

- 32 bits after the prefix. With the 32-device cap the chance of an accidental clash is about
  1 in 8 million, and T-033 catches it if it happens.
- The `dp-` prefix tells auto ids from hand-picked ones in logs and on the device page.
- It passes the id rule below (starts with a letter, only `[a-z0-9-]`).
- The raw machine id never leaves the machine. `machine-id(5)` asks applications to use a
  keyed or application-specific hash, never the raw value; the fixed prefix makes the hash
  specific to deskpane, so the id cannot be matched against other software's ids.
- The `v1` in the prefix lets a later format change produce new ids on purpose.

### Machine id sources and fallback

| OS | Source, in order | Notes |
|---|---|---|
| Linux | `/etc/machine-id`, then `/var/lib/dbus/machine-id` | World-readable, works under `DynamicUser` and `ProtectSystem=strict`. Must be 32 hex chars; anything else counts as missing. Raspberry Pi OS images ship it empty and systemd fills it on first boot. |
| Windows | `HKLM\SOFTWARE\Microsoft\Cryptography` value `MachineGuid` | Readable by normal users. Read with `reg.exe query` through `std::process::Command`, so no new crate. Must look like a GUID. |
| Other / missing / malformed | none | Stop with: `deskpane: error: no machine id found (/etc/machine-id is missing or empty). Pass a name: --device-id <name>`, exit code 2 (the same code clap uses for usage errors). |

No hostname fallback: cloned images and default hostnames (the same name on every fresh Pi) are exactly the cases
that produce duplicates, so a hostname-based id would bring the original bug back quietly.

Known limit: a VM or disk image cloned after first boot shares the machine id, and so the
deskpane id. T-033 refuses the second machine with a clear error, and the fix is
`--device-id <name>` or regenerating the machine id (`systemd-machine-id-setup` after emptying
the file). INSTALL.md troubleshooting gets that line.

### The id rule moves into the contract

Today `validate` only checks that `device_id` is non-empty, at most 64 bytes and free of control
characters. The installers already enforce `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`. beta.3 makes the
daemon enforce the same rule, so ids are safe in logs, unit files, shell lines and replies.
The label rules stay as they are.

### Changes by file

| File | Change |
|---|---|
| `reporter/src/services/machine_id.rs` (new) | `read_machine_id() -> Option<String>` per OS (`#[cfg(target_os = "linux")]`, `#[cfg(windows)]`); pure `parse_linux_machine_id(text)`, `parse_windows_reg_output(text)`, `derive_device_id(machine_id) -> String`. |
| `reporter/src/main.rs` | `--device-id` and `--label` become `Option<String>`; an empty value counts as not given (the Pi env file and units pass `${DEVICE_ID}` which may be empty). New `resolve_identity(device_id, label, machine_id, hostname) -> Result<(String, String), String>`. Startup line prints the id and says whether it is automatic. |
| `reporter/Cargo.toml` | Adds `sha2` (RustCrypto). See decision 1. |
| `daemon/src/contract.rs` | New `check_device_id` with the rule above, called from `validate`. Reason text never echoes the value (as today). |
| `deploy/env.example` | `DEVICE_ID=` and `LABEL=` empty, with a comment: empty means automatic id and hostname label. Existing `/etc/deskpane/env` files are kept by install.sh, so upgraded Pis keep `pi-zero`. |
| `deploy/install.sh` | `--device-id` and `--label` stay optional and keep their checks; the summary prints the id the Pi reports as (from `--once`, see T-033). |
| `get.sh` | `--reporter-only` no longer derives a device id from the hostname; `DEVICE_ID` in `reporter.env` stays empty unless the user passed one. Label default moves to the client too. |
| `deploy/systemd/deskpane-reporter.service`, client unit in get.sh | Unchanged `ExecStart`; an empty `${DEVICE_ID}` now means automatic. |

### Tests to write first

`reporter/src/services/machine_id.rs`:

- `linux_machine_id_is_trimmed_and_lowercased`
- `linux_machine_id_empty_file_is_missing`
- `linux_machine_id_wrong_length_is_missing`
- `linux_machine_id_non_hex_is_missing`
- `windows_reg_output_yields_the_guid`
- `windows_reg_output_without_machineguid_is_missing`
- `windows_reg_output_with_malformed_guid_is_missing`
- `derived_id_has_dp_prefix_and_8_hex_chars`
- `derived_id_is_stable_for_the_same_machine_id` (fixed input, fixed expected string, so a
  change of algorithm breaks the test on purpose)
- `derived_id_differs_for_different_machine_ids`
- `derived_id_passes_the_contract_id_rule`

`reporter/src/main.rs`:

- `explicit_device_id_is_used_as_given`
- `empty_device_id_means_automatic`
- `missing_device_id_and_no_machine_id_is_an_error`
- `empty_label_defaults_to_hostname`
- `long_hostname_label_is_cut_to_64_bytes_on_a_char_boundary`
- `empty_hostname_label_defaults_to_the_id`

`daemon/src/contract.rs`:

- `device_id_starting_with_dash_is_rejected`
- `device_id_with_space_is_rejected`
- `device_id_with_non_ascii_is_rejected`
- `device_id_with_dot_dash_underscore_passes`
- `auto_style_device_id_passes`

Shell (manual, in the T-028 fake-root harness): get.sh `--reporter-only --daemon ...` with no
`--device-id` writes `DEVICE_ID=` empty.

### Contract and docs

- `docs/CONTRACT.md`: `device_id` rule `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`; the field table says
  the reference client derives it from the machine id by default.
- `docs/INSTALL.md` (doc-updater): `--device-id` optional everywhere; troubleshooting line for
  cloned images.

## T-033: reject duplicate ids, error on the device

### Ownership rule

The table entry gains the source IP of the report that created or last refreshed it:

```
DeviceTable: device_id -> DeviceEntry { report, last_seen: Instant, owner: IpAddr }
```

When a valid report arrives for `device_id` from source IP `src`:

| Table state | Result |
|---|---|
| id not in table | store, `owner = src` (subject to `MAX_DEVICES` as today) |
| id in table, `owner == src` | store (normal update) |
| id in table, `owner != src`, owner silent for `>= OFFLINE_AFTER` (15 s) | store, `owner = src` (the id was free) |
| id in table, `owner != src`, owner online | drop, log, maybe reply `id_in_use` |

Ownership uses the **IP only, not IP and port**. The client binds a new random port every time
it starts, so keying on the port would lock a restarted reporter out of its own id for 15 s
after every restart. The duplicate check runs before the table-full check, so a duplicate is
reported as a duplicate even when the table is full.

### Reply packet (new, daemon to reporter)

Today the protocol is one way. beta.3 adds one reply type, sent from the daemon's listening
socket (port 8765) to the source address of the report:

```json
{"v":1,"error":"id_in_use","device_id":"dev-server","owner_ip":"192.168.1.60"}
```

And, only when the report asked for it (see `--once` below):

```json
{"v":1,"ok":true,"device_id":"dp-3f9a0c41"}
```

| Field | Rule |
|---|---|
| `v` | Reply format version, `1`. A client ignores a reply with another `v`. |
| `error` | Present on refusal. Only `"id_in_use"` in v1. A client treats an unknown code as a warning, never as a reason to exit. |
| `ok` | `true` on an acknowledged report. |
| `device_id` | The id the reply is about. Already validated by the contract rule, so at most 64 safe bytes. |
| `owner_ip` | The IP that owns the id. Text, at most 45 bytes. |

Limits:

- A reply is sent only after a report has parsed and passed `validate`, never for junk.
- The serialized reply must be at most 192 bytes **and** smaller than the report that caused
  it; otherwise it is not sent. Measured: the smallest valid report (1-char id and label, empty
  hostname and ip, all numbers 0) is 155 bytes, plus the id length beyond 1. The largest
  `id_in_use` reply is 101 bytes plus the id length (165 bytes with a 64-byte id and a 45-byte
  IP). The reply carries the request's own id, so it is always at least 50 bytes smaller than
  the request, and the daemon can never amplify.
- At most one reply per source IP per 10 s (`REPLY_EVERY`), and at most 20 replies per second
  in total. The limiter remembers at most 256 source IPs and drops the oldest.
- The duplicate log line on the daemon goes through the same limiter, so a flood cannot fill
  the journal.

Versioning and compatibility:

- Old reporters (beta.1, beta.2) never read their socket; a reply sits in the kernel buffer and
  is discarded with the socket. Nothing breaks, they just get no error, as today.
- A new reporter against an old daemon gets no replies; it keeps working as today. `--once`
  says "no answer from the daemon (older than 0.1.0-beta.3, or blocked)" as a warning.

### Report packet: one optional field

`--once` needs a positive answer, so a report may carry `"ack": true`. The daemon parses a
packet type that wraps `Report`:

```rust
// what arrives over UDP: a report plus optional request flags
struct ReportPacket {
    #[serde(flatten)]
    report: Report,
    #[serde(default)]
    ack: bool,
}
```

`Report` and `DeviceStatus` stay exactly as they are, so the Unix socket shape the dashboard
reads does not change. Old daemons ignore the unknown `ack` field (serde ignores unknown fields
by default, and `Report` does not use `deny_unknown_fields`). The ack reply uses the same rate
limiter. A loop reporter never sets `ack`.

### Reporter behaviour

The client socket is already `connect`ed to the daemon, so the kernel only delivers packets
whose source is the daemon's IP and port.

- `UdpClient` gains `try_receive(&self, buf) -> Received` with `Received::{Packet(len),
  Nothing, Refused}`, using a non-blocking read. `Refused` is the ICMP port unreachable that a
  connected UDP socket reports as `ECONNREFUSED` (daemon not running on that host).
- In the loop, after each send, the client drains replies without blocking, then sleeps as
  today. The send loop never waits on the network.
- On `id_in_use` the client prints, once per reply:
  `deskpane: error: device id "dev-server" is already used by 192.168.1.60. Pick another one: --device-id <name>`
  If `owner_ip` is a loopback address, it adds ` (the Pi itself)`.
- It exits with code **10** (`EXIT_ID_IN_USE`) only when the conflict lasts: a second
  `id_in_use` reply arrives at least 16 s (`CONFLICT_GRACE`) after the first, with no gap of more
  than 25 s between replies. Otherwise it keeps reporting.

Why the grace window: a reporter whose IP changes (DHCP renew to a new address, laptop moving
from WiFi to cable) is refused for up to 15 s, because the daemon still sees the old IP as the
online owner. Without a grace window that reporter would exit, and with
`RestartPreventExitStatus` it would stay down for good. With the window, a reporter that changed
IP sees at most two replies about 10 s apart (rate limit) and then is accepted once the old IP
goes offline at 15 s; it never reaches 16 s of conflict. A real duplicate keeps the owner online,
gets replies at 0, 10 and 20 s, and exits at about 20 s. The 15 s gap on the dashboard after an
IP change is acceptable (the device shows offline for a moment, then continues).

### `--once`: one report and an answer (install-time check)

`deskpane report --once` (part of the planned one-shot report, next to T-013) builds one report,
sends it with `"ack": true`, waits up to 3 s for a reply, and exits:

| Exit | Meaning | Line printed |
|---|---|---|
| 0 | acknowledged | `deskpane: reported to 192.168.1.50:8765 as dp-3f9a0c41 (My PC)` |
| 10 | `id_in_use` | the error line above (no grace window: at install time a clash is real) |
| 12 | no answer, or `Refused` | `deskpane: warning: no answer from 192.168.1.50:8765 (Pi off, firewall, or a daemon older than 0.1.0-beta.3)` |

Installer use:

- `get.sh --reporter-only --daemon X`: runs `--once` with the same id and label before it enables
  the unit. Exit 10: stop with the error, the unit is written but not enabled, and the line says
  how to rerun with `--device-id`. Exit 12: warn and continue (installing a client before the Pi
  is up is normal). See decision 3.
- `install.sh` (Pi): after the units restart, runs
  `bin/deskpane report --once --daemon 127.0.0.1:8765` with the configured id and label, retrying
  for up to 5 s while the daemon binds. Exit 10 means a LAN machine already uses the Pi's id:
  print the error and the fix (edit `/etc/deskpane/env`), do not fail the whole install. The
  summary prints the id the Pi reports as. The Pi's own running reporter and the check both come
  from 127.0.0.1, so they never clash with each other.

### Changes by file

| File | Change |
|---|---|
| `daemon/src/device_table.rs` | `DeviceEntry { report, last_seen, owner }`; `insert_report(table, report, source: IpAddr, now, max_devices, offline_after) -> InsertOutcome` with `Stored`, `TableFull`, `IdInUse { owner: IpAddr }`. `forget_silent` unchanged in behaviour. |
| `daemon/src/services/reply_limiter.rs` (new, generic) | `ReplyLimiter::new(per_source: Duration, per_second: u32, max_sources: usize)`, `allow(&mut self, source: IpAddr, now: Instant) -> bool`. Pure, takes the time as a parameter. |
| `daemon/src/services/udp.rs` | `UdpSocketServer::send_to(&self, bytes, peer) -> bool`; `try_new` (see T-027); `UdpClient::try_receive`. |
| `daemon/src/contract.rs` | `ReportPacket`, `Reply` (serde, `skip_serializing_if` for absent fields), `MAX_REPLY_BYTES = 192`, `REPLY_VERSION = 1`, `build_reply(...) -> Option<Vec<u8>>` that enforces both size rules. |
| `daemon/src/main.rs` | `udp_server` parses `ReportPacket`, calls the new `insert_report`, and on `IdInUse` or `ack` asks the limiter and sends the reply. `broadcast_statuses` reads the new entry type. New consts `REPLY_EVERY`, `REPLIES_PER_SECOND`, `MAX_REPLY_SOURCES`. |
| `reporter/src/services/conflict.rs` (new) | `ConflictTracker::new(grace, max_gap)`, `on_conflict(now) -> bool` (true means exit). Pure. |
| `reporter/src/main.rs` | Reply draining in `run`; `--once`; exit codes `EXIT_ID_IN_USE = 10`, `EXIT_NO_ANSWER = 12`. |
| `deploy/systemd/deskpane-reporter.service`, client unit in get.sh | `RestartPreventExitStatus=10`. |
| `deploy/install.sh`, `get.sh` | `--once` check as above. |

### Tests to write first

`daemon/src/device_table.rs`:

- `new_device_is_owned_by_its_source`
- `owner_can_update_its_device`
- `other_source_is_refused_while_owner_is_online`
- `refusal_reports_the_owner_ip`
- `other_source_takes_over_once_owner_is_offline`
- `owner_silent_exactly_offline_after_counts_as_offline` (pins the boundary, matching
  `broadcast_statuses`, which uses `silent_for < OFFLINE_AFTER` for online)
- `same_ip_different_port_is_the_same_owner` (the source is an `IpAddr`; this test lives in
  `main.rs` or wherever the `SocketAddr` is reduced to its IP)
- `duplicate_is_reported_before_table_full`
- the existing cap and forget tests, updated to the new signature

`daemon/src/services/reply_limiter.rs`:

- `first_reply_to_a_source_is_allowed`
- `second_reply_within_window_is_refused`
- `reply_after_window_is_allowed`
- `different_sources_are_limited_separately`
- `global_per_second_cap_applies_across_sources`
- `oldest_source_is_dropped_at_max_sources`
- `time_going_backwards_does_not_panic`

`daemon/src/contract.rs`:

- `id_in_use_reply_has_version_error_id_and_owner`
- `ack_reply_has_version_ok_and_id`
- `reply_is_never_larger_than_the_request`
- `reply_over_max_bytes_is_not_built`
- `packet_without_ack_parses_with_ack_false`
- `packet_with_ack_true_parses`
- `report_without_ack_still_parses_as_report` (old reporters keep working)

`daemon/src/services/udp.rs`:

- `server_send_to_reaches_the_client`
- `client_try_receive_returns_nothing_when_no_packet`
- `client_try_receive_returns_the_reply`
- `client_try_receive_reports_refused_when_nobody_listens` (loopback; Linux delivers the ICMP
  error to a connected socket)

`reporter/src/services/conflict.rs`:

- `single_conflict_does_not_exit`
- `conflict_lasting_past_grace_exits`
- `two_replies_ten_seconds_apart_do_not_exit` (the IP-change case)
- `gap_longer_than_max_gap_restarts_the_window`

`reporter/src/main.rs`:

- `id_in_use_message_names_the_id_and_owner`
- `loopback_owner_is_called_the_pi_itself`
- `unknown_reply_version_is_ignored`
- `unknown_error_code_is_a_warning_not_an_exit`

End to end (a `#[tokio::test]` in `daemon/src/main.rs` tests, real sockets on 127.0.0.1 and a
second loopback address such as 127.0.0.2, which Linux routes without setup):
`second_source_with_same_id_gets_id_in_use_reply`.

### Contract and docs

- `docs/CONTRACT.md`: "No response is expected back" becomes "The daemon may answer with one
  small reply" plus the reply section (shape, fields, limits, rate limit, version rule), the
  ownership rule, the optional `ack` field, and the reporter exit codes.
- `docs/INSTALL.md` (doc-updater): the error message and its fix; the exit codes in
  troubleshooting.
- `SECURITY.md` (doc-updater): the reply, its rate limit, and the spoofing limit below.

### Security notes

- **Squatting (unchanged trust level).** v1 has no authentication. Before beta.3 any LAN host
  could overwrite a device's entry by sending its id; after beta.3 it can instead hold the id
  first and lock the real device out while it keeps sending. Same trust model, different
  symptom; the real device now gets a clear error naming the squatter's IP.
- **Reply spoofing.** A LAN host that can see a reporter's traffic can forge `id_in_use` replies
  from the Pi's IP and port to the reporter's port. Sent twice 16 s apart, they make the
  reporter exit and, because of `RestartPreventExitStatus`, stay down. This is a denial of
  service of one reporter, not code execution: the reply is parsed into a fixed struct, and only
  the validated `device_id` and an IP are printed. This attacker can already flood or spoof
  reports today. It is closed by authenticating packets (T-006). See decision 4: the vision plan
  puts token auth in v0.2.0 for the control API and calls HMAC on UDP reports "not forced", so
  closing this needs the UDP HMAC item scheduled explicitly.
- **Reflection and amplification.** Replies only answer fully valid reports, are never larger
  than the request, are limited per source IP and globally, and the limiter's memory is capped.
  A spoofed-source flood turns into at most one packet of at most 165 bytes per victim per 10 s.
- **Information disclosure.** `owner_ip` tells a sender which IP holds an id. The sender has to
  know the id already, the same IPs are shown on the dashboard, and any LAN host can scan. Fine
  on a trusted LAN, which is the v1 deployment model.
- **Pi's own reporter on 127.0.0.1.** A LAN host cannot spoof a loopback source: Linux drops
  packets from the network with a 127/8 source as martians. A LAN machine that picks the Pi's
  id is refused while the Pi's reporter runs, and is told the owner is "the Pi itself".
- **NAT.** Two reporters behind the same NAT share one source IP, so a duplicate id between them
  is not detected and they overwrite each other as before. Rare on a home LAN (the Pi and its
  reporters are normally on the same subnet); documented.

## T-027: port in use, one error, no restart loop

| File | Change |
|---|---|
| `daemon/src/services/udp.rs` | `UdpSocketServer::try_new(addr) -> Result<Self, std::io::Error>`. `new` (panicking) is removed; its callers use `try_new`. The service stays generic: it returns the error, the meaning lives in `main.rs`. |
| `daemon/src/main.rs` | On `ErrorKind::AddrInUse`: `deskpane-daemon: error: UDP port 8765 is already in use by another program. Find it with: sudo ss -ulpn 'sport = :8765'` and `std::process::exit(EXIT_PORT_IN_USE)` with `EXIT_PORT_IN_USE = 11`. Other bind errors: one line, exit 1. The port in the message comes from `LISTEN_ADDR`. |
| `deploy/systemd/deskpane-daemon.service` | `RestartPreventExitStatus=11`. |

Tests first (`daemon/src/services/udp.rs`): `try_new_on_free_port_succeeds`,
`try_new_on_port_in_use_returns_addr_in_use`, `try_new_on_invalid_address_returns_error`
(these replace the two `..._panics` tests). In `main.rs`: a pure
`bind_error_message(addr, &error) -> (String, i32)` with tests
`addr_in_use_gives_exit_11_and_names_the_port` and `other_bind_error_gives_exit_1`.

Manual check: `systemd-analyze verify` on the unit; on the Pi, hold 8765 with a python socket,
`systemctl restart deskpane-daemon`, expect one journal line and `NRestarts=0`.

## T-034: screen changes (pi/dashboard.py)

### What changes

| Function | Change |
|---|---|
| new `pi/layout.py` | Pure helpers with no pygame import: `read_version(dashboard_dir) -> str`, `overview_layout(width, height, clock_h, date_h, summary_h) -> dict`. Moving the maths here lets it be tested without pygame or a framebuffer. |
| `read_version` | Reads `<dashboard_dir>/../VERSION`. install.sh copies the bundle's `VERSION` to `/opt/deskpane/releases/<v>/VERSION` and the dashboard to `.../dashboard/`, so `current/VERSION` is exactly one level up. Strips whitespace, accepts only the same version regex install.sh uses, caps at 32 chars, returns `"dev"` when the file is missing, unreadable or malformed (running from the repo there is no `VERSION` file next to `pi/`). Read once at startup. |
| `main` (dashboard.py) | `fonts["clock"]` from `Font(None, 84)` to `Font(None, 120)`; calls `read_version` once and passes it to `draw_overview`. |
| `overview_layout` | Moves to `layout.py`; takes the rendered heights and centres the clock, date and summary block vertically, instead of the fixed `height // 2 - 76` and `+ 72` offsets that assume an 84 px font. No `divider_y`. |
| `draw_overview` | Gains a `version` parameter; draws `v0.1.0-beta.3` (or `dev`) in `fonts["tiny"]`, `SUBTEXT` colour, bottom left at 8 px from the edges. Bottom centre has the page dots and bottom right the network warning icon (`width - 28, height - 28`), so bottom left is the free corner. The `pygame.draw.line` divider call is deleted. |

The device pages are unchanged (they keep their own line under the header).

### Tests (first, for the pure parts)

`pi/tests/test_layout.py`, written with stdlib `unittest` so it runs with
`python3 -m unittest discover -s pi/tests` and also under pytest:

- `test_read_version_from_file_one_level_up`
- `test_read_version_strips_newline`
- `test_read_version_missing_file_is_dev`
- `test_read_version_malformed_is_dev`
- `test_read_version_too_long_is_dev`
- `test_overview_block_is_vertically_centred`
- `test_overview_items_do_not_overlap`
- `test_overview_layout_has_no_divider`
- `test_overview_fits_on_480x320_with_120px_clock`

CI: the `scripts` job in `ci.yml` adds `python3 -m unittest discover -s pi/tests`. No new
dependency, since `unittest` ships with Python.

Off-device visual check: render one overview frame with `SDL_VIDEODRIVER=dummy` to an
offscreen `pygame.Surface` and save a PNG (pygame installed on the dev machine). The designer's
`scripts/capture-demo*` may already do this; tdd-guide reuses it rather than adding another
script. On the Pi: check that the clock does not clip at 120 px with the real default font, and
that the version text is readable at 16 px on the 3.5" panel.

## Order of work

1. T-027 (smallest, independent, fixes a real crash loop).
2. T-032 contract id rule and machine id derivation.
3. T-033 daemon side (table, limiter, reply), then reporter side (drain, tracker, `--once`).
4. Installer and unit changes for T-032, T-033 and T-027.
5. T-034 screen.
6. CONTRACT.md by tdd-guide with the code; INSTALL.md and SECURITY.md by doc-updater;
   security-specialist reviews the reply path and the units before beta.3 is tagged.

Exit codes (one table for docs and units):

| Binary | Code | Meaning | systemd |
|---|---|---|---|
| `deskpane` | 2 | usage error, including "no machine id, pass --device-id" | restarts (fix the settings) |
| `deskpane` | 10 | device id in use by another machine | `RestartPreventExitStatus=10` |
| `deskpane` | 12 | `--once`: no answer | not used by units |
| `deskpane-daemon` | 11 | UDP port in use | `RestartPreventExitStatus=11` |

## Decisions for the CEO

1. **New dependency `sha2`** (RustCrypto, pure Rust, widely used) for the id hash.
   Recommendation: yes. The alternative, a home-made non-cryptographic hash, saves one crate
   but goes against the `machine-id(5)` advice to hash the id properly before it leaves the
   machine.
2. **Tighten the daemon's `device_id` rule** to `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`.
   Recommendation: yes, now in beta. Our installers already enforce it; only hand-run
   reporters with spaces or other symbols in the id would be refused, and they get a clear
   reason in the daemon log.
3. **`--once` with no answer during `get.sh --reporter-only`**: warn and continue, or stop.
   Recommendation: warn and continue, because installing clients before the Pi is on is normal.
   An `id_in_use` answer always stops.
4. **Schedule UDP report authentication (T-006 HMAC) for v0.2.0**, since it is what closes
   reply spoofing and squatting. Recommendation: yes, list it in the v0.2.0 row of the vision
   plan next to the token store, so the tokens are reused for UDP.

## Amendments after review (2026-11-07, code-reviewer and security-reviewer)

These replace the matching parts above.

- **Run mode never exits on a conflict.** A forged or passing conflict must not become a
  permanent outage. After the grace window the loop prints one line
  (`... is still used by <ip>. Pausing reports for 300 s, then trying again. ...`), sends
  nothing for 300 s (`CONFLICT_BACKOFF`), then reports again, repeating while the conflict
  lasts, one log line per pause. `ConflictTracker::on_cycle` returns `Quiet`, `Warn`,
  `Silent` or `BackOff`; `after_back_off` restarts the grace window without a new warning.
  Exit 10 now comes only from `--once`.
- **systemd:** `RestartPreventExitStatus=10` is removed from `deskpane-reporter.service` and
  from the get.sh client unit. `RestartPreventExitStatus=11` stays on the daemon.
- **Reply draining:** at most 8 packets per cycle (`MAX_REPLIES_PER_CYCLE`), at most one
  conflict line and one warning line per cycle.
- **Exit 12 wording** adds "or another check from this machine in the last 10 s" (the
  per-source reply limit).
- **Windows:** `reg.exe` is run by absolute path, `%SystemRoot%\System32\reg.exe` (fallback
  `C:\Windows`), never found by search. A failed read with `ConnectionReset` (Windows
  WSAECONNRESET) counts as `Refused`, like `ConnectionRefused` on Linux. CI gets a
  `windows-check` job (clippy for `x86_64-pc-windows-gnu`).
- **get.sh `--reporter-only`:** the `--once` check runs before `reporter.env` and the unit are
  written, so a failed check leaves an existing setup untouched. Without `--device-id` or
  `--label`, the values already in `/etc/deskpane/reporter.env` are reused, so upgrading a
  beta.1 or beta.2 client keeps its id.
- **Limiter sizing:** a test pins `MAX_REPLY_SOURCES >= REPLY_EVERY (s) * REPLIES_PER_SECOND`,
  so the limiter never forgets a source still inside its window. One `reply_source(peer)`
  gives the canonical IP to both the ownership check and the limiter.

| Binary | Code | Meaning | systemd |
|---|---|---|---|
| `deskpane` | 2 | usage error, including "no machine id, pass --device-id" | restarts (fix the settings) |
| `deskpane` | 10 | `--once` only: device id in use | not used by units |
| `deskpane` | 12 | `--once` only: no answer | not used by units |
| `deskpane-daemon` | 11 | UDP port in use | `RestartPreventExitStatus=11` |
