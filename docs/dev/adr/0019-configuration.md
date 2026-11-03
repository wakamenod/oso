# 0019. Configuration

- Status: Accepted
- Date: 2026-11-03

## Context

There is no scripting ([ADR 0001](0001-no-user-scripting.md)), so user
customization is declarative. Repositories must not be able to configure
anything that executes ([ADR 0009](0009-project-trust.md)).

## Decision

- User configuration is **TOML**: key bindings, menus, theme, fonts and
  per-mode settings. It is validated against a schema and reloads on change.
- Inside repositories, oso reads **only EditorConfig**: indentation, line
  endings and similar declarative settings. No oso-specific project files.
- Default key bindings follow vanilla Emacs (no evil), plus the author's
  customizations as shipped defaults.

## Consequences

- Configuration can never execute code.
- Nested menu definitions are somewhat verbose in TOML.

## Alternatives considered

- **KDL:** nicer for nested menus, but less common and with weaker tooling.
- **RON:** maps directly to Rust types, but awkward to write by hand.
- **An oso-specific project file:** more flexible, but more attack surface.
