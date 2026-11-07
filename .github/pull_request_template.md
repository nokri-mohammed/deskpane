<!--
The pull request title must be a Conventional Commit message, for example:
  fix: keep the label when the reporter restarts
  feat: show network traffic on the device page
  feat!: rename the cpu_pct field   (breaking change)
release-please uses the title to pick the next version. See CONTRIBUTING.md.
-->

## What changed

<!-- Describe the change and why it is needed. Link the issue, for example "Fixes #12". -->

## Tests run

<!-- Tick what you ran. Add anything else you tested, for example on a real Pi. -->

- [ ] `cargo test --workspace --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo fmt --all --check`
- [ ] `shellcheck deploy/install.sh deploy/uninstall.sh scripts/*.sh`
- [ ] `python3 -m py_compile pi/*.py`
- [ ] Tested on a Raspberry Pi (say which model):

## Checklist

- [ ] The title is a Conventional Commit message with the right type.
- [ ] New code has tests.
- [ ] Rust code follows the style in CONTRIBUTING.md (no shorthand, comments above the code).
- [ ] A new `Report` field is optional, or the title marks a breaking change with `!`.
- [ ] `docs/CONTRACT.md` and other docs are updated if the behavior changed.
- [ ] No passwords, keys, real WiFi names, real IP addresses or real hostnames in any file.
- [ ] No em dashes in docs or comments.
