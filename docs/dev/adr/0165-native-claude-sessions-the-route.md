# 0165. Native Claude sessions: the route

- Status: Accepted, amended by 0170 and 0173
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
  JSON, and gives the CLI back the input it sent, unchanged, as `updatedInput` on an `allow`
  (but an `AskUserQuestion`'s, which takes the person's answers, part 2e-2, and an
  `ExitPlanMode`'s, which takes the person's edit of the plan and a mode after it, part 3d).
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
  to `AskUserQuestion` answers nothing. Part 2e-2 replaced the question with the card: see
  there.

### Part 2e-2: the permission card, questions and plans

The author's (a) on each of Q1 to Q4 of 2026-11-22, and on the earlier Q3 (point moves to the
card).

Measured on 2026-11-22 (CLI 2.1.330, `--model=haiku`, a stub prompt tool, made-up prompts):

- **`ExitPlanMode` goes through the prompt tool.** The stream's `tool_use` has an empty input;
  the request carries `{plan, planFilePath}`, the CLI having written the plan's file itself,
  unasked. Allowed, `system/status` says `permissionMode: default` and the result "User has
  approved your plan"; denied with a message, the result is that message, and the mode stays
  `plan`.
- **`AskUserQuestion`:** the request's input is the stream's. `updatedInput.answers` takes
  `{QUESTION: LABEL}`, and several labels joined by `, ` for a question that takes several
  ("Your questions have been answered: …").

As built:

- **The card** (`sessions::native::card` for its text, `app/sessions/hosting/native/permissions/card.rs`
  for the rest): `*card: NAME*`, an internal buffer in `Mode::Card`, read-only and withheld
  from agents, docked in `DockSlot::Card` between the session's transcript and its composer
  while a call of that session waits, and gone once answered. It is the only place the call
  is asked (Q1 (a)): part 2e-1's question in the echo area is gone, and elsewhere the session's
  `needs input` (`permission`, `question` or `plan`) leads there, as a terminal's dialog does.
  Each session's call has its card, as each holds one connection.
