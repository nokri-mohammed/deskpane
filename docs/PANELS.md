# Plugin panels

A plugin panel is a page on the Pi that shows a plugin's own numbers. You write it as a small TOML
file. There is no code in it: deskpane checks the file when the dashboard starts and draws it with
its own renderer. If you know web dev, think of it as a JSON layout that a fixed React component
renders, never `eval`.

The numbers come from the plugin's metric packets (`docs/CONTRACT.md`, "Metric packets"). The panel
only says where and how to draw them.

## Where it lives and how it is turned on

- In the repo: `plugins/<name>/panel/<name>.panel.toml`. The release bundle ships these files, and
  never plugin source.
- On the Pi: `/opt/deskpane/current/plugins/<name>/panel/<name>.panel.toml`.
- Turned on with `PANELS=<name>,<name>` in `/etc/deskpane/env`. Restart both services after a
  change: `sudo systemctl restart deskpane-daemon deskpane-dashboard`. The daemon also accepts
  numbers only from the plugins listed there.
- Plugin pages come after the device pages, in the order `PANELS` lists them.
- A missing or invalid panel is skipped with one line in the dashboard's journal
  (`journalctl -u deskpane-dashboard`). It never stops the dashboard.

## The page

deskpane draws the header (the panel's `title` and the clock) and the page dots. The panel draws
inside the **content rect: 480 x 256 pixels**, from `(0, 0)` at its top left, which sits just below
the 40 px header. Nothing a panel says can draw outside it.

## An example

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

[[element.threshold]]
below = 0.0
color = "WARN"

[[element]]
kind = "ring"
bind = "weather.rain_pct"
min = 0
max = 100
cx = 360
cy = 100
r = 60
label = "Rain"
```

## The file

| Key | Rule |
|---|---|
| `schema` | exactly `"deskpane.panel/1"` |
| `title` | the header text, 1 to 48 characters |
| `reads` | 1 to 16 names, each `"<plugin>.<key>"` of **this** plugin (a panel cannot read other plugins, devices or agents) |
| `element` | 1 to 32 elements, drawn in file order |

Unknown keys anywhere are refused. The whole file is at most 16 KB.

## Elements

| `kind` | Required | Optional |
|---|---|---|
| `text` | `x`, `y`, and either `text` (a fixed label) or `bind` + `format` | `missing` (default `--`), `size` (default `mid`), `color` |
| `ring` | `bind`, `min`, `max`, `cx`, `cy`, `r` | `label`, `color` |
| `bar` | `bind`, `min`, `max`, `x`, `y`, `w`, `h` | `color` |
| `icon` | `name`, `x`, `y` | `color` |

Every element may also have up to 8 `[[element.threshold]]` entries. At most 8 text elements may
use the `huge` or `big` size, the costly ones to draw.

- **`bind`** names one of `reads`.
- **Positions** are whole pixels inside the content rect:
  - text: `x` 0 to 479 and `y` 0 to 255; long text is cut with an ellipsis at the right edge.
  - bar: `x + w` at most 480 and `y + h` at most 256.
  - ring: radius 8 to 128, and the whole circle inside the rect.
  - icon: a 24 px box, so `x` at most 456 and `y` at most 232.
- **`min`/`max`**: finite numbers within 1e9 either way, `min` below `max`. A ring or bar fills by
  where the value sits between them, clamped to empty and full.
- **`color`**: `TEXT`, `SUBTEXT`, `ACCENT`, `WARN` or `CRIT`. The defaults are `TEXT` for text
  and icons, and `ACCENT` for rings and bars.
- **`size`**: `huge`, `big`, `mid`, `small` or `tiny`.
- **`name`** (icon): `up`, `temp`, `disk` or `network`.
- **Literals** (`title`, `text`, `missing`, `label`, `format`): 1 to 48 characters. Not allowed:
  - control characters;
  - invisible format characters (bidi overrides, zero-width spaces);
  - unassigned or private-use code points;
  - more than 2 combining accents in a row.

## `format`: the one field

`format` is plain text with exactly **one** `{field}` or `{field:spec}`, where `field` is the
element's own `bind`:

| spec | Shows | 12.46 becomes |
|---|---|---|
| none | up to 2 decimals, trailing zeros dropped | `12.46` |
| `d` | a whole number, halves away from zero | `12` |
| `.0f`, `.1f`, `.2f` | that many decimals | `12`, `12.5`, `12.46` |
| `%` | a whole number with `%` after it, **not** multiplied by 100 | `12%` |

That is the whole grammar. A second field, `{{` escapes, `!r`, attribute or index access (`.x`,
`[0]`), widths and alignment (`:>10`), and any stray brace are refused when the panel loads.
deskpane never hands plugin text to Python's `str.format` (`docs/SECURITY.md`, HIGH-1).

## Missing and old data

A value counts as missing when the plugin never sent it, or when the plugin's packet is older than
its `ttl_s`. A missing value draws:
- a text element: its `missing` text in `SUBTEXT`;
- a ring: `--` in `SUBTEXT`;
- a bar: an empty track.

Thresholds never fire on a missing value.

## Draw budget

deskpane times every plugin page it draws. A panel that takes longer than 100 ms, 3 frames in a
row, is replaced by a "this panel is too slow to draw" line until the dashboard restarts. That
gets one line in the journal. The caps above keep an ordinary panel at a few milliseconds.

## Thresholds

```toml
[[element.threshold]]
above = 30.0
color = "CRIT"
```

Each threshold has exactly one of `below` or `above`, plus a `color`, an `icon`, or both. Only
icon elements can switch their `icon`. The checks are strict (`below = 0` does not match 0), and
the first threshold that matches wins.
