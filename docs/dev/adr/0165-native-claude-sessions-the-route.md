# 0165. Native Claude sessions: the route

- Status: Accepted
- Date: 2026-11-21

## Context

On 2026-11-21 the author asked for oso's Claude sessions to stop running the `claude` TUI in a
terminal buffer, and to use oso's own buffers instead: a conversation buffer, a prompt buffer,
permission questions in oso, diffs in oso. This goes further than
[ADR 0130](0130-agent-chat.md) (Proposed), which put a chat view *beside* the TUI and left the
route open (its Q1 and Q11). This ADR compares the routes from an implementation point of view
and records the one the author chose.

Three accepted or proposed ADRs bear on it:

- [ADR 0031](0031-claude-code-integration.md) chose the TUI and excluded "the `-p` control
  protocol, the Agent SDK, ACP, transcript parsing for the primary view". Its reasons: the
  control protocol's wire format is not published, the author's Emacs client carried many
  version-specific workarounds, it felt slow (mostly on the editor side: single-threaded JSON,
  rendering and MCP tools on the UI thread, CLI restarts), and a billing change once aimed at the
  headless and SDK route.
- [ADR 0121](0121-agent-sessions-and-spaces.md) defines a session as a hosted `claude` in an oso
  terminal, judged by reading its screen.
- ADR 0130 designs the chat buffer, the composer, permission and question cards, two views and the
  single-writer rule. Most of its buffer design holds on any route and is reused here.

### The reference: the author's Emacs client

The author's Emacs package, emacs-claude-code ("ECC", public, GPL-3.0-or-later), runs
`claude -p` and renders the session in ordinary buffers. Read for this ADR at release 0.5.1. It
is not one route but the third one below: it starts

```
claude -p --input-format stream-json --output-format stream-json --verbose
  --permission-prompt-tool stdio --session-id ID | --resume ID [--fork-session]
  [--include-partial-messages] [--forward-subagent-text] [--replay-user-messages]
  [--prompt-suggestions] [--include-hook-events] [--model M] [--permission-mode MODE]
  [--effort E] [--autocompact N] [--mcp-config JSON] [--settings JSON]
```

and writes the control protocol itself. From its code, the control requests it sends are
`initialize`, `interrupt`, `set_permission_mode`, `get_usage`, `side_question`, `get_settings`
and `remote_control`, plus `control_cancel_request`. It answers `can_use_tool`. Everything else
it does is the documented stream, `claude` subcommands (`auth`, `plugin … --json`), files on
disk (`~/.claude/sessions`, `~/.claude/projects`, settings files) and Emacs itself.

### The routes

- **B-doc: `claude -p` with stream-json, documented flags only.** oso writes user messages and
  nothing else. This is ADR 0130's route B.
- **B-ctl: `claude -p` with stream-json, and oso writes a named subset of the control protocol.**
  This is ECC's route.
- **A: the Agent SDK in a sidecar,** TypeScript or Python, speaking a protocol of oso's own to
  oso. The SDK writes the control protocol. This is ADR 0130's route A.
- ACP through `claude-agent-acp` is route A with one more protocol and a third-party adapter
  (ADR 0130, Alternatives); it is not considered further.

### What was measured (2026-11-21, CLI 2.1.330)

Five probes with `--model haiku`, in an empty scratch directory, no project, no real keys; a few
US cents at most in all.

1. **One long-lived process takes many turns.** User messages on stdin, one `result` per turn;
   the process stays until stdin closes. `system/init` arrives with the first turn, not at start
   (ECC measured the same). The `initialize` control response carries `commands` (name,
   description), `models`, `account`, `current_permission_mode`, `pid` and more.
