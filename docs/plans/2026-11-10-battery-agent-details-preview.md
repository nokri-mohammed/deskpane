# Local preview, agent details, battery and replay

Date: 2026-11-10. Author: architect. Status: approved by the CEO with its defaults on 2026-11-10;
T-061 and T-064 built and in review (uncommitted), T-062, T-063 and T-060 not started.

Tickets: T-061 (local preview), T-062 (agent row line 3), T-063 (model and tokens per agent),
T-060 (sessions survive a Pi restart, design approved by the CEO), T-064 (PiSugar battery).
T-063 covers the "model, tokens" part of T-056 (agent details page); the rest of T-056 stays open.

## Order

| Step | Ticket | Why here |
|---|---|---|
| 1 | T-061 Local preview | Every later change can be seen on this PC before it goes to the Pi. |
| 2 | T-062 Row line 3 | Small, Python only, shows the new three-line layout in the preview. |
| 3 | T-063 Model and tokens | Adds the per-session state file on the PC that T-060 builds on. |
| 4 | T-060 Replay after a Pi restart | Reuses the T-063 state file; adds the replayer and the daemon rule. |
| 5 | T-064 PiSugar battery | Needs the hardware, a third-party service and a security-specialist review. |

Each step is its own branch and pull request: tdd-guide builds test first, code-reviewer checks
Rust against `rust-patterns`, security-reviewer checks steps 3 to 5 (new input from a file and a
local socket), security-specialist checks step 5 (new service on the Pi), doc-updater updates
`CONTRACT.md`, `INSTALL.md`, `CONTRIBUTING.md` and `CLAUDE.md` at the end of each step.

## Contract changes at a glance

All changes are optional fields. No version number changes: the agent packet section of
`CONTRACT.md` already says an unknown field is ignored, serde has no `deny_unknown_fields`
anywhere in `contract.rs`, and the dashboard reads every field with `.get`.

| Link | Field | Type | Rule | Step |
|---|---|---|---|---|
| agent packet | `model` | string, optional | 1 to 24 bytes, only `A-Z a-z 0-9`, space, `.` and `-` | T-063 |
| agent packet | `tokens` | integer, optional | 0 to 10^12 (`MAX_TOKENS`) | T-063 |
| agent packet | `since` | integer, optional | sender ms of the first hook of the session, `since <= t` | T-060 |
| agent packet | `state_t` | integer, optional | sender ms of the last state change, `since <= state_t <= t` | T-060 |
| agent packet | `replay` | bool, optional | `true` only on a replayed packet | T-060 |
| agent packet | `sent` | integer, optional | sender ms when a replay was sent; required when `replay`, `t <= sent`, `sent - since <= 7 days` | T-060 |
| report | `battery_pct` | number, optional | finite, 0 to 100 | T-064 |
| report | `charging` | bool, optional | | T-064 |
| socket session | `model` | string or null | always present, null when not sent | T-063 |
| socket session | `tokens` | integer or null | always present, null when not sent | T-063 |
| socket device | `battery_pct`, `charging` | as in the report | left out when the report has none | T-064 |
| socket device | `local` | bool | `true` when the report came from a loopback address (the Pi itself) | T-064 |

Compatibility:

- Old reporter, new daemon: no new fields, everything works as today (`model`, `tokens` null,
  no battery).
- New reporter, old daemon: the extra fields are ignored. A replay packet is read as a normal
  packet with an old `t`: for a known session the reorder guard keeps it (same `t` is stored)
  and it refreshes `last_seen`, so `quiet` never shows on an old daemon; for an unknown session
  it creates the row with `running_s` from zero. Harmless; `INSTALL.md` keeps "upgrade the Pi
  first".
- New daemon, old dashboard: the new keys are ignored.
- New dashboard, old daemon: `.get` returns None, nothing new is drawn.

The largest agent packet grows by about 130 bytes (24-byte model, four 13-digit numbers, the
`replay` flag), still far under the 1500-byte read limit. A `contract.rs` test checks the
largest valid packet stays under 1024 bytes.

---

## 1. T-061 Local preview of the Pi dashboard

### Problem

The only way to see a screen change today is to build the bundle, deploy it to the Pi and look
at the panel. There is no `bun dev` for the dashboard. `dashboard.py` always opens `/dev/fb1`
(`FrameBuffer`), reads taps from `/dev/input/event0` (`TouchWatcher`), checks `wlan0`
(`NetworkMonitor`, which would show the network problem icon on this PC) and needs a daemon on
the Unix socket.

### Design

One command runs the real `pi/dashboard.py` on this PC (Linux under WSL2; WSLg is present, so a
window works) against a mock data source.

**Display backend switch.** New `pi/display.py` with one function and three small classes, all
with the same interface as `FrameBuffer` (`surface_for_drawing()`, `flip()`, `close()`) plus
`poll_events()`:

| `DESKPANE_DISPLAY` | Class | What it does |
|---|---|---|
| `fb` (default) | `FrameBuffer` (today's class, unchanged) | Writes RGB565 to the panel, taps from evdev. |
| `window` | `WindowDisplay` | A pygame window of 480 x 320 times `DESKPANE_SCALE` (1 to 4, default 2). Mouse click, Space or Right arrow is a tap; `N` toggles the network problem screen; `S` saves a PNG; `Q` or Esc quits. |
| `png` | `PngDisplay` | Offscreen. Writes `latest.png` (temp file plus rename) every frame, and numbered frames when `--frames N` is given. |

Every backend draws on the same 16-bit surface with the panel's masks (`MASKS` from
`fb_display.py`), and the window scales it with nearest-neighbour, so the colours and pixels
match the panel, not a 32-bit approximation.

`dashboard.py` changes, kept small:

- `FB_PATH` detection and `import fb_display` move into `open_display()`, because
  `fb_display.py` sets `SDL_VIDEODRIVER=dummy` at import, which would stop the window.
- Taps come from a `PointerTaps` object with the same `taps` and `last_pos` as `TouchWatcher`,
  fed by `display.poll_events()` inside the existing event loop (which already drains
  `pygame.event.get()` for QUIT). Mouse positions are divided by the scale (pure function
  `scale_tap(pos, scale)` in `layout.py`).
