# Agent status view (idea 1)

Date: 2026-11-08. Author: architect. Status: approved 2026-11-08, building. tdd-guide builds
it test first.

Tickets: T-038 to T-047 and T-050 to T-053 for the first beta, T-048 and T-049 for later. See
[Tasks](#tasks).

Changes on 2026-11-08 after approval:

- The CEO approved all six decisions in section 9 as recommended.
- The CEO approved character packs (section 10): an optional pixel-art character per project,
  with emotes per state, falling back to the built-in icons when no pack is installed.
- New state `error`, sent on `StopFailure` (it was `needs_you` before).
- T-038 spike results added (section 1). T-038 is partly done; the rest moved to T-047.

## Why

The CEO runs several Claude Code sessions on different computers. Today the only way to know
which one is waiting for a permission click, or has finished, is to walk to each screen. The Pi
already sits on the desk and already receives small UDP packets from every machine. It can show
one more thing: a row per agent session with its project, what it is doing and one of the
states the CEO cares about: **working**, **done**, **needs you** or **error**.

Goals:

1. Status arrives on its own, from Claude Code hooks. The agent does not know the Pi exists and
   no prompt or skill has to mention it.
2. "Needs you" is impossible to miss on the screen.
3. A hook never slows an agent down in a way you notice, and never breaks a session, even when
   the Pi is off.
4. Nothing private leaves the machine: no prompt text, no file contents, no commands.
5. The design fits the existing contract, validation style and security model, and leaves room
   for the MCP task list (idea 3) and authentication (v0.2.0) without rework.

## Summary of the design

| Item | Decision |
|---|---|
| Source of status | Claude Code command hooks run `deskpane agent`, which reads the hook JSON on stdin and sends one UDP packet. |
| Transport | The same UDP port 8765, a new packet with `"kind": "agent"`. Reports keep their shape (no `kind` means a report). |
| States on the wire | `idle`, `working`, `done`, `needs_you`, `error`, `ended`. The client maps hook events to states. |
| Session key | `sid` = first 12 hex chars of SHA-256 over a fixed prefix and Claude Code's `session_id`. The raw id never leaves the machine. |
| Sent by default | `sid`, the machine's `device_id`, project (basename of the project folder), state, a short "doing" text built from the tool name only, sender time. |
| Never sent | Prompt, tool input or output, file paths, commands, notification text, assistant messages, transcript path. |
| Client rules | Exit 0 always, print nothing on stdout, internal deadline 500 ms, hook `timeout` 2 s, exec form, `onFailure: "continue"`. |
| Setup | `deskpane agent hooks-snippet` prints the JSON to paste. `deskpane agent install-hooks` (opt-in) merges it into `~/.claude/settings.json` with a backup, never overwriting other entries. |
| Daemon | New pure `agent_table.rs`: at most 16 sessions, at most 8 per source IP, owned by source IP, reorder guard on sender time, "quiet" after 10 min of silence while working, forgotten after 4 h of silence or at once on `ended`. |
| Replies | None. Agent packets never get a reply, so the amplification rules from beta.3 are untouched. |
| Dashboard link | The socket line becomes the envelope from the vision plan: `{"v":1,"views":{"stats":{...},"agents":{...}}}`. |
| Screen | An agents page (one big card for 1 session, rows for 2 to 5, more pages above 5), amber "needs you" rows, a one-line count on the overview. Pure helpers in `pi/layout.py` with unit tests. |
| Characters | Optional paid or free pixel-art pack, installed with `--characters <zip>` into `/var/lib/deskpane/characters/`, never in git or a bundle. One look per project, an emote per state. Without a pack, the built-in icons. Section 10. |
| Context usage | Hooks cannot see it (verified). Only the status line command receives `context_window.used_percentage`. Planned as a later opt-in (T-048), not in the first beta. |
| Release | First beta of the v0.2.0 line, after v0.1.0 is stable (decision 1). |

## 1. Hook facts (verified 2026-11-08)

Sources, read on 2026-11-08:

- Hooks reference: https://code.claude.com/docs/en/hooks (full text from
  https://code.claude.com/docs/en/hooks.md)
- Status line: https://code.claude.com/docs/en/statusline

Facts below are from those pages. Anything marked **(confirmed)** was seen in the T-038 spike.
Anything still marked **(T-047)** could not be tested headless and is checked with the CEO on a
real interactive session.

### T-038 spike results (2026-11-08)

Method: a throwaway project with a logging hook on every event, run headless with `claude -p`
(Claude Code 2.1.319, model haiku) on WSL2 Linux. Run by the chief of staff.

Confirmed:

1. A normal session goes `SessionStart` (`source` `startup`), `UserPromptSubmit`, then
   `PreToolUse` and `PostToolUse` pairs, then `Stop`, then `SessionEnd` (`reason` `other` in
   `-p` mode).
2. The Task tool's `tool_name` is `Agent`. Subagents fire `SubagentStart` and `SubagentStop`.
   Hooks inside a subagent carry the **parent's `session_id`**, plus `agent_type` (for example
   `general-purpose`). So subagent work counts as the parent session working. There is no
   separate row.
3. A tool that needs approval goes `PreToolUse` then `PermissionRequest`, about 100 ms apart.
   `PreToolUse` fires first.
4. A denied tool fires **no `PostToolUse` and no `PostToolUseFailure`**. The next event was
   `Stop`. So `needs_you` is left by the next `PreToolUse`, `UserPromptSubmit` or `Stop`, never
   by a failure event. The mapping below already does this: `Stop` sends `done`.
5. Hook cost: a Python logging hook takes about 80 ms per call on WSL2, almost all of it Python
   start-up. A static Rust binary should be far below the 30 ms target. T-047 measures the real
   binary.
6. `CLAUDE_PROJECT_DIR` is set in the hook environment.

Not testable headless, moved to T-047: what fires after Esc, whether `/clear` gives `SessionEnd`
then `SessionStart` (`source` `clear`), whether open sessions pick up new hooks, and native
Windows timing.

### What every hook receives on stdin

Common fields (hooks page, "Common input fields"): `session_id`, `prompt_id` (absent until the
first prompt), `transcript_path`, `cwd`, `scratchpad_dir`, `permission_mode` (not on every
event), `effort` (on tool events), `hook_event_name`. Inside a subagent also `agent_id` and
`agent_type`.

The hook process also gets the environment variable `CLAUDE_PROJECT_DIR`, the project root where
the session started (hooks page, "Exec form and shell form" and "Reference scripts by path").
Unlike `cwd`, it does not move when the session changes directory, so it is the better source for
the project name. **(confirmed)**: it is set in the hook environment.

### Events we use and what each one carries

| Event | When it fires (docs) | Extra fields we read | Extra fields we never send |
|---|---|---|---|
| `SessionStart` | a session starts or resumes; `source` is `startup`, `resume`, `clear`, `compact` or `fork` | `source` | `model`, `session_title`, `context_tokens` |
| `UserPromptSubmit` | a prompt is submitted, also a `/loop` tick, a background subagent reporting back, a message from another session | none | `prompt` (the full prompt text) |
| `PreToolUse` | before every tool call, whether or not it needs permission | `tool_name` | `tool_input`, `tool_use_id` |
| `PermissionRequest` | the moment Claude Code is about to ask you for permission (no delay) | `tool_name` | `tool_input`, `permission_suggestions` |
| `PostToolUse` | after a tool call succeeds | `tool_name` | `tool_input`, `tool_response` (can be large) |
| `PostToolUseFailure` | after a tool call fails | `tool_name` | everything else |
| `Notification` | Claude Code sends a notification; `notification_type` is the matcher | `notification_type` | `message`, `title` |
| `Stop` | the main agent finished responding. **Does not run after a user interrupt** (Esc). API errors fire `StopFailure` instead | `background_tasks` (only its length) | `last_assistant_message`, task descriptions and commands |
| `StopFailure` | the turn ended on an API error; `error` is one of a fixed list (`rate_limit`, `overloaded`, `authentication_failed`, `billing_error`, `server_error`, `unknown`, ...) | `error` (the field) | `error_details`, `last_assistant_message` |
| `SessionEnd` | the session ends; `reason` is `clear`, `resume`, `logout`, `prompt_input_exit` or `other` | none | none |

Notification types that matter (hooks page, "Notification"):

- `permission_prompt`: a permission prompt has waited about **six seconds** with no typing.
- `idle_prompt`: Claude finished about **60 seconds** ago and you have not typed since.
- `elicitation_dialog`, `elicitation_url_dialog`: an MCP server asks you a question, after about
  six seconds.
- `agent_needs_input`: a background session waits on your input (agent view open).

The docs say plainly: to react the moment Claude asks for permission, use `PermissionRequest`,
not the `permission_prompt` notification. So `PermissionRequest` is our main "needs you" signal
and the notification is a backup.

