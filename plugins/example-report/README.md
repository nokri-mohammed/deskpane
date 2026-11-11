# example-report

The simplest deskpane plugin: a **data source** that sends one device report to
the daemon on an interval, the same way the reference reporter sends real
machine stats. It shows up as a device page on the dashboard with no new panel,
because it sends the existing `Report` kind.

Use it as the starting point for a data-source plugin: copy it with
`scripts/new-plugin.sh <name>`, then replace `build_report` in `source/main.rs`
with your real source of numbers (a sensor, an API, a file).

## Layout

```
example-report/
  manifest.toml        what this plugin is and declares (schema 1)
  source/
    Cargo.toml         the data-source crate, depends only on deskpane-sdk
    main.rs            builds a Report and sends it; std-only argument parsing
  README.md
  LICENSE
```

## Run it

```bash
# build and run against a daemon (sends every 5 s)
cd source && cargo run -- --daemon 192.168.1.50:8765
# or send once and exit
cargo run -- --daemon 192.168.1.50:8765 --once
```

## Test it

```bash
cd source && cargo test
```

The tests check the demo report is a valid in-range device and that the
argument parsing handles `--once`, `--daemon` and bad input.

## What it declares

See `manifest.toml`: it `publishes` a device report, `reads` nothing, makes no
HTTP calls and draws no panel. CI checks the code does only what the manifest
says.
