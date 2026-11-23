# 0173. Starts wait for the configuration's first look

- Status: Accepted
- Date: 2026-11-23

## Context

The configuration file is read on a thread of its own, and the UI thread applies what the
thread found once the window is open ([ADR 0108](0108-the-user-configuration-file.md)). Until
that first look is applied, the settings in force are the built-in ones. Every hosted session
start reads `[sessions]` and `[terminal]` as they are in force when it starts. So a session
started in the first moments after oso starts used no configured `[sessions] claude`. It
searched `PATH` for a bare `claude`, and neither `[sessions] env` nor `[terminal]`
`shell`/`login` applied. The macOS performance bench found it. Its first session ended with
exit 127 and "oso: no claude on an absolute PATH entry outside this directory and its project;
set [sessions] claude".

[ADR 0138](0138-language-servers.md) already has the rule for language servers: no open settles
before the configuration file was looked at once (`app/starts.rs`). Nothing held the other
starts.

What reads the configuration as it starts:

- **A new session** (`begin_session`): `[sessions] mode` chooses native or the TUI before its
  buffer is made ([ADR 0165](0165-native-claude-sessions-the-route.md)). Then `claude`,
  `steer_worktrees`, `env` and `[terminal]` go into its start order. The person's `claude`,
  start-shared `y`, an agent's `session_start` and `start_worktree_session`, an orchestrator's
  start and a worktree brief's all come here.
- **A fork**: `[sessions] mode` chooses native or the TUI, as for a new session, then the same
  settings go into its start order.
- **A resume or adoption**, native or in a terminal: `claude`, `steer_worktrees`, `env` and
  `[terminal]`, in its start order only. Whether it is native comes from the session's record.
- **A shell terminal** (`terminal`): `[terminal]` ([ADR 0120](0120-the-terminal.md)).
- **A new space**: `[spaces] root` for its directory ([ADR 0136](0136-issue-spaces.md)), and
  `[spaces] naming` with `[sessions]` and `[terminal]` for the `claude -p` that names it
  ([ADR 0159](0159-space-new-starts-the-orchestrator-and-display-names.md)).
- **An agent's session tool call**: `agent_starts` is set by the first look only
  ([ADR 0135](0135-agents-start-sessions-without-asking.md)). Before it, an agent's start would be
  asked even when `config.toml` lets it start without asking.

## Decision

The author chose the fix and answered each question on 2026-11-23, through the orchestrator
session.

- **Until the configuration's first look is done, every start above waits.** They run in the
  order they came once it is done, after the settings are applied and after opens settle. A
  missing file, a refused one and one with errors are looks too. With no file at all there is
  nothing to wait for.
- **Nothing is said to the person.** A start just happens a moment later.
- **Where each waits**:
  - A new session waits after its directory is found, so `claude` with no directory starts
    where the person was when they pressed the key. Nothing is reserved or made for it before
    then. What its caller does with its key (`Then`: an agent tool's reply, an orchestrator's
    space) follows it. Its trust is judged when it goes. One the person started is shown
    where they are when it goes, unless they moved to another window or buffer meanwhile:
    then it starts behind, so that what they type stays where they are (as ADR 0159's
    `behind`).
  - A fork waits whole, before its key is reserved and its buffer made.
  - A resume or adoption makes its buffer at once and marks the session starting, as
    before. Only its start order waits, and when it goes it takes `claude`, `steer_worktrees`,
    `env` and `[terminal]` as they are in force then. A native one's control layer counts its
    `initialize` wait from then. It is judged again as a resume typed then would be: a native
    one's trust, and an orchestrator's space with its repositories as they are trusted then,
    so that a project or repository distrusted meanwhile is not given. One whose session was
    forgotten meanwhile starts nothing. One whose buffer was killed meanwhile, or judged no
    longer allowed, is refused, as a start thread's refusal is. A second adoption of a
    conversation whose adoption waits or is under way says "That conversation is being
    adopted".
  - A shell terminal makes and shows its buffer at once. Its shell waits and takes `[terminal]`
    as it is in force then. A buffer killed meanwhile starts nothing.
  - A new space waits once its repository is looked at, and a space directory waits before it
    is made. The space is found again by its id.
  - An orchestrator's start waits whole, keyed by its space, so that where it is shown is
    judged when it goes. Meanwhile its space counts as starting one, so a second
    `space-orchestrator-start` says "Its orchestrator is starting", and a space closed
    meanwhile starts none.
  - **An agent's session tool call that changes something waits whole (Q1 (a))**, so it is
    planned with `agent_starts` and the rest of the configuration in force. A read is answered
    at once. A call is accepted and its reply just waits, never refused and never told oso is
    busy. A call whose connection the channel says has gone meanwhile is dropped, with no
    reply. One whose client left unnoticed, as while a `Usual` call waits, runs and its reply
    goes nowhere, within the channel's reply timeout.
