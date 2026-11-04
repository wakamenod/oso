# 0046. Performance regression checks

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0024](0024-dev-build-profile.md) and
[ADR 0025](0025-latency-measurement.md) decided what to measure and left the
thresholds, the baseline and the tools to M1-8. Measuring also showed that
startup misses its budget, for reasons other than the ones first suspected.

GitHub's runners are not one machine. The same code, run 6 times, varied by
up to 3 times on Linux and 3.7 times on macOS, between runs of one job.

## Decision

### What is measured

- `just perf` runs a harness (`crates/oso/examples/perf`) against an optimized
  build with the dev feature. It builds in `target/perf-build`, so a binary with
  the control socket never sits at `target/release/oso`.
- **Key latency** runs from the socket thread reading a key, or the window
  event arriving, to `present` returning for the first frame after it. A key
  that draws nothing has no sample. It does not include the GPU's work, the
  display's refresh or the compositor, nor frames that later fill in glyphs
  beyond the per-frame limit.
- **Startup** runs from just before the process starts to the first presented
  frame. A frame skipped because the window is hidden does not count.
- **Opening a file** runs from the request, with the window up, to a complete
  screen. A launch with a file is reported separately.
- **Typing** is measured in two halves until M2 can insert text: an unbound
  key's trip through the UI thread, and an edit in the core structures without
  a window.
- **"No freeze"** means no key over 100 ms.
- Repeated runs give the medians: 9 launches, 5 runs of 400 keys, 5 file opens
  and 3 runs on each big file. A `_max_` metric is the median of each run's
  worst key. This keeps a slowdown that happens every time and ignores one
  hiccup of a shared machine, such as the 790 ms seen once on macOS.
- A run fails if fewer than 95% of its keys draw a frame, if a key gets no
  frame or a sample is dropped, or if `M->` and `M-<` do not reach the ends of
  the file. Losing the expensive keys must not look like an improvement.

### The socket

- Two dev-only methods. `take-timings` hands over key samples (at most 16384)
  and reports startup, load times and the GPU. It returns no buffer text.
- `open-file` visits a file with the window up, because nothing else can until
  M2. It goes away once `run-command` can reach a find-file command.
- `open-file` reads no more than a same-user client could read itself
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)), runs nothing,
  and is not limited: each call adds a buffer and a thread, in a dev build.

### The baseline

- A push to `main` uploads its results as artifacts, kept for 90 days. A pull
  request fetches the newest one (`.github/scripts/perf-baseline.sh`).
- Only runs with `event=push`, `branch=main` and this repository as the head
  count, so a pull request or a fork cannot plant a baseline. The job has
  `actions: read` and no write permission.
- It passes with a note if there is no baseline, or if the baseline has another
  schema, OS, GPU or text backend: its numbers say nothing about this code. The
  next push to `main` sets a new one. A failing download or API call fails the
  job, so an outage does not switch the check off.
- Pushes to `main` only report. A regression accepted on purpose becomes the
  next baseline instead of failing every later pull request.

### The thresholds

- A metric regresses if it grows by a ratio **and** by a number of
  milliseconds, so noise on tiny numbers cannot fail. A gated metric that stops
  being measured fails too.
- The thresholds are in `compare.rs`, per runner. Each is at least twice the
  spread seen over 6 runs of the same code there: twice the ratio's distance
  from 1, and twice the difference, rounded up. Change them the same way, from
  new runs. `compare.rs` and `bench.rs` need approval like other check
  definitions, and the commit hook lists them.
- Freezes use one threshold everywhere: 3 times and 100 ms, well above the
  25 ms that was the worst seen.

