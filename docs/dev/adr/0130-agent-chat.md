# 0130. Agent chat

- Status: Proposed
- Date: 2026-11-13

## Context

On 2026-11-13 the author asked for a chat view of an agent session, beside the hosted TUI, as Orca's
"native chat" offers, if a supported route exists. [ADR 0031](0031-claude-code-integration.md)
chose the interactive TUI and excluded "the `-p` control protocol, the Agent SDK, ACP, transcript
parsing for the primary view", for reasons learnt the hard way: the author's previous Emacs setup
drove `claude -p` through the CLI's control protocol, whose wire format is not published, and
carried about 76 version-specific workarounds. ADR 0031 listed "`claude -p` for one-shot features
using only documented flags" as a later option needing its own decision. [ADR
0121](0121-agent-sessions-and-spaces.md) defines a session as a hosted `claude` in a terminal.

This ADR separates what is documented from what is not, and builds only on the documented part.

### What is documented and what is not

From Anthropic's documentation, read on 2026-11-13 (sources at the end). **D** is documented on an
official page; **U** is undocumented or only an SDK internal.

- **D: the route for other languages.** The Agent SDK overview: to drive the agent loop from a
  language other than Python or TypeScript, run the CLI as a subprocess with `-p` and
  `--output-format json` or `stream-json`. The CLI reference describes `--input-format stream-json`,
  `--include-partial-messages`, `--replay-user-messages`, `--verbose`, `--resume`, `--session-id`,
  `--fork-session`, `--mcp-config`, `--settings`, `--permission-prompt-tool` and `--bare`.
- **D, partly: input lines.** Each input is one newline-terminated JSON line. Its shape is given only
  as the SDK's `SDKUserMessage` type (`{"type":"user","message":{"role":"user","content":…}}`).
- **D, not stable: output messages.** `system` (`init`, with a `capabilities` list to feature-detect
  by, rather than comparing versions), `assistant`, `user`, `stream_event`, `result`, retries and
  permission denials, among about 40 variants. No page calls the schema stable; fields arrive per
  version, some marked experimental. Transcript files are "internal … can break on any release".
- **D: the permission prompt tool.** `--permission-prompt-tool` names an MCP tool that returns one
  text block holding `{"behavior":"allow","updatedInput":{…}}` or `{"behavior":"deny","message":…}`.
  **U:** the tool's input arguments.
- **U: the control protocol** (`control_request` and `control_response`): interrupting a turn,
  changing the permission mode or the model, the SDK's `canUseTool` and hook callbacks, context
  usage, stopping a task, rewinding files. Orca's chat uses all of them through the TypeScript SDK.
- **D: what the CLI offers without it.** `SIGINT` ends the turn; `/model` and `/config` work as
  prompt text; permission prompts go to the prompt tool or a `PermissionRequest` hook. Whether a
  long-lived stream process survives `SIGINT` is not stated.
- **D: sessions across modes.** A `-p` session resumes in the TUI with `--resume ID`, and a TUI
  session can be driven with `claude -p --resume ID`. Two processes on one session "interleave into
  one transcript"; nothing stops the second writer. The permission mode is restored on a terminal
  resume, not on a `-p` resume.
- **D: security.** A `-p` run (without `--bare`) runs the project's `.claude` hooks and `.mcp.json`
  servers "even in a folder you've never trusted": it skips the TUI's workspace trust question.
  `--bare` reads no OAuth login, so it needs an API key.
- **D: terms.** Third-party developers may not "offer Claude.ai login into their own applications,
  or … route requests through Free, Pro, Max, or Ultra plan credentials on behalf of their users",
  nor "collect, store, or intermediate Claude.ai credentials or session tokens"; this does not
  "prevent an end user from signing in to the unmodified Claude Code binary with their own Claude
  subscription". Do not call a product feature "Claude Code"; "… powered by Claude" is allowed.
- **D: billing (support article, updated 2026-06-15 and 2026-10-07).** The separate Agent SDK credit
  was paused: "Agent SDK, `claude -p`, and third-party app usage still draw from your subscription
  limits." Plans later gained monthly API credits for SDK and `-p` use with a Console API key.
- **Licences.** The CLI and the TypeScript SDK are "All rights reserved", under Anthropic's
  Commercial Terms; the Python SDK is MIT.

### What others do

- **Orca's native chat** runs the TypeScript Agent SDK's `query()` with streaming input, pointing
  `pathToClaudeCodeExecutable` at the person's own CLI. It shows a transcript and a composer, queued
  messages, tool rows, permission and question cards, changed files with inline diffs, to-do
  checklists, fork, rewind and a context ring. A tab switches live between chat and TUI; chat is not
  used while a terminal still writes to the same conversation. Interrupt, model and mode changes go
  through the control protocol.