Events we do not install in the first beta: `SubagentStart`, `SubagentStop` (subagent tool calls
already arrive as `PreToolUse` and `PostToolUse` with the parent's `session_id` and an
`agent_type` **(confirmed)**, so they keep the parent row working), `Elicitation` (the
notification covers it), `PreCompact`, `TaskCreated` and the rest.

### Event to state mapping

The client does this mapping in one pure function (`map_event`), so it is tested in one place.

| Hook event | Condition | State sent | "doing" text sent |
|---|---|---|---|
| `SessionStart` | `source` is `compact` | `working` | `compacting` |
| `SessionStart` | any other `source` | `idle` | empty |
| `UserPromptSubmit` | | `working` | `thinking` |
| `PreToolUse` | | `working` | tool label (below) |
| `PermissionRequest` | | `needs_you` | `allow <tool label>?` |
| `PostToolUse`, `PostToolUseFailure` | | `working` | tool label |
| `Notification` | `permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input` | `needs_you` | `permission`, `question` or `input` |
| `Notification` | `idle_prompt` | `done` | empty |
| `Notification` | any other type | nothing sent | |
| `Stop` | `background_tasks` is empty or missing | `done` | empty |
| `Stop` | `background_tasks` has entries | `working` | `background` |
| `StopFailure` | | `error` | `API error: <error>` (only from the fixed list, else `unknown`) |
| `SessionEnd` | | `ended` | empty |
| anything else | | nothing sent | |

Tool label: a built-in tool name as Claude Code sends it (`Bash`, `Edit`, `Write`, `Read`,
`WebFetch`, `Agent`, ...). An MCP tool (`mcp__<server>__<tool>`) becomes just `MCP`, because the
server name is user-chosen and can say more than you want on a desk screen. Cut to 32 bytes on a
character boundary.

Why `PostToolUse` is needed: `PreToolUse` runs before the permission check, about 100 ms ahead
of `PermissionRequest` **(confirmed)**. So the order for a tool that needs approval is
`PreToolUse` (working), `PermissionRequest` (needs you), you click allow, the tool runs,
`PostToolUse` (working). Without `PostToolUse` the row would say "needs you" for the whole run of
a long command after you already approved it.

Denied tool: no `PostToolUse` and no `PostToolUseFailure` comes **(confirmed)**. The next
`PreToolUse` (working), `UserPromptSubmit` (working) or `Stop` (done) moves the row on. In the
spike the next event was `Stop`. No rule depends on a failure event to leave `needs_you`, so a
denial never leaves the row stuck. If nothing at all follows a denial, the row stays `needs_you`,
which is true: Claude is waiting for you.

Why `StopFailure` is its own state: an API error ends the turn. The agent is not waiting on a
click, so `needs_you` (and its focus jump) would be wrong, but the CEO still wants to see it. It
gets the `error` state: red, and with a pack the Sad emote.

### Known gaps in what hooks can tell us

- **Interrupt.** `Stop` does not fire after Esc. The row stays `working` until the next event.
  The daemon marks a `working` row **quiet** after 10 minutes without a packet (section 5).
  Whether `idle_prompt` fires after an interrupt is not stated **(T-047)**.
- **`/clear`.** Expected: `SessionEnd` then `SessionStart` with `source` `clear` and a new
  `session_id`, so the old row walks off and a new one appears **(T-047)**.
- **Crash or kill.** No `SessionEnd`. The row stays until it is forgotten after 4 hours.
- **Long tools.** A 20-minute build sends `PreToolUse`, then nothing until `PostToolUse`. That is
  why "quiet" is shown as a soft hint ("no news for 12m"), not as an error.
- **Headless runs** (`claude -p`, scheduled agents): hooks run too, so these sessions appear.
  In non-interactive mode a `PermissionRequest` with no decision is denied, so the row shows
  "needs you" for a moment. Acceptable; documented.

### Context usage: only the status line sees it

- No hook event carries token counts or context use. The common fields and every event schema
  we use were checked. The only near miss: `SessionStart` with `source` `resume` or `fork` gets
  `context_tokens` (tokens the first request will resend), once, at start.
- The **status line** command gets `context_window.used_percentage` (input tokens only, may be
  `null` early in a session), `context_window.context_window_size`, and
  `cost.total_duration_ms` (wall-clock time the session has run, across resumes). It also gets
  `session_id`, so the same `sid` can be computed.
- The status line runs at session start, after each assistant message, after `/compact`, on a
  permission mode change and on an optional `refreshInterval` timer. Updates are debounced at
  300 ms and an in-flight run is cancelled when a new one starts.
- A user has one `statusLine` setting. Many users already have one (this team does).

So context usage needs the user to route their status line through `deskpane` (or call it from
their own status line script). That is a separate opt-in step with its own trade-offs, planned
as T-048 (section 8), not part of the first beta. "How long it has been running" does not need
it: the daemon measures it (section 5).

### Hook execution facts that shape the client

| Fact (hooks page) | Consequence for us |
|---|---|
| Default timeout is 600 s for command hooks, 30 s on `UserPromptSubmit`; `"timeout": <seconds>` sets it | We set `"timeout": 2`. Our own deadline is 500 ms, so the hook timeout is only a backstop. |
| Matching hooks for an event run in parallel; an identical handler in several settings files runs once | Installing in both user and project settings does not double-send. |
| Exit 0: stdout is parsed as JSON; on `UserPromptSubmit` and `SessionStart` plain stdout is **added to Claude's context** | `deskpane agent` must print nothing on stdout, ever. Diagnostics go to stderr only with `--verbose`. |
| Exit 2 blocks on blocking events; other codes are non-blocking errors with a notice | We always exit 0. |
| `"onFailure": "continue"` / `"block"` (v2.1.295+) | We set `"continue"` explicitly. |
| `"async": true` runs the hook in the background without blocking; timeout not enforced | Possible, but hooks then race each other (see "Sync, not async" in section 2). |
| Exec form (`command` + `args`) spawns the program with no shell; on Windows `command` must be a real `.exe` | We use exec form with an absolute path to `deskpane` / `deskpane.exe`. No quoting problems, no PowerShell traps. |
| Windows without Git Bash: a failing shell-form `PreToolUse` or `PermissionRequest` hook can block the tool call | Exec form plus exit 0 plus `onFailure: "continue"` keeps us out of this case. |
| `SessionEnd` hooks share a 1.5 s budget | Our hook finishes well inside it. |

## 2. Client side: `deskpane agent`

### Command shape

```
deskpane agent --daemon 192.168.1.50:8765 [--device-id dp-3f9a0c41] [--project <name>] [--hide-project] [--verbose]
deskpane agent hooks-snippet --daemon 192.168.1.50:8765 [same options]
deskpane agent install-hooks --daemon 192.168.1.50:8765 [--settings <path>] [--dry-run] [same options]
deskpane agent uninstall-hooks [--settings <path>] [--dry-run]
deskpane agent test --daemon 192.168.1.50:8765
```

A new `Agent` variant in the clap `Command` enum in `reporter/src/main.rs`, with its own
subcommands. Project logic for the hook path goes in `reporter/src/agent.rs` (or `main.rs` if
small); generic plumbing (settings merge, deadline runner) goes in `reporter/src/services/`.

### The hook path, step by step

1. Note the start time and the sender time `t` (milliseconds since the Unix epoch).
2. Read stdin to the end, at most 8 MiB. `PostToolUse` carries the tool's whole output, so the
   input can be large; we still drain it so Claude Code never sees a broken pipe. Over the cap:
   stop, send nothing, exit 0.
3. Parse into a small struct with only the fields we read (`session_id`, `hook_event_name`,
   `tool_name`, `notification_type`, `source`, `error`, the length of `background_tasks`). serde
   skips everything else. A parse error: send nothing, exit 0.
4. `map_event` (pure) returns the state and "doing" text, or "send nothing".
5. Build the packet: `sid` from the session id, `device_id` from `--device-id` (or derived from
   the machine id as the reporter does), project from `--project`, else the basename of
   `CLAUDE_PROJECT_DIR`, else the basename of `cwd`, cut to 32 bytes, control characters removed.
6. Resolve `--daemon` and send one datagram with a non-blocking socket. A host name is resolved
   on a helper thread; if it is not resolved within the deadline, give up.
7. Exit 0. At 500 ms after start the process exits 0 whatever it is doing.

Every failure path ends in "send nothing, exit 0". Nothing is printed on stdout. With
`--verbose`, one line per hook goes to stderr, for debugging the setup.

### Speed

- One process spawn, one small read, one parse, one `sendto`. No reply is awaited. No file is
  written.
- The machine id is not read on the hook path when `--device-id` is given. On Windows reading it
  spawns `reg.exe`, which would add a second process per hook. `hooks-snippet` and
  `install-hooks` resolve the id once and write it into the hook's `args`.
