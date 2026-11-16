# 0144. Agent starts with no limit, and in the background

- Status: Accepted
- Date: 2026-11-16

## Context

[ADR 0135](0135-agents-start-sessions-without-asking.md) lets a session the person started start
sessions through `start_worktree_session` and `session_start` without asking, when
`[sessions] agents_start_without_asking = true`. It keeps a limit (Q5): at most
`agents_start_limit` (1 to 8, default 4) live unasked sessions per caller. Past the limit, the
call asks.

The author runs an orchestrator per issue space (ADR 0136), which starts many workers. The author
asked, through the coordinator, for two changes on 2026-11-16:

- **No confirmation** when an agent starts a worktree session. With the limit, a busy
  orchestrator's fifth start still opens a question.
- **No move to the new session's buffer.** Every session start runs `App::prepare_terminal`,
  which shows the new terminal in the selected window. That window also takes over the PTY's
  size. So an agent's start takes the person away from what they were doing, whether it asked or
  not.

## Decision

### No limit (amends ADR 0135, Q5)

- **`[sessions] agents_start_limit = "none"`** lifts the limit on a caller's live unasked sessions.
  The setting still takes a whole number from 1 to 8. Unset, it is 4, so the limit goes only
  where the person writes `"none"`.
- **`"none"`, not `0`:** `0` would read as "no unasked starts at all".
- **Any other value is refused,** `"None"` among them, with "takes a number from 1 to 8, or
  "none"", and the default applies.
- **The value is read at oso's start,** like the rest of the setting (ADR 0135, Q6). A later
  change says `agents_start_limit = "none" applies from the next start`.
- **Every other rule holds:**
  - the loop guard (Q9): at most 20 effectful calls since the person last typed in the caller;
  - the 60 s refusal after a fall-back question is refused;
  - the caller rule (Q3), trust (Q4) and every check before the question.
- **Code:** `sessions::tools::StartLimit` (`Most(n)` or `Unlimited`) replaces the `u8` in
  `starts_without_asking`. `config` parses it and spells it in its messages.

### Agent starts stay in the background

- **A session an agent's tool starts** (`start_worktree_session` or `session_start`, without
  asking or on the person's `y`) gets its terminal buffer, its row in the pane and its modeline
  marks as before. The selected window keeps its buffer. Nothing shows the new terminal until the
  person goes to it.
- **A `y` counts too:** the person answers the question from wherever they were working.
- **The person's own starts still show their session,** because the key asked for it:
  `claude` (`C-c a c`), `session-worktree` (`C-c a w`), `session-resume`, `session-fork`,
  `session-adopt`, an orchestrator's start, and the `y` to starting where another session runs.
- **Which starts are an agent's:** the session's origin (ADR 0135, Q10). `answered` and `unasked`
  come only from the tools, so `begin_session` shows the terminal only for `person`. A resumed
  session keeps its recorded origin but goes through `session-resume`, which always shows it.
- **Size (amends ADR 0120):** a terminal started behind starts at the size of the selected tiled
  window, also when a prompt is open. No window owns its size until a window is selected on it.
  From then it follows that window, as any terminal does.
- **`terminal`** (ADR 0120, Q2) goes to the running terminal selected last. A terminal started
  behind has never been selected, so it is not the one `terminal` takes
  (`Terminals::behind`).
- **Attention:** a session started behind is not seen (ADR 0121) until the person shows it. Its
  `needs input` and `done` ask for attention as for any session.

## The author's choice

The author accepted this ADR on 2026-11-16, through the coordinator, with both trade-offs in
Consequences: up to 20 starts each time the person types in a caller with `"none"`, and sessions
out of sight until the person goes to them.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0135](0135-agents-start-sessions-without-asking.md), Limits: "At most 4 live sessions
  started without asking per caller (Q5), `[sessions] agents_start_limit` (1 to 8, read with the
  setting)". It also takes `"none"`, for no limit but the loop guard.
- ADR 0135, the author's choices: "At most 4 live unasked sessions per caller,
  `agents_start_limit` 1 to 8, and past that the call asks (Q5)". Still the default; `"none"`
  lifts it.
- ADR 0135, Consequences: "A prompt injection in a session the person started can start up to the
  limit of sessions". With `"none"`, up to 20 each time the person types in that session.
- ADR 0135, Alternatives considered, "No limit". Kept as the reason for the default; the person
  may now choose it.
- [ADR 0120](0120-the-terminal.md), "In split windows, the PTY takes the size of the window that
  last selected the terminal", and the same rule in its 4b notes. A terminal started behind is
  owned by no window until one is selected on it. Until then it keeps the selected tiled window's
  size from its start.
- ADR 0120, Q2: `terminal` "switches to the terminal still running that was selected last". A
  terminal started behind does not count as selected.

## Consequences

- An orchestrator the person started can run all its workers with no question, and the person's
  window stays on their own work. The echo area's line, the pane's `unasked` mark and
  `agent-starts-ask` still say and stop what agents start.
- The sessions agents start are now out of sight until the person goes to them. The echo line and
  the pane row are the only cue, as they were for a start the person did not watch.
- With `"none"`, a prompt injection in a session the person started can start up to 20 sessions
  each time the person types in it. They cost tokens and work under the person's Claude Code
  permissions. Nothing else caps live sessions. Repeated starts that stop can push the person's
  own stopped, unpinned sessions out of oso's 256-entry record (ADR 0121), so `session-resume`
  can no longer find them. The default keeps the limit for anyone who does not write `"none"`.
- A background session's terminal is told it is not focused, so no key the person types reaches
  it until they go to it.

## Alternatives considered

- **`0` for no limit:** reads as "none allowed".
- **A higher cap** (say 64): still a question, only later, and a number with no meaning of its own.
  The loop guard already bounds a run.
- **Background only for unasked starts:** a `y` is answered from where the person works, and
  taking them away after it is the same cost.
- **Background for the person's own keys too:** `C-c a c` and `C-c a w` are asked for to work in
  the new session. Showing it is what the key is for.
- **A `[sessions]` setting for showing agent starts:** nobody asked for the old behaviour. The
  pane row and `C-x b` reach a session in one step.
