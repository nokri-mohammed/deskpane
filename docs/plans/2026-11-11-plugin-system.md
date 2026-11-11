# Plugin system

Date: 2026-11-11. Author: architect. Status: APPROVED 2026-11-11 (CEO said yes to Q1
declarative-first and Q7 merge-gated with no runtime sandbox; Phase 0 greenlit). Worked example
added 2026-11-11; security-specialist review folded in 2026-11-11, see `docs/SECURITY.md` and the
Phase 2 security done-bars.

Ticket: T-067. This is a shape-and-direction plan, not a detailed design. It says what a plugin
is, where it runs, what it may touch, how a contributor builds and tests one, how we review and
ship it, and in what order we build all of that. It deliberately phases the risky parts (a
runtime sandbox, a plugin store) to the end and marks them clearly.

## The decision this plan is built around

The CEO has decided the trust model, and the whole plan follows it:

- deskpane is an open-source project. Contributors (close friends first, strangers later) add
  plugins by **pull request**. The CEO reviews and merges. Merged plugins ship inside the
  official release.
- This is **merge-gated trust**: the primary wall is CEO review plus CI plus tests, not a
  runtime sandbox. A plugin that reaches a Pi has already been read by a human and passed CI.
- The risky parts are **later and clearly marked**: (a) a runtime WASM sandbox with a capability
  model, needed only when plugins can be **side-loaded** (installed on a Pi without the CEO's
  merge); (b) a plugin **store or registry**. v1 needs neither.
- Even inside the merge model we keep defense in depth: a **narrow host API** from day one, CI
  that bans dangerous calls (network, filesystem, process spawn) in plugin code, and plugins
  that **declare** the data and capabilities they use. The narrow API is the thing that lets the
  WASM sandbox slot in later without a rewrite.

## The grain of the codebase this plan leans on

deskpane already has the seam a plugin system needs. The architecture splits **data** from
**render** across two languages:

```
reporter / any sender --UDP JSON--> daemon (Rust, tokio) --Unix socket envelope/sec--> dashboard (Python, pygame)
       data source                    validate + tables                    render
```

- The **data plane** is the Rust daemon. It takes one JSON object per UDP datagram, validates it
  hard (`validate`, `validate_agent` in `daemon/src/contract.rs`), keeps a capped table per kind
  (`device_table.rs`, `agent_table.rs`, ownership by source IP, eviction, expiry), and broadcasts
  one envelope line a second on the Unix socket (`{"v":1,"views":{"stats":..,"agents":..}}`).
- The **render plane** is the Python dashboard. `pi/dashboard.py` reads the envelope and draws a
  fixed ring of pages: overview, agents pages, one page per device. `pi/layout.py` already holds
  the **pure** layout logic (no pygame), unit-tested, separate from the drawing. That split is
  the template for a panel plugin: pure "where things go" plus a small set of draw primitives.
- The **contract** (`docs/CONTRACT.md` and `daemon/src/contract.rs`) is the fixed interface
  between the two planes, versioned, with a compatibility rule (unknown fields ignored, new
  fields `#[serde(default)]`, breaking changes are `feat!`; see T-014).
- There is already a **local preview** (`pi/preview.py`, `pi/mock_daemon.py`,
  `pi/preview_scenarios.py`, `scripts/preview.sh`) that runs the real dashboard on a PC against
  mock scenario data or a real loopback stack, in a window or as headless PNG frames on a
  480x320 panel. This is the plugin test harness, already built.
- There is already a **licensed, git-ignored, validated asset system**: character packs
  (`docs/CHARACTERS.md`). A pack is a zip, installed with `--characters` into
  `/var/lib/deskpane/characters`, strictly validated (size caps, path safety, no symlinks),
  loaded at runtime, never committed, guarded by `scripts/check-no-pack-files.sh` in CI and by
  the bundle script. This is the template for plugin packaging and licensing.

So most of the machinery exists. The plugin system is mostly about **naming the seams, making
deskpane's own panels ride them, and opening them to outside contributions safely**.

## 1. What a plugin is: two kinds by trust and difficulty

A plugin is one of two kinds. They differ sharply in risk, so they are built in that order.

### 1a. Data-source plugins (safe, do first)

A data-source plugin is **a program or script that emits contract-shaped JSON** to the daemon,
exactly the way a reporter already does. It runs as its **own OS process** (on any machine on
the LAN, or on the Pi), sends UDP datagrams to port 8765, and shows up on the screen.

Why this is the safe kind: **no plugin code runs inside deskpane**. The daemon treats every
packet, from the reference reporter or from a third-party plugin, as untrusted input and runs it
through the same wall: parse, `validate`, cap the table, ownership by source IP, expiry. A
hostile data source can at worst send junk (dropped) or squat an id (already handled, T-006 for
auth later). It can never read the Pi's disk or run code on it, because it is a different
process on (usually) a different machine.

The reference reporter is simply the first data-source plugin. `deskpane report` and
`deskpane agent` already prove the pattern: two kinds of sender (`kind` absent = report,
`kind:"agent"` = agent), both validated, both capped, both shown.

**What v1 allows for data sources:** a data-source plugin sends the **existing** packet kinds
(a `Report`, or an agent packet) or a **new, narrow packet kind** reserved for panel data (see
section 2). It ships as a small program plus a manifest. The language is the contributor's
choice, because the wire format is JSON over UDP, not a linked API; the SDK provides the contract
types and a send helper in Rust and Python so most people do not start from scratch. Recommended
default for v1: data-source plugins extend the existing kinds, and a generic "custom metric"
packet kind is added only when a real plugin needs data that no existing field carries.

### 1b. Render / panel plugins (harder, where the untrusted-code risk lives)