- Target, measured in T-047 with the real binary: p95 under 30 ms per hook on Linux, under 80 ms
  on Windows (Defender scanning on spawn is the unknown). Two hooks run per tool call
  (`PreToolUse` and `PostToolUse`). The T-038 spike measured about 80 ms for a Python logging
  hook on WSL2, almost all Python start-up; a static Rust binary has no such start-up.

### Sync, not async

Claude Code can run hooks with `"async": true`, which costs the agent nothing. The catch: async
hooks run in parallel with whatever comes next, so `PreToolUse` and `PermissionRequest`, which
fire within milliseconds of each other, can reach the Pi in the wrong order. The worst case is
"needs you" arriving first and "working" second: the screen then says working while the agent
waits for you, which is the one thing the feature must get right.

Recommendation: install the hooks as normal (sync) hooks with `"timeout": 2`. The daemon also
keeps a reorder guard on `t` (section 5) as a second line of defence. If T-047 shows the sync cost
is noticeable on Windows, we switch only `PreToolUse` and `PostToolUse` to async and keep
`PermissionRequest`, `Notification`, `Stop`, `StopFailure` and `SessionEnd` sync. That is
decision 4 (approved).

### Settings: the snippet

`deskpane agent hooks-snippet` prints this (paths and ids filled in), for the user to merge into
`~/.claude/settings.json` (Windows: `%USERPROFILE%\.claude\settings.json`):

```json
{
  "hooks": {
    "SessionStart":       [{ "hooks": [H] }],
    "UserPromptSubmit":   [{ "hooks": [H] }],
    "PreToolUse":         [{ "hooks": [H] }],
    "PermissionRequest":  [{ "hooks": [H] }],
    "PostToolUse":        [{ "hooks": [H] }],
    "PostToolUseFailure": [{ "hooks": [H] }],
    "Notification": [{
      "matcher": "permission_prompt|idle_prompt|elicitation_dialog|elicitation_url_dialog|agent_needs_input",
      "hooks": [H]
    }],
    "Stop":               [{ "hooks": [H] }],
    "StopFailure":        [{ "hooks": [H] }],
    "SessionEnd":         [{ "hooks": [H] }]
  }
}
```

where each `H` is:

```json
{
  "type": "command",
  "command": "/usr/local/bin/deskpane",
  "args": ["agent", "--daemon", "192.168.1.50:8765", "--device-id", "dp-3f9a0c41"],
  "timeout": 2,
  "onFailure": "continue"
}
```

On Windows `command` is the absolute path of `deskpane.exe`, for example
`C:\\Program Files\\deskpane\\deskpane.exe`. The matcher is a plain `|` list, which the docs treat
as exact names.

### Settings: `install-hooks` (opt-in)

Editing a file the user cares about is the risky part. Rules:

1. Find the file: `--settings <path>`, else `~/.claude/settings.json` (Windows:
   `%USERPROFILE%\.claude\settings.json`). A missing file is created with only our block.
2. Parse it. If it is not valid JSON, or `hooks` is not an object, or an event's value is not an
   array: **stop, change nothing**, print the reason and point to `hooks-snippet`.
3. Back up first: copy to `settings.json.deskpane-backup-YYYYMMDD-HHMMSS` next to it, and print
   the backup path. No backup, no write.
