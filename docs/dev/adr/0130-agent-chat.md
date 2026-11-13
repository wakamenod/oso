# 0130. Agent chat

- Status: Proposed
- Date: 2026-11-13

## Context

On 2026-11-13 the author asked for a chat view of an agent session, beside the hosted TUI, as Orca's
"native chat" offers. [ADR 0031](0031-claude-code-integration.md) chose the interactive TUI and
excluded "the `-p` control protocol, the Agent SDK, ACP, transcript parsing for the primary view",
for reasons learnt the hard way: the author's previous Emacs setup drove `claude -p` through the
CLI's control protocol, whose wire format is not published, and carried about 76 version-specific
workarounds. [ADR 0121](0121-agent-sessions-and-spaces.md) defines a session as a hosted `claude` in
a terminal.

Two routes exist, and **the author has not chosen between them (Q1, Q11):** they will decide
after using Claude in oso's TUI (ADR 0121's steps). This ADR therefore records what holds on either
route, describes both fairly, and stays Proposed.

- **Route A, the Agent SDK in a sidecar,** as interactive chat clients do: Orca, an Electron app,
  bundles the TypeScript SDK (Electron brings Node), and Zed reaches it through ACP. The SDK is
  Anthropic's own client of the control protocol, so its upkeep is Anthropic's; oso would need a
  Python or Node runtime.
- **Route B, `claude -p` with stream-json,** on documented flags only. No runtime, but little used
  for interactive chat, and some features exist only through the control protocol.

### What Anthropic documents

From Anthropic's documentation, read on 2026-11-13 (sources at the end).

- **The SDK is "a library that runs the Claude Code binary"**, over its stdio. Two exist:
  - **Python**, `claude-agent-sdk` on PyPI, MIT licence. `ClaudeSDKClient` holds a long-lived,
    streaming conversation with `query`, `receive_messages`, `interrupt`, `set_permission_mode`,
    `set_model`, `rewind_files`, `get_context_usage`, `stop_task`, `get_server_info`, and the MCP
    status and toggles. `ClaudeAgentOptions` has `cli_path` (a specific `claude` executable),
    `can_use_tool` (the permission callback), `extra_args`, `enable_file_checkpointing` and the
    usual `cwd`, `resume`, `mcp_servers`. The package carries a CLI of its own.
  - **TypeScript**, `@anthropic-ai/claude-agent-sdk`, "All rights reserved", under Anthropic's
    Commercial Terms. Its `Query` has the Python methods and more (`supportedCommands`,
    `supportedModels`, `accountInfo`, `applyFlagSettings`, `reloadSkills`, …). It bundles a native
    `claude` as an optional dependency; `pathToClaudeCodeExecutable` points at another. "The SDK
    version tracks the bundled Claude Code version."
- **Permissions:** `can_use_tool(tool_name, input, context)` is called for each call that the
  person's allow rules and the permission mode do not settle, and returns allow (with the input,
  possibly changed) or deny (with a message Claude sees). **`AskUserQuestion`** reaches the same
  callback: 1–4 questions of 2–4 options, answered by allowing with `updated_input` carrying
  `answers` (question text to the chosen label).
- **Messages:** user, assistant, system (with task started, progress and notification), result
  (with `usage`, `total_cost_usd`, `model_usage`), stream events and rate-limit events. No page
  calls their schema stable.
- **File rewind:** with `enable_file_checkpointing` and replayed user messages, `rewind_files(id)`
  restores files to their state at a user message. It does not undo what a Bash command changed
  (except in files the conversation had edited), subagents' edits, or created and deleted
  directories.
- **Security:** a non-interactive run, which an SDK session is, runs the project's `.claude` hooks
  and `.mcp.json` servers "even in a folder you've never trusted".
- **Terms:** the SDK's use is "governed by Anthropic's Commercial Terms of Service". The SDK
  overview: "Unless previously approved, Anthropic does not allow third party developers to offer
  claude.ai login or rate limits for their products, including agents built on the Claude Agent
  SDK. Use the API key authentication methods … instead." The legal page: this does not "prevent an
  end user from signing in to the unmodified Claude Code binary with their own Claude
  subscription". Do not call a product feature "Claude Code".
