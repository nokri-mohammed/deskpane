# Plugin system Phase 2: build breakdown

**Status:** approved by the CEO 2026-11-11 with D1 to D10 as recommended. Parent plan:
[2026-11-11-plugin-system.md](2026-11-11-plugin-system.md) (T-067, approved). This file turns the
parent's Phase 2 ("render/panel plugins, the merge-gated MVP") and its security done-bars into
slices that each ship as one PR. Where this file and the parent disagree, this file wins for
Phase 2, and the parent's worked example is updated to match once the slices land.

Phase 0 and the Phase 1 foundation shipped in v0.1.0-beta.5 (contract crate, panel registry, SDK,
`plugins/example-report`, `scripts/new-plugin.sh`). Today a plugin can only show up as a device
page, because it can only send the existing `Report`. Phase 2 lets a plugin publish its own
numbers and describe its own page.

## Decisions to approve

Each has a recommendation. Approving the plan approves the recommendations unless you say
otherwise.

| # | Decision | Recommendation |
|---|---|---|
| D1 | Add a custom-metric packet now? (parent Q5 said "later, when a real plugin needs it") | **Yes, now.** The first real panels (weather, a UPS, a Pi-hole) all carry numbers that are not device fields. This closes Q5. |
| D2 | What a metric value can be | **Numbers only in v1.** No text values: text adds an injection and size surface and every first panel can map states to numbers (a weather code, 0/1 for up/down). Text values can come later as an additive change. |
| D3 | How panels name a value | **`<plugin>.<key>`**, for example `weather.temp_c`. Plugins cannot collide, and a reviewer sees at a glance whose data a panel reads. |
| D4 | What a panel may read | **Its own plugin's metrics only in v1.** Not device fields, not agent sessions, not other plugins. Smallest surface; widen later if a real panel needs it. |
| D5 | How "old data" is handled | **The packet carries `ttl_s`** (10 s to 24 h). After it, the panel shows its `missing` text in `SUBTEXT`. A weather source that sends every 10 minutes sets 1800; a fast sensor sets 30. |
| D6 | Which plugin pages show on the Pi | **None by default; opt in with `PANELS=weather,ups` in `/etc/deskpane/env`.** Plugin pages sit after the device pages in the ring. This is the parent's Q4 "curated, opt-in" answer. |
| D7 | Who draws the page header | **deskpane draws it** (title, clock, page dots, same look as the device page). The panel draws only its content rect, 480 x 256 below the 40 px header and above the 24 px dots strip. Every plugin page looks like part of deskpane. |
| D8 | Where data-source plugins run in Phase 2 | **Where reporters run: any always-on machine on your LAN** (your server, or the Pi itself started by hand). Shipping plugin binaries in the Pi bundle with their own systemd units is Phase 3 packaging. |
| D9 | Reading LAN services (Home Assistant, Pi-hole, a NAS API) | **Out of Phase 2, its own design and security review next.** The SSRF rule (HIGH-2) blocks private addresses on purpose. The likely answer is that the LAN address comes only from the user's own config file, never from plugin source, with a separate capability flag; that needs the security-specialist before it is built. |
| D10 | Drawing primitives in v1 | **Text, ring gauge, bar, built-in icon.** Character-pack sprites in plugin panels come later (they touch the pack licence rules). |

## Slice 1: the metric packet (wire, daemon, envelope, SDK)

**The packet** (UDP, same port and path as reports and agent packets):

```json
{"kind":"metric","v":1,"plugin":"weather","ttl_s":1800,"values":{"temp_c":12.4,"rain_pct":40,"code":3}}
```

| Field | Rule |
|---|---|
| `plugin` | `^[a-z][a-z0-9-]{0,31}$`, the manifest `name` |
| `values` | 1 to 16 entries; key `^[a-z][a-z0-9_]{0,31}$`; value a finite number with absolute value at most 1e9 |
| `ttl_s` | integer, 10 to 86400 |
| size | within the existing 1500-byte UDP buffer |