4. Merge, never overwrite: for each event, add one matcher group with our handler, unless a
   handler of ours is already there. "Ours" means `command` ends in `deskpane` or `deskpane.exe`
   and `args[0]` is `agent`. An existing handler of ours is replaced in place (new daemon address
   or id), so running it twice changes nothing. Every other key, event and handler is kept as is,
   in the same order (serde_json's `preserve_order` feature).
5. Write atomically: write a temp file in the same folder, check the original's modified time
   has not changed since step 2 (Claude Code may be writing it), then rename over it. If it
   changed, stop and ask the user to run it again.
6. `--dry-run` prints the merged file and writes nothing.
7. Print: what was added, the backup path, and "Restart open Claude Code sessions" (whether
   running sessions pick up new hooks is not documented for this case **(T-047)**).
8. `uninstall-hooks` removes only handlers that match rule 4, removes matcher groups and event
   keys left empty by that, with the same backup and atomic write.

The settings merge is a pure function (`merge_agent_hooks(settings: Value, handler: Value) ->
Result<Value, String>` and `remove_agent_hooks`) in `reporter/src/services/claude_settings.rs`,
so tdd-guide tests it on fixture files with no disk access. Only the thin wrapper touches files.

### `deskpane agent test`

Sends a fake session (`project` = `deskpane test`) through `idle`, `working`, `needs_you`, `done`,
`error` over about 10 seconds, then `ended`. With a pack installed this also shows every emote
and the walk-off. The user watches the screen. This replaces an ack reply: the
proof is visual, and the daemon stays reply-free for agent packets.

### Windows, Linux and WSL

| Where Claude Code runs | Settings file | Binary | Network notes |
|---|---|---|---|
| Linux | `~/.claude/settings.json` | Linux `deskpane` | none |
| Windows (native) | `%USERPROFILE%\.claude\settings.json` | `deskpane.exe`, exec form, absolute path | exec form avoids PowerShell and Git Bash differences |
| WSL | the Linux `~/.claude/settings.json` inside WSL | Linux `deskpane` inside WSL | WSL2 NAT mode: packets leave through Windows and reach the Pi from the Windows host's IP. Mirrored mode: same IP as Windows. Both work, because sessions are keyed by `sid`, not by IP. |

Windows and WSL are separate Claude Code installs with separate settings; each needs its own
setup. The WSL machine id differs from the Windows `MachineGuid`, so WSL agents carry a different
`device_id` than the Windows reporter (the screen shows the id when no reporter runs with it, see
section 6).

## 3. Wire shape

### Same port, new packet kind

Recommendation: keep UDP 8765 and add a `kind` field. A packet with no `kind` is a report, so
every existing reporter keeps working untouched.

Alternatives considered:

- **A second port** (for example 8767): clean separation, but one more firewall rule for every
  user, one more socket and bind error path in the daemon, and the `security-specialist`'s port
  review. No gain for a 200-byte packet.
- **Fields added to `Report`**: wrong shape. An agent is not a device; it has no CPU or disk
  numbers, and many agents share one machine.

### Agent packet

```json
{"kind":"agent","v":1,"sid":"3f9a0c41b2d7","device_id":"dp-3f9a0c41","project":"deskpane","state":"needs_you","doing":"allow Bash?","t":1794152400123}
```

| Field | Type | Rule |
|---|---|---|
| `kind` | string | exactly `"agent"` |
| `v` | integer | agent packet version, `1`. Any other value is dropped. |
| `sid` | string | exactly 12 characters `[0-9a-f]` |
| `device_id` | string | the existing id rule `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$` (`check_device_id`) |
| `project` | string | 0 to 32 bytes UTF-8, no control characters. Empty shows as `?`. |
| `state` | string | one of `idle`, `working`, `done`, `needs_you`, `error`, `ended` (longest is `needs_you`, 9 bytes) |
| `doing` | string | 0 to 32 bytes UTF-8, no control characters |
| `t` | integer | sender wall clock, milliseconds since the Unix epoch, `>= 0` |
| `ctx` | integer, optional | 0 to 100, context used in percent. Not sent in the first beta (reserved for T-048). |

Size: the largest valid packet (64-byte id, 32-byte project and doing with multi-byte text) is
under 260 bytes, far inside the 1500-byte read limit. Adding `error` does not change it.

Nothing about characters goes on the wire. The look is chosen on the Pi from the project name
(section 10), so the packet, the contract and the daemon do not know packs exist.

Validation follows `validate` for reports: one function `validate_agent(&AgentPacket) ->
Result<(), String>` in `daemon/src/contract.rs`, the reason names the field and never repeats the
bad text, the whole packet is dropped on any failure. Unknown fields are ignored (no
`deny_unknown_fields`), so later optional fields like `ctx` do not need a version bump. `v` only
changes for a breaking change.

### How the daemon tells the two apart

```rust
// only the kind, every other field is ignored at this step
#[derive(Deserialize)]
struct PacketKind {
    #[serde(default)]
    kind: Option<String>,
}
```

`udp_server` parses `PacketKind` first, then:

- no `kind`: parse `ReportPacket` as today (old and new reporters).
- `kind` is `"agent"`: parse `AgentPacket`, `validate_agent`, store.
- any other `kind`: drop, log `unknown packet kind from <peer>` (without the value).

Parsing twice costs microseconds at this packet rate. An explicit probe is easier to read and
gives clearer errors than a serde `untagged` enum.

### Old daemons

A daemon from v0.1.x parses every datagram as a report. An agent packet has `device_id` but no
`label`, so it fails with `bad report from <peer>: missing field ...`, is dropped and nothing
else happens. The cost is one log line per hook on that Pi. Install notes say: upgrade the Pi
first. No crash, no reply, no wrong data.

## 4. Privacy

| Data | Default | Why |
|---|---|---|
| `sid` | sent, as a 12-hex SHA-256 prefix over `"deskpane-agent-session-v1:" + session_id` | Rows need a stable key. The raw session id names the transcript file on disk and is used by `--resume`; there is no reason to put it on the LAN. 48 bits is plenty for 16 rows. |
| `device_id` | sent | Already on the wire in every stats report. Lets the screen show the machine's label. |
| project | sent: basename of `CLAUDE_PROJECT_DIR` (else `cwd`), cut to 32 bytes | It is the main thing the CEO wants to see. The full path is never sent (it contains the user name). |
| tool label | sent: built-in tool name, MCP tools as plain `MCP` | Answers "what is it doing" without content. |
| state words | sent | fixed words from the client |
| `StopFailure` error | sent only if it is in the documented fixed list, else `unknown` | fixed vocabulary |
| sender time `t` | sent | ordering; reveals only when the hook ran |
| prompt, tool input, tool output, file paths, commands, `last_assistant_message`, notification `message` and `title`, `transcript_path`, `cwd`, `error_details`, model, session title | **never** | content |

Opt-in controls in the first beta:

- `--project <name>`: show this alias instead of the folder name.
- `--hide-project`: send an empty project (the screen shows `?` and the machine label).

Later, opt-in only (T-049, not in the first beta): `--detail` adds a file basename for
`Edit`/`Write`/`Read` and the program name (first word) for `Bash`. Both can leak (file names of a
private project, a script name), so they stay off by default.

Two honest limits, written into SECURITY.md:

- Packets are plain JSON on the LAN, like stats. A host that can see the traffic learns project
  names, tool names and when you work.
- The screen is physical. Visitors see project names. `--project` and `--hide-project` are the
  answer.

## 5. Daemon side

### Agent table (`daemon/src/agent_table.rs`, new, pure)

Same pattern as `device_table.rs`: plain data in, outcome out, times passed in as `Instant`.

```
AgentTable: sid -> AgentEntry {
    packet: AgentPacket,      // latest valid packet (state, project, doing, device_id, t)
    owner: IpAddr,            // source IP of the packet that created or last refreshed it
    first_seen: Instant,      // for "running for"
    state_since: Instant,     // when the state last changed
    last_seen: Instant,       // for quiet and forget
}
```

`insert_agent(table, packet, source, now, limits) -> AgentOutcome`:

| Table state when a valid agent packet for `sid` arrives from IP `src` | Outcome |
|---|---|
| `state` is `ended`, `sid` in table, owned by `src` | entry removed: `Removed` |
| `state` is `ended`, `sid` not in table | nothing: `Ignored` |
| `sid` owned by another IP that sent something in the last 120 s (`AGENT_TAKEOVER_AFTER`) | dropped: `NotOwner` (no reply) |
| `sid` in table and `packet.t` is older than the stored `t` by less than 5 s (`AGENT_REORDER_WINDOW`) | dropped: `OutOfOrder` |
| `sid` in table otherwise | stored; `state_since` reset only if the state changed: `Stored` |
| `sid` new, `src` already owns 8 entries (`MAX_AGENTS_PER_SOURCE`) | dropped: `SourceFull` |
| `sid` new, table has 16 entries (`MAX_AGENTS`) | evict the `idle`, `done` or `error` entry with the oldest `last_seen` and store; if there is none, drop: `TableFull` |

Notes on the rules:

- **Ownership by IP, like device ids**, but no `id_in_use` reply: an accidental `sid` clash is
  practically impossible, so a clash means forgery, and forgers get no answer. Takeover after
  120 s of silence covers a laptop that changed IP mid-session (the next hook after the change is
  dropped for up to 2 minutes, then accepted).
- **Reorder guard**: a packet older than the stored one by up to 5 s is a late arrival and is
  dropped. A packet older by more than 5 s means the sender's clock jumped back (NTP), so it is
  accepted, otherwise a clock change could freeze a row.
- **Eviction** keeps the table useful when old rows pile up (each `/clear` starts a new session,
  crashed sessions linger), and never evicts a `working` or `needs_you` row. An `error` row is
  evictable: its turn has ended, like `done`.
- **Per-source cap** stops one machine (or one forger without source spoofing) from taking the
  whole table.

`forget_agents(table, now, forget_after) -> Vec<String>` removes entries silent for 4 h
(`AGENT_FORGET_AFTER`), whatever their state, and returns their `sid`s for one log line each.
Called from the existing one-second tick, next to `forget_silent_devices`.

### Derived display fields (computed at broadcast time)

| Field | Rule |
|---|---|
| `running_s` | `now - first_seen`, whole seconds. Resets if the Pi restarts. T-048 can replace it with `cost.total_duration_ms` from the status line. |
| `state_s` | `now - state_since`, whole seconds |
| `quiet` | `state == working` and `now - last_seen >= 10 min` (`AGENT_QUIET_AFTER`). Never set for `needs_you`, `error`, `done` or `idle`: no news is normal there. |
| `label` | the `label` of the device with the same `device_id` in the device table, else empty |

### Constants (`daemon/src/main.rs`)

| Constant | Value | Why |
|---|---|---|
| `MAX_AGENTS` | 16 | 3 to 4 screen pages; far above a normal day's open sessions |
| `MAX_AGENTS_PER_SOURCE` | 8 | one busy machine fits; one forger cannot fill the table |
| `AGENT_TAKEOVER_AFTER` | 120 s | IP change mid-session recovers in 2 minutes |
| `AGENT_REORDER_WINDOW` | 5 s | far longer than any LAN reordering, far shorter than a clock change |
| `AGENT_QUIET_AFTER` | 10 min | long tools and interrupts; a soft hint only |
| `AGENT_FORGET_AFTER` | 4 h | a "needs you" left over lunch is still there; a crashed session is gone by evening |

### Logging

Agent packets arrive much more often than stats (every tool call). A misconfigured client, or an
old daemon's mismatch in the other direction, must not fill the journal. All `bad`, `rejected`,
`not owner`, `table full` and `unknown kind` lines for agent packets go through a per-source log
limiter (the existing `ReplyLimiter`, a second instance used only for log lines: one line per
source per 10 s). Stored packets are not logged. A new session and a forgotten one get one line
each.

### Broadcast to the dashboard: the envelope

Today each socket line is a JSON array of `DeviceStatus`. Agents need a second list, so the line
becomes the envelope the vision plan already designed, with two views:

```json
{"v": 1,
 "views": {
   "stats":  {"devices": [ ...DeviceStatus, unchanged... ]},
   "agents": {"sessions": [
     {"sid": "3f9a0c41b2d7", "device_id": "dp-3f9a0c41", "label": "my-pc",
      "project": "deskpane", "state": "needs_you", "doing": "allow Bash?",
      "running_s": 1520, "state_s": 34, "quiet": false, "ctx": null}
   ]}
 }}
```

This pulls one v0.2.0 item (the envelope) forward, with only `stats` and `agents`. It is an
internal format: daemon and dashboard ship in the same bundle. The dashboard still accepts a bare
array (treats it as stats only) for one release, so a half-upgraded dev setup does not show an
error screen. Later views (`tasks`, `canvas`) add keys; nothing here moves.

## 6. Screen

### Pages

- The agents page joins the page ring after the overview and before the device pages, so one tap
  from the clock reaches it. With no sessions there is no agents page (the vision plan's rule: a
  view with no content contributes no page).
- 1 session: one big card. Project in the large font, the state word large and coloured, "doing"
  under it, machine label and "running 25m" at the bottom.
- 2 to 5 sessions: one row each, rows share the space between the header (40 px) and the page
  dots (24 px): 256 px, so 64 px per row for up to 4 rows and 51 px for 5.
- More than 5: more agents pages, 5 rows each. Sorting puts "needs you" first, so page 1 always
  shows every waiting session while there are 5 or fewer.
- With a character pack, the icon is replaced by the project's character and its emote
  (section 10). Everything else on this page stays the same, and the page works the same with no
  pack.

### Row layout (480 px wide)

```
| 8px state bar | icon | project (bold)                 12m |
|               |      | my-pc · allow Bash?    NEEDS YOU   |
```

- Left: an 8 px bar in the state colour, then a 24 px icon (no pack) or a 64 px character cell
  (pack: 48 px character plus a 32 px emote at its top right, section 10). The text column
  starts after whichever is drawn.
- Line 1: project (ellipsised to fit), right-aligned time in state (`45s`, `12m`, `3h`).
- Line 2: machine label (or `device_id` when no reporter runs with that id), a dot, the "doing"
  text, and the state word right-aligned.

### Colours and icons per state

Colour is never the only signal: every state also has an icon and a word.

| State | Colour | Icon | Word |
|---|---|---|---|
| `needs_you` | amber, and the **whole row filled** amber with dark text | `!` in a circle | NEEDS YOU |
| `working` | blue | the existing pulse dot (`draw_pulse_dot`) | WORKING |
| `working` + `quiet` | grey-blue | `?` | QUIET, line 2 says `no news for 12m` |
| `done` | green | check mark | DONE |
| `done` for more than 30 min | dimmed green, sorted last | check mark | DONE |
| `error` | red bar and red word, row not filled | `x` in a circle | ERROR, line 2 says `API error: rate_limit` |
| `idle` | grey | open circle | READY |

These icons are the built-in fallback. With a pack, the character and emote take the icon's
place; the colour bar, the amber fill and the word stay, so colour and word still carry the
state for anyone who cannot read the sprite.

### "Needs you" made prominent

1. The amber filled row (above).
2. On the overview page, a line under the device summary: `Agents: 1 needs you · 1 error · 2
   working`, with the "needs you" part in amber and the "error" part in red. Parts with a zero
   count are left out. Hidden when there are no sessions.
3. Focus jump: when a session **enters** `needs_you`, the dashboard switches to the agents page
   for 30 s, unless the screen was tapped in the last 10 s. Any tap ends the focus. It jumps once
   per transition, not again while the row stays amber. This is decision 3 (approved). Entering
   `error` does **not** jump: nothing is waiting on a click (see section 11, item 2).

### Pure helpers in `pi/layout.py`

Following the beta.3 pattern (no pygame import, stdlib `unittest` in `pi/tests/`):

| Helper | Returns |
|---|---|
| `sort_agents(sessions)` | `needs_you` (longest waiting first), `error` (most recent first), `working`, `idle`, `done` (most recent first, dimmed ones last) |
| `agent_style(state, quiet, state_s)` | colour name, icon name, word, `dim` flag; `error` gives red, `x`, ERROR |
| `agent_pages(count, per_page=5)` | list of `(start, end)` slices |
| `agent_rows_layout(width, height, n_rows, header_h, footer_h)` | one rect per row; one card rect for a single session |
| `fmt_elapsed(seconds)` | `45s`, `12m`, `3h`, `2d` |
| `agent_summary(sessions)` | the overview line parts with counts, or `None` when empty |
| `fit_text(text, max_w, measure)` | text ellipsised to fit; `measure` is injected, so tests pass a fake width function |
| `should_focus_agents(prev_states, new_states, last_tap, now)` | `True` only on a new transition into `needs_you` and no recent tap |
| `parse_socket_line(obj)` | `(devices, sessions)` from the envelope or from a legacy bare array |

`dashboard.py` gets `draw_agents` (wrapped in the existing per-page try/except) and the overview
line. Drawing code stays thin; the decisions live in the helpers. The character helpers live in
their own pure module, `pi/charpack.py` (section 10), so `layout.py` stays about layout.

## 7. Security

Same model as stats: a trusted home LAN, no authentication, no encryption in v0.1.x and in this
first beta. Untrusted input still enters in one place (`udp_server`), is checked by one function
(`validate_agent`) and stored through one pure module (`agent_table.rs`).

What a forger on the LAN can do:

| Attack | Effect | Limit |
|---|---|---|
| Fake sessions | rows that do not exist, including fake "needs you" | at most 8 per source IP; 16 in total; forged rows expire after 4 h |
| Flip a real session's state | needs the `sid` (only visible to a host that can see the traffic) and must send from the owner's IP or wait for 120 s of silence | ownership by IP |
| Text on the screen | up to 32 bytes of project and 32 of "doing" text | same class as device labels today (64 bytes); no control characters |
| Fill the table to hide real sessions | with spoofed source IPs, 16 forged `working` rows block new real sessions | real rows already shown keep updating; eviction never removes real `working` rows, but also cannot make room |
| Journal spam | | per-source log limiter |
| Reflection or amplification | none | agent packets never get a reply |

Why this is acceptable for a beta: the worst outcome is a false or missing row on a desk screen.
No code runs on the content, nothing is persisted, no reply goes out, and the same LAN host can
already show false device stats today. The new thing compared with stats is the *meaning* of a
fake "needs you": it can make you walk to your computer for nothing. That is an annoyance, not a
breach.

How later work covers it:

- **v0.2.0 authentication (T-006).** The token store and HMAC on UDP reports apply to agent packets
  unchanged: the same MAC field, the same per-machine key. The daemon then drops (or marks as
  "unverified") agent packets from machines without a key. The `kind` probe means signed and
  unsigned packets can coexist during the upgrade.
- **Pairing through the Pi portal (idea 2).** The 6-digit code on the screen gives a machine its
  key. `deskpane agent install-hooks` can then write the key reference into the hook `args`, so
  agent packets are signed from the first hook.
- **No new listener, no new port.** The security-specialist's firewall advice (allow 8765 from
  the LAN only) covers this feature as is.

