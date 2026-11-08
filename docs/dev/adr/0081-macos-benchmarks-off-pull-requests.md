# 0081. macOS benchmarks off pull requests

- Status: Accepted
- Date: 2026-11-08

## Context

[ADR 0046](0046-performance-regression-checks.md) runs the performance workflow on every
pull request, on Linux and macOS, since macOS is the only runner that draws text. It
expected about 10 minutes of wall-clock time per pull request.

The benchmark itself now takes about 30 minutes on a macOS runner. Each push to a pull
request starts five macOS jobs: `just ci`, `public-api-macos`, `just e2e`, the benchmarks
and the build time. GitHub runs only a few macOS jobs at once for an account, so with
several pull requests open they queue. On 2026-11-08 one push waited about two hours for
a macOS runner before its benchmarks started, and took 2 h 40 min in all. The benchmarks
hold a runner the longest, so every other macOS job waits behind them.

The macOS runner is also the noisiest: its thresholds are wide, so it catches only large
regressions. `just perf-remote` (see `docs/dev/testing.md`) runs the same harness on
another Mac in alternating pairs, the base first, with a real GPU, and gives steadier
numbers.

## Decision

The author accepted this on 2026-11-08, through the coordinator. It amends
[ADR 0046](0046-performance-regression-checks.md) as follows.

- The `benchmarks` job of `perf.yml` runs on Linux only for pull requests. Pushes to
  `main` still run it on Linux and macOS, so the macOS baseline stays current.
- `perf.yml` can be started by hand (`workflow_dispatch`) for any branch. That run
  includes macOS, and compares with the baseline as a pull request does. It cannot plant
  a baseline: only pushes to `main` count.
- A pull request that changes the input, render or load paths is measured with
  `just perf-remote` on a Mac before it is merged, and its description gives the result.
  Where no such Mac is set up, start the workflow by hand instead.
- The build-time jobs stay on every platform. They take about a minute.

## Consequences

- Each push to a pull request starts four macOS jobs instead of five, and none of them
  holds a runner for 30 minutes. Pull requests wait less for each other.
- A pull request no longer fails on its own for a large regression in text drawing on
  macOS. The push to `main` reports it, as a warning, and it becomes the baseline.
  Catching it before then rests on `just perf-remote` or a run started by hand.
- Linux still checks the input, render and load paths on every pull request, without text.
- The macOS baseline keeps moving with `main`, so a run started by hand compares against
  recent code.

## Alternatives considered

- **Keeping everything:** the queue grows with the number of open pull requests, and a
  push costs hours.
- **Also skipping the macOS build time on pull requests:** frees about a minute of
  runner time per push, a small gain for losing a platform's record.
- **Running the macOS benchmarks only when the input, render or load paths change:**
  most pull requests touch them, so it saves little, and the path list would need
  upkeep.
- **Fewer repetitions on pull requests:** makes the noisiest runner noisier, for a
  shorter but still long hold.
- **A self-hosted runner on a Mac:** would run code from pull requests, forks included,
  on a machine that is not thrown away after each job.
