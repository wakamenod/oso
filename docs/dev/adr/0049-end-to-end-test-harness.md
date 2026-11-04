# 0049. End-to-end test harness

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0037](0037-test-pyramid.md) makes end-to-end tests over the control socket
layer 5 of the pyramid: the real app, locally with `just e2e` and on Linux CI with
lavapipe, few of them, and only for seams and critical flows. It leaves open how
they stay out of `just ci`, how they start the app, how they avoid an instance the
user has running, and what can be asserted where there is no text backend
([ADR 0036](0036-text-system-and-glyph-atlas.md)).

## Decision

- **An integration test, `crates/oso/tests/e2e`,** behind
  `required-features = ["dev"]`, whose tests are `#[ignore]`d. `just e2e` runs them
  with `--ignored`. So `cargo test` skips them, and `just ci`, which uses
  `--all-features`, compiles them and lists them as ignored. It is `cfg(unix)`, like
  the socket ([ADR 0032](0032-control-socket-transport.md)). `cargo test` gives
  filtering, a report per test and captured output for nothing.
- **Instances start through `oso_platform::process`,** with the binary cargo built
  for the test (`CARGO_BIN_EXE_oso`). No new exception to
  [ADR 0010](0010-process-and-network-policy.md) was needed, and the architecture
  test is unchanged. The allowlist that module applies means a test cannot set a
  variable for an instance, so `just e2e` sets `RUST_LOG`, which is passed on by
  name along with what reaches the display and the control directory.
- **A test reaches only its own instance:** every connection and `oso ctl` call
  names its process ID, nothing asks for "the only instance", and each connection
  checks the process ID that `version` reports. Instances share the user's control
  directory, so a test instance that starts removes stale files there, as any
  instance does.
- **Cleanup is in `Drop`, so it runs on a panic.** An instance is asked to exit, so
  that it removes its socket and screenshots, and is killed after 5 s. Then its own
  files are removed by name, if any are left. Test files are in a scratch directory,
  also removed in `Drop`, with fake secrets only.
- **Nothing sleeps a fixed time.** `send-keys` and `run-command` have finished when
  they return. Everything else polls `get-state` within a 30 s limit, and every
  socket read has the same limit.
- **Failures name what was awaited and quote no buffer text.** The log of each
  instance is captured through a pipe, and printed when a test panics, which
  `cargo test` shows only for a failed test. The fake secret is taken out of it, in
  case a bug put it there. `just e2e` logs at `oso=debug`, as `just run-dev` does. The
  secret flow stops its instance and checks the whole log, raw, for the secret, and
  needs a line it knows is there, so that it cannot pass without a log.
- **One test at a time** (`--test-threads=1`). Windows would otherwise cover one
  another and compete for the focus. The suite takes 2 to 4 s on the author's Mac,
  1 to 5 s on Linux CI and 5 to 12 s on macOS CI, so there is little to gain.
- **The flows are the ones in [`testing.md`](../testing.md):** keys and commands,
  screenshot, invalid bytes, secret files, the restart handoff, exit, and a check
  of the cleanup itself. The handoff runs `export-state` and `import-state` directly.
  `oso dev`'s build loop stays a manual check, since it needs cargo and an edit to a
  source file, and the order of a restart has unit tests.
- **Pixels:** the screenshot flow reads the PNG itself, with `miniz_oxide`, already a
  dependency of the `dev` feature. A file of empty lines draws only the cursor on
  every OS, so the cursor's place and size are checked the same way everywhere. Text
  pixels are asserted on macOS, and their absence on Linux, so that a Linux backend
  has to change the line.
- **CI:** `e2e.yml` runs `just e2e` on Linux with Xvfb and lavapipe, and on macOS, with
  a read-only token, skipping changes only to `docs/`. ADR 0037 asks only for Linux.
  macOS is added because it is the one runner that draws text, so it is the only
  automated check of text pixels. It is its own workflow, so that `paths-ignore` stays
  simple.
- **`crates/oso/tests/e2e` is not on the list of files that need approval:** these are
  ordinary tests that grow with features, and review checks that they are not
  weakened.

## Consequences

- `just ci` runs one more test binary, whose tests are all ignored, and its time
  with nothing to build stays about 7 s. An edit to a library crate costs up to 3 s
  more, to check, lint and link the test crate again. An edit to the tip crate does
  not, since a test cannot link a binary crate, and `oso dev` and `just build-time`
  never build tests.
- Local runs take the focus, and real input reaches the windows, since they are
  ordinary windows on the screen. Flows that check positions can be disturbed by
  someone typing or scrolling over them. CI has no such input.
- On the fifth run on the author's Mac, the cleanup flow found another instance's
  state changed. The cause is unknown: real input to its window fits, but 125 later
  runs of the same check, 25 of them with every CPU busy, did not show it again. So
  where real input can reach the windows, that flow compares that instance's buffers
  and whether it is alive. Where the `CI` variable is set, there is no such input, so
  it compares every field of the state, and names the ones that differ without quoting
  them. If it fails there, real input was not the explanation, and this needs another
  look.
- A harness killed by `SIGKILL` cannot clean up: its instances keep running and its
  scratch directory stays. Ctrl-C reaches the instances too, since they share the
  terminal's process group, but leaves the scratch directory, which holds only
  generated files.
- Linux checks that the app starts, draws, takes input and guards secrets, but no
  text. macOS, on CI and on the author's Mac, checks text pixels too.
- The macOS job adds about 3 minutes of runner time to each pull request, on a runner
  that is noisier than Linux ([ADR 0046](0046-performance-regression-checks.md)).
- The list of variables an instance needs exists twice, here and in `oso dev`,
  because the tip crate is a binary and its code cannot be imported by a test.
- The benchmark harness has a socket client of its own (`examples/perf/app.rs`), so a
  change to the protocol or to how an instance stops touches both. They can share one
  when the benchmarks move to `oso_platform::process`, which ADR 0046 leaves for
  later. This one must keep its rule of never quoting a response line.

## Alternatives considered

- **A cargo feature for the tests:** `just ci` uses `--all-features`, so it would
  turn them on and open windows.
- **An example with its own `main`, as for the benchmarks:** needs its own filtering
  and reporting, and an exception to ADR 0010 like the one in
  [ADR 0046](0046-performance-regression-checks.md).
- **A private control directory per test:** would keep even stale-file cleanup away
  from the user's instances. `process::spawn` cannot set a variable, and
  `XDG_RUNTIME_DIR` also leads to the Wayland display. Addressing by process ID is
  enough.
- **Tests in parallel:** the suite is short, and windows would overlap on the
  author's Mac.
- **Linux only on CI, as ADR 0037 asks:** would leave text pixels to local runs.