Client-side safety (security-reviewer to check):

- The hook never prints to stdout (it would be injected into Claude's context on
  `UserPromptSubmit` and `SessionStart`).
- The hook parses untrusted-ish input (the transcript of a session can contain anything, and
  `tool_response` can be huge) only into a fixed small struct, with an input cap.
- `install-hooks` never writes without a backup, never writes over a file it could not parse,
  never removes handlers that are not ours, writes atomically.

Character pack install (security-specialist owns `install.sh`, security-reviewer checks the
validator): the zip is a local file the CEO supplies, but it is still treated as untrusted. It is
opened only by a stdlib validator, never extracted with `unzip`, never run, and only fixed names
are written to a fixed root-owned folder. Section 10 has the rules.

## 8. Scope and release

### First beta (v0.2.0-beta.1, see decision 1)

- `deskpane agent` hook path, `hooks-snippet`, `install-hooks`, `uninstall-hooks`, `test`.
- Agent packet in the contract, `kind` probe, `validate_agent`.
- `agent_table.rs` with caps, ownership, reorder guard, quiet, forget, eviction.
- Socket envelope with `stats` and `agents`.
- Agents page, overview line, focus jump.
- `error` state end to end.
- Character packs: pure helpers, `--characters` on `install.sh` and `get.sh`, sprite drawing with
  fallback, bundle and repo guards (section 10).
- Docs: CONTRACT.md, INSTALL.md (per OS), SECURITY.md, CHARACTERS.md (pack format), README one
  line, CLAUDE.md layout.

### Later

- **T-048: context usage.** `deskpane agent statusline`: reads the status line JSON, sends an
  agent packet with `ctx` (from `context_window.used_percentage`) and `running` from
  `cost.total_duration_ms`, and prints a short status line itself (project and context bar). For
  users with their own status line: a `--quiet` mode they can call from their script with the
  same stdin. A `ctx` packet only updates an existing row, never creates one. `install-hooks`
  never touches an existing `statusLine`.
- **T-049: opt-in detail** (`--detail`, section 4).
- Subagent count on a row (`SubagentStart` / `SubagentStop`), if the CEO wants it. The spike
  confirmed both events exist and that subagent hooks carry the parent's `session_id`.
- Pinning a look to a project from the Pi portal (idea 2), section 10.
- Signed agent packets with v0.2.0 auth (falls out of T-006, no separate design).

### Room for the MCP task list (idea 3), not designed here

- The envelope gets a `tasks` view next to `agents`; the page ring already groups by view.
- `kind` dispatch on UDP and the per-view tables keep each producer separate. The task list will
  arrive over the control API (vision plan), not UDP, so nothing here constrains it.
- A session row has a stable key (`sid`). If the MCP bridge can later learn which session it
  serves, a row could show `3/5 tasks`. Whether the bridge can know the session is an open
  question for that plan.

### Tasks

Order is top to bottom. tdd-guide writes the failing tests first for every build task.

