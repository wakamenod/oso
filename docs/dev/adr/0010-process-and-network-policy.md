# 0010. Process spawning, network and dependency policy

- Status: Accepted
- Date: 2026-11-03

## Context

Rules that the compiler does not check are easy for agents to break silently.
Process spawning, networking and dependencies are the riskiest areas.

## Decision

- **Process spawning:** only `oso-platform::process` may spawn, enforced by
  clippy `disallowed_methods`.
  - Executables resolve from absolute paths in user config, or from `PATH`
    without the current or project directory.
  - The environment is sanitized and every spawn is logged.
- **No network code in oso.** `cargo-deny` bans networking crates. Network
  access happens only in child processes the user starts, such as `git push`.
- **Regex:** only the linear-time `regex` crate. Backtracking engines are
  banned.
- **Dependencies:**
  - `cargo-deny` checks advisories, licenses, sources and bans.
  - `cargo-audit` runs in CI and builds use `--locked`.
  - New dependencies need human approval.
  - cargo-vet is not adopted for now.
- **Unsafe:** `oso-core` forbids it. Elsewhere, `// SAFETY:` comments are
  required.

## Consequences

- Most security rules fail the build when broken, instead of relying on
  review.
- Adding a dependency is deliberately slow.

## Alternatives considered

- **Convention-only rules in docs:** agents miss them.