- `NetworkMonitor` reads `NET_IFACE` (default `wlan0`); `NET_IFACE=none` means always ok. The
  preview sets `none`, and `N` or a scenario step forces the problem screen.

**Mock data source.** Two modes:

1. Default, `pi/mock_daemon.py` (stdlib only): a thread that listens on a Unix socket in
   `$XDG_RUNTIME_DIR` (or the cache dir) and writes one envelope line a second, exactly the
   shape `parse_socket_line` reads (`{"v":1,"views":{"stats":{"devices":[...]},"agents":
   {"sessions":[...]}}}`). Lines come from pure `pi/preview_scenarios.py`:
   `envelope_at(name, t)` returns the envelope for second `t` of a scenario, so durations tick
   and states change on a timeline. The mock already sends the new fields of this plan
   (`model`, `tokens`, `battery_pct`, `charging`, `local`), so the CEO can see T-062 to T-064
   before the daemon changes ship.
2. `--real`: builds and runs the real daemon (`cargo run -p daemon` with
   `SOCKET_PATH=<runtime dir>/deskpane-preview.sock`, `LISTEN_ADDR=127.0.0.1:18765`), one real
   reporter (`deskpane report --daemon 127.0.0.1:18765 --device-id preview-pc --label "Preview PC"`)
   and `deskpane agent test` in a loop. This is the end-to-end check for daemon changes
   (T-060, T-063).

Scenarios (all fake names, no real hosts):

| Name | What it shows |
|---|---|
| `busy` (default) | 3 devices, 6 sessions in every state across 2 machines, one turns `needs_you` at 20 s (focus jump), models and tokens. |
| `single` | 1 session: the big card. |
| `worst` | 32 devices, 16 sessions (4 agents pages), 32-byte project names, 64-byte labels, `999k tokens`, `quiet`, a ghost walking off, an unknown state. |
| `offline` | A device going offline at 10 s and a session with `silent_s` growing. |
| `battery` | The Pi (`local: true`) at 87 % charging, then 15 % not charging (red), then no battery fields. |
| `empty` | No devices, no sessions, then the daemon disconnects (socket closed). |

**Character packs.** The preview sets `CHARACTERS_DIR` to `/var/lib/deskpane/characters` when
it exists, else to the first folder under `assets/characters/` that loads with
`sprites.CharacterPack.load`, else to a missing folder, which gives the built-in icons. The pack
is only read in place: never copied, bundled or written anywhere. `--no-pack` forces the icons.
PNG frames can contain pack art, so they go to `${XDG_CACHE_HOME:-$HOME/.cache}/deskpane-preview/`,
outside the repo, and `scripts/check-no-pack-files.sh` keeps guarding `assets/characters/`.

**Headless and browser.** `--headless` uses the `png` backend. `--serve PORT` also starts
`http.server` bound to `127.0.0.1` only, serving the cache folder and a small `index.html` that
reloads `latest.png` every 500 ms, scaled 2x with `image-rendering: pixelated`. WSL2 forwards
`localhost`, so the Windows browser opens `http://localhost:8780`. `--tap-every S` taps on a
timer, so headless runs walk through the pages.

**Command.** `scripts/preview.sh`. It finds Python 3.11 or newer (`/usr/bin/python3` here; the
pyenv `python3` shim on this PC is 3.7 and is skipped), creates `.venv-preview/` once (in
`.gitignore`), installs `pygame==2.6.1` into it, and runs `pi/preview.py` with `PYTHONPATH=pi`,
so the dashboard's flat imports (`import charpack`) work unchanged.

```bash
scripts/preview.sh                                # window via WSLg, scenario busy, scale 2
scripts/preview.sh --scenario worst --scale 3     # any scenario, bigger window
scripts/preview.sh --headless --serve 8780        # no window; open http://localhost:8780
scripts/preview.sh --headless --frames 20 --tap-every 3   # 20 PNGs in ~/.cache/deskpane-preview/
scripts/preview.sh --real                         # real daemon + reporter + agent test on loopback
```

`display.py` ships in the Pi bundle (the dashboard imports it); `package-pi-bundle.sh` leaves
out `preview.py`, `mock_daemon.py` and `preview_scenarios.py`.

### Files touched

`pi/display.py` (new), `pi/preview.py` (new), `pi/mock_daemon.py` (new),
`pi/preview_scenarios.py` (new), `pi/dashboard.py` (backend switch, `PointerTaps`, `NET_IFACE`),
`pi/layout.py` (`scale_tap`, `pick_backend`), `scripts/preview.sh` (new),
`scripts/package-pi-bundle.sh` (exclude preview files), `.gitignore` (`/.venv-preview/`),
`pi/tests/test_preview_scenarios.py` (new), `pi/tests/fixtures/envelope_v1.json` (new),
`daemon/src/contract.rs` (fixture test), `CONTRIBUTING.md`, `CLAUDE.md`.

### Contract changes

None. The mock speaks envelope v1. To stop the mock from drifting from the real shape, a daemon
test serializes a full `Envelope` (one device, one session, every optional field set) and
compares it with `pi/tests/fixtures/envelope_v1.json`; a Python test checks every scenario line
has at least the keys of that fixture, with the same types.

### Tests

- `test_preview_scenarios.py` (stdlib, no pygame): every scenario at t = 0, 10, 60 s parses with
  `parse_socket_line`, matches the fixture keys, durations grow with `t`, `worst` has 16
  sessions and 32-byte projects, `battery` drops the fields at the end.
- `test_layout.py`: `pick_backend` (env values, unknown value falls back to `fb` with a log
  line), `scale_tap` (scale 1 to 4, clicks outside clamp to the screen).
- Rust: `envelope_fixture_matches_file` in `contract.rs`.
- Manual smoke (pygame needed, so not in CI): `scripts/preview.sh --headless --frames 3` exits 0
  and writes three 480 x 320 PNGs.

### Done when

The CEO runs `scripts/preview.sh`, sees the dashboard in a window, clicks through every page, and
the headless mode shows the same frames in the Windows browser. Tests pass, the bundle does not
contain the preview files, and no pack file or PNG lands in the repo.

---

## 2. T-062 Fix agent row line 3

### Problem

On the CEO's screen every agent shows line 1 the project, line 2 the state word, and line 3
`test`.

Root cause, found:

