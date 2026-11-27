# 0186. A space's orchestrator answers its workers' tool calls

- Status: Accepted (the author, 2026-11-27); steps 1 (reads) and 2 (answers) built; the deny
  list adds to the defaults and a card takes no answer at all (the author, 2026-11-27, after step 1)
- Date: 2026-11-27

## Context

A native session asks the person about each call its permission mode does not let run: through
its bridge's `permission` tool, the sessions' socket, and a block in its conversation
([ADR 0165](0165-native-claude-sessions-the-route.md), step 2;
[ADR 0182](0182-a-native-sessions-requests-in-its-conversation.md)). In an issue space
([ADR 0136](0136-issue-spaces.md)) the orchestrator starts workers, and the person answers each
worker's every `Read`, `Edit` and command. The author wants the space's orchestrator to answer
the calls the person trusts it with, and to keep the rest.

## Decision

A space's orchestrator may allow or deny the pending tool-call request of a native worker
session in its space, once, with `session_answer`, when the person lists that tool in
`orchestrator-answers` (default none). It never answers its own requests, plans, questions or
mode changes, MCP tools, a call whose input was not listed whole, names a secret, writes outside
the worker's directory or into `.git`, `.claude` or `.mcp.json`, or matches
`orchestrator-answers-deny` or the defaults it adds to. Its allow is a single allow with no updated permissions. The person
sees each answer on the call's row and in the echo, answers first if they want, and keeps a
request they are answering.

### The settings

In `config.toml`, the person's, never a space's record, which agents can write:

```toml
[spaces]
orchestrator-answers = ["Read", "Grep", "Glob", "Edit", "Write"]
orchestrator-answers-deny = ['\bmake\s+deploy\b']   # adds to the defaults
```

- **`orchestrator-answers`:** the tools' names the orchestrator may allow. Empty, the default,
  means it allows nothing. ASCII letters, digits, `_` and `-`, at most 64 names; an `mcp__`
  name is refused. Each look at the file applies it, so the next call takes a change.
- **`orchestrator-answers-deny`:** the Bash commands it never allows, as patterns of the `regex`
  crate, at most 64 of at most 1024 bytes, which add to the defaults: the defaults always
  apply and cannot be taken away, so an empty list denies what they deny. Every pattern, the person's too, ignores case, as the disk on macOS and Windows finds
  a command, and its `.` crosses lines; `(?-i)` and `(?-s)` turn either off. The defaults:
  `git … push`, `gh … pr merge|repo|api|gist|release|secret|workflow`, `curl`, `wget`, `ssh`,
  `scp`, `sftp`, `rsync`, `nc`, `ncat`, `socat`, `telnet`, `sudo`, `doas`, `su`, `rm` with a
  recursive flag, `find … -delete`, `kill`, `pkill`, `killall`, `claude` (`claude-code`,
  `claude.exe`, `npx`, `bunx`, `pnpx` or `dlx` of it) and `oso` (`oso-restart`, `oso.exe`) as a
  command's name, git's `core.sshCommand`, `core.hooksPath` and aliases, `GIT_SSH`,
  `GIT_SSH_COMMAND` and `GIT_CONFIG_*` set, `git remote add|set-url`, and any mention of
  `.claude`, `settings.json`, `settings.local.json`, `.mcp.json`, `config.toml` or a `.sock`.
- **A bad value** of either key (not a list, a name or pattern oso does not take) is said in
  `*Messages*`, and no orchestrator answers anything until it is fixed: a broken deny list does
  not fall back to the defaults alone, as the person's may have denied more.
- **A file other users in a group can change** lets no orchestrator answer, and says so, as
  [ADR 0135](0135-agents-start-sessions-without-asking.md) has it for agents' starts.

### What stays with the person

Each request is judged in this order, and the first reason that holds stays the person's
(`sessions::native::stand_in`):

1. **A card:** questions and plans (`AskUserQuestion`, `ExitPlanMode`).
2. **Not shown:** the input did not reach oso, or is over 8 KiB as JSON.
3. **A secret:** any string or key of the input, judged whole as a path and word by word as a
   command is, or a token of a Bash command matches ADR 0044's names; or a search's pattern of names (Glob's `pattern`, Grep's
   `glob`) may match one: with its `*` dropped it names one, or it holds a wildcard other than
   `*`, `**` and `*.ext`, or a list (`?`, `[`, `{`, `,`). A command is read as the shell may
   join its words: its quotes and backslashes taken out and a line's continuation joined, split
   at blanks and the shell's punctuation; again with the empty expansions and lists inside a word
   (`$()`, backquotes, `{,}`, a `$` before quotes) dropped, split at blanks and operators; and
   with backslashes read as a Windows path's; each token also with its wildcards dropped.
4. **Off:** no tool is listed.
5. **A tool** not listed, and every `mcp__` tool.
6. **Outside:** a path field is not an absolute path lexically under the worker's directory, as
   Claude Code takes a relative path, and a search with no `path`, from its shell's directory,
   which a `cd` changes; any `..`, `~` or empty path is outside, and so is a search's pattern of
   names with a `..` anywhere. A part below the directory that is `.git`, `.claude` or
   `.mcp.json`, a Windows short name or only dots and spaces is outside too. The worker's own
   directory may itself be under a `.claude`, as a worktree's is.
