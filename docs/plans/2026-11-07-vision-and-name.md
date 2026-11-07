# Plan: vision, views and a new name (T-022)

Status: name and v0.1.0 rename approved by the CEO on 2026-11-07 (see Decisions); questions 4
to 7 stay open for later milestones. Owner: architect.

Repo state when written: `273cfe6` on `refactor/services-and-team`.

## 1. Vision

A small screen on your desk that shows what you, your machines and your AI agents put on it.
Today it shows live stats from the computers on your network. Next it shows your task list,
kept up to date by an AI agent, and a free canvas that you design in a web page.

Stats is view one of several. The project is a tiny display server, not a homelab monitor.

## 2. Architecture: from stats relay to display server

### What exists today

```
reporter (any machine) --UDP JSON Report--> daemon (Pi) --Unix socket, JSON array/sec--> pi/dashboard.py
```

- `daemon/src/contract.rs`: `Report`, `DeviceStatus`, `validate` and the limits.
- `daemon/src/device_table.rs`: capped, expiring table of the latest report per device.
- `services/`: `UdpSocketServer`, `Broadcaster` (tokio broadcast of JSON), `UnixSocketServer`.
- `pi/dashboard.py`: page 0 is an overview, then one page per device. A tap cycles pages.

### Where it goes

The daemon becomes the one owner of what the screen shows. It holds one state per view and
sends all of it to the dashboard. Producers never talk to the dashboard directly.

```
reporter ------UDP Report-------------\
                                       \
MCP bridge --HTTPS /api/tasks (token)---> daemon: ViewState { stats, tasks, canvas } --Unix socket--> dashboard
                                       /           persists tasks and canvas to disk
web portal --HTTPS /api/canvas (token)/
```

| View | Data | Who may write | Persisted |
|---|---|---|---|
| `stats` | `DeviceTable` (as today) | any reporter on the LAN (later: signed reports, T-006) | no, it is live data |
| `tasks` | `TaskList`, at most 20 tasks | holders of a write token with the `tasks` scope | yes, `/var/lib/<name>/tasks.json` |
| `canvas` | `Scene` (vector JSON, section 4) | holders of a write token with the `canvas` scope | yes, `/var/lib/<name>/canvas.json` |

### What stays

- UDP reports, `contract.rs`, `validate`, `device_table.rs`, the cap and expiry. Stats keeps
  working with no change for reporters.
- `Broadcaster` and `UnixSocketServer`: they already send "one JSON line" to every client.
- The Python dashboard as the only thing that touches the framebuffer and touch device.
- The rule that the daemon validates everything before storing it.

### What changes

1. **Socket line becomes an envelope.** Today each line is a JSON array of `DeviceStatus`.
   It becomes one object with every view:

   ```json
   {"v": 1,
    "focus": null,
    "views": {
      "stats":  {"devices": [ ...DeviceStatus... ]},
      "tasks":  {"rev": 7, "title": "Today", "tasks": [ ... ]},
      "canvas": {"rev": 3, "scene": { ... }}
    }}
   ```

   The daemon sends a line every second (stats needs it) and also right after a task or
   canvas change. `rev` lets the dashboard skip redrawing a canvas that did not change. This
   is an internal format: the daemon and dashboard ship in the same bundle with the same
   version, so changing it is not a breaking change for users.
2. **A control API.** One HTTP listener on the Pi (proposed port 8766) with `/api/tasks`,
   `/api/canvas` and the portal's static files. It is a new generic service
   (`services/http_server.rs`, a thin axum wrapper) plus project logic in `main.rs`, the same
   split as today. Every write needs a bearer token (section 5).
3. **Persistence.** Tasks and canvas are user data, so the daemon saves them under
   `StateDirectory=` (`/var/lib/<name>/`) and loads them at start. Stats stays in memory.
4. **View model in the dashboard.** Each view contributes pages to one ring:
   `[stats overview, device 1..n, tasks pages, canvas]`. A tap still moves to the next page;
   the page dots group by view. A view with no content contributes no page (no empty task
   page). `focus` lets a writer ask for a view to be shown now, for at most 300 seconds, for
   example after an agent updates the list. Any tap clears the focus.
5. **The dashboard gets one draw function per view** (`draw_tasks`, `draw_canvas`), each
   wrapped in the existing per-page try/except.

## 3. Task view over MCP

### Which side runs the MCP server