1. `draw_agent_card` (one session) draws title, state word, then `session_line2(session)`;
   `draw_agent_row` (2 to 5 sessions) draws `session_line2` after the project.
   `session_line2` is `agent_line2` in `layout.py`: the machine `label`, a dot, then `doing`.
   For `done` and `idle` the `doing` text is empty, so only the label is left.
2. The label comes from the daemon: `broadcast_statuses` in `daemon/src/main.rs` joins each
   session's `device_id` with the device table and copies that device's `report.label`.
3. On the CEO's WSL machine the reporter runs as the system service `deskpane-reporter.service`
   (`/usr/local/bin/deskpane report --daemon ... --device-id ... --label test`, checked with
   `ps`; the label value is `test`). The hooks send the same device id, so every session from
   that machine gets the label `test`.

Not the cause: `pi/layout.py` and `pi/dashboard.py` have no hardcoded `test`;
`deskpane agent test` uses the project `deskpane test` (line 1, and only for its own fake
session). I did not read `~/.claude/settings.json` (the tool refused because the file holds a
token, which is right); it is not needed, the hook args only carry `--daemon` and
`--device-id`, and the label never comes from the hook.

### Design

Two parts.

**a. Configuration (CEO, one minute).** Give the WSL reporter a real name: edit `LABEL=` in
`/etc/deskpane/reporter.env` (for example `LABEL=WSL`) and run
`sudo systemctl restart deskpane-reporter`, or rerun the get.sh line with `--label "WSL"`.

**b. Layout.** Every row and the card get a fixed meaning per line:

| Line | Row (2 to 5 sessions) | Card (1 session) |
|---|---|---|
| 1 | project, time in state on the right | project |
| 2 | `doing` (or `no news for 12m`), state word or NEEDS YOU badge on the right | state word, very large |
| 3 | details: `Opus 5.5 · 182k tokens · WSL` | `doing` |
| 4 | | details line, then `for 3m · running 2h` |

The details line is built by a new pure `agent_details(session, show_machine)` in `layout.py`:
model, tokens, machine, joined with `DOT` (U+00B7). Each part is left out when missing. The
machine name is shown only when the visible sessions come from two or more `device_id`s
(`show_machine = len(device_ids) > 1`), so a single PC no longer repeats its name on every row.
`fit_text` cuts from the end, so the machine name goes first when the line is too long.

Fonts: line 3 of a row uses `tiny` (16, about 11 px); a 5-row page has 51 px rows, and
18 + 2 + 14 + 2 + 11 = 47 px fits. `AGENT_LINE_GAP` drops from 3 to 2 only for three-line rows.
Characters used: letters, digits, `.`, `k`, `M`, space and `·`; the built-in
`freesansbold.ttf` has all of them (checked with `pygame.freetype`: U+00B7 and U+2026 have
glyphs; U+26A1 lightning and arrows do not, so nothing in this plan uses them).

Before T-063 ships, `model` and `tokens` are null, so line 3 shows only the machine (when two
or more machines) or nothing.

### Files touched

`pi/layout.py` (`agent_details`, `agent_line2` without the label, `fmt_tokens` lands in T-063),
`pi/dashboard.py` (`draw_agent_row` three lines, `draw_agent_card` order), `pi/preview_scenarios.py`
(already has the fields), `pi/tests/test_layout.py`.

### Contract changes

None.

### Tests

`agent_details`: all parts, each part missing, `show_machine` false, label empty falls back to
the device id, a ghost shows only the machine. `agent_line2`: no label any more, quiet text
unchanged. Row geometry: three lines fit in 51 px with the real font heights (heights passed in,
pure).

### Done when

The preview `busy` and `worst` scenarios show the three lines without overlap at 5 rows, the
CEO's screen shows no `test` after the relabel, and the single card shows the state word on
line 2 and `doing` on line 3.

---

## 3. T-063 Model and tokens per agent

### Problem

The screen does not say which model an agent runs or how much it has used. The CEO wants
"Opus 5.5 · 182k tokens" on the details line.

### What Claude Code gives a hook

Every hook gets `session_id`, `transcript_path`, `cwd`, `hook_event_name` (and
`permission_mode`). `SessionStart` also documents `source` and `model`. Nothing gives tokens.
`HookInput` today reads `session_id`, `hook_event_name`, `tool_name`, `notification_type`,
`source`, `error`, `background_tasks`, `cwd`, `agent_id`, and its comment says the transcript
path is skipped on purpose.

The transcript is a JSONL file, 0600, owned by the user. Checked on this PC (keys only, no
content printed): the biggest one is 118 MB and 30 000 lines. Lines with `"type":"assistant"`
carry `message.id`, `message.model` (for example `claude-opus-5-5`) and `message.usage` with
`input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`. One
message is written as up to 10 lines with the same `message.id` (one per content block), each
repeating the usage, so a plain sum counts a message many times. Many other line types exist
and some lines are huge (tool output, request blobs). The format is not a documented API.

### Design

**Reading usage, incrementally.** New service `reporter/src/services/transcript_usage.rs`:

```rust
/// Running totals over the assistant messages of one transcript.
pub struct UsageCursor {
    // bytes of the file already read, always at a line start
    pub offset: u64,
    // file length at the last read; a shorter file means it was rewritten
    pub seen_len: u64,
    // totals of the finished messages
    pub tokens: u64,
    // the message still being written: its id and its latest usage
    pub open_id: String,
    pub open_tokens: u64,
    // model of the latest assistant message, e.g. claude-opus-5-5
    pub model: String,
}

/*
 * reads complete lines from cursor.offset, at most max_bytes, and updates
 * the cursor. A line longer than MAX_LINE_BYTES is skipped unparsed.
 *
 * returns: Ok(true) when the end of the file was reached
 */
pub fn advance(cursor: &mut UsageCursor, file: &mut File, max_bytes: u64) -> Result<bool, String>
```

- Seek to `offset`, read at most `max_bytes`, stop at the last newline, so `offset` is always a
  line start.
- A line is parsed only when it contains `"type":"assistant"` and is at most
  `MAX_LINE_BYTES` (1 MiB); it is parsed into a minimal serde struct (`type`, `message.id`,
  `message.model`, the four usage numbers), so no text is kept.