| Task | What | Who |
|---|---|---|
| T-038 | **Partly done 2026-11-08.** Spike, no repo code: a throwaway logging hook on a headless `claude -p` session on WSL2 Linux. Confirmed event order, denied-tool behaviour, subagent `session_id`, `Agent` tool name, `CLAUDE_PROJECT_DIR`, Python hook cost (section 1). The interactive and Windows checks moved to T-047. | chief of staff ran it, architect amended the plan |
| T-039 | Contract: `PacketKind`, `AgentPacket`, `AgentState` (with `error`), `validate_agent`, limits (`AGENT_PACKET_VERSION`, `MAX_PROJECT_BYTES`, `MAX_DOING_BYTES`, `SID_LEN`). | tdd-guide |
| T-040 | `daemon/src/agent_table.rs`: `insert_agent`, `forget_agents`, ownership, both caps, eviction (`idle`, `done`, `error` evictable), reorder guard, derived fields. | tdd-guide |
| T-041 | Daemon wiring: `kind` dispatch in `udp_server`, log limiter for agent lines, forget on the tick, envelope broadcast with the label join. | tdd-guide |
| T-042 | Client hook path: `deskpane agent`, `map_event` (`StopFailure` sends `error`), `sid` derivation, project derivation, input cap, 500 ms deadline, no stdout, exit 0. | tdd-guide |
| T-043 | Client setup: `hooks-snippet`, `install-hooks`, `uninstall-hooks` (pure merge in `services/claude_settings.rs`), `test` (now also walks through `error`). | tdd-guide |
| T-044 | Dashboard: `parse_socket_line`, agents page with built-in icons (including `error`), overview line, focus jump; helpers in `pi/layout.py` with tests. Visual pass on the rendered PNGs. | tdd-guide, then designer |
| T-050 | Character pack pure helpers in `pi/charpack.py`: pack format constants, frame rects, look list, `assign_looks`, `character_pose`, emote timing, `anim_frame`, best scale, zip entry plan, PNG header check. Tests with generated fixtures only. | tdd-guide |
| T-051 | Pack install: `pi/charpack_install.py` (thin, stdlib), `install.sh --characters` and `--remove-characters`, `get.sh --characters` (pass through, refuse with `--reporter-only`, refuse an older bundle), `uninstall.sh --purge` removes the folder, `provision-pi.sh --characters` copies the zip to the Pi first. Fake-root runs with `DESKPANE_DESTDIR`. | tdd-guide, then security-specialist reviews the shell changes |
| T-052 | Sprite drawing: `pi/sprites.py` (thin pygame loader and scaled-frame cache), characters in rows and on the card, emotes, walk-off ghost, 5 fps only while sprites animate on screen, partial band flush if the full frame ghosts, fallback to T-044 icons with no pack or a bad pack. Visual pass on rendered PNGs (kept out of git). | tdd-guide, then designer |
| T-053 | License guards: `package-pi-bundle.sh` refuses to build if any pack file would enter the bundle; a CI step fails if `git ls-files` lists anything under `assets/characters/` or if the bundle listing has a pack file name. | tdd-guide, security-reviewer checks |
| T-045 | Reviews: rust-patterns and tests (code-reviewer); packet parsing, caps, settings editing, stdout rule, privacy table, the pack zip validator (security-reviewer); no port or unit change, the new `/var/lib/deskpane/characters` folder and its modes (security-specialist). | code-reviewer, security-reviewer, security-specialist |
| T-046 | Docs: CONTRACT.md agent packet (with `error`) and envelope, INSTALL.md hook setup for Linux, Windows and WSL and the `--characters` step, new `docs/CHARACTERS.md` (pack format, so a CC0 pack can be dropped in), SECURITY.md (agent packets, privacy limits, forging, pack validation, the pack never goes in git or a bundle), README one line, CLAUDE.md layout and test counts, TEAM.md log line. | doc-updater |
| T-047 | Real check: Pi plus real sessions on Linux, Windows and WSL; measure hook cost with the real binary; the T-038 leftovers (what fires after Esc, `/clear` giving `SessionEnd` then `SessionStart` with `source` `clear`, whether open sessions pick up new hooks, native Windows timing); characters at 5 fps on the real panel with no ghosting; screenshot for `docs/media/` taken **without** the paid pack. | chief of staff with the CEO |
| T-048 | Later: context usage through the status line. | architect designs, tdd-guide builds |
| T-049 | Later: opt-in `--detail`. | tdd-guide |

T-050 to T-053 are listed before T-045 because the reviews, docs and real check cover them too.
T-050 can start in parallel with T-039; T-052 needs T-044 and T-050; T-051 needs T-050.

### Tests to write first

`daemon/src/contract.rs`:

- `packet_without_kind_is_a_report` (old reporters unchanged)
- `packet_with_kind_agent_parses_as_agent`
- `unknown_kind_is_rejected_without_echoing_it`
- `agent_packet_wrong_version_is_rejected`
- `sid_must_be_12_lowercase_hex`
- `agent_device_id_uses_the_device_id_rule`
- `project_over_32_bytes_is_rejected`, `doing_over_32_bytes_is_rejected`
- `agent_text_with_control_characters_is_rejected`
- `unknown_state_is_rejected`
- `every_state_word_parses` (`idle`, `working`, `done`, `needs_you`, `error`, `ended`)
- `agent_packet_unknown_fields_are_ignored`
- `ctx_out_of_range_is_rejected`, `missing_ctx_is_fine`
- `largest_valid_agent_packet_fits_in_1500_bytes`

`daemon/src/agent_table.rs`:

- `new_session_is_stored_and_owned_by_source`
- `state_change_resets_state_since`, `same_state_keeps_state_since`
- `ended_removes_the_session`, `ended_for_unknown_sid_is_ignored`
- `other_ip_is_refused_while_owner_is_recent`, `other_ip_takes_over_after_takeover_window`
- `older_packet_inside_reorder_window_is_dropped`
- `much_older_packet_counts_as_clock_change_and_is_stored`
- `per_source_cap_refuses_ninth_session`
- `full_table_evicts_oldest_done_or_idle`
- `full_table_never_evicts_working_or_needs_you`
- `full_table_may_evict_error`
- `working_becomes_quiet_after_quiet_window`, `needs_you_is_never_quiet`, `error_is_never_quiet`
- `needs_you_then_done_leaves_needs_you` (the denied-tool sequence from the spike)
- `forget_removes_sessions_silent_for_forget_window`
- `time_going_backwards_does_not_panic`

`daemon/src/main.rs` (end to end on 127.0.0.1, like the beta.3 tests):

- `agent_packet_appears_in_the_envelope`
- `agent_packet_gets_no_reply`
- `report_and_agent_packets_both_work_on_one_port`
- `agent_label_comes_from_the_device_table`

`reporter` (`agent.rs` and `services/claude_settings.rs`):

- `map_event` per row of the mapping table, one test each, plus `unknown_event_sends_nothing`
- `mcp_tool_label_is_plain_mcp`, `tool_label_is_cut_on_a_char_boundary`
- `stop_with_background_tasks_is_working`
- `stop_failure_sends_error_state`
- `stop_failure_with_unknown_error_says_unknown`
- `subagent_tool_call_maps_like_a_main_tool_call` (fixture with `agent_type` set and the parent
  `session_id`: same `sid`, `working`)
- `agent_tool_label_is_agent`
- `sid_is_stable_for_a_fixed_session_id` (fixed expected string), `sid_is_12_hex`
- `project_prefers_claude_project_dir_over_cwd`, `project_is_basename_only`,
  `project_alias_wins`, `hide_project_sends_empty`
- `bad_stdin_sends_nothing_and_exits_0`, `oversized_stdin_sends_nothing_and_exits_0`
- `hook_never_writes_stdout` (runs the binary with a sample hook JSON and asserts empty stdout)
- `merge_into_empty_settings_adds_all_events`
- `merge_keeps_other_hooks_and_keys_in_order`
- `merge_twice_is_unchanged` (idempotent)
- `merge_replaces_our_old_handler_in_place`
- `merge_refuses_non_object_hooks`, `merge_refuses_non_array_event`
- `remove_deletes_only_our_handlers`, `remove_drops_groups_left_empty`

`pi/tests/test_layout.py`:

- `test_sort_puts_needs_you_first_longest_waiting_first`
- `test_sort_puts_dim_done_last`
- `test_sort_puts_error_after_needs_you_before_working`
- `test_agent_style_quiet_working_is_grey_with_question_mark`
- `test_agent_style_error_is_red_with_x`
- `test_agent_summary_counts_error_and_drops_zero_parts`
- `test_focus_not_triggered_by_error`
- `test_agent_pages_split_by_five`
- `test_single_session_gets_card_layout`
- `test_rows_fit_between_header_and_dots_for_1_to_5`
- `test_fmt_elapsed_units`
- `test_agent_summary_none_when_empty`
- `test_fit_text_ellipsises_with_fake_measure`
- `test_focus_only_on_new_transition_into_needs_you`
- `test_focus_suppressed_after_recent_tap`
- `test_parse_socket_line_envelope_and_legacy_array`

`pi/tests/test_charpack.py` (T-050, stdlib only; fixtures are tiny PNGs and zips built in the
test with `zlib`, `struct` and `zipfile`, so no test needs the paid pack, Pillow or pygame):

- `test_frame_rect_skips_header_row`
- `test_frame_rect_variant_direction_and_column` (variant 2 counted from 0, back, walk frame 3 gives x 112,
  y 176)
