# 0021. Development tooling

- Status: Accepted
- Date: 2026-11-03

## Context

[ADR 0018](0018-development-process.md) requires the tooling for agentic
development (hooks, skills, reviewer agents, CI) to be built early in M1, and
[ADR 0010](0010-process-and-network-policy.md) relies on lints and
`cargo-deny` to enforce its rules. This ADR records how that tooling is set
up. See [`agentic-workflow.md`](../agentic-workflow.md).

## Decision

- **Commit checks:** a Claude Code `PreToolUse` hook
  (`.claude/hooks/pre-commit.sh`) runs before every `git commit`. It runs
  `just ci` and a gitleaks scan of staged, unstaged and untracked changes,
  and blocks the commit if either fails. CI runs the same checks. There are
  no git hooks (`.githooks`).
- **Formatting:** a `PostToolUse` hook runs `cargo fmt` after a Rust file is
  edited, and nothing heavier.
- **Permissions:** `.claude/settings.json` asks before edits to
  `Cargo.toml`, `Cargo.lock`, `deny.toml` and `.claude/`, and before
  `cargo add`, `remove`, `update` and `install`.
- **Lints:** lint levels live in `Cargo.toml` `[lints]`
  (`undocumented_unsafe_blocks` and `disallowed_methods` are `deny`). The
  `disallowed_methods` list lives in `clippy.toml` and bans
  `std::process::Command::new`.
- **Toolchain:** pinned to an exact Rust version in `rust-toolchain.toml`,
  with clippy and rustfmt. Upgrading it is a deliberate change.
- **Dependencies:** `deny.toml` checks advisories, a minimal permissive
  license allowlist ([ADR 0020](0020-license.md)), crates.io-only sources,
  and bans on networking crates and backtracking regex engines. `just deny`
  and `just audit` run `cargo-deny` and `cargo-audit`. Cargo commands in the
  `Justfile` use `--locked`.
- **Skills and agents:** the `adr` and `add-dependency` skills, and the
  read-only `security-reviewer` and `perf-reviewer` agents.
- **CI (GitHub Actions):**
  - `just ci` on Linux, macOS and Windows, plus `cargo-deny`, `cargo-audit`
    and a gitleaks scan of the full history.
  - Third-party actions are pinned by commit SHA with a version comment.
  - The workflow token is read-only, checkout does not persist credentials,
    and only `main` writes the build cache.
  - Downloaded tools are verified by checksum.
- **Branch protection:** a ruleset on `main` blocks force pushes and branch
  deletion only, so small chores can still be pushed directly
  ([ADR 0018](0018-development-process.md)).

## Consequences

- Breaking `just ci` or committing a detectable secret is blocked before the
  commit when working through Claude Code, and caught by CI otherwise.
- Commits through Claude Code wait for a full `just ci` run.
- Action SHAs, the gitleaks checksum and the toolchain version need manual
  updates.
- The CI check that release builds contain no dev-only features is deferred
  to M1-7, when dev features exist.
- The `add-command` and `verify-in-app` skills and `crates/*/AGENTS.md` are
  still planned.

## Alternatives considered

- **Git hooks (`.githooks`):** need per-clone setup and are easy to skip.
  The Claude hook covers agent commits, and CI covers everything.
- **Lint levels in `.cargo/config.toml` or crate attributes:** `[lints]` is
  the standard place and applies to all targets.
- **Unpinned or tag-pinned actions:** tags can be moved by the action's
  owner.
- **Required status checks on `main`:** would block direct chore pushes.