A render plugin **draws a page (a panel) on the screen**. This is the hard kind, because render
runs **inside the Python dashboard process on the Pi**. Whatever a panel can do, the dashboard
process can do, and that process today imports `subprocess`, `socket` and the filesystem. A panel
is code (or data interpreted by code) with access to the Pi, so this is where the real trust
question lives.

Two ways to let outsiders add a panel:

- **(A) Python modules loaded by the dashboard.** Easy and powerful: a contributor writes a
  `draw(...)` function with full pygame and gets pixel-perfect control. The cost: it is arbitrary
  Python running in the dashboard process. In the merge model, **code review is the only wall**
  between that module and the Pi's filesystem and network. A later version of a trusted plugin
  can turn malicious and there is no runtime stop.
- **(B) A declarative panel format.** A panel is a **description** (text at x,y bound to
  `cpu_pct`, a ring gauge bound to a memory ratio, a bar, an icon, a sprite from the pack), in
  JSON or TOML, interpreted by a **fixed renderer** that deskpane owns. Safer: the contributor
  ships data, not code, so there is no filesystem or network reach to review. Less powerful: only
  what the primitive set and the binding expressions allow.

**Recommendation for v1: declarative-first, with Python-module panels reserved for deskpane's
own core panels and a very small set of heavily reviewed core contributions.** Reasons:

1. It keeps the **host API honest and narrow from day one** (section 2). A declarative panel can
   only name a primitive and a data field; it cannot call anything. That is exactly the surface a
   WASM sandbox would later enforce, so declarative panels need **no sandbox, ever**, and the
   general contributor path stays safe by default.
2. It matches the grain of the code. `pi/layout.py` already computes "where things go" as pure,
   tested functions returning plain geometry; `dashboard.py` turns that geometry into draw calls.
   A declarative panel is the same split, generalised: a manifest-driven layout plus a fixed
   primitive renderer.
3. It is the 80% case. Most panels people want are "show these numbers as a gauge, a bar, a line
   of text, a colour that crosses a threshold". The declarative format covers that without any
   code review burden beyond "does the manifest say what it does".
4. The powerful 20% (a genuinely custom drawing) stays possible through a **Python-module panel**,
   but that path is **the exact code that triggers the WASM requirement** the day side-loading is
   wanted (section 7). Keeping it rare and core-only keeps that trigger small.

So: outside contributors get the declarative format (safe, reviewable at a glance, sandbox-ready).
deskpane's own overview, device and agents pages ride the same registry as Python modules **only
as a sunset**, not an open door: every in-process Python panel is arbitrary code in the dashboard
process with filesystem and network reach, defended only by review, which is the exact weak wall
this plan argues against. The direction is therefore fixed: **new panels are declarative**; the
three existing Python pages stay through Phase 0 as migration debt, with the stated intent to
re-express them in the declarative format so the dashboard process eventually loads no panel code
at all. "Pulling a contribution into core" must not become a back door for third-party Python; a
code panel is a large review and a core-ownership decision, never a routine merge.

## 2. The contract and host API: the narrow fixed surface

A plugin may use only a small, fixed, versioned surface. Keeping it narrow is the whole game: it
is both the defense-in-depth wall now and the sandbox boundary later.

**Data a panel may read (read-only):** named fields from the current socket envelope. The
`DeviceStatus` fields (`cpu_pct`, `mem_used_mb`, `mem_total_mb`, `disk_*`, `temp_c`, `uptime_s`,
`battery_pct`, `charging`, `online`, `local`, label, hostname, ip) and the agent session fields.
A panel declares which it reads; the renderer passes it only those. Nothing else: no environment,
no files, no clock beyond the frame time.

**What a panel may draw (into its own rect only):** a fixed primitive set, matching what the
dashboard already draws. Text (string, position, size role, colour token), rectangle, line,
circle, arc (the ring gauge), filled bar, an icon from the built-in set, a sprite from the
installed character pack. Colours are **tokens** (`ACCENT`, `WARN`, `CRIT`, `TEXT`, `SUBTEXT`,
`BG`), not arbitrary RGB, so every panel stays inside the house palette and the dark theme. The
panel draws into a given rect (its page or its slot); it cannot touch the rest of the screen.

**Timers and animation:** the renderer passes the frame time (`now`, `t0`), as the dashboard does
today for the pulse dot and the character animation. A panel is a **pure function of (data, rect,
now)**, like `layout.py`. No `sleep`, no real clock, no threads: the panel cannot block the 3 fps
draw loop.

**An allowlisted HTTP GET for data sources only.** Some panels want data from the network (a
weather line, a CI status). That network call is **never** made by render code. A **data-source
plugin** declares an allowlisted host and path; a fetcher (in the daemon or a sidecar process,
not in the dashboard) performs the GET, parses the result into a contract packet, and sends it in
over the normal path. The panel then just reads the resulting field. Network stays entirely out
of the render plane and entirely inside the validated data plane.

**Versioned and backward compatible.** The manifest carries a `contract_version` and a
`host_api_version`. The same compatibility rule the contract already follows applies (unknown
fields ignored, new primitives and fields added, never removed or repurposed inside a major; a
breaking change is a major bump and a `feat!` commit; T-014). A plugin that names a host API
version the installed deskpane does not support is refused at load with one log line and its page
is skipped, the same way a bad character pack is skipped today.

**What is never in the host API:** filesystem access, process spawn, arbitrary network, arbitrary
imports, unbounded loops, raw framebuffer access, reading other panels' data. These are the exact
things the CI banned-call scan enforces (section 4) and the exact things the WASM capability model
would enforce later (section 7). They line up on purpose.

