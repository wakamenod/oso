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
| Medium | command scenarios: key sequences to state | integration tests in `tests/` with no window |
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

### Level 2: end-to-end tests over the control socket (from M1)

- Start a dev build and drive it over the dev control socket:
  - `send-keys` sends input
  - `get-state` reads buffer text, cursors and the modeline as JSON
  - `screenshot` captures a frame
- Agents use these tests to check behavior in the real app. A Claude Code
  agent can read the screenshots to check visuals.

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

- `just ci` on Linux, macOS and Windows
- `cargo-deny` and `cargo-audit`
- a gitleaks scan of the full history

Planned:

- a check that release builds contain no dev-only features (M1-7)
- performance and build-time budgets
- snapshot tests on Linux with lavapipe, once they exist
