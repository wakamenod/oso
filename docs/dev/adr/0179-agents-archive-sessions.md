# 0179. Agents archive sessions

- Status: Proposed
- Date: 2026-11-23

## Context

A space's orchestrator lists its space's sessions in `*Spaces*` and through `session_list`.
The list fills with rows whose work is done: sessions whose worktree git or `gh` removed after a
merge, plan sessions whose worktree was removed with git, sessions done in a worktree that is
still there. Only the person archives them, with `A` in `*Spaces*` or `C-c a A` in a session's
buffer ([ADR 0121](0121-agent-sessions-and-spaces.md)). An agent's `worktree_remove` archives the
sessions of a worktree only when it removes that worktree itself ("Removal archives its
sessions, as amended").

Most of those rows still run. On 2026-11-23 thirteen of the orchestrator's rows were live idle
`claude` processes whose worktree was gone. A tool that only archived stopped sessions would
refuse almost all of them, so the tool has to stop an idle session first. ADR 0121 planned
`session_archive` ("Change, asks; stops it if running"), and its Consequences say agents stop and
archive sessions only through a question. `session_stop`, an agent's stop on its own, is on hold
by the author's choice.

The author asked for `session_archive` and chose its rules on 2026-11-23, through the
orchestrator session: (b) for the running session (stop first, only when idle by both halves of
check 4), the asking as proposed, the session_rename scope, images kept, the worktree never
touched.

## Decision

- **`session_archive { session }`,** the session by its id or name from `session_list`. It is a
  `Change` tool of `sessions::tools::TOOLS`, `Asks::UnlessAllowed`, not destructive, for hosted
  sessions only (`outside: false`), and it starts no session (`starts: false`). Its channel
  method has the same name, and its reply is held as long as the person takes (`Hold::Person`).
- **Who archives which:** a space's current orchestrator archives its own space's sessions; any
  hosted session archives the sessions it started, by the record's parent. That is
  `session_rename`'s scope ([ADR 0155](0155-renaming-sessions.md)).
- **Never:** the caller itself; an orchestrator, current or earlier, or a session in a space's own
  directory, failing closed as a rename does (only the person archives one); a pinned session;
  an archived one; another space's; another program's session, which is not hosted; one still
  being started. Each is refused, and the refusal says why.
- **A stopped or failed session:** only its row is archived, with no question, as the person's
  `A` does. `A` under `Archived` brings it back.
- **A session that runs is stopped first, only when it is idle by check 4's rule**
  ([ADR 0121](0121-agent-sessions-and-spaces.md), step 4): its screen says idle, done or ready
  for review, *and* Claude Code's registry entry for its process is that process's
  (`Live::Yes`) and says `idle`. Refused: a session working, needing input or not judged yet;
  one oso does not know the process of; one oso hung up already, until it has stopped; an entry
  that is missing, cannot be matched to the process, or says anything but `idle`. An entry that
  says `shell`, which Claude Code writes while a shell the session runs in the background still
  runs, is refused with "a background shell of that session still runs". The stop is the
  person's `k` (`App::stop_quietly`, the body of `App::hang_up_session`, in the watch's files,
  not the tools'), and the session's end is a stop, not a failure. A native session
  ([ADR 0165](0165-native-claude-sessions-the-route.md)) is judged the same way: its screen state
  comes from its stream, and its `claude -p` writes a registry entry (`entrypoint: sdk-cli`)
  whose `status` reads `idle` between turns (seen on 2026-11-23); check 4 depends on that too.
- **The registry is read off the UI thread,** one file, the process's own `<pid>.json`
  (`registry::read_pid`), on a short thread `oso-session-idle` (`sessions::registry_look`), and
  judged by `sessions::tools::registry_idle` over `registry::activity`, the one classifier check
  4 uses too. One look runs at a time per caller; one that takes over 5 s is answered that
  nothing was stopped. Every rule is looked at again on the UI thread once it is read: the same
  session by id, the same caller on the same connection, and the rules still stop the same
  process first.
- **The registry half is advisory against an agent of the same user:** a session's own `Bash`
  could write the entry, as it could signal the process itself. The screen half is oso's own.
- **Asking:** archiving a stopped session asks nothing (`sessions::tools::archive_asks`). A stop
  follows the person's configuration as starts do
  ([ADR 0135](0135-agents-start-sessions-without-asking.md)): it goes without asking when
  `[sessions] agents_start_without_asking` is on and no `agent-starts-ask` holds it back, for a
  caller the person started (`sessions::tools::stops_without_asking`), and the session is not
  shown in the selected window, where the person may be about to type. Else the person is asked
  "let session "X" stop idle session "Y" in "DIR" and archive it? Its worktree is kept.", the
  directory telling apart two sessions of one name. The project's trust, which ADR 0135 asks of
  a start, is not asked: a stop runs nothing in a project, and the session's directory may be
  gone. No start limit counts it. After the person's `y`, the registry is read again and the
  rules looked at again before the stop, as the question may have waited; a connection that
  goes meanwhile drops the stop, and the echo area says so. `n` refuses the same call for 60 s.
- **Rate:** every call the guard admits counts toward the 30 a minute of
  [ADR 0178](0178-a-rate-limit-on-the-session-tools.md), a stopped session's too. A call refused
  before the guard (no such session, out of scope, pinned, archived, not idle on screen) costs
  nothing, as for a rename; it learns only what `session_list` shows the caller.
- **Its worktree and branch are never touched,** whether or not its directory is still there.
  `worktree_remove` removes a finished worktree, and its tool description says so.
- **Its images stay** until the week's sweep ([ADR 0142](0142-attaching-the-clipboard-s-image-to-a-session.md)),
  as for the stranded records a start archives: the person's `A` brings the row back, but not a
  deleted image. The person's `A` still drops them.
- **What is said:** the echo area and `*Messages*`: `Agent: session "X" archived session "Y"; A
  under Archived in *Spaces* brings it back`, with ", stopping it first" or ", stopping it first
  without asking". `*Agent*` logs `session_archive: from X`, then `archived` or `kept running`.
  The reply: `id`, `name`, `archived: true`, `stopped`, `asked`, and a note for a stop without
  asking.
- **No agent unarchives.** Only the person brings a row back.
- **Check 4 says when a background shell keeps a session busy.** A worktree's removal and
  listing name a session settled on screen whose registry entry says `shell` apart from the busy
  ones (`RepoFacts::hosted_shell`, `Held::shell`, `Check::Shell`): "session X in it still runs a
  background shell (Claude Code's registry says shell)". An orchestrator's listing says "a
  session in it still runs a background shell", naming none, as it names no busy session.
- **New items,** all in the binary crate: the module `sessions::registry_look` (`spawn_look`,
  `Found`) and the thread `oso-session-idle`; `registry::{read_pid, activity, Activity}`, the
  per-pid read now shared with `move_to_space`'s looks; in `sessions::tools`, `Archivable`,
  `Archive`, `Runs`, `NotIdle`, `archivable`, `archive_asks`, `stops_without_asking`,
  `registry_idle`, `archive_question` and `archived_said`; `Check::Shell`, `Held::shell`,
  `Looked::shell`, `RepoFacts::hosted_shell`, and `Snapshot::held` taking the shell pids too;
  in the app, `UserEvent::ArchiveLooked`, `Plan::Archive`, the tools' `archive_looks`,
  `App::stop_quietly`, `App::admit_asking` and `App::reach_over`, the scope a rename and an
  archive share. No oso-core public item.

## Changes to accepted ADRs

- [ADR 0121](0121-agent-sessions-and-spaces.md), the session tools' table, "`session_archive` |
  Change, asks | stops it if running, hides it from the list; its worktree is kept": it stops a
  running session only when check 4's rule says it is idle; a stopped session's archive asks
  nothing, and a stop asks as ADR 0135 lets a start go without asking.
- ADR 0121, Consequences, "Agents can start, stop and archive sessions only through a question
  the person answers": an agent archives a stopped session with no question, and stops an idle
  one to archive it without a question as the person's configuration allows.
- ADR 0121, step 4, check 4 "no busy session": a session busy only as its registry entry says
  `shell` is refused with its own reason.
- [ADR 0135](0135-agents-start-sessions-without-asking.md), "What it covers", "`session_stop`,
  `session_archive`, `merge_branch` and the read tools that ask are unchanged": `session_archive`'s
  stop goes without asking under the same setting, the caller the person's, with no trust asked
  and no limit counted.
- [ADR 0142](0142-attaching-the-clipboard-s-image-to-a-session.md), point 6, "A session's
  directory goes when the session is archived": an agent's archive keeps it until the week's
  sweep.
- [ADR 0178](0178-a-rate-limit-on-the-session-tools.md), Decision, "Which calls count": the list
  gains `session_archive`.

## Consequences

- An orchestrator clears its space's finished rows without the person, including the idle
  sessions in worktrees already gone. The person sees each in the echo area and brings any back
  with `A`, then `session-resume`.
- A stop needs both halves of check 4 at the moment it runs. A session that settles on screen
  while a background task still runs is not stopped, and the agent learns which half refused.
  The native-4a-trust case of 2026-11-23, done on screen but `shell` in the registry for a
  `gh run list` loop it ran in the background, is refused with that reason by both this tool and
  `worktree_remove`.
- A stop without asking has no limit but the rate: an orchestrator can stop every idle session
  of its space in a minute. Each is idle by both halves, and each is resumed with its
  conversation.
- An idle session the person is about to type in, in a window not selected, can be stopped
  without asking, and a text half typed in its input box is lost. One in the selected window
  is always asked about, and the person's `agent-starts-ask` makes every such stop ask.
- The session tools' guard test (ADR 0121, step 6) keeps `hang_up` and the process module out of
  the tools' files, by name. The stop is reached through `App::stop_quietly`, outside them, as a
  removal's through `stop_idle_sessions_in`; the guard does not tell such a call apart. A guard
  that named the stops a tool may reach would be a change to a check file; the author left the
  guard as it is for now (2026-11-23).

## Follow-up

- **A stricter guard:** `crates/oso/tests/architecture/agent_tools.rs` could name the stops a
  tool file may reach (`stop_quietly`, `stop_idle_sessions_in`) and allow each only from the
  tool that needs it, so that a new tool's stop fails the check until a decision lets it.
- Ready for review counts as settled, as for a removal: an agent can stop and archive a session
  whose work waits for review. The review itself is the worktree's and stays.
- Between the registry's read and the stop, a few milliseconds without asking and a second read
  after a `y`, a new turn could begin; the screen's state is looked at again just before, which
  catches a turn the screen shows.
- `session_stop` stays on hold: no agent stops a session without archiving it.

## Alternatives considered

- **Refuse every running session (the person's `k` first).** Simple, but on 2026-11-23 it would
  have archived almost none of the rows the author wanted gone, all live idle processes.
- **Never ask, as `worktree_remove` (ADR 0121, Q14).** It stops idle sessions with no question
  once every check holds. Rejected for the author's earlier answer for `session_stop`: a stop
  follows the configuration as starts do.
- **Ask for every archive.** The row alone is the person's `A` to undo, and asking for each would
  keep the person in the loop the author wanted to leave.
- **Drop the images, as the person's `A` does.** The archive is undone with `A`, the deletion is
  not, so an agent's archive keeps them, as the start's stranded archive does.
- **Read the registry from the sessions worker's poll.** No poll reads hosted entries' status,
  and a reading seconds old could pass a session whose turn began since; one fresh file is cheap.
- **A `stop` argument, or `session_stop` as its own tool.** The author put `session_stop` on hold;
  a stop here exists only to archive.