- **Claude's text, fenced:** the card's first row is oso's title and its last oso's keys, a
  plain row in oso's own face (not ADR 0075's chips, so that they stay the card's text);
  what Claude wrote (a command, a diff, a reason, a question and its options, a plan) is
  between them, each row of it after `│ ` in a face of its own. Each of Claude's line feeds
  starts a row of its own (another separator is shown as itself, and a command holding one
  says not all of it is shown), and a row longer than the card is broken into rows at the
  card's width, measured as the display draws it, the rows it goes on to fenced `┊ ` and a
  mark that would start one put on `◌`, drawn again when that width changes, the buffer never
  wrapping: no text of Claude's
  runs past the card's edge or wraps unfenced, and a row of the card is a row on screen. At
  most 10 rows are fenced, what is left out saying so (ADR 0170's rule). What the card shows
  of a call's target is what the stream says of it, the request saying the same; what only
  the request carries (a reason, an edit's strings, a question's text, a plan) is shown from
  it. A title says when the card does not show all the call does (`(not all of it is shown:
  RET shows it)` first, the title cut to the card): for a call
  that does more than read, a command with more than its description and limit or holding a
  control character, and an edit with more than its file and strings.
- **`RET`** shows the whole call, or the whole plan, in the card itself, its keys on its
  title row and at most 2000 fenced rows, scrolled as any buffer is, and goes back: a buffer
  of its own in the host's window would take the transcript, and so the card, out of it. What
  it shows is made the first time it is asked for.
- **What it says (Q4 (a)):** a command after `$ `, each of its lines a row; an `Edit`'s,
  `MultiEdit`'s or `Write`'s change as `-` and `+` lines in ADR 0122's faces, at most 200 of
  256 bytes each; the reason (Bash's `description`) after `Why:`.
- **Focus:** point moves to the card after 500 ms with no key, while the person is in that
  session's transcript, or its composer with no draft in it, and the card's window shows all
  of it. A key answers it (`y`, `n`, `N`, a digit, `o`, `RET`) only after 500 ms with no key
  and since the card became the selected buffer, however point came there (the automatic
  move, `C-x o`, the mouse, a prompt over it closing, longer after one an agent opened, or a
  new call filling it); never from a macro; never while its window shows less than all of it
  from its start, at the width it was drawn at, unwrapped, which says to make the window
  taller. No agent command runs while a card is selected, a look included, as a scroll or a
  wrap could hide what it asks. A held answer key
  answers once (ADR 0072). Its other keys, as `C-c C-o` (`native-card-leave`) or `C-x o` to
  leave it, are any buffer's. Answered, the card goes and point goes to the composer.
- **Keys:** `y` allows once (or approves a plan), `n` denies (declines the questions, or keeps
  planning), `N` denies with a note read in the minibuffer, which the CLI is told; the call's
  row then says ` · denied: NOTE`, or ` · denied: by the person`. An allowed call gains no
  mark: its row goes on to say how it ran (but an approved plan, whose row says how it was
  approved, part 3d). A note or an own answer that a yank of a secret
  entry turned secret is refused. The approved public items: `Mode::Card`,
  `NativeAction::{Allow, Deny, Note, Choose(u8), Other}` and `SessionRequest::CardText(String)`,
  which carries a note or an own answer from its prompt's command (`native-card-note-text`,
  `native-card-own-text`) to the host, as `session-rename-to` does.
- **`AskUserQuestion` (Q3 (a)):** one question at a time, its options numbered; a digit answers
  a question that takes one and moves on, and in one that takes several chooses or unchooses
  it, `RET` giving those chosen; `o` answers in the person's own words, and `n` declines. oso
  answers with `{"behavior": "allow", "answers": {…}}`, which the bridge puts in the call's
  `updatedInput`, and only those: answers the model put in its own input are taken out, and
  an `AskUserQuestion` whose questions cannot be read is denied unasked. Answers for any other
  tool change nothing. At most 4 questions of 9 options, each text cut.
- **`ExitPlanMode` (Q2 (a)):** the plan is shown as the request gives it, marked so, as only
  the request carries it: the request is the session's own `claude`'s, by ADR 0121's claim of
  its bridge, and the call is still found by its id and tool in the stream. `y` approves
  (and part 3d's `a`, `A` and `e`).


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
  its end, while oso waits to quit or restart, or as oso exits (a restart that waits, part 4b,
  lets it resume).
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

### Part 2f: composed sends

The author's (a) on each of part 2f's Q1 to Q4 of 2026-11-22, and (a) on agents' briefs, through
the coordinator; the escapes are step 2's Q5 (a). ADR 0121 ("Composed text" and "An agent's
brief"), ADR 0122 ("Pasted messages are escaped") and ADR 0163 ("A native session's question")
are amended in place for them.

Measured on 2026-11-22 (CLI 2.1.330, `--model=haiku`, `--tools=`, a made-up file outside the
directory): an `@PATH` puts the file in the context at the text's start and after a space, a
tab, a line feed, VT, FF, NBSP, U+3000, U+2028 or U+FEFF, which is what Rust's
`char::is_whitespace` takes and U+FEFF; not after `'`, `"`, `[`, `<`, `:`, `=`, `-`, `*`, `\`, a
letter, U+0001, NEL or U+200B. A message
starting with `/` runs a command (part 2d); one starting with a space does not.

- **Escapes** (step 2's Q5 (a), now ADR 0121's "Composed text, as amended", on both routes,
  part 2f's Q2 (a)): what oso composed gets a space before a leading `/`, and a `\` before an
  `@` at the start or after whitespace (`sessions::send::for_claude`), once, at the one
  delivery path, `paste_into_session`, a control or invisible character looked through; a
  leading `!` or `#` gets the space too. Put into a draft, a `\` also follows text that ends
  in whitespace before an `@` of the draft, so that nothing put in makes the person's `x@y`
  the start of a word, a kept image's `@PATH` (`sessions::send::reference`) included, which is
  otherwise not escaped, nor is what the person types.
- **A send to a native session** (Q1 (a)): `paste_into_session` and `send_to_session` put the
  escaped text into its composer at point, not sent, and `send_to_session` shows the session
  with point in the composer after it; the person sends it with `C-c C-c`. A stopped session's
  composer takes it, its send waiting as part 2a says. Nothing goes in that would make the
  composer hold more than 64 KiB, though a send may be 256 KiB: "The composer would hold over
  64 KiB: nothing was put in"; nor into one a yank turned secret, which sends nothing. `session-send`, `session-mention`, review comments, git
  drafts and orchestrator offers all go this way, and say "Put into NAME's composer: C-c C-c
  in its composer sends it".
- **A clipboard image** (Q3 (a)): `C-c C-y` in the composer, or the transcript, puts the kept
  file's `@PATH` there, plain text in the composer's own face; the session is shown only if
  the person is still in it when the image is kept, as a read and a save take a while.
- **A review's question** (Q4 (a)), the person having confirmed it in oso's prompt: sent at
  once, or queued behind the turn under way (`Queue::send_confirmed`, which leaves a queued
  message edited in the composer as it is, and a paused queue paused), escaped, the
  composer's draft untouched; the transcript shows in the review layout's slot as a terminal
  does. A stopped session is still refused, as part 2b had it: "NAME: it has stopped: C-c C-r
  first", the question kept in the prompt's history.
- **An agent's brief** (ADR 0121, "An agent's brief, as amended") is escaped once where the
  start takes it, so a native session's first message from an agent is escaped too; the
  person's own `*Brief*` goes as typed, and part 2d's `/clear` check sees only the person's.
- **Built with** `sessions::send::for_claude` and `reference` (pure, with unit tests for each
  separator measured, a paste's dropped controls, what is not escaped and the draft's boundary),
  `app/sessions/hosting/native/composer/composed.rs` (`compose_into`, `send_confirmed`), the
  delivery path in `app/sessions/watch/sending.rs` (`paste_into_session` over `Outgoing`,
  `Shown`), `Begin::brief_to_send`, and the e2e flow `sessions::native::composed`; the send
  and worktree tool flows check the escapes in a terminal paste and a brief.

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

## Step 3, as built

Step 3 is built in parts, one pull request each, from the author's answers of 2026-11-23,
through the space's orchestrator. Each part's choices are recorded here as it lands.

### Part 3a: the status line

The author's (a) on each of Q-A1 to Q-A3 and Q-C1 of 2026-11-23.

Measured on 2026-11-22 and 23 (CLI 2.1.330, `--model=haiku`, made-up prompts, empty
directories):

- The context `/context` counts is the last main-thread answer's `usage.input_tokens +
  cache_creation_input_tokens + cache_read_input_tokens` (26 905 against its "27k / 1m (3%)");
  the `result`'s `usage` sums every call of the turn, so it is not. The window is
  `result.modelUsage[MODEL].contextWindow`. `/context`'s own reply is `<synthetic>`, with no
  usage.
- Plan mode answers with Sonnet under `--model haiku`, and `system/init` still says haiku.
- `result.total_cost_usd` is the session's running total, and it carries over `--resume`: a
  resumed process's `get_usage`, before its first turn, gives the earlier processes' cost.
- `rate_limit_event` names the window that binds (`rateLimitType: seven_day`) with its use as
  a fraction (0.28); `get_usage`'s `rate_limits` gives `five_hour` and `seven_day` as percent
  (29), and some 30 other windows by names of the CLI's own, null here.
- `system/status` with `status: null` gives `permissionMode` when the mode changes.
- `initialize`'s `models` lists each model's `value` (an alias or an id), `resolvedModel` and
  `displayName`; `default` resolves to a model but names none (`Default (recommended)`).

As built:

- **The line** (`sessions::native::status`, the author's Q-A1 to Q-A3 and Q-C1 (a)): the
  modeline's note says, after the run's state and the queue's count, `MODEL · MODE · ctx N% ·
  $C · 7d 28%`, each part once known, joined by ` · `.
  - The model is the last main-thread, non-synthetic answer's, else `system/init`'s (Q-A3),
    shown by the name `initialize` lists for its id (an alias's, not `default`'s), else as its
    id less `claude-`; an effort level follows it once oso noted one (Q-C1; part 3c notes it).
  - The mode is the CLI's word for it, from `system/init` and `system/status`.
  - `ctx N%` is the context used (Q-A1), over the shown model's window, once a `result` gave
    it; when the `result`s named one model only, over its window, whatever its id.
  - The cost is the last `result`'s, `$0.82`, whole from $10 (`$12`); before one,
    `get_usage`'s, if more than nothing.
  - The window is the last `rate_limit_event`'s, `5h` or `7d`; before one, the more used of
    `get_usage`'s two. Other windows are not shown.
- **What reads as itself:** a part holds ASCII alone: a model's id letters, digits and
  `._:/@[]-`, a mode or an effort level letters, digits, `-` and `_`, a listed model's name
  printable ASCII with its words one space apart. One that holds anything else, or is longer
  than 256 bytes (not cut), is not shown, so that no control, invisible, bidi or look-alike
  character, nor a dot like the ` · ` the parts are joined with, gets in: a part could then
  pass for two, and in a narrow modeline keep a mode no more in view. A model or a mode the
  CLI gives that is not shown clears the one shown before, rather than leave it saying what
  the CLI no more says. A cost or a
  percentage that is not a number from 0 to a bound, or is -0, is none. At most 64 models and
  16 windows are kept.
- **`get_usage`** (`sessions::native::usage`, over the control layer, Q-A2 (a)): asked once a
  process, as soon as its `initialize` is answered with success and its stream is there to
  write it (the answer may come before the start thread says the process started), so that a
  resumed session shows its cost and window before its first turn; the stream keeps them
  current after. Its answer comes through the control layer, as `interrupt`'s does. Refused,
  or unanswered within 10 s, it is logged; unanswered, it is forgotten, and a later answer is
  none. One whose process ended waits for nothing, and has no deadline. Tier 1 never asks. Of
  its answer only `rate_limits.five_hour`, `rate_limits.seven_day` and
  `session.total_cost_usd` are read.
- **A narrow modeline** (oso-core's `editor/status.rs`): a native session's note takes at most
  96 cells, and its ` · ` parts drop whole from the right before its name would go under its
  least, the first part never (`editor/status/note.rs`). The host and oso-core must join and
  split by the same separator.
- The note is written again only when an event may change it, and a frame is asked for only
  when it changed while the session is shown.
- **Public API:** none.

### Part 3d, as built: editable plan review

The author's (a) on Q-D1 (where a plan is edited) and Q-D2 (approving with a mode).

Measured on 2026-11-23 (CLI 2.1.330, `--model=haiku`, an MCP `--permission-prompt-tool`, a
made-up prompt in an empty directory):

- **An edited plan is honoured:** an `allow` whose `updatedInput.plan` replaces Claude's makes
  the tool's result "User has approved your plan… ## Approved Plan (edited by user):" with the
  edit, its `tool_use_result` `planWasEdited: true`; the CLI writes the edit over its plan
  file, and the model carries out the edited plan.
- **A mode after it:** `updatedPermissions: [{type: "setMode", mode, destination: "session"}]`
  on that `allow` gives `system/status` `permissionMode: acceptEdits` (or `auto`) at the
  approval, and the next `init` says so: no control request, so it works on tier 1 too.
  Without it the CLI goes back to `default`, as part 2e-2 measured.

As built:

- **The card's keys (Q-D2 (a)):** on a plan's card `y` approves the plan (manual), `a` approves
  it with accept edits after it and `A` with auto, as Claude Code's own dialog offers; `e`
  edits it; `n`, `N` and `RET` are as before. They answer after the card's quiet wait, as
  every key of it does. The plan's row then says ` · approved`, ` · approved, edited`, or
  either with `, accept edits` or `, auto`, as it runs and once done.
- **Auto asks first (the author, after review):** `A` and the edit's `C-c C-A` ask "Approve
  NAME's plan … and leave it in auto mode …? (y or n)" before they approve, as `A` is the
  first letter of many a sentence typed where point came to the card after the quiet wait;
  `a` does not ask. The question is kept for the call it asked about (`plans.asked`), and its
  `y` (`native-plan-confirmed`, which runs only as a question's answer) sends
  `NativeAction::Confirmed`: the host approves only that call, if it still waits on its card,
  from the edit it was asked in, still in front and checked again. The `y` counts only after
  the card's quiet moment since the question opened and since the key before it: a question
  takes every other key without closing, so a sentence typed on ("Are you…") would answer
  it; one too soon approves nothing, and `A` asks again. The card's own checks (its quiet
  wait, the window showing all of it) are not run again on the `y`: the call is the one the
  person saw at `A`, bound by its id, whose input a repeated request may not change. A
  question closed otherwise is dropped after that event, so no later `y` approves.
- **The edit (Q-D1 (a)):** `*plan: NAME*`, the person's draft as a composer is: an editable
  buffer withheld from agents, whose changes the agent channel refuses, in `Mode::Plan`,
  which it keeps, highlighted as Markdown, with keys of its own. `e` makes it from the
  request's plan the first time for that call, and shows it in the window that carries the
  card, in place of the transcript, as `RET`'s `*result: NAME*` is shown; a later `e` for the
  same call shows it as it was left. `C-c C-c` there approves the plan as edited, manual
  after it, `C-c C-a` with accept edits and `C-c C-A` with auto (the author, after review),
  and goes to the composer; `C-c C-k` gives the edit up and goes back to the card,
  which waited all the while. A plan the request cut is not edited, as the edit would lose
  its end.
- **What goes back:** `protocol::permission` carries, on an `ExitPlanMode`'s `allow`, `plan`,
  the edit, only if it differs from Claude's, at most 64 KiB and never cut (`allow_plan`
  gives none past it), and `mode`, `acceptEdits` or `auto` (`PlanMode`). The bridge puts the
  edit in the call's `updatedInput` and the mode in `updatedPermissions` as a `setMode`; it
  reads them only for an `ExitPlanMode`, whose answer carrying either in any other shape
  denies, and ignores them for any other tool, so no other call, and no other mode, is ever
  sent. Setting the mode at any
  other moment is part 3b's.
- **No agent command** runs while the edit is the selected buffer, a look included, as for a
  card: cursors or a region an agent left would steer the person's typing into what they
  approve.
- **Refused:** `C-c C-c`, `C-c C-a` and `C-c C-A` from a macro, for a call that no longer waits or another call's
  edit, on an empty plan, on one over 64 KiB, and on one a yank of a secret entry turned
  secret; the card's `y`, `a` and `A` while an edit for its call differs from Claude's, as
  they would approve the plan the person changed. A restart is refused while an edit the
  person changed waits, as the composer's message refuses it.
- **The card closing otherwise** (denied, its call or session gone): an edit the person never
  changed goes with it; a changed one stays, stale, and the echo area says where, once. A
  later plan's `e` is refused until `C-c C-k` gave it up, so that no edit is lost unasked.
  Whether an edit was changed is its text against Claude's plan; the restart, which sees only
  the buffer, goes by whether it was modified.
- **The card's approvals in the edit:** `C-c C-a` and `C-c C-A` are the card's
  `native-card-approve-accept-edits` and `native-card-approve-auto`, which in `*plan: NAME*`
  approve the edit with that mode after it. They are `Person`, as every key of the card is.
- **Parsed as Markdown:** the edit is highlighted on the syntax threads, Claude's text among
  it, under part 2g's reasoning for replies: nothing the parse makes leaves the buffer, and a
  yank that makes it secret stops it (ADR 0111).
- **Public items** (approved by the author): `Mode::Plan`, `NativeAction::EditPlan`,
  `NativeAction::Approve(Approval)` and `enum Approval {Manual, AcceptEdits, Auto}`, and
  `NativeAction::Confirmed` for auto's `y`; `NativeAction::Cancel` is `C-c C-k` in the edit
  too.

### Part 3e, as built: the to-do strip

The author chose (a) on both of its questions (2026-11-23, through the space's orchestrator),
and approved the public item `NativeAction::Todo`. The options, as they were put:

- **Q-E1: the to-do tools are off by default on current models.** (a) oso sets nothing; the
  strip shows when the person turns the tools on in Claude Code's own settings, and the docs
  say how. (b) oso sets `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` for native sessions, which changes
  Claude's tools from the TUI's. (c) No strip in step 3.
- **Q-E2: the strip after a resume.** (a) Empty until the next to-do call. (b) Read the CLI's
  own task files under `~/.claude/tasks/`.

Measured on 2026-11-23 (CLI 2.1.330, `--model=haiku`, made-up prompts, empty directories):

- **No to-do tool by default:** `-p`'s default tools hold none on current models. Claude
  Code's documents give the task tools (`TaskCreate`, `TaskUpdate`, `TaskGet`, `TaskList`) by
  default only to some older models, and to the others with `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`
  or when `--tools` names them; `CLAUDE_CODE_ENABLE_TASKS=0` gives the older `TodoWrite`.
- **What the tools give back,** in the result message's `tool_use_result`:
  - `TaskCreate {subject, description, activeForm}`: `{task: {id, subject, description}}`.
  - `TaskUpdate {taskId, status | subject | …}`: `{success, taskId, updatedFields,
    statusChange: {from, to}}`. A new subject is only in the call's input
    (`updatedFields: ["subject"]`, no `statusChange`); a task deleted says `deleted` in both
    `updatedFields` and `statusChange.to`.
  - `TodoWrite {todos: [{content, status, activeForm}]}`, the whole list each time:
    `{oldTodos, newTodos}`.
  - The CLI also keeps its tasks in files of its own under `~/.claude/tasks/`.

As built:

- **oso sets no variable (Q-E1 (a)):** the strip shows once the person turns the tools on in
  Claude Code's own settings (`env`'s `CLAUDE_CODE_ENABLE_TODO_TOOLS`), which `ui.md` and the
  user's guide say; a native session's tools stay the TUI's.
- **The list** (`sessions::native::todo`): only the main thread's `TaskCreate`, `TaskUpdate`
  and `TodoWrite` calls count, each by what it gave back, matched with its call by id, and
  none that is an error or a `TaskUpdate` that did not succeed or names another task than its
  call. A `TodoWrite` replaces the list whole. At most 100 items, each text cut to 256 bytes
  as read (`wire::todo`, the only place that reads them), and at most 64 calls waiting for
  their result. A subagent's calls are not the session's list.
- **`*todo: NAME*`,** an internal buffer, read-only and withheld from agents, as the card is,
  docked in `DockSlot::Strip`, under the card and above the composer, while the list has an
  item. Folded it is one line, `☐ 2/5 ▶ ITEM`, the item in progress or else the first not
  started, and `C-c C-t list` at its right edge where there is room; `C-c C-t`
  (`native-session-todo`, `Person`, in the transcript and the composer) opens it to a row an
  item, `☑`, `▶` or `☐` and its text, at most 6 rows (with more, the 5 around the first not
  completed and `… N more`), and folds it back. It shows while an item is not completed, and
  once all are until the turn ends; then the list goes, and the strip with it, point going
  to the composer if it was there.
- **Claude's text, fenced (ADR 0170):** oso's marks and keys are in faces of oso's own, and
  each item's text, after them, in a face of its own, its control characters as spaces, cut
  to the strip's width with `…`, measured as the display draws it: none of it runs past the
  strip's edge, wraps, or stands where oso's keys are. The strip's width is its window's
  less the column a row keeps for the wrap mark. It is drawn again when its window's width
  changes.
- **After a resume (Q-E2 (a)):** a session resumed has no list until its next to-do call;
  oso never reads the CLI's task files. A restart of oso brings a stopped session back with
  none too. A tier-1 interrupt, which ends the process and resumes it, keeps the list, as the
  person's turn goes on and the CLI keeps its tasks; a tier-2 one keeps the process.
- **Built with** `sessions::native::wire::todo` and `sessions::native::todo` (pure, with unit
  tests for each shape measured, the main thread only, the limits, the turn's end and the
  strip's text at narrow widths and with marks that would join oso's text),
  `app/sessions/hosting/native/todo.rs`, and the e2e flow `sessions::native::todo`.
- **A mark at either end of an item's text** (a combining mark or a joiner first, a prepend
  last) would join oso's space before it or its cell after it into one cluster, so that the
  text measured alone would be narrower than drawn: such a mark goes on `◌`, as on the card;
  and a cut that leaves a prepend last drops it, so that oso's `…` is a cell of its own. Every
  row is at most the strip's width.
- **Left as it is:** the strip's code beside the card's (its width, buffer and faces) is a
  copy of the card's with its slot; one shared dock helper waits until step 3's parts are in.
  The strip's window shows in the agent channel's state as a docked window of its rows,
  folded or opened, as the card's does (ADR 0170), not its text. `C-c C-t` works in the
  transcript and the composer, not in the strip itself, whose buffer has no keymap of its
  own.

### Part 3f, as built: prompt suggestions and the recap

The author's (a) on Q-F1 and Q-F2 of the step 3 plan, and the recap as below (2026-11-23),
through the coordinator; the author approved the one public item, `Editor::echo_withheld_text`.
No `NativeAction` was added: `M-n` stays `History`.

Measured on 2026-11-23 (CLI 2.1.330, `--model=haiku`, `--tools=`, made-up prompts, an empty
directory): with `--prompt-suggestions=true` (or the bare flag), the CLI writes one
`{"type": "prompt_suggestion", "suggestion": TEXT, "uuid", "session_id"}` 0.7 to 1.3 s after a
turn's `result`, with no line between them, but none after a conversation's first turn (three
runs). It is a small model call, billed into the next `result`'s
`model_usage_since_last_result`, and the streaming docs do not name it. The CLI's own "while
you were away" summary is the TUI's: nothing in `-p` sends it, so the recap is oso's.

- **The suggestion (Q-F1 (a)):** every native session starts with `--prompt-suggestions=true`
  (`sessions::command::NATIVE`). `wire` reads the line as `Event::Suggestion`, cut to 256
  bytes; a blank one is not one. It is kept while no turn runs: one that comes after a turn
  started, a queued message's say, is old. Only one that `*Messages*` would show as it is is
  kept (`sessions::native::suggest::suggestion`): a line break, a control, bidi or invisible
  character, or a mark that draws as nothing after its base, such as a variation selector,
  makes it none, so that what goes into the composer is the line its row showed,
  never a line hidden below it. It shows dim at the transcript's end, after the queue's rows,
  as `Suggested: TEXT`, and goes when a turn starts (oso's send, or the CLI's replay or
  request), at each send and when the process ends; the next turn's replaces it.
- **`M-n` takes it (Q-F2 (a)):** past the newest message sent, that is with no message of the
  history shown, in an empty composer that is still one and that no yank turned secret, as
  `compose_into` asks, `M-n` puts the suggestion in, not sent; `native-composer-next`'s
  description says so. It is Claude's text, which what Claude read can steer, put in by oso:
  so it is composed text, escaped as part 2f has it (`send::for_claude`): a leading `/`, `!`
  or `#` gets a space and an `@` that starts a word a `\`, which the person sees in the
  composer. A suggested command or file to attach then runs only if the person takes the
  escape out. The security review agreed, with the line breaks refused as above.
- **The recap:** a turn whose `result` comes while the person is not in front of the session
  (its transcript or its composer in the selected window of a focused oso window, no prompt
  open) is marked, and a turn that starts takes the mark away. After each event, a marked
  session in front of the person so, however they came back to it, is recapped once the echo
  area is empty, so that what oso said as they came back stays until the next key: it says `NAME (last reply): SENTENCE`, the first sentence of the turn's last reply, the session's own and
  not a subagent's, escaped and cut to the echo area's cells with `…`, once. The last reply is
  looked for back to the person's last message or the turn before's end, in the last 10000
  rows, a reply with nothing in its first 1 KiB passed over. The first sentence ends at the
  first `.`, `!` or `?` that whitespace or the end follows, at `。`, `！` or `？`, or at the end
  of the reply's first line with text, a heading's, list item's or quote's mark left out,
  looked for in the first 1 KiB. It is said with `Editor::echo_withheld_text`: no log line,
  not in `*Messages*`, and left out of what the control socket and the agent channel read (the
  channel's `echo`, and the dev socket's modeline beside it), as the transcript it quotes is
  withheld from agents. The `(last reply)` marker, the author's choice after the security
  review, keeps it from passing for a message of oso's own, as `proj: Saved.` would.
- **Costs:** a suggestion is a copy of at most 256 bytes and one redraw of the transcript's
  end, once a turn, with a frame asked for only if a window shows the session; the recap walks
  back over at most 10000 rows once, when the session comes in front, and after each event
  only the marked sessions are looked at.
- **Built with:** `sessions::native::suggest` and `sessions::native::recap` (pure, with tests
  of the line, which suggestion is kept, the last reply, the first sentence and the recap's
  cut); `app/sessions/hosting/native/suggesting.rs` (the events and `M-n`'s take) and
  `native/recap.rs` (the mark, and the recap after each event, from `sync_native_sessions`),
  with tests of what keeps and clears each; the queue's drawing in `composer/queued.rs`; a
  test that the agent channel does not read an echo withheld so; and the e2e flow
  `sessions::native::suggest`.

## Step 4, as built

Step 4 is built in parts, one pull request each, from the author's answers of 2026-11-23
(Q4-1 to Q4-11, the recommended option for each), through the space's orchestrator. Each
part's choices are recorded here as it lands.

### Part 4b: `oso-restart` waits for native turns

The author's (a) on Q4-1: the restart waits without asking, and `C-g` cancels it.

- **Before this part,** `oso-restart` never waited for a native session: its exit ended every
  native process group with `SIGKILL`, a turn under way among them, and the new instance
  brought each session back stopped.
- **What holds it.** A native session whose process runs or is starting holds it while it is
  starting, is being interrupted (step 2c), has a turn under way by its stream, or is found by
  the sessions' worker working or asking the person, as a permission, question or plan card
  does (ADR 0133's busy, `Editor::busy_native_sessions`), in
  `app/sessions/hosting/native/holds.rs`. A stopped or failed session holds nothing. Any other
  session being started, or held for the configuration's first look (ADR 0173), holds it too,
  as it does an exit that waits. So does a git run, which the restart would end: `oso-restart`
  asks first while git runs (ADR 0127, Q7), but a run that starts while the restart waits would
  otherwise be ended with no question. A dired file operation (ADR 0134) holds it too, as the
  restart would end it part way. Terminals do not hold it: they are carried, their programs
  running on. While a file operation runs, `C-g` stops it first, and a second `C-g` cancels
  the restart. After `oso-restart`'s question about a git run, `y` now waits for that run.
- **The wait** (`app/restart/wait.rs`). `oso-restart` first checks, as before, that this
  instance may hand over; refused, it says why and waits for nothing. Then, while anything
  holds it, the restart waits with no question: `Editor::set_restart_when_idle` leaves a flag on
  the editor, and the echo area says "oso-restart waits for *claude: NAME* to be idle; C-g
  cancels", naming the native sessions, git's buffers and the listing of a file operation up
  to three and counting the rest,
  and "a session starting" for another session. Before the event loop waits, the host looks,
  with nothing allocated: once nothing has held the restart for 2 s straight (ADR 0133's
  `IdleClock` and hold), the restart begins as it would have, and its checks run again. A
  prompt open, text being composed, or a search, undo, jump or keyboard macro under way, each
  of which would refuse it and ends by itself, puts it off by another whole hold. Any other
  refusal then, as a buffer that may hold secrets shown or a queued message, ends the wait and
  says why. With nothing to wait for, it begins at once, as before. Unix only, where
  `oso-restart` runs.
- **Cancelling.** `C-g`, where oso has the keys (a native session's buffers among them), says
  "Restart when idle cancelled". `C-x C-c` cancels it, and so does `quit-when-idle`, as the exit
  replaces the restart; a restart that waits or begins in turn replaces an exit that waits,
  which would end what it carries, and the exit's guard forgets what was agreed to. Only one of
  the two waits at a time. `C-x C-c`'s question does not name a native session's turn (it reads
  terminals only, `busy_terminals`), so with nothing else at risk it exits at once; that gap is
  older than this part, and its fix is its own change.
- **Meanwhile,** a native session's queue sends nothing on its own, as for an exit that waits,
  and neither while the restart is under way (`Turn::leaving`), since each message would start
  a turn the restart waits for again. The person's own send goes, and starts the hold again. A
  queued message still refuses the restart once it would begin (part 2a's rule). A session
  interrupted with a signal resumes as it would, and holds the restart while it starts: part
  2c's "Nothing resumes … while oso waits to quit or restart" holds for an exit that waits and
  a restart under way, not a restart that waits, so that a session that ran is still running
  when the restart carries it.
- **After the wait,** the restart is the one of before: the instance going still ends each
  native process, now idle, whose conversation Claude Code has kept (a `SIGKILL` after a
  `result` loses nothing, measured 2026-11-23), and the new instance brings it back stopped.
  Ending them gently and resuming them in the new instance is part 4c's.
- **New public items:** `oso-core`'s `Editor::{set_restart_when_idle, restart_when_idle}`,
  approved by the author through the orchestrator.
- **Tests.** What holds the restart, and that an ended session holds nothing
  (`native::holds`); the echo's names and count, and the hold with something held, nothing
  held and a prompt open when it is due (`restart::wait`); `C-g`, `C-x C-c` and
  `quit-when-idle` cancelling the wait (`commands::session`), and `C-g` in a native session's
  buffer by keys (`dispatch`). End to end (`sessions::native::restart_wait`): a fake whose
  turn waits for a release file holds `oso-restart`, which starts no instance while it waits;
  once released, the new instance takes over; and `C-g` cancels a wait.
- **Not done:** a mark while the restart waits, other than the echo said once (an agent's
  session ending its turn chooses the moment within the hold, and an agent's starts keep it
  waiting; the person cancels); a test of the
  app's own wiring (the queue held, a refusal once due) beside the end-to-end flows, as the app
  has no harness of its own.

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
- [ADR 0133](0133-keeping-terminals-over-an-update.md), step 2, as built: "The host refuses
  as `check-handover` with terminals does …, then stops the agent channel and starts the
  `oso-restart` thread": while a native session has a turn under way, asks the person, is
  starting or is being interrupted, the host waits first, with no question, until they have
  been idle for 2 s, and `C-g` or `C-x C-c` cancels the wait (part 4b). Any session starting,
  ADR 0173's held starts, git's runs and a file operation hold it too; terminals still do
  not.
- [ADR 0133](0133-keeping-terminals-over-an-update.md), step 5, as built, "Waiting.": `w` and
  `quit-when-idle` also cancel a restart that waits, and `oso-restart`, waiting or beginning,
  cancels an exit that waits and resets its guard, so that only one of the two waits (part
  4b).
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