Recommendation for v1: **a stdio MCP server that runs on the agent's machine**, as a new
subcommand of the client binary (`<name> mcp`). The agent (Claude Code, Claude Desktop or any
MCP host) launches it like any local MCP server. It turns tool calls into HTTPS calls to the
Pi's control API, using a token from its environment.

Why not an MCP server on the Pi first:

- stdio is the transport every MCP host supports, and it needs no listener of its own.
- The current spec (revision 2026-07-28, checked on modelcontextprotocol.io) changed
  Streamable HTTP: no protocol sessions, no GET stream, new required headers
  (`MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name`). Older clients still speak the
  2025-03-26 to 2025-11-25 shape. Serving both from a Pi Zero is more surface than value
  for v1.
- The spec says a Streamable HTTP server **must** validate `Origin` (DNS rebinding),
  **should** bind to localhost when local, and **should** authenticate every connection. A
  LAN-facing MCP endpoint on the Pi would need all of that, plus the same token checks the
  control API already has.
- The Pi keeps one network write path (the control API) instead of two.

Later (optional): the daemon also serves `/mcp` (Streamable HTTP) behind the same token for
hosts that only take remote MCP servers. It reuses the control API code.

### Tools

| Tool | Arguments | Result |
|---|---|---|
| `list_tasks` | none | the current list with ids |
| `set_tasks` | `title?`, `tasks[]` | replaces the whole list |
| `add_task` | `title`, `due?`, `priority?` | the new task's id |
| `complete_task` | `id`, `done?` (default true) | the updated task |
| `remove_task` | `id` | ok |
| `clear_tasks` | none | ok |
| `show_tasks` | `seconds?` (1 to 300, default 60) | ok; sets `focus` |

Tool descriptions say plainly that the content appears on a physical screen.

### Task JSON and limits

```json
{"id": "t-3f9a", "title": "Renew the TLS cert", "done": false,
 "due": "2026-11-12", "priority": "high", "source": "laptop-agent"}
```

| Field | Rule |
|---|---|
| `id` | set by the daemon, 1 to 32 chars `[A-Za-z0-9_-]` |
| `title` | 1 to 80 bytes UTF-8, no control characters (same rule as report text today) |
| `done` | bool |
| `due` | optional `YYYY-MM-DD` |
| `priority` | optional `high`, `normal`, `low` |
| `source` | set by the daemon from the token's name, never from the request |
| list `title` | optional, up to 32 bytes |
| list size | at most 20 tasks; a request body at most 8 KB |
| rate | at most 10 writes per minute per token |

### How a remote agent connects

1. On the Pi: `sudo <name> token add laptop-agent --scope tasks` prints a token once and stores
   only its hash in `/etc/<name>/tokens` (0600 root).