| Metric | Linux | macOS | Windows |
|---|---|---|---|
| `key_p50_ms` | 3.5x, 0.75 ms | 3x, 1.5 ms | |
| `key_p99_ms` | 5.5x, 2.5 ms | 6.5x, 12 ms | |
| `startup_ms` | 2.25x, 55 ms | 3.25x, 410 ms | |
| `open_first_screen_ms` | 2x, 4.5 ms | 4.75x, 44 ms | |
| `open_full_load_ms` | 3x, 370 ms | 4x, 530 ms | |
| `big_load_ms` | 3x, 3750 ms | 3.5x, 5100 ms | |
| `big_keys_p99_ms` | 4.75x, 2.5 ms | 4.25x, 7 ms | |
| `line_load_ms` | 3.25x, 235 ms | 2.25x, 235 ms | |
| `line_keys_p99_ms` | 4.25x, 1.75 ms | 2.75x, 3.5 ms | |
| `big_keys_max_ms`, `line_keys_max_ms`, `big_trip_max_ms` | 3x, 100 ms | 3x, 100 ms | |
| `dev_build_ms` | 2x, 1400 ms | 4.25x, 3500 ms | 1.75x, 1375 ms |

### What CI can measure

- **Linux:** Xvfb and lavapipe, from Ubuntu's signed archive. It presents
  frames, and it is the quieter runner. It has no text backend
  ([ADR 0036](0036-text-system-and-glyph-atlas.md)), so it times no shaping or
  glyphs.
- **macOS:** a paravirtualized GPU, which is not hardware. It is the only runner
  that draws text, and it is noisy.
- **Windows:** no control socket yet
  ([ADR 0032](0032-control-socket-transport.md)), so only `just build-time`.
- No runner has a hardware GPU, so the 8 ms check of ADR 0025 is in the code
  and never fires in CI today. The budgets are read by hand from `just perf` and
  `just build-time` on the author's Mac, which lists each budget beside its
  metric.

### Startup

- AppKit's default menu is not built on macOS. It took about 55 ms, the one
  large cost oso controls. Its Command shortcuts would clash with Command =
  Meta in M2.

### The process exception

- `crates/oso/examples/perf/process.rs` may start processes, as an exception to
  [ADR 0010](0010-process-and-network-policy.md). The harness starts the app
  under test, `cargo` and `git`, from `PATH`, with no sanitized environment and
  no spawn log. It is an example behind `required-features = ["dev"]`, and never
  ships. This does not extend to anything else, including the `oso dev`
  supervisor.

## Consequences

- Large regressions in the input, render and load paths fail pull requests, and
  the numbers appear in each job's summary.
- Only large ones do. The thresholds reflect runner noise: bringing the default
  menu back would pass on macOS and does not exist on Linux, so small
  regressions are the author's to notice in `just perf`.
- GPU time is outside the key latency, so a frame that costs the GPU more does
  not show unless drawables back up.
- Each merge may grow a number by just under its threshold, and the baseline
  follows, so a slow drift can pass. Only the budgets read by hand catch it.
- A key sent over the socket waits for its reply before the next, so one stall
  affects one sample, and one sample in 400 does not move a p99. The worst-key
  metrics are there for that.
- The workflow takes about 10 minutes of wall-clock time on each pull request:
  macOS benchmarks 9 to 11, Linux 5, the build-time jobs 2 to 6. It runs on all
  of them, since macOS is the only runner that draws text.
- Windows open and take the focus while `just perf` runs on the author's Mac. A
  window hidden by another app fails the run with a clear error.
- There are no Command-Q or Command-H until oso has a menu.

## Alternatives considered

- **Building and measuring `main` in the same job:** would cancel most runner
  variance and allow tighter thresholds, at twice the time per pull request.
  ADR 0024 and ADR 0025 chose the last result on `main`. It is the next step if
  these thresholds miss real regressions.
- **The maximum across runs for `_max_` metrics:** would catch a freeze in one
  run out of three, and would fail on any hiccup of a shared runner.
- **`actions/cache` for the baseline:** needs no extra permission, but entries
  can be evicted, and tracing a value to its commit is harder.
- **A branch of committed results:** keeps history, but needs `contents: write`
  against ADR 0021.
- **Building the GPU device in the background, showing the window after the
  first frame, starting from an app bundle:** measured, with no gain. The GPU
  takes about 3.5 ms, and wgpu will not present to a hidden window.
- **`oso-platform::process` first:** follows ADR 0010 to the letter, but brings a
  public module and its policy into a change about measuring.
