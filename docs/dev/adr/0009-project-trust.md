# 0009. Project trust model

- Status: Accepted
- Date: 2026-11-03

## Context

The author opens third-party repositories and uses git and language servers.
In editors, opening a file or project has repeatedly been made to execute
code, for example:

- Vim modelines (CVE-2019-12735)
- Emacs enriched-mode (CVE-2017-14482)
- Emacs Lisp completion macro expansion (CVE-2024-53920)
- VS Code workspace settings, which led to Workspace Trust

Language servers such as rust-analyzer run a project's `build.rs` and proc
macros.

## Decision

- Projects open silently in **restricted mode**, with a modeline indicator
  and no popup. `M-x trust-project` trusts a project.
- Trust is stored in user config, never in the repository.
- Restricted mode allows viewing, editing, saving, in-process highlighting,
  in-process git status and opening a terminal (an explicit user action).
- Trusted mode adds language servers, formatters and format on save, git
  write operations (hooks run), and org babel execution on explicit action.
- Nothing executes on open: no modelines, no file-local variables, no
  `shell:` or `elisp:` link types.

## Consequences

- Opening any repository is safe by default.
- Language features need one explicit trust step per project.

## Alternatives considered

- **Prompt on first open (VS Code style):** interrupts work, and users learn
  to click through.
- **No trust model:** unacceptable, given the attack history above.
