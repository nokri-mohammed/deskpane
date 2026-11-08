# Feature ideas

Ideas for deskpane that are not planned yet. Add an idea here first. When the owner picks one,
it gets a plan in `docs/plans/` and tasks in the backlog, and its status changes here.

Status: **idea** (written down), **designing** (a plan is being written), **planned** (plan
approved), **building**, **shipped**, **dropped** (with the reason).

| # | Idea | Status | Value | Effort | Score | Needs first | Plan |
|---|---|---|---|---|---|---|---|
| 1 | Agent status view | building (characters are done) | 5 | 2 | **2.5** | nothing | [2026-11-08-agent-status-view.md](plans/2026-11-08-agent-status-view.md) |
| 5 | Hide or remove a device from the screen | idea | 3 | 2 | **1.5** | nothing | |
| 3 | MCP task list | idea | 4 | 3 | **1.3** | best after 1 (same screen page and client) | [vision plan](plans/2026-11-07-vision-and-name.md) |
| 6 | Agent details page (stats per agent) | idea | 4 | 3 | **1.3** | after idea 1 ships (same packet, table and page ring) | |
| 2 | Settings portal on the Pi | idea | 4 | 4 | **1.0** | nothing, but unlocks authentication for all the others | |
| 4 | Free canvas from a web portal | idea | 3 | 5 | **0.6** | 2 (pairing and keys) and a public service | [vision plan](plans/2026-11-07-vision-and-name.md) |

### How the score works

Score = Value ÷ Effort, so a high score means a lot of benefit for little work. The table is
sorted by score. Re-score an idea when we learn more (for example after its plan is written).

- **Value (1 to 5):** how much it improves daily use or makes deskpane stand out. 5 = you
  would use it every day and people would share it.
- **Effort (1 to 5):** how much work across Rust, Python, docs and security. 1 = a day,
  3 = a beta, 5 = several releases or a new service.
- **Needs first:** what must exist before it. A low score can still go early if it unlocks
  others (idea 2 brings the keys that make the rest safe on a shared network).

Why each idea got its numbers:

1. **Agent status (5 / 2):** daily use for you, unique to deskpane; hooks already exist in
   Claude Code, so it is a client command, a packet kind and one screen page.
5. **Hide a device (3 / 2):** fixes a real annoyance; small, but the dashboard needs a way to
   tell the daemon, which today only talks one way.
3. **MCP task list (4 / 3):** strong value; reuses the agent page and client from idea 1, plus
   an MCP server.
2. **Portal (4 / 4):** many small wins and the base for authentication, but a web server,
   pairing and a settings store make it the biggest Pi-side job.
4. **Canvas (3 / 5):** nice to show off, but needs a public web service and authentication
   first.

## 1. Agent status view

A page on the screen that shows the AI agents working on your computers: which project, what
each one is doing, and whether it is **working**, **done** or **needs you**.

- Claude Code hooks send the status automatically, so the agent does not need to know about
  the Pi. A hook runs a small command when something happens (a prompt is sent, a tool runs,
  the agent stops, the agent asks for permission).
- The screen can also show how long the agent has been running and how full its context is.
- "Needs you" is the most useful state: you see at a glance which session is waiting.
- Status: building. The daemon, the hook client, the screen and the optional character packs
  are built. Character packs are done. The real-device checks are left (T-047).

## 2. Settings portal on the Pi

A small web page served by the daemon on your home network, opened from a phone or laptop.

- Pairing instead of a password: the screen shows a 6 digit code, you type it in, and that
  browser is trusted.
- Personal settings: device labels, hide or remove devices, clock format, page order,
  brightness.
- Communication settings: turn on authenticated reports, give keys to reporters (QR code or
  short code), later choose the connection type (UDP, TCP with TLS).
- Built in stages: read-only status page first, then pairing and settings, then communication
  settings.
- Private to the home network. It is not the public canvas portal (idea 4).

## 3. MCP task list

An MCP server with tools such as `add_task`, `complete_task` and `set_progress`. An agent calls
them when it decides to, and its to-do list shows on the screen.

## 4. Free canvas from a web portal

People design a screen in a web page, and the design appears on the Pi. Needs a public
service, so it comes after authentication.

## 5. Hide or remove a device from the screen

Close a device you no longer want to see, instead of waiting 5 minutes for it to be
forgotten. Could be a long press on the screen or a switch in the portal (idea 2).

## 6. Agent details page

Tap an agent row to open a page with stats about that one session, like the device pages do
for a computer.

- From hooks (numbers only, no content): how long it ran, time spent working versus waiting for
  you, tools used (count and the top few by name), subagents started, prompts sent, errors.
- From the status line (needs a small bridge from the user's status line script to
  `deskpane agent`, designed as T-048): model name, context used in percent and tokens, lines
  added and removed. The 5 hour limit is per account, not per agent, so it belongs on the
  overview page.
- Never sent: prompts, file names, commands, tool input or output, messages.
- Extends the agent packet with optional numeric fields, so old daemons and clients keep working.
