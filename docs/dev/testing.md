# Testing

## Principles

- Keep logic out of GPU and window code so that most behavior can be tested
  with plain `cargo test`.
- Prefer property-based tests (`proptest`) for positions, offsets and
  coordinate mappings, where boundary cases are easy to miss.
- `just ci` (check, lint, fmt-check, test) must pass before every commit.

## Testability by layer

| Testability | Layer | Strategy |
|---|---|---|
| High | `oso-core`: buffers, selections, display model, registry, keymaps | inline unit tests and property tests |
| High | pure coordinate functions | unit tests |
| Medium | command scenarios: key sequences to state | integration tests in `crates/*/tests/` with no window |
| Medium | the running app | end-to-end tests over the dev control socket |
| Low | `oso-render` GPU code | logic split out and unit-tested; pixels checked by snapshot tests |

## Test levels

### Level 1: logic tests, no GPU (always)

- Run in `cargo test` and `just ci`.
- Cover:
  - cursor and selection motion, including multiple cursors
  - buffer edits on Japanese text and emoji
  - display-model mappings in both directions, with folds, inlays and wraps
  - keymap resolution and command dispatch
- Reach every command through the registry, as a user would.

### Level 2: end-to-end tests over the control socket (from M1-7a)

- Start a dev build (`just run-dev`, or `cargo build -p oso --features dev`)
  and drive it with `oso ctl` over the dev control socket
  ([ADR 0023](adr/0023-control-socket-protocol-and-restart.md),
  [ADR 0032](adr/0032-control-socket-transport.md)):
  - `send-keys` sends input in Emacs notation, through the same path as
    keys from the window ([ADR 0033](adr/0033-key-presses-and-notation.md))
  - `get-state` reads the state as JSON. Today this is the cursor position
    and the surface size; buffers, cursors and the modeline come in M1-7b.
  - `screenshot` writes a PNG of the current frame and returns its path
- Agents use these checks to verify behavior in the real app, following the
  `verify-in-app` skill. A Claude Code agent reads the screenshots to check
  visuals.
- Not covered yet: resizing the window and real OS key events. These are
  still checked by hand.
- The transport and protocol themselves are tested without a window or GPU
  (`oso-platform` control tests, the server tests in the tip crate), so CI
  runs them on Linux and macOS. Windows is not supported yet (ADR 0032).

### Level 3: snapshot tests (after text rendering lands)

- Render headlessly and compare PNGs with a tolerance.
- macOS runs locally on Metal. Linux CI uses Mesa's software Vulkan driver
  (lavapipe), since oso has no GL backend.
- Golden images are used only once the UI is stable enough that false alarms
  stay rare.

## Performance tests

- Benchmarks cover:
  - key-to-photon latency, through the control socket with frame timestamps
  - startup time
  - opening large files
  - editing very long lines
- CI compares results against the budgets in
  [`roadmap.md`](roadmap.md#performance-budgets).
- Key-to-photon latency is measured inside the app, from receiving a key
  event to presenting the frame that reflects it. CI fails on a regression
  against `main`, and on p99 over 8 ms where a hardware GPU is available.
  Real photon latency is checked by hand from time to time
  ([ADR 0025](adr/0025-latency-measurement.md)).
- The dev incremental build time of the tip crate is measured with
  `just build-time`. The author's Mac decides the 2 s budget, and CI fails
  only on a large regression ([ADR 0024](adr/0024-dev-build-profile.md)).

## Fuzzing

`cargo-fuzz` targets cover file loading and decoding, the display model under
random edits, highlighting and terminal escape-sequence handling. Agents can
run them for long periods.

## CI

GitHub Actions (`.github/workflows/ci.yml`) runs on every pull request and
every push to `main`. See [ADR 0021](adr/0021-development-tooling.md).

- `just ci` on Linux, macOS and Windows. `check`, `lint` and `test` cover
  the dev features too (`--all-features`).
- `just release-safety` on Linux: release builds contain no dev-only
  features, crates or code ([ADR 0032](adr/0032-control-socket-transport.md))
- `cargo-deny` and `cargo-audit`
- a gitleaks scan of the full history

Planned:

- performance and build-time budgets
- snapshot tests on Linux with lavapipe, once they exist