## 3. The SDK and developer experience

This is the part that decides whether contributors show up, so it gets real weight.

**`plugins/` directory, one folder per plugin.** Each plugin is a self-contained folder:

```
plugins/<name>/
  manifest.toml        name, author, version, contract/host-api versions, kind, declares, license
  panel/               a declarative panel file (and/or, core only, a reviewed Python module)
  source/              a data-source program (optional), any language that can send UDP JSON
  tests/               the plugin's own tests (pure panel layout, data-source output shape)
  README.md
  LICENSE
```

**The SDK is three small things, mostly already in the tree:**

1. **The contract types.** Extract the shared wire types out of `daemon/src/contract.rs` into a
   small `deskpane-contract` crate the daemon and reporter already depend on, and publish a mirror
   `deskpane_contract` Python module (the shapes `pi/layout.py` already parses). A plugin author
   gets the types, the limits and `validate` for free, in the language they send from.
2. **Helper functions.** A UDP send helper (the reporter already has `UdpClient` and
   `send_datagram`), a declarative-panel schema plus a validator, and the colour tokens and
   primitive list as data. Small surface, no framework.
3. **The local preview as the test harness (the key piece, already built).** A contributor runs
   their plugin on the **mock 480x320 screen with scenario data, with no Pi**:

   ```
   scripts/preview.sh --plugin plugins/<name> --scenario busy
   scripts/preview.sh --plugin plugins/<name> --headless --serve 8780   # live frames in a browser
   ```

   `preview.py` already loads the real `dashboard.py` against `mock_daemon.py` scenario lines. The
   new `--plugin` flag registers the plugin's panel into the page ring and, for a data-source
   plugin, feeds a scenario that includes its data. The contributor sees exactly what the Pi would
   show, iterates in a window, and saves a PNG to attach to the PR.

**A plugin template.** `plugins/example-metric/` ships as a working, minimal plugin: a data source
that sends one custom number and a declarative panel that draws it as a ring gauge, with tests.
`scripts/new-plugin.sh <name>` copies it and rewrites the manifest.

**CONTRIBUTING guide section and a "write your first plugin" walkthrough:** scaffold from the
template; edit the manifest (name, author, license, declared data); add a data source that sends
a metric; add a declarative panel that reads it; `scripts/preview.sh --plugin ...` to see it;
`python3 -m unittest` / `cargo test` for the plugin's tests; attach a preview PNG; open the PR.
The whole loop is on the contributor's PC.

## 4. CI gates and the CEO's merge/review checklist

Merging a plugin PR must be a **routine, not a gut call**. CI does the mechanical checks; the
human checklist is short.

**CI auto-checks on every plugin PR (all machine-decidable):**

- **Builds.** Rust data sources build (`cargo build --locked`); the panel file parses against the
  declarative schema; any Python (core panels) byte-compiles.
- **Tests.** Every `plugins/<name>/tests/` runs in CI, Rust and Python, same runners as today.
- **Lint.** `cargo fmt --check` and `clippy -D warnings` for Rust; a Python linter for any core
  Python panel; `rust-patterns` style on Rust in the repo.
- **Banned-call scan (defense in depth, not a wall).** A deny scan over plugin code for network
  (`std::net`, `reqwest`, Python `socket`, `urllib`, `requests`), filesystem writes/reads outside
  the plugin, process spawn (`std::process`, `subprocess`, `os.system`), dynamic code (`eval`,
  `exec`, `__import__`, `ctypes`), and raw framebuffer access. A declarative panel has no code, so
  it passes trivially. Be honest about what this scan is: a static deny list catches honest
  mistakes and lazy attackers, but a determined contributor evades it (string-built calls, reflection,
  `unsafe` or FFI, a `build.rs` or proc-macro that runs code at compile time, or a transitive
  dependency that brings the banned call in a crate the grep never reads). So the scan **reduces
  what CEO review must catch; it does not replace it**, and it is backed by hard dependency rules
  below.
- **Dependency and `unsafe` rules for Rust data sources (what makes the scan mean anything).** A
  Rust data source must carry `#![forbid(unsafe_code)]`, commit its `Cargo.lock` (which is diffed
  on every update), build with `--locked`, and draw only from a small vetted dependency allowlist;
  crates with a `build.rs` or proc-macro are denied unless explicitly whitelisted. `cargo-deny` and
  `cargo-vet` run in CI. Without a dependency allowlist, "no `std::net` in the plugin's own file"
  proves nothing, because the banned call can live one crate down.
- **Manifest valid.** Schema-checked: required fields present, versions in range, `kind` matches
  what the folder contains, license present and on the allowlist.
- **Declared matches actual (enforced at ground truth, not grepped).** The `declares` block is
  meaningful only where it is runtime-enforced or checked against observed behaviour, never inferred
  from source text alone. For data: the declared `publishes`/`reads` are validated against the
  actual packet the preview test harness observes the plugin emit. For panels: the schema parser
  checks every `bind` and `threshold.field` is listed in `reads`. For capabilities: the SDK is the
  only fetch path and `allowlisted_get` honours only the declared hosts at runtime, so an undeclared
  host cannot be reached even if a scan missed it. An undeclared read or capability fails CI.
- **Size and memory budget.** The plugin folder is under a size cap; the manifest's declared
  per-plugin memory and draw-cost budget is within the global budget (section 5); no committed
  binary or pack asset (reuse the `check-no-pack-files.sh` pattern).
- **No secrets or private data.** Scan for real IPs, hostnames, SSIDs, tokens (the repo already
  forbids these).

**The CEO's human review checklist (short, after CI is green):**

