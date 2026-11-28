# 0181. Remote Control of native sessions

- Status: Accepted, amended by 0165 ("The mark", 2026-11-24) and by 0188 (connected at start, as
  Claude Code's `remoteControlAtStartup` says, 2026-11-28)
- Date: 2026-11-24

## Context

Claude Code's Remote Control connects a session to claude.ai: the person follows it, sends it
messages and stops it from a browser or a phone. The author used it with TUI sessions and wants
it in native ones ([ADR 0165](0165-native-claude-sessions-the-route.md)), which have it turned
off ("Remote Control is off, as amended") "until a feature of its own asks for it, in an ADR of
its own". This is that ADR.

**Measured** on 2026-11-23 and 24, CLI 2.1.330, Haiku, scratch directories:

- `claude -p` with stream-json takes a control request, `{"subtype":"remote_control",
  "enabled":true,"name"?,"reattach_session_id"?,"keep_session_on_exit"?}`. Its answer has the
  session's address (`session_url`, `https://claude.ai/code/session_…`) and its id there
  (`bridge_session_id`, `cse_…`). `system/bridge_state` says `ready`, then `connected` about
  half a second later. `enabled:false` is answered with success and no `bridge_state`. It works
  before the first turn, and asks for no consent, which the TUI asks once.
- It is refused for an unknown subtype (`unsupported_request`), under `disableRemoteControl`,
  and with `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`. `initialize` says
  `remote_control_available`.
- A new process of the conversation does not connect by itself. Asked again, with the id and
  `keep_session_on_exit`, it came back to the same session on claude.ai, after a clean end and
  after a kill.
- A message from the phone: `command_lifecycle` `queued` and `started` with an id, then a `user`
  line with that id, `isReplay` and `origin {"kind":"human"}`, and the turn's stream as any
  other. What oso sends has no `origin` and no `command_lifecycle`.
- The transcript has such a message with `promptSource` `sdk`, as oso's own, and, unlike them,
  `origin` and `clientPlatform`.
- A mode change is `system/status` with the mode, as one made here is.
- With the stdio permission route the CLI asked on standard output, and then cancelled its
  question, answered elsewhere. With oso's route, the `oso mcp` bridge's tool, oso was asked and
  nothing was on the stream; what the phone shows then was not measured.
- In `acceptEdits` and `auto` a `Write` ran unasked. Claude Code's `permissions.ask`, and a
  `PreToolUse` hook that answers `ask`, both made oso's card come in those modes too.

**Documented:** the conversation, with Claude's replies and tool activity, is stored on
Anthropic's servers. The documentation names the TUI, the desktop app and VS Code, not `-p`: the
request is not documented, and can change with any CLI.

**Read in the CLI, not measured:** what was said before is sent as the session connects, with
the git remote's address, the branch and the directory. The remote side can interrupt, change
the mode (to `bypassPermissions` only where the session was started so), the model and the
effort, read files and see the workspace's changes. Other sessions of the account can message
it.

## Decision

The author answered six questions on 2026-11-24, through the orchestrator session, and approved
the parts RC-0 to RC-3: what the phone does (1), its mode changes (2), how it is turned on (3),
orchestrators (4), RC-0 now (5), the third probe (6). They accepted this ADR the same day, with
what the security review asked for that was left open: what the session tools ask.

### The author's answers

1. **(a) Follow, send and stop.** Permission questions are answered in oso, on its card, through
   the `oso mcp` bridge's tool as now. The stdio route (RC-4) is not wanted.
2. **(b) A mode change from the phone is accepted and shown, and never carried on.**
3. **(b) Per session, by the person's key,** and connected again across `oso-restart`.
4. **(b) Orchestrators are allowed,** turned on one by one.
5. **Yes:** RC-0 first, on its own: off in every native start (built, #488).
6. **Later:** the third probe, with the author's phone, waits for the author.

### Where the security review asked for more

The security reviewer's advice, on the design, and what the author chose:

| Advice | Chosen |
| --- | --- |
| A mode change from elsewhere kept from removing oso's card, with the two blocks measured above; or ADR 0165's Q4 amended openly, as remote control of what runs | The second. It is amended below |
| No connecting again by itself; nothing kept across a restart | Across `oso-restart` only, as below, failing closed |
| No orchestrators, space directories, restricted projects or agents' sessions | Orchestrators allowed. Agents' sessions never, an orchestrator of theirs neither. A restricted project runs no native session |
| The session tools ask the person while the session is connected | As advised, below. Until it is built (RC-3), RC-2 refuses orchestrators |
| The person's keyboard alone, per session, no configuration default | As advised |
| A question each time, naming what is sent | As advised |
| A mark until the process ended, which no line of the stream clears | As advised |
| Off ends the process | As advised |

### What holds

- **Off by default, everywhere.** Every native start is blocked as today. No configuration,
  project, agent, session tool or channel method turns it on: one command,
  `native-remote-control`, `Person`, for one session, while it is idle. A session an agent
  started is never connected.
- **Turning it on** asks each time, naming what is sent and kept on Anthropic's servers: what
  was said and what the tools gave so far and from now on, the session's name, the git remote's
  address, the branch and the directory; and that whoever holds the account can drive the
  session. A remote whose address holds a user name or a password is refused. After the `y` the
  process ends and resumes without the block, under trust or the space's gate judged then, and
  oso asks, where `initialize` says it is available. A session that may not be stopped and
  resumed at once (a turn, no message yet, a resume held) is refused, as a mode's tier-1 switch
  is.
- **The mark** is in the modeline and in `*Spaces*` from the start of a process without the
  block, asked or not, until it ended, as such a process may connect of itself. The stream adds
  words to it (connected, dropped and why, escaped), and neither sets nor clears it.
- **Turning it off** writes `enabled:false`, ends the process and resumes it blocked. Claude.ai
  keeps what it has: oso says so.
- **Before a fork, a switch to the TUI, a stop, an archive, a kill of its buffer or oso's exit**
  it is turned off, as far as `enabled:false` can still be written. A fork and the TUI's process
  are not connected by oso. A process that ends to resume at once (an interrupt's signal, a
  mode, a repository added) comes back blocked, and oso says that it is off.
- **Across `oso-restart`** a session that was on connects again, asking nothing, as it is no
  turning on: it shows the mark and a notice at once. It stays off, and says so, unless the
  parent's restart state carried that the person had it on, the conversation's id, the directory
  and its identity, the trust or space gate and the session's id answered are the same, and the
  request goes. Never from the sessions' record alone; no other respawn connects unless the
  person asks.
- **A mode change that no key here made** is shown in the modeline and as a row. While it is in
  force it decides what asks: `acceptEdits` and `auto` run calls that oso's card would ask
  about. The next process is never given it: it starts in `plan` if that was the mode held
  before, else in `default`, and the person's earlier choice of a mode that asks less is over
  until they make it again here. A session that comes back with no such state (stopped, after
  oso's restart) starts as the person's Claude Code settings say, as ADR 0165's Q-B2 has it.
  Claude's own plan mode cannot be told from the phone's: while connected it is not carried
  either. oso still sends neither `bypassPermissions` nor `dontAsk`.
- **A message that came another way** has a row of its own, in the transcript, live and after a
  resume, that says so. It is never the person's message at oso: it takes no place in the
  composer's history, makes no choice of a model or an effort, and no text of it is put in the
  composer.
- **The address** is kept only as an answer of exactly claude.ai's shape gave it, is never
  opened by oso, and copied only on the person's key; `connect_url` is never read. Neither the
  address nor the id is logged, put in the sessions' record or given to an agent.
- **Session tools ask.** While a session is connected, a call of its that starts a session (a resume
  among them, as ADR 0135's rule decides both), removes a worktree or archives a session asks the
  person at oso, whatever the configuration lets go without asking; a call refused before its
  question is refused as now. It is for that time only: once the process ended, the rules are as
  before. An archive of a stopped session's row, which asks nothing today, asks then too.
- **Questions stay oso's.** No line from the phone or the stream answers or closes a card.

### What the stream may say

The stream is not authenticated: a process the session runs can write a line to it. So the lines
above are read strictly, and what they can come to is bounded by how oso started the process:

- **Started blocked, it is off,** whatever the stream says. Started without, it is marked,
  whatever the stream says.
- **No line goes back to off.** A refused or late answer and a `failed` line are "dropped",
  still marked. A forged line can make it say "connected" or "dropped" wrongly. Once turned off,
  no line changes where it is until the process ended; messages and modes are judged till then.
- **An answer** counts for the request that waits, by its id, once. That id is random, so that a
  line written by another process does not answer it, and gives no address and no id.
- **A message's origin only takes away.** It can make a message less than the person's, never
  more.
- **A mode** is shown as said, and never carried, as above.
- **What a model, a tool or a project prints** comes inside another line's JSON string, and is
  never a line.
- **The canary.** In a process started blocked, an `initialize` that says Remote Control is
  available, or a `bridge_state` that says it connects, means that the block may no longer hold,
  as with a later CLI. oso says so once a process, in a row and the log, marks and stops
  nothing, and from then on judges that process's messages and modes as a marked one's, which
  only takes away. A forged line can make it do that.

### The parts

- **RC-0** (built, ADR 0165 "Remote Control is off, as amended"): off in every native start.
- **RC-1:** the wire, the state, the canary and the mode's rule, with nothing that turns it on.
- **RC-2:** the command, its question, the respawn, the marks, off, and across `oso-restart`;
  the transcript's messages from elsewhere no longer read as the person's; orchestrators
  refused.
- **RC-3:** the rows of messages and of mode changes from elsewhere, live and past; the session
  tools' questions while a session is connected, and with them orchestrators.

## Part RC-1, as built

- **Wire.** `sessions::native::wire::remote`: `remote_control`'s request and answer,
  `bridge_state` (`ready`, `connected`, `reconnecting`, `failed`, `policy_disabled`, the third
  and the last read in the CLI), `command_lifecycle` and `origin`. A state is a whole word as
  the CLI writes it, a message's id a UUID, an address and an id of exactly their shape, at most
  64 letters and digits; a reason is cut to 256 bytes. `Event::Connection` and
  `Event::Lifecycle`; `User::origin`; `Answer::remote` and `Answer::remote_available`. A
  question the CLI cancels (`control_cancel_request`) and a request of its own stay unread: oso
  is asked through the `oso mcp` bridge.
- **State.** `sessions::native::remote::Remote`, one a process: off, connecting, on, dropped,
  ended; a request waits 30 s (an answer took up to 7 s, measured); at most 64 ids of messages
  that come. It judges how a message came (`User::came`), which the rows read: one from
  claude.ai is a note that says so until RC-3, whatever it holds, and takes no replay from what
  oso sent. Only `Remote::begin` gives what the line that turns it on is written with.
- **Mode.** `ModeState` is told whether the process may be connected, and keeps what the next
  process is given once the CLI said a mode no switch here gave; one said while a switch to it
  waits for its answer is the switch's only once that came. Off, with the canary silent, as it
  is until RC-2, ADR 0165's part 3b stands unchanged.
- **The canary** is a note row and a log line.
- **Settings.** What a session's `--settings` holds moved from `worktree::steer` to
  `sessions::settings`, where a status line (ADR 0126) can join it; `steer` keeps the hook and
  the tools it is asked about.
- **Tests.** A fixture put together from the recorded lines, scrubbed; each recorded and forged
  line at each state; text that holds the lines, in every shape that carries a model's, a tool's
  or a project's text; lines that are nearly one; addresses that are nearly claude.ai's; the
  modes, the person's `auto` put to `default` and back from elsewhere among them, and random
  ones; the fuzz target, where no line may turn it on; and the app's routing.
- **Not built:** a command, a question, a connection, a mark.
- **New public items:** none in a library crate.

## Part RC-2, as built

The author answered on 2026-11-24, through the orchestrator session: the question (short, with a
report), the mark (`⇄`), off during a turn, `git@`, `S-TAB`, and what is brought forward from RC-3.
The orchestrator session chose the stricter side in three the security review raised: off's bound,
the five calls that ask, and forks of an agent's session.

- **Commands.** `native-remote-control` (on asks, off does not) and
  `native-remote-control-address`, `Person`, `M-x` only. The question, "Connect NAME to claude.ai,
  its conversation and tool output too? (y or n)", opens from the command (ADR 0072), with
  `*Remote Control: NAME*` in the session's window, withheld from agents: what is sent, with the
  git remotes' addresses, the branch and the directory, that it stays, and who drives the session
  then. git is read on a thread meanwhile. Its `y` counts only with the report whole and all of it
  in a window, a quiet moment after it was and after the key before, never from a macro, a held key
  or held input; everything is judged again then. It closes as oso's window loses the front.
- **Refused,** each in words: an agent's session and its forks (while oso runs, and over its
  restart; after a plain restart of oso a fork reads as the person's, as ADR 0135 has it); an
  orchestrator, an earlier one, and any session in a space's directory; no process, a start, a turn,
  a request that waits, an interrupt, a mode's or TUI's switch, a repository being added, no
  message yet, a resume held, secret text in the transcript or the composer, and a project not
  trusted now.
- **A remote's address.** "A remote whose address holds a user name or a password is refused", but
  ssh's user `git` (`git@host:path`, `ssh://git@…`), a host's account for everyone, is let through.
  Every address the configuration names is read, what a rewrite makes of it too, under the trust
  judged by the start thread, just before the line is built; what is none of git's plain forms, a
  git that does not answer, remotes kept as git once did, a directory in another repository, more
  addresses than the report shows whole (12), and any other address or branch than it showed refuse. The start then goes on blocked, and says why, naming no address.
- **The respawn.** Leave to connect is a value that cannot be copied, made at the `y` or from the
  parent's restart state, held from the stop to that end alone and given to that resume; it alone
  decides the block in the line and the state in the app. Such a line has
  `"remoteControlAtStartup": false` too, so that only oso's request connects it (measured, below).
  `keep_session_on_exit` is always asked, as coming back was measured only with it. An answer with
  the id of another session oso has turns it off.
- **Off** is told at once. Idle, the process ends and resumes blocked. During a turn it ends once
  the turn is over, 60 s on at the latest, at a second press of the command, or at once if the
  request was not written, was refused or was not answered in 5 s; until then the mark says
  `⇄ going off` and a row says when it ends. A held resume keeps the session stopped, never
  connected. Before any other end oso makes, the request is written, and an idle process has a
  second to read it before `SIGTERM`; as oso exits, as far as it is still read. A fork turns its
  source off first. The words are "claude.ai keeps what it has, and may still list this session":
  whether it stays listed with `keep_session_on_exit` is not measured.
- **The mark** is `⇄ connecting`, `⇄ claude.ai`, `⇄ dropped` or `⇄ going off`, first in the
  modeline's note, with a mode no key here gave in it (`⇄ auto: claude.ai`), as a narrow modeline drops
  the note's later parts and cuts the end; before the note's first part, a native session's modeline drops the
  space's and the project's names, then the position and `Session`, and then shortens the name: at
  44 columns it is whole. Going off, the mode is left out, as it would read as what goes off; in `*Spaces*`, `⇄` before the name, never cut, which no name or reason begins
  with; also while its buffer is killed and its process ends. At most six rows a process say that
  it is on or dropped, the latter "and may connect again by itself".
  Amended 2026-11-24: a mode no key here gave is in the session's footer too, from the same value (ADR
  0165, "The footer, as built"); and while the mark leaves it out, going off, the note has it as any.
- **Across `oso-restart`** a connected session is not told off. The state carries its id there, its
  directory, and the device and inode of that and of its repository, which the start thread
  compares; Windows has none, so nothing connects again there. It also names the forks of
  agents' sessions (`native_agents`), or says that they are not known, and then none is connected. The mode is carried as it was once
  its process ended; where the state is made while it runs (`oso dev`), one that asks less is not. A
  restart that stays resumes it off.
- **The transcript.** A line with `promptSource` `sdk` and an `origin`, or any with a
  `clientPlatform`, came another way: a row "From claude.ai, not from oso: …", never a prompt or a
  brief, and no later message is the conversation's first. One typed in the TUI has an `origin`
  too, and stays the person's.
- **Mode.** From a mode no key here gave, `S-TAB` goes to `default` first, so that one set just
  before the key does not make it choose one that asks less. A first mode said by `system/status`
  is one of these; `initialize`'s and `system/init`'s are the process's own.
- **Brought forward from RC-3:** a row for such a mode, at most eight a process; and a connected
  session's `session_start`, `start_worktree_session`, `session_resume`, `space_add_repo` and the
  stop of `session_archive` ask, whatever the configuration lets go, by one rule
  (`sessions::tools::CallerStanding`). `worktree_remove`, and an archive of a stopped session's row, ask
  nothing until RC-3, for a connected session too.
- **One-shot runs** (a space's name, a dictation's fix) are started with the block. Whether
  `--safe-mode` heeds it is not measured.
- **Tests** are fakes': none connects.
- **Measured** by the third probe on 2026-11-24, CLI 2.1.331, Haiku, a scratch repository, the author
  with their phone, whose own settings have `remoteControlAtStartup` true:
  - Blocked, `initialize` said `remote_control_available` false and `remote_control_auto_enable`
    true, and no `bridge_state` came in a turn.
  - Unblocked, with `remoteControlAtStartup` false, it said available true and auto_enable false, and
    the first `bridge_state` (`ready`, then `connected`) came after oso's request was written.
  - Seven messages from the phone each had `command_lifecycle` `queued` and `started`, a replay with
    `origin` `human`, and `completed` or, once, `cancelled`; in the transcript, `promptSource` `sdk`,
    `origin` `human` and `clientPlatform` `ios`. Those looked at in oso were rows of their own.
  - Two calls of a message's turns, in `default`, asked in oso alone and ran after its answer.
  - The phone's Stop ended a turn after 2.1 s (`aborted_streaming`); the process stayed and the next
    message ran.
  - `@readme.md` in a message put the file in the conversation as an attachment: no call, and oso
    was not asked. It is accepted below, measured for an ordinary file; none that may hold secrets
    was tried.
  - Turned off while idle, `enabled:false` was answered with success within about a second, and the
    next process was blocked. oso's log and records held neither the address nor the id.
- **Not measured:** a message during a turn, the modes the phone offers and a mode set there, what it
  shows while a request waits in oso, whether the session is listed or archived after off, coming back
  over `oso-restart`, a fork of a connected session, and whether `--safe-mode` heeds the block.
- **New public items,** approved: `NativeAction::{RemoteControl, RemoteConfirmed, RemoteAddress}`.

## Changes to accepted ADRs

- **ADR 0165, "The route":** "Nothing else of the protocol is written": also `remote_control`,
  at the person's command alone. It has no tier 1 ("a failure or an unknown answer falls back",
  "every request has a documented fallback"): it fails, and oso says so. Its table's row for
  Remote Control, "not proposed"; its change to ADR 0031, "writes the five control requests",
  and its Consequences' "five control requests too": one more.
- **ADR 0165, Q4** ("by the person only", "No agent or channel method changes the mode"): the
  holder of the person's account changes it too, from claude.ai, while the person has the
  session connected. It is remote control of what runs unasked. No agent or channel method does.
- **ADR 0165, part 3b,** the mode a later process is given: as above while Remote Control is
  asked for.
- **ADR 0165, "The buffers",** "the rule that only the person writes to a session are ADR
  0130's", and **ADR 0130** (superseded; ADR 0165 keeps it by reference), "Only the person
  writes to a chat": a message may come from the person's account on claude.ai.
- **ADR 0165, part 4c** (restart resumes): a session that was on connects again, as above.
- **ADR 0165, "Remote Control is off, as amended":** off until the person turns it on for a
  session.
- **ADR 0165, part 5a,** "A message is the person's only where the transcript marks it so": and
  only if it did not come another way (RC-2).
- **ADR 0165, part 4d:** Remote Control is turned off before the switch to the TUI.
- **ADR 0121,** deferred, "anything cloud or remote": this one thing.
- **ADR 0176,** "What runs natively there": an orchestrator can be connected, from RC-3 on.
- **ADR 0165, "What is not"** blocked, "oso's one-shot runs … take none": they are started with it.
- **ADR 0165, part 3b,** the next mode of `S-TAB`: as RC-2 has it.
- **ADR 0135,** "What it covers" and "As built", and **ADR 0153,** "ADR 0135's rule decides whether it
  asks": a connected session's starts and resumes ask, whatever the setting.
- **ADR 0121,** Q14 and step 6, `worktree_remove` asks nothing once its checks hold: a connected
  session's asks.
- **ADR 0179,** "Asking": a connected session's archive asks, a stopped session's row's too.

## Consequences

- The person follows and drives a native session from a phone, and answers its questions at oso.
- **What is accepted, with the answers above:**
  - A mode set from the phone lets calls run that oso's card would ask about, until the process
    ends or the mode is set back.
  - What the session reads or prints, secret files it was let read among it, is kept on
    Anthropic's servers, and turning it off deletes nothing. oso's withholding of secrets does
    not reach there.
  - The remote side may read the session's files with no card of oso's (measured for `@` and an
    ordinary file).
  - Whoever holds the account can send a connected orchestrator a message, and it has the session
    tools, which then ask the person at oso before they start, remove or archive.
- oso has no network code still: `claude` connects, as it does for the model.
- The request is not documented. A CLI that changes it makes the command refuse; one that
  ignores the block is told by the canary.
- `docs/dev/security.md` says what is connected, and by whom.

## Alternatives considered

- **Permission questions on the phone, over the stdio route** (RC-4): a question oso answers or
  the phone does, on a stream any process of the session can write to, with changed input and
  rules in the answer. Not wanted.
- **Holding the modes with an enforced `ask`:** works (measured), and makes `auto` and
  `acceptEdits` useless in a connected session, at oso too.
- **A configuration default:** an agent can write `config.toml`.
- **The TUI for it** (`C-c C-t` and back): what works today, and leaves the session's rows.
- **No connecting again across a restart:** each `oso-restart` would cut the phone off.