- Same `message.id` as `open_id`: replace `open_tokens` (the last line of a message has the
  final usage). New id: add `open_tokens` to `tokens`, open the new one.
- Model `<synthetic>` (local error messages) and empty models are skipped.
- A file shorter than `seen_len` (rewritten) resets the cursor to zero.
- Shown tokens = `tokens + open_tokens`, saturating, capped at `MAX_TOKENS`.

Token count = `input_tokens + output_tokens + cache_creation_input_tokens`. Cache reads are left
out by default: they repeat the whole context on every call and reach tens of millions in a
long session, which says nothing useful (open question 1).

**Budget inside the hook.** The hook has 500 ms in total. It reads at most 256 KiB of new
transcript per call (`HOOK_TRANSCRIPT_BUDGET`), a few milliseconds. A normal turn writes far
less than that, so the cursor keeps up. The first hook of a session that started before the
hooks were installed may face a 100 MB file: it reads its 256 KiB and leaves the rest to the
replayer (T-060), which has no deadline and reads in 1 MiB steps with constant memory. Until
the cursor reaches the end, `tokens` is not sent (the screen shows only the model), so a wrong
low number is never shown. Memory stays at one buffer of 256 KiB in the hook and 1 MiB in the
replayer, besides the existing 8 MiB stdin cap.

Usage is read on `UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure`, `Stop`,
`SubagentStop` and `SessionStart` (the events after which the transcript has grown); not on
`PreToolUse` and `PermissionRequest`, which must stay fastest.

**Where the cursor lives: the session state file.** New service
`reporter/src/services/session_state.rs`, shared with T-060. One file per session, so parallel
sessions never write the same file:

| OS | Folder |
|---|---|
| Linux, WSL | `$XDG_STATE_HOME/deskpane/agents/`, else `$HOME/.local/state/deskpane/agents/` |
| Windows | `%LOCALAPPDATA%\deskpane\agents\` |

- Folder 0700, files 0600 (Unix); on Windows the user's `LocalAppData` is private to the user by
  default. Written with the existing `write_atomically` (temp file plus rename).
- File name `<sid>.json` (the 12-hex hash, never the raw session id). It holds: the
  `UsageCursor`, `transcript_path` (local only, never sent: it contains the user name), the
  pretty model, `since`, `state_t`, and for T-060 the latest packet per key.
- At most 4 KiB per file (`read_capped`), at most 16 files (the oldest by `t` is removed when a
  17th is written).
- Concurrent hooks of one session (parallel tools, subagents) take a non-blocking lock on
  `<sid>.lock` (`File::try_lock`, stable since Rust 1.89; the toolchain is 1.99.1). A hook that
  does not get the lock skips the state update and sends its packet with the values it read; the
  next hook catches up. No hook ever waits for a lock.

**Model name.** Pure `pretty_model(id)` in `agent.rs`: strip `claude-`, drop a trailing 8-digit
date and a `[1m]` style suffix, capitalise the family, join the version numbers with `.`:
`claude-opus-5-5` gives `Opus 5.5`, `claude-sonnet-5-5` gives `Sonnet 5.5`,
`claude-haiku-4-5-20251001` gives `Haiku 4.5`. Anything that does not match the pattern is
cleaned to the allowed characters and cut to 24 bytes; an empty result sends no model. The
model comes from the transcript (it follows `/model` switches); `SessionStart`'s `model` field
is used until the transcript has an assistant message.

**Never sent:** prompt text, tool input or output, file contents, the transcript path, the raw
session id. Only `model` (24 bytes, restricted characters) and `tokens` (a number).

**Daemon.** `AgentPacket` gets `model: Option<String>` and `tokens: Option<u64>`
(`#[serde(default, skip_serializing_if = "Option::is_none")]`), `validate_agent` checks them
(new `MAX_MODEL_BYTES = 24`, `MAX_TOKENS = 1_000_000_000_000`, a `check_model_chars` that names
the field and never repeats the value). The table keeps the last non-null value per session, so
a packet without them (a hook that skipped the read) does not blank the line. `AgentSession`
gets `model` and `tokens`, always present.

**Screen.** `fmt_tokens(n)` in `layout.py`: under 1000 as is (`640 tokens`), under a million
whole thousands (`182k tokens`), under a billion one decimal (`1.2M tokens`), then `1.0B`.
Details line:
`Opus 5.5 · 182k tokens`. No `tokens` (null, or the cursor still catching up): the model alone.

### Files touched

`reporter/src/services/transcript_usage.rs` (new), `reporter/src/services/session_state.rs`
(new), `reporter/src/services/mod.rs`, `reporter/src/agent.rs` (`HookInput.transcript_path`,
`HookInput.model`, `pretty_model`, `build_agent_packet` with model and tokens),
`reporter/src/main.rs` (`run_hook`: state file, budgeted read), `daemon/src/contract.rs`,
`daemon/src/agent_table.rs` (keep last model and tokens), `pi/layout.py` (`fmt_tokens`,
`agent_details`), `reporter/tests/agent_hook.rs`, `docs/CONTRACT.md`.

### Contract changes

Agent packet `model`, `tokens`; socket session `model`, `tokens` (table at the top). Agent
packet `v` stays 1.

### Tests

- `transcript_usage.rs`: one message in 10 lines counts once; two messages add up; a partial
  last line is not read until its newline arrives; a 2 MiB line is skipped; a non-assistant
  line is skipped; `<synthetic>` is skipped; a shorter file resets; the budget stops mid-file
  and the next call goes on; overflow saturates; junk JSON lines are skipped.
- `session_state.rs`: 0600 and 0700 on Unix, atomic write, 4 KiB cap, 16-file cap removes the
  oldest, a held lock makes the second writer skip, a corrupt file is replaced.
- `agent.rs`: `pretty_model` table (above names, dates, suffixes, junk, 24-byte cut).
- `contract.rs`: model too long, bad characters, format characters, tokens over the cap,
  missing fields fine, largest packet under 1024 bytes.
- `agent_table.rs`: a packet without model keeps the stored one.
- `agent_hook.rs` (real binary): a temp transcript of 50 MB with the hook under 500 ms, stdout
  empty, exit 0; the packet holds model and tokens but no text from the transcript (search the
  sent bytes for a marker string planted in a tool output line).
