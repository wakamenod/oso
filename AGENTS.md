# AGENTS.md

oso is an Emacs-like GUI editor in Rust. Priorities, in order: **speed,
security, developer extensibility, fast iteration**. Read
`docs/dev/architecture.md` before larger changes. Decisions are in
`docs/dev/adr/`; do not contradict an accepted ADR without proposing a new
one. Open topics are in `docs/dev/open-questions.md`.

## Language

- Everything in the repo is English: code, comments (`//`, `///`, `//!`),
  identifiers, commit messages and docs.
- The repo is public. Never commit secrets, personal paths or environment
  details, employer or client names, or unfixed vulnerabilities.

## Hard rules

- Never block the UI thread on I/O, parsing or processes.
- Opening a file or project must never execute anything.
- Spawn processes only through `oso-platform::process`. oso has no network
  code.
- Use only the `regex` crate for regular expressions.
- No `unsafe` in `oso-core`. Elsewhere, every `unsafe` block needs a
  `// SAFETY:` comment.
- Commands are plain functions over host-owned state, with no globals. They
  operate on the window's selection set (multiple cursors).
- Ask before adding or upgrading dependencies, or before editing
  `Cargo.toml`, `Cargo.lock`, `deny.toml`, `.claude/` or the files that
  define the checks (`Justfile`, `clippy.toml`, `rust-toolchain.toml`,
  `.cargo/`, `.github/`, `guardrails/`, the guard tests in
  `crates/oso/tests/`, gitleaks config). In auto mode, do not ask before
  editing files; ask only about design changes and new dependencies
  (ADR 0043).
- Dev-only code (control socket, hot reload) stays behind dev cargo features.
- Respect the dependency direction and layer rules (ADR 0038,
  `guardrails/layers.txt`). A new crate, a new crate edge, a new public
  trait or module in a library crate, or a change to the layer rules needs
  an ADR. Each crate's own rules are in its `AGENTS.md`.

## Workflow

- Run `just ci` (check, lint, fmt-check, test) before every commit. Run
  `just deny` and `just audit` when dependencies change, and
  `just release-safety` when dev features change.
- Check user-visible changes in the running app with the `verify-in-app`
  skill.
- Features: a worktree branch, then a PR. Small chores may go to `main`.
- Review your own changes against `REVIEW.md` before asking for review.
- When a decision is made, record it as an ADR (`docs/dev/adr/`) and update
  the related docs.