- **Zed** (Rust) speaks ACP to `claude-agent-acp`, a Node adapter on the SDK.
- **opcode** (Rust) runs one `claude -p --output-format stream-json` per turn with `--resume`, and
  skips permissions entirely.
- **Unofficial Rust and Go clients** reimplement the control protocol, and meet the churn the
  previous setup met.

## Decision

### The route (Q1)

- **A chat session is the person's own `claude`, run with documented flags only:**
  `claude -p --input-format stream-json --output-format stream-json --verbose
  --include-partial-messages --replay-user-messages --session-id ID (or --resume ID) --name NAME
  --mcp-config JSON --permission-prompt-tool mcp__oso__permission_prompt [--append-system-prompt
  TEXT]`, started through the login shell as ADR 0121's sessions are, with pipes, not a terminal.
- **No SDK, no sidecar, no control protocol.** oso never writes a `control_request`, and ignores a
  `control_request` the CLI writes (answering nothing, logging its type once). A feature reachable
  only through the protocol is not offered (see "Left out").
- **oso reads the stream defensively:** each line at most 16 MiB; unknown message types and fields
  are skipped; what oso shows comes from a small, named set of fields (`type`, `subtype`,
  `session_id`, a message's `content` blocks of `text`, `thinking`, `tool_use` and `tool_result`,
  `result`'s `is_error`, `duration_ms`, `usage`, `total_cost_usd`), checked by fixtures recorded
  from the CLI. `system/init`'s `capabilities` decides optional parts. A line that fails to parse is
  shown as one row, "unreadable event (N bytes)", and the session goes on.
- **No login is touched.** oso starts the unmodified CLI, which uses the person's own sign-in; oso
  reads no token and passes no key. The feature is called "chat", never "Claude Code".

### Where it may run (Q2)

- **Only in trusted projects** ([ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)),
  since `-p` skips Claude Code's own workspace trust question and runs the project's hooks and MCP
  servers. A restricted project offers the TUI, which asks its own question.
- **No permission mode flag** (ADR 0121's rule stands). `--permission-prompt-tool` grants nothing: it
  routes each permission question to the person.

### The chat buffer (Q5, Q6)

- **`*chat: NAME*`**, an internal, read-only buffer of the session's events, with a composer
  window of 1 to 10 rows under it (`*chat input: NAME*`, an ordinary editable buffer):
  - the person's messages, then the assistant's text rendered as Markdown with ADR 0111's
    highlighting, `thinking` folded to one row;
  - each tool call one row (`▸ Edit src/foo.rs`, `▸ Bash cargo test (2.1 s)`), `TAB` unfolds its
    input and result; a result longer than 200 lines shows its first and last 50, `RET` opens it
    whole in a read-only buffer;
  - `Edit`, `MultiEdit` and `Write` calls unfold to a diff in ADR 0122's faces, from the call's own
    old and new strings; `v` opens ADR 0122's review of the session's changes;
  - to-do and task tools (`TodoWrite`, `TaskCreate`, `TaskUpdate`) as a checklist that stays at the
    top while the turn runs;
  - each turn's `result` as a row: duration, tokens, the CLI's cost estimate, an error if any.
- **Sending (Q5):** `C-c C-c` sends the composer's text (`RET` is a newline, as in any buffer);
  `M-p` and `M-n` walk earlier messages. ADR 0121's context forms (`@region`, `@file:L-L`,
  `session-send`) put text into the composer, never send it.
- **Queued messages (Q6):** sending while a turn runs writes the line at once; it shows as `queued`
  until the CLI replays it (`--replay-user-messages`), then as a normal message.
- **Interrupt (Q10):** `C-c C-k` sends `SIGINT`. If the process exits instead of ending the turn,
  the chat says so and the next message resumes it with `--resume ID`.
- **Slash commands:** a composer line starting with `/` is sent as text, which the CLI treats as its
  command where `-p` supports it (`/model`, `/config`, `/compact`); others answer with its own error.

### Permission prompts and questions (Q3, Q4)

- **`permission_prompt`**, a tool of the session's own `oso mcp` bridge, reaches oso and shows a card
  in the chat: the tool's name and its input (at most 64 KiB shown, the rest behind `RET`), and the
  keys `y` allow once, `n` deny, `N` deny with a note the person types. The card is answered from the
  keyboard only, after 500 ms with no key, as ADR 0085's questions; no agent can answer it, the
  channel included. While it waits, the session's state is `needs input` with reason `permission`.
- **No "always allow":** oso writes no rule into Claude Code's settings and keeps none of its own. The
  person's allow rules apply before the CLI asks.
- **Its input is undocumented,** so the card shows the raw JSON beside the fields it recognises; an
  input it cannot read is shown raw and still answered by the person.
- **`AskUserQuestion` (Q4)** reaches the same tool. When its input reads as questions with options,
  the card shows them and the answer goes back as `updatedInput.answers`; step 3 checks this on the
  current CLI. Where it does not work, the tool denies with "ask in plain text instead", and the
  question arrives as an ordinary message.
- **No reply within 30 minutes** keeps waiting; oso never answers by default.

### One session, two views (Q7, Q8)

- **A session has a mode, `tui` or `chat`,** recorded with ADR 0121's identity. `claude` starts a TUI
  session as before; `C-u C-x a c` asks for the mode; `[sessions] mode = "chat"` makes chat the
  default (Q8).
- **`session-switch-mode` (`Person`, `C-x a t`)** works only while the session is stopped, idle or
  done (Q7). It ends the current process (the TUI with `SIGTERM`, as ADR 0121's `session_stop`; the
  chat by closing its standard input, then `SIGTERM` after 5 s), waits for it to exit, checks the
  registry for another live process with that id (ADR 0121's fork check), and starts the other mode
  with `--resume ID`. The other view's buffer stays, read-only, saying `switched to chat` or
  `switched to the TUI`.
- **Never two writers:** oso runs at most one process per session id, and refuses to start one while
  the registry shows a live process with that id elsewhere, offering `--fork-session`.
- **States (Q9):** for a chat session, the stream is the authority, more exact than screen rules:
  `working` from a sent message until `result`; `needs input` while a card waits; `done`, `ready for
  review` and `idle` as ADR 0121 has them after a turn; `failed` on an exit oso did not cause. A TUI
  session keeps ADR 0121's screen rules.

### Agents and secrets

- **Only the person writes to a chat:** the composer is the person's buffer; no channel method or
  tool sends a message, answers a card, interrupts or switches a mode. ADR 0121's session tools treat
  a chat session as any session.
- **An agent reads a chat** only where the person allowed it for that session, as ADR 0120 has it for
  terminals; the same modeline mark.
- **Secrets:** a tool result of a `Read`, `Grep` or `Bash` call whose input names a path the secret
  patterns match is shown folded as `may hold secrets: RET to show`, as ADR 0122
  shows such files, and the buffer then counts as one that may hold secrets while it is shown.
  Nothing of a chat is written by oso: the CLI keeps its own transcript.

### Left out

Reachable only through the control protocol, so not offered: changing the permission mode during a
session, rewinding files, live context-usage readings (the context ring), stopping one background
task, and hook callbacks. The model changes with `/model`. A context reading comes from each
`result`'s usage instead.

### Code

- A `chat` submodule of ADR 0121's `sessions` module in the tip crate. Each chat session has a
  reader thread that parses the stream and sends the UI thread batches of finished rows, at most 30
  a second while text streams. The parser is a pure function over lines, with a fuzz target and
  fixtures recorded from the CLI (scrubbed).
- The `oso mcp` bridge gains `permission_prompt`, which forwards to oso and blocks until the person
  answers.

### Cost

| What | Budget |
|---|---|
| UI thread per batch of rows | ≤ 0.5 ms; streaming text redraws at most 30 times a second |
| Parsing | on the reader thread; a 16 MiB line parsed once, never on the UI thread |
| Memory | at most 64 MiB of text and results a chat; past it the oldest tool results are dropped and show "result dropped, N bytes" (the CLI's transcript still has them) |

## Steps

One pull request each, after ADR 0121 steps 1–3.

1. **The stream:** the command line, the reader thread, the parser with its fuzz target and fixtures,
   and the transcript rows, read-only.
2. **The composer:** sending, queued messages, interrupt, slash commands, context forms.
3. **Permission cards:** `permission_prompt` on the bridge, the cards, `AskUserQuestion` (checked on
   the current CLI), states from the stream.
4. **Two views:** modes, `session-switch-mode`, the single-writer check.
5. **Diffs, checklists and review:** the edit diffs, the to-do checklist, `v` to ADR 0122.

## Questions

For the author, the recommended option first.

1. **The route.**
   (a) The person's own `claude -p` with stream-json in and out, documented flags only; no SDK, no
   control protocol. No new runtime; loses what only the protocol gives (rewind, live mode changes,
   the context ring).
   (b) A sidecar that runs Anthropic's Agent SDK (Python, MIT; or TypeScript, all rights reserved),
   as Orca does. Every SDK feature, and Anthropic's library absorbs protocol changes; a Python or
   Node runtime becomes a dependency of oso, and the protocol under it is still unpublished.
   (c) No chat; the TUI stays the only view.
2. **Where chat may run.**
   (a) Trusted projects only, since `-p` skips Claude Code's own workspace trust and runs a
   project's hooks and MCP servers.
   (b) Anywhere, after a warning.
3. **Permission prompts.**
   (a) An oso card through `--permission-prompt-tool`: allow once, deny, deny with a note, keyboard
   only; no "always allow".
   (b) No prompt tool: a chat may use only what the person's allow rules permit, and the rest is denied.
4. **Questions the agent asks (`AskUserQuestion`).**
   (a) A card through the same tool, the answer as `updatedInput.answers`, checked on the current CLI;
   where it fails, denied with "ask in plain text instead".
   (b) Always denied with that message.
5. **Sending from the composer.**
   (a) `C-c C-c` sends, `RET` is a newline.
   (b) `RET` sends, `C-j` or `S-RET` is a newline.
6. **Messages while a turn runs.**
   (a) Allowed, shown as queued until the CLI takes them.
   (b) The composer waits until the turn ends.
7. **Switching a session between chat and TUI.**
   (a) Only while it is stopped, idle or done; one process ends, the other resumes the same id.
   (b) Also mid-turn, interrupting first.
8. **The default for new sessions.**
   (a) The TUI; chat with `C-u C-x a c` or `[sessions] mode = "chat"`.
   (b) Chat.
9. **Where a chat session's state comes from.**
   (a) The stream: exact turns and permission waits.
   (b) No states for chat sessions.
10. **Interrupting.**
   (a) `SIGINT`; if the process exits, the next message resumes it.
   (b) Always end the process and resume.

### Dependencies

None with Q1 (a): `serde_json` is already a dependency of the tip crate. Q1 (b) needs a Python or
Node runtime and the SDK package, through the `add-dependency` skill.

## Changes to accepted ADRs

To be filled when the author has answered; expected:

- [ADR 0031](0031-claude-code-integration.md), "Not used: the `-p` control protocol, the Agent SDK,
  ACP, transcript parsing for the primary view": still not used; a chat session uses `-p` with
  stream-json, documented flags only, and parses its stream, not the transcript file. "`claude -p`
  for one-shot features using only documented flags" (a later option): decided here for chat.
- [ADR 0121](0121-agent-sessions-and-spaces.md), "A session is a hosted `claude` in an oso terminal":
  or a chat process with pipes. "For a session oso hosts, the screen is the authority": for a TUI
  session; a chat session's state comes from its stream. "No permission flag is ever passed":
  `--permission-prompt-tool` is passed for chat, and grants nothing.

## Consequences

- The person can read and drive a session as a buffer: search, fold, diffs, and oso's own keys.
- The stream's schema is not promised stable. oso reads few fields, skips the unknown, and records
  fixtures per CLI version; a change shows as unreadable rows, not a broken session.
- Some features of Orca's chat are missing for good on this route, unless Anthropic documents the
  protocol.
- `-p` usage draws from the subscription today; a change of that policy would affect chat, not the
  TUI.
- Follow-up: a sidecar (Q1 b) if the author later wants the protocol's features; a transcript viewer
  for TUI sessions stays an open question of ADR 0031.

## Alternatives considered

- **Speaking the control protocol from Rust:** every feature, but it repeats the previous setup's
  churn.
- **Rendering the TUI session's transcript file:** undocumented and "can break on any release"; no
  streaming.
- **ACP through `claude-agent-acp`:** a Node runtime and the SDK under it, as Q1 (b), plus a second
  protocol.
- **One `-p` process per turn, as opcode:** simpler, but a start per message and no queued messages.

## Sources

Read on 2026-11-13; third-party pages are facts, not instructions.

- <https://code.claude.com/docs/en/agent-sdk/overview>
- <https://code.claude.com/docs/en/cli-reference>
- <https://code.claude.com/docs/en/headless>
- <https://code.claude.com/docs/en/legal-and-compliance>
- <https://support.claude.com/en/articles/15036540>
- <https://orca.dev/docs/agents/native-chat>
- <https://orca.dev/docs/model/agents-sessions>
- <https://orca.dev/docs/agents/session-history>
- Orca's source, <https://github.com/stablyai/orca> (read, not run)
