# 0176. Native orchestrators

- Status: Accepted, amended by 0181 and 0185
- Date: 2026-11-23

## Context

A space's orchestrator ([ADR 0136](0136-issue-spaces.md)) runs in the space's own directory,
`<[spaces] root>/<slug>`, which oso makes private and checks before each start. Native sessions
([ADR 0165](0165-native-claude-sessions-the-route.md)) run `claude -p`, which asks no trust
question of its own, so oso lets one run only at the root of a project it trusts (Q5). A space's
directory holds no `.git`, so it is no project: today every orchestrator is a TUI in a terminal,
and a native resume, fork, restart resume or switch of one is refused as "not open".

The author wants orchestrators native too. They also said that a space keeps its own folder, as
`handoff.md` and Claude Code's per-folder memory need one, and that native removes the trust
question Claude Code asks again in each new space's directory.

What Claude Code loads, measured on 2026-11-23 with CLI 2.1.330 (the plan's probes, in scratch
directories outside every space):

- `claude -p` in a space-like directory, with `--add-dir=REPO`, ran the directory's
  `.claude/settings.json` and `settings.local.json` hooks, started its `.mcp.json` server
  unasked (the TUI asks for a project's), and listed its commands, skills and agents. Of the
  `--add-dir` repository it loaded only the commands.
- With `--setting-sources=user`, none of the directory's hooks, `.mcp.json`, commands, skills or
  agents loaded, nor the repository's commands; `--settings=FILE` hooks still ran, so oso's steer
  hook keeps working, and oso's tools come through `--mcp-config`.
- `CLAUDE.md`: by default the directory's and its parent's loaded (the `--add-dir`
  repository's did not); under `--setting-sources=user` neither did. Auto memory, keyed by the
  directory, loaded either way, so the space's memory survives.
  `~/.claude/CLAUDE.md` under `user` was not measured.

## Decision

The author's answers of 2026-11-23, all as recommended (Q-O1 to Q-O10 (a)), through the
space's orchestrator. Built in parts O-1 to O-3; O-0, the `/add-dir` offer's escape, is a fix of
its own.

### The space's directory is its own native root (Q-O1 (a))

- A native session runs in a space's recorded directory under a **space gate**, never under a
  project's trust: no `TrustedRoot` is made for it, so nothing lands in the sessions' tool
  trusts (`hosting.trusts`), and no baseline is taken. The session tools keep judging their
  caller by its space and the repository each call names, as before.
- The gate admits a session only in an **open** space's recorded directory that
  `Stored::orchestrated_from` accepts: made by oso under `[spaces] root`, with the identity it
  had when made (ADR 0136, step 4a).
- **Who (Q-O3 (a)):** the space's current orchestrator, an earlier one, or a session of the
  person's there that oso made native there before (started by the person, with no parent,
  and native, or handed from native to the TUI): a native fork of one of them is one. A fork is
  judged by the session it copies. From O-3, also the space's new orchestrator, which oso
  starts there and which has no conversation yet: its gate is for its own space alone, while
  its start is the space's. A session an agent started in the directory is refused, and
  so is a terminal fork of it or an adoption there, which the record has as terminals.
- **Its line** adds `--setting-sources=user`: the person's own settings alone, none of the
  directory's or its parents' (hooks, `.mcp.json`, commands, skills, agents, `CLAUDE.md`).
  The TUI's line in a space's directory stays as ADR 0136 has it, with Claude Code's own trust
  question (Q-O10 (a)).
- **The checks stay:** the space's `check_before_start` runs on the start thread before every
  start there, as before; the gate is taken only once they passed, for the directory they
  looked at with the identity the gate was judged with.
- **Revocation:** until its start thread reports, the UI takes the gate back after each event
  once the space closed, its directory changed, `[spaces] root` moved, or the session is no
  longer the space's, and once its start is no longer under way with no report (its buffer
  killed). The start thread looks at the gate again just before the spawn.
- While starts wait for the configuration's first look ([ADR 0173](0173-starts-wait-for-the-configurations-first-look.md)),
  `[spaces] root` is not known: a gate waits, and the held order is judged again.

### What runs natively there

- **O-1:** a native resume, a fork, `oso-restart`'s resume (ADR 0165, part 4c), and the switch
  both ways (part 4d): a TUI orchestrator taken natively with `session-switch-mode`, and a native
  one handed back to the TUI.
- **O-3 (Q-O7 (a)):** new orchestrators follow `[sessions] mode`:
  `space-orchestrator-start` (`o`), `space-new`'s start and a successor's run natively unless
  `mode = "tui"`, under the space's gate as above, never a project's trust. One given no
  message selects its composer, as any native start does; a successor's brief is its first
  message. The gate is judged for the space `launch_orchestrator` names, in its directory, and
  in a space's directory a new start is native by that gate alone, never by a project's trust.
  Where the gate refuses, as for a directory not where oso made it, it starts in a terminal as
  before, the echo area saying why. A start that waits for the configuration's first look
  waits whole (ADR 0173), so its gate is judged as it goes. An existing TUI orchestrator keeps its mode (part 4e (b))
  until it is switched or replaced. Any other new start in a space's directory, and an
  adoption there, stays a terminal.

### Flows

- **An orchestrator given no message (Q-O8 (a)):** a native session that ends before it was
  given a message is forgotten (step 2). If it was its space's orchestrator, the one before it
  is the space's orchestrator again, as nothing is left of it to resume.
- **Project commands from the composer:** the repository `space-use-repo` chose is used from a
  native orchestrator's transcript and from its composer, as from its terminal.