- `test_idle_uses_columns_0_to_3_walk_uses_4_to_7`
- `test_look_count_is_three_per_character` (15 characters give 45 looks; 2 give 6)
- `test_look_for_project_is_stable` (fixed project name, fixed expected index)
- `test_assign_looks_never_repeats_on_screen`
- `test_assign_looks_keeps_existing_looks_when_a_project_leaves`
- `test_assign_looks_takes_next_free_on_collision`
- `test_assign_looks_reuses_when_more_projects_than_looks`
- `test_hidden_project_key_uses_device_id`
- `test_pose_per_state` (one test per row of the state table in section 10)
- `test_happy_shows_for_5_seconds_after_done_then_none`
- `test_surprised_once_for_a_new_session_not_on_first_line`
- `test_emote_pop_in_plays_4_frames_then_holds_last`
- `test_anim_frame_at_5_fps_cycles_4_frames`
- `test_best_scale_is_integer_and_capped_at_8`
- `test_png_header_check_reads_size`, `test_png_header_check_rejects_non_png`
- `test_zip_plan_accepts_full_pack_and_skips_extras`
- `test_zip_plan_rejects_path_traversal`, `test_zip_plan_rejects_absolute_path`,
  `test_zip_plan_rejects_backslash_and_drive_paths`
- `test_zip_plan_rejects_symlink_entry`
- `test_zip_plan_rejects_duplicate_names`, `test_zip_plan_rejects_encrypted_entry`
- `test_zip_plan_rejects_oversized_entry_and_too_many_entries`
- `test_zip_plan_rejects_wrong_png_size`
- `test_zip_plan_needs_npc_1_and_contiguous_numbers`
- `test_zip_plan_allows_missing_emotes`
- `test_install_writes_only_fixed_names_into_target` (into a temp folder)
- `test_install_failure_leaves_old_pack_in_place`

`deploy` (T-051 and T-053, fake root with `DESKPANE_DESTDIR`, generated fixture zip):

- `install.sh --characters` with a good zip fills the folder, root-owned, `0644` files
- `install.sh --characters` with a zip-slip zip fails and changes nothing
- `install.sh` with no flag on an upgrade keeps the folder untouched
- `install.sh --remove-characters` removes only the folder
- `uninstall.sh` keeps the folder, `uninstall.sh --purge` removes it
- `get.sh --reporter-only --characters x.zip` is refused
- `package-pi-bundle.sh` fails when a pack file is placed where the bundle would pick it up

Manual: `deskpane agent test` against the dev daemon and the real Pi; a real session with a
permission prompt on each OS (T-047); the same with the pack installed and with none.

## 9. Decisions for the CEO

All six approved as recommended on 2026-11-08.

1. **Which release.** Approved 2026-11-08. The vision plan says v0.1.0 is the rename only, and the envelope is a v0.2.0
   item. Recommendation: finish v0.1.0 first (beta.3 Pi check, T-035), then ship the agent view as
   v0.2.0-beta.1, ahead of T-006 auth in the same v0.2.0 line. It keeps v0.1.0 small and stable,
   and the envelope lands once.
2. **Project name on the screen by default.** Approved 2026-11-08. Basename of the project folder (recommended), with
   `--project` and `--hide-project` to change it, or hidden by default. Recommendation: show it;
   it is the main point of the feature, and the full path is never sent.
3. **Focus jump to the agents page on "needs you"** Approved 2026-11-08. (30 s, once per transition, any tap ends it,
   not within 10 s of a tap). Recommendation: yes. Without it, "needs you" is only visible when
   you happen to be on the right page.
4. **Sync hooks.** Approved 2026-11-08. Install all hooks as normal (sync) hooks with a 2 s timeout (recommended), and
   only move `PreToolUse` and `PostToolUse` to async if T-038 measures a noticeable cost on
   Windows. Fully async is cheaper but can show "working" while the agent waits for you.
5. **`install-hooks` in the first beta.** Approved 2026-11-08. Ship it next to `hooks-snippet` (recommended), with the
   backup, refuse-on-parse-error and merge-only rules above. The alternative is snippet only,
   which is safer but hand-editing JSON is where most setup errors come from.
6. **Context usage.** Approved 2026-11-08. Leave it out of the first beta and design T-048 as an opt-in status line
   step (recommended). It needs the user to give up or chain their status line, which is a bigger
   ask than hooks.

## 10. Character packs (approved by the CEO 2026-11-08)

Each project on the agents page gets a small pixel-art character, and the character shows the
state with a pose and an emote bubble. It is optional: with no pack installed, the page uses the
built-in icons from section 6 and works exactly the same.

### The pack and its license

The CEO owns a paid pack, Vryell "Tiny Neighbours", saved as
`assets/characters/bot-charatcters.zip` (the folder is in `.gitignore`). Its license says "Do NOT
resell/redistribute". So:

- The pack is **never** committed to the public repo, never put into a release bundle, and never
  shown in public docs or `docs/media/` screenshots. Screens in the repo use the built-in icons
  or a free pack.
- It reaches the Pi only through `--characters <zip>` on the CEO's own machines (section below).
- T-053 adds two guards so a mistake cannot ship it: the bundle script refuses pack files, and CI
  fails if git tracks anything under `assets/characters/`.
- Rendered PNGs from the designer's visual pass with the pack go to a scratch folder, never into
  the repo.

### Pack format v1 (verified on the real pack, 2026-11-08)

This is the format `docs/CHARACTERS.md` documents, so a free CC0 pack with the same layout can be
dropped in later.

Characters: `NPC_<n>.png`, `n` from 1 to 15, at the root of the zip.

- Each is exactly 128 x 208 px, RGBA, made of 16 x 16 frames: 8 columns by 13 rows.
- Row 0 is a header (the words "IDLE" and "WALK"); it is skipped.
- Columns 0 to 3 are idle frames; columns 4 to 7 are walk frames.
- Rows 1 to 4, 5 to 8 and 9 to 12 are three variants (skin tones), called variant 0, 1 and 2.
- Inside a variant the four rows are directions, in this order (checked visually on NPC_1 and
  NPC_7): **front** (facing the viewer), **left** (facing screen left), **back**, **right**
  (facing screen right).
- The right row is usually the mirror of the left row, but not always (NPC_5, NPC_6 and NPC_9
  differ in some variants). So the right row is read from the sheet, never made by mirroring.
- No frame in the 15 sheets is empty.

Frame position, 0-based: `x = 16 * (column)`, `y = 16 * (1 + 4 * variant + direction)`, where
direction is front 0, left 1, back 2, right 3, and column is the frame number for idle or 4 plus
the frame number for walk.

Emotes: `Emotes/<Name>_emote_16x16.png`, each 64 x 16 px: 4 frames of a pop-in, the last one is
the full bubble with its icon. Names: Angry, Evil, Exclamation, Happy, HeartBreak, Heart,
Interrogation, Sad, Star, Surprised. We use five: Exclamation, Interrogation, Happy, Sad,
Surprised.

Rules for other packs: at least `NPC_1.png`, numbers contiguous from 1, at most 15. Any emote
may be missing; a missing emote just is not drawn. Extra files are ignored.

The real zip also holds `Patrons.txt`, `Thanks.txt`, `Palette/Palette.txt`,
`Palette/rosyandblue.png` and two folder entries. Those are skipped, not copied (see section 11,
item 1).

### Looks: one per project

- A look is one character in one variant: 15 x 3 = 45 looks in the full pack. Look index `i`
  maps to character `i // 3 + 1`, variant `i % 3`.
- The look key is the project name. When the project is hidden (empty), the key is `?@` plus the
  `device_id`, so two hidden projects on different machines can still differ.
- Start index: the first 8 bytes of SHA-256 over `"deskpane-look-v1:" + key`, as a number, modulo
  the look count. Python's built-in `hash()` is not used: it changes on every start.
- `assign_looks(keys_on_screen, previous, look_count)` is pure. A project that already has a look
  keeps it. A new project takes its start index, or the next free index after it (wrapping) if
  that look is taken. So two projects on screen never share a look, and a look does not jump
  when another project leaves.
- "On screen" means every session in the latest socket line, not only the current page, so
  paging never swaps looks.
- With more projects than looks (a small free pack), looks are reused, starting from the start
  index. At most 16 sessions exist (`MAX_AGENTS`), so the full pack never runs out.
- All sessions of one project share the project's look.
- Later: the Pi portal (idea 2) could let the user pin a look to a project. `assign_looks` then
  takes the pins as fixed entries in `previous`. Not in this beta.

### State to pose and emote (approved table)

| Agent state | Character | Emote |
|---|---|---|
| `needs_you` | front, idle frames | Exclamation, row filled amber as in section 6 |
| `idle` (waiting for a prompt) | front, idle frames | Interrogation |
| `working` | right, walk frames in place | none |
| `working` + `quiet` | right, idle frames (stopped walking) | none; line 2 says `no news for 12m` |
| `done` | front, idle frames | Happy for the first 5 s in `done` (`HAPPY_FOR_S`), then none |
| `done` dimmed (over 30 min) | front, first idle frame only, dimmed | none |
| `error` | front, idle frames | Sad |
| new session | appears | Surprised for 2 s (`SURPRISED_FOR_S`), then the state's emote |
| ended | walks off to the right, about 2 s (`WALK_OFF_S`) | none |