- `test_layout.py`: `fmt_tokens` edges (0, 999, 1000, 999 999, 1 000 000, 10^12).

### Done when

In the preview with `--real`, a real Claude Code session on this PC shows `Opus 5.5 · Nk tokens`
on its row, the number matches a hand count of the transcript (deduplicated by message id)
within one message, and the hook still finishes under 500 ms with a 100 MB transcript.

---

## 4. T-060 Agent sessions survive a Pi restart

### Problem

Hooks send one UDP packet per event and the daemon keeps sessions in RAM. After the Pi is off
and on again, a session only comes back on its next event. An idle or `done` session, or one
waiting for the CEO, may not send anything for hours, so it is missing exactly when it matters.

### What runs on the CEO's PC

The WSL reporter runs as the system unit `deskpane-reporter.service` with `DynamicUser=yes`
(checked: the process runs in `system.slice/deskpane-reporter.service` as a `deskpan+` dynamic
user). That user cannot read the CEO's home folder, and on Windows the reporter is a Task
Scheduler task or a console window. So the replay cannot live in the `deskpane report` loop. It
needs a small per-user process.

### Design

**State file.** Every hook writes its session's latest state into the T-063 file before it
sends: the packet per key (`main` plus up to 8 subagent keys, the same cap as
`MAX_PENDING_PER_SESSION`), `since` (sender ms of the first hook it saw for this session) and
`state_t` (sender ms of the last state change). `SessionEnd` deletes the file and its lock.
Files whose newest `t` is older than 4 hours (`AGENT_FORGET_AFTER`, same as the daemon) are
expired by the replayer and by any `SessionStart` hook.

**The replayer: `deskpane agent replay --daemon ADDR`.** One per user:

- Every 12 s it reads the state folder and sends each live session's packets again, oldest key
  first, with `"replay": true` and `"sent": <now ms>`, at most 32 packets per round.
- It also advances the T-063 usage cursors that are behind (1 MiB per step, no deadline).
- Single instance: an exclusive lock on `replay.lock` in the state folder. A second one exits 0
  at once.
- It exits when the folder has had no live sessions for 10 minutes.

**Who starts it (default: on demand).** Each hook, after it sends, checks `replay.lock` with
`try_lock`; when the lock is free and the session is live, it spawns `deskpane agent replay`
with the same `--daemon` and exits. The spawn is detached: stdin, stdout and stderr null; on
Unix `process_group(0)` so it survives the hook's process group; on Windows the
`DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`
creation flags (a job that forbids breakaway makes the spawn fail; the hook then just goes on,
see the fallback). Spawning takes a few milliseconds, well inside the 500 ms deadline. Nothing
to install, no service, no new open port: it only sends UDP to the daemon like the hooks do.

Fallback when spawning is not possible (Windows job without breakaway, a locked-down machine):
`deskpane agent install-hooks --replay-service` adds a systemd user unit
(`~/.config/systemd/user/deskpane-replay.service`) on Linux, or prints the Task Scheduler steps
on Windows (same pattern as the reporter in `INSTALL.md`). Open question 3.

**Daemon rule: replays are idempotent.** `AgentEntry` gets `restored: bool`. In
`insert_agent`:

| Packet | Session in table | Result |
|---|---|---|
| live | any | today's rules; `restored = false`; on create, `first_seen = now - (t - since)` and `state_since = now - (t - state_t)` when those fields are present |
| replay | known, `restored == false` | `ReplayIgnored`: nothing changes, nothing is logged. The daemon already has fresher news from live packets. |
| replay | known, `restored == true` | today's rules per key (this rebuilds the pending `needs_you` keys of subagents), but `last_seen = now - (sent - t)`, not `now` |
| replay | unknown | create like a new session, except: no eviction (a full table returns `TableFull`, so replays never push out a live row); the recently-ended guard applies; `first_seen = now - (sent - since)`, `state_since = now - (sent - state_t)`, `last_seen = now - (sent - t)`, `restored = true` |

All ages are differences of the sender's own clock (`sent - since`), so a clock offset between
the PC and the Pi does not matter. Each age is clamped to `MAX_CARRIED_AGE` (7 days) and
`now.checked_sub(age)` falling back to `now` (an `Instant` cannot go before boot). So after a
Pi restart a session reappears within 12 s with the right `running_s`, `state_s` and
`silent_s`, `quiet` still works (replays never refresh `last_seen` to now), and the 4-hour
forget still fires because `last_seen` follows the last real event.

No flicker: `first_seen` is set once, on create, and ignored replays change nothing, so
`running_s` never jumps. The reorder guard never refuses a replay of the newest packet, because
equal `t` is stored (`packet_with_the_same_time_is_stored`); an older replay key for a restored
entry is refused as today, which is right.

Validation in `validate_agent`: `since <= state_t <= t`, `replay` requires `sent`, `t <= sent`,
`sent - since <= 7 days`. Each reason names the field only.

### Files touched

`reporter/src/main.rs` (`AgentAction::Replay`, `run_replay`, spawn after send, state write in
`run_hook`), `reporter/src/agent.rs` (`build_agent_packet` with `since`, `state_t`; pure
`replay_packets(state, now)`), `reporter/src/services/session_state.rs` (per-key packets,
expiry, list live sessions), new `reporter/src/services/detached.rs` (spawn per OS, tested with a
harmless child), `daemon/src/contract.rs` (fields, validation), `daemon/src/agent_table.rs`
(`restored`, replay rules, carried ages), `daemon/src/main.rs` (`ReplayIgnored` not logged),
`reporter/tests/agent_hook.rs`, `docs/CONTRACT.md`, `docs/INSTALL.md` (Linux, WSL, Windows
notes, state folder, how to stop it: `deskpane agent replay --stop`).

### Contract changes

Agent packet `since`, `state_t`, `replay`, `sent` (table at the top). No socket change.

### Tests

- `agent_table.rs`: replay creates with carried `first_seen`, `state_since`, `last_seen`; a
  replay for a live-known session changes nothing (whole entry equal before and after); a
  replay never evicts; replays after a takeover window obey ownership; a live packet clears
  `restored`; a restored working session turns `quiet` after 10 min of no real events, with
  replays arriving every 12 s; the forget window counts from the last real event; ages over 7
  days are clamped; `checked_sub` fallback (age larger than the time since a fake start);
  pending subagent keys rebuilt from two replayed keys.