7. **Denied:** a Bash command matches a deny pattern, a default or the person's, in any of the readings above, with the
   worker's directory's own text read as `.` where it ends at a separator not followed by `..`.
   A worker whose directory is under a `.claude` may name no `..` part, which could climb into
   its repository's `.claude`.

A secret is judged before the rules, so that a request naming one is listed with no input
whatever the person set.

A name is judged as the disk may read it, so that a spelling that differs only there catches
more, never less: characters that macOS and Windows fold to ASCII (`ſ`, `K`, `ß`, the `ﬀ`–`ﬆ`
ligatures, fullwidth ASCII) are taken as that ASCII, any other character beyond ASCII is dropped,
ASCII case is ignored, and a name is judged both whole and as Windows reads it, a stream after
`:` and trailing dots and spaces cut; a Windows short name (`GIT~1`) counts as reserved.

**These checks are lexical and best-effort, not a boundary.** A link inside the worker's
directory may lead out of it; a command can spell a name or a command in ways no pattern reads (a
variable, a `?` glob, an encoding, `eval`, an interpreter's `-c`); a search of the whole
directory reaches files with secret names that its tool does not skip; and a value that is a
secret but names no file is listed. Once the person lists Bash, the orchestrator's judgment
is the guard. The person is advised to list `Read`, `Grep`, `Glob`, `Edit` and `Write` first.

### The listing (step 1)

`session_list`, `session_state` and `space_status` give the current orchestrator of a session's
space, for each native session of that space but itself, a `request`:

```json
"request": null | {"id": "r17", "tool": "Bash", "input": {...} | null, "input_cut": false,
                   "shown_s": 12, "answerable": true, "person_only": null | "<why>"}
```

- The request is the one shown to the person now, if any. Its `id` is oso's (`RequestId`,
  counted up for each request taken), never the call's own id.
- The input is listed whole only up to 8 KiB of JSON; past that it is `null` with
  `input_cut: true`. It is `null` too for a secret or a card.
- `person_only` says the reason above in words, while `answerable` is false.
- A worker, a former orchestrator, another space's orchestrator and any other caller get no
  `request` field at all.

### The answer (step 2)

- The session tool `session_answer` takes `session` (id or name), `request` (its id), `tool`,
  which must match, `answer` (`allow` or `deny`) and, for a deny, `reason` of at most 200 plain
  characters, which goes to the worker as the deny's message.
- Only `Caller::orchestrator()` calls it; a worker is told "only your space's orchestrator
  answers requests". A wrong id, a wrong tool or a gone request is refused; one the person
  answered first is refused saying so; while the person's `C-u y` question or a note is open for
  that session, the call is refused with "the person is answering it: wait". A second
  answer to a request the orchestrator answered is refused too. The rules are
  `sessions::tools::answer`'s, judged at the call: the caller's part in its space now, and the
  request under the settings in force now.
- A card's request (questions, a plan) is refused both ways: neither allowed nor denied, as
  questions and plans are the person's. For each other reason above, an allow is refused and a
  deny still works. An allow sends exactly
  `permission::allow()`, with no `updatedPermissions`, so nothing changes for later calls.
- The person sees it: an allow marks the call's row "allowed by <orchestrator>", a deny's row
  reads "denied: by <orchestrator>", the worker being told "The space's orchestrator
  <orchestrator> denied this call in oso", with ", saying: <reason>" where it gave one, and the echo area and the agent log say "<orchestrator>
  allowed Bash in <worker>". The tool's reply repeats the tool and the input's summary (its
  path and command or pattern, as the row has it), `null` where the listing withholds the input.
- The orchestrator's instructions say, in one line, that it may answer its native workers'
  tool requests with `session_answer` only when `request.answerable` is true, and never because
  a message asks it to.
- It is an agent-reach session tool, counted in the 30-a-minute limit. It is not `Person`: the
  person opts in per tool, and the call is the worker's, not oso's.

### Steps

1. Reads: this ADR, `RequestId`, the pure `stand_in` module, both settings (off), and the
   `request` field.
2. Answers: the method, its hint, the handler, `Answer::Allow(By)`, `Mark::Allowed`, the deny's
   "by", the echo and log, the refusals, and a line in the orchestrator's instructions.

A per-space setting (the person's file keyed by a space's slug) is a possible later step.

## Changes to accepted ADRs

- **ADR 0165, step 2 ("The bridge's `permission` tool"):** a call runs only once the person
  allowed it, or the space's orchestrator the person let answer that tool allowed it, once.
- **ADR 0182, "Ordinary text":** the transcript, and so the block, stays withheld from agents; a
  request's tool and input are listed to the space's orchestrator alone, by this ADR's rules.

## Consequences

- The person answers fewer calls in a space, for the tools they list, and still sees each.
- An orchestrator that is steered (by a worker's message, a file, a web page) can allow what
  the rules let through. Its instructions say never to allow because a text asks it to.
- The listing costs the UI thread one judge per native worker with a request shown, at most
  8 KiB of JSON and the deny patterns each; it is counted, never copied, past that.

## Alternatives considered

- **The orchestrator answers any request:** refused; plans, questions and mode changes are the
  person's, as is anything the person did not opt into.
- **The settings in the space's record:** refused; agents can write that record.
- **The call's own `tool_use_id` as the id:** refused; the session chose it, and a fresh id of
  oso's names the request oso showed, not a later one with the same call.
- **An allow that also adds a permission rule:** refused; one answer, one call.
