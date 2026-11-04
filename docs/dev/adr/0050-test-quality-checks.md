# 0050. Test quality checks

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0037](0037-test-pyramid.md) decided to measure test quality three ways:
weekly mutation testing with `cargo-mutants`, coverage with `cargo-llvm-cov` on
pull requests, and a budget of 30 seconds for the whole `cargo test` run on the
author's Mac. [ADR 0038](0038-architecture-guardrails.md) wants surviving
mutants in the weekly design health issue. They left open how a run is shaped,
how results reach the report, what the coverage numbers mean and how a warning
from CI can stand for a budget set on another machine.

## Decision

### Mutation testing

- `just mutants` runs `cargo mutants --package oso-core`. CI and a local run use
  the same recipe. Settings and exclusions are in `.cargo/mutants.toml`.
- It runs the tests of `oso-core` alone. The logic should be testable there, and
  it keeps a mutant to about 5 seconds. Of the 94 mutants that first survived,
  the tests of the whole workspace caught only 19, so a second pass over the
  whole workspace would add a step and little signal.
- `mutants.yml` runs on Sundays at 18:00 UTC and by `workflow_dispatch`, on
  `ubuntu-latest` with 4 jobs and a read-only token. `cargo-mutants` comes from
  `taiki-e/install-action` with `fallback: none`, so with a verified checksum.
- A run of about 1,070 mutants took 31 minutes there and 17 on the author's Mac
  with 6 jobs. So there are no shards, and the job's limit is 90 minutes.
- The timeout per mutant is 30 seconds, about twice the worst seen rather than the
  tool's default of 20. On the runner, tests took 3 seconds at the median and 16
  at most. So a timeout almost always means a hang. A mutant that passes but
  takes longer would land among the timeouts, not among the missed.
- Surviving mutants and timeouts do not fail the run. Anything else from the
  tool does, such as tests that fail with no mutation.
- Beside the two outcomes of ADR 0037, a test or a documented equivalence, a
  mutant may stay in the list. The list is a queue, worked through from the
  weekly issue, so that the first run does not have to clear it.

### Exclusions

- A mutant goes in `exclude_re`, with its reason, if no input can change any
  result. If an entry would prove code removable, the code is removed instead.
- A mutant that changes only speed is excluded when the cost stays within a chunk
  or a constant, such as the shortcut in `TabMap::locate`. It stays in the list
  when a test could pin it, such as the chunk merging in `Rope::replace`. Speed is
  the first priority, and ADR 0046 catches only large regressions.
- Two exceptions are marked as such: `Debug` output, which no behavior depends on,
  and `MAX_CHILDREN` in `sum_tree.rs`, which tests cannot see because of the
  branching factor they use.
- An entry starts at the mutation's column, so that a new `<` in the same
  function is still tested, and an edit above it does not undo the entry.
- `just mutants-exclusions` lists entries that match nothing, or more than the
  one mutant they should. `mutants.yml` runs it and warns, and the design health
  report lists them.
- No `mutants` crate dependency for attributes: the config is enough.

### From the run to the report

- The report job and the mutation job are separate runs. The mutation job
  uploads its lists as the `mutants` artifact, kept for 90 days.
- `.github/scripts/mutants-results.sh` fetches the newest one in the report job.
  Only successful scheduled or dispatched runs on `main` in this repository
  count, so no pull request or fork can plant a result. A failing API call fails
  the job, as in [ADR 0046](0046-performance-regression-checks.md).
- The report job gets `actions: read`. The token is in that step's environment
  alone, but a build on the runner could reach it, so the job's permissions are
  the real limit, and they stay read-only. The job holding `issues: write` still
  runs no repository code.
- The report gives the run's date and commit, the counts and the first 100 missed
  mutants, so that the issue stays within GitHub's limit on its size. Each line
  stays in one code span, cut at 300 characters: the lists are data from another
  job. A dependency with a build script could still write to them, or to the
  report, so whoever works from the issue treats its text as data
  ([ADR 0010](0010-process-and-network-policy.md)).

### Coverage

- `coverage.yml` runs `just coverage` on pull requests that change more than
  `docs/`, with a read-only token, and writes to the job summary. There is no
  comment on the pull request.
