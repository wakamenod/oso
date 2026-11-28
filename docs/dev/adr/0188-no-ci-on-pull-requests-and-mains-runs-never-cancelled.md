# 0188. No CI on pull requests, and main's runs never cancelled

- Status: Accepted; "Perf queued, Wayland per push, as amended" (2026-11-29)
- Date: 2026-11-28

## Context

The author is oso's only developer. Since
[ADR 0174](0174-fast-checks-on-pull-requests.md) a pull request ran only the fast checks, but
waiting for them still cost hours a day. On 2026-11-28 the author chose, through the
orchestrator, a new merge rule. A pull request merges once its local checks pass:

- `just ci`, which the commit hook runs;
- `just clippy-windows` and `just public-api-check`;
- the window e2e flows that the change needs.

Only `main`'s CI runs, after the merge, and what it finds is fixed in a follow-up pull request.

`main`'s runs cancelled each other. Every workflow that runs on `main` had
`cancel-in-progress: true` in a group named by the branch. So a merge cancelled the run of the
merge before it, and a broken commit showed as cancelled, not failed. On 2026-11-27 six of the
nine CI runs on `main` were cancelled this way. Once `main`'s CI is the only CI, each commit needs
a result of its own.

The repository has no branch protection on `main`. Its one ruleset blocks deletion and force
pushes, and no status check is required.

## Decision

### Pull requests

- **A pull request runs no CI job, except `adr-cited`** (`structure.yml`). It is the one
  check that reads the pull request's description, so it cannot run on `main`. It takes about
  20 seconds on GitHub's Ubuntu runner, and builds nothing. The merge does not have to wait
  for it.
- **The `secrets` scan does not run on pull requests.** The commit hook runs gitleaks on
  staged, unstaged and untracked files for every commit made through Claude Code. A pushed
  branch is public whether CI runs or not, so a scan on the pull request would stop nothing.
  `main`'s `secrets` job still scans the full history after each merge.
- **`ci.yml`, `e2e.yml`, `perf.yml` and `e2e-weston-repeat.yml` lose their `pull_request`
  triggers.** The `changes` job, which skipped the build for a docs-only pull request, goes
  too. A push to `main` always ran everything, and still does.
- **Labels start nothing.** `e2e`, `perf` and `weston-repeat` mean nothing now. To run one of
  these workflows on a branch before it merges, start it by hand:
  `gh workflow run ci.yml --ref <branch>`, or `e2e.yml`, `perf.yml` or
  `e2e-weston-repeat.yml`. `ci.yml` and `e2e.yml` gain `workflow_dispatch` for this.
  - Only people with write access can start a workflow, so a fork cannot. The runner's
    job-started hook already accepts `workflow_dispatch`, from any branch of this repository.
  - A run started by hand on a branch keeps the branch's build folder (`pr`) on the Mac, and
    writes no cache. It cancels the same branch's earlier run of the same workflow.
  - `perf.yml` opens or updates the issue "Performance regression on main" only for a run on
    `main`. It had opened it for any run that was not a pull request's, a branch's run
    started by hand included. A branch still never becomes the baseline: only pushes to
    `main` do (`perf-baseline.sh`).

### `main`

- **`ci.yml` never cancels a run on `main`.** Each push to `main` has a concurrency group of
  its own, so every commit runs every job and ends with its own `ci-ok`. `e2e.yml`, which
  `ci.yml` calls, has a group of its own per push run too.
  - The jobs of several commits run side by side on GitHub's runners. The Mac's jobs wait in
    line on its runners, one `just ci (macos)` and one `just e2e (macos)` at a time.
  - No bound is set. GitHub fails a job that has waited 24 hours.
- **`perf.yml`, `coverage.yml`, `codeql.yml` and `e2e-wayland.yml` never cancel a running run
  on `main`:** `cancel-in-progress` is false there. GitHub keeps one run going and one waiting
  in each group, and a newer push replaces the waiting one. The newer run covers the replaced
  run's commit along with its own.
  - These workflows only inform, or cost the most: a benchmark takes up to 45 minutes on
    GitHub's macOS runner. Queuing every commit would pile them up.
  - The nightly perf run still measures a head of `main` that no push run measured.
