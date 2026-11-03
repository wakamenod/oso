# 0025. Key-to-photon latency measurement

- Status: Accepted
- Date: 2026-11-03

## Context

The budget for key press to photon is p99 ≤ 8 ms, and regressions must fail
CI ([`roadmap.md`](../roadmap.md#performance-budgets)). CI machines have no
display, so light leaving a screen cannot be measured there.

## Decision

- **Measure inside the app.** oso records, for each key event, the time it
  received the event and the time it presented the first frame that reflects
  it. The difference is the measured latency.
- Dev builds report the latency samples over the control socket
  ([ADR 0023](0023-control-socket-protocol-and-restart.md)). Benchmarks run
  an optimized build with the dev feature enabled.
- **In CI**, a benchmark sends keys through the control socket and computes
  p99.
  - It fails on a regression against the last result on `main`.
  - It also fails when p99 exceeds 8 ms, on platforms where a hardware GPU is
    available. Software rendering (lavapipe on Linux) is checked only for
    regressions.
- **True photon latency**, including the compositor and the display, is
  checked by hand from time to time on the author's Mac.

## Consequences

- Regressions in oso's own input, layout and render path are caught in CI.
- The OS input path, the compositor and the display are outside the CI
  measurement.
- Keys sent through the socket skip the OS input path, so CI numbers are
  slightly optimistic.

## Alternatives considered

- **Measuring by hand only:** cannot fail CI on regressions as the roadmap
  requires.
- **Measuring real photons in CI:** not possible without a display and a
  camera or sensor.
