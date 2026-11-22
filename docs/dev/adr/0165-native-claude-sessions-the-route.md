# 0165. Native Claude sessions: the route

- Status: Accepted, amended by 0170
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

## Step 1, as built

The author answered step 1's questions on 2026-11-21, through the space's orchestrator:

- A session's first message is its brief, or for `session-new` a one-line prompt; nothing more
  is sent until step 2's composer.
- New starts, resumes and forks in trusted projects run native under `[sessions] mode =
  "native"`; adopting, `move_to_space`'s resume and carried sessions stay TUI sessions until
  step 4.
- A call that would ask is denied by the CLI's manual mode, the person's own allow rules still
  applying, and shows as a denied row.
- An agent reads a native session's buffer only after the person allowed it, as a terminal's.
- A tool result is as ADR 0130 has it: one folded row, `TAB` unfolds it, past 200 lines its
  first and last 50, `RET` shows it whole.
- A resumed session's buffer starts with one line; its earlier turns are step 5's.
- Replies are plain text with the rows' faces in part 1b; Markdown highlighting moved to
  step 2, part 2g.

Part 1a, the pure part (`sessions::native`), with the security, performance and architecture
reviews' findings:

- **One module owns the wire, not one file** (the author's choice): `wire` reads the stream's
  lines, with `wire/messages` for the messages' bodies and `wire/tools` for a tool call's
  input, which is read into a typed value. Nothing past `wire` touches JSON. Tier 2's requests
  go to `wire/control` in step 2.
- **Rows:** consecutive `Read`, `Grep` and `Glob` calls share a row (`Read 3 files`), as
  [`native-sessions-ux.md`](../native-sessions-ux.md) decided, at most 50, each call folding
  on its own; never a call that names a path outside the session's directory or one that may
  hold secrets (ADR 0044), which keeps a row of its own (the author's choice).
- **The person's messages:** a replayed message is shown as the person's only when oso sent its
  text; any other replay is a note, or nothing when a command or a hook wrapped it.
- **Cost:** a reply that grows is drawn again from its last line only; a result shows at most
  64 KiB, walking only what it shows; a diff keeps at most 1 MiB; a folded row that shows
  nothing new reports no change.
- **States:** a replayed message or a request is `Working`, a turn's `result` is idle; an
  error result shows in its row, and `failed` stays the exit's. ADR 0121's tracker holds a new
  session's screen back for a grace period and debounces idleness; part 1b adds an entry for a
  stream, which needs neither, so the tracker is reused with that entry, not unchanged.
- **Fixtures:** a four-turn session recorded from CLI 2.1.330 in an empty directory with
  `--safe-mode`, scrubbed of ids, paths, signatures, the account's values and the setup's
  lists; a proptest target checks that a buffer kept by the changes alone holds what rendering
  every row gives.

Part 1b-1, the editor's part:

- **`Mode::NativeSession`**, shown as `Session` in the modeline: an internal buffer whose text
  and highlights the host writes, read-only to keys, withheld from agents until the person
  allows them, and listed and killed as a person's buffer is.
- **One row at a time:** `Editor::replace_native_rows` puts a row's text and runs in place of a
  byte range, at the cost of the change; the rest keeps its highlights, and a cursor at the end
  follows new rows.
- **A terminal's session flags** (hosting a session, running, a turn under way, agents' reads)
  apply to a native session's buffer too, so ADR 0132's keys, ADR 0133's quit-wait and the
  agents' read gate take it; carrying over a restart still takes terminals only.

Part 1b-2a, the session itself:

- **The process:** `[sessions] mode = "native"` starts a new session in a trusted project as
  `claude -p` with [`command::NATIVE`]'s flags and no brief on its line, on the start thread,
  through `launch` and `process::spawn`, with a terminal session's variables, pipes and a
  session of its own. Its first message goes on its standard input. A person's start with no
  brief opens `*Brief*` for it (multi-line, `C-c C-c` starts, `C-c C-k` drops), not a one-line
  prompt: a prompt the host opens has no way back to it. An agent's start with no brief, and a
  restricted project, keep the terminal.
- **Reading:** `sessions::stream` reads standard output in lines of at most 16 MiB on
  `oso-native-read`, which parses them, and hands batches to the UI thread at most 30 times a
  second from `oso-native-batch`; a full queue holds the reader, and so the process, back. The
  last 16 KiB of standard error are kept, never logged, and the end of them is shown when the
  process fails.
- **Drawing and states:** the UI thread folds each batch into the session's rows and draws each
  row that changed once, where it is. What the events say goes to the worker as the tracker's
  targets; a native session's tracker has no grace period.
- **Its end:** the process stops (input closed and `SIGTERM` to its group at once, `SIGKILL`
  after 5 s) when its buffer is killed, which archives the session, or when the session list
  stops it; oso's exit and a restart kill its group. Once it ended, what is left of its group,
  such as a job its tools left in the background, is ended too before it is reaped, as a
  terminal's hang-up would. The bridge takes the stream's process as the session's `claude`
  until it is reaped.
- **The record** keeps whether a session is native; the next oso brings a stopped native
  session back as a native buffer with no process. Resuming it waits for step 2, and a fork of
  it runs in a terminal until then (R1 below). A send, a paste or `session-explain` to a native session
  says why not until step 2.
- **A brief is the person's:** the agent channel's changes are refused in `*Brief*`, a worktree
  session's or a native session's first message, as in a commit's message.
- **Not covered, residual:** if oso itself dies (a crash or `SIGKILL`), nothing signals the
  native `claude`: it sees its input end and finishes the turn under way, tools included.
  Linux's parent-death signal or a watchdog would cover it; left for a later step.

The author approved the process this step starts (REVIEW.md's gate on spawning): a long-lived
`claude -p` child with pipes, started through `launch` and `process::spawn` as ADR 0159's
naming run is, with no new spawn API and no new variables. `RET` on a long result opens it in a
read-only buffer withheld from agents, and `n`, `p`, `]` and `[` move between prompts and tool
rows from part 1b-2 on.

Part 1b-2b, the buffer's keys (B1 and B2 (a), as the author chose):

- **`TAB`** folds or unfolds the row at point: thinking, or a tool call with its input and
  result. In an open row of several calls, on a call's lines, it folds that call alone; on the
  row's own line, the row. Point goes to what it folded. Where each call of such a row is, the
  host keeps as it draws the row, so a key draws nothing to find it.
- **`RET`** shows the whole result of the call at point, or its whole change, read-only in
  `*result: NAME*`, one per session, internal and withheld from agents as `*Session explain*`
  is, since a result may hold what the session read. It is put in at the cost of the result's
  size (at most `wire`'s 1 MiB and its change's headers), on the person's key; the call it
  shows already is shown as it is. Its line is made one line as the row's is.
- **`n`/`p`** go to the start of the next or previous message of the person's, **`]`/`[`** of
  the next or previous tool row, with a count; back from a line of one past its first, to its
  start first. They go by the rows' kinds, never by their text.
- **Every key is `Person`:** the buffer is withheld, and the host does the work after the
  command, where the channel's guards no longer see it, so what a key folds or says would tell
  an agent of the rows.
- **Session mode's `C-c` keys** (ADR 0132, ADR 0143) are bound in a native session's buffer
  beside its own, but `C-c +` and `C-c -`, which let an agent read a terminal's screen; those
  that need a terminal say why. The keys are carried out by the host from the row ledger,
  through `SessionRequest::Native` and `NativeAction` (the author approved the public API's
  change).

Step 1's measurements (2026-11-22, CLI 2.1.330, `--model=haiku`, made-up prompts, the
person's own settings and hooks loaded, on the author's Mac):

- **Start to first event:** `system/init` arrives 0.24 to 0.57 s after the spawn, in three
  runs; the replay of the first message and the reply's first delta come together, at 1.1 to
  1.2 s, once the model answers; a one-word turn's `result` follows at 1.1 to 1.25 s.
- **Memory:** a native session is two processes holding about 300 MiB together (299.7 to
  306.6 MiB), the same after its turn as at its peak. The person's long-running TUI sessions on
  the same machine held 430 to 990 MiB each (mean 716 MiB), so a `-p` session costs no more
  than a TUI one; they were not started alike, so this is no like-for-like comparison.
- **`ExitPlanMode`** reaches a `--permission-prompt-tool` in `--permission-mode plan`: the tool
  is called with `tool_name: "ExitPlanMode"` and its input, the plan's text and its
  `planFilePath`, and a denial comes back to the model as a failed result and is listed in the
  turn's `permission_denials`. Step 2's plan review needs no stdio route: the bridge's
  permission method gets it as any other call. The plan file is written under
  `~/.claude/plans/` before the call, whatever the answer.
- **`SendMessage` reaches a `-p` session:** `system/init` carries `messaging_socket_path`, the
  session is listed to its peers (as `interactive`, as ADR 0151's check found), and a peer's
  message comes as a `user` line with `isReplay` and `isSynthetic`, then a turn of its own,
  between oso's sends. The rows show it as a note, not as the person's (part 1a); step 2's
  composer must expect turns it did not start.
- The stream also carried `system/session_title_changed` and `command_lifecycle`, which the
  fixtures lack; `wire` reads any unknown line as `Event::Other`, shown as nothing.

Resume, fork and a restart, as the author chose on 2026-11-22 (the plan for step 2):

- **R1 (c):** a native session's resume and fork wait for step 2, as a resumed session could
  take no message before the composer. Until then its resume refuses and says that
  `session-fork` continues it in a terminal, and a fork of it runs in a terminal. Step 1 ends
  with part 1b-2b.
- **R2 (a):** Q8's resume after an oso restart comes later still: until then a native session
  comes back stopped, and the person resumes it.
- **When it is built:** a session resumes as it ran, a native one natively and a TUI one in
  its terminal. A native resume reuses its buffer and keeps its rows, with one "Resumed" row;
  it takes no baseline, but its project's trust is looked at again before the spawn, apart
  from the baseline's root. In a project no longer trusted it refuses and points to
  `session-fork`. A fork follows the mode: native in a new buffer in a trusted project under
  `mode = "native"`, else in a terminal. New starts follow the mode, as now.

## Step 2, as built

Step 2 is built in parts, one pull request each; the author answered its questions on
2026-11-22, through the space's orchestrator. Each part's choices are recorded here as it
lands.

Measured on 2026-11-22 for step 2 (CLI 2.1.330, `--model=haiku`, made-up prompts, empty
directories):

- **The call comes before the question:** the stream's `assistant` line with a call's
  `tool_use` arrives about 20 ms before the CLI calls its `--permission-prompt-tool` with
  `{tool_name, input, tool_use_id}`, so the call can be found in the stream by its id.
- **The prompt tool is not the model's:** it is not in `system/init`'s tools, and the model's
  own search for it found nothing. `--safe-mode` leaves out `--mcp-config`'s servers, prompt
  tool included.

### Part 2e-1: the permission route

The author chose (a) on Q4: the sessions' own socket. The reviews' findings were fixed
before the pull request; the author approved the public API's two additions and the listening
check's change.

- **`<pid>-session.sock`,** `oso_platform::control::Socket::Session`: an instance's socket in
  release builds besides `<pid>-agent.sock` (ADR 0085's **Socket:**), which listens without the
  person's command, from a native session's start until none runs or starts, with the agent
  channel off or on. It is the agent channel's server with a policy of its own
  (`server::Policy::Sessions`): only a connection whose first request claims a running native
  session and that ADR 0121's claim finds to be that session's bridge, one per session, beside
  the bridge's connection to the channel (`sessions/hosted/claims.rs`, one check for both);
  never the person's question about a connection; and only `version` and `permission`, held
  until the person answers or the bridge goes. The agent channel refuses `permission`. The
  channel's slots, questions and limits are untouched, and stopping the channel stops no
  question. The architecture check that guards listening names its one start.
- **The bridge's `permission` tool:** a native session's `--mcp-config` starts `oso mcp --pid
  PID --permission`, and its line names `--permission-prompt-tool=mcp__oso__permission`. Only a
  bridge started with `--permission` lists the tool, so a terminal session's model never sees
  it. The bridge connects to the sessions' socket as MCP's `initialize` comes, before the
  session's first turn, so that a command the session runs, whose parent is the session's
  `claude` too, cannot take the slot first; a second claimant is refused, and the person told.
  The bridge sends the tool's name, the call's id and its input, if that is at most 64 KiB as
  JSON, and gives the CLI back the input it sent, unchanged, as `updatedInput` on an `allow`.
  Whatever fails, it answers `deny` with why.
- **What the person is asked:** the call as the session's own rows have it, found by its id
  and tool among the calls whose whole message came and that have no result, and only if the
  request's input reads to the same target. A request whose call does not show within 2 s, or
  whose call stops waiting before it is asked, is denied unasked, and a call is asked about
  once (`sessions::native::asks`). One question at a time, the oldest first, never while
  another agent question's answer is untaken: `Agent: Session NAME asks to run TOOL TARGET:
  allow it?`, with "(not all of it is shown: ? shows the call)" unless the call only reads, or
  is a command with nothing but its description and limit; `?` shows the input whole, escaped,
  in `*Session call*`, withheld from agents. It is the agent channel's kind of question,
  answered only from the keyboard and only after 500 ms with no key, and its label is withheld
  from agents' state (`Editor::ask_for_agent_withheld`); `y` allows, `n` or `C-g` denies. While
  it waits, the session is `needs input` with reason `permission`. The bridge going, the session
  ending or the socket stopping closes it and denies.
- **Left for part 2e-2:** the docked card with the call's preview and reason (U1), `N` with a
  note, `AskUserQuestion` and `ExitPlanMode`; until then they are asked as any call, and a `y`
  to `AskUserQuestion` answers nothing.

### Part 2a: the composer and the queue

The composer is a dock of the session's buffer ([ADR 0170](0170-docked-windows-under-a-sessions-transcript.md),
part 2a-1). The author chose (a) on Q1 and on each of Q1a to Q1e:

- **The composer,** `*compose: NAME*`, an editable buffer in `Composer` mode, withheld from
  agents, docked under the transcript as soon as the session's buffer is made. `i` in the
  transcript goes to it, `C-c C-o` back; `C-x o` steps through it. `M-p` and `M-n` walk the
  messages sent, at most the last 100.
- **Sending:** `C-c C-c` writes the composer's text as the person's message, at most 64 KiB,
  through the session's writer thread (`oso-native-write`, at most 16 lines waiting): a send
  never waits on the process, and a process that does not read refuses it.
- **The queue (Q1a–Q1e (a)):** oso writes to the session only while it is idle. A message sent
  while a turn runs, oso's or one it did not start, as a peer's, waits in oso's queue, at most
  20: grey rows at the transcript's end, under the last row, `Queued (N)` and each message's
  first line, and `N queued` in the modeline. One goes on each turn's end, the first, never
  joined. On a queued row, `RET` takes it back into the composer, above what is there; `e` edits
  it in the composer in its place, `C-c C-c` putting it back and `C-c C-k` giving the edit up;
  `d` drops it unasked; `M-<up>` and `M-<down>` move it, point with it. `C-c C-q` goes to the
  queue. `e` refuses while the composer holds text, which it would replace. An edited message
  keeps the send's limits (not empty, at most 64 KiB), and the queue's first, while edited,
  waits: the next goes once the edit is put back or given up.
- **Pausing:** the queue pauses when a delivery fails, the person's or the queue's (the
  queue's goes back first), when the process ends, and from part 2c on after an interrupt:
  nothing queued goes until the person sends again, or, while the process runs, sends with an
  empty composer, which sends the next once the session is idle. A send while the process
  starts waits, unpaused, for its first turn's end; one after it ended is queued, and the queue
  stays paused, its heading saying the session stopped. Nothing queued goes while oso waits
  for the sessions to be idle to exit (`quit-when-idle`, ADR 0133), as it would start a turn
  the exit waits for; what the person sends then goes, and the exit's wait starts again. A
  refused send changes nothing. `C-u C-c C-c` (send now, interrupting) and `C-c C-k` as an
  interrupt came with part 2c.
- **The rules are one pure state machine,** `sessions::native::queue`, told whether a turn runs,
  whether the process starts, runs or ended, and whether oso waits to exit, and tested there;
  the app carries out what it comes to, and the queue goes from one place, after each event.
  The session's run state and modeline note (`starting`, `stopped`, `failed`, `N queued`) are
  `note`'s alone.
- **What is sent:** only from a buffer in `Composer` mode, which a brief and a composer keep
  (`Buffer::set_mode` refuses to change it), and never text a yank of a secret entry turned
  secret, as a review message (ADR 0122); a `*Brief*`'s first message too.
- **History:** `M-p` walks the messages the composer sent; the first message, written in
  `*Brief*`, is not among them.
- **A restart (the author's choice (b), 2026-11-22):** a restart ends a native session as
  before (R2 (a)); the composer is made again with it, empty. A restart is refused, naming the
  session and quoting nothing, only while its composer holds text or its queue holds messages,
  which it would lose: the person sends, clears or drops them. The restart's exit looks at the
  queues again, as they are not in the buffers' generation. Carrying a draft and a queue over
  a restart is a possible follow-up, not wanted now.
- **A turn oso did not start:** a message queued as a reply to the turn under way goes when the
  session is next idle, which may be after a peer's message made another turn: the person reads
  the transcript before it goes, as nothing marks that turn yet. Such a turn is known only from its first event: a
  queued message may go in the moment before it, accepted.
- **A killed composer** is docked again, empty, the next time the session's composer keys need
  it; the edit of a queued message it held is given up.
- **Left for later parts:** asking before a session's buffer with messages queued is killed or
  archived; resuming a stopped session to take its queue came with part 2b.

The author approved the public API's change: `Mode::Composer` and `NativeAction`'s composer
and queue variants.

### Part 2b: resume, fork and a start with no brief

R1 as built, with the author's Q2 (a) and their choices of 2026-11-22:

- **A resume** of a stopped native session (`session-resume`, and an agent's `session_resume`,
  ADR 0153) is native, in its own buffer: its rows stay, one "Resumed." row follows, no turn is
  under way, and, for the person's, the buffer is shown with its composer selected. It takes
  no baseline. A queue that ended stays paused, its heading back to "C-c C-c on an empty
  composer sends the next". A start the start thread refuses, as when the conversation runs
  elsewhere (`Report::Live`, which offers a fork), leaves the buffer stopped with an "It did
  not resume." row.
- **Trust, failing closed (the author's choice):** a native resume or fork runs only at the
  root of a project that an open buffer says is trusted now, and the start thread looks at it
  again before the spawn. Else a resume refuses and says why: the project is not open (open a
  file of it), not trusted now, or the session runs below its root; `session-fork` then
  continues it in a terminal. The trust a session started with is not used: it is not taken
  back when the person untrusts the project. **Follow-up:** judging trust from the trust list
  by the root's path and identity, on the start thread, so that a resume needs no open buffer.
- **A fork** follows the mode: native, in a new buffer with its composer selected, under
  `mode = "native"` at such a root; else in a terminal. It takes no baseline, as a terminal
  fork took none.
- **A start with no brief** (the author's (a) on the `*Brief*` dedupe): a person's new native
  start begins at once, with its composer selected, where the first message is written;
  `*Brief*` is no longer a native start's (the worktree flow keeps its own), and an agent's
  start with no brief still runs in a terminal. A message sent before the process runs waits
  for it.
- **Part 2a, amended:** with `*Brief*` gone from native starts, the first message is written
  in the composer, so it goes into `M-p`'s history, and the secret check is the composer's.
- **A session given no message:** Claude Code keeps no conversation for a `claude -p` that was
  written nothing, and `--resume` of it fails ("No conversation found", a probe of
  2026-11-22). A native session that ends, is killed or is left at oso's exit before it was
  given a message is forgotten, its line dropped from the record, when it ends cleanly with
  nothing in its composer or queue, when its buffer is killed, and at oso's exit. Ended any
  other way (a failure, or a draft or queue written), it stays, with its standard error and
  what was written, and its resume says it has no conversation.
- **The stream on a resume:** `claude -p --resume` with `stream-json` writes only the new
  turn's events (init, the replay of the new message, its reply, its result), never the earlier
  conversation (a probe of 2026-11-22): the rows kept are the transcript, and a resume costs
  the UI thread one row.
- **Baselines:** the start's order says whether to take one: a new session and an adopted one
  do (ADR 0122), a resume and a fork do not.
- **A review's question** (ADR 0163) to a stopped native orchestrator is refused for now, as a
  native session takes messages in its composer: part 2f's composed sends carry it.

### Part 2c: the control layer and the interrupt

Tier 2's first two requests, with tier 1 as their fallback, and the author's (a) on each of
Q1 to Q3 of 2026-11-22.

Measured on 2026-11-22 (CLI 2.1.330, `--model=haiku`, a made-up prompt, an empty directory):

- `initialize` is answered within 0.2 s, in one line of about 36 KB: `commands`, `agents`,
  `models`, `account`, `pid`, `current_permission_mode`, `session_state`, a `capabilities`
  array of its own and more.
- `interrupt` is answered `success` with `still_queued` even while no turn runs, so a success
  alone does not say a turn ended. During a turn it is followed by a non-replayed
  `[Request interrupted by user]` user message and the turn's `result`
  (`error_during_execution`, `aborted_streaming`); the process stays, and the next message is
  answered. Without an `initialize` first it works too.
- A request the CLI does not know is answered `error`, `error_code: unsupported_request`.
- `system/init` lists `interrupt_receipt_v1` among its capabilities.

As built:

- **The control layer** (`sessions::native::control`, over `wire::control`'s lines): whether
  the CLI speaks it, for which requests, the ids of oso's requests (`oso-N`) and which one an
  answer is to, a pure state machine at the caller's instants. `initialize` is written first,
  as the process starts, before the first message, through the same writer queue as the
  messages. Tier 2 is on once it is answered with success; `interrupt` then goes unless the
  last `system/init` left out `interrupt_receipt_v1`. Before the first `system/init`, which
  comes with the first turn's start, the answer is enough, as the request has its own
  fallback; the control layer starts with the start's order, as the answer may come before
  the start thread says the process started. An error, or no answer within 10 s, leaves the
  session on tier 1; a success that comes later still turns tier 2 on. Why tier 2
  is off is logged once a process, the CLI's words escaped. Of an answer only its id, its
  success and its error, cut to 256 bytes, are read; what `initialize` answers otherwise is
  left for part 2d. oso answers no `control_request` of the CLI's: it asks for none. Each
  later request (`set_permission_mode`, `get_usage`, `side_question`) is its own module over
  this one, as `interrupt` is.
- **Measured, the first reply:** writing `initialize` first was within the noise of the first
  reply's time (CLI 2.1.330, haiku, a made-up prompt, 7 runs each: median 3.3 s with it, 2.6 s
  without, ranges 2.1–3.7 s and 2.2–3.0 s); written after the first message, it was still
  answered first, in 0.3 to 0.6 s. The model's latency dominates, so it stays first, as the
  SDKs write it.
- **Tier 2** (`sessions::native::interrupt`): an interrupt is the `interrupt` request, and the
  process stays. With no answer within 2 s, an error, or the turn still under way 5 s after the
  answer, it falls back to tier 1, and why is logged.
- **Tier 1:** the process alone is sent `SIGINT` (`Child::interrupt`), not its group: the CLI
  ends its own tools, as it does for a TUI's `C-c`, which is a byte. It ends the turn, then the
  process (step 1's measurement); if it has not ended within 5 s, it is stopped as the session
  list stops it. Either way it is resumed at once with `--resume`, in its buffer, as part 2b's
  resume is, under the trust judged then; refused, it stays stopped with an "It did not
  resume" row. An "Interrupted with a signal" row says so, and its end is no failure; for a
  moment the session reads stopped, then starting, as a resume does. Nothing resumes when the
  person stopped it or is asked whether to, when the replacement of an orchestrator waits for
  its end, while oso waits to quit or restart, or as oso exits.
- **Windows has no tier 1:** with no `SIGINT`, the process would end at once and leave its
  tools running, so an interrupt the request does not carry is given up, and the person is
  told that `session-stop` stops the session.
- **Keys:** `C-c C-k` in the composer gives up the edit of a queued message, or else
  interrupts the turn under way; in the transcript, the author's Q2 (a), it is
  `native-session-interrupt`, where session mode leaves `C-c C-k` to a terminal a native
  session has none of. `C-u C-c C-c` sends the composer's text
  now: put first in the queue, it goes once the interrupted turn ends. With no turn to
  interrupt, or while the process starts, it is `C-c C-c` (Q3 (a)). One interrupt goes at a
  time; the modeline says `interrupting` until its turn ended.
- **The queue:** an interrupt pauses it until the person sends; after a send now, the queue
  goes on as after any send (Q1 (a)). A tier-1 resume keeps it as the interrupt left it.
- **Public API (approved):** `oso_platform::process::Signal::Interrupt` and
  `Child::interrupt`; on Windows, which has no `SIGINT`, the process ends at once, as
  `terminate` does there.

### Part 2d: slash commands

The author's (a) on each of Q1 to Q3 of 2026-11-22.

Measured on 2026-11-22 (CLI 2.1.330, `--model=haiku`, made-up prompts, an empty directory):

- `initialize`'s answer lists the commands, 71 here, each with `name`, `description` (up to
  1.4 KB, a skill's from its file), `argumentHint`, `aliases` (a plugin's skill is aliased
  `plugin:name`) and `builtin`. `system/init` lists their names only, and apart
  `terminal_slash_commands` (`doctor`, `color`, `focus`, `reload-plugins`).
- A listed command sent as text never comes back as the person's message, even with
  `--replay-user-messages`: a local one (`/model`, `/effort`, `/usage`, `/context`, `/color`)
  answers with a synthetic assistant message, a prompt one (`/init`, a skill, `/doctor`)
  replays only its wrapped `<command-name>`. A text whose first word is no command
  (`/tmp/x …`, `/notacommand`) is replayed and answered as a message.
- `/clear` in `-p` starts a conversation with a new session id, which the next `system/init`
  and `result` carry. A leading space, newline or U+FEFF, and another case (`/CLEAR`,
  `/Clear`), make it a message, which clears nothing; a no-break space after it ends its word,
  as other whitespace does (`/clear\u{a0}x` clears). `--resume` and `/compact` keep the
  session id. `/rename` also writes `system/session_title_changed`.

As built:

- **The list** (`sessions::native::slash`, over `wire::slash`): the last `initialize` answer
  of the session's processes; until a process gives one, as on tier 1, each `system/init`'s
  names. A new process keeps the old list until its own comes. At most 512 commands and 8
  aliases each, each hint and description cut to its first line and 256 bytes; a name or alias
  longer than 256 bytes, or holding a space, a control, an invisible or a bidi character, is
  dropped, as it could not be typed after a `/` or could pass for another's. It is the
  project's text as much as the CLI's, so it is shown as completion rows show any text, its
  controls and bidi characters escaped.
- **Completion (Q1 (a)):** `/` typed as an empty composer's first character, by the person or
  a macro's replay, opens a `Command:` minibuffer completion (`native-composer-command`), as
  `M-x`'s does: each row the command's name, then its argument hint and description, with
  `not built in` before a project's or a plugin's, once the list says whose each is, as a
  skill may take a built-in's name;
  orderless, `&` matching the annotation. `RET` puts `/name ` first in the composer, in place
  of a word after a leading `/` that is, or begins, a listed name, ready for its arguments;
  before the text otherwise, as before a path, point staying in it. `C-g` leaves the `/`.
  `C-c /` opens it from anywhere in the composer. With no command listed yet, `/` is only a
  `/`. The host lists the selected composer's session's rows, made once a version of its
  list, again only when that session or its list changed. The dispatcher opens the prompt
  after the person's `/`, as `review-layout` opens its menu, so no public item was needed.
- **A command sent:** a text runs a command only when it starts with `/` itself and its first
  word, ended by whitespace or U+FEFF as the CLI ends it, is a listed name or alias; its row is
  the person's at once, as no replay comes. A first message given before the list is known (a
  brief) shows no row of its own when it is a command.
- **`/clear` (Q2 (a)):** refused at each write, the queue's included, saying to start a new
  session instead, and left out of completion: oso's record would keep the old session id,
  and a resume or a fork would reach the conversation before the clear. Once the list says
  which commands are the CLI's own, a word that any listed `clear` of the CLI's answers to is
  refused, whatever is listed before it, and not a project's skill of such a name alone; until
  then, and with a CLI that marks none its own, its names (`/clear`, `/reset`, `/new`) are. A
  refused queued message goes back first, and the queue waits for the person to drop it. A
  brief that would clear, an agent's or a worktree's, is not written: it waits in the
  composer, saying why.
- **A conversation that moved anyway:** a `system/init` or `result` whose session id is not
  the one its process's first `system/init` gave adds a row saying so, and the session neither
  resumes after a signal (part 2c), nor by `session-resume`, nor forks, each saying why.
  **Follow-up:** following the new id, as Q2's (b) would.
- **A terminal's own commands (Q3 (a)):** left out of completion, and sent if typed; until
  the first `system/init` lists them, the measured four.

### Part 2g: Markdown in replies

The author chose (a) on Q8 and on each of the plan's questions (2026-11-22); the space's
orchestrator chose (a) on the last. The author accepted the exception to ADR 0111's Q2 below
(2026-11-22). No public API changes.

- **Once whole (Q8 (a)):** a reply is highlighted once its block closed, by its message or by
  the end of its turn (an interrupt's too), and a subagent's too; while it streams it is plain.
  Its text is copied, at most 64 KiB, and opened as a document no buffer owns
  ([ADR 0122](0122-agent-review.md), step 8) on ADR 0111's threads, in Markdown, under
  `SyntaxKey::Reply` and a key never used before, with the session buffer's verdict. Fences
  take their own language as in any Markdown buffer (ADR 0111, step 7). Only the report that
  says it is done counts, once.
- **On its row:** the row draws each line after a mark (`● `, or two spaces past the first,
  after its indent), an empty line's without the spaces at its end, so that no row ends in
  spaces, which the trailing whitespace aid would show (as for every row's lines); the report's runs are put past each line's mark, which keeps its face, in
  one walk over the lines and the runs (`sessions::native::markdown`), and laid over the row's
  bytes in the buffer's highlights (`Editor::set_internal_highlights`), O(log n) and a step a
  run. The rows' faces as highlights (`Face::highlight`, `Rendered::runs`) moved to
  `sessions::native::render` so that the pure part shares them. Wrapping is the window's and
  folding redraws other rows only: both leave the runs where they are. A row drawn again is
  plain: what was open for it is closed at once, and a whole reply drawn again is opened again;
  a report under a key it no longer waits under is dropped, and one whose runs do not cover the
  row's bytes changes nothing. Markdown's markers stay, in their face; hiding them waits for
  the display map.
- **Budgets:** a reply past 64 KiB (the author's choice, Q1), cut short, or whose runs, its
  marks and breaks among them, would pass 65536 stays plain. At most 4 replies are open at
  once, of every session, as many as hold a tree at once, and at most 256 wait, the newest
  first; the oldest past that stay plain. They share the schedule with a review's sides: a
  full schedule makes them wait a second. A reply whose row is on screen as it opens ranks as
  shown, the others after buffers no window shows; it keeps that rank until done. The threads'
  own limits (ADR 0111: the parse's time, a line past 16 KiB) bound the rest: a reply that
  fails stays plain and nothing is said (Q5 (a)), as a reply is read-only and a message for
  each would be noise.
- **Replies are parsed whatever they quote (an exception to ADR 0111, Q2):** a session
  buffer is never judged secret, as no file is its source, so a reply that quotes a file the
  model read, a secret one too, is parsed like any other. That is safe because nothing the
  parse makes leaves the buffer: the buffer is withheld from agents
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md)), the threads' messages and logs carry no text and a
  parse's time is not reported (ADR 0111), the document is closed once its report comes and
  nothing of it is saved, and the runs only colour text the person already sees. The verdict
  still goes with the text, so a buffer judged otherwise would not be opened.
- **`global-font-lock-mode` (Q2 (a)):** off closes what is open, and the editor drops every
  buffer's highlights, a session's faces among them. On again, a session's faces are laid
  again from its rows once a window shows it, a reply's as plain, in one change of its
  highlights, and then its newest whole replies are asked for, before the others, at most 256;
  a session started while it was off has its faces, and its replies are asked for at once.
  While it is off, rows drawn are still looked at, so a reply that streams then is highlighted
  once whole after it is on.
- **Measured** (release, the author's Mac, with throwaway tests):
  - 64 KiB of Markdown made of headings, emphasis, links, lists and a Rust fence every 150
    bytes is highlighted in 30 to 57 ms of the worker's time, in 2 or 3 slices, and gives
    22770 runs on its row; a 16384-run limit, the plan's, would have left such a reply plain,
    so it is 65536.
  - On the UI thread, at 59259 runs, the projection takes 0.31 ms, laying the runs over a
    10 MiB buffer's highlights 0.39 ms, and the text's copy 0.01 to 0.09 ms: under 1 ms, once
    a reply.
  - Laying a session's faces again, for 2000 rows, 1000 of them edits with a 20-line diff,
    in 631 KB, takes 4.8 ms, once, when a window shows it; laying them for every session at
    once would cost that for each, which waiting for a window avoids. An event with nothing
    to do takes 1.4 µs.

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