- **Leaving:** once an exit or an `oso-restart` is under way, what waits is dropped, not run:
  it would be lost with the instance (ADR 0133). An agent's dropped call gets no reply.
- **Order and repeats:** strictly the order they came, across kinds, with nothing merged.
  Each runs as if typed when it goes, so its own checks see the ones before it. A second
  `claude` in the same directory asks "another session runs in this directory", a second
  resume of the same session says it runs, and a second `terminal` shows the first. That
  question is asked one at a time: a start that would ask while it is open, or while others
  wait, waits its turn. A yes starts its own and the next is judged again, so it asks, with
  the same text, only if another session still runs there; one that starts unasked starts
  behind. An `n` or `C-g` drops its own and the others that wait for the same directory, as
  the person said no to starting there. Once an exit is under way, what waits is dropped. A
  start the start thread's look at the registry sends back to ask is judged in full again
  when its turn comes: it makes its buffer and start thread once more before it asks.
- **The bound (Q2 (a), Q5 (a)):** five seconds after the first start that waited, if the look
  has not come, as on a hung mount or a window slow to open, everything that waits runs with
  the settings in force, the built-in ones. `*Messages*` says once: "The configuration was not
  read in 5 s: what waited for it started with the built-in settings". From then on nothing
  waits, whether the look comes later or not. A later look applies to later starts, as any
  look does.
- **The end-to-end check (Q3 (a)):** a dev build's `OSO_DEV_CONFIG_DELAY_MS` holds the first
  look back by that many milliseconds, at most a minute. It is a sleep on the `oso-config`
  thread before it first reads the file, never on the UI thread. It is compiled only with the
  `dev` feature, and `just release-safety` checks that its name is not in a release build.
- **Left out (Q4 (a)):**
  - The scrollback of terminals a restart hands over (`adopt_terminals`). They are running
    already, and keep the built-in scrollback if the handover comes before the look.
  - `[git] pass_env`. A git run with hooks before the first look is given fewer variables, so it
    fails safe.
  - Language servers keep [ADR 0138](0138-language-servers.md)'s own rule, which has no bound.
  - A read by an agent's session is answered at once, so its role is judged with the
    built-in `[spaces] root`. An orchestrator carried over a restart under another root may be
    taken for a former one until the look, and is then shown less, not more.
  - What waits is not handed over by an `oso-restart` in those first seconds, as a start
    under way is not. Idle quit counts what waits as a start under way.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0121](0121-agent-sessions-and-spaces.md), "`[sessions] claude` in `config.toml` may name
  the executable by absolute path; otherwise the login shell finds `claude` on its `PATH`": and
  a session's start, resume, fork or adoption waits for the configuration's first look, at most
  five seconds, so that `[sessions]` and `[terminal]` are the file's. The agents' session tools
  that change something wait whole, and reads are answered at once.
- [ADR 0165](0165-native-claude-sessions-the-route.md), "`[sessions] mode = "native"` opts in": a
  new session or a fork started before the configuration's first look chooses native or the
  TUI once it has been looked at, as above.
- [ADR 0120](0120-the-terminal.md), "The shell (Q18) is `config.toml`'s `[terminal] shell`": a
  terminal opened before the configuration's first look shows its buffer at once, and its shell
  starts once the file has been looked at, at most five seconds later.
- [ADR 0136](0136-issue-spaces.md), "the root is `[spaces] root` in the person's `config.toml`",
  and [ADR 0159](0159-space-new-starts-the-orchestrator-and-display-names.md), "`[spaces] naming
  = "claude" | "local"`": a space made, or its directory made, before the configuration's first
  look waits for it, as above.

## Consequences

- A session, terminal or space started right after oso starts runs as the person configured
  it. The bench's first session finds its `claude`.
- An agent connected over a restart handover waits a moment for its first changing call, and
  an unasked start is not asked about only because the look had not come.
- A start can wait up to five seconds when the configuration's thread is stuck. After that,
  starts use the built-in settings, which `*Messages*` says.
- What waits holds boxed closures over the app on the UI thread. They are few, as only the
  person's commands and agents' calls in those first moments make them.

## Alternatives considered

- **Hold every start at its command (whole):** simpler, but `claude` with no directory would
  find its directory after the person may have moved, and an orchestrator's or worktree's
  start, which comes from no command, would not wait.
- **Hold only the start order, for new sessions too:** `[sessions] mode` is read before a new
  session's buffer is made, so a native session would start in the TUI.
- **Refuse a start while the configuration is unread (Q2 (b)):** the person would have to
  start it again for a delay that is usually a few milliseconds.
- **Wait for the look with no bound:** a hung configuration mount would hold every session
  start for the whole run.
- **Start with the defaults, silently, once the bound passes (Q2 (c)):** that is the bug, with
  nothing to explain it.