**Daemon:** a new pure table `daemon/src/metric_table.rs` like `device_table.rs`: at most 16 plugins,
a plugin is owned by its source IP while its data is fresh (a second IP is refused and logged
through the existing log limiter), replaced whole on each packet, forgotten `ttl_s + 300` seconds
after the last packet. No reply is sent (agent packets set this precedent). Refusals are logged,
never echoed.

**Envelope:** additive, `v` stays 1, older dashboards ignore it:

```json
{"v":1,"views":{"stats":[...],"agents":{...},"metrics":{"weather":{"values":{"temp_c":12.4},"age_s":42,"stale":false}}}}
```

**Contract and mirror:** `MetricPacket`, `validate_metric`, the limit constants and `MetricsView` in
`deskpane-contract`; `PacketKind` gains `metric`; `pi/deskpane_contract.py` parses `views.metrics`.
The existing golden fixture `pi/tests/fixtures/envelope_v1.json` gains a `metrics` view and stays
checked from both sides (built: one fixture is simpler than two, and the preview mock test only
requires the mock to send a subset of the fixture's keys). Built with one addition: at most 8
plugins per source IP, the same per-source idea the agent table uses.

**SDK:** `deskpane_sdk::send_metrics(daemon, plugin, ttl_s, values)`.

**Security review (2026-11-11), built into the slice:** MEDIUM-1, one packet with `ttl_s` 86400
could hold a plugin name (or, with spoofed IPs, the whole table) for a day. Fixed: the daemon
accepts metric packets only for plugins listed in `PANELS` (the same setting as D6, read from
`/etc/deskpane/env`), and an owner holds a name for at most 15 minutes of silence. LOW-2 (a
takeover log line was not rate limited), LOW-3 (`parse_metrics` trusted names) and LOW-4 (a huge
int could raise) are fixed; LOW-5 got a contract test that an empty metrics view is left out.
LOW-1 (evict stale entries when full) is moot: with at most 16 turned-on names the 16-slot table
cannot fill with anything else. Residual risk, accepted for v1 and recorded in `docs/SECURITY.md`:
no packet authentication, so a spoofed owner IP can change a turned-on plugin's numbers (T-006).

**Tests:** every validation rule at its edge (17 values, a 33-char key, NaN, infinity, 1e9 + 1,
ttl 9 and 86401, bad plugin names), table cap, IP ownership, expiry, envelope round trip, the
golden fixture.

## Slice 2: the panel file, validator and renderer

**File:** `plugins/<name>/panel/<name>.panel.toml`, parsed with Python's built-in `tomllib` (the
Pi runs Python 3.13, checked 2026-11-11, so no new dependency).

```toml
schema = "deskpane.panel/1"
title = "Weather"
reads = ["weather.temp_c", "weather.rain_pct"]

[[element]]
kind = "text"
bind = "weather.temp_c"
format = "{weather.temp_c:.0f} C"
missing = "no data"
x = 24
y = 40
size = "huge"
color = "TEXT"

[[element.threshold]]
below = 0.0
color = "WARN"

[[element]]
kind = "bar"
bind = "weather.rain_pct"
min = 0.0
max = 100.0
x = 24
y = 200
w = 432
h = 14
color = "ACCENT"
```

**Elements:** `text` (a literal `text`, or `bind` + `format`), `ring` (`bind`, `min`, `max`, `cx`,
`cy`, `r`, optional `label`), `bar` (`bind`, `min`, `max`, `x`, `y`, `w`, `h`), `icon` (`name` from
the built-in set, `x`, `y`). Shared: `color` is a palette token (`TEXT`, `SUBTEXT`, `ACCENT`, `WARN`,
`CRIT`), `size` is a role (`huge`, `big`, `mid`, `small`, `tiny`), `threshold` entries compare the
element's bound value to a constant (`below` / `above`) and change its colour or icon.