- `contract.rs`: every new rule rejects and names the field.
- `session_state.rs`: per-key cap, `SessionEnd` removes the files, expiry at 4 h.
- `agent.rs`: `replay_packets` order and cap, `sent` set, no expired session.
- Integration (`reporter/tests/agent_hook.rs` plus a daemon on loopback in the test): start a
  session with hooks, kill the daemon, start a new daemon, the session is back within one
  replay round with `running_s` within 1 s of the truth; a second replayer exits at once.
- Windows: the `windows-check` CI job builds and clippies `detached.rs`; the spawn itself is a
  manual check on the CEO's Windows PC (T-047 style).

### Done when

With the preview in `--real` mode: three sessions (idle, done, needs you) on screen, restart the
daemon, all three are back within 15 s with unchanged `running_s`, and nothing flickers for a
minute. Then the same on the real Pi with a power cycle, from WSL and from Windows.

---

## 5. T-064 PiSugar battery on the display

### Problem

The CEO added a PiSugar battery to the Pi. The screen does not show the charge, so the Pi can
run flat without warning.

### How PiSugar is read

From the public README of `PiSugar/pisugar-power-manager-rs`: `pisugar-server` (runs as root,
it owns the I2C bus) answers text commands, one per line, on a Unix socket
(`/tmp/pisugar-server.sock`) and on TCP `0.0.0.0:8423`, plus a WebSocket on `0.0.0.0:8422` and a
web UI on `0.0.0.0:8421` with HTTP digest auth that defaults to `admin`/`admin`.

| Command | Reply | Models |
|---|---|---|
| `get model` | `model: PiSugar 3` | all |
| `get battery` | `battery: 87.53` (percent) | all |
| `get battery_charging` | `battery_charging: true` | PiSugar 2 family; deprecated on newer models |
| `get battery_power_plugged` | `battery_power_plugged: true` | newer models (PiSugar 3) only |

PiSugar 2 and 2 Pro (IP5209 / IP5312 power chips) estimate the percent from the voltage, so it
jumps; PiSugar 3 has its own microcontroller and reports a steadier value and the plug state.
The reader asks `get model` once, then uses `battery_power_plugged` for a model name containing
`3` and `battery_charging` otherwise. To be confirmed on the CEO's board during the build
(which model it is).

### Who reads it: the Pi's own reporter

The Pi's reporter (`deskpane-reporter.service`, `--daemon 127.0.0.1:8765`) gets
`--battery pisugar` (from `BATTERY=pisugar` in `/etc/deskpane/env`). Each report cycle it asks
the socket, with a 200 ms connect-and-read timeout and replies capped at 128 bytes, and adds
`battery_pct` and `charging` to its `Report`. No answer, a parse error or no flag: the fields are
left out.

Why the reporter and not the dashboard: the dashboard keeps one data source (the daemon socket)
and stays free of hardware calls; the same optional fields later serve laptops (a `--battery
system` reader); the daemon validates the numbers like every other report field. The dashboard
needs to know which device is the Pi itself: the daemon adds `local: true` to a
`DeviceStatus` whose owner IP is loopback (it already stores the owner IP in `DeviceEntry`).

Generic part in `reporter/src/services/pisugar.rs`: `PiSugarClient::query(command) ->
Option<String>` over `UnixStream` with timeouts; pure `parse_reply(name, line) -> Option<String>`,
`parse_percent`, `parse_bool`. Unix only (`#[cfg(unix)]`); on Windows the flag prints a usage
error.

### No PiSugar

No `--battery` flag, no socket, or `pisugar-server` stopped: the fields are absent, nothing is
drawn, and the reporter logs one line at start and one per state change (not every cycle).

### How it is drawn

New `draw_battery(surface, font, pos, pct, charging)` in `dashboard.py`, colours and text from a
pure `battery_style(pct, charging)` in `layout.py`:

- An outline 22 x 11 px with a 2 x 5 px nub, filled in proportion to the percent, then the
  percent in `tiny` (`87%`).
- Charging: a small bolt drawn as a 6-point polygon inside the outline (the font has no
  lightning glyph). Not charging: no mark.
- Under 20 %: fill and text in `CRIT` red; otherwise `TEXT`. Colour is never the only signal:
  the fill level and the number say the same.
- Overview: top right, 8 px in (bottom left has the version, bottom centre the page dots,
  bottom right the network icon). Pi's device page: left of the clock at the top right.

### Security and permissions (for security-specialist)

- `pisugar-server` defaults open three LAN ports, one a web UI with default credentials. The
  plan requires turning all three off and keeping only the Unix socket. Its options live in
  `/etc/default/pisugar-server`; the exact flags must be confirmed against the installed
  version by security-specialist. Defence in depth: the existing firewall drops 8421 to 8423 on
  `wlan0`.
- The socket moves from `/tmp` to `/run/pisugar/pisugar-server.sock` (a systemd drop-in with
  `RuntimeDirectory=pisugar`), mode 0660, group `deskpane`, because the reporter unit has
  `PrivateTmp=yes` and cannot see `/tmp` of other units. The reporter unit gets
  `BindReadOnlyPaths=` only if needed; `RestrictAddressFamilies` already allows `AF_UNIX`.
- deskpane does not install or ship `pisugar-server` (third party, its own installer). The CEO
  installs it from PiSugar's docs; `install.sh --battery pisugar` only checks the socket exists,
  writes `BATTERY=pisugar` and the drop-in, and security-specialist reviews those lines.
- No new open port from deskpane. The battery numbers enter the daemon through the existing
  report validation.

### Files touched

`reporter/src/services/pisugar.rs` (new), `reporter/src/main.rs` (`--battery`, `build_report`),
`daemon/src/contract.rs` (`Report.battery_pct`, `Report.charging`, `DeviceStatus.local`,
`validate`), `daemon/src/main.rs` (`local` from the owner IP), `pi/layout.py`
(`battery_style`, `local_battery(devices)`), `pi/dashboard.py` (`draw_battery`, overview,
device page), `deploy/systemd/deskpane-reporter.service` (`BATTERY` from the env file, see point 20),
`deploy/env.example`, `deploy/install.sh`, `docs/CONTRACT.md`, `docs/INSTALL.md`,
`docs/HARDWARE.md`.

