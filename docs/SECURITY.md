# deskpane security invariants

Date: 2026-11-11. These are binding rules, not aspirations. They come out of the plugin-system
plan (`docs/plans/2026-11-11-plugin-system.md`) and its security-specialist review. Write code to
these; a change that weakens one is a top-level decision, not a quiet refactor.

## v1 security posture

deskpane v1 is **merge-gated, declarative-first, narrow-API, no runtime sandbox**. Code reaches a
Pi only through a pull request the CEO reads and merges, that passed CI, and that shipped in a
signed release. The CI banned-call scan is **defense in depth that reduces what review must catch,
not a wall that replaces review**. This posture is acceptable for the friends-first threat model
(contributors are close friends first, strangers later) precisely because there is no install path
that puts code on a Pi without a CEO merge. The moment that stops being true, the WASM invariant
below fires.

## The WASM-sandbox trigger (binding invariant)

A runtime sandbox (wasmtime on the Pi, exposing only the host API as imports, with a linear-memory
limit and a fuel cap per panel) becomes **mandatory** the instant there is **any path that installs
code onto a Pi without a CEO-merged pull request**: a side-load flag, a store install, a
"paste this URL" installer, a "drop a plugin folder on the Pi and it loads" convenience, anything.
At that instant the human wall is gone and the narrow host API must be enforced at runtime, not just
by CI.

- **Declarative panels and out-of-process data sources never need the sandbox**, because they are
  data (validated), not code. Character packs are already installed out-of-band safely for the same
  reason.
- **Enforced by construction, not by policy.** Phase 3 bundle signing plus a loader assertion must
  mean the Pi loader loads a **code** panel only if it is present in the signed bundle. A
  side-loaded or unsigned code panel is refused at load, so the trigger cannot be crossed by
  accident or by a convenience patch that forgot the rule.

Nothing in v1 as planned crosses this line: there is no side-load, no store, no URL installer; data
sources are separate OS processes; declarative panels are data. Keep it that way until the sandbox
exists.

## Plugin review checklist additions

These are mechanical items added to the CEO's merge checklist (plan section 4), so the subtle,
by-design holes are caught by rule and not by gut feel:

- **Declarative `format` strings use only the closed numeric grammar.** A `format` value is a field
  reference from the panel's declared `reads` plus an optional allowlisted numeric spec (integer,
  `.0f`, `.1f`, percent) plus literal text. Reject any attribute access, index access, `__`-name,
  nested or empty braces. The renderer must never pass plugin text to `str.format`, `format_map`,
  `%`-formatting, an f-string, `eval` or `exec`. Adversarial validator tests run in CI.
- **Any capability, host-allowlist or `Cargo.lock` change is a flagged top-level decision.** An
  update PR touching the `declares` block, a declared capability, the host allowlist, or the
  lockfile is never a routine "version bump" merge; it is reviewed as a new plugin and flagged under
  the "changes what every user gets" rule.
- **A data source uses only the SDK fetch path.** Raw networking (`std::net`, `reqwest`, `socket`,
  `urllib`, `requests`, hand-rolled sockets) is banned in plugin code. The only sanctioned fetch is
  the SDK's `allowlisted_get`.

## Runtime controls the design must implement (the four HIGH findings)

These are the Phase 2 security done-bars from the plan, restated here as the canonical list. Each is
a hard gate before any third-party panel is accepted.

### 1. Closed format grammar (no eval-family on plugin text)

The declarative renderer parses `format` with deskpane's own closed grammar and does its own
`float()`/`int()` conversion and substitution. No `str.format`/`format_map`/`%`/f-string/`eval`/
`exec`. Adversarial tests in the validator: `{field.__class__}`, `{0.__globals__}`, nested braces,
empty `{}`, unknown field name. Rationale: Python's format mini-language reaches module globals
through attribute and index access, which would be arbitrary code in the dashboard process delivered
by a file CI treats as "just data".

### 2. `allowlisted_get` resolves and pins (SSRF blocked at the fetcher)

The SDK's `allowlisted_get` is the only fetch path for data sources and must:

1. resolve the host itself;
2. reject any resolved address in a private, loopback, link-local, CGNAT or metadata range
   (127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, 100.64.0.0/10, ::1,
   fc00::/7, fe80::/10);
3. connect to the pinned resolved IP with the Host header set, so a DNS rebind between the check
   and the connect cannot swap the address;
4. refuse all redirects, or re-run the full allowlist and IP-range check on every hop;
5. require HTTPS;
6. enforce a hard timeout and a response-size cap.

The daemon has no auth in v1, so an SSRF to the Pi's own ports, the LAN, link-local metadata, or a
Tailscale peer must be blocked inside the fetcher. Hostname string-matching alone is not sufficient.

### 3. CI scan is defense in depth, backed by dependency rules

A static deny scan catches honest mistakes and lazy attackers but is evadable (string-built calls,
reflection, `unsafe`/FFI, a `build.rs` or proc-macro that runs code at compile time, a transitive
crate). What makes the `declares`/allowlist cross-check meaningful:

- Rust data sources carry `#![forbid(unsafe_code)]`, commit a `Cargo.lock` that is diffed on every
  update, build with `--locked`, and draw only from a small vetted dependency allowlist; crates with
  a `build.rs` or proc-macro are denied unless explicitly whitelisted. `cargo-deny` and `cargo-vet`
  run in CI.
- The declaration is enforced at ground truth, not grepped: data `publishes`/`reads` are validated
  against the packet the preview harness observes; panel `bind`/`threshold.field` must be in `reads`;
  capabilities are runtime-enforced because the SDK is the only fetch path and honours only declared
  hosts.
- The real v1 wall is CEO review; the scan reduces what review must catch.

### 4. Updates reviewed as new plugins; bundle pins by hash

"Benign now, malicious later" (clean at v1.0, an exfiltration line at v1.3) is defended, not just
named:

- A plugin update PR is reviewed with the same gates as a brand-new plugin.
- Any diff touching the `declares` block, a declared capability, the host allowlist, or `Cargo.lock`
  is a flagged top-level decision (a lockfile bump is a code change: the xz-style vector).
- The release bundle pins each plugin by content hash, and `scripts/package-pi-bundle.sh` refuses
  anything not pinned, so what ships is exactly what was reviewed.

## Load-time and content limits (so a bad plugin cannot crash the Pi)

- Byte caps on the manifest and panel files enforced **before** parse; depth and element-count caps
  in the schema validator (defeats billion-laughs-style nesting and giant values).
- Per-plugin isolation: one plugin failing at load or during a frame's draw skips only that page,
  never the core pages or other plugins. A total draw-cost budget per frame.
- Every plugin-sourced number (including the custom-metric packet kind) passes the contract's
  finite-and-bounds check (`check_range`), so NaN, Inf and absurd magnitudes never reach the
  formatter or the geometry math.
- The declarative renderer applies the same control-character and Unicode-format-character rejection
  the data plane already uses (`check_text`, `check_no_format_chars`) plus a hard length cap on
  every string it draws, bound or literal (defeats overflowing, bidi-spoofed and glyph-bomb text).
- Reuse the character-pack validator discipline (size caps, path safety, no symlinks) as the
  required bar, not just the inspiration.