**The format grammar (security done-bar HIGH-1).** `format` is literal text with at most one
`{field}` or `{field:spec}`; `field` must be in `reads`; `spec` is one of `d`, `.0f`, `.1f`,
`.2f`, `%`. deskpane's own `format_value(value, spec)` makes the string. Never `str.format`,
`format_map`, `%`, f-strings or `eval` on plugin text. Anything else is refused at load:
attribute or index access, a second field, empty or nested braces, a stray brace, an unknown
field. Adversarial tests (`{weather.temp_c.__class__}`, `{0}`, `{}`, `{{x}}`,
`{weather.temp_c!r}`, `{weather.temp_c:>{n}}`) run in CI.

**Caps (MEDIUM finding):** file at most 16 KB, at most 32 elements, at most 8 thresholds per
element, literals at most 48 characters with no control characters, coordinates and sizes inside
the 480 x 256 content rect, all numbers finite.

**Code:**
- `pi/panels.py`, pure and stdlib only: `load_panel(text) -> (panel, None) | (None, reason)`,
  `format_value`, `pick_color` (thresholds), `element_geometry`, and `panel_values(panel,
  metrics_view, now)` that returns only the declared reads, marking stale or missing ones.
- `pi/dashboard.py`: `draw_plugin_panel` behind `PANEL_DRAW["plugin"]`; the host header and dots
  are drawn by the same code as the device page.
- `pi/layout.py`: `PAGE_PROVIDERS` gains `("plugin", ...)` after the device pages, listing enabled
  panels that loaded.
- Loading: at start, for each name in `PANELS`, read
  `/opt/deskpane/current/plugins/<name>/panel/<name>.panel.toml`; a missing or invalid panel is
  skipped with one log line, as a bad character pack is today.
- `pi/deskpane_contract.py`, `scripts/package-pi-bundle.sh`, the `install.sh` guard and the deploy
  test lists all gain `panels.py` (the lesson from the beta.5 bundle bug).

**Tests:** validator unit tests including the adversarial set, `format_value` table, thresholds,
stale and missing values, the ring builder with 0, 1 and 3 enabled panels, and a lockstep test
that `PANEL_DRAW` and `PAGE_PROVIDERS` still agree.

**Built (2026-11-11):** as planned, with these details settled in code and `docs/PANELS.md`.
- `%` shows the value as a whole percent and does not multiply by 100, so `rain_pct = 40` reads
  `40%`.
- `d` rounds halves away from zero.
- The format field must be the element's own `bind`.
- A text element with `bind` needs a `format`.
- Thresholds use strict comparisons, and the first match wins.
- Icons sit in a 24 px box.
- The dashboard unit now reads `/etc/deskpane/env` (`EnvironmentFile=-`) for `PANELS` and sets
  `PLUGINS_DIR`.
- The bundle and `install.sh` copy only `plugins/<name>/panel/<name>.panel.toml` regular files with
  a valid name (never source or symlinks).
- `tomllib` is imported lazily, so the 3.7 local preview still runs and just loads no panels.

## Slice 3: `scripts/preview.sh --plugin` and an example panel

- `scripts/preview.sh --plugin plugins/<name>` loads that panel and enables it in the preview.
  Values come from `plugins/<name>/preview.toml` (sample values, optionally a short timeline so
  thresholds and staleness can be seen). With `--real`, the plugin's own binary runs against the
  preview daemon on 127.0.0.1:18765 instead.
- `plugins/example-metrics`: a data source that sends two invented numbers plus a panel with a
  ring, a bar and a big number. `scripts/new-plugin.sh --kind metrics` scaffolds from it.
- This closes the open Phase 1 item (`--plugin` for data sources) too.

**Done when:** the example panel shows on the mock screen from one command, and its tests run in CI.

## Slice 4: `allowlisted_get` (security done-bar HIGH-2)

