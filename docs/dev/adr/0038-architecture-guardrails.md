# 0038. Architecture guardrails

- Status: Accepted, amended by 0111
- Date: 2026-11-03

## Context

Agents write most of oso's code, one change at a time. Each change can be
locally reasonable while the overall design drifts:

- dependencies start to point the wrong way
- modules grow into grab bags
- public APIs widen without anyone deciding to
- global state creeps into commands

Existing checks cover security (clippy bans, `cargo-deny`) and performance,
but not structure. Some core files already pass 1,000 lines including tests,
and the author's previous editor client grew files of over 4,000 lines.

The principle from [`agentic-workflow.md`](../agentic-workflow.md) applies:
enforce mechanically, then review, then document. New limits use a
**ratchet**: existing violations are recorded in an allowlist that may only
shrink, so nothing has to be fixed at once and nothing gets worse.

## Decision

### Enforced mechanically

1. **Dependency direction.**
   - **Between crates:**
     - `oso-core` and `oso-platform` depend on no workspace crate.
     - `oso-render` may depend on `oso-core` and `oso-platform`.
     - Nothing depends on the tip crate `oso`.
   - **External crates stay in their owning crate,** enforced with
     `cargo-deny` `wrappers`:
     - `wgpu` only in `oso-render`
     - `winit` only in `oso`
     - OS bindings (`objc2*`, `libc`, Windows bindings) only in
       `oso-platform`
   - **Between modules:** a rules file lists the allowed layers inside each
     crate (for example, `display_map` must not use `command`). A test scans
     `crate::` paths, fails on a forbidden edge, and fails on cycles.
2. **Public API snapshots.**
   - The public API of each library crate is recorded with
     `cargo-public-api` in a committed file.
   - CI fails when the API differs from the file. `just public-api` updates
     it, so every API change shows up as a reviewed diff.
   - The job installs a nightly toolchain only for rustdoc JSON.
3. **Size limits, with a ratchet.**
   - At most 600 lines of non-test code per file.
   - Clippy's `too_many_lines` (80) and `cognitive_complexity` (25)
     thresholds apply.
   - Existing violations are listed in an allowlist. A test fails when a
     listed file is back under the limit, so the entry must be removed.
   - Function-level exceptions need `#[expect(..., reason = "...")]`.
4. **Design-rule lints.**
   - `disallowed_types` and `disallowed_macros` ban global state:
     - `OnceLock`
     - `LazyLock`
     - `thread_local!`
     - `static mut` (through the matching lint)

     Exceptions need an `#[expect]` with a reason. This enforces "commands
     hold no global state"
     ([ADR 0002](0002-dev-hot-reload.md), [ADR 0034](0034-command-registry-and-keymaps.md)).
   - `oso-core` denies `unwrap_used` and `expect_used` outside tests.
   - `println!`, `eprintln!`, `dbg!`, `todo!` and `unimplemented!` are
     banned. Use `log` and real errors instead.

### Review

5. **`arch-reviewer` subagent.** It is read-only and runs alongside
   `security-reviewer` and `perf-reviewer`. It checks a change against
   `architecture.md`, `display-model.md`, the ADRs and the layer rules, and
   flags:
   - responsibilities leaking into the wrong module
   - duplication
   - abstractions leaking platform or GPU details
   - premature generality
   - naming that hides intent
6. **Structural changes need an ADR.** Structural changes are:
   - a new crate
   - a new edge between crates
   - a change to the layer rules or the allowlist
   - a new public trait or public module in a library crate

   The pull request template asks for the ADR. CI fails when the layer rules
   or the crate graph change and the PR description cites no ADR.

### Documentation and inspection

7. **Module responsibility headers.**
   - Every source file starts with a `//!` comment that states the
     module's responsibility and what it must not depend on. A test fails
     on files without one.
   - Each crate gets a short `AGENTS.md` with its rules.
8. **Weekly design health check.** A scheduled CI job reports:
   - the largest files and functions, and how they grew
   - new dependency edges
   - the growth of the public API
   - allowlist entries
   - surviving mutants ([ADR 0037](0037-test-pyramid.md))

   The report becomes a GitHub issue, and its findings feed regular
   refactoring pull requests.

## Consequences

- Most structural drift fails the build instead of waiting for review.
- The thresholds are starting points. Changing them is itself a structural
  change and needs an ADR.
- A nightly toolchain appears in CI for the public API job only.
- Implementation is planned as M1-10. Until then, the rules are reviewed by
  hand.

## Alternatives considered

- **Review only:** agents and reviewers both miss gradual drift.
- **Fixing all existing violations before adding limits:** a large, risky
  refactor. The ratchet gets the same end state gradually.
- **External architecture tools (`cargo-modules` and similar) as the
  enforcement mechanism:** useful for graphs and the health report, but a
  small in-repo test with a rules file is easier to keep exact and to
  extend.