- **Billing:** "Agent SDK, `claude -p`, and third-party app usage still draw from your subscription
  limits" (support article, 2026-10-07).

### What others do

- **Orca's native chat** runs the TypeScript SDK's `query()` with streaming input and
  `pathToClaudeCodeExecutable` set to the person's own CLI ("the SDK falls back to a bundled binary
  that is not installed"). It shows a transcript and a composer, queued messages, tool rows,
  permission and question cards, inline diffs, to-do checklists, fork, rewind (only when idle) and
  a context ring; a tab switches between chat and TUI, never with both writing.
- **Zed** (Rust) speaks ACP to `claude-agent-acp`, a Node adapter on the TypeScript SDK.
- **opcode** (Rust) runs one `claude -p` per turn and skips permissions.
- **Unofficial Rust and Go clients** reimplement the control protocol, and meet the churn the
  previous setup met.

## Decision

### The route (Q1: open)

On either route:

- **A chat session is the person's own `claude`,** resolved through the login shell as ADR 0121's
  sessions are, with oso's session id and the session's `oso mcp` bridge as its MCP server.
- **No login is touched (Q16, not asked yet).** The unmodified CLI uses the person's own sign-in;
  oso reads no token and passes no key. The feature is called "chat", never "Claude Code".
  Publishing oso with chat waits for the terms check that is already an open question.
- **oso's Rust code never writes the control protocol.** On route A the SDK does, inside the
  sidecar; on route B nobody does, and a `control_request` the CLI writes is ignored and logged
  once.

Where the routes differ:

| | A: the SDK in a sidecar | B: `claude -p` with stream-json |
|---|---|---|
| Process | the person's `claude` under the SDK (`cli_path`), in a sidecar of oso's own | `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --replay-user-messages`, with pipes |
| Runtime | Python 3.10+ and the SDK (or Node, Q11), through `chat-setup` | none |
| Upkeep | the SDK follows the CLI; oso keeps the sidecar and the fields it reads | oso reads the documented stream; the schema is not promised stable |
| Permission cards | the SDK's `can_use_tool` | `--permission-prompt-tool` naming a bridge tool; its input is undocumented |
| `AskUserQuestion` | documented through `can_use_tool` | unverified through the prompt tool; else denied with "ask in plain text instead" |
| Interrupt | `interrupt()`; the session stays | `SIGINT`; whether a long-lived process survives it is undocumented, else the next message resumes |
| Model | `set_model` | `/model` sent as prompt text |
| Permission mode | `set_permission_mode` | none during a session |
| Context ring | `get_context_usage` | each result's `usage` |
| Rewinding files | `rewind_files` | between turns, `claude -p --resume ID --rewind-files UUID` with checkpointing on (on the checkpointing page, not in `--help`) |
| Stopping one background task | `stop_task` | none |
| Who uses it | Orca (TypeScript, in Electron), Zed through ACP | opcode, one process per turn, no permissions; rare for interactive chat |
| Terms | the SDK overview's sentence on claude.ai login names SDK agents | the same CLI and login; the legal page's carve-out for the unmodified binary |

### Route A's sidecar (Q11: open; Q12; Q13 not asked yet)

The rest of this section holds only on route A, written for Python; with TypeScript (Q11 (b)) the
sidecar would be a Node script and `chat-setup` would install the SDK with the person's `npm`.

- **The sidecar** is a small program of oso's own, `oso_chat.py`, which holds one
  `ClaudeSDKClient` and speaks to oso over its standard input and output. One sidecar per chat
  session. `cli_path` is the person's `claude`, never the copy the SDK package carries.

- **Its code (Q13)** lives in oso's repository beside the `chat` module, is compiled into oso's
  executable, and runs as `PYTHON -I -c SCRIPT`: `-I` ignores `PYTHON*` variables and the user's
  site directory and keeps the current directory off the import path. It is never read from a
  project or from disk. It runs through `oso-platform::process` with ADR 0120's environment
  allowlist, from oso's state directory; the SDK's `cwd` is the project.
- **Its protocol (Q13):** JSON lines, each at most 16 MiB, on standard input and output; standard
  error goes to oso's log, bounded.
  - sidecar to oso: `hello` (protocol version, SDK version, Python version), `event` (an SDK
    message as a JSON object), `permission` (an id, the tool's name, its input), `reply` (to a
    request), `error`;
  - oso to sidecar: `send` (a user message), `interrupt`, `answer` (a permission id, allow or deny,
    the input or a message), `set_model`, `set_mode`, `rewind`, `context`, `stop_task`, `close`.
  A protocol version oso does not know ends the session with a message.