2. **The documented permission route works,** including `AskUserQuestion`. With
   `--permission-prompt-tool mcp__probe__ask` and a stub stdio MCP server, the tool was called
   with `{tool_name, input, tool_use_id}`. A text result holding
   `{"behavior":"deny","message":…}` denied the call (the message became the tool result, and the
   `result` listed it in `permission_denials`). For `AskUserQuestion`,
   `{"behavior":"allow","updatedInput":{…, "answers":{QUESTION: LABEL}}}` answered it ("Your
   questions have been answered: …"). This settles ADR 0130 Q4's "unverified". The MCP route
   receives no `permission_suggestions`, `display_name` or `description`; the stdio route does.
   `ExitPlanMode` through the MCP route was not tried.
3. **SIGINT ends the turn, then the process.** A `result` with `subtype: error_during_execution`,
   `is_error: true`, `terminal_reason: aborted_streaming` arrives at once; the process then exits
   with status 0, and a user message written after the interrupt is never answered. On B-doc,
   every interrupt is a respawn with `--resume`.
4. **The control protocol works as ECC uses it.** With `--permission-prompt-tool stdio`, a write
   produced `control_request` `can_use_tool` with `tool_name`, `display_name`, `input`,
   `description` and `permission_suggestions` (`addRules`, `addDirectories`); a
   `control_response` deny was honoured. `get_usage` answered with `rate_limits`,
   `rate_limits_available`, `session`, `subscription_type` and `behaviors`, before any prompt.
5. **A `-p` child registers itself.** `~/.claude/sessions/<pid>.json` appeared with
   `kind: "interactive"`, `entrypoint: "sdk-cli"`, the session id and a `messagingSocketPath`;
   `system/init` carries `messaging_socket_path`. So ADR 0121's fork check, ADR 0155's messaging
   name and the CLI's `SendMessage` inbox see it. Delivery of a `SendMessage` into a `-p` session
   was not tried.

Also seen in every turn: `system/status` (`requesting`), `rate_limit_event` with
`rate_limit_info` (`status`, `resetsAt`, `rateLimitType`, `unifiedWindows` with `utilization`),
and a `result` with `total_cost_usd`, `usage`, `modelUsage` (per model: `contextWindow`,
`maxOutputTokens`, `costUSD`), `permission_denials` and `terminal_reason`. `system/init` lists
`slash_commands`, `terminal_slash_commands`, `skills`, `plugins`, `mcp_servers`,
`permissionMode` and a versioned `capabilities` array (`interrupt_receipt_v1`,
`get_usage_detached_v1`, …).

From ECC's notes (CLI 2.1.261–2.1.286):

- **Slash commands work when sent as user text.** `/model NAME`, `/effort`, `/compact`,
  `/context`, `/rename`, `/color`, `/reload-plugins` and the person's skills and commands run;
  their answers are synthetic assistant messages (model `<synthetic>`, zero usage) or
  `system/local_command`. `/compact` reports `system/status` `compacting` and then
  `compact_result: success`, and a `compact_boundary`. What the TUI draws itself is not a command
  at all: `/btw` (the control request `side_question`), `/login` and `/logout` (`claude auth`),
  `/plugins` (`claude plugin … --json`), `/skills` (`get_settings`, settings files),
  `/permissions`, the `/usage` dialog (`get_usage`) and the `/resume` picker. A host provides
  those or goes without.
- **No lock on a session id.** Two processes resuming one id write one transcript, which branches.
  ADR 0130's single-writer rule is required.
- Hosting the TUI in a terminal for a while and taking the session back headless with `--resume`
  works (ECC's `ecc-tui-open`).

### What Anthropic documents

Read on 2026-11-21; sources at the end.

- **There is no Rust SDK.** The SDK overview names Python and TypeScript and, for other
  languages, running the CLI as a subprocess. Community Rust crates reimplement the protocol.
- **The SDK runs the Claude Code binary.** TypeScript bundles a native `claude` per platform as an
  optional dependency (`pathToClaudeCodeExecutable` points at another; "the SDK version tracks the
  bundled Claude Code version"); Python bundles one too (`cli_path` points at another). Both
  speak the control protocol over that binary's stdio: the same `claude -p` stream underneath.
- **The control protocol is not a public interface.** No page specifies it. The Python SDK's MIT
  source (`_internal/query.py`) and the TypeScript package's type declarations show it: requests
  `initialize`, `interrupt`, `set_permission_mode`, `set_model`, `rewind_files`, `mcp_status`,
  `get_context_usage`, `mcp_reconnect`, `mcp_toggle`, `stop_task`; from the CLI `can_use_tool`,
  `hook_callback`, `mcp_message`.
- **What the SDK adds over flags:** `canUseTool` with suggestions, hooks as code, streaming input,
  `interrupt()`, `setPermissionMode()`, `setModel()`, `rewindFiles()` with file checkpointing,
  `getContextUsage()`, `supportedCommands()`, `supportedModels()`, `accountInfo()`,
  `reloadSkills()`, `stopTask()`, in-process MCP tools (`createSdkMcpServer`, `tool()`),
  `forkSession` and `resumeSessionAt`. Python lacks some (`supportedCommands`, `supportedModels`).
  Every one of them is a control request a B-ctl host can send itself; in-process tools are what
  oso's `oso mcp` bridge already is.
- **Interrupt** is documented only as SIGINT (and the SDK's `interrupt()`).
- **`claude -p` runs the project's `.claude` hooks and `.mcp.json` servers "even in a folder you've
  never trusted"**, and skips the workspace trust dialog (`--help` says so too).
- **Since 2.1.302, `-p` starts in manual mode:** a call that needs approval is denied unless a
  PermissionRequest hook or `--permission-prompt-tool` approves it. A prompt tool cannot approve a
  tool marked `requiresUserInteraction`. `--permission-prompts none` denies everything that would
  ask.
- **Transcripts** are `~/.claude/projects/<slug>/<id>.jsonl`, in a format the docs call internal.
  `-p` and SDK sessions are left out of the TUI's picker and bare `--continue`, but resume by id.
- **Terms:** "Unless previously approved, Anthropic does not allow third party developers to offer
  claude.ai login or rate limits for their products, including agents built on the Claude Agent
  SDK." The legal page does not prevent "an end user from signing in to the unmodified Claude Code
  binary with their own Claude subscription" (ADR 0130). B-doc and B-ctl run the person's own
  binary; route A runs it under Anthropic's SDK, which the sentence names. Subscription usage is
  drawn on all three.
- **Licences:** the Python SDK is MIT; the TypeScript SDK is "all rights reserved" under the
  Commercial Terms, so oso could never ship it, only have the person install it.

### What the routes share

The bulk of a native session is the same on every route: reading the stream (assistant, user and
tool results, `stream_event` deltas, a dozen `system` subtypes, `result`, `rate_limit_event`) and
drawing it. The SDK hands its caller the same stream messages; it does not hide them. What the
SDK hides is the control half: about ten request types. So route A trades a sidecar for not
writing those ten, while oso still reads the same message shapes and still meets their changes.

## Decision

The author chose (a) on each question below on 2026-11-21, through the space's orchestrator.

### The route: B, documented first, with a small control layer (Q1)

- **A native session runs the person's own `claude -p`** with
  `--input-format stream-json --output-format stream-json --verbose --include-partial-messages
  --replay-user-messages`, oso's session id (`--session-id`, `--resume`, `--fork-session` as ADR
  0121 and ADR 0153 have them), ADR 0121's `--mcp-config` for the `oso mcp` bridge and its other
  flags. It is resolved through the login shell and started through `oso-platform::process`
  (ADR 0121's `launch`), with pipes, one long-lived process per session.
- **Tier 1 uses documented flags only, and is complete on its own.** Each gap has a documented
  answer: interrupt is SIGINT then a respawn with `--resume`; a permission-mode change is a
  respawn with `--permission-mode`; the model and effort are `/model` and `/effort` sent as text;
  usage is each turn's `rate_limit_event` and `result`; the commands are `system/init`'s.
- **Tier 2 is a named set of control requests, each behind one module, each optional.** The
  set: `initialize` (the command list before the first turn), `interrupt` (keeps the process),
  `set_permission_mode`, `get_usage`, `side_question` (`/btw`). `initialize` is sent first, as
  the process starts; each other one is sent only when the `initialize` answer or `system/init`
  (its `capabilities`, `claude_code_version`) says the CLI has it. Each has a timeout, and a
  failure or an unknown answer falls back to tier 1 with one log line. Nothing else of the
  protocol is written. This amends ADR 0130's "oso's Rust code never writes the control protocol".
- **Why not the SDK:** it adds a runtime the person must install (Python 3.10+ or Node), a second
  process per session, a pinned package with its own dependency tree outside `cargo deny` and
  `cargo audit`, and the terms sentence that names SDK agents. It removes only the control half,
  which tier 1 does without and tier 2 keeps to five requests with fallbacks. ADR 0130's sidecar
  design stays the documented alternative if tier 2's churn proves worse than expected.
- **Why the control layer is acceptable now** when ADR 0031 refused it: ADR 0031's slowness was
  Emacs's single thread, which oso does not have (reader threads, `serde_json`, ADR 0130's
  budgets); the protocol has two public reference clients to read changes from (the Python SDK's
  source, the TypeScript declarations) and a versioned `capabilities` array; and every request
  has a documented fallback, so a CLI that changes one degrades a feature, not the session.

### Permission questions (Q2)

- **Through the `oso mcp` bridge**, as `--permission-prompt-tool mcp__oso__permission`: documented,
  measured above, and the bridge already proves which session it belongs to (ADR 0121). The card is
  ADR 0130's: keyboard only, after 500 ms with no key, `y` allow once, `n` deny, `N` deny with a
  note; `AskUserQuestion` as a question card answered through `updatedInput.answers`; no agent can
  answer one. While it waits the session is `needs input` with reason `permission` or `question`.
- **The bridge's permission method works while the agent channel is off.** Today the bridge's tools
  fail until the person turns the channel on (ADR 0085); a native session cannot depend on that.
  The permission method is accepted only from a bridge whose proof names a session oso hosts, and
  only carries a question to the person; it is not a channel tool and gives no agent any reach.
  This amends ADR 0085 narrowly.
- The stdio route (`--permission-prompt-tool stdio`, `can_use_tool`) is the fallback if the bridge
  route meets a tool it cannot approve (`requiresUserInteraction`, possibly `ExitPlanMode`); it is
  measured in step 1 and decided there.

### The buffers

ADR 0130's chat buffer and composer, renamed: the session buffer is `*claude: NAME*` (the name a
hosted session's terminal has today, so ADR 0121's lists, keys and agent tools find it), with a
composer window under it. Transcript rows, folding, tool rows, edit diffs in ADR 0122's faces,
to-do checklists, result rows, defensive reading, sending with `C-c C-c`, queued messages, and the
rule that only the person writes to a session are ADR 0130's.

### One session, and the TUI as a hand-off (Q3)

- **A session has a mode, `native` or `tui`** (ADR 0130's `chat` renamed). New sessions are native
  once step 4 below lands; until then `[sessions] mode = "native"` opts in.
- **The TUI stays as a hand-off,** as ECC's `ecc-tui-open` does and ADR 0130's
  `session-switch-mode` describes: while a session is idle, stopped or done, oso ends the `-p`
  process, starts the TUI in a terminal with `--resume`, and takes the session back the same way.
  ADR 0121's screen rules stay for that terminal. Never two writers.

### Where it may run (Q5)

- **Only in trusted projects** ([ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)),
  as ADR 0130 Q2 (a): `-p` skips Claude Code's trust question and runs the project's hooks and MCP
  servers. In a restricted project a session is a TUI, which asks its own question.

### The permission mode (Q4)

- The person may start a session in, and switch it between, `default`, `acceptEdits`, `plan` and
  `auto`, shown in the modeline (`auto` is what the author uses in ECC). `bypassPermissions` and
  `dontAsk` are never set. No agent or channel method changes the mode. On tier 1 a switch is a
  respawn with `--resume`, only while idle; on tier 2 it is `set_permission_mode`. This amends ADR
  0121's "no permission flag is ever passed".

### Across an oso restart or update (Q8)

- An idle, stopped or done native session's `-p` process is ended before the restart, and the new
  oso starts it again with `--resume`, in the same buffer. A turn in progress or a waiting card
  delays the restart as ADR 0133's quit-wait does for a working session.
- Passing the pipes to the new oso (ADR 0133's fd passing) is not done; it may come later.

### Code

- A `native` submodule of ADR 0121's `sessions` module in the tip crate (ADR 0121 Q24: no new
  crate). Inside it, one file owns the wire: parsing stream lines and writing user messages and the
  tier 2 requests; nothing else touches JSON. A reader thread per session parses lines (at most
  16 MiB each) and sends the UI thread batches of finished rows, at most 30 a second while text
  streams. The parser is a pure function with a fuzz target and fixtures recorded from the CLI
  (scrubbed of ids and account fields), one set per tested version.
- ECC is a reference for behaviour and measured facts, not a source of code: it is GPL-3.0 and oso
  is MIT or Apache-2.0 (Q7).

## ECC's features on each route

"How on B" names tier 1 or tier 2 where they differ. Effort for oso: S (days), M (a week or two),
L (more), "have" (oso has it).

| ECC feature (module) | How ECC does it | B (`claude -p`) | A (SDK sidecar) | Effort |
|---|---|---|---|---|
| Transcript with streaming text (`render`, `dispatch`) | stream-json, `stream_event` deltas, one node per block | same | same messages through the sidecar | M |
| Prompt region, history, `C-c C-c` (`chat`, `prompt`) | Emacs buffer | composer (ADR 0130) | same | S |
| Queue while a turn runs (`queue`) | held in Emacs, one per turn | same, held in oso | same | S |
| Slash completion (`prompt-capf`) | `initialize` commands + `system/init` | t1: `system/init` (first turn from a cache); t2: `initialize` | `supportedCommands()` (TS only) | S |
| `@` file references, editor context (`context`) | host-side | same | same | S |
| Pasted images (`image`) | saved file, `@path` | same, or a base64 image block | same | S (have storage, ADR 0142) |
| Allow / deny / deny with a note (`perm`) | `can_use_tool` over stdio | t1: bridge prompt tool (measured) | `canUseTool` | M |
| Edit a proposal before allowing (`review`) | allow with `updatedInput` | same through the prompt tool | same | M |
| Permission suggestions, allow patterns (`perm`) | `permission_suggestions` echoed as `updatedPermissions` | none on the MCP route (ADR 0130 Q3: no "always allow" anyway); stdio route has them | `canUseTool` suggestions | — |
| `AskUserQuestion` buffer (`perm`) | allow with `answers` | same through the prompt tool (measured) | same | S |
| Plan review in an editable buffer (`plan`) | `ExitPlanMode` request; deny with feedback or allow plus `setMode` | t1: prompt tool if it reaches it (step 1), mode change by respawn; t2: `set_permission_mode` | `canUseTool` + `setPermissionMode` | M |
| Permission modes, `auto`, S-TAB (`chat`) | `--permission-mode`, `set_permission_mode` | t1: flag, respawn to switch; t2: request | `setPermissionMode` | S |
| Interrupt (`proc`) | `interrupt` request | t1: SIGINT, then respawn (measured); t2: request | `interrupt()` | S |
| `/model`, `/effort` (`proc`) | slash text, noted locally | same | `setModel()`, or slash text | S |
| Usage float (`usage`, posframe) | `get_usage`: plan windows, session cost | t1: `rate_limit_event` + `result` (the hit window only); t2: `get_usage` | not in the documented SDK API; `rate_limit` events | S |
| Context left (`hint`) | last usage ÷ the model's window | same (`modelUsage.contextWindow`) | `getContextUsage()` | S |
| `/btw` side question (`btw`, posframe) | `side_question` request | t1: a one-shot `claude -p --resume ID --fork-session --no-session-persistence --tools ""`; t2: request | TS `Query` has it (undocumented) | S–M |
| Prompt suggestions (`hint`) | `--prompt-suggestions` | same (documented) | option | S |
| Notifications: finished, waiting, exited; pulse (`notify`, `visual`) | host-side | oso attention (ADR 0121), fed by the stream | same | have |
| Sessions list with states, answer from anywhere (`sidebar`, `answer`) | host-side | `*Spaces*` (ADR 0136, 0140), fed by the stream | same | have, re-fed |
| Spaces and worktrees, worktree MCP tools (`space`, `worktree`) | tabs, git, MCP | ADR 0136, ADR 0121's session tools | same | have |
| Editor MCP tools: xref, imenu, diagnostics (`mcp`) | loopback HTTP MCP server | `oso mcp` (ADR 0085) | same, or in-process SDK tools | have |
| Review, ediff style, comments as one prompt, Claude's comments, tour (`review*`) | diff/ediff buffers, `review_*` MCP tools | ADR 0122; comments go as a user message, not a bracketed paste | same | have, delivery S |
| Jump to source from diffs, headings, paths (`visit`) | structured tool results | same (`tool_use_result`, `structuredPatch`) | same | M |
| Revert buffers after a tool edit, warn on unsaved (`sync`) | tool results name files | same, with ADR 0029 | same | S |
| Bash diffs (`bashEditDiff`) | a field of Bash results | same (stream field) | same | S |
| Subagents, tasks, tool heartbeats (`dispatch`) | `--forward-subagent-text`, `task_*`, `tool_progress` | same | same | M |
| Hook events (`dispatch`) | `--include-hook-events` | same | same, or hooks as code | S |
| Compaction (`dispatch`) | `/compact`, `status`, `compact_boundary` | same | same | S |
| Past sessions: read a recording, resume (`history`, `lifecycle`) | reads the transcript, `--resume` | same; adopt reads it already (ADR 0121) | same | M (render) |
| Search past conversations (`search`) | grep, then parse hit lines | ADR 0145 | same | have |
| Restore after a restart (`restore`) | a file of open sessions | ADR 0121 records; a live process: see Q8 | same | have / M |
| Hand-off to the TUI and back (`tui`) | stop, TUI with `--resume`, follow, `--resume` back | same, with oso's terminal (ADR 0120) | same | M |
| Restart in place (`lifecycle`) | `--resume` | same | `reinitialize()` or a new sidecar | S |
| Rename (`proc`) | `/rename` text | same; ADR 0155 | same | have |
| Skills, plugins, hooks, capabilities buffers (`skill`, `plugin`, `hooks`, `capabilities`) | `initialize`, `get_settings`, `claude plugin … --json`, settings files | t1: subcommands and files; t2: `initialize` | `reloadSkills()`, `supportedCommands()` | M (later) |
| `/login`, `/logout`, `/status` (`auth`) | `claude auth`, login in a terminal | same | same | S (later) |
| Remote Control (`proc`) | `remote_control` request | not proposed | unknown | — |
| Inline answer and rewrite (`inline`) | a fork session; `claude -p --json-schema` | same (ADR 0159 already runs `-p` one-shots) | same | M (later) |
| Markdown, tables, mermaid (`markdown`, `table`, `mermaid`) | Emacs-side renderers | oso's Markdown and highlighting (ADR 0111) | same | M |
| Unknown messages kept (`dispatch`) | an `unknown` node | one "unreadable event" row (ADR 0130) | same | S |

The author's ECC settings map to: `auto` mode → Q4; notify `pulse` → ADR 0121's attention blink;
usage in a float → tier 2 `get_usage` (tier 1 a smaller float); `/btw` in a float → tier 2
`side_question`; review style `ediff` → ADR 0122's two-pane review; MCP enabled → `oso mcp`;
prompt suggestions → `--prompt-suggestions`; completion at point → the composer's completion of
`/` and `@`.

## What oso reuses, and what changes

- **Session records** (ADR 0121): reused; a `mode` field is added.
- **State judging** (ADR 0121's screen rules, feed, engine, `session-explain`, recap from the
  screen, hold rules, the `› Message from` rule): not used for native sessions. The stream is the
  authority: `working` from a sent message until its `result`, `needs input` while a card waits,
  `failed` on an exit oso did not cause, `done`, `ready for review` and `idle` after a turn as ADR
  0121 has them. The `Tracker`, `State`, `turn_ended`, attention, the spinner and the `*Spaces*`
  rows are reused unchanged, fed from the stream. The screen rules stay for TUI hand-offs.
- **Spaces** (ADR 0136, 0140, 0160, 0161): reused; nothing in them reads the screen. Orchestrator
  messaging needs the CLI's inbox, which a `-p` child has (measured 5); delivery is checked in
  step 1.
- **Agent channel and `oso mcp`** (ADR 0085): reused; gains the permission method (Q2).
- **Review** (ADR 0122): the baseline is taken before spawn and does not depend on the TUI; reused.
  `turn_ended` is exact now. Comments are sent as a user message instead of a bracketed paste,
  which also removes the bracketed-paste refusal.
- **Terminal** (ADR 0120): no longer hosts native sessions; kept for hand-offs and shells.
- **`move_to_space`** (ADR 0151): it moves an outside TUI session and resumes it in oso; reused,
  the resume starts a native session. Its `kind == "interactive"` check also matches `-p`
  children (measured 5), so it must keep refusing sessions oso itself hosts by its existing
  checks.
- **`session_resume`, resume, fork, rename** (ADR 0153, 0155): argv only; reused. `/rename` is sent
  as text where the TUI took it typed.
- **Adopt** (ADR 0121): reads the registry and transcripts, not the screen; reused.
- **Conversation search** (ADR 0145): reads transcripts, which `-p` writes; reused.
- **Clipboard image** (ADR 0142): storage reused; delivery goes into the composer as `@PATH`, or as
  an image block.
- **Restart and update hand-off** (ADR 0120 step 11, ADR 0133): carries PTY fds and a terminal
  snapshot; it does not apply. See Q8.
- **`space-new` naming** (ADR 0159): already a `claude -p` one-shot through `launch` and
  `process::spawn` with pipes and a time limit; the template for the native child.

## Steps

One pull request each, after the author's answers. Nothing here changes TUI sessions until step 4.

1. **The stream, read-only.** The command line, the reader thread, the parser with its fuzz target
   and fixtures from the current CLI, and the transcript rows: messages, Markdown, thinking folded,
   tool rows, edit diffs, result rows. States from the stream into ADR 0121's tracker, so
   `*Spaces*` and attention work. Measure: start time to the first event, memory per session,
   `ExitPlanMode` through the prompt tool, `SendMessage` into a `-p` session. Opt-in with
   `[sessions] mode = "native"`, trusted projects only.
2. **The composer and permission cards: the first useful native session.** Sending, queued
   messages, interrupt (tier 2, tier 1 as its fallback), the bridge's permission method and its
   cards, `AskUserQuestion`, review comments and clipboard images as user messages, slash commands
   as text with completion.
3. **Modes, model and usage.** The permission mode with its key, `/model` and `/effort`, the
   context row, usage from `rate_limit_event` (and `get_usage` on tier 2), prompt suggestions, plan
   review.
4. **Hand-off and restart.** `session-switch-mode` to the TUI and back, the single-writer check,
   restart per Q8; then native becomes the default for new sessions.
5. **The rest,** each its own change: `/btw`, jump to source, subagent and task rows, past sessions
   rendered from their transcript, skills and plugins buffers, `/login`.

## Questions

The author chose (a) on each, on 2026-11-21. The options are kept as they were put.

1. **The route.**
   (a) `claude -p`, documented flags first, with a small control layer (`initialize`, `interrupt`,
   `set_permission_mode`, `get_usage`, `side_question`), each optional with a documented fallback.
   (b) `claude -p`, documented flags only: interrupt and mode changes respawn the process; no
   `/btw` as a request, a smaller usage view.
   (c) A Python SDK sidecar (ADR 0130's route A): every SDK feature; Python and a pinned SDK to
   install; the terms sentence names SDK agents.
   (d) A TypeScript SDK sidecar: the most features; Node, and a licence that rules out shipping it.
2. **Permission questions.**
   (a) Through the `oso mcp` bridge's documented prompt tool, with its permission method accepted
   while the agent channel is off (amends ADR 0085 narrowly); the stdio route only where step 1
   finds the bridge cannot approve a tool.
   (b) The stdio route (`can_use_tool`) always: richer requests, no bridge hop, but the control
   protocol carries every permission.
3. **The TUI's place.**
   (a) Native is the default once hand-off works; the TUI stays a hand-off for what native lacks.
   (b) Native only; no TUI sessions.
   (c) Both, chosen per session indefinitely (ADR 0130's two views).
4. **Permission modes.**
   (a) `default`, `acceptEdits`, `plan` and `auto`, by the person only, shown in the modeline;
   never `bypassPermissions` or `dontAsk`.
   (b) As ADR 0130 Q15: no `auto`.
   (c) The default mode only.
5. **Where native sessions run.**
   (a) Trusted projects only; a restricted project gets a TUI session (ADR 0130 Q2 (a)).
   (b) Anywhere, after a warning.
6. **The ADRs.**
   (a) On acceptance, this ADR supersedes ADR 0130 (keeping its buffer design by reference) and
   amends ADR 0031, ADR 0085 and ADR 0121.
   (b) Fold the route into ADR 0130 and keep this ADR as the comparison only.
7. **ECC's code.**
   (a) A reference for behaviour and measured facts only; oso's Rust is written fresh.
   (b) The author relicenses chosen ECC parts for oso.
8. **Native sessions across an oso restart or update.**
   (a) Idle sessions are stopped and resumed with `--resume` after the restart; a turn in progress
   delays the restart as ADR 0133's quit-wait does; passing the pipes over comes later if wanted.
   (b) Pass the pipe fds to the new oso from the start (ADR 0133's fd passing).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0031](0031-claude-code-integration.md), "**Not used:** the `-p` control protocol, the Agent
  SDK, ACP, transcript parsing for the primary view": a native session runs `claude -p` with
  stream-json, and writes the five control requests of "The route" with their fallbacks. The Agent
  SDK and ACP stay unused. "The conversation UI is Claude Code's own TUI, not a native oso view.
  Permission prompts and dialogs appear inside the TUI": only for a TUI session; a native session's
  conversation and questions are oso buffers.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "**Off by default.**" and "The channel answers
  nothing until the person agrees": except the bridge's `permission` tool, which oso answers for a
  bridge whose proof names a native session oso hosts, with the channel off and without the
  connection's question. It only carries the question to the person, and gives no reach. "**Tools:**
  `state`, `text`, `commands`, `look`, `change` and `open`": and `permission`, named only as a
  native session's `--permission-prompt-tool`.
- [ADR 0121](0121-agent-sessions-and-spaces.md), "A session is a hosted `claude` in an oso
  terminal": or a native session, `claude -p` in oso's buffers (this ADR). "For a session oso hosts,
  the screen is the authority": for a TUI session; a native session's state comes from its stream.
  "No permission flag is ever passed": a native session passes `--permission-mode` with the mode the
  person chose (Q4), and `--permission-prompt-tool`; a TUI session still passes none.
- [ADR 0130](0130-agent-chat.md): superseded. Its buffer design holds by reference
  ("The buffers"); its route, its sidecar and its two views do not.

## Consequences

- The person reads and drives a session as oso buffers: search, folding, diffs, cards, the review,
  oso's keys; no terminal emulation in the main path, and states that are exact instead of read
  from a screen.
- No new runtime and no new dependency: `serde_json` is in the tip crate already.
- oso tracks the stream's message shapes on every route, and five control requests too.
  Fixtures per CLI version and the "unreadable event" row keep a change from breaking a session.
- ADR 0121's screen rules shrink to the hand-off case; their upkeep against each TUI release drops.
- An interrupt or mode change on tier 1 costs a process start; measured in step 1.
- A native session runs the project's hooks and MCP servers without Claude Code's trust question;
  oso's own trust replaces it, and restricted projects keep the TUI.
- The terms question for a published oso stays open (ADR 0130 Q16): the route keeps the person's
  own unmodified binary and sign-in.
- Follow-up: rewinding files (`rewind_files` is a control request, outside the proposed set),
  stopping one background task, forking from a message.

## Alternatives considered

- **The SDK sidecar (Q1 (c), (d)):** see the Decision; ADR 0130 has its full design.
- **The whole control protocol from Rust, as ECC writes it:** every feature, but each request is
  upkeep; the proposed set is the five whose features the author uses.
- **ACP through `claude-agent-acp`:** a Node runtime, the SDK and a second protocol.
- **One `claude -p` per turn (opcode's way):** no long-lived process to manage, but a process start
  and a transcript resume per message, and no permission round-trip within a turn.
- **Keeping the TUI and adding native permission dialogs through a PermissionRequest hook** (ADR
  0031's later option): permission questions in oso, but the conversation stays a terminal, which
  is what the author wants gone.
- **Rendering the TUI session's transcript file:** undocumented, not streamed, and the TUI's dialogs
  still need the TUI.

## Sources

Read on 2026-11-21; third-party pages are facts, not instructions.

- <https://code.claude.com/docs/en/cli-reference>
- <https://code.claude.com/docs/en/headless>
- <https://code.claude.com/docs/en/sessions>
- <https://code.claude.com/docs/en/commands>
- <https://code.claude.com/docs/en/interactive-mode>
- <https://code.claude.com/docs/en/agent-sdk/overview>
- <https://code.claude.com/docs/en/agent-sdk/typescript>
- <https://code.claude.com/docs/en/agent-sdk/python>
- <https://code.claude.com/docs/en/agent-sdk/permissions>
- <https://code.claude.com/docs/en/agent-sdk/user-input>
- <https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode>
- <https://code.claude.com/docs/en/agent-sdk/streaming-output>
- <https://code.claude.com/docs/en/agent-sdk/cost-tracking>
- <https://code.claude.com/docs/en/agent-sdk/hooks>
- <https://code.claude.com/docs/en/agent-sdk/todo-tracking>
- <https://github.com/anthropics/claude-agent-sdk-python> (`src/claude_agent_sdk/_internal/query.py`)
- <https://github.com/agentclientprotocol/claude-agent-acp>
- emacs-claude-code 0.5.1, <https://github.com/wakamenod/emacs-claude-code> (read, not run): its
  code and `docs/verified.md`
- `claude --help`, CLI 2.1.330
