# 0146. A state directory of its own for checks

- Status: Accepted
- Date: 2026-11-17

## Context

oso keeps what it remembers in its state directory: the sessions it hosted and the spaces
([ADR 0121](0121-agent-sessions-and-spaces.md), [ADR 0136](0136-issue-spaces.md)), reviews'
copies ([ADR 0122](0122-agent-review.md)), emergency copies
([ADR 0051](0051-m2-plan-decisions.md)) and the theme chosen last. It is
`oso_platform::privacy::state_home()`'s `oso`: `$XDG_STATE_HOME` or `~/.local/state` on Linux,
`~/Library/Application Support` on macOS and `%LOCALAPPDATA%` on Windows.

On macOS nothing in the environment moves it. A dev instance started for `verify-in-app`, and
every end-to-end flow that does not set a `HOME` of its own, reads and writes the person's real
records. During ADR 0145's step 2 checks, two forked sessions and a space of a check's fake
transcripts landed in the author's own `sessions` and `spaces`; they were archived by hand.

## Decision

The author chose (2026-11-17, through the coordinator):

- **`OSO_STATE_DIR` names oso's own state directory,** the one that stands for
  `state_home()`'s `oso`, when it is set to an absolute path. A relative one, or none, leaves the
  default. It is honoured on every system and in every build: what can set oso's environment can
  move `HOME` anyway.
- **`oso_platform::privacy::state_dir()`** gives it: `$OSO_STATE_DIR`, else `state_home()`'s
  `oso`. It is a new public function, approved by the author, with the variable's name as
  `STATE_DIR_VARIABLE`. Emergency copies' default directory and the app's state directory
  (sessions, spaces, reviews, the theme) come from it; `state_home()` stays as it is.
- **An instance passes it on:** `oso-restart`'s new instance inherits it with every name that may
  pass, and `oso dev`'s supervisor lists it among what an instance needs.
- **The end-to-end flows** set it for every instance to the flow's own `state` directory, beside
  its emergency copies, unless the flow sets it itself. Instances a flow starts in one directory
  share it, as a restart's two do.
- **`verify-in-app`** starts its dev instance with `OSO_STATE_DIR` in its scratch directory.

## Consequences

- No check writes the person's records any more, on any system.
- A person may move oso's state elsewhere with the variable, for a second instance with state of
  its own.
- Flows that set a `HOME` of their own for their state now keep it in their `state` directory
  instead, which is as private to them.

## Alternatives considered

- **Honour `XDG_STATE_HOME` on macOS.** It would move other programs' state with it in a dev
  shell, and Windows would still need a variable.
- **Only in dev builds.** The release flows and a person's second instance would gain nothing,
  and the environment can move `HOME` in any build.
- **Set `HOME` in every check.** It moves the person's shell, Claude Code's settings and git's
  configuration too, which a check often needs as they are.