- The names of `main`'s checks are unchanged, but `changes` is gone: `just ci` ×3,
  `e2e / just e2e` ×2, `release-safety`, `public-api`, `deny`, `audit`, `markers`, `secrets`,
  `ci-ok`.

### How long the queue takes

Recent runs on `main` took, per job:

- `just e2e (macos)` about 17.5 minutes;
- `just ci (macos)` 4 to 9 minutes;
- the hosted jobs up to 24 minutes, `just ci (ubuntu-latest)` the longest.

After a burst of five merges, the last commit's results arrive in about this order:

- the hosted jobs after 25 to 40 minutes. GitHub's limit on hosted jobs at once is shared with
  `perf.yml`, coverage and CodeQL, which run on the same pushes;
- `just ci (macos)` after about 45 minutes;
- `just e2e (macos)` after about 90 minutes.

Merges on a busy day came 5 to 25 minutes apart, so the Mac's e2e line grows during a burst
and drains after it.

## Changes to accepted ADRs

- [ADR 0166](0166-macos-ci-on-a-self-hosted-mac-and-fewer-jobs-per-pull-request.md):
  - "These run on each pull request that changes code", as ADR 0174 changed it. Now: only
    `adr-cited`.
  - "A docs-only pull request skips the code jobs." Now: no pull request runs a code job, and
    `changes` is gone.
  - "For now `ci-ok` is not a required check. Once the runner has stayed online for a week,
    the author adds it to the `main` ruleset as the one required check." Dropped: a required
    check would block every pull request, as none runs `ci-ok`.
  - "The `perf` label runs `perf.yml` on a pull request". Now: a run started by hand on the
    branch does.
  - "A nightly perf run measures only a head of `main` that no push run measured, as when a
    later push cancelled that run". Now: as when a newer push replaced its waiting run. A
    running perf run is never cancelled on `main`.
  - "The orchestrator's merge rule waits for these names". Now: it waits for no CI check, and
    watches `main`'s run after the merge.
  - "Merging `main` into a branch still runs that branch's jobs again". Now: it runs none.
- [ADR 0174](0174-fast-checks-on-pull-requests.md):
  - "A pull request runs the fast checks only". Now: it runs `adr-cited` only.
  - "The `e2e` label runs them on a pull request". Now: no label runs anything. A run of
    `e2e.yml` started by hand on the branch replaces the label.
- [ADR 0049](0049-end-to-end-test-harness.md), as ADR 0174 changed it: "`e2e.yml` runs it
  on pushes to `main` … and on a pull request labelled `e2e`". Now: on pushes to `main`, and
  on a branch when started by hand.
- [ADR 0046](0046-performance-regression-checks.md) and
  [ADR 0081](0081-macos-benchmarks-off-pull-requests.md): "the `perf` label", as ADR 0166
  set it. Now: a run started by hand on the branch.
- [ADR 0021](0021-development-tooling.md): "Known gaps in the hook, all covered by CI".
  Now: CI covers them only after the merge, on `main`.

## Consequences

- A pull request shows one check, `adr-cited`, and uses no runner of the Mac's.
- A Windows-only, macOS-only or Linux-only failure, or an e2e failure the local run skipped,
  reaches `main` and is fixed forward. For a risky change, a run of `ci.yml` started by hand
  on the branch checks all three systems and e2e before the merge.
- Every commit on `main` has a `ci-ok` of its own, so a failure names the merge that caused
  it.
- Several merges in a row hold the Mac longer, as above. `just perf-remote` on the same Mac
  still pauses the runners first.
- The repository's settings need no change: no check is required. The labels `e2e`, `perf`
  and `weston-repeat` can be deleted, or kept, as they do nothing. Fork pull requests still
  need approval for every outside contributor, as `structure.yml` still runs on them.

## Alternatives considered

- **Keep the fast checks on pull requests, without waiting for them:** they would hold
  GitHub's runners for every push to a branch, and their results would come after the
  merge anyway.
- **`secrets` on pull requests as well:** it would find a secret only after the push had
  already made it public, and the hook already scans each commit.
- **`adr-cited` on `main`, reading the merged pull request's description through the API:**
  it would report a missing ADR only after the merge, with no description left to fix.