In `deskpane-sdk`, a small blocking HTTPS client built on `ureq` with `rustls` (no async runtime,
small, pure Rust; it goes on the dependency allowlist of slice 6). A plugin constructs it with
its hosts (`AllowlistedClient::new(&["api.open-meteo.com"])`), and CI checks that list matches the
manifest's `http_get`. For every request it:

1. accepts only `https://` to a listed host;
2. resolves the host itself, once, and refuses any address in 127.0.0.0/8, 10.0.0.0/8,
   172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, 100.64.0.0/10, 0.0.0.0/8, ::1, fc00::/7,
   fe80::/10 or an IPv4-mapped form of those;
3. connects to that pinned address with the hostname kept for TLS and the Host header, so a DNS
   rebind cannot swap the address after the check;
4. refuses redirects;
5. times out (5 s connect, 10 s total) and stops reading after 64 KB.

**Tests:** the address table, IPv4-mapped IPv6, a resolver that answers private, `http://`
refused, a redirect refused, the size cap and the timeout against a local test server.
The security-specialist reviews this slice before it merges.

## Slice 5: the weather plugin (the first real panel)

`plugins/weather`: Open-Meteo (free, no API key), latitude and longitude from the command line or
the user's config file (never hardcoded; the repo keeps placeholders). Publishes `temp_c`,
`feels_c`, `rain_pct`, `wind_kmh`, `code`; sends every 10 minutes with `ttl_s = 1800`. Panel: big
temperature, a condition icon picked by thresholds on `code`, a rain-chance bar, wind as text.
Runs on an always-on machine (D8). Its README is the contributor walkthrough, and the parent
plan's worked example is updated to the real files.

## Slice 6: CI gates for plugin PRs (security done-bars HIGH-3 and HIGH-4)

A new `plugins` job in `ci.yml`, run for each folder in `plugins/`:

- manifest schema check, and the panel validator from slice 2 on every panel file;
- `cargo build --locked` and the plugin's tests; `Cargo.lock` must be committed;
- `#![forbid(unsafe_code)]` present in each data source;
- `cargo-deny` with a dependency allowlist: a crate outside it, or any crate with a `build.rs` or
  proc-macro not explicitly allowed, fails the job;
- the banned-call scan (`std::process`, `std::fs` writes, raw `std::net` outside the SDK,
  `unsafe`), documented as a speed bump, not a wall;
- a job summary that lists what changed against `main` in each plugin's capabilities, hosts,
  dependencies and lockfile, so an update to an already-merged plugin is reviewed as a new plugin
  (HIGH-4);
- a pull request template with the review checklist from `docs/SECURITY.md`.

## Phase 2 done bar

From the parent plan, made concrete: a contributor can scaffold a plugin, build it and see its
page in `scripts/preview.sh --plugin` with no Pi; a plugin PR runs green through every slice 6
gate; the weather plugin is merged and its page runs on the real screen from a release; all four
HIGH security done-bars are met and the security-specialist has signed off on slices 1, 2, 4 and 6.

## Order and who does what

| Slice | Size | Review |
|---|---|---|
| 1. Metric packet | M | code review, security-specialist (new untrusted input) |
| 2. Panel file and renderer | L | code review, security-specialist (format grammar) |
| 3. Preview `--plugin` and example panel | M | code review |
| 4. `allowlisted_get` | M | code review, security-specialist |
| 5. Weather plugin | M | code review |
| 6. CI gates | M | code review, security-specialist |

Each slice is test-first, is its own PR with a Conventional Commit title (PRs are squash-merged),
and updates `docs/CONTRACT.md`, `CLAUDE.md` and `docs/TEAM.md` as it lands. Slices 1 to 3 give a
visible result (an example panel on the preview and on the Pi) before any network code exists.

## Out of scope for Phase 2

LAN-service reads (D9); plugin binaries and units in the Pi bundle, bundle signing and the plugin
gallery (Phase 3); re-expressing the overview, agents and device pages as declarative panels
(the parent's sunset, later); text metric values (D2); sprites in plugin panels (D10); any
runtime sandbox (Phase 4, only if side-loading is wanted).
