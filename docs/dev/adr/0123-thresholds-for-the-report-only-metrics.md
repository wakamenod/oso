# 0123. Thresholds for the report-only metrics

- Status: Accepted
- Date: 2026-11-12

## Context

Many metrics of the benchmark harness were only reported: no regression in them failed CI or
`just perf-remote`. Among them were keys and typing with four and sixteen windows
([ADR 0112](0112-window-splits.md)), `DEL`, the input method's preedit, the `goto-line`,
`M-x` and `C-x C-f` prompts, and `RET` with indentation ([ADR 0115](0115-indentation.md)).
Each was left for the author to set thresholds once there were numbers.

[ADR 0046](0046-performance-regression-checks.md) sets a threshold at twice the spread of 6
runs of the same code on a runner. Three sources were measured on 2026-11-12:

- **The reference Mac**, a Mac with a real GPU as `just perf-remote` runs on: 6 runs of the
  same `main` in 3 alternating pairs. Every key and typing 99th percentile stayed within 8%
  across them, and the windows' metrics as closely as the one-window keys and typing they
  repeat.
- **The Linux runner**: 99 results of pull requests and `main`. Each was compared with the
  `main` result before it, as CI compares them, which gave 89 pairs. The runners seem to be
  two kinds of machine, since `startup_ms` falls either between 52 and 60 ms or between 70
  and 77 ms. A pull request and its baseline can run on different kinds, so the spread of
  same-code runs on one machine under-reads what CI sees. The typing gate of 2.0 times and
  0.75 ms failed 3 of the 89 pairs with no regression in the code.
- **The macOS runner**: 25 earlier results of `main`. No macOS job finished on `main` between
  the windows' metrics landing and this decision, as each merge cancelled the one before it,
  so the windows', highlighting's and indentation's metrics have no macOS runner numbers.

The roadmap's budget, key press to photon p99 ≤ 8 ms, covers every key. Only `key_p99_ms`
failed over it, on a real GPU ([ADR 0025](0025-latency-measurement.md)).

## Decision

The author accepted this on 2026-11-12, through the coordinator. The thresholds are in
`crates/oso/examples/perf/compare.rs` and the budgets in `report.rs`.

- **Windows** ([ADR 0112](0112-window-splits.md)): `win4_key_p99_ms`, `win16_key_p99_ms` and
  `win16_other_p99_ms` take `key_p99_ms`'s thresholds; `win4_type_p99_ms` and
  `win16_type_p99_ms` take `type_p99_ms`'s. Both runners gate them. On Linux they failed no pair
  under these thresholds; on macOS the typing threshold is wide against the runner's typing
  spread.
- **The Linux typing threshold** becomes 3 times and 2 ms, from 2.0 times and 0.75 ms, for
  `type_p99_ms`, `syntax_type_p99_ms` and `syntax_prepare_p99_ms`. It failed none of the 89
  pairs, and none came closer than 0.6 of the way to failing. From a baseline of about 1.1 ms,
  typing still fails above 3.3 ms.
- **The other keys on the screen of mixed text** take the typing thresholds on both runners:
  `del_p99_ms`, `ime_preedit_p99_ms`, `prompt_type_p99_ms`, `mx_type_p99_ms`,
  `file_prompt_type_p99_ms` and `indent_ret_p99_ms`. `indent_ret_p99_ms` had only 4 Linux results,
  and takes typing's thresholds as [ADR 0056](0056-typing.md) gives a typing metric.
- **`DEL` on the big files**: `big_del_p99_ms` takes `big_type_p99_ms`'s thresholds and
  `line_del_p99_ms` takes `line_type_p99_ms`'s.
- **`line_trip_max_ms`** takes the freeze threshold, 3 times and 100 ms, as `big_trip_max_ms`
  has.
- **Reported only**, as before: the worst keys (`key_max_ms`, `type_max_ms`), each one sample
  that varies up to 7 times within one macOS job; the medians, beside a gated 99th percentile;
  keys while a big file loads (`*_loading_max_ms`), which on the macOS runner went from 31 to
  144 ms between two results; `startup_first_launch_ms`, which compiles shaders;
  `startup_from_main_ms`, `open_from_launch_ms` and `open_first_content_ms`, beside
  `startup_ms` and `open_first_screen_ms`; the edits without a window, under 0.1 ms; and the
  parts kept to explain a change (`syntax_handle_p99_ms`, `syntax_off_*`,
  `indent_ret_handle_p99_ms`).
- **A Mac with a real GPU** has its own table, which gates the same metrics. Each threshold is
  twice the spread of the 6 runs, as ADR 0046 sets them, but never under 1.5 times and 1 ms:
  for example 1.5 times and 1 ms for `key_p99_ms`, `type_p99_ms` and `win16_type_p99_ms`,
  2.25 times and 5.25 ms for `file_prompt_type_p99_ms` (1.5 times and 1 ms since
  [ADR 0124](0124-three-drawables.md), see As built), and 1.5 times and 17.5 ms for
  `startup_ms`. `indent_ret_p99_ms`, not measured there yet, takes typing's. The freezes keep
  the freeze threshold. `just perf-remote` then fails on a slowdown of about half, where the
  macOS runner's table needed several times. CI does not change, as its runners have no
  hardware GPU.
- **Every key's latency fails over its budget on a real GPU.** `BUDGETS` lists each gated
  key and typing 99th percentile at 8 ms. `compare.rs` holds each 99th percentile in
  `BUDGETS` to it, where it held only `key_p99_ms`. A frame's preparation,
  `syntax_prepare_p99_ms`, is not a key's latency and has no budget.

### A known near-budget metric