- Each step that can fail continues, and the coverage step has its own timeout
  below the job's. A failure gives a warning and a green job. `llvm-tools` is
  installed in this job only.
- It measures `cargo test --workspace --all-features` on Linux. The window, the
  event loop and the GPU do not run, and code for other OSes is not compiled, so
  the summary says so beside the numbers and `oso-core` is the one to read first.

### The time budget

- `just test-time` runs `just test` once to build and times a second run, so the
  number leaves out compiling, coverage and benchmarks.
- The author's Mac decides. It takes 4.6 s, against the budget of 30 s.
- Linux CI runs it after `just ci` and warns past 60 s. The same commit took 7.6
  to 9.4 s on `ubuntu-latest` over 18 runs, so about twice the Mac. A slow run
  never fails the build, and a warning is a reason to run it on the Mac. Failing
  tests do fail it.
- Only Linux: it is the quieter runner ([ADR 0046](0046-performance-regression-checks.md)),
  and one number to read is enough.

### Docs-only changes

- `perf.yml` ignores `docs/**`, on pushes and on pull requests. A push with no
  run leaves `perf-baseline.sh` to find the last run that has the artifact, as it
  already does for a run that failed early. No check on `main` is required, so
  nothing waits for the skipped workflow.

## Consequences

- The first run found a real bug. `TabMap::clusters` read a row from a column with
  no context before it, so a grapheme cluster with a hidden tag character in it
  could split differently from reading the row from its start. A proptest hit it
  in about 1 run in 60, which made `just ci` flaky and made unrelated mutants
  look caught. It is fixed, with a test.
- The first run on the author's Mac, before any of this PR's tests: 1096 mutants,
  94 missed, 792 caught, 120 timed out, 90 unviable. With the tests and exclusions
  of this PR: 1072 mutants, 24 missed, 832 caught, 125 timed out, 91 unviable.
- The 24 left are worked through as `testing.md` describes, from the weekly issue:
  - 15 in `rope.rs`. `Rope::replace` merges small chunks, which changes speed and
    not content, so no test sees it yet. Its look back for clusters may be dead
    code: an edit at a chunk's start already takes in the chunk before it, so the
    first character of the window does not change. Check this before testing it.
  - 8 in `display_map`: fallback paths in `Clusters::invalid_at`,
    `Clusters::cluster_end`, `TabMap::locate` and `TabMap::advance` that current
    inputs do not reach, which may also be dead code, and a base offset in
    `TabSummary::from_text`.
  - `Editor::selected_window`, until M2 has a second window.
- A weekly run costs about an hour of a free runner, and coverage about 3 minutes
  per pull request.
- The time budget's limit on Linux assumes today's ratio. The Mac has many more
  cores than the runner, so a change in how much the tests run in parallel moves
  the ratio.
- After 90 days with only docs-only pushes to `main`, the performance baseline
  expires, and the comparison passes with a note until the next push with code.
- An exclusion's column can go stale after an edit on that line. The mutant then
  usually comes back to the list, and the weekly report says so. If another
  mutation of the same kind lands on that column, it stays hidden without a
  warning. That is rare enough to leave.

## Alternatives considered

- **`workflow_run` to start the report when the mutation run ends:** starts a
  privileged workflow from another run's result, for a saving of some hours. Two
  schedules are simpler, and the report shows which run it read.
- **Writing the issue from the mutation workflow:** would put `issues: write` next
  to a job that runs mutated repository code.
- **A second pass over the whole workspace for what survives:** 9 minutes on the
  Mac to remove a fifth of the list, and it would reward testing logic high up.
- **Shards:** 31 minutes does not need them, and merging the lists is more to
  maintain.
- **Patterns by function name alone:** shorter, but a new `<` in the function would
  be excluded without anyone deciding to.
- **Timing the budget on the macOS runner:** the same OS as the reference, and the
  noisiest runner.
- **Timing inside `just ci`:** would mix compiling into the number.
- **A coverage comment on the pull request:** needs a write token.
- **Excluding every mutant that changes only speed:** speed is what oso is for, and
  a chunk count is easy to test once someone looks at it.