1. Does the plugin do what the manifest and README say, and nothing else?
2. Does it read only the data it declares? Any new capability justified?
3. For a code panel only: read every line; any surprise (a loop, a string built into a call, an
   import) is a block.
4. Is the attached preview screenshot sane, on-palette, and does it fit 480x320?
5. Are the tests meaningful, not empty?
6. Is the license compatible and the authorship honest?
7. Anything that changes what every user gets (a new default panel in the ring, a bundled asset's
   license) is flagged as a top-level decision, not merged quietly.

## 5. Plugin packaging and the manifest

**Manifest (`manifest.toml`):** `name`, `author`, `version`, `contract_version`,
`host_api_version`, `kind` (`data-source` | `panel` | `both`), `declares` (the data fields read
and the capabilities used: `http_get` hosts, pack art, timer), `license`, and the entry points
(the panel file, the data-source binary). This is the single source the CI checks and the loader
reads.

**Licensing, with the character-pack precedent.** Code and declarative panels are open source and
committed under `plugins/`. **Some assets must never be committed**, exactly as with character
packs: a paid or non-redistributable asset (sprites, fonts) is git-ignored, installed separately
with its own `--...` flag, validated the way packs are, and guarded by CI. A plugin that needs
such an asset declares it and ships only the code plus install instructions, never the asset.

**How a plugin ships in the release.** Merged plugins live in `plugins/` in the repo.
`scripts/package-pi-bundle.sh` vendors the selected plugins into the Pi bundle (declarative panels
as data; Rust data sources as extra binaries or optional systemd units). The bundle stays within
the licence guard (no pack/asset files). Signing and provenance of the whole bundle is Phase 3
(folds in T-031).

**How the Pi loads plugins at runtime, within 512 MB.** The dashboard builds its **page ring from
a registry of installed panel manifests** instead of the hardcoded ring in `page_ring`. At start
it reads each installed manifest, checks the host API version, and adds the panel to the ring;
a panel that fails to load is skipped with one log line (the character-pack failure model). Budget:

- **Declarative panels** cost almost nothing: they are data interpreted by one renderer, no new
  process, no new import. This is why they are the default.
- **Rust data-source plugins** run as **separate OS processes** (their own systemd unit or a
  reporter the user runs), isolated by the OS, counting against the existing device/session table
  caps. They do not touch the dashboard's memory.
- **Python code panels** (core only) run **in the dashboard process**, so they share its memory
  and must carry a declared memory and draw-cost budget, enforced at load; the total across
  loaded code panels is capped, the way `MAX_DEVICES` caps the table. A panel over budget is not
  loaded.

A global cap on the number of installed panels (like the device cap) keeps the ring and memory
bounded on the Pi Zero 2 W (about 512 MB RAM, 4 small cores, 480x320, 3 fps draw loop).

## 6. Phased roadmap

Each phase ships something usable and has a clear done bar. The risky phases are last and marked.

**Phase 0: harden the contract, move deskpane's own panels onto the panel API (no outsiders).**
Extract `deskpane-contract` (Rust) and `deskpane_contract` (Python). Introduce the internal panel
registry and host-API primitive set, and re-express the existing overview, device and agents
pages as registered panels built on it; the page ring comes from the registry. Document the
contract and host-API versioning rule (folds in T-014). No plugin loading yet, no outside code.
*Done when:* every current page renders through the panel registry, the preview and all tests are
pixel-identical to today, the contract crate and Python module are extracted and depended on, and
the versioning rule is written down.

**Phase 1: data-source plugins.** Formalise "a reporter is a plugin": the `plugins/` layout, the
manifest, the contract types and UDP send helper in the SDK, a template data-source, and preview
support for third-party data. Add a narrow custom-metric packet kind only if a real plugin needs
it. *Done when:* a sample data-source plugin in `plugins/` sends data, shows on the mock screen
via `scripts/preview.sh --plugin`, has tests, and CI runs them.

**Phase 2: render/panel plugins = the merge-gated MVP (what contributors actually use).** The
declarative panel format and its validator, the host API wired to the registry, the full SDK, the
`scripts/preview.sh --plugin` flow, the CI gates (section 4), and the review checklist (section 4).
*Done when:* a contributor can scaffold, build and see a new panel on the preview with no Pi; a
plugin PR runs green through every CI gate end to end; one example third-party-style panel is
merged and rides in a release.

**Phase 2 security done-bars (non-negotiable, required before any third-party panel is accepted).**
These came out of the security-specialist review and each is a hard gate, not a nice-to-have:

- **Closed format grammar (HIGH-1).** The declarative renderer uses no `str.format`, `format_map`,
  `%`-formatting, f-string, `eval`, `exec` or equivalent on plugin text. A `format` value is parsed
  by deskpane's own closed grammar: one declared field plus an optional allowlisted numeric spec
  (integer, `.0f`, `.1f`, percent) plus literal text, no attribute or index access, nothing
  reachable through `__`. Adversarial validator tests are green in CI (see the worked-example
  security note and `docs/SECURITY.md`).
- **`allowlisted_get` is the only fetch path and resolves-and-pins (HIGH-2).** Data sources may do
  HTTP only through the SDK's `allowlisted_get`, and raw networking is banned by the scan. That
  helper must: resolve the host itself; reject any resolved address in a private, loopback,
  link-local, CGNAT or metadata range (127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16,
  169.254.0.0/16, 100.64.0.0/10, ::1, fc00::/7, fe80::/10); connect to the pinned resolved IP with
  the Host header set, so a DNS rebind between the check and the connect cannot swap the address;
  refuse all redirects, or re-run the full allowlist and IP-range check on every hop; require
  HTTPS; and enforce a hard timeout and a response-size cap. The daemon has no auth in v1, which is
  exactly why an SSRF to the Pi's own ports or the LAN has to be blocked inside the fetcher.