`file_prompt_type_p99_ms`, typing into `C-x C-f` over 100,000 names while a worker matches
them ([ADR 0096](0096-completion-on-a-worker-past-5000-candidates.md)), measured 4.75 to
7.31 ms over the 6 runs on the reference Mac (median 6.33 ms), against the 8 ms budget.
Within those runs, 6 of the 30 processes measured more than 8 ms, up to 9.9 ms. It does not
fail today, as a result's value is the median of its 5 processes, but a small slowdown would
make it fail. Making it faster is a task of its own. [ADR 0124](0124-three-drawables.md) did,
and it is no longer near the budget (As built).

## Changes to accepted ADRs

[ADR 0046](0046-performance-regression-checks.md), "The thresholds": "Each is at least twice
the spread seen over 6 runs of the same code there". Now each threshold is also checked
against the recent pairs of a result and the baseline before it, on that runner, and must
fail none of them. On a Mac with a real GPU, a table of its own applies, never under 1.5
times and 1 ms.

ADR 0046, "The runners": "the 8 ms check of ADR 0025 is in the code". Now it covers every
key and typing 99th percentile in `BUDGETS`, not only `key_p99_ms`.

## Consequences

- A regression in a window, a prompt, the input method, `DEL` or `RET` fails CI, where it
  used to pass unseen.
- Linux fails typing later: a doubling of 1 ms passes, where it failed before. The reference
  Mac's table catches what CI now lets through.
- `file_prompt_type_p99_ms` can fail the budget on the reference Mac with a small slowdown, as
  noted above.
- The macOS runner's thresholds for the new windows', highlighting's and indentation's
  metrics come from their one-window peers, not from their own runs. They are checked again
  once 6 results of `main` complete there.
- Each new threshold makes `compare.rs` longer by a line per table. The three tables must
  name the same metrics, which a test checks.

## Alternatives considered

- **Keep the Linux typing threshold at 2.0 times and 0.75 ms.** It failed about 3% of the
  pairs with no regression in the code, and the new metrics beside it would fail more:
  `file_prompt_type_p99_ms` 8 of 89.
- **2.5 times and 1.5 ms for Linux typing.** It failed no pair, but the metrics beside
  `type_p99_ms` came within 0.8 to 0.93 of failing.
- **Gate the windows' metrics on Linux only**, until 6 macOS runs finish. Their macOS
  thresholds are wide against the runner's spread, so gating both runners now costs little.
- **Keep the reference Mac on the macOS runner's table.** Then only a slowdown of several
  times would fail there, against a spread of 8%.
- **List the new budgets for reading by hand only.** The budget covers every key, and the
  reference Mac is where it can be checked.

## As built

### The reference Mac after three drawables (2026-11-13)

[ADR 0124](0124-three-drawables.md) gave each surface three drawables, and the reference Mac's
lines were measured again: 6 runs of `main` in 3 alternating pairs, with no swapping. `main`
moved between the second and third pair, so 5 runs are of one commit and 1 of a later one; the
later run fell inside every range below. The method is unchanged (ADR 0046): twice the ratio's
distance from 1 and twice the difference, rounded up to 0.25, never under 1.5 times and 1 ms.
The author chose this on 2026-11-13, through the coordinator.

| Metric | 5 runs of one commit (ms) | Spread | Threshold |
|---|---|---|---|
| `file_prompt_type_p99_ms` | 1.93 to 2.01 | 1.04 times, 0.08 ms | 1.5 times and 1 ms, from 2.25 times and 5.25 ms |
| `win16_other_p99_ms` (`C-x o`) | 3.48 to 3.68 | 1.06 times, 0.20 ms | 1.5 times and 1 ms, as before |
| `indent_ret_p99_ms` | 3.20 to 3.29 | 1.03 times, 0.09 ms | 1.5 times and 1 ms, as before, now from its own runs |

- **`C-x C-f` over 100,000 names** measured at most 2.13 ms in any of the 30 processes, against
  the 8 ms budget. From a baseline of about 2 ms it now fails above about 3 ms, where it failed
  above about 7.2 ms.
- **Every gated key and typing 99th percentile** has a spread that gives the minimum, 1.5 times
  and 1 ms. Every ordered pair of the runs passes every line; the closest came 0.33 of the way to
  failing (`open_first_screen_ms`).
- **The lines set from the first 6 runs keep their wider gates**, where these runs alone would
  give narrower ones (the author's choice): `line_keys_p99_ms` at 2 times and 1.5 ms,
  `open_first_screen_ms` at 2 times and 5 ms, and the floors of `startup_ms`,
  `open_full_load_ms`, `big_load_ms` and `line_load_ms`. Nothing in ADR 0124 changed them, and
  the wider of two sets of runs is the safer gate.

Observations, which change no threshold:

- **`C-x o` still spikes now and then in a single process.** Of the 25 processes of the 5 runs, 2
  measured over 6 ms (6.39 and 6.61 ms, against about 3.5) and none over 8 ms. A result's value
  is the median of its 5 processes, so it passes.
- **The worst key moving on the big files is bimodal**: `big_keys_max_ms` and
  `line_keys_max_ms` measured either about 2.5 ms or about 16 ms in a process (medians 16.1 and
  9.5 ms over the runs), about one refresh at 60 Hz. They stay far under the freeze threshold of
  3 times and 100 ms.

### The macOS runner, still unchecked (2026-11-13)

The macOS runner's thresholds for the windows', highlighting's and indentation's metrics still
come from their one-window peers. Of the results of `main` there, the 5 complete ones are from
before those metrics; every run since was cancelled by the next merge. They are checked again
once 6 results of `main` complete there.
