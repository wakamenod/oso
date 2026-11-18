# 0156. `terminal` switches only to shells

- Status: Accepted
- Date: 2026-11-18

## Context

[ADR 0120](0120-the-terminal.md) has `terminal` switch "to the last terminal still running", or
start one. A hosted session's buffer, `*claude: NAME*` ([ADR 0121](0121-agent-sessions-and-spaces.md)),
is a terminal too. So `M-x terminal` typed in a session's buffer switched to that session, the
one selected last, and seemed to do nothing. From another buffer it could switch to a session
rather than a shell. `C-u M-x terminal` in a session's buffer did the same: the `C-u` goes to
the session's program, so `terminal` ran with no prefix.

The author asked on 2026-11-18, through the space's orchestrator, that `M-x terminal` always
give a plain shell.

## Decision

- **`terminal` switches to the person's shell terminal still running that was selected last,
  or starts one.** A shell terminal is one `terminal` started. A hosted or adopted session's
  buffer never counts, nor does any terminal running something other than a shell, however
  recently selected. With a prefix argument it still always starts a new one. How `C-u` reaches
  a terminal's program is unchanged.
- **The tip crate marks a terminal as the person's shell when `terminal` starts it**
  (`Hosted::person_shell`, `Terminals::add_shell`). Every other start leaves it unmarked:
  sessions, stopped sessions brought back, sessions resumed in their buffers. An unmarked
  terminal is never switched to, so a start that forgets the mark gives a new shell rather
  than another program's keys.
- **The mark is not `Buffer::hosts_session`'s opposite.** A `claude` carried over a restart
  whose session did not come back hosts no session, but is no shell either.
- **A restart carries the mark** ([ADR 0133](0133-keeping-terminals-over-an-update.md)): each
  terminal's entry in the state says `shell`. A terminal comes back as a shell only if its entry
  carries no session and does not say it is no shell. A state from before, with no `shell`,
  counts one with no session as a shell, as every such terminal was then.
- **`C-x p m`, `project-terminal`,** once built, follows the same rule: it switches only to the
  person's shell terminals started in the project.

## Changes to accepted ADRs

- [ADR 0120](0120-the-terminal.md), "Many terminals are buffers in oso's windows (Q5)":
  "`terminal` switches to the last terminal or starts one". Now: to the last of the person's
  shell terminals, never a session's buffer.
- ADR 0120, "Project terminals": "`C-x p m`, `project-terminal`, switches to the last terminal
  started in the selected buffer's project". Now: the last of the person's shell terminals
  started there.
- ADR 0120, step 4a, "Lifecycle": "`terminal` switches to the last terminal still running (Q2)".
  Now: the last of the person's shell terminals still running.
- [ADR 0144](0144-agent-starts-with-no-limit-and-in-the-background.md), "`terminal` (ADR 0120,
  Q2) goes to the running terminal selected last", and its "Changes to accepted ADRs" entry on
  Q2. Now: the running shell terminal selected last. A terminal started behind is a session's,
  so it is never one; it still is not selected, so no window owns its size.

## Consequences

- `M-x terminal` from a session's buffer gives a shell: the last one, else a new one in the
  session's project root, else its directory. That starts a process where it used to switch, as
  `terminal` always may from another buffer.
- A new way to start a terminal must say whether it is the person's shell. Not saying keeps it
  out of `terminal`'s pick.
- The restart state's terminal entries gain a `shell` field. Older binaries ignore it.

## Alternatives considered

- **Derive the pick from `Buffer::hosts_session`:** no mark to keep. Rejected: a carried
  `claude` whose session did not come back, and a buffer whose session is only reserved, host
  no session, and `terminal` would switch the person's keys into them.
- **Skip only the selected buffer when it is a session's:** smallest change. Rejected: from
  another buffer `terminal` could still switch to a session.
- **Infer a shell from the carried state's missing session alone:** no new field. Rejected:
  a `claude` whose session did not come back carries none at the next restart, and would come
  back as a shell.