Details:

- `done` timing uses `state_s` from the daemon, so it survives a dashboard restart without
  showing Happy again for an old `done`.
- "New session" is a `sid` that was not in the previous socket line. On the first line after the
  dashboard starts, nothing counts as new, so a restart does not make every row say Surprised.
- "Ended" is a `sid` that was in the previous line and is gone now (`ended`, forgotten or
  evicted). The dashboard keeps a ghost of that row in its old slot for `WALK_OFF_S`: the
  character walks right and leaves the cell, the word says ENDED in grey. Ghosts are not counted
  in the overview line and do not trigger a focus jump. With no pack, the row just disappears,
  as in section 6.
- Emote pop-in: frames 0 to 3 play once at the animation rate, then frame 3 holds while the emote
  is shown.
- `character_pose(state, quiet, state_s, dim, new_for_s, leaving_for_s)` is pure and returns
  direction, idle or walk, emote name or none, and a horizontal offset (for the walk-off). Frame
  numbers come from `anim_frame(now_ms, fps, frame_count)`, also pure.

### Drawing

- Integer scaling only, nearest neighbour (`pygame.transform.scale`, which does no smoothing).
- Rows: character at 3x (48 px) in a 64 px cell, emote at 2x (32 px) at the top right of the
  cell, overlapping the character's head. A 51 px row (5 rows) still fits the 48 px character. The
  approved 3x for rows means the bubble cannot sit above the head; this placement is the
  designer's to tune in T-052.
- Single card: the largest integer scale that fits the space above the text, at most 8x
  (`best_scale(avail_w, avail_h, 16, max_scale=8)`). On the 480 x 320 screen that is 8x (128 px),
  with the emote at 4x (64 px) above and to the right of the head.
- Animation: 5 fps (`CHAR_FPS`), inside the approved 4 to 6 fps. The dashboard runs at 3 fps
  today because a full 307 KB frame takes about 150 ms over SPI and faster redraws ghosted. So:
  1. The dashboard raises its rate to 5 fps only while the agents page is shown and at least one
     sprite is animating. Every other page stays at 3 fps.
  2. 200 ms per frame is above the 150 ms transfer, so a full flush should be clean. If T-047
     shows ghosting on the real panel, T-052 adds a partial flush: the screen is drawn in
     landscape and rotated, so the character column on the left of every row becomes one
     contiguous band of rows in the portrait framebuffer. Writing only that band is about a
     tenth of the frame.
  3. If both fail, fall back to 3 fps for sprites and report it (section 11, item 3).
- Cache: frames are cut with `subsurface` and scaled once per look and scale, only for looks in
  use, and dropped when no session uses that look. About 0.3 MB per look at 3x, so 16 looks stay
  under 5 MB on the Pi Zero 2 W.
- Loading uses pygame only (already a dashboard dependency). No Pillow on the Pi.

### Fallback: no pack, or a bad pack

- No folder, or an empty one: built-in icons. No log line beyond one "no character pack" at
  start.
- At start, `pi/sprites.py` loads each expected file and checks its size against the format. A
  file that fails to load or has the wrong size is skipped with one log line. If `NPC_1.png`
  fails, the whole pack is treated as absent.
- Any drawing error in the sprite path falls back to the icon for that row, inside the existing
  per-page try/except. A broken pack can never blank the agents page.

### Install: `--characters <zip>`

Folder: **`/var/lib/deskpane/characters/`**. Why this one and not `/opt/deskpane/characters/`:

- `/opt/deskpane/` holds releases, and `uninstall.sh` removes the whole of `/opt/deskpane`. The
  pack is not part of any release, so it should not live in the release tree.
- `/var/lib/<package>/` is the standard place for state that outlives upgrades. An upgrade never
  touches it, which gives "an upgrade keeps the copied pack" for free.
- The dashboard runs with `ProtectSystem=strict`, which keeps `/var/lib` readable and read-only.
  No unit change is needed. The folder is not a systemd `StateDirectory=`, because that would
  make it owned by the service user; here root owns it and the dashboard only reads it.
- Owner root:root, folder `0755`, files `0644`. The path is set by `CHARACTERS_DIR` in the
  dashboard unit's environment, default `/var/lib/deskpane/characters`.

`install.sh --characters <path>` (and `--remove-characters`):

1. Check the path is a regular file (not a symlink, not a device) and at most 4 MiB. The real pack
   is about 100 KB.
2. Run the validator: `python3 -I <release>/dashboard/charpack_install.py <zip> <target>`. It is
   stdlib only and never calls `unzip`. It reads the zip's entry list and builds a plan with the
   pure `plan_pack_install` from `charpack.py`.
3. The whole zip is refused if any entry: has an absolute path, a drive letter, a backslash, a
   NUL, or a `..` part; is a symlink (Unix mode in `external_attr`); is encrypted; appears twice;
   or if the zip has more than 64 entries. This covers zip-slip and symlink tricks even in
   entries we would skip.
4. Allowed entries are exactly `NPC_<1..15>.png` at the root and `Emotes/<Name>_emote_16x16.png`
   for the ten names. Each is read with a cap of 64 KiB (reading stops at cap plus one byte, so a
   zip bomb cannot fill memory), must start with the PNG signature and have the exact width and
   height in its IHDR header (128 x 208 or 64 x 16). Total copied at most 2 MiB.
5. Other entries are skipped and listed by count ("skipped 6 extra entries"), never by content.
6. Files are written under **fixed names from the allowlist**, never from the zip's own names,
   into `characters.new` next to the target. Then the old folder is renamed to `characters.old`,
   `characters.new` to `characters`, and `characters.old` is removed. Any failure before the
   swap removes `characters.new` and leaves the old pack in place.
7. The dashboard is restarted (install.sh already restarts it), so the pack loads.

- No `--characters`: the folder is left exactly as it is, on first install and on upgrade.
- `--remove-characters` removes only the folder; the dashboard falls back to icons.
- `uninstall.sh` keeps the folder; `uninstall.sh --purge` removes `/var/lib/deskpane`.
- `get.sh --characters <path>`: the zip must already be on the Pi (for example copied with
  `scp`). get.sh turns it into an absolute path and passes it to install.sh. It refuses the flag
  with `--reporter-only`, and refuses it with a clear message if the downloaded bundle's
  install.sh does not know the flag (older bundle), the same check it already does for
  `--no-summary`.
- `provision-pi.sh pi-zero --characters <zip>`: copies the zip to a temp file on the Pi over
  SSH, runs install.sh with it, removes the temp file. For the CEO's dev setup.

### Pure helpers and tests

All the logic is in `pi/charpack.py`, no pygame and no Pillow imports, tested in
`pi/tests/test_charpack.py` (list in section 8). Thin wrappers:

| Module | Pure or thin | Does |
|---|---|---|
| `pi/charpack.py` | pure | format constants, `frame_rect`, `look_count`, `look_for_key`, `assign_looks`, `character_pose`, `anim_frame`, `best_scale`, `png_size(header_bytes)`, `plan_pack_install(entries)` |
| `pi/charpack_install.py` | thin, stdlib | opens the zip, calls `plan_pack_install`, copies the planned files, swaps the folder |
| `pi/sprites.py` | thin, pygame | loads PNGs from `CHARACTERS_DIR`, cuts frames with `frame_rect`, scales and caches |
| `pi/dashboard.py` | thin | asks `charpack` what to draw and `sprites` for the surface |

Tests never need the paid pack: they build tiny PNGs (correct header and size, a few bytes of
image data) and zips with `zlib`, `struct` and `zipfile` inside the test.

## 11. Open items for the CEO (character packs)

1. **Extra files in the zip.** The brief says the zip should hold only the expected PNG names, but
   the real pack also has `Patrons.txt`, `Thanks.txt` and a `Palette/` folder. Refusing them
   would refuse the CEO's own pack. Approved 2026-11-08: refuse the whole zip on anything unsafe
   (zip-slip, symlinks, encryption, duplicates, too many entries), but skip harmless extra
   entries without copying them.
2. **`error` and the focus jump.** Approved 2026-11-08: `error` gets a red bar, the Sad emote and a
   count on the overview line, but no focus jump, because nothing is waiting on a click. An API
   error such as `rate_limit` usually clears on its own.
3. **Animation rate on the real panel.** The approved 4 to 6 fps is above today's 3 fps limit,
   which was set to stop ghosting. Approved 2026-11-08: 5 fps on the agents page only, with the
   partial band flush if T-047 shows ghosting, and 3 fps for sprites as the last fallback.
4. **Public screenshots.** Approved 2026-11-08: the README image and `docs/media/` never show the paid
   pack's art (built-in icons or a CC0 pack instead), as the license forbids redistribution.
