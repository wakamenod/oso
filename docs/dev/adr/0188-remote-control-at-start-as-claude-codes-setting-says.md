# 0188. Remote Control at start, as Claude Code's setting says

- Status: Accepted (the author, 2026-11-28, through the orchestrator session); amends 0181
- Date: 2026-11-28

## Context

[ADR 0181](0181-remote-control-of-native-sessions.md) connects a native session to claude.ai only
by the person's key, `native-remote-control`, with a question each time. No configuration,
project, agent, tool or channel method turns one on there. The author's TUI sessions connect as
they start, as Claude Code's own setting `remoteControlAtStartup` says. On 2026-11-28 the author
decided that native sessions do the same.

**Read in CLI 2.1.331, not measured:** the CLI takes the setting from the managed settings first
(the managed file and its folder, a preferences plist, Windows' policy key, settings an
organization serves), then from `--settings`, then from the person's `settings.json`, else from
the legacy `~/.claude.json`. It counts only `true`. A `false` in the project's or the local
settings turns it off. A `true` there is ignored, with a warning: "repo-scoped settings cannot
enable Remote Control".

## Decision

**Every native start follows the person's `remoteControlAtStartup`, as the TUI's does.** This
covers the person's sessions, those an agent started, their forks and orchestrators, with no
question at the start. A repository whose git remote's address holds a password, or a user name
other than ssh's `git`, is never connected, as before.

- **Reading.** oso reads `settings.json` in Claude Code's configuration directory
  (`$CLAUDE_CONFIG_DIR`, else `~/.claude`), and `managed-settings.json` and
  `managed-settings.d/*.json` in the system's managed directory (`/Library/Application
  Support/ClaudeCode`, `/etc/claude-code`, `C:\Program Files\ClaudeCode`).
  - **Rule.** A managed `false` turns it off. Otherwise a managed `true` turns it on, and
    otherwise the person's `true` does. A `false` in the session directory's
    `.claude/settings.json` or `.claude/settings.local.json` turns it off.
  - **Not read:** the plist, the policy key, served settings and `~/.claude.json`.
- **When.** The configuration's thread looks at the person's and the managed files beside
  `config.toml`, once a second and on focus and save, by their stamps. So the first native start
  waits for it with the configuration's first look
  ([ADR 0173](0173-starts-wait-for-the-configurations-first-look.md)), and the app knows as it
  orders a start whether to mark it. The start thread reads the files again just before the
  line, with the project's. It connects only if that reading still says on; otherwise the start
  goes on blocked and says nothing.
- **Fails closed.** These read as off:
  - a file that is not a regular one;
  - one larger than 256 KiB;
  - one that is not UTF-8 JSON of an object;
  - a value other than `true` or `false`;
  - more than 64 managed documents.

  `*Messages*` says this once, naming the file and quoting none of it. A missing file is no
  setting.
- **What the start passes** is the line ADR 0181's part RC-2 measured: no `disableRemoteControl`,
  `remoteControlAtStartup: false` in `--settings`, and oso's own request once `initialize` says it
  is available, with the session's name and `keep_session_on_exit`. The CLI is not left to
  connect by itself: that is not measured with `-p`, and its request would have no random id. The
  address rules, and the look under trust, are RC-2's. A space's own directory is judged under
  its gate instead, and must be in no repository, nor below one.
- **What the person sees:** the mark, as before (`⇄ connecting`, then `⇄ claude.ai`). The first
  time a session connects this way while oso runs, a row says: "Remote Control connects this
  session to claude.ai, as remoteControlAtStartup in your Claude Code settings says: its
  conversation and tool output, earlier ones too, its name, git remotes, branch and directory
  are sent there and kept, and whoever holds the account can drive it. native-remote-control
  turns it off for this session." A start that stays off for its remotes says why, as before.
- **Secret text.** A session whose transcript or composer holds secret text
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)) is not connected at its start, as its start would send
  what was said.
- **The person's key** turns a marked session off, as before. That off holds for the session
  from then on: its respawns, its resumes and its resume across `oso-restart` stay blocked, until
  the person turns it on again with the key, which asks RC-2's question. oso turns it off in the
  same way when:
  - claude.ai answers another session's id;
  - a restart's reconnection is refused.
- **Resumes and respawns** follow the setting, as new processes of the TUI would. A respawn asks
  as a new start does, with no session id there: whether the CLI then comes back to the session
  it had is not measured.
- **Forks** follow it too, as a new session. Where the setting is on, the source is no longer
  turned off before the fork. One answered with another session's id is turned off, as before.
- **`oso-restart`** comes back to the session there as RC-2 has it. The restart's state also
  carries that the person turned a session off.

## Changes to accepted ADRs

- **ADR 0181, "What holds":**
  - "Off by default, everywhere … No configuration, project, agent, session tool or channel
    method turns it on: one command" now reads: off unless the person's Claude Code settings
    connect each session at start, or the person's command does.
  - "A session an agent started is never connected": it is, by the setting.
  - "Turning it on asks each time": at the person's key alone.
  - "Before a fork … it is turned off": unless the setting connects the fork.
- **ADR 0181, "Part RC-2, as built":**
  - In "Refused", an agent's session and its forks and an orchestrator: the setting connects
    them. The key still refuses them, as RC-2 has it.
  - "A fork turns its source off first": as above.
- **ADR 0181,** the parts: RC-3's orchestrators come by the setting, and its session tools' asks
  stay as RC-2 built them.
- **ADR 0165, "Remote Control is off, as amended":** off unless the setting or the person
  connects it.

## Consequences

- A native session is on claude.ai as the person's TUI sessions are, with the mark and the
  first row in place of a question.
- **What is accepted, beyond ADR 0181's:**
  - Whatever can write the person's `~/.claude/settings.json`, any session's own tools among
    them, turns it on for every later start, the TUI's as before. The configuration's thread
    reads the change within a second.
  - An organization's `remoteControlAtStartup` set only in a plist, a policy key or served
    settings is not seen. A `disableRemoteControl` there still refuses oso's request, in the CLI.
  - Agents' sessions and orchestrators are connected. Whoever holds the account, or another
    session of it, can send them a message, and their session tools then ask as ADR 0181's
    part RC-2 built them.
  - An orchestrator's other repositories, given with `--add-dir`, are not judged for their
    remotes. What it reads of them is sent, as anything a session reads is.
- `docs/dev/security.md` says what connects a session.

## Alternatives considered

- **The CLI's own connection at start:** not measured with `-p`; no random id, name or
  `keep_session_on_exit` of oso's.
- **The CLI's reading, from `initialize`'s `remote_control_auto_enable`:** it would need the
  process to start without the block first, and an unauthenticated stream line would decide.
- **A question at each start:** the author chose none.