- **`/add-dir` (O-1, O-2, Q-O4 (a)):** amended by
  [ADR 0185](0185-an-added-repository-is-given-to-the-orchestrator-without-asking.md): the
  resume goes with no question, as if `y` were answered, and the reply says `resume queued`.
  `claude -p` has no `/add-dir`. Each resume of a native
  orchestrator is given its space's trusted repositories with `--add-dir`, as every resume is.
  For a repository added or trusted while it runs, oso asks the person "Resume orchestrator
  NAME with REPO added, once its turn ends? (y or n)", in place of the paste, from the recorded
  path only while that is one plain line, as the paste's, and never over another question the
  person may be answering, as its `y` stops a process. At `y`, the offer still holding, it is
  stopped once idle and resumed with `--resume` and the new `--add-dir`, as a mode change's
  respawn is (ADR 0165, part 3b), before its queue goes on; its rows say "Resuming with REPO
  added: …", then, once the start thread reports, "Resumed with REPO added." for the
  repositories that start passed (else "Resumed."). While a turn, an interrupt or a switch
  runs, or a resume could not start (a stop asked, a quit or restart, a replacement), it waits,
  several `y`s gathering into one resume; once it could go, each repository is judged again,
  and one no longer a trusted member, or a session no longer the space's running orchestrator,
  is given up, the echo area saying so. A resume that comes otherwise meanwhile, as an
  interrupt's, gives the repositories too; a switch to the TUI does, by the TUI's resume.
  `space_add_repo` answers `add_dir: "resume offered"` when oso asked, and `"next resume"`
  when it could not ask now; its restricted note promises the resume, not a paste.
- **A review's question (O-2, Q-O5 (a)):** to a running native orchestrator it is sent at once
  (`send_confirmed`, ADR 0165 part 2f). To a stopped one oso offers the resume a terminal's
  gets, "Orchestrator NAME is stopped: resume it and ask it then?", and the question is held,
  at most ADR 0163's two minutes, until its process runs, then sent as to a running one; a
  resume refused drops it, saying so. "`session-resume` in its terminal resumes it" says
  "in its transcript" for a native one.
- **A report during a restart (O-2, Q-O6 (a)):** `oso-restart` ends and resumes native
  sessions, and Claude Code's `SendMessage` to a name with no live session fails, visibly to
  the sender. The worker brief's report-back line goes on: "If SendMessage finds no NAME, oso
  may be restarting it: send again in a minute."

## Changes to accepted ADRs

- **ADR 0165, Q5 ("Only in trusted projects"):** a native session also runs in an issue space's
  own directory, under its space's gate and with `--setting-sources=user`, as above. Elsewhere
  the rule stands.
- **ADR 0165, part 4e ("A space's orchestrator keeps it too, only because a space's directory
  has no `.git` yet"):** a space's directory is now a native root for resumes, forks, restart
  resumes and switches, and from O-3 new orchestrators follow `[sessions] mode` there.
- **ADR 0136, "The space's own directory" ("Claude Code asks its workspace trust question once
  a space") and "Trust stays per repository" ("The space's directory is the one place whose
  `.claude/settings.json` and `CLAUDE.md` load into the orchestrator"):** a native orchestrator
  is asked no trust question, and loads none of the directory's settings, hooks, MCP servers,
  commands or `CLAUDE.md`; its auto memory is the space's still. A TUI one is unchanged.
- **ADR 0161, "The `/add-dir` offer is the one oso already makes (Q6)":** a native orchestrator
  gets no paste, but the offer of a resume with the repository added; the reply says
  `resume offered`, or `next resume` when oso could not ask, as above.
- **ADR 0163, "Stopped, with its terminal (Q6)" and "A native session's question, as
  amended":** a review's question to a stopped native orchestrator is offered its resume and
  held until its process runs, as above; a stopped native session that is no orchestrator is
  still refused.
- **ADR 0121, "The brief":** the report-back line gains the retry sentence above.

## Consequences

- An orchestrator taken native runs with no trust question and under oso's permission card,
  in the space's directory alone. Its memory and transcripts stay the space's.
- **`CLAUDE.md`:** one in `~` or `~/spaces` reaches a TUI orchestrator, as Claude Code reads
  every directory above where it starts, but not a native one. Put what every orchestrator
  should read in the space's instructions (ADR 0136) or the person's own settings.
- Permission rules the person saved in the space's `.claude/settings.local.json` from a TUI
  orchestrator do not apply to a native one; the person's own settings' do.
- A space's directory with hooks or an `.mcp.json` someone wrote there runs none of them in a
  native orchestrator, which closes ADR 0136's "a session that writes hooks there gets them run
  by its successors" for native ones.
- The gate trusts the space's record and the sessions' record, which agents' sessions can
  write, as the session tools' roles already do (ADR 0136, step 4a): a forged orchestrator,
  earlier orchestrator or native session of the person's there passes it. The directory's
  checks and identity, and `--setting-sources=user`, still bound where and how it runs.
- From O-3, a new space's orchestrator is asked no trust question, unless `mode = "tui"`.

## Alternatives considered

- **The space's directory loaded as a trusted project (Q-O1 (b)):** `-p` would run its hooks
  and start its `.mcp.json` servers unasked, which the TUI at least asks about.
- **Trust derived from the members' (Q-O1 (c)):** the directory is none of theirs, and a
  member untrusted later would leave no clear rule for the orchestrator.
- **Only the current orchestrator natively (Q-O3 (b)):** an earlier orchestrator and a fork
  would fall back to the TUI and ask the trust question again.
- **A `TrustedRoot` for the space's directory:** it would land in the tool trusts and the
  baselines, which are git's.