### Contract changes

Report `battery_pct`, `charging`; socket device `battery_pct`, `charging`, `local` (table at the
top). Reports keep having no version field; old daemons ignore the new fields.

### Tests

- `pisugar.rs`: parse replies of both models, junk, missing colon, `NaN`, out of range,
  over-long reply; a fake server on a temp socket (`/tmp/phd-test-{pid}-pisugar.sock`) for
  timeout, closed socket and a slow server.
- `contract.rs`: battery bounds, `NaN` rejected, missing fields fine.
- `main.rs` (daemon): loopback owner gives `local: true`, a LAN owner `false`.
- `test_layout.py`: `battery_style` at 0, 19.9, 20, 100, charging or not; `local_battery` with
  none, one and two local devices.
- Preview scenario `battery` for the drawing.

### Done when

The preview shows the icon on the overview and the Pi page in all three states, the real Pi
shows the PiSugar level within 2 % of `pisugar-server`'s web UI before it is turned off, nothing
is drawn with the PiSugar unplugged, and security-specialist confirms ports 8421 to 8423 are
closed (`ss -ltnp` on the Pi).

---

## Open questions for the CEO

| # | Question | Recommended default |
|---|---|---|
| 1 | Which tokens count? | Input + output + cache writes. Leave cache reads out (they show tens of millions in a long session). |
| 2 | Show the machine name on line 3? | Only when sessions come from 2 or more machines. And rename the WSL reporter's label from `test` (for example `WSL`). |
| 3 | How is the replayer started? | Spawned on demand by the hooks, exits after 10 idle minutes. Fallback: a user service or a Task Scheduler task. |
| 4 | Subagent tokens in the total? | Not in this round: main transcript only. Add them later from `SubagentStop`'s transcript path. |
| 5 | Who reads the battery? | The Pi's reporter, through `pisugar-server`'s Unix socket only, with its three network ports turned off. |
| 6 | Low battery colour | Red under 20 %, no amber step. |
| 7 | Battery on the agents page header too? | No: overview and the Pi page only. |
| 8 | Preview data source | The Python mock by default; `--real` for daemon changes. |

### Open points from building T-061 (2026-11-10, tdd-guide)

| # | Point | What was done, recommended default |
|---|---|---|
| 9 | Scenario names | The CEO asked for `normal`; it is an alias of `busy`. A `needs-you` scenario was added next to the plan's six. |
| 10 | Network problem from a scenario step | Not built: only the N key forces it. Add a timeline step only if a scenario needs it. |
| 11 | Browser page taps | The page at `--serve` is view only. A `POST /tap` route could turn clicks into taps later; left out to keep the server read only. |
| 12 | `install.sh` touched | One preflight line now also requires `dashboard/display.py` (the dashboard imports it). security-specialist owns the file: a glance before commit. |
| 13 | Worst case on the real dashboard | Seen in the preview, logged as T-065: page dots overflow with 37 pages, a 64-byte hostname runs under the clock, emoji draw as a box. Not fixed here (dashboard change, not preview). |
| 14 | `--real` shows this PC | The reporter sends this PC's real hostname and address to the loopback daemon; frames stay in the cache folder, never in the repo. |
| 15 | shellcheck | Not installed on this PC, so `scripts/preview.sh` was not shellchecked locally; CI runs it. |
| 16 | Window click-through | The window was opened and ran without errors (scale 2 and 3), but clicks were not driven by a test: the CEO's first run is the check. |

### Open points from building T-064 (2026-11-10, tdd-guide)

Built with the security specialist's PiSugar review of 2026-11-10 applied; these differ from
section 5 above, and the code follows the points below.

