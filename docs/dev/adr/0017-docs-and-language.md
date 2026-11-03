# 0017. Documentation and language policy

- Status: Accepted
- Date: 2026-11-03

## Context

The repository is public on GitHub. Design docs must be readable from every
git worktree and from CI, so they need to be committed. Public docs are not a
security risk in themselves, because the source is already public. Some
content must still never be published.

## Decision

- Everything in the repository is **English**: code, comments, identifiers,
  commit messages and docs.
- Design docs and ADRs are committed **Markdown** under `docs/dev/`.
- User documentation is English by default, with a Japanese edition under
  `docs/user/`.
- Never commit any of the following:
  - secrets
  - personal paths or environment details
  - employer or client names
  - descriptions of unfixed vulnerabilities, which go through GitHub private
    security advisories
- Secret scanning runs with GitHub push protection and gitleaks.

## Consequences

- Agents in any worktree see the same design docs.
- Private notes and thinking in Japanese stay outside the repository.

## Alternatives considered

- **Japanese org docs kept out of git:** not visible in worktrees or CI.
- **A private repository:** more freedom, but CI minutes on macOS and Windows
  are limited, and GitHub's free security features are weaker.
