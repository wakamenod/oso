# 0174. Fast checks on pull requests

- Status: Accepted, amended by 0188
- Date: 2026-11-23

## Context

The author is oso's only user and merges a pull request once its fast checks pass, without
waiting for the macOS `just ci` or either `just e2e`. Under
[ADR 0166](0166-macos-ci-on-a-self-hosted-mac-and-fewer-jobs-per-pull-request.md) those
still ran on every pull request, so they held the Mac's two runners and kept the pull
request showing as running after it could merge. The author chose this through the
orchestrator on 2026-11-23.

## Decision

- **A pull request runs the fast checks only:** `just ci (ubuntu-latest)`,
  `just ci (windows-latest)`, `release-safety`, `public-api`, `deny`, `audit`, `markers`,
  `secrets`, `changes`, and `adr-cited` in `structure.yml`. Its `ci-ok` waits for these
  and nothing else, so it turns green when they pass.
- **`just ci (macos)` runs on pushes to `main` only.** The commit hook runs the same
  `just ci` on the author's Mac for commits made through Claude Code, but not for merges
  ([ADR 0021](0021-development-tooling.md)). The macOS public API snapshot and the macOS
  release-safety search, which are steps of that job, move to `main` with it.
- **`just e2e` (Linux and macOS) and the pixel tests run on pushes to `main`**, from their
  own reusable workflow, `e2e.yml`, which `ci.yml` calls. `ci-ok` on `main` waits for them,
  as for every other job. On `main` their checks are named `e2e / just e2e (ubuntu-latest)`
  and `e2e / just e2e (macos)`. `ci.yml` never calls `e2e.yml` on a pull request: there
  `e2e.yml` skips its job without the label, and `ci-ok` would count that as passed.
- **The `e2e` label runs them on a pull request**, from `e2e.yml`'s own `pull_request`
  trigger, as the `perf` label runs `perf.yml`: on the push or label event that finds it,
  and on each later push while it stays. It runs on a docs-only pull request too.
  - Only people with triage access can add a label, so a fork cannot start it. A fork's
    macOS job still goes to `macos-latest`, and the runner's hook still refuses it.
  - Labelling a fork's pull request `e2e` (or `perf`) runs its head's workflow files, as
    approving it does, so read its `.github/` changes first.
  - Adding another label runs nothing. Its run has a concurrency group of its own, so it
    cancels no run that tests.
  - The pull request's `ci-ok` does not wait for them. They are `just e2e (ubuntu-latest)`
    and `just e2e (macos)` in the `E2E` workflow.
- "E2E under Weston, repeated" and `perf.yml` are unchanged.

## Changes to accepted ADRs

- [ADR 0166](0166-macos-ci-on-a-self-hosted-mac-and-fewer-jobs-per-pull-request.md):
  - "These run on each pull request that changes code: in `ci.yml`: `just ci` ×3,
    `just e2e` ×2 (moved there from `e2e.yml`) …". Now: `just ci` ×2 (Linux, Windows),
    with the rest of that list unchanged. The macOS `just ci` and `just e2e` ×2 run on
    pushes to `main`, and `just e2e` runs on a pull request labelled `e2e`. The e2e job is
    in `e2e.yml` again, which `ci.yml` calls on `main`.
  - "`ci-ok` … fails when any other job of `ci.yml` failed or was cancelled." It still
    does. On a pull request those jobs are only the fast checks.
  - "A pull request starts 14 jobs instead of about 22 … two run on the Mac." Now 11 run,
    and none runs on the Mac unless the pull request is labelled `e2e`. The skipped `e2e`
    jobs take no runner.
  - "The orchestrator's merge rule waits for these names". Now, on a pull request:
    `just ci (ubuntu-latest)`, `just ci (windows-latest)`, `release-safety`, `public-api`,
    `deny`, `audit`, `markers`, `secrets`, `changes`, `adr-cited`, `ci-ok`. The macOS
    `just ci` and both `just e2e` are watched on `main`.
- [ADR 0049](0049-end-to-end-test-harness.md):
  - "`e2e.yml` runs `just e2e` … skipping changes only to `docs/`", as ADR 0166 changed
    it. Now: `e2e.yml` runs it on pushes to `main`, where `ci.yml` calls it, and on a pull
    request labelled `e2e`, docs-only or not.
  - "The macOS job adds about 3 minutes of runner time to each pull request". Now: only to
    a pull request labelled `e2e`.
- [ADR 0021](0021-development-tooling.md): "Known gaps in the hook, all covered by CI".
  Now: CI covers them for macOS `just ci` and for e2e only after the merge, on `main`. A
  merge of `main` into a branch is not checked on macOS before that branch merges.

## Consequences

- A pull request no longer holds the Mac's runners, and shows green when its fast checks
  pass.
- A pull request can merge with a macOS-only or e2e failure, which is found on `main` and
  fixed forward. The commit hook's local `just ci` covers the macOS build and tests for
  commits made through Claude Code. Nothing covers e2e before the merge unless the label
  is added.
- Fewer jobs warm the Mac's `pr` build folders, so a labelled pull request's macOS e2e
  builds more from nothing.

## Alternatives considered

- **The label in `ci.yml`'s own `pull_request` types:** `ci.yml` would then run on every
  label event. A run started by another label would end with a `ci-ok` of its own, which
  reads as the pull request's latest.
- **A copy of the e2e job in a label-only workflow:** two copies of the runner and build
  steps would need to be kept alike. The reusable workflow keeps one.
