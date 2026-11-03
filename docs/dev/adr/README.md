# Architecture Decision Records

Each ADR records one decision: its context, the decision, its consequences
and the alternatives considered. ADRs are not rewritten after acceptance. To
change a decision, add a new ADR that supersedes the old one, and update the
old one's status.

| # | Title | Status |
|---|---|---|
| 0001 | [No user scripting](0001-no-user-scripting.md) | Accepted |
| 0002 | [Dev-only hot reload: restart-and-restore plus data reload](0002-dev-hot-reload.md) | Accepted |
| 0003 | [Platform support tiers](0003-platform-tiers.md) | Accepted |
| 0004 | [Workspace crate layout](0004-crate-layout.md) | Accepted |
| 0005 | [OS-native text rendering on a monospace cell grid](0005-text-rendering.md) | Accepted |
| 0006 | [Layered display model and decorations](0006-display-model.md) | Accepted |
| 0007 | [Multiple cursors](0007-multi-cursor.md) | Accepted |
| 0008 | [Minibuffer, menus and popups as floating windows](0008-floating-ui.md) | Accepted |
| 0009 | [Project trust model](0009-project-trust.md) | Accepted |
| 0010 | [Process spawning, network and dependency policy](0010-process-and-network-policy.md) | Accepted |
| 0011 | [Git integration](0011-git-integration.md) | Accepted |
| 0012 | [Native tree-sitter for syntax](0012-tree-sitter.md) | Accepted |
| 0013 | [Persistence: emergency save only](0013-persistence.md) | Accepted |
| 0014 | [Panic strategy](0014-panic-strategy.md) | Accepted |
| 0015 | [Terminal](0015-terminal.md) | Accepted |
| 0016 | [Dev control socket](0016-dev-control-socket.md) | Accepted |
| 0017 | [Documentation and language policy](0017-docs-and-language.md) | Accepted |
| 0018 | [Development process](0018-development-process.md) | Accepted |
| 0019 | [Configuration](0019-configuration.md) | Accepted |
| 0020 | [License](0020-license.md) | Accepted |
| 0021 | [Development tooling](0021-development-tooling.md) | Accepted |

## Template

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD

## Context
## Decision
## Consequences
## Alternatives considered
```
