# 0021. Development tooling

- Status: Accepted, amended by 0043 and 0080
- Date: 2026-11-03

## Context

[ADR 0018](0018-development-process.md) requires the tooling for agentic
development (hooks, skills, reviewer agents, CI) to be built early in M1, and
[ADR 0010](0010-process-and-network-policy.md) relies on lints and
`cargo-deny` to enforce its rules. This ADR records how that tooling is set
up. See [`agentic-workflow.md`](../agentic-workflow.md).

## Decision

- **Commit checks:** a Claude Code `PreToolUse` hook
  (`.claude/hooks/pre-commit.sh`) runs before every Bash command that looks
  like `git commit`. The match is loose and errs toward extra runs.
  - It runs `just ci` and a gitleaks scan of staged, unstaged and untracked
    changes, ignoring `gitleaks:allow` comments. It blocks the commit if
    either fails.
  - It asks the human when the commit touches a file that needs approval,
    however that file was edited.
  - It fails closed: any unexpected error, a timeout or a missing script
    blocks the commit. It also blocks `git -C <dir> commit`.
  - CI runs the same checks. There are no git hooks (`.githooks`).
- **Formatting:** a `PostToolUse` hook runs `cargo fmt` after a Rust file is
  edited, and nothing heavier.
- **Permissions:** `.claude/settings.json` asks before edits to files that
  need approval, and before `cargo add`, `remove`, `update` and `install`.
  These files are `Cargo.toml`, `Cargo.lock`, `deny.toml`, the `.claude/`
  configuration, and the files that define the checks: `Justfile`,
  `clippy.toml`, `rust-toolchain.toml`, `.cargo/`, `.github/` and gitleaks
  configuration.
- **Lints:** lint levels live in `Cargo.toml` `[lints]`
  (`undocumented_unsafe_blocks`, `disallowed_methods`, `disallowed_types`
  and `allow_attributes_without_reason` are `deny`). The lists live in
  `clippy.toml`: process spawning APIs (`std::process::Command::new` and its
  async counterparts) and `std::net` sockets are banned.
- **Toolchain:** pinned to an exact Rust version in `rust-toolchain.toml`,
  with clippy and rustfmt. Upgrading it is a deliberate change.
- **Dependencies:** `deny.toml` checks all features for advisories, a
  minimal permissive license allowlist ([ADR 0020](0020-license.md)),
  crates.io-only sources, and bans on networking crates, backtracking regex
  engines and process-spawning helper crates. `just deny`
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
  - Tools are installed with checksum verification: through
    `taiki-e/install-action` with no unverified fallback, or by a pinned
    sha256 for gitleaks. `cargo-deny-action` is not used because its image
    downloads `cargo-deny` without a checksum.
  - The gitleaks job fails if the repository contains a gitleaks allowlist.
- **Branch protection:** a ruleset on `main` blocks force pushes and branch
  deletion only, so small chores can still be pushed directly
  ([ADR 0018](0018-development-process.md)).

## Consequences

- Through Claude Code, most commits that break `just ci` or contain a
  detectable secret are blocked before they are made. CI and GitHub push
  protection catch the rest.
- Commits through Claude Code wait for a full `just ci` run (about 30 s from
  a cold build today).
- Known gaps in the hook, all covered by CI:
  - It checks the working tree as it is before the Bash command runs, so a
    command that both edits files and commits is checked on the old state.
    This includes `git add -f` of an ignored file in the same command.
  - It does not see commits made through git aliases, scripts,
    `cd <other checkout> && git commit`, plumbing (`commit-tree`), or
    `merge`, `cherry-pick`, `revert` and `am`.
- The approval prompts (Edit ask rules and the commit-time ask) reach a
  human only in the default permission mode. In auto mode, Bash can edit
  these files without the Edit prompt, and the commit-time ask was observed
  to pass without a prompt. There, the agent must ask in the conversation
  before editing them, and PR review is the last line of defense.
- The format hook runs `cargo` in the checkout being edited, which honors that
  checkout's `rust-toolchain.toml`. Do not edit untrusted checkouts with these
  hooks enabled.
- Action SHAs, the gitleaks checksum and the toolchain version need manual
  updates.
- The CI check that release builds contain no dev-only features is deferred
  to M1-7, when dev features exist.
- With `wildcards = "deny"`, `cargo-deny` treats path dependencies without a
  version as wildcards. When M1-2 splits the workspace, add
  `allow-wildcard-paths = true` to `deny.toml` (the crates are
  `publish = false`) or give path dependencies a version.
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