2. On the agent's machine: install the client binary (same download as the reporter today) and
   add it to the MCP host's config, for example `command: <name>`, `args: ["mcp", "--pi",
   "192.168.1.50"]`, with the token in an environment variable.
3. The bridge pins the Pi's TLS certificate fingerprint on first use (section 5).

## 4. Canvas view

### What a design is

Recommendation for v1: **a vector JSON scene**, not a bitmap.

```json
{"bg": 0, "items": [
  {"t": "rect",   "x": 10, "y": 10, "w": 120, "h": 60, "fill": 3, "r": 8},
  {"t": "circle", "x": 240, "y": 160, "r": 40, "stroke": 5, "width": 3},
  {"t": "line",   "pts": [[0, 300], [480, 300]], "stroke": 1, "width": 2},
  {"t": "text",   "x": 20, "y": 200, "s": "Hello", "size": 2, "fill": 7}
]}
```

- Coordinates in the 480x320 screen space, integers only.
- Colors are indexes into a fixed 16-color palette. Text size is one of 3 sizes in the
  dashboard's own font.
- Limits: at most 256 items, at most 64 points per line, text at most 64 bytes, scene body at
  most 16 KB.

Why vector over a 480x320 bitmap:

- The daemon can validate every field, the same way it validates reports. A bitmap means an
  image decoder on the Pi, which is attack surface and CPU on a Pi Zero.
- 16 KB instead of 300 KB (raw RGB565) per update.
- Drawing is pygame primitives the dashboard already uses.
- A bitmap or image item can come later as a fixed-size 1-bit or 4-bit layer if people ask.

### Who hosts the portal

v1: **the Pi serves the portal itself**, as static files from the control API listener. A
simple editor page (shapes, text, palette) that posts the scene to `/api/canvas`. No build
step, no external service, no account. It works only on your LAN.

Later, sharing with other people (a hosted portal or invite links) needs a relay outside the
LAN. That is a different product decision with real cost and moderation work. It is not in
this roadmap.

### How a design reaches the Pi

Browser on the LAN -> HTTPS POST `/api/canvas` with a token cookie -> daemon validates and
saves the scene -> next socket line carries it with a new `rev` -> dashboard redraws.

### Moderation and auth: what must exist before canvas ships

"Anyone can draw on someone's screen" is the main risk. Before canvas ships:

1. Every write needs a token with the `canvas` scope. No anonymous drawing, even on the LAN.
2. Pairing: the portal shows a pairing code on the Pi's screen; the browser gets a token only
   after the code is typed in. This proves physical presence.
3. A way to clear the canvas from the device itself (long press on the canvas page) that works
   even if the network is hostile.
4. Size limits and a rate limit (at most 6 writes per minute per token).
5. The Pi never fetches anything a design points at: no image URLs, no fonts, no links.
6. Revoking a token from the Pi (`<name> token remove <name>`) takes effect at once.
7. For any future sharing beyond the owner: designs from others go to a pending queue and the
   owner approves each one on the screen. Not in v1.

## 5. Security compared with today

Today (`SECURITY.md`): LAN only, no authentication, no encryption. The only network input is
UDP stats, and the worst a bad sender can do is show false numbers or fill the 32-device table.

The new views add **write paths of free text and drawings onto a physical screen**, plus a
TCP/HTTP listener on the Pi. That changes the threat model:

| Risk | Today | With tasks and canvas |
|---|---|---|
| Who can change the screen | anyone on the LAN, numbers only | must be token holders only |
| Content | numbers and short names | free text and drawings (abuse, fake "call this number" messages) |
| Listener | UDP only | HTTP(S) listener: needs `Origin` checks, CSRF protection for the portal, body limits |
| Secrets on the wire | none | a bearer token over plain HTTP on Wi-Fi can be sniffed |
| Data at rest | none | tasks and canvas files under `/var/lib/<name>/` |

What must land first:

- **T-006 token auth lands before the task view ships.** Tokens with names and scopes, stored
  hashed, revocable, checked by the control API. This is not optional for any write path.
- **TLS on the control API ships with the task view.** Recommended: the installer makes a
  self-signed certificate for the Pi; the MCP bridge pins its fingerprint on first use; the
  browser shows a one-time warning for the portal. The private CA in `../pi-homelab-ca` is the
  upgrade path to proper mTLS (the planned TCP+mTLS transport).
- **HMAC on UDP reports** (the other T-006 item) is not forced by these features. Stats stays
  read-only display data. It can follow.
- `SECURITY.md` must be rewritten when the control API ships: what tokens protect, what they
  do not, and that the device is still for a trusted LAN.

## 6. Roadmap

| Milestone | Scope | Out of scope |
|---|---|---|
| **v0.1.0** | stats as it is today, under the new name. The rename only (section 8). | any new feature, socket envelope, HTTP listener |
| **v0.2.0: foundation** | socket envelope with `stats` only, dashboard view ring, token store and `token add/list/remove` (T-006), HTTP control API with TLS, read-only `GET /api/state`. Rewrite `SECURITY.md`. | tasks and canvas content |
| **v0.3.0: task view** | `TaskList` with persistence and limits, `/api/tasks`, the `<name> mcp` stdio bridge with the tools in section 3, task pages on the dashboard, `focus`. | remote `/mcp` endpoint on the Pi |
| **v0.4.0: canvas view** | `Scene` validation and persistence, `/api/canvas`, the portal on the Pi, pairing code, clear-by-long-press. | sharing beyond the owner, bitmaps |
| later | remote Streamable HTTP `/mcp`, signed UDP reports, TCP+mTLS reporters, shared canvas with an approval queue | |

Task view comes before canvas: it is smaller (text only, no editor UI), it reuses the report
validation rules, and the MCP bridge proves the token and TLS foundation that canvas also
needs.

## 7. Name

Brief: "a small screen that shows what you or your agents put on it". Not homelab-only, not
Pi-only (the client runs on any machine).

Checked on 2026-11-07: GitHub repository search (`gh api search/repositories?q=<name>+in:name`),
GitHub user or org with that exact login (`gh api users/<name>`), crates.io
(`/api/v1/crates/<name>`, 404 means free) and PyPI (`/pypi/<name>/json`, 404 means free). A
web search for products with the same name.

| Name | Rationale | GitHub repos (top by stars) | GitHub login | crates.io | PyPI |
|---|---|---|---|---|---|
| **deskpane** | a pane of glass on your desk that shows what you put on it | 11 hits; exact name: `0obriano0/DeskPane` (3 stars, a web window manager), `ben-denham/deskpane` (1 star, demo) | free | free | free |
| sillpane | a pane on the window sill; quiet, ambient | 0 hits | free | free | free |
| tinypane | says "small screen" directly | 4 hits, none exact (TinyPanel repos, 1 star max) | free | free | free |
| postpane | a pane you post things to, like a fridge note | 3 hits, none exact (0 stars) | free | free | free |
| cornerpane | the screen in the corner of your desk | 2 hits; a `cornerpane` user with one repo (0 stars) | taken (user) | free | free |
| screenling | a little screen creature that you and your agents feed | 29 hits, near names (ScreenLingo), a `screenling` user | taken (user) | free | free |

Rejected while checking: `glancepad` (a macOS app called GlancePad exists, and Glance is a
popular dashboard project), `pebbleboard` and `nookboard` (Pebble and Nook are trademarks),
`placard` (taken on crates.io and PyPI, 651-star repo), `wallpane` (close to the popular
`wallpanel` projects).

**Recommendation: `deskpane`.** It says what it is in one word, it is not tied to a homelab or
to a Raspberry Pi, and "pane" fits every view (stats, tasks, canvas). The crate, PyPI and
GitHub login are free; the two same-name repos are tiny and in other domains. The web search
found no product called DeskPane. Runner-up: `sillpane` (completely unused, but the meaning
is less obvious).

Proposed names under `deskpane`: repo `<owner>/deskpane`, binaries `deskpane-daemon` (Pi) and
`deskpane` (client CLI: `deskpane report`, later `deskpane mcp`, `deskpane token`).

## 8. Rename impact

Counts from `git grep -c` at `273cfe6`. "pi-homelab" appears 146 times in 27 files, "homelab"
(any case, outside `docs/plans/`) 160 times in 26 files.

| Item | Today | Proposed | Where (count) | When |
|---|---|---|---|---|
| Repo name | `pi-homelab-dashboard` | `deskpane` | `.github/ISSUE_TEMPLATE/config.yml` (2), README, docs links | now (no public repo yet) |
| Project name in docs | pi-homelab-dashboard | deskpane | `docs/INSTALL.md` (33), `README.md` (9), `SECURITY.md` (5), `CLAUDE.md` (5), `CONTRIBUTING.md` (3), `docs/diagram/*.html` (4 + 4), `docs/CONTRACT.md` (1), `.github/ISSUE_TEMPLATE/bug_report.md` (4 homelab) | now |
| Install root | `/opt/pi-homelab-dashboard` | `/opt/deskpane` | `deploy/install.sh`, `deploy/uninstall.sh`, 3 units, `SECURITY.md`, `docs/INSTALL.md` | now |
| Config dir | `/etc/pi-homelab-dashboard` | `/etc/deskpane` | `deploy/install.sh`, `deploy/uninstall.sh`, `deploy/env.example` (2), units | now |
| Socket | `/run/pi-homelab-dashboard/daemon.sock` | `/run/deskpane/daemon.sock` | `homelab-daemon.service`, `homelab-dashboard.service` | now |
| Socket default in code | `/tmp/pi-homelab-dashboard.sock` | `/tmp/deskpane.sock` | `daemon/src/main.rs` (1), `pi/dashboard.py` (1), README (1) | now |
| Service user | `pi-homelab` | `deskpane` | `deploy/install.sh`, `deploy/uninstall.sh`, 3 units, `SECURITY.md` | now |
| Units | `homelab-daemon`, `homelab-dashboard`, `homelab-reporter`, `unbind-fbcon` | `deskpane-daemon`, `deskpane-dashboard`, `deskpane-reporter`, `deskpane-unbind-fbcon` | file names in `deploy/systemd/`; references: `homelab-daemon` 24, `homelab-reporter` 19, `homelab-dashboard.service` 9, `unbind-fbcon` 23 (mostly `docs/INSTALL.md` and `deploy/install.sh`) | now |
| config.txt markers | `# >>> pi-homelab-dashboard` | `# >>> deskpane` | `deploy/install.sh`, `deploy/uninstall.sh` | now |
| Pi bundle | `pi-homelab-dashboard-pi-aarch64.tar.gz` | `deskpane-pi-aarch64.tar.gz` | `pi-aarch64` 23 refs: `scripts/package-pi-bundle.sh` (3), `scripts/provision-pi.sh` (3), `release.yml`, `ci.yml`, `docs/INSTALL.md` (7), `deploy/README.txt` (2), README, CLAUDE.md, CONTRIBUTING.md | now |
| Reporter archives | `reporter-linux-x86_64.tar.gz` etc. | `deskpane-linux-x86_64.tar.gz` etc. | 20 refs: `release.yml` (3), README (2), `docs/INSTALL.md` (7), plans | now |
| Binary names | `daemon`, `reporter` | `deskpane-daemon`, `deskpane` | `[[bin]]` or package names in `daemon/Cargo.toml`, `reporter/Cargo.toml`, units `ExecStart`, `package-pi-bundle.sh`, `release.yml`, docs | now: `daemon` and `reporter` are generic names on a user's PATH (Debian even has a `daemon` package) |
| Crate names | `daemon`, `reporter` | `deskpane-daemon`, `deskpane-cli`, or keep | `Cargo.toml` (2), `Cargo.lock` (2), `use daemon::` (9 lines in .rs), release-please `extra-files` jsonpaths (2) | optional; renaming binaries via `[[bin]] name` is enough for v0.1.0 |
| CLI shape | `reporter run --daemon ...` | `deskpane report --daemon ...` | `reporter/src/main.rs`, units, docs | decide now (open question 3); cheapest before anyone scripts it |
| release-please package | `pi-homelab-dashboard` | `deskpane` | `release-please-config.json` (1) | now |
| Log prefixes | `daemon:`, `reporter:`, `dashboard:`, `udp:` | keep | `daemon/src/main.rs` (9) and others | keep: they name components, not the product |
| Directories | `daemon/`, `reporter/`, `pi/` | keep | | keep for now; renaming `pi/` to `dashboard/` is cosmetic |
| Env vars | `LISTEN_ADDR`, `SOCKET_PATH`, `PROTOCOL`, `DEVICE_ID`, `LABEL` | keep | | keep: generic, no product name |
| UDP port | 8765 | keep | | keep |
| Private CA repo | `../pi-homelab-ca` | keep | `CLAUDE.md`, `README.md`, `docs/CONTRACT.md` | keep: private and separate |
| Plans | old names in `docs/plans/*` | keep | 2026-11-07 plans | keep: history |

Rename now, all at once, in one `feat!:` or `refactor!:` commit before v0.1.0: nobody has
installed the bundle yet, so no migration code is needed. After v0.1.0 every path, user and
unit rename needs an upgrade step in `install.sh`.

## Decisions (CEO, 2026-11-07)

1. Name: `deskpane`. Repo: `nokri-mohammed/deskpane`.
2. Binaries: `deskpane-daemon` (Pi) and `deskpane` (client), set with `[[bin]] name`; crate and
   directory names stay `daemon` and `reporter`.
3. CLI: `deskpane report` replaces `reporter run`, with the same flags (`--daemon`,
   `--device-id`, `--label`, `--interval`).
4. The rename ships in v0.1.0 as ticket T-023, rename only. Questions 4 to 7 below stay open for
   later milestones.

## 9. Open questions for the CEO

1. Name: `deskpane` (recommended) or another from the shortlist? Who owns the repo
   (`nokri-mohammed/deskpane`)?
2. Rename binaries in v0.1.0 to `deskpane-daemon` and `deskpane` (recommended), or keep
   `daemon` and `reporter` until later?
3. CLI shape: change `reporter run` to `deskpane report` now (recommended, before anyone
   scripts it), or keep `run`?
4. Is the task view LAN-only for good, or should an agent outside the home (a cloud agent)
   reach it one day? That decides whether we ever need a relay.
5. TLS approach for v0.2.0: self-signed with fingerprint pinning (recommended), or the private
   CA from `../pi-homelab-ca` from the start?
6. Canvas: owner-only on the LAN for v0.4.0 (recommended). Is sharing with other people a goal
   at all? It brings moderation work and a hosted service.
7. Should the MCP bridge live in the same client binary as the reporter (recommended, one
   download per machine) or be its own binary?
