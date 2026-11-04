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
| 0022 | [Text buffer and file loading](0022-text-buffer-and-file-loading.md) | Accepted |
| 0023 | [Control socket protocol and restart-and-restore](0023-control-socket-protocol-and-restart.md) | Accepted |
| 0024 | [Dev build profile and build-time budget](0024-dev-build-profile.md) | Accepted |
| 0025 | [Key-to-photon latency measurement](0025-latency-measurement.md) | Accepted |
| 0026 | [M2 UI layout](0026-m2-ui-layout.md) | Accepted |
| 0027 | [Theme](0027-theme.md) | Accepted |
| 0028 | [Handling secrets in memory and on disk](0028-secrets-handling.md) | Accepted |
| 0029 | [File changes on disk and editing while loading](0029-file-changes-on-disk.md) | Accepted |
| 0030 | [M2 editing command semantics](0030-m2-editing-commands.md) | Accepted |
| 0031 | [Claude Code integration: host the official TUI](0031-claude-code-integration.md) | Accepted |
| 0032 | [Control socket transport, limits and release check](0032-control-socket-transport.md) | Accepted |
| 0033 | [Key presses and Emacs key notation](0033-key-presses-and-notation.md) | Accepted |
| 0034 | [Command registry, keymaps and key resolution](0034-command-registry-and-keymaps.md) | Accepted |
| 0035 | [SumTree, rope chunks and the cell grid](0035-sumtree-rope-and-cell-grid.md) | Accepted |
| 0036 | [Text system, glyph atlas and the default font](0036-text-system-and-glyph-atlas.md) | Accepted |
| 0037 | [Test pyramid](0037-test-pyramid.md) | Accepted |
| 0038 | [Architecture guardrails](0038-architecture-guardrails.md) | Accepted |
| 0039 | [Architecture guardrail mechanics](0039-architecture-guardrail-mechanics.md) | Accepted |
| 0040 | [Buffers, file loading, cursor motion and scrolling](0040-buffers-loading-motion-and-scrolling.md) | Accepted |
| 0041 | [Module responsibility headers](0041-module-responsibility-headers.md) | Accepted |
| 0042 | [Statics, API snapshot upkeep and render errors](0042-statics-snapshots-and-render-errors.md) | Accepted |
| 0043 | [No edit approval in auto mode](0043-no-edit-approval-in-auto-mode.md) | Accepted |
| 0044 | [Control socket trust and secret-pattern buffers](0044-control-socket-trust-and-secret-buffers.md) | Accepted |
| 0047 | [Process spawning and the restart handoff](0047-process-spawning-and-restart-handoff.md) | Accepted |

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