- **The runtime (Q12):** `chat-setup` (`Person`) runs the person's own `uv` in a project terminal
  (ADR 0120), visibly and stoppably: a virtual environment in oso's data directory, and the SDK
  installed from a requirements file of exact versions and hashes (`--require-hashes`) that oso
  carries, for the SDK and every package it pulls in. Nothing is installed without the person's
  command; oso itself has no network code. `[chat] python`, an absolute path, names another
  interpreter instead.
- **Version pinning (Q12):** oso pins one SDK version, tested with recorded fixtures. A sidecar whose
  `hello` names another version warns once and runs. A newer oso may raise the pin; `chat-setup`
  installs it.
- **No interpreter or no SDK:** a chat start says "run chat-setup" and offers the TUI.

On route B there is no sidecar: oso runs the `claude -p` line above through `oso-platform::process`
and reads its stream on the reader thread, with the same limits.

### Where it may run (Q2)

- **Only in trusted projects** ([ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)),
  since an SDK session skips Claude Code's own workspace trust question and runs the project's hooks
  and MCP servers. A restricted project offers the TUI, which asks its own question.

### The chat buffer (Q5, Q6)

- **`*chat: NAME*`**, an internal, read-only buffer of the session's events, with a composer window
  of 1 to 10 rows under it (`*chat input: NAME*`, an ordinary editable buffer):
  - the person's messages, then the assistant's text rendered as Markdown with ADR 0111's
    highlighting, thinking folded to one row;
  - each tool call one row (`▸ Edit src/foo.rs`, `▸ Bash cargo test (2.1 s)`); `TAB` unfolds its
    input and result; a result longer than 200 lines shows its first and last 50, `RET` opens it
    whole in a read-only buffer;
  - `Edit`, `MultiEdit` and `Write` calls unfold to a diff in ADR 0122's faces, from the call's own
    old and new strings; `v` opens ADR 0122's review of the session's changes;
  - to-do and task tools (`TodoWrite`, `TaskCreate`, `TaskUpdate`) as a checklist that stays at the
    top while the turn runs; background tasks as rows, `k` on one stops it (`stop_task`);
  - each turn's result as a row: duration, tokens, the CLI's cost estimate, an error if any.
- **oso reads events defensively:** what it shows comes from a small, named set of fields, checked by
  fixtures recorded from the pinned SDK; unknown messages and fields are skipped; an event that does
  not read is one row, "unreadable event (N bytes)", and the session goes on.
- **Sending (Q5):** `C-c C-c` sends the composer's text (`RET` is a newline, as in any buffer);
  `M-p` and `M-n` walk earlier messages. ADR 0121's context forms (`@region`, `@file:L-L`,
  `session-send`) put text into the composer, never send it.
- **Queued messages (Q6):** sending while a turn runs passes the message to the SDK at once; it shows
  as `queued` until the CLI takes it, then as a normal message.
- **Slash commands:** a composer line starting with `/` is sent as text, which the CLI treats as its
  command where an SDK session supports it (`/compact`, `/config`); others answer with its own error.

### Interrupting, the model and the mode (Q10, Q14, Q15)

The keys are the same on both routes; what each does on route B is in the table above.

- **Interrupt (Q10):** `C-c C-k` calls `interrupt()`, which ends the turn and keeps the session.
  `C-c C-q` ends the sidecar (closing its input, then `SIGTERM` after 5 s); the next message starts
  it again with `resume`.
- **The model (Q14):** `C-c C-m` asks for a model name (completing the names in `[chat] models`) and
  calls `set_model`.
- **The permission mode (Q15):** a chat starts in the default mode, as ADR 0121 has every session.
  `C-c C-p` (`Person`) cycles default, `acceptEdits` and `plan` with `set_permission_mode`, shown
  in the modeline. `bypassPermissions` and `dontAsk` are never set. No tool or channel method
  changes it, and each start of the sidecar begins in the default again.
- **The context ring (Q14):** after each result, one `get_context_usage` call; the modeline shows
  `ctx 63%`.

