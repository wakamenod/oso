# 0048. Permission prompts depend on the mode

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0043](0043-no-edit-approval-in-auto-mode.md) says agents do not ask about
file changes in auto mode, but the author was still prompted. ADR 0021 and
ADR 0043 assumed that the approval prompts reach a human only in the default
mode. That is wrong. The Claude Code documentation says explicit `ask` rules
and a `PreToolUse` hook's `"ask"` still prompt in auto mode, and settings rules
cannot depend on the mode.

## Decision

- The `Edit(...)` ask rules leave `.claude/settings.json`. A `PreToolUse` hook
  (`.claude/hooks/gated-edit.sh`) asks instead, using the same file list.
- That hook and the pre-commit hook read `permission_mode` from their input.
  They do not ask in `auto`, and they ask in every other mode. A missing or
  unreadable mode counts as not auto.
- `.claude/hooks/gated-paths.sh` holds the one list of gated files.
- The `Bash(cargo add:*)`, `remove`, `update` and `install` ask rules stay, in
  every mode. Dependency changes remain the author's decision (ADR 0043).

## Consequences

- Edits and commits of gated files no longer prompt in auto mode.
- These can still prompt in auto mode:
  - the `cargo` commands above
  - the classifier pausing auto mode after repeated blocks. Writes to protected
    paths such as `.claude/` and `.cargo/` go to the classifier.
- A session may need a restart to pick up the new settings.

## Alternatives considered

- **Dropping the prompts in every mode:** simpler, but it also removes the
  approval step in the default mode, which ADR 0043 keeps.
- **Keeping the ask rules:** they cannot tell the modes apart.
