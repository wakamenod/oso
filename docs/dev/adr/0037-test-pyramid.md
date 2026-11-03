# 0037. Test pyramid

- Status: Accepted
- Date: 2026-11-03

## Context

[`testing.md`](../testing.md) describes test levels, but not which layer a
test belongs in or how many tests each layer should have. In M1 the suite is
about 250 unit and property tests plus command scenario tests, and it runs in
about 10 seconds. End-to-end checks over the control socket exist, but only
as ad hoc agent checks (`verify-in-app`), not as regression tests. The only
planned way to test what the screen shows is pixel snapshots, which are slow
and brittle. Agents write most tests, so test quality needs measuring, not
just test counts.

## Decision

Tests form a pyramid. Lower layers have many fast tests; higher layers have
few.

| Layer | What it checks | Where it runs | Amount |
|---|---|---|---|
| 1. Unit, property and reference-model tests | `oso-core` logic. Data structures are compared against a naive model (for example, the rope against a `String`) | every OS, `just ci` | most |
| 2. Command scenario tests | key sequences through the `Dispatcher`, then the resulting state, with no window | every OS, `just ci` | every command |
| 3. Text-grid snapshots | the screen rendered as a grid of characters (buffer text, cursors, modeline, minibuffer, decorations), compared as text | every OS, `just ci` | every visible feature |
| 4. Pixel snapshots | rendering details only text cannot show (glyphs, colors, underlines) | Linux CI with lavapipe | few |
| 5. End-to-end tests over the control socket | the real app: socket, input path, rendering pipeline, critical user flows | locally (`just e2e`) and Linux CI with lavapipe | few |
| 6. Manual checks | IME, real OS key events, overall look on macOS | the author | as needed |

Alongside the pyramid:

- fuzzing (continuous)
- performance budgets ([ADR 0025](0025-latency-measurement.md))
- the measures of test quality below

### Rules

- A bug fix adds a test at the **lowest layer that reproduces the bug**.
- Every command gets layer 2 tests (part of the `add-command` skill).
- Every feature that changes what is drawn gets layer 3 snapshots.
- Layers 4 and 5 cover seams between components and critical flows only.
  Never test logic there that a lower layer can test.
- Snapshot updates are reviewed like code. An unexplained snapshot change is
  a review finding.

### Measuring test quality

- **Mutation testing** with `cargo-mutants` runs on a weekly CI schedule.
  Surviving mutants in `oso-core` are triaged: add a test, or document why the
  mutant is equivalent.
- **Coverage** is measured with `cargo-llvm-cov` and reported on pull
  requests. It never fails the build.
- **Time budget:** the whole `cargo test` run stays within 30 seconds on the
  author's Mac. CI warns when it grows past that.

### Tooling

- Text-grid snapshots use `insta`, subject to the dependency approval of
  [ADR 0010](0010-process-and-network-policy.md).
- `cargo-mutants` and `cargo-llvm-cov` are CI tools, not crate dependencies.

## Consequences

- Most of what the screen shows is checked deterministically on all three
  OSes, without a GPU.
- The test infrastructure needs new work:
  - a text-grid renderer for snapshots
  - an automated E2E harness on Linux with lavapipe
  - scheduled mutation and coverage jobs

  The roadmap plans these as M1-9.
- Snapshot files live in the repository and must stay readable.

## Alternatives considered

- **Pixel snapshots as the only way to test the screen:** slow, GPU-bound and
  brittle across fonts and platforms.
- **E2E checks left to agents without automation:** no protection against
  regressions.
- **A hard coverage threshold:** encourages tests written to raise the number
  rather than to catch bugs. Mutation testing measures test quality better.