- **Parse and draw limits so a bad plugin cannot crash or hang the Pi (MEDIUM-1).** Byte caps on
  the manifest and panel files enforced **before** parse; depth and element-count caps in the
  schema validator; per-plugin isolation so one plugin failing at load or during a frame's draw
  skips only that page and never the core pages or other plugins; and a total draw-cost budget per
  frame. This reuses the character-pack validator discipline (size caps, path safety, no symlinks)
  as the required bar, not just the inspiration.
- **Every plugin number and string goes through the existing content checks (MEDIUM-2).** Every
  plugin-sourced number, including the new custom-metric packet kind, passes the contract's
  finite-and-bounds check (`check_range`) so NaN, Inf and absurd magnitudes never reach the
  formatter or the geometry math. The declarative renderer applies the same control-character and
  Unicode-format-character rejection the data plane already uses (`check_text`,
  `check_no_format_chars`) plus a hard length cap on every string it draws, bound or literal, so a
  panel cannot draw overflowing, bidi-spoofed or glyph-bomb text.

**Phase 3: packaging, signing, a gallery of git links.** Vendoring plugins into the signed bundle,
build provenance and bundle signing (folds in T-031), per-plugin budgets enforced at load, and a
**gallery**: a curated page in the docs that links to external plugin git repos. This is a list of
links, reviewed by hand, not a runtime store. *Done when:* plugins ship in a signed release,
budgets are enforced on load, and the gallery lists external plugin repos with a short review note
each.

**Phase 4 (ONLY IF side-loading is wanted): the WASM sandbox and capability model; and,
separately, an optional store.** The moment anyone can install a **code** panel on a Pi **without**
the CEO's merge, review stops being the wall and a runtime sandbox becomes mandatory (section 7).
Then: compile code panels to WebAssembly, run them with wasmtime on the Pi (aarch64 is supported;
see the note below), expose **only** the host API as imports, and cap memory and CPU per panel
(linear-memory limit plus fuel). Declarative panels still need no sandbox. Separately and
independently, an optional store or registry with its own review pipeline. *Done when (if
pursued):* a code panel runs sandboxed with a memory and fuel cap and only the declared host
imports, and the side-load path **refuses** any non-sandboxed code panel.