- **Queue `ci.yml`'s runs on `main` (`cancel-in-progress: false` with `queue: max`):** each
  run would wait for the one before it to end, so five merges would take about two and a half
  hours, though the hosted jobs could run side by side.
- **One running and one waiting run of `ci.yml` on `main`** (`cancel-in-progress: false`, the
  default queue): it bounds the wait, but a replaced commit gets no result of its own, which
  is what this change is for.
- **A bound on the Mac's e2e job only**, as a concurrency group on that job: the commit whose
  e2e job was replaced would end with a cancelled job, and its `ci-ok` would fail.

## Perf queued, Wayland per push, as amended

Amended on 2026-11-29, with the author's choices through the orchestrator. After three quick
merges on 2026-11-28, GitHub cancelled three runs that were still waiting: `perf.yml` for two
commits and `e2e-wayland.yml` for one. GitHub keeps one waiting run per concurrency group, and
a newer one replaces it even with `cancel-in-progress: false`. So those commits had no result of
their own, and the title "every commit on main checked" did not hold.

Measured on `main` that day: `perf.yml` takes about 42 minutes a run, `e2e-wayland.yml` about
20, coverage and CodeQL about 10 to 12 each. All run on GitHub's runners. `perf.yml` never runs
on the self-hosted Mac (ADR 0166); only `just perf-remote` does, by hand, with its runners
paused.

- **`perf.yml` queues every run: `queue: max` with `cancel-in-progress: false`.** GitHub
  then keeps up to 100 runs waiting in the group, starts them first in, first out, and
  replaces none.
  - Each push to `main` is measured, against the push before it, so a regression names one
    merge. The runs never overlap.
  - The cost is time. After a burst of five merges, the last one's result comes about
    3.5 hours later, and a busy day's queue drains in the evening. The nightly run then has
    little left to measure: only a head of `main` whose push run was cancelled by hand.
  - GitHub refuses `queue: max` together with `cancel-in-progress: true`. So a run started
    by hand on a branch waits for the branch's earlier one, and does not cancel it.
  - Checked on 2026-11-29 with three runs started by hand on a branch. The second and third
    both stayed waiting, and when the first was cancelled, the second started.
  - actionlint 1.7.12, its latest release, does not know `queue` yet (rhysd/actionlint#657).
    oso runs actionlint in neither `just ci` nor CI. Run it by hand with
    `-ignore 'unexpected key "queue" for "concurrency"'`, as well as the ignore for the `$/`
    paths of the actions lockfile.
- **`e2e-wayland.yml` has a group of its own for each push to `main`**, as `ci.yml` has. Every
  commit gets a result. Each merge adds one Ubuntu job of about 20 minutes, which runs beside
  the others.
- **Coverage and CodeQL keep the newest waiting commit only**, as above. Each run reads the
  whole code at its head, so a skipped commit loses no alert and no number, only which merge
  caused it.

What this changes above:

- The title, "No CI on pull requests, and every commit on main checked". Now: "No CI on pull
  requests, and main's runs never cancelled". Every commit gets its own run of `ci.yml`, its
  e2e, `e2e-wayland.yml` and `perf.yml`. Coverage and CodeQL check only the newest waiting
  commit.
- "`perf.yml`, `coverage.yml`, `codeql.yml` and `e2e-wayland.yml` never cancel a running run
  on `main` … a newer push replaces the waiting one." Now: only coverage and CodeQL work this
  way.
- "Queuing every commit would pile them up." Now `perf.yml` queues every commit, on GitHub's
  runners alone, one run at a time.
- The rejected alternative "Queue `ci.yml`'s runs on `main`" stays rejected: queued, the
  hosted jobs of several commits could not run side by side.

Rejected:

- **A group of its own for each `perf.yml` run on `main`:** each result would come in about
  42 minutes. But a run would often start before the one before it had uploaded its result,
  and compare with the one before that. A regression would then fail twice. Each run also
  holds two of GitHub's macOS runners, and three quick merges would reach the free plan's
  limit of five.
- **`perf.yml` on the newest commit only:** costs nothing, but a regression would name a range
  of merges.
- **Groups of their own for coverage and CodeQL:** cheap, but they would add nothing but which
  merge caused a finding.
