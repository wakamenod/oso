# Agentic development workflow

oso is developed with Claude Code. This document describes how agents work
on the project and where humans stay in the loop. The decision record is
[ADR 0018](adr/0018-development-process.md).

## Rules: enforce, review, then document

Agents can miss rules buried in long documents. Rules are therefore handled
in three tiers:

1. **Enforce mechanically** whatever tooling can check: clippy lints,
   `cargo-deny`, hooks, permission settings and CI.
2. **Review** what needs judgment, using [`REVIEW.md`](../../REVIEW.md) and
   reviewer subagents.
3. **Document** intent and rationale in [`AGENTS.md`](../../AGENTS.md), these
   docs and ADRs.

When an agent makes a mistake, add a rule to prevent it from happening again.
Use the first tier that can catch it. Keep `AGENTS.md` short and prune it
regularly.

## Files

```
AGENTS.md                   always loaded: priorities, hard rules, workflow
crates/*/AGENTS.md          crate-specific rules
REVIEW.md                   review checklist
guardrails/                 layer rules, size allowlist, public API snapshots
docs/dev/                   design docs, ADRs, open questions
.claude/settings.json       permissions and hooks
.claude/hooks/              hook scripts
.claude/skills/             repeatable procedures
.claude/agents/             reviewer subagents
clippy.toml, deny.toml      lint and dependency policy
rust-toolchain.toml         pinned toolchain
.github/workflows/          CI, the ADR check, the design health report
.github/pull_request_template.md
```

## Tooling

The setup is recorded in
[ADR 0021](adr/0021-development-tooling.md).

### Permissions and hooks

- `.claude/settings.json` asks for human approval before edits to the files
  listed in `AGENTS.md`, and before `cargo add`, `remove`, `update` or
  `install`.
- Pre-commit hook (`.claude/hooks/pre-commit.sh`): before `git commit`, run
  `just ci` and a gitleaks scan of staged, unstaged and untracked changes.
  Block the commit if either fails, and ask the human if the commit touches
  a file that needs approval. The hook fails closed.
- Post-edit hook (`.claude/hooks/fmt-rs.sh`): run `cargo fmt` after a `.rs`
  file is edited, and nothing heavier.
- Both hooks check the checkout Claude is working in, so they work in
  worktrees.

### Mechanical checks

- Clippy denies undocumented `unsafe` blocks, `allow` attributes without a
  reason, process spawning outside `oso-platform::process`, `std::net`
  sockets, and Unix domain sockets outside the dev control socket
  (`Cargo.toml` `[lints]`, `clippy.toml`).
- `just deny` and `just audit` check dependencies against `deny.toml` and
  the RustSec database. `just secrets` scans the git history.
- Architecture guardrails ([ADR 0038](adr/0038-architecture-guardrails.md),
  [ADR 0039](adr/0039-architecture-guardrail-mechanics.md)):
  - `crates/oso/tests/architecture` enforces the dependency direction
    between crates and modules (`guardrails/layers.txt`), confines wgpu,
    winit and OS bindings to their crates, applies the 600-line file
    limit with its ratchet (`guardrails/size-allowlist.txt`), and rejects
    `static mut`.
  - Clippy bans global state (`OnceLock`, `LazyLock`, `thread_local!`),
    printing and `todo!`, and limits functions to 80 lines and a
    cognitive complexity of 25. `oso-core` denies `unwrap` and `expect`
    outside tests. Exceptions need an `#[expect]` with a reason.
  - `cargo-deny` allows `wgpu` only in `oso-render` and `winit` only in
    `oso`.
  - `just public-api-check` (CI only; it needs nightly) compares the
    public API with `guardrails/public-api`. `just public-api` updates it.
  - The structure workflow fails a pull request that changes the layer
    rules, the crate graph or the API, or grows the allowlist, unless its
    description cites an ADR.
  - A weekly workflow writes `just health-report` (largest files and
    functions, new dependencies, API growth, size exceptions) into a
    GitHub issue.
- CI runs `just ci` on all three platforms, plus `just release-safety`
  (no dev-only code in release builds), `cargo-deny`, `cargo-audit` and
  gitleaks.

### Skills

- `adr`: record a decision with the standard template, and update the index,
  related docs and open questions.
- `add-dependency`: research maintenance, `unsafe` usage, transitive
  dependencies, license and `cargo-deny` results, report back, and wait for
  approval.
- `verify-in-app`: start a dev build, drive it with `oso ctl` over the
  control socket (`send-keys`, `get-state`, `screenshot`), read the
  screenshots, and stop the app.
- `add-command`: implement the function, register it with a docstring and
  argument spec, add a key binding, write unit tests, and check it end to
  end over the control socket with `verify-in-app`
  ([ADR 0034](adr/0034-command-registry-and-keymaps.md)). Regenerating the
  command reference is added once the reference exists.

### Reviewer subagents

Read-only `security-reviewer`, `perf-reviewer` and `arch-reviewer`
([ADR 0038](adr/0038-architecture-guardrails.md)) agents follow the matching
sections of `REVIEW.md`. `arch-reviewer` also checks the architecture docs,
the ADRs and the layer rules. They check the blast radius of a change and ask for
fail-then-pass evidence. They run in parallel after implementation, and their
findings are fixed before human review.

## Flow of a feature

```
plan → implement in a worktree → tests → verify in the app
→ reviewer subagents → PR → 3-OS CI → human gate review → merge
```

Small chores may go directly to `main`.

## Human gates

The author reviews only these, and everything else passes on agent review:

- design decisions (ADRs)
- new dependencies
- new `unsafe`
- security-sensitive code: process spawning, file writing, the trust model,
  the control socket
- merges to `main`

## Documentation language

- Everything in the repository is English: code, comments, identifiers,
  commits and docs.
- User documentation is English by default, with a Japanese edition.
- The repository is public. See
  [`security.md`](security.md#what-must-never-be-committed) for what must
  never be committed.
