# 0166. macOS CI on a self-hosted Mac, and fewer jobs per pull request

- Status: Accepted
- Date: 2026-11-21

## Context

CI has become the bottleneck. On a recent pull request:

- the Linux and Windows jobs took 8 to 12 minutes;
- the macOS jobs (`just ci`, `just e2e`, the build time) were the slow part. GitHub's hosted
  macOS arm64 runners queued for a long time ("not acquired by Runner … capacity
  constraints"), and the jobs hit the 30-minute timeout, so they were cancelled and run again;
- about 22 jobs ran for each push, and each merge of `main` into a branch ran all of them again.

[ADR 0081](0081-macos-benchmarks-off-pull-requests.md) already took the macOS benchmarks
off pull requests. It turned down a self-hosted Mac because it "would run code from pull
requests, forks included, on a machine that is not thrown away after each job".

The repository is public. Its only ruleset on `main` blocks deletion and force pushes, and no
status check is required. Fork pull requests need approval only from first-time contributors.

## Decision

The author accepted this on 2026-11-21, through the orchestrator.

### macOS on a self-hosted Mac

- `just ci (macos)` and `just e2e (macos)` run on a runner on the author's Mac with the labels
  `[self-hosted, macOS, ARM64, oso]`. Setting it up, updating it and removing it are in
  [`docs/dev/ci-runner.md`](../ci-runner.md). The author does it by hand.
- **A fork's pull request never runs on the Mac.** For `pull_request`, GitHub runs the
  workflow files of the pull request's head, so a fork can edit any condition in them. The
  boundary is therefore on the machine and in the settings, and the workflow is the third
  layer:
  1. The runner's job-started hook (`ACTIONS_RUNNER_HOOK_JOB_STARTED`), a root-owned script
     outside the runner's folder, fails the job before any step runs unless:
     - the repository is this one, and
     - the event is a push to `main`, a `pull_request` whose head repository is this one,
       `schedule` or `workflow_dispatch`.

     Every other event is refused, `pull_request_target` included.
  2. Fork pull requests need approval for every outside contributor, not only first-time
     ones (a repository setting). The author reads a fork's workflow changes before approving.
  3. The unmodified workflows send a fork's macOS jobs to GitHub's `macos-latest`, an
     ephemeral VM:
     `github.event.pull_request.head.repo.full_name != github.repository`.
- **The trust boundary is write access to the repository.** Today that means the author and
  the author's agents, who can push to `main` directly anyway. A branch from this repository
  could change the runner user's own files. That is accepted.
- No workflow uses `pull_request_target`. The token stays `contents: read`, checkout does not
  persist credentials, and no secret is used. The only credential on the Mac is the runner's
  registration, which can take jobs but cannot push, and is revoked by removing the runner.
- **The runner is a dedicated standard macOS user.** It is not an admin and has no Apple ID,
  keys or keychain items. Other users' homes are `chmod 700`, because every local user is in
  the `staff` group, which can read a home by default. The user stays logged in to a GUI
  session, because `just e2e` opens windows. The runner is the Mac mini, with that user at
  its console. One runner, so jobs run one at a time and e2e windows do not compete for focus.
- **The runner is not ephemeral.** Re-registering it for each job would need an admin token
  on the Mac.
  - Checkout cleans the workspace (`git clean -ffdx`) for each job.
  - The build (`target`) is kept outside the workspace, as a link: one folder for `main` and
    one for everything else, so `main` never builds on what a branch left.
  - The hooks kill processes a job left behind and keep the build folders under 40 GB.
  - GitHub-hosted runners keep `Swatinem/rust-cache` as before. The self-hosted Mac skips it.
- The pinned tools keep their checksums. `install-just` and `install-zig` are unchanged, and
  `fetch-ghostty-vt` skips installing minisign when the Mac already has it.
- **While the runner is offline, macOS jobs stay queued**, and GitHub fails them after
  24 hours. Setting the repository variable `OSO_MACOS_RUNNER` to a runner label, such as
  `macos-latest`, sends them to GitHub's runners until it is unset. Re-run the queued jobs
  after setting it.
- The benchmarks and the build time stay on GitHub's runners. Their thresholds were set
  there, and the Mac's other jobs would add noise.

### Fewer jobs per pull request

- **These run on each pull request** that changes code:
  - in `ci.yml`: `just ci` ×3, `just e2e` ×2 (moved there from `e2e.yml`),
    `release-safety`, `public-api`, `deny`, `audit`, `markers` and `secrets`;
  - in `structure.yml`: `adr-cited`.
- **These run on pushes to `main`, nightly at 17:00 UTC, and by hand**, and not on pull
  requests:
  - benchmarks and the build time ×3 (`perf.yml`);
  - coverage;
  - CodeQL (daily now, weekly before);
  - the headless-Wayland flows, now in `e2e-wayland.yml`.
- **A docs-only pull request skips the code jobs.** It is one that changes only files under
  `docs/` and Markdown files at the repository root. Markdown elsewhere is not docs:
  `crates/oso-term/third-party.md` and `patches/README.md` are test inputs. Such a pull
  request runs `changes`, `markers`, `secrets`, `adr-cited` and `ci-ok`.
- **`ci-ok` is a summary job in `ci.yml`.** It runs always and fails when any other job of
  `ci.yml` failed or was cancelled. A skipped job counts as passed there, and nowhere else.
  For now `ci-ok` is not a required check. Once the runner has stayed online for a week, the
  author adds it to the `main` ruleset as the one required check.
- **The `perf` label runs `perf.yml` on a pull request**, on Linux and macOS: the benchmarks
  and the build time ×3, compared with `main` as before. Only people with triage access can
  add a label, so a fork cannot start it. Adding another label measures nothing, and the
  workflow can still be started by hand.
- **A regression found on `main` or nightly is not missed.**
  - The comparing job fails, so the run on `main` shows red, and GitHub emails whoever pushed.
  - A `perf-report` job opens the issue "Performance regression on main", or adds a comment to
    it. The issue holds the summaries and the run's link. `perf-report` holds the only write
    token (`issues: write`) and runs no repository code.
  - A push's result still becomes the baseline. So a regression accepted on purpose fails
    once, not on every later run.
- **A nightly perf run measures only a head of `main` that no push run measured**, as when a
  later push cancelled that run. Otherwise it would compare a commit with itself and report
  noise.
- **Workflows with a schedule have the event in their concurrency group**, so a nightly run
  does not cancel a push's.

## Changes to accepted ADRs

- [ADR 0046](0046-performance-regression-checks.md):
  - "Pushes to `main` only report." Now: they fail on a regression, open or update the
    regression issue, and still become the baseline.
  - "The workflow takes about 10 minutes of wall-clock time on each pull request." Now:
    pull requests run it only with the `perf` label.
- [ADR 0081](0081-macos-benchmarks-off-pull-requests.md):
  - "The `benchmarks` job of `perf.yml` runs on Linux only for pull requests" and "The
    build-time jobs stay on every platform." Now: neither runs on a pull request without the
    `perf` label, and with it both run on every platform they cover.
  - Its rejected alternative, a self-hosted Mac, is taken. A fork's code never runs there, for
    the reasons above.
- [ADR 0049](0049-end-to-end-test-harness.md): "`e2e.yml` runs `just e2e` on Linux …
  and on macOS … It is its own workflow." Now: the `e2e` job is in `ci.yml`, behind the same
  docs-only check, and the macOS job runs on the self-hosted Mac. It now runs on every push to
  `main`, docs-only pushes included, as the rest of `ci.yml` does.
- [ADR 0050](0050-test-quality-checks.md): "`coverage.yml` runs `just coverage` on pull
  requests that change more than `docs/`." Now: on pushes to `main` that change more than
  `docs/`, nightly and by hand, still only in the job summary.

## Consequences

- A pull request starts 14 jobs instead of about 22 (perf.yml's four jobs show as skipped, taking no runner), and a docs-only one starts 5. Of the
  14, two run on the Mac, and none waits for GitHub's macOS runners unless it comes from a
  fork.
- The Mac's jobs run one at a time. With several pull requests open they queue on the Mac,
  but without the hosted runners' timeouts. They build incrementally from the kept `target`.
- A pull request no longer shows coverage, CodeQL findings, benchmarks or build time. They
  appear after the merge, on `main`, or nightly. Ask for benchmarks with the `perf` label.
- The orchestrator's merge rule waits for these names:
  - `just ci (ubuntu-latest)`, `just ci (macos)`, `just ci (windows-latest)`;
  - `just e2e (ubuntu-latest)`, `just e2e (macos)`;
  - `release-safety`, `public-api`, `deny`, `audit`, `markers`, `secrets`, `changes`,
    `adr-cited`, `ci-ok`.
- `(macos-latest)` became `(macos)` in two names, as the job may run on either kind of runner.
- When the Mac is off, macOS checks wait until it is back or `OSO_MACOS_RUNNER` is set.
- `just perf-remote` on the same Mac pauses the runner first (`svc.sh stop`), or CI jobs add
  noise to its numbers.
- **Residuals:**
  - A job on the Mac can reach the local network. The runner doc gives an optional packet
    filter rule.
  - A branch from this repository can change the runner user's files, its hooks setting
    included, so a later job may run without the hooks. The hook scripts themselves are
    root-owned.
  - GitHub does not say whether the job-completed hook runs after a failed job, so the
    job-started hook cleans up too.
- Merging `main` into a branch still runs that branch's jobs again, though now 14 of them.

## Alternatives considered

- **A guard only in the workflows' `if`:** a fork's pull request runs its own copy of the
  workflow and can remove it.
- **An ephemeral runner, registered again for each job:** needs a token with admin rights
  stored on the Mac, a secret that a job could read.
- **A wiped `target` for every job, restored from `rust-cache`:** safer against stale
  artifacts, but much slower. Separate folders for `main` and branches keep `main`'s results
  independent.
- **Two runners on the Mac:** faster, but two e2e runs would take the focus from each other.
- **A required check now:** an offline runner would block every merge until
  `OSO_MACOS_RUNNER` is set. It waits until the runner has proved itself.
- **The benchmarks on the self-hosted Mac:** steadier hardware, but its thresholds would need
  setting again, and its other jobs add noise. `just perf-remote` already gives steady
  numbers when they matter.
- **Path filters at the workflow level (`paths-ignore`) for everything:** a skipped workflow
  reports no check at all, so a required check would wait forever. `ci-ok` passes instead.