WASM-on-Pi feasibility (facts, 2026-11-11): the Pi Zero 2 W is a full aarch64 Linux board (quad
Cortex-A53, ~512 MB), not a microcontroller, so a server-class runtime is viable here even though
it is not on a Cortex-M part. wasmtime officially supports aarch64 (Raspberry Pi 4 and newer, AWS
Graviton) and its Winch baseline compiler gained AArch64 plus the component model in wasmtime 35.
A FOSDEM 2026 talk put runtime footprints (indicative slide figures) at wasmtime ~308 KiB and
WAMR interpreter ~80 KiB; a Nov 2025 arXiv benchmark found wasm3 lighter and faster than WAMR on
tiny devices. Takeaway: wasmtime is the natural choice on the Pi Zero 2 W if we want the component
model and Rust-first ergonomics; WAMR or wasm3 are the fallback if per-panel memory proves tight.
None of this is on the v1 path; it is only Phase 4, only if side-loading is wanted. Sources:
[wasmtime aarch64 (Fedora package)](https://extras.getpagespeed.com/fedora/42/aarch64/repoview/wasmtime.html),
[Winch AArch64 + component model](https://bytecodealliance.org/articles/winch-aarch64-support),
[Wasmtime on Pico 2 / memory limits (Zulip)](https://bytecodealliance.github.io/zulip-archive/stream/438936-SIG-Embedded/topic/Running.20Wasmtime.20on.20Pico.202.3A.20Overcoming.20Memory.20issues.html),
[FOSDEM 2026 Rust+Wasm on constrained devices](https://archive.fosdem.org/2026/schedule/event/8TDKRT-rust-wasm-embedded/),
[wasm3 vs WAMR benchmark (arXiv 2512.00035)](https://arxiv.org/abs/2512.00035).

## A worked example: a weather plugin

**Illustrative, not the final API.** This shows the actual files a contributor would write for a
"both" plugin (a data source plus a panel), so the shape of sections 1 to 5 is concrete. Names,
the declarative keys, the SDK helpers and the custom-metric packet kind are illustrative; the
real schema is pinned in Phase 2. Two assumptions made here, both flagged: the data source is
written in **Rust** (the repo is Rust-first and the SDK extracts a Rust `deskpane-contract`
crate, so a contributor gets the types and the `UdpClient` send helper for free; Python is the
other supported language and would look the same), and it sends a **custom-metric packet kind**
(`kind:"metric"`, open question Q5), because `weather_temp_c` is not a device field.

### 1. Folder tree

```
plugins/weather/
  manifest.toml
  source/
    Cargo.toml
    main.rs                 data-source program (Rust, rust-patterns)
  panel/
    weather.panel.toml      the declarative panel, no code
  tests/
    source_test.rs          checks the built packet shape and bounds
    test_panel.py           checks the panel file parses and binds weather_temp_c
  README.md
  LICENSE
```

### 2. `plugins/weather/manifest.toml`

```toml
name = "weather"
author = "a-contributor"
version = "0.1.0"
# the contract and host-API versions this plugin targets (section 2)
contract_version = "1"
host_api_version = "1"
# ships both a data source and a panel
kind = "both"
license = "MIT"

[declares]
# the field this plugin publishes (data source) and reads (panel); CI fails if
# the code reads or publishes anything not listed here (section 4)
publishes = ["weather_temp_c"]
reads = ["weather_temp_c"]

[declares.capabilities]
# the only host the data source may GET; CI checks the code calls no other
http_get = ["api.open-meteo.com"]
# the panel asks for no frame timer (it does not animate)
timer = false

# entry points (section 5)
[entry.source]
bin = "deskpane-weather"
packet_kind = "metric"      # illustrative custom-metric kind, Q5
key = "weather_temp_c"

[entry.panel]
file = "panel/weather.panel.toml"
title = "Weather"
```

### 3. `plugins/weather/source/main.rs` (the data source)

```rust
// A weather data-source plugin: reads the current temperature from the one
// allowlisted host and sends it to the daemon as a custom-metric packet. It
// runs as its own process (a user service or timer on the Pi, or on any LAN
// machine), so nothing here runs inside the dashboard. Illustrative: the clap
// CLI, the --interval loop, the lat/long flags and the JSON field pick are
// elided; a real plugin loops like the reporter instead of sending once.

use deskpane_contract::{MetricPacket, METRIC_PACKET_VERSION};
use deskpane_sdk::{allowlisted_get, now_ms, UdpClient};

// the daemon address and the one host declared in manifest.toml
const DAEMON: &str = "192.168.1.50:8765";
const WEATHER_HOST: &str = "api.open-meteo.com";
const WEATHER_PATH: &str = "/v1/forecast?latitude=52.5&longitude=13.4&current=temperature_2m";

fn main() {
    // one reading, then send; a failed reading sends nothing, so the panel
    // keeps its last value or shows its "missing" text
    let reading = fetch_temp_c();
    let temp_c = match reading {
        Ok(value) => value,
        Err(reason) => {
            eprintln!("deskpane-weather: no reading: {reason}");
            return;
        }
    };

    let sent = send_metric(temp_c);
    match sent {
        Ok(()) => {}
        Err(reason) => eprintln!("deskpane-weather: send failed: {reason}"),
    }
}

/*
 * fetches the current temperature in Celsius from the allowlisted host only.
 * allowlisted_get refuses any host not declared in manifest.toml, so this
 * program cannot be pointed at the LAN or the Pi itself
 */
fn fetch_temp_c() -> Result<f64, String> {
    let body = allowlisted_get(WEATHER_HOST, WEATHER_PATH);
    let text = match body {
        Ok(t) => t,
        Err(e) => return Err(format!("http: {e}")),
    };

    // the JSON parse and the current.temperature_2m pick are elided here; the
    // real code also bounds-checks the value before it is sent
    let parsed = parse_current_temp(&text);
    match parsed {
        Ok(value) => Ok(value),
        Err(e) => Err(format!("parse: {e}")),
    }
}

/*
 * builds one custom-metric packet and sends it over UDP, the same send helper
 * and the same path every reporter uses; the daemon validates and caps it like
 * any other packet
 */
fn send_metric(temp_c: f64) -> Result<(), String> {
    let packet = MetricPacket {
        kind: "metric".to_string(),
        v: METRIC_PACKET_VERSION,
        device_id: "weather-berlin".to_string(),
        key: "weather_temp_c".to_string(),
        value: temp_c,
        t: now_ms(),
    };

    let client = UdpClient::new();
    let socket = match client {
        Ok(c) => c,
        Err(e) => return Err(format!("socket: {e}")),
    };

    let sent = socket.send(DAEMON, &packet);
    match sent {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("udp: {e}")),
    }
}
```

### 4. `plugins/weather/panel/weather.panel.toml` (the declarative panel)

```toml
# A declarative weather panel: a label and the temperature as big text. No
# code: every element names a primitive from the host API (section 2), a
# position and, where it shows a value, a field to bind. Positions are in
# panel-rect coordinates (0,0 is the top-left of the rect the renderer hands
# this panel). Colours are palette tokens, never raw RGB.
schema = "deskpane.panel/1"
title = "Weather"

# the fields this panel reads; must match manifest.toml [declares].reads
reads = ["weather_temp_c"]

# the small grey label
[[element]]
kind = "text"
text = "Weather"
x = 20
y = 28
size = "small"          # a size role: clock|huge|big|mid|small|tiny
color = "SUBTEXT"       # a palette token

# the temperature as big text, bound to the field
[[element]]
kind = "text"
bind = "weather_temp_c"
# {weather_temp_c} is the bound field and .0f is an allowlisted number spec.
# This is NOT a Python format string. The renderer parses it with a closed
# grammar (a field name plus one allowlisted numeric spec) and does its own
# number-to-string conversion; see the security note below this block.
format = "{weather_temp_c:.0f}°C"
missing = "N/A"         # shown when no reading has arrived
x = 20
y = 72
size = "huge"
color = "TEXT"

# a threshold recolours the element above to WARN when the bound value is
# below freezing; it only compares a read field to a constant, no expressions
[[element.threshold]]
field = "weather_temp_c"
below = 0.0
color = "WARN"
```

**Security note on `format` (this is the sharpest edge in the whole design).** The `format`
string above looks like a Python format string, but the renderer must **never** implement it with
`str.format`, `format_map`, `%`-formatting, an f-string, `eval`, or anything equivalent. Python's
format mini-language allows attribute and index access, so a panel file (plain TOML that CI treats
as "just data" and the reviewer skims) could ship a value like
`{weather_temp_c.__class__.__init__.__globals__[os].system}` and reach module globals, which is
arbitrary code running in the dashboard process on the Pi. That would defeat the central claim
that declarative panels are data, not code, exactly where no one is looking.

The renderer therefore parses `format` with a **closed grammar**: a single field reference, which
must be one of the panel's declared `reads`, followed by an optional allowlisted numeric spec
(integer, `.0f`, `.1f`, or percent) and surrounding literal text. deskpane's own code does the
`float()`/`int()` conversion and the string substitution. No attribute access, no indexing,
nothing reachable through a `__` name, no nested or empty braces. The panel validator ships
adversarial tests (`{weather_temp_c.__class__}`, `{0.__globals__}`, nested braces, empty `{}`, an
unknown field name) and they run in CI. This grammar is part of the Phase 2 schema and must exist
before any third-party panel is accepted. See `docs/SECURITY.md`.

### 5. The commands the contributor runs

```bash
scripts/new-plugin.sh weather                               # copy the template, rewrite the manifest
# edit manifest.toml, source/main.rs, panel/weather.panel.toml
scripts/preview.sh --plugin plugins/weather --scenario busy # see the Weather page on the mock 480x320, press S to save a PNG for the PR
cargo test -p deskpane-weather                              # the data-source test
python3 -m unittest discover -s plugins/weather/tests       # the panel test
# then open the PR:
git checkout -b plugin/weather
git commit -am "feat(plugins): weather panel and data source"
git push -u origin plugin/weather                           # open a PR; CI runs the section 4 gates
```

### 6. The steps the CEO runs after merging (as the installer)

```bash
# the plugin now ships in the release; upgrade the Pi from your own machine
scripts/provision-pi.sh pi-zero
# or on the Pi itself: curl -fsSL https://.../get.sh | sudo bash
```

Then turn the panel on in the plugin registry (section 5: the page ring is built from installed
manifests, each with an enable flag):

```toml
# /etc/deskpane/plugins.toml  --  which installed panels appear, and in what order
[weather]
enabled = true          # adds a Weather page to the tap ring
```

And enable the data source so readings flow (illustrative unit name):

```bash
sudo systemctl enable --now deskpane-weather.timer   # fetch and send on a schedule
```

The dashboard reloads the registry, adds the Weather page, and the number appears once the first
packet arrives (N/A until then).

## 7. Security

The security-specialist will review this plan after the architect, so this section is explicit.

**The merge-gated threat model.** A plugin reaches a Pi only after the CEO reads the PR, CI passes,
and it ships in a release. The attacker we are defending against is a **contributor (or a
compromised contributor account) who gets malicious code past review**, and secondarily a
**data-source plugin that sends hostile packets** (already handled by the existing UDP validation
wall). The dashboard process on the Pi can read the filesystem and reach the network, so a code
panel that gets in has that reach.

**Why review alone is a weak wall, said plainly.** The CEO cannot deeply audit every line of every
PR, and will audit less as volume grows. Worse, a plugin that is benign when merged can **turn
malicious in a later version**: v1.0 is clean, v1.3 adds a line that exfiltrates. Review catches
the obvious; it does not catch the subtle or the future. So review is necessary but not
sufficient, and the design must not rest on it alone.

**Defense in depth for v1 (all independent of review):**

1. **Declarative-first panels.** The general contributor path ships **data, not code**. A
   declarative panel physically cannot call the network or the filesystem, because it has no call
   site. This removes the entire class of risk for the common case, review or no review.
2. **A narrow host API.** Even a code panel is handed only named data and draw primitives. The
   surface to misuse is small and fixed.
3. **CI banned-call scan, plus hard dependency rules.** Network, filesystem, process spawn and
   dynamic code are rejected mechanically in plugin code. Said honestly, a static deny scan is a
   speed bump, not a wall: a determined contributor evades it with string-built calls, reflection,
   `unsafe`/FFI, a `build.rs` or proc-macro, or a transitive crate. What gives it teeth is the
   dependency discipline in section 4: `#![forbid(unsafe_code)]`, a committed and diffed
   `Cargo.lock`, a vetted dependency allowlist, denied build scripts and proc-macros, and
   `cargo-deny`/`cargo-vet`. The scan reduces what review must catch; the real v1 wall is still CEO
   review.
4. **Declared data and capabilities.** A plugin states what it reads and uses; CI fails if the
   code does more. Drift between claim and behaviour is caught.
5. **Data sources are out-of-process.** They run as separate OS processes and go through the same
   validate-and-cap wall as any reporter, so they cannot reach into deskpane at all.
6. **Per-plugin budgets and load-time skipping.** A panel that fails to load or exceeds its budget
   is skipped with a log line, never crashing the display (the character-pack and per-frame
   draw-error model already in `dashboard.py`).
7. **Update review and version pinning (the answer to "benign now, malicious later").** Naming that
   risk is not defending against it, so v1 commits to a concrete control. A plugin update is a PR
   reviewed with the **same** gates as a brand-new plugin (banned-call scan, dependency rules,
   `declares` cross-check, tests), never a lighter "just a version bump" pass. Any diff that touches
   the `declares` block, a declared capability, the host allowlist, or `Cargo.lock` is a flagged
   top-level decision, reusing the "changes what every user gets" rule, because a dependency version
   bump is a code change even when the plugin's own source is untouched (this is the xz-style vector
   the deny scan is blind to). The release bundle pins each plugin by content hash and
   `scripts/package-pi-bundle.sh` refuses anything not pinned, so what ships is exactly what was
   reviewed.

**The precise trigger for a mandatory WASM sandbox.** The sandbox becomes **required** the moment
there is **any path that installs code (a Python-module panel, or later a WASM panel) onto a Pi
without going through the CEO's merge**: a side-load flag, a store install, a "paste this URL"
installer, anything. At that instant the human wall is gone and the narrow API must be enforced at
**runtime**, not just by CI. Until such a path exists, v1 ships with no sandbox and that is a
deliberate, documented choice. Declarative panels never need the sandbox. Note that character
packs and declarative panels can already be installed out-of-band safely **because they are data**,
validated, not code; this trigger is specifically about code.

**What the security-specialist should scrutinise:**

- The banned-call scan: is the deny list complete, and can it be evaded (string-built calls,
  unusual imports, Rust `unsafe`, build scripts, macros, transitive crates)? Should Rust data
  sources be restricted to a vetted dependency set or `#![forbid(unsafe_code)]`?
- The declarative format: can a "data-only" panel still cause harm (a binding expression that is
  really a mini-language, a resource-exhaustion draw, a format-string or text-injection issue like
  the control/format-character checks already in `validate_agent`)?
- The allowlisted HTTP GET for data sources: where it runs, SSRF and redirect handling, timeouts,
  response size caps, and whether it can be pointed at the LAN or the Pi itself.
- The in-process Python-core-panel path: is it worth keeping at all, or should even core panels be
  declarative so the dashboard process never loads third-party code?
- Supply chain: a contributor account compromise, the "benign now, malicious later" version
  problem, and whether we pin and diff plugin versions across releases.
- The Phase 4 WASM boundary (when/if reached): the exact capability set, memory and fuel caps on a
  512 MB board, and that **no** host import leaks beyond the declared API.
- Reconfirm the v1 position: is "no sandbox, merge-gated, no side-load" acceptable to ship, and is
  the side-load trigger watertight.

## 8. What this costs

Rough sizes (shape-level, for sequencing, not estimates):

- **Phase 0:** small to medium. A refactor with no behaviour change but a lot of tests to keep the
  pages pixel-identical. Mostly mechanical, low risk.
- **Phase 1:** small. Largely formalising what reporters already do, plus a template and preview
  wiring.
- **Phase 2:** medium to large. A new declarative format and its interpreter, the SDK, the CI
  gates, the review checklist, and the docs. This is the real build.
- **Phase 3:** medium. Packaging, signing/provenance, budgets, the gallery page.
- **Phase 4:** large, and only if side-loading is wanted. A new runtime dependency (wasmtime or
  WAMR), a toolchain for compiling panels to WASM, and the capability model. A store is a separate
  large effort on top.

**New ongoing burden (this is the honest part):**

- **Review load.** Every plugin PR is now the CEO's to read, decide and merge, forever. This grows
  with contributors and never goes away.
- **Versioning discipline.** The contract and host API are now public interfaces. Breaking them
  breaks other people's plugins, so changes need the compatibility rule applied every time, major
  bumps, and deprecation windows.
- **Support and compatibility.** Plugin bug reports, "my panel broke in the new release", and
  keeping old plugins working across deskpane versions.

**The honest note.** This turns deskpane from **"my dashboard"** into **"a platform"**. That is a
real change of character: more reach and more contributors, but also a standing review and
maintenance commitment, a public API we cannot casually change, and a security surface that grows
with every merge. The phasing keeps that commitment small until Phase 2, and keeps the heaviest
risk (sandbox, store) optional and last. The CEO should go in wanting the platform, not just the
plugins.

## 9. Open questions for the CEO (each with a recommended default)

1. **Declarative-first for contributor panels?** *Recommend yes:* outside panels are declarative;
   Python-module panels are core-only and heavily reviewed. Keeps the contributor path safe by
   default and sandbox-ready. (If you would rather give contributors full Python power from day
   one, say so, but then the WASM trigger effectively arrives with the first external code panel.)
2. **How far in v1?** *Recommend:* build Phase 0 and Phase 1 first (own panels on the API, data
   sources), then Phase 2 as the first contributor-facing release. Stop and reassess before Phase
   3. Do not scope Phase 4 now.
3. **Who is the first audience?** *Recommend:* close friends only for the first few plugins, to
   shake out the SDK and the review flow before strangers can open PRs.
4. **Bundle every merged plugin, or a curated default set?** *Recommend:* a small curated default
   set ships in the release; everything else is opt-in by install, so the default image stays
   lean on the Pi Zero 2 W.
5. **Custom-metric packet kind now or later?** *Decided 2026-11-11: now* (Phase 2 plan D1). Original recommendation, later: extend the existing kinds first,
   add a generic custom-metric kind only when a real plugin needs a field no existing one carries.
6. **Keep in-process Python core panels, or make even core panels declarative?** *Recommend a
   sunset, not an open door* (security-specialist agreed): keep the existing three pages as Python
   through Phase 0 only as migration debt, set the hard direction that all new panels are
   declarative, and plan to re-express the three pages declaratively so the dashboard process
   eventually loads no panel code. A permanent "core Python panel" path keeps the attack surface
   and the eventual WASM scope larger than they need to be.
7. **Is "no sandbox, merge-gated, no side-load" acceptable to ship as v1?** *Recommend yes,*
   documented, with the side-load trigger written into SECURITY.md so the day someone wants
   side-loading, the WASM requirement is already on record.
8. **Reserve the name "deskpane plugin" / a `plugins/` namespace and a manifest version now?**
   *Recommend yes:* fix the manifest schema version and the folder layout early so the first
   plugins do not need migrating later.

**Next:** the security-specialist review is done and folded in (the four HIGH findings are now the
Phase 2 security done-bars, the supply-chain control is section 7 point 7, and the WASM trigger and
review-checklist additions are in `docs/SECURITY.md`). The verdict: Phase 0 is safe to build as
written, since it adds no outside code; the HIGH items are gates for Phase 2, before any contributor
panel is accepted. Outstanding for the CEO: answer the open questions, starting with Q1 (declarative
first) and Q7 (ship v1 with no sandbox). On approval, Phase 0 is the first build.
