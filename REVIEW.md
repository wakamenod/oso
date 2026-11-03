# REVIEW.md

A checklist for reviewing changes to oso, used by human reviewers and
reviewer agents. Flag anything that matches below. Items marked **[gate]**
need the author's explicit approval.

## Security

- [ ] **[gate]** New or upgraded dependency: is it justified, who maintains
  it, how much `unsafe` does it use, how many transitive dependencies does it
  add, and does it pass `cargo-deny`?
- [ ] **[gate]** New `unsafe`: does it have a `// SAFETY:` comment, and is it
  outside `oso-core`?
- [ ] **[gate]** Changes to process spawning, file writing, the trust model,
  the control socket, `deny.toml` or the files that define the checks (see
  `AGENTS.md`).
- [ ] Nothing executes on file or project open. External tools run only in
  trusted projects.
- [ ] No network code, and no process spawning outside
  `oso-platform::process`.
- [ ] No buffer contents written to disk outside safe save and emergency save.
  Secret-pattern files are excluded.
- [ ] Untrusted input (file content, terminal output, git config) cannot
  trigger commands, clipboard reads or echo-back sequences.
- [ ] Dev-only code stays behind dev features.
- [ ] No secrets, personal paths, employer or client names, or unfixed
  vulnerabilities in code, docs or commit messages.

## Performance

- [ ] No blocking I/O, parsing or process waits on the UI thread.
- [ ] No per-keystroke work that is O(buffer size). Work is limited to visible
  rows or done incrementally.
- [ ] No unnecessary allocation in hot paths: rendering, layout, input
  handling.
- [ ] Large files and very long lines still behave.
- [ ] No regression in the performance budgets or the 2 s tip-crate build
  budget.

## Correctness and design

- [ ] Commands are registered with a docstring and argument spec, and work
  with multiple cursors.
- [ ] Positions are converted through the display model, never assuming one
  buffer line per screen row.
- [ ] Full-width characters, emoji and combining characters are handled on
  the cell grid.
- [ ] Panics cannot corrupt state beyond the failing command.
- [ ] Platform-specific code is behind `oso-platform` traits.
- [ ] Consistent with accepted ADRs, or a new ADR is proposed.
- [ ] Dependencies point the allowed way, and each module stays within the
  responsibility in its `//!` header (ADR 0038).
- [ ] No new global state, and no file or function grows past the size
  limits without an ADR.
- [ ] Public API changes are intended and visible in the API snapshot.

## Tests and docs

- [ ] Tests follow the pyramid (ADR 0037): unit or property tests for new
  logic, scenario tests for every command, text-grid snapshots for anything
  drawn. End-to-end tests only for seams and critical flows.
- [ ] A bug fix comes with a test at the lowest layer that reproduces it.
- [ ] Snapshot changes are explained.
- [ ] Docs, ADRs and `open-questions.md` updated where affected.
- [ ] English only.