| # | Point | What was done, recommended default |
|---|---|---|
| 17 | Socket path | Default `/run/pisugar/pisugar-server.sock`, `PISUGAR_SOCKET` overrides (absolute paths only). pisugar-server's own `/tmp` default is not used: deskpane units have `PrivateTmp=yes`. |
| 18 | Fixed commands only | The client cannot send any other command: `Query` has three variants mapped to constant lines (`get battery`, `get battery_power_plugged`, `get battery_charging`), and a test checks the list and the bytes on the wire. `get model` from section 5 was dropped: the reporter tries `battery_power_plugged` first, falls back to `battery_charging` and remembers the one that answered. |
| 19 | Percent range | Finite values up to 5 points outside 0 to 100 are clamped (PiSugar 2 estimates overshoot); further out, `NaN` or text is no reading. The daemon still rejects anything outside 0 to 100. |
| 20 | Reporter unit (security-specialist to review) | `SupplementaryGroups=pisugar` on `deskpane-reporter.service` only. `BATTERY` reaches the reporter as an environment variable from `/etc/deskpane/env` (clap `env = "BATTERY"`; `--battery` stays for manual use and wins over the variable); the unit no longer passes `--battery ${BATTERY}`, because after a rollback (`ln -sfn releases/<old>`) an older binary keeps the new unit and would reject the flag and crash-loop (review MEDIUM 1, 2026-11-10). An empty `BATTERY` or a `--battery ""` from the first T-064 unit still counts as `none`. The sandbox needs nothing else: `/run` is outside `PrivateTmp`, `ProtectSystem=strict` makes `/run` read-only but `connect()` to a socket inode is not a write to the file system, `RestrictAddressFamilies` has `AF_UNIX`, and there is no `InaccessiblePaths`. To confirm on the Pi with `systemd-analyze security deskpane-reporter` and the reporter log line. |
| 21 | install.sh (security-specialist to review) | Creates the system group `pisugar` with `groupadd --system` when `getent group pisugar` fails, only when the reporter is installed (systemd refuses a unit whose `SupplementaryGroups` group does not exist). Fake-root tests in `pi/tests/test_deploy_characters.py`. `install.sh --battery pisugar` from section 5 was not built: `BATTERY` is set in `/etc/deskpane/env` by hand, after the INSTALL.md steps. |
| 22 | pisugar-server setup | Not done by deskpane. `docs/INSTALL.md` "PiSugar battery" has the pinned route, still marked pending the security setup and a real Pi check. See point 26 for what was checked in the package. |
| 23 | Bolt position | The plan put the bolt inside the 22 x 11 outline; at that size its tips fell into the 1 px gap and it did not read. It is drawn as a 7 x 11 amber polygon left of the percent instead (`bolt_points`). |
| 24 | Battery on other device pages | `battery_fields(device)` draws the icon on any device page whose report has the fields (today only the Pi sends them; later laptops could). The overview shows only the one online `local: true` device, so a LAN host cannot put a battery on the overview. Loopback sources from the network are dropped by Linux (martian packets), so `local` cannot be forged from the LAN. |
| 25 | `--real` preview | With `PISUGAR_SOCKET` set, `scripts/preview.sh --real` also passes `--battery pisugar` to its reporter, for a loopback check against a fake socket. |
| 26 | pisugar-server 2.4.1 facts (review MEDIUM 2, 2026-11-10) | Checked in the release `pisugar-server_2.4.1-1_arm64.deb` (SHA-256 `9d5e70a7...76d4a3` matches) by unpacking it with `dpkg-deb -e` / `-x`, nothing run, and in the 2.4.1 source (`pisugar-server/src/main.rs`, `pisugar-core/src/model.rs`). Debconf questions (`templates`, asked by `config`): `pisugar-server/model` select, choices exactly `PiSugar 3`, `PiSugar 2 (2-LEDs)`, `PiSugar 2 Pro`, `PiSugar 2 (4-LEDs)`, default `PiSugar 3`; `pisugar-server/address` select `0.0.0.0` or `127.0.0.1`, default `0.0.0.0`; `pisugar-server/web`, `/tcp`, `/uds` booleans, default true; also `auth-username`, `auth-password` (and an unused `led`). `postinst` builds `OPTS="--config config.json --model '<model>'"` plus `--web /usr/share/pisugar-server/web --http ADDR:8421 --ws ADDR:8422`, `--tcp ADDR:8423`, `--uds /tmp/pisugar-server.sock` for each true answer, writes it to `/etc/default/pisugar-server` (a conffile), sets auth in `config.json` with `jq` only when a password was given, runs `raspi-config nonint do_i2c 0`, and enables and starts the unit only when it `was-enabled` before, so a first install leaves it disabled (INSTALL.md now runs `systemctl enable`). Packaged unit `/lib/systemd/system/pisugar-server.service`: `EnvironmentFile=/etc/default/pisugar-server`, `ExecStart=/usr/bin/pisugar-server $OPTS`, `ExecStopPost=/bin/rm -f /tmp/pisugar-server.sock`, `WorkingDirectory=/etc/pisugar-server`, `Restart=on-failure`, runs as root, no sandbox. `--config` is opened as given (`PathBuf::from`), so `config.json` resolves against the working directory; INSTALL.md now passes the absolute `/etc/pisugar-server/config.json`. CLI (clap derive): `--config/-c`, `--tcp/-t`, `--uds/-u`, `--uds-mode` (octal text, default `666`, so `660` must be passed), `--ws/-w`, `--web` (requires `--http`), `--http`, `--debug/-d`, `--syslog/-s`, `--model` (required, one of the four names), `--strict`. The UDS server binds without removing an old file (a stale socket makes it log `UDS bind error` and run without one; `RuntimeDirectory` avoids that) and serves one connection at a time (deskpane opens one per query and closes it). I2C: PiSugar 3 at `0x57`, PiSugar 2 family `0x75` (battery) and `0x32` (RTC). The old INSTALL.md `--model "PiSugar 2"` was not a valid name and is fixed. Still to confirm on the Pi: systemd keeps `--model "PiSugar 3"` as one argument, the socket is `srw-rw---- root pisugar`, `ss` shows no port. |
| 27 | Review fixes (2026-11-10) | One 300 ms budget per battery cycle (`PiSugarClient::deadline`, every query takes it; nothing is sent once it has passed), tested against a fake server that never answers. `connect()` keeps no timeout (LOW 4, comment in `pisugar.rs`). The preview mock no longer sends `model` and `tokens` (T-063 adds them to the contract first); the golden fixture now sets `battery_pct` and `charging`, and a test refuses any mock key the fixture does not have. The preview frame server answers 403 unless `Host` is `localhost:PORT` or `127.0.0.1:PORT`. `RealStack` spawns agent tests and takes its child list for `stop()` under one lock. The device page hostname is cut with `fit_text` to end 8 px (`HOST_CLEAR_GAP`) left of the battery, or of the clock without one. |

## Risks

1. **Transcript format is not a public API.** A Claude Code update can rename fields; the reader
   then finds no usage and sends no `tokens` (the screen shows the model only), never junk.
   One parser test per observed shape; T-047-style check after each Claude Code update.
2. **Huge transcripts.** A 100 MB file read in the hook would break the deadline; the 256 KiB
   budget and the replayer catch-up prevent that. `tokens` stays hidden until caught up.
3. **Windows detached spawn.** If Claude Code's job object forbids breakaway, the replayer dies
   with the hook; the fallback is the Task Scheduler task (question 3).
4. **pisugar-server exposure.** Its defaults open a web UI with default credentials on the LAN.
   It must be locked to the Unix socket before the PiSugar is used with deskpane.
5. **Instant arithmetic after boot.** Carried ages larger than the uptime cannot be subtracted;
   `checked_sub` falls back to `now`, so `running_s` is then too short, never a panic.
6. **Preview fidelity.** pygame 2.6.1 here versus 2.1.x on the Pi: freetype rendering may
   differ by a pixel; colours match because the 16-bit surface is shared. The real panel stays
   the final check (T-047).
7. **Pack art leak.** Headless PNGs can contain the licensed pack; they go to the cache folder
   outside the repo, never to `docs/media/`.
8. **Mock drift.** The golden envelope fixture written by a daemon test keeps the mock honest.

## Tasks

- T-061 Local preview (`scripts/preview.sh`, display backends, mock daemon, scenarios).
- T-062 Agent row line 3 (layout, details line, CEO relabels the WSL reporter).
- T-063 Model and tokens per agent (transcript cursor, session state file, contract fields).
- T-060 Sessions survive a Pi restart (replayer, idempotent replay rule, carried start times).
- T-064 PiSugar battery (reporter reader, `local` flag, icon, pisugar-server lock-down).