### Rewinding (Q14)

- **File checkpointing is on for chat sessions.** `r` on one of the person's messages, only while
  the session is idle, asks a question naming the files that would be restored and saying that
  Bash's changes and directories are not undone; `y` calls `rewind_files`. The conversation is not
  rewound; the files are. Unsaved oso buffers of those files are not touched: the question names
  them, and they show the file changed on disk ([ADR 0029](0029-file-changes-on-disk.md)).

### Permission prompts and questions (Q3, Q4)

- **`can_use_tool` (Q3)** in the sidecar sends oso a `permission` line and waits (on route B, the
  bridge's prompt tool reaches oso instead). oso shows a card in
  the chat: the tool's name and its input (at most 64 KiB shown, the rest behind `RET`), and the keys
  `y` allow once, `n` deny, `N` deny with a note the person types. The card is answered from the
  keyboard only, after 500 ms with no key, as ADR 0085's questions; no agent can answer it, the
  channel included. While it waits, the session's state is `needs input` with reason `permission`.
- **No "always allow":** oso returns no permission updates, writes no rule into Claude Code's
  settings and keeps none of its own. The person's allow rules apply before the callback is called.
- **`AskUserQuestion` (Q4)** shows its questions and options on a card (number keys pick, `RET`
  confirms, multi-select where asked, and a free-text answer), returned as `updated_input` with
  `answers`.
- **No reply within 30 minutes** keeps waiting; oso never answers by default.

### One session, two views (Q7, Q8, Q9)

- **A session has a mode, `tui` or `chat`,** recorded with ADR 0121's identity. `claude` starts a TUI
  session as before; `C-u C-x a c` asks for the mode; `[sessions] mode = "chat"` makes chat the
  default (Q8).
- **`session-switch-mode` (`Person`, `C-x a t`)** works only while the session is stopped, idle or
  done (Q7). It ends the current process (the TUI with `SIGTERM`, as ADR 0121's `session_stop`; the
  chat's sidecar as `C-c C-q` does), waits for it to exit, checks the registry for another live
  process with that id (ADR 0121's fork check), and starts the other mode resuming the same id. The
  other view's buffer stays, read-only, saying `switched to chat` or `switched to the TUI`.
- **Never two writers:** oso runs at most one process per session id, and refuses to start one while
  the registry shows a live process with that id elsewhere, offering a fork.
- **States (Q9):** for a chat session, the SDK's messages are the authority, more exact than screen
  rules: `working` from a sent message until its result; `needs input` while a card waits; `done`,
  `ready for review` and `idle` as ADR 0121 has them after a turn; `failed` on an exit oso did not
  cause. A TUI session keeps ADR 0121's screen rules.

### Agents and secrets

- **Only the person writes to a chat:** the composer is the person's buffer; no channel method or
  tool sends a message, answers a card, interrupts, rewinds, or changes the model or the mode. ADR
  0121's session tools treat a chat session as any session. Every chat command is `Person` (ADR
  0085), since each starts or steers a process.
- **An agent reads a chat** only where the person allowed it for that session, as ADR 0120 has it for
  terminals; the same modeline mark.
- **Secrets:** a tool result of a `Read`, `Grep` or `Bash` call whose input names a path the secret
  patterns match is shown folded as `may hold secrets: RET to show`, as ADR 0122 shows such files,
  and the buffer then counts as one that may hold secrets while it is shown. Nothing of a chat is
  written by oso: the CLI keeps its own transcript.

### Left out

On route A with Python, what only the TypeScript SDK offers (the lists of commands and models,
account information, flag settings at run time, reloading skills). On route B, what only the control
protocol offers: changing the permission mode during a session, live context readings, stopping one
background task, and hook callbacks. Forking a chat from a message waits for a later decision.

### Code

- A `chat` submodule of ADR 0121's `sessions` module in the tip crate, and the sidecar's Python
  source beside it. Each chat session has a reader thread that parses the sidecar's lines and sends
  the UI thread batches of finished rows, at most 30 a second while text streams. The line parser is
  a pure function, with a fuzz target and fixtures recorded from the pinned SDK (scrubbed).
- The sidecar's own tests run it against a stub `claude_agent_sdk` module, so they need Python but
  no network and no `claude`.

### Cost

| What | Budget |
|---|---|
| UI thread per batch of rows | ≤ 0.5 ms; streaming text redraws at most 30 times a second |
| Parsing | on the reader thread; a 16 MiB line parsed once, never on the UI thread |
| Start | the sidecar's start and the SDK's import off the UI thread; the time to the first event measured in step 1 |
| Memory | at most 64 MiB of text and results a chat; past it the oldest tool results are dropped and show "result dropped, N bytes" (the CLI's transcript still has them); the sidecar's own memory measured in step 1 |

## Steps

One pull request each, after ADR 0121 steps 1–3; nothing here goes into them or holds them up.

1. **The stream:** on route A, `oso_chat.py`, its protocol, `chat-setup` and the requirements file
   (through the `add-dependency` skill); on route B, the command line; then the reader thread, the
   parser with its fuzz target and fixtures, and the transcript rows, read-only. Waits for Q1.
2. **The composer:** sending, queued messages, interrupt, slash commands, context forms.
3. **Permission cards:** `can_use_tool`, the cards, `AskUserQuestion`, states from the SDK's
   messages.
4. **Two views:** modes, `session-switch-mode`, the single-writer check.
5. **Diffs, checklists and review:** the edit diffs, the to-do checklist, background tasks, `v` to
   ADR 0122.
6. **The model, the mode, the context ring and rewinding.**

## Questions

For the author, the recommended option first. Q1–Q10 were asked of the first draft; Q9 and Q10
were restated for the SDK route; Q11–Q16 came from it. Q1 and Q11 are open, Q13–Q16 not asked yet.

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
   (a) The SDK's messages: exact turns and permission waits.
   (b) No states for chat sessions.
10. **Interrupting.**
    (a) The SDK's `interrupt()`: the turn ends, the session stays; a separate key ends the sidecar.
    (b) Always end the sidecar and resume.
11. **The SDK's language.**
    (a) Python (`claude-agent-sdk`, MIT). Its client has what oso needs (interrupt, mode and model
    changes, rewind, context usage, stopping a task); oso's sidecar script may be shipped freely.
    (b) TypeScript. Gets features first and has a few more; needs Node, and is "all rights reserved",
    so it can never be shipped with oso, only installed by the person.
12. **Installing the runtime and the SDK.**
    (a) `chat-setup` runs the person's own `uv` in a visible terminal: a virtual environment in oso's
    data directory and the SDK at oso's pinned version, from a hash-checked requirements file;
    `[chat] python` may name another interpreter. A different SDK version warns and runs.
    (b) The person installs it by hand and names the interpreter; oso only checks the version.
    (c) oso ships a Python and the SDK with itself. Nothing to install, but a large bundle, and oso
    takes on Python's updates.
13. **The sidecar's code and how oso talks to it.**
    (a) A small Python script of oso's own, compiled into oso's executable and run with `python -I
    -c`; JSON lines over its standard input and output.
    (b) A script installed beside the virtual environment by `chat-setup`, which the person may
    edit; the same JSON lines.
14. **Which SDK features come with chat.**
    (a) All in steps: first the stream, composer, cards and two views; then the model, the mode, the
    context ring and rewinding files (only when idle, after a question; Bash's changes are not
    undone).
    (b) The first steps only; the model, the mode, the context ring and rewinding later.
15. **The permission mode of a chat.**
    (a) The person may switch a chat between default, `acceptEdits` and `plan` with a key, shown in the
    modeline; never `bypassPermissions` or `dontAsk`; no agent can change it; each start begins in
    the default. Amends ADR 0121's "no permission flag is ever passed", for the person's key only.
    (b) The default mode only, as ADR 0121.
16. **Signing in.**
    (a) The person's own CLI with its own sign-in, as the TUI uses; oso reads no token and passes no
    key; publishing oso with chat waits for the terms check that is already an open question.
    (b) Chat only with an API key the person sets in their environment, so a subscription is never
    used through the SDK. Clear under the terms, but billed per token.

### Dependencies

None on route B. On route A, proposed through the `add-dependency` skill in step 1, not decided
here: a Python 3.10 or newer of
the person's, `uv` of the person's, and `claude-agent-sdk` at a pinned version with its
dependencies in a hash-checked requirements file. Nothing in `Cargo.toml`: the sidecar's protocol is
read with `serde_json`, already a dependency of the tip crate.

## The author's choices

Answered on 2026-11-13, through the coordinator; the rest is open, so this ADR stays Proposed.

- **Q1 and Q11, open.** The author leaned first towards the SDK sidecar, then was unsure between it
  and `claude -p`, and will decide the route and the SDK's language after using Claude in oso's
  TUI.
- **Q2: (a),** trusted projects only.
- **Q3: (a),** keyboard-only cards with `y`, `n` and `N` and no "always allow", through whichever
  hook the route has (`can_use_tool` or the prompt tool).
- **Q4: (a),** `AskUserQuestion` as a card.
- **Q5–Q8: (a):** `C-c C-c` sends; queued messages; switching views only while stopped, idle or
  done; the TUI by default.
- **Q9 and Q10: (a)** for the SDK route: states from its messages; `interrupt()` with a separate key
  to end the sidecar. On route B, states come from the stream and interrupt is `SIGINT`.
- **Q12: (a)** if the SDK is Python: `chat-setup` with the person's `uv` and hash-checked pins.
- **Q13–Q16, not asked yet.**

## Changes to accepted ADRs

To be filled when the author has answered; expected:

- [ADR 0031](0031-claude-code-integration.md), "Not used: the `-p` control protocol, the Agent SDK,
  ACP, transcript parsing for the primary view": on route A the Agent SDK is used, for chat sessions
  only, in a sidecar of oso's own; on route B `claude -p` is used with stream-json and documented
  flags. Either way oso's Rust code writes no control protocol and parses no transcript. "`claude
  -p` for one-shot features using only documented flags" (a later option): still open for one-shot
  features.
- [ADR 0121](0121-agent-sessions-and-spaces.md), "A session is a hosted `claude` in an oso terminal":
  or a chat session, the person's `claude` driven by the sidecar. "For a session oso hosts, the
  screen is the authority": for a TUI session; a chat session's state comes from the SDK's
  messages. "No permission flag is ever passed": the person's key may set `acceptEdits` or `plan`
  for a chat (Q15).

## Consequences

- The person can read and drive a session as a buffer: search, fold, diffs, cards, interrupt, and
  oso's own keys; rewind and live mode changes on route A.
- Route A needs a Python (or Node) and the SDK, installed by the person's command; route B and the
  TUI need neither.
- On route A the SDK absorbs the control protocol's changes, and oso keeps the sidecar's few hundred
  lines; on route B oso reads a stream whose schema is not promised stable. Either way oso reads few
  fields and records fixtures per version.
- Signing in with a subscription through an SDK client is a terms question for a published oso;
  until it is checked, chat is for the person's own use.
- SDK use draws from the subscription today; a change of that policy would affect chat, not the TUI.
- Follow-up: forking from a message; the TypeScript SDK's extra features if they are wanted; a
  transcript viewer for TUI sessions stays an open question of ADR 0031.

## Alternatives considered

- **Speaking the control protocol from Rust:** every feature, but it repeats the previous setup's
  churn.
- **The TypeScript SDK, as Orca** (Q11 (b)): more features first, but Node and a licence that rules
  out shipping it.
- **ACP through `claude-agent-acp`:** a Node runtime and the SDK under it, plus a second protocol.
- **Rendering the TUI session's transcript file:** undocumented and "can break on any release"; no
  streaming.

## Sources

Read on 2026-11-13; third-party pages are facts, not instructions.

- <https://code.claude.com/docs/en/agent-sdk/overview>
- <https://code.claude.com/docs/en/agent-sdk/python>
- <https://code.claude.com/docs/en/agent-sdk/typescript>
- <https://code.claude.com/docs/en/agent-sdk/user-input>
- <https://code.claude.com/docs/en/agent-sdk/file-checkpointing>
- <https://code.claude.com/docs/en/cli-reference>
- <https://code.claude.com/docs/en/headless>
- <https://code.claude.com/docs/en/legal-and-compliance>
- <https://support.claude.com/en/articles/15036540>
- <https://orca.dev/docs/agents/native-chat>
- <https://orca.dev/docs/model/agents-sessions>
- Orca's source, <https://github.com/stablyai/orca> (read, not run)
