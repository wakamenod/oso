# 0165. Native Claude sessions: the route

- Status: Accepted, amended by 0170, 0173, 0176, 0180, 0181 and 0182; "Worktrees' resumes", "Following the end, as amended", "Remote Control is off, as amended" and part 3f's suggestion (2026-11-24)
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
  once step 4 below lands; until then `[sessions] mode = "native"` opts in. Since part 4e they
  are, and `[sessions] mode = "tui"` opts out.
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
  `auto` (as built, part 3b: a session starts in Claude Code's settings' mode, and `S-TAB`
  switches it), shown in the modeline (`auto` is what the author uses in ECC). `bypassPermissions` and
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
| Remote Control (`proc`) | `remote_control` request | not proposed; off in every native session ("Remote Control is off, as amended") | unknown | — |
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
  first and last 50, `RET` shows it whole. Since "The rows' look, as built", its first 12 lines.
- A resumed session's buffer starts with one line; its earlier turns are step 5's. Since part
  5a, a resume whose rows hold none of its conversation shows its earlier turns before it.
- Replies are plain text with the rows' faces in part 1b; Markdown highlighting moved to
  step 2, part 2g.

Part 1a, the pure part (`sessions::native`), with the security, performance and architecture
reviews' findings:

- **One module owns the wire, not one file** (the author's choice): `wire` reads the stream's
  lines, with `wire/messages` for the messages' bodies and `wire/tools` for a tool call's
  input, which is read into a typed value. Nothing past `wire` touches JSON. Tier 2's requests
  go to `wire/control` in step 2.
- **Rows:** consecutive `Read`, `Grep` and `Glob` calls share a row (`Read 3 files`; since "The rows'
  look, as built", `Read ×3`), as
  [`native-sessions-ux.md`](../native-sessions-ux.md) decided, at most 50, each call folding
  on its own; never a call that names a path outside the session's directory or one that may
  hold secrets (ADR 0044), which keeps a row of its own (the author's choice).
- **The person's messages:** a replayed message is shown as the person's only when oso sent its
  text; any other replay is a note, or nothing when a command or a hook wrapped it.
- **Cost:** a reply that grows is drawn again from its last line only; a result shows at most
  64 KiB (8 KiB since "The rows' look, as built"), walking only what it shows; a diff keeps at most 1 MiB; a folded row that shows
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
  agents' read gate take it; carrying over a restart still takes terminals only. As built, the
  host never marks a native buffer as hosting a session, so ADR 0132's session keys do not
  apply there, and the quit-wait and `C-x C-c`'s question saw its turn only once
  `Buffer::is_busy` read it by the buffer's kind (2026-11-23; ADR 0133, step 5, "Native
  sessions").

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
- **Amended 2026-11-24 (the author):** while a request waits in front of the person (its card,
  composer, transcript or plan's edit selected), the agent channel's `open` shows no file, and
  while any card waits an agent does not switch highlighting; a held key answers once, `RET` and
  a plan's chords among them.
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
  never waits on the process, and a process that does not read refuses it. While a
  dictation's fix runs, a send waits for it ([ADR 0175](0175-dictation-through-typewhisper.md),
  step 2).
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
  continues it in a terminal. The trust a session started with is not used: the
  resume is judged where it runs. **Follow-up:** judging trust from the trust list
  by the root's path and identity, on the start thread, so that a resume needs no open buffer.
  Built in part 4a, and, for a worktree, "Worktrees' resumes": an open buffer still decides
  where there is one.
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
  the UI thread one row. Rows that hold none of it, as in a new instance or a fork, are given
  its earlier turns from Claude Code's transcript since part 5a.
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

Measured on the TUI on 2026-11-23, for part 4d (CLI 2.1.330, haiku, `--tools=`, the TUI driven
only by writing to its pty): a bracketed paste of `@/outside/file …` and a carriage return
attaches the file there too (the model read its word with no tools), and `\@/outside/file`
does not, so the escape holds on the terminal route as well. The TUI's positional prompt and
stream-json's standard input are still assumed to read text the same way.

- **Escapes** (step 2's Q5 (a), now ADR 0121's "Composed text, as amended", on both routes,
  part 2f's Q2 (a)): what oso composed gets a space before a leading `/`, and a `\` before an
  `@` at the start or after whitespace (`sessions::send::for_claude`), once, at the one
  delivery path, `paste_into_session`, a control or invisible character looked through; a
  leading `!` or `#` gets the space too. Put into a draft, a `\` also follows text that ends
  in whitespace before an `@` of the draft, so that nothing put in makes the person's `x@y`
  the start of a word, a kept image's `@PATH` (`sessions::send::reference`) included, which is
  otherwise not escaped, nor is what the person types.
- **oso's own command** (amended 2026-11-23): the space's offer to paste `/add-dir PATH` into
  its orchestrator (ADR 0136, step 3; ADR 0161) is not escaped, as the escape's space made it
  no command since this part. It goes as a `sessions::send::Command`, which only
  `Command::add_dir` makes, from the space's recorded path, and only while that is absolute,
  in UTF-8, with no control or invisible character, no blank but a space, none at its end and
  no `@` after one, the command within 512 bytes; otherwise nothing is offered. The path is
  one of the space's repositories that the person trusts, looked at again at `y`, though an
  agent may have named it (`space_add_repo`) and anyone its directory; no other text takes
  this way. Only a terminal takes it: `claude -p` runs no `/add-dir`, so a native orchestrator
  is offered none, and a native session's composer refuses a command.
- **A send to a native session** (Q1 (a)): `paste_into_session` and `send_to_session` put the
  escaped text into its composer at point, not sent, and `send_to_session` shows the session
  with point in the composer after it; the person sends it with `C-c C-c`. A stopped session's
  composer takes it, its send waiting as part 2a says. Nothing goes in that would make the
  composer hold more than 64 KiB, though a send may be 256 KiB: "The composer would hold over
  64 KiB: nothing was put in"; nor into one a yank turned secret, which sends nothing. `session-send`, `session-mention`, review comments and git
  drafts all go this way, and say "Put into NAME's composer: C-c C-c
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
  after its indent; since "The rows' look, as built", two spaces on each), an empty line's without the spaces at its end, so that no row ends in
  spaces, which the trailing whitespace aid would show (as for every row's lines); the report's runs are put past each line's mark, which keeps its face, in
  one walk over the lines and the runs (`sessions::native::markdown`), and laid over the row's
  bytes in the buffer's highlights (`Editor::set_internal_highlights`), O(log n) and a step a
  run. The rows' faces as highlights (`Face::highlight`, `Rendered::runs`) moved to
  `sessions::native::render` so that the pure part shares them. Wrapping is the window's and
  folding redraws other rows only: both leave the runs where they are. A row drawn again is
  plain: what was open for it is closed at once, and a whole reply drawn again is opened again;
  a report under a key it no longer waits under is dropped, and one whose runs do not cover the
  row's bytes changes nothing. Markdown's markers stayed, in their face; amended on
  2026-11-24 ("The look: Markdown" below): most are hidden.
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
  again from its rows once a window shows it, those in view at once and the others in slices
  ("The look: Markdown" below), and then its newest whole replies are asked for, before the others, at most 256;
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

### The look: Markdown

The author decided on 2026-11-24 that the native view takes ECC's look and keeps oso's
structure, approved the plan's mock-ups, and chose, for this part (D1): `M-w` copies what is
shown and `w` a reply's raw Markdown (Q8 (a)); a link's target is hidden only behind plain
words. ECC is GPL: its look was read, and no code or colour of it is here. It amends part 2g.

- **Drawn without marks:** a reply's rows are drawn by `sessions::native::shown`, a pure walk over
  its text: no `#` before a heading, no `*` around emphasis, no backticks around inline code, no
  fence lines; `• ` for a `- ` or `* ` (an ordered list's numbers and `+ ` stay), `│ ` for a quote's
  `> `, a line of 32 `─` for a rule, a link as its text. oso writes the rows' text, so nothing is
  concealed by the display: the buffer holds what shows. An empty line stands for a fence's line
  where none was. Faces are existing ones, or the session's own since ("The rows' colours"): what
  hidden marks were around keeps a colour (a heading, emphasis, inline code, a link). Real bold and
  italic are a later part; tables are "The look: tables".
- **Faces:** `shown` gives a reply's faces at once, also while it streams. The parse (part 2g)
  still runs once the reply is whole: its runs lie on a fence's code, and on what has no face
  of `shown`'s; code whose language gave it none is in inline code's face.
- **While it streams,** a line is drawn without its marks once it ends; the last one is as
  written until then. `RowChange::Grew` carries a `LineStart`, the place a line of the text starts
  at with the lines drawn before it and what the lines before say of it (an open fence, a
  list), so that only what follows is drawn again and drawing from any of them gives the
  lines drawing whole does.
- **As written:** HTML, indented code, `+ ` items, `_` and `~` emphasis, images, reference links,
  backslash escapes, a setext heading's underline, a fence in a quote or right after a bullet, and
  a fence whose info string is not one word of at most 16 bytes, as it would hide words. A `*` that
  could be a wildcard, a pointer or arithmetic stays: one opens only after a space, a bracket,
  a quotation mark or a letter that is not ASCII, and before what a word starts with; one
  closes only after what a word ends with and before no ASCII letter or digit; none is read in
  an address. (`rm *foo*`, not in code, is emphasis, in its colour, as in any Markdown.) Marks
  are matched on one line only. Past 16 KiB of a line, 256 KiB of a reply, 64 rules or 8
  quote marks, text is as written, so that a crafted reply draws little more than it holds.
- **What Claude writes cannot hide or pass for oso's:**
  - A link's target is hidden only when it starts with `http://` or `https://`, has no title,
    is its line's only address, and the link's text is plain words: letters and digits of
    Latin, Greek, Cyrillic, kana, the ideographs and Hangul, and spaces, so no `.`, `/`, `:` or
    `@` and nothing that looks like them; and when nothing next to it could go on its text,
    and the rest of its line is words and the stops that end them, so that nothing there could
    read as its target. Otherwise the target follows the text in parentheses, or the text is the
    target. Other lines are not looked at: nothing opens a link here, and `w` copies what it is.
  - Bidi controls and invisible characters are in the text as they came, so the display shows
    them as in any buffer.
  - A line that starts, past its spaces and a tree's `│`s, with a character that could be a
    row's mark (`▸`, `›`, `●`, `∴`, `◌`, `·`, `…`, `✗`, `⚠`, `──`, or anything that is not ASCII, a
    letter as above or one of a few signs, or one a combining mark changes) keeps its marks;
    inside a fence, whose lines are hidden by then, it is drawn behind a `│ `, as is one that
    starts with `>`. What tells a reply from a row is that and its place, past its mark; its
    faces are none of a row's, whatever a fence's code is ("The rows' colours"). A row a
    soft wrap starts is not looked at: the continuation indent (ADR 0100) keeps it off the rows'
    column.
  - A mark between two characters that could join once it is hidden stays: it goes only
    between those known to stand on their own.
- **Keys:** `w` (`native-session-copy-markdown`, `Person`, `NativeAction::CopyMarkdown`) copies
  the reply at point as it came to the kill ring and the clipboard; `M-w` copies what shows.
- **Font lock on again** reads each reply's Markdown again, so a session's faces are laid for
  the rows in view at once, and 512 rows or 128 KiB an event, of one session at a time, the newest first, each slice
  asking for a frame, after which the next comes. Its replies are asked for after the last.
- **Measured** (release, the author's Mac, with throwaway tests, before and after, replies of
  headings, emphasis, inline code, a link, a list and a Rust fence every 150 bytes):
  - A reply of 64 KiB that streams in 40-byte deltas in a wrapping buffer, each folded in, drawn
    and wrapped: p50 9.4 to 10.3 µs, p99 26 to 29 µs; its end, the last line drawn once more, 48 to
    47 µs; drawn at once whole, 0.21 to 0.45 ms. One of 1 MiB in 400-byte deltas: p99 35 to
    44 µs.
  - Font lock on again over 10,000 replies of 1.1 KB: 41.5 ms in one event, to 1.4 to 1.6 ms an
    event, with those in view in the first.
  - A resume's rows, once: 10,000 replies of 1.1 KB rendered in 12 to 47 ms, and the most a resume
    reads, 8 MiB in 2000 replies, in 8 to 32 ms (4 ms for 1 MiB). Past one frame at the bound, once
    a resume: accepted, as part 5a bounds what is read.
  - A reply of 1 MiB made of one mark, such as rules, quotes or `[a](`: at most 6.2 ms and half
    again its bytes; of 64 KiB, 0.7 ms.
- **Built with** `sessions::native::shown` (`walk`, `LineStart`, `Piece`, `Style`), its
  `inline` and `signs`, `render::reply_lines`, which `render::tail`, `markdown::project` and
  `plain` share, `Open.mark` in `rows`, and the host's `markdown::faces`. One new public item, the author's OK of 2026-11-24:
  `NativeAction::CopyMarkdown`. Unit tests, a property that a reply drawn as it streams, cut
  anywhere, is the one drawn whole, the streams' fuzz target, and the e2e flow
  `sessions::native::markdown`.

### The look: tables

The plan's part D2, whose mock-ups the author approved on 2026-11-24. It amends "The look:
Markdown", where a table was drawn as written.

- **What is a table:** lines that start with `|`, at most as far in as text may be: a header, a line
  of dashes for as many columns (`:` aligns), then rows, until a line that does not start with one. Not
  in a fence, where it is code, nor in a quote. A row's cells past the last column are part of its
  last, `\|` is a pipe, and a tab a space: no word of it goes.
- **As drawn:** box rules (`┌─┬┐`, `│`, `├─┼┤`, `└─┴┘`) with a space beside each cell, in
  `SessionTable`; a header's text in emphasis's face, and the inline marks of a cell hidden as a
  line's. A rule between all body rows of a table in which a cell wraps, and none otherwise (the
  space's orchestrator). `M-w` copies what shows, `w` the Markdown, as before.
- **Its width** is the narrowest window's that shows the session, as a turn's result's, and at most
  100 columns, a reply's indent among them; 80 while no window shows it. Two windows of different
  widths show the one text, so it fits both.
- **Its columns** are measured in cells, as the display draws clusters: Japanese and other wide
  characters two, a control, bidi or invisible character as its escape. When they do not fit, the
  widest give way first, each keeping its longest word up to 16 cells where that fits, then 4 cells. A
  cell wraps as a wrapped line's rows do (ADR 0100: after a space or beside CJK, kinsoku kept; the
  rules are copied, as oso-core's are its own), and inside a word longer than its column, whose row
  then ends in `↩` in the rules' face, as does one broken beside a wide character where the cell holds
  an address: so no address reads as two.
- **While it streams** it is as written until its end: the next line that is none of its, or the
  reply's end. Then it is drawn again from its header (`RowChange::Grew`, whose `LineStart` knows an
  open table's start; `shown::again`). A delta inside it draws its last line only.
- **As written,** whole: a table whose columns cannot have 4 cells each (the space's orchestrator);
  one of more than 24 columns, with a line of more than 16 KiB, or of more lines than 64 and one more
  for each 4 of its bytes; what would take its reply past 64 KiB of tables, 16,384 cells of them or
  2048 lines, each taken before it is drawn, whether or not it then is; and one with a cell that would not show between rules as it
  measures: a character that joins the space beside it, such as a combining mark at a cell's start,
  or what the display cuts at a cluster's limits. Past 512 of a reply none is read as a table.
- **What Claude writes cannot pass for oso's:** a box's lines are past a reply's indent, so its `│` is not a
  nested row's guide; its rules' class and colour are a reply's alone, at least 0.05 in OKLab from a
  quiet word, an alert, a waiting call, a card's title and a change's lines, and a card's frame has
  other corners (no text: 4.5:1 on the background, 3:1 on the line at point); a cell's text has a reply's classes. A link in a cell hides its target only when no
  other cell of its row holds a link or anything but words.
- **A width change** lays out again, only for a session that a window shows: at once the rows in
  view, and what a reply that streams drew; then the newest 256 tables, or as many as take 131,072
  cells to draw, in slices of 64 KiB read and drawn an event, each asking for a frame; an older one as
  it comes into view. Only a reply's tables' lines are replaced, in one change, so the rest of it keeps
  its code's highlights; a table's own lines take none from the parse. `Rows` keeps the width, as an open reply's `LineStart` counts its tables' lines.
- **Earlier turns** are put in at once, so past 131,072 cells a reply's tables are as written until
  they are drawn as for a width change.
- **Measured** (release, this Mac, a throwaway test run twice each way in turn; 10,000 rows, 500 of
  them replies with a table of 13 rows, a 60 by 100 window that wraps), before and after:
  - a delta of 40 bytes of a 64 KiB reply of such tables, folded in, drawn and wrapped: p50 21.6 to
    21.8 and 22.8 µs, p99 29 to 30 and 179 µs, as 1 in 33 ends a table of 1.3 KB; at most 93 and 209 µs;
  - such a reply drawn at once, whole: 0.17 and 1.85 ms;
  - a frame's rows laid out: p50 21 and 22 µs, p99 26 and 26 µs;
  - a new width: 23 to 33 events of at most 1.6 ms each, the first with what is in view; with
    none, 1 µs an event;
  - crafted replies of 256 KiB (cells of one letter, of Hebrew, tables that fail once laid out, 30,000
    tables): 0.4 to 6.1 ms as they come, a new width at most 5.7 ms an event;
  - a resume's 2000 replies, at once: of four such tables each, 10 MB: 16 and 27 ms; crafted ones, 14
    to 90 and 15 to 99 ms.
- **Built with** `sessions::native::shown::table`, `shown::again` and `LineStart`'s open table,
  `render::tables`, `Rows::set_width` and the host's `native/widths.rs`
  (`native/results.rs` before). Two new public items, the author's OK of 2026-11-24:
  `oso_core::syntax::Highlight::SessionTable` and `oso_render::Face::SessionTable`, in the line
  numbers' colour of each theme. Unit tests; properties that every line of a box is as wide as its
  rules whatever its cells hold, that a reply that streamed is the reply whole at every width, and
  that a parse's runs lie where they should; the streams' fuzz target, with tables and width
  changes; a text-grid snapshot; and the e2e flow `sessions::native::markdown`.

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
  - The mode is the CLI's word for it, from `system/init` and `system/status`; part 3b holds
    it (`sessions::native::mode`), and reads `initialize`'s answer too.
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

### Part 3b: permission modes

The author's (a) on each of Q-B1 to Q-B3 of 2026-11-23, and their choices of the same day:
tier 2's request with a 2 s wait, else tier 1; the mode shown changes only when the CLI says
so; `--permission-mode` on every later process.

Measured on 2026-11-23 (CLI 2.1.330, `--model=haiku`, made-up prompts, empty directories):

- `set_permission_mode {mode}` is answered `success {"mode": M}`, then `system/status` says
  `permissionMode: M`; to the mode the session is in already, a success and no status. An
  unknown mode is an `error` (`invalid_mode`). Sent during a turn, it is answered at once, and
  the turn's next call runs in the new mode.
- `--resume` does not keep the mode: a session left in `acceptEdits` resumed in `default`, and
  `--resume --permission-mode plan` resumed in `plan`.
- `--permission-mode` takes `manual` as another name for `default`, which the CLI then says.
- `initialize`'s answer says `current_permission_mode`.

As built:

- **The type** (`channel::protocol::mode::PermissionMode`): `default`, `acceptEdits`, `plan`
  and `auto`, the only modes oso sends, on the command line, in the request, and through a
  session's bridge as part 3d's plan approval's `setMode`, which takes only `acceptEdits` and
  `auto` (`permission::AFTER_PLAN`, in place of 3d's own `PlanMode`). It sits below
  `sessions`, in `channel`, so that the bridge and the sessions share it. It has no value for
  `bypassPermissions` or `dontAsk`, so neither can be sent. No agent or channel method takes one.
- **The mode held** (`sessions::native::mode::ModeState`): the one the CLI said last, in
  `system/init`, `system/status` or the first `initialize` answer that succeeded (the control
  layer takes no later one, for its commands and models either; one after a refusal or a wait
  that ended is still taken, as step 2c has it), held to what reads as itself as part 3a's
  parts are; a mode oso does not send is shown as the CLI says it. It is the one copy:
  the status line is given it to write (part 3a, amended).
- **The key** (Q-B1 (a)): `S-TAB` in the transcript and the composer,
  `native-session-cycle-mode` (`Person`), switches to the next, in the CLI's terminal's order:
  `default`, `acceptEdits`, `plan`, `auto`, and round (the author kept the terminal's order
  after the security review, though `plan` goes to `default` through `auto`, which on tier 2
  holds from the turn's next call); from a mode oso does not send, to `default`. From a mode oso put the process in that it has not said yet, the next after it,
  so that two quick presses go two steps. Before the CLI said a mode, it says the mode is not
  known yet. There is no oso
  setting: a new session starts as Claude Code's settings say.
- **Tier 2:** the `set_permission_mode` request, over the control layer, as `interrupt` is. A
  success changes nothing shown: the modeline waits for the CLI's `system/status`. With no
  answer within 2 s, an error, or the request not written, it falls back to tier 1, and why is
  logged.
- **Tier 1** (Q-B3 (a)): an idle process is stopped and resumed at once with `--resume
  --permission-mode`, as part 2c's tier-1 interrupt resumes, under the trust judged then; a row
  says "Changing the mode to M: claude ends, and resumes its conversation in it". During a turn
  it is refused: "it can change once idle". Windows has a tier 1 here, as no signal is needed.
  A session never given a message has no conversation to resume, so it has none. While the
  process starts, before `initialize` was answered, a switch is refused too, and so it is on
  tier 1 while a resume could not start (a stop asked, oso waiting to quit or restart, an
  orchestrator's replacement waiting, a conversation that moved), as the process would only
  stop. A switch whose process ends without resuming is over, and a row says the mode did not
  change.
- **Every later process** is given `--permission-mode` (the author's choice after the security
  review, 2026-11-23, an orchestrator's resume included): a resume (the person's, or an
  orchestrator's `session_resume`), part 2c's tier-1 interrupt's resume, which lost a mode
  other than the settings' before, and a fork, its source's. The mode is the one a tier-1
  switch asked for; else the one oso gave the process before, until that process said a mode,
  so that a step down is kept though the process ends before it speaks; else the one the CLI
  said last, if it is `default` or `plan`, or if the person switched to it here (a request
  answered with success, or a tier-1 switch); else `default`, once the person switched here,
  so that a later process never goes back to the settings' mode, which may ask less. A session
  nobody switched is given none, and starts as the settings say. A fork starts with its
  source's state: the mode its source's next process would get, pending, and the person's
  choice. A tier-1 switch whose resume is then refused (trust, or an orchestrator's start) is
  over, and a row says the mode did not change. One switch goes at a time. A request's answer
  is noted in the stream's order, so a mode the CLI says after it wins.
- **The stream is not authenticated** (the security review): a tool the session runs, with
  the person's leave, can write a line to the stream that says any mode. So a mode that asks
  less, `acceptEdits` or `auto`, is shown as said but carried only when the person chose it;
  a line that says a mode asking more is carried, the safe side. A line can still make the
  modeline say another mode than the CLI's until the CLI says one again. Part 3d's plan
  approved with a mode (`a`, or `A` after its question) counts as the person's choice of it,
  as a switch does (`ModeState::chose`), once the bridge took the answer: the author approved,
  on 2026-11-23, that it is carried across resumes, forks and an orchestrator's resume until
  the person switches.
- **Across an oso restart** (Q-B2 (a)): the mode is not in the record, so a session resumed
  after the restart starts as the settings say.
- **Public API (approved):** `NativeAction::CycleMode`.
### Part 3c, as built: `/model` and `/effort`

The author's choices of 2026-11-23: `Model:` completes over `initialize`'s `models`, `Effort:`
over the chosen or current model's levels, and on tier 1 the five `--effort` levels; the model
and effort are noted only from a `result` with `local_command` `model` or `effort` and no
error; the effort shows after the model once noted (Q-C1 (a)).

Not measured with the commands themselves: `/model` can save the person's default model, so no
probe types it. Read instead from the CLI's own text (2.1.330) and `--help`:

- `/model NAME` replies `Set model to …`, or `Kept model as …`, `Current model: …` with no
  name, and `Model '…' not found` or `… isn't available …` for a model it does not take.
  `/effort LEVEL` replies `Set effort level to …`, `Effort level set to auto` for `auto` and
  `unset`, `Unknown option: …`, `Couldn't set effort level: …`, or `Effort '…' exceeds the cap
  …; set to '…' instead` when the person's settings cap it. Each is the `result`'s text, as the
  synthetic reply's (step 3's probes).
- `--effort` takes `low`, `medium`, `high`, `xhigh` and `max`; `--model` an alias or a full
  name.
- `initialize`'s `models` give each model's `description`, `supportsEffort` and
  `supportedEffortLevels` (part 3a's fixture).
- Whether `--resume` keeps a `/model` or `/effort` choice is unmeasured, so every later process
  is given both.

As built:

- **Completion** (`commands::agent_sessions::composer`, `dispatch/slash.rs`): `/model` chosen in
  part 2d's `Command:` prompt, once it put `/model ` first in the composer, opens `Model:`
  (`native-composer-model`), and `/effort` opens `Effort:` (`native-composer-effort`), each when
  the host listed rows for it; `C-c /` on a `/model` or `/effort` the composer begins with, as
  a word of its own, opens its argument's prompt in place of `Command:`. The one chosen goes
  after the command: on `C-c /`, in place of the word after it on its line; chained after
  `Command:`, which put the command before what the composer held, in place of none (the
  dispatcher gives the prompt a prefix argument, which says so). It is not sent: the person
  sends it, as any message. `C-g` leaves `/model `, to be written by hand. The rows keep the
  CLI's order, the levels from the least, and `Model:` takes a full model name it does not
  list, as `--model` does. Both commands are `Person`. No public item: the dispatcher chains
  the prompts, as 2d opens `Command:`.
- **The lists** (`app/sessions/hosting/native/slash.rs`, beside 2d's): the models the
  selected composer's session's last `initialize` listed (part 3a's list, at most 64), each row
  its value, then its name and its description's first line, cut to 256 bytes and escaped as
  completion rows are; and the levels of the model chosen with `/model` (by its value or id),
  else of the model shown, as `initialize` lists them (at most 8, each a word), else the five,
  as on tier 1 or for a model the list does not name. A model that takes no effort has none,
  and `Effort:` does not open. They are made again only when the session, its model list, or
  the model the levels are of changed.
- **Noted** (`sessions::native::choice`): a `/model` or `/effort` the composer sent, the CLI's
  own command (its list's, or its name while the list says not whose a command is), is asked;
  the `result` that ends its turn notes it when it names that local command, is no error, its
  subtype `success`, and its reply starts `Set model to `, `Set effort level to ` and the level
  asked, or `Effort level set to auto`, which clears the level. The author chose this stricter rule on
  2026-11-23, so that a reply that set nothing, or set another level than asked, notes nothing,
  and no mistyped model goes on a later process's command line; a later CLI that words its
  reply otherwise notes nothing either, the safe side. One message is under way at a time, so
  one command is asked at a time, and any `result`, or a new process, ends it.
  - What is noted is what the person sent: a model that is one word, reads as a model's id
    (part 3a's `Kind::Id`) and does not start with `-`; a level one of the five, in lower
    case. Another argument (two words, `ultracode on`) notes nothing.
  - The modeline's model stays the stream's (part 3a): `system/init` names the new one at the
    next turn. The level shows after it from the `result` on (Q-C1 (a)).
  - A level noted stays across a `/model`, as the CLI says nothing of it then: a later
    process is given it with the new model. That does not break a start: `claude -p --effort
    high` ran and answered with `haiku` and with `claude-haiku-4-5`, an older model `initialize`
    does not list (measured 2026-11-23). No listed model says it takes no effort, so that case
    itself was not seen.
  - The stream is not authenticated (part 3b): a forged `result` can note what the person
    asked where the CLI refused or capped it, and a later process then starts with it.
    Whether the CLI holds `--model` and `--effort` to the person's settings, as it holds
    `/model` and `/effort`, is not measured.
- **Every later process** (`NativeOrder`, `command::Line`): a resume (the person's or an
  orchestrator's), part 2c's tier-1 interrupt's resume, part 3b's tier-1 mode switch and a fork
  are given `--model=` and `--effort=` with what was noted, beside `--permission-mode`, one
  argument each; a fork starts with its source's. A new session is given neither, and neither
  is in the record, so a session resumed after oso's restart starts as the person's settings
  say.
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
- **Amended 2026-11-24 (the author): an edit asks first, and its approvals wait.** An edit that
  differs from Claude's plan asks y or n before every approval, `C-c C-c` and `C-c C-a` as
  `C-c C-A`, saying how many lines differ: "Approve NAME's plan as you edited it (+2 −1 lines)?
  (y or n)". The question is auto's, above, kept with the approval it asked of, and its `y`
  approves the edit only as it was when asked. A plan the person did not change asks nothing
  but auto's question. The approvals in the edit wait 500 ms from when the edit became the
  selected buffer, as the card's keys do from when the card did, and longer after a prompt an
  agent opened closed; not since the key before, as the person writes the edit and then
  approves it.
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
No `NativeAction` was added then (the amendment below adds one): `M-n` stays `History`.

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
  makes it none, so that what goes into the composer is one line a row shows as it is,
  never a line hidden below it. It showed dim at the transcript's end, after the queue's rows,
  as `Suggested: TEXT` (now in the composer's placeholder, as amended below), and goes when a turn starts (oso's send, or the CLI's replay or
  request), at each send and when the process ends; the next turn's replaces it.
- **`M-n` takes it (Q-F2 (a)):** past the newest message sent, that is with no message of the
  history shown, in an empty composer that is still one and that no yank turned secret, as
  `compose_into` asks, `M-n` puts the suggestion in, not sent; `native-composer-next`'s
  description says so. It is Claude's text, which what Claude read can steer, put in by oso:
  so it is composed text, escaped as part 2f has it (`send::for_claude`): a leading `/`, `!`
  or `#` gets a space and an `@` that starts a word a `\`, which the person sees in the
  composer. A suggested command or file to attach then runs only if the person takes the
  escape out. The security review agreed, with the line breaks refused as above.
- **In the placeholder, and `C-c C-s`, as amended (2026-11-24):** the author found the row alone
  of no use and asked for the behaviour of their own Emacs package; the rest is the space's
  orchestrator's, inside that. The row is gone: the suggestion is an empty composer's
  placeholder, `TEXT   (C-c C-s takes it)`, over the usual hint
  ([ADR 0170](0170-docked-windows-under-a-sessions-transcript.md), amended).
  - `C-c C-s` (`native-session-take-suggestion`, `Person`), in the transcript and the composer,
    puts it at the composer's end, not sent, and goes there, point after it. With none, as in a
    stopped session, it says so; a transcript set aside in the TUI says what its other keys do;
    a card and a plan's edit do not bind it. It asks with `NativeAction::Suggestion`, the name
    approved for this part and first added now.
  - With a draft it goes after it, escaped, with a space first unless the draft ends in
    whitespace (`send::after_draft`): joined with no space, a draft's last `@` or `/` would
    make a file to attach or a command of what `for_claude` never read so. It is refused, the
    draft left as it is and the echo area saying why, after a draft that is a command, a shell
    command or a memory (its first character `/`, `!` or `#`, behind what a paste drops
    too), as it would be Claude's text as its argument; after one that leaves a quoted `@"` open;
    beneath a prompt; in a composer that is no longer one or
    that a yank turned secret; past 64 KiB in all; and in a composer that is hidden, as the
    person could not read it. The composer's view follows point to its end, so that a long draft
    shows what went in. A send that waits for a dictation's fix is dropped by it, as by any edit.
  - The host puts it in after the event, as `i` goes to the composer. Each key and commit of the
    person's is an event of its own, so none overtakes it; the keys a keyboard macro replays, or
    that were held while a command waited, run together, so those after `C-c C-s` run before it,
    as those after `i` do. A change for both is left to a step of its own.
  - The text kept, the escapes, and when it goes are as above. `M-n` stays, in an empty
    composer. No buffer holds the suggestion, so what reads the transcript, the dev control
    socket among it, no longer reads it, nor does the agent channel, which never did; a
    screenshot of the window still shows it.
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
- **Costs:** a suggestion is a copy of at most 256 bytes, once a turn, with a frame asked for
  only if a window shows the session, and as it shows, the measure of that text in each frame; the recap walks
  back over at most 10000 rows once, when the session comes in front, and after each event
  only the marked sessions are looked at.
- **Built with:** `sessions::native::suggest` and `sessions::native::recap` (pure, with tests
  of the line, which suggestion is kept, the last reply, the first sentence and the recap's
  cut); `app/sessions/hosting/native/suggesting.rs` (the events and the takes), over
  `composer/composed.rs`, and
  `native/recap.rs` (the mark, and the recap after each event, from `sync_native_sessions`),
  with tests of what keeps and clears each; the placeholder, `composer/placeholder.rs` and
  `app/draw/tiled/placeholder.rs`; a
  test that the agent channel does not read an echo withheld so; and the e2e flow
  `sessions::native::suggest`.

## Step 4, as built

Step 4 is built in parts, one pull request each, from the author's answers of 2026-11-23,
through the space's orchestrator. Each part's choices are recorded here as it lands.

### Part 4a: trust from the record of trusted projects

The author's Q4-10 (a), part 2b's follow-up:

- **Where it applies:** a native resume (`session-resume`, an agent's `session_resume`, the
  tier-1 respawn of part 3b, which is a resume, and a restart's resume once part 4c lands) and
  a native fork. A new start keeps part 2b's rule: an open, trusted project at its root.
- **An open buffer decides:** with a buffer of the project at the session's root open, its
  trust now is the answer, as in part 2b: open and not trusted, or its trust taken back,
  refuses whatever the record says.
- **With none open, the record decides,** in two looks. The UI thread asks whether the record
  holds the root's path (if not, a fork or an adoption runs in a terminal, and a
  resume is judged by its worktree's claim, "Worktrees' resumes" below), and the order carries a
  copy of the record to the start thread (a few lines; at the file's 256 KiB cap about 14 000, a
  copy of about 1 MiB, once per resume or fork, as oso-core gives no line of it alone). There,
  before the root's other checks, the root's identity is read (not following a link; a
  directory) and judged by path and identity as an issue space's member is
  (`TrustedRoot::of_member`, ADR 0136, step 4): a directory made again at the path, a link
  there, a file or nothing is refused, "the record of trusted projects does not hold the
  directory at its project's root now". The usual look again before the spawn follows. The
  record holds no worktree claim; a resume of a native session oso has in a linked worktree is
  judged by ADR 0114's check, read afresh on its start thread ("Worktrees' resumes" below). A
  new start, a fork, an adoption or a move into a space in one still needs an open, trusted buffer
  of it. Only at the project's root, as before.
- **Taken back until it starts** (the security review's finding): the start thread's checks
  before the spawn can take seconds (the registry, a worker's reads with git), so the UI keeps
  each native resume's or fork's trust until its start thread reports, and after each event
  takes it back once its project is no longer trusted, by an open buffer of it or by the
  record's path, or once it is no longer starting with no report from its start thread, as
  when its buffer is killed meanwhile. The root the start thread makes from the record shares
  that revocation, as an open buffer's does, so the look before the spawn refuses it. This
  also closes the same window for a resume or fork an open buffer judged, which part 2b left
  open. A start that waited is judged again with the record as it is when it goes.
- **Another spelling of the root:** an open project whose identity the record pairs with the
  session's root, as on a case-folding file system, refuses when it is restricted, as the one
  spelled so would. A trusted one decides nothing: the record judges the root, so that the
  start thread checks the identity where the session runs. Windows has no identity to pair.
- **A record not read yet** waits, as ADR 0173's held starts do: a resume makes its buffer and
  is marked starting at once, and its order waits; a fork waits whole. Each goes once the
  record's thread has reported, unless what waits still waits for the configuration, and is
  judged again then. Within the held starts' one bound, five seconds after the first start
  that waited (one kept again as the configuration's look lets it go keeps that deadline), it
  is judged with what is known: a record still unread trusts nothing, and the resume says "the record of trusted projects is
  not read" (a fork runs in a terminal). `*Messages*`' "The configuration was not read in 5 s"
  is said only when the configuration was the one not read. A record refused or broken, or no
  record at all, is read: nothing waits for it, and it trusts nothing.
- **Session tools:** a fork judged by the record keeps no trust of its own for its tools
  (`hosting.trusts`), so its caller's project is judged by an open buffer, its own transcript
  among them ("Transcripts count as open").
- **Built with:** `sessions/start/trust.rs` (the judge, pure, with its table of tests, and the
  start thread's look, with tests on real directories: made again, gone, a file, a link);
  `app/sessions/hosting/trust.rs` (the editor's projects and record into the judge); the
  record's first report in `app/projects.rs`, which lets the waiting starts go; and the e2e
  flow `sessions::native::resume::a_native_session_resumes_with_no_file_of_its_project_open`.

### Worktrees' resumes (the author's answers of 2026-11-24)

The author's (a) on Q1, Q2, Q5 and Q6. Part 4a's record holds a worktree's main repository, not
the worktree, and a native session's transcript is an internal buffer, which no lookup of
open buffers counted then, and which still decides no resume; so a native session in a worktree resumed only with a file of it open,
and after `oso-restart` stayed stopped.

- **The rule.** With no buffer of its project open and the record read, a resume whose root's
  path the record does not hold is judged by ADR 0114's check on its start thread. A root the
  record holds stays the record's to judge: held with another identity, it is refused. The
  order carries a copy of the record and the home directory. There, the root must be where its
  path leads with no link on the way, the session's directory and its project's root; the walk
  must find it as its own root, never the home directory; its claim must hold both ways; and
  the record must trust the main repository by path and identity. Nothing is run, and nothing
  is read on the UI thread or on the `oso-project` thread. The worktree's own directory is
  compared before and after the reads, as the main repository's is.
- **Twice:** once where part 4a's look is made, and the whole of it again just before the spawn,
  when it must also give the same directory by its identity.
- **Taken back until it starts:** as part 4a's, with the claim's own rule, as the UI does not
  know which root the claim names: an open buffer of the worktree that is not trusted, the
  record refused, or any root leaving the record while it starts takes it back. A root added
  does not.
- **Which resumes:** of a session oso has: the person's `session-resume`, an agent's
  `session_resume`, `oso-restart`'s resume and the resume of a restart that stays, the respawn
  after an interrupt, a mode change or a repository added, a review's question's `y`, and a TUI
  session taken back natively (part 4d-2), once its terminal is gone: the switch's checks before
  that ask no claim, so that nothing is hung up that may not come back. Not a new start, a fork,
  an adoption or a move into a space, held ones among them; a session moved before resumes as
  any other. An issue space's own directory stays its gate's
  (ADR 0176).
- **A record not read yet** is waited for, as in part 4a, also before an open buffer of its
  project that is not trusted refuses such a resume, as no project is trusted until the record
  is read. **A known limit:** a file of the worktree carried over a restart is asked for its
  project again only once the record is read. A resume that waited for the record is judged then,
  before that answer, so it may find the buffer not trusted and refuse; it says that
  `session-resume` resumes it once it is trusted, which it then soon is. Seen once end to end, the
  record was read before the instance going had gone, and such a session resumed. A plain
  repository's carried file does not refuse so, as the record alone trusts it.
- **No baseline, no tool trust:** a resume takes no baseline, as before, and nothing is kept in
  `hosting.trusts`; an agent's `session_resume` asks as it did (ADR 0153), until
  "Transcripts count as open".
- **Accepted.** Trusting a repository trusts its worktrees' settings, hooks and `.mcp.json` at a
  resume too, also at a restart with no person there, as at their start and as a repository's
  own root's: the author accepted it. They run as they are at the restart, whatever changed
  while the session was stopped. As in ADR 0114, a directory git records at the path again is
  the worktree.
- **What is said.** A native resume refused on its start thread or after it waited, and one oso
  makes itself (a restart's, a respawn), says why in its transcript, "It did not resume: WHY",
  and after a restart "It did not resume after oso-restart: WHY", with the
  mode carried as before. "Resumed after oso-restart." comes once its process started, not as it
  is ordered, and no "Resumed." row comes before it; so does "oso-restart stayed here: resumed.".
  Once every resume of a restart has reported, the echo area and `*Messages*` say once
  "oso-restart left 2 sessions stopped: A, B; each transcript says why", three names at most,
  then "and N more", in place of a line for each. Its row of `*Spaces*` takes a second line, "not
  resumed: WHY", cleaned as a recap is, while it is stopped, until it runs; kept in memory only
  (it amends ADR 0131's Q2, as the author answered Q5). There each of oso's own reasons has words
  short enough for a recap's 80 characters, as "not resumed: repository not trusted;
  trust-project, then session-resume"; an error of the system's or of a space's checks may still
  be cut, and a narrow list cuts what to do, which the transcript says. In the transcript a
  claim's WHY is "its repository is not trusted now: trust-project it, then session-resume" where
  the repository its `.git` names is not a root of the record, "its project is no longer trusted"
  once taken back, and "its directory is neither a trusted repository nor a worktree of one now"
  for anything else. What its `.git` names is never shown.
- **Built with:** `sessions/start/trust.rs` (the judge, `Claimed` and its two looks),
  `TrustedRoot::of_claim`, `project::detect_root`, and
  `app/sessions/hosting/native/resumed.rs` (what is said). No new public item of a library
  crate.
- **Tests:** the judge's table and the looks at real directories, each refusal, what changes
  between the look and the spawn, and the taking back (`start/trust/tests/claims.rs`);
  `check_main`'s comparison (`project`); the rows' and the echo's words; the list's second line.
  End to end (`worktree_resume`): a restart resumes an agent's worktree session with no file of
  it open; one whose repository was untrusted stays stopped and says why, until it is trusted
  again and resumed; an orchestrator's `session_resume`. Not tested: the app's own wiring of
  each caller's flag and the switch's checks (the app has no harness).

### Transcripts count as open (the author's answer of 2026-11-24, Q3), as built

A native session's own transcript counts as an open buffer of its project wherever ADR 0132 and
ADR 0153 mean one, as a TUI session's terminal does: for the session tools' trust, so an unasked
`session_resume`, `session_start` and the worktree tools under what ADR 0135 and ADR 0153 allow;
for a turn's end look for ready for review; for the tree's listings, `w`, `*Worktrees*` and its
removals from `*Spaces*` and from an orchestrator's buffer; for saved reviews and those a restart
carried; for the repositories oso knows; and for where `session-adopt` and a fork of what
`session-search` finds place a conversation. Each is gated by trust as before. Where this ADR's
parts say an open buffer decides a native start, resume, fork or adoption, a file or a terminal
is meant.

- **Never a native start where it runs, or a resume.** What judges one stays on files and
  terminals: no transcript, its session's or another's, makes one native or refuses one. A
  worktree just made starts under its repository's trust, as before, which a transcript may
  give, as a terminal does. The sweep that takes back what sessions started under (ADR 0121)
  stays on files and terminals too, as a start's order shares what it takes back; a tool's call
  asks the transcripts first, so a restricted one refuses its session's tools.
- **An agent's `session_start` in its own directory** is judged native or not as before: a
  transcript alone gives that start no trust, and so no baseline.
- **What a tool holds is taken back** with what its session started under, where both are of one
  directory: the tools act under that, not under a fresh look at the buffer.
- **A new session from `*Spaces*`** in a project only a transcript has open takes its root and
  its baseline there, and is not native. The selected buffer's project is a new start's as
  before, whichever buffer it is.
- **Placing a conversation:** the deepest open project that holds its directory, a transcript's
  among them, with two exceptions, where the files and terminals decide as before. A transcript
  never changes whether the conversation runs natively; it could only where the record itself
  holds a root nested in another open project. And a restricted transcript never takes a
  conversation out of the project of an open file or terminal.
- **At a resume** a transcript's project is found again, as a terminal's is.
- **Which transcripts:** those of the sessions in the table, stopped ones and archived ones whose
  buffer stays among them, as TUI sessions' terminals; not one whose start is under way, nor one set aside while its
  session runs in the TUI, whose terminal counts. The files and terminals are asked first.
- **`Editor::buffers()` is unchanged:** a transcript stays an internal buffer, with nothing to
  save, hand over or list.
- **Measured** (release, one pass over every open project): 0.4 µs at 50 buffers and 10
  transcripts, 42 µs at 500 and 256, the sessions' cap. No lookup runs for each key; the tree's
  asks at most 8 times a second, while a repository of it is not listed.
- **Built with:** `app/sessions/hosting/open_projects.rs` (what counts), `placing.rs` beside it,
  and `app/sessions/tools/trust.rs`. No new public item of a library crate.
- **Tests:** what counts and what does not, the tools' trust, a new session's and a placed
  conversation's mode, the sweep left alone (unit tests over a real editor). End to end
  (`worktree_resume`): with no file of a native worktree session open, its turn's end with
  changes makes it ready for review, its orchestrator resumes it without asking, and asks once
  its repository is untrusted. Not tested: each lookup's own call (the app has no harness).

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
  Ending them gently and resuming them in the new instance is part 4c's (below).
- **Two older faults,** found by the end-to-end flow, kept any restart from happening while a
  native session was in the record, or while no terminal ran. The new instance brings a native
  session back with its empty composer, which a restart makes again rather than carries
  (part 2a), and `carry::import_allowed` counted it as something the instance had opened; it
  no longer does. An export that carries no terminal hands over no masters, as the order asks
  for none, and the exit refused it as terminals exported but not handed over; it now lets
  such an export go.
- **New public items:** `oso-core`'s `Editor::{set_restart_when_idle, restart_when_idle}`,
  approved by the author through the orchestrator.
- **Tests.** What holds the restart, and that an ended session holds nothing
  (`native::holds`); the echo's names and count, and the hold with something held, nothing
  held and a prompt open when it is due (`restart::wait`); `C-g`, `C-x C-c` and
  `quit-when-idle` cancelling the wait (`commands::session`), and `C-g` in a native session's
  buffer by keys (`dispatch`); an empty composer that came back lets an import in, and one
  with a message does not (`carry`). End to end (`sessions::native::restart_wait`): a fake whose
  turn waits for a release file holds `oso-restart`, which starts no instance while it waits;
  once released, the new instance takes over; and `C-g` cancels a wait.
- **Not done:** a mark while the restart waits, other than the echo said once (an agent's
  session ending its turn chooses the moment within the hold, and an agent's starts keep it
  waiting; the person cancels); a test of the
  app's own wiring (the queue held, a refusal once due) beside the end-to-end flows, as the app
  has no harness of its own.

### Part 4c: `oso-restart` resumes native sessions in place

Q8 (a), with the author's answers of 2026-11-23: Q4-2 (a), the mode, model and effort carry
across through the restart's state; Q4-3 (a), no transcript row is carried, and one row says
"Resumed after oso-restart."; Q4-4 (a), a composer's draft or a queued message still refuses
the restart; Q4-10 (a), the resume is judged by part 4a's trust.

- **Before this part,** the instance going killed every native process with `SIGKILL` as it
  wrote its records, and the new instance brought each session back stopped.
- **The instance going** (`app/sessions/hosting/native/restarting.rs`, `app/restart.rs`).
  Once part 4b's wait is over and the restart's checks pass, and the agent channel is off,
  each native session whose `claude -p` runs and that was given a message is stopped as the
  session list stops it: its standard input closed and its group sent `SIGTERM`, `SIGKILL`
  after 5 s. Its end is no failure, and no resume starts of itself meanwhile (the tier-1
  respawn and the interrupt's resume are held, as a restart under way holds them). Nothing
  waits on the UI thread: each end is an event, and once each has ended the checks run
  again, as the person may have written meanwhile, and the restart's thread starts as
  before. A session given no message has no conversation, and the exit ends it as before.
  Nothing resumes in the instance going while its restart goes on: `session-resume` says
  "oso-restart is under way" (the security review's finding: a resume then would be a second
  writer once the new instance resumed it too), and a session started meanwhile keeps the
  restart here, as does one that runs at the exit. One not ended within 10 s keeps the
  restart here; it is not resumed, and says that `session-resume` resumes it. A session oso
  is stopping already, at the person's word or for an orchestrator's successor (ADR 0136),
  holds the restart's wait (part 4b) until it has ended, and is not resumed: the stop wins,
  and a successor starts before the restart, not during it. Nor is one the person is asked
  whether to stop. An instance that is going (its window closed meanwhile) resumes nothing
  and starts no restart.
- **The state** (`carry/natives.rs`) names each session ended so, or under `oso dev`, whose
  order ends none first, each that runs at the export, which the exit ends: its session id,
  and the permission mode its next process would have been given (part 3b's rules), its
  model and its effort level (part 3c). It is an optional field, `native_resumes`, at most 64
  entries, each read on its own: an entry that does not read (an id not of 36 ASCII
  characters, a mode oso never sends, a model or level too long) is left out, never refusing
  the restart. A reader from before this part knows no such field, and brings the sessions
  back stopped, as before. The state version stays 5.
- **The state is written last,** once the terminals are in, and so is left out rather than
  refusing a restart that would not fit with it.
- **A restart that stays here,** for any reason, its new instance not starting, refusing the
  state or told to stay out, resumes here each session it ended, under the trust judged now,
  each saying "oso-restart stayed here: resumed." or why it did not.
- **The new instance** reads the field only from its parent, as it reads which instances a
  bridge follows. It resumes nothing until the instance before has gone: `oso-restart`'s
  `oso-carried` thread waits, as for the terminals' masters, until its parent is no longer
  the instance going (at most 10 s, else nothing resumes), and asks `import-terminals` then,
  with a count of 0 when no terminal came; `oso dev`'s supervisor asks it once the old
  instance has stopped, or once one that agreed has gone. A word that they stay
  (`import-terminals` with `abandon`, or being told to stay out), or masters that do not
  match, drops them; `handover`'s `before_gone` says which to the sessions, beside the
  terminals' import. Then, once the record of sessions is read,
  each id the record has as a stopped native session with its buffer is resumed with
  `session-resume`'s checks: the name, directory and root are the record's, never the
  state's; its trust is part 4a's, or for a linked worktree its claim's ("Worktrees'
  resumes"), as no person is there to open a file (an open buffer of
  the project decides; else the record of trusted projects, by path and identity); its start
  waits for the configuration's first look (ADR 0173); and the start thread refuses a
  conversation live elsewhere, by Claude Code's registry. A refusal leaves it stopped, its
  row saying why. The mode, model and effort carried are its own either way, as they were in
  the instance before, and its next resume starts with them: the refusal's row names the
  mode. Only a stopped session is resumed,
  and one the person or an agent resumed meanwhile is not resumed again. Its next process
  starts in the mode carried, taken as the person's choice,
  so that a mode that asks less is not carried from what its stream says later, and with the
  model and effort carried, read again as part 3c reads them. A session the state does not
  name, a stopped or failed one among them, stays stopped.
- **One writer.** The process before has ended, and its group been killed, before the
  restart's thread starts; the new instance resumes only once the instance going has gone;
  and the start thread's look in the registry refuses a conversation still live. Under
  `oso dev`, an old instance that agreed to exit but has not gone while terminals go with it
  has the new one resume anyway, and the registry's look is then what refuses a process not
  yet ended.
- **Not done:** a mark while the sessions end, other than the echo said once. The
  transcript's past rows are part 5a's: a session the new instance resumes shows its earlier
  turns before "Resumed after oso-restart.".
- **Tests.** The field's round trip, its refusals and its cap (`carry::natives`), and that a
  state that would not fit leaves it out; the new instance's wait for its parent with no
  terminal (`carry::coming`); the supervisor's word after a stop, and none while the old one
  stays or has not gone (`dev::restart`); the mode and choice a resume starts with
  (`sessions::native::{mode, choice}`). End to end (`sessions::native::restart`): an idle
  session in `acceptEdits` is resumed by the new instance, run 2's arguments carrying
  `--resume=ID` and `--permission-mode=acceptEdits`, and takes a message; with
  `OSO_DEV_RESTART_FAIL=hello` the instance going resumes it itself; with `=exit`, after the
  new instance took the state, the instance going resumes it and the new one, told to stay
  out, starts no run.
- **Not tested:** the app's own wiring alone (a non-parent's import that names sessions, the
  record's matching, the 10 s bound, a resume refused during the restart), as the app has no
  harness of its own beside the end-to-end flows.
- **New public items:** none.

### Part 4d-1: a native session handed to the TUI

The author's (a) on Q4-5 to Q4-8 of 2026-11-23, through the space's orchestrator, carrying out
"One session, and the TUI as a hand-off" (Q3) one way; part 4d-2 takes the session back.

- **`session-switch-mode`** (`Person`): `C-c a t`, the Agents menu's new head `t`, in its
  "Session" group (Q4-8 (a)), which ADR 0143 had kept for ADR 0129's dictation; from the
  transcript or its composer, whose `C-c C-t` stays the to-do strip's (part 3e). A TUI
  session's terminal has `C-c C-t` (part 4d-2).
- **When.** A native session switches while it is idle, done or stopped. It is refused while
  its process starts, a turn is under way, it is being interrupted (step 2c), its permission
  mode is being changed (part 3b), it asks the person (a card, a plan's edit among them), or
  messages are queued, which the TUI would not send; and, as no resume could go, when it was
  never given a message, its conversation moved to an id oso does not follow (part 2d), or a
  resume is held (a stop asked, oso waiting to quit or restarting, an orchestrator being
  replaced). The rules, and where each switch is, are a pure state machine,
  `sessions::native::switch`: ending, then (once the process ended and was reaped) starting
  the other mode, then over, one switch per session at a time.
- **Never two writers.** The CLI does not refuse two processes writing one conversation (two
  `-p --resume` of one id both ran and appended, measured 2026-11-23), so this is oso's alone:
  the TUI starts only once the stream said the native process ended, which it says after the
  reap, and its start is a resume, whose start thread reads Claude Code's registry and refuses
  a live process of the conversation elsewhere, as every resume does (ADR 0121, "Restore and
  the fork hazard"), with the second look 5 s later; a `-p` process is in that registry
  (`entrypoint: sdk-cli`, measured 2026-11-23), so another instance's process of the
  conversation refuses it too. The native process ends as a stop ends it: standard input
  closed and `SIGTERM` to its group, `SIGKILL` after 5 s (an idle `-p` exits 0 in 0.8 s,
  measured). A stream whose end comes with its process not reaped (its reader failed, the
  security review's finding) has the process's group killed then, for any native session, and
  a switch it ended is given up, the session staying stopped and native, as it is when a stop
  was asked or made (the person's `k`, a space's close, a worktree's removal, the second
  security pass's finding), an exit or a restart was asked, or an orchestrator's replacement,
  while the process ended;
  the transcript and the echo area say why. A switch under way holds `oso-restart` (part 4b),
  so that the restart carries the TUI, not a session half way.
- **The TUI** resumes the conversation in a new terminal, `*claude: NAME*`, made stopped and
  started as `session-resume` starts a stopped terminal session, with `--resume ID` and what
  the session held (Q4-7 (a), amending ADR 0121): `--permission-mode` with the mode its next
  process would have started in (part 3b; none if nobody switched it and the CLI said one
  that asks less, as part 3b carries a mode), and `--model` and `--effort` as chosen (part 3c).
  A TUI resume of a `-p` conversation takes it up under the same id, its earlier turns shown,
  and honours `--permission-mode` (measured 2026-11-23). No `--permission-prompt-tool`: the
  TUI asks in its own screen. Claude Code's own trust question may show there, as `-p`
  skipped it. ADR 0121's screen rules and the tracker's screen entry take over from its start,
  and agents read the terminal only once the person lets them, as any session's (ADR 0120,
  Q16): nothing of the transcript's leave is carried, as there is none. The record says
  `tui` from the hand-off on, so oso's next start brings the session back as a stopped
  terminal session. What the native session held goes to that first TUI start alone, kept
  nowhere: a later resume of the TUI session, after a refused start, its exit or oso's next
  start, starts as any TUI session does, in the mode the person's Claude Code settings give it
  (ADR 0121), and `session-resume` of it turns the agent channel on, as any session's start
  does. Each window of the frame's tree that showed the transcript shows the terminal, the
  side window and docked ones apart, and the resume shows it in the selected window, as
  `session-resume` does; the transcript's docks go with it (ADR 0170).
- **The transcript** (Q4-5 (a)) is set aside as `*native: NAME*`, its rows and composer kept,
  whole, for part 4d-2 to resume in, with a row "In the TUI since here, in *claude: NAME*." It
  stays read-only and withheld from agents, as every native transcript is, its modeline says
  `in the TUI`, and its keys and its composer's, folding and moves among them, only say "This
  session runs in the TUI now: its transcript sends nothing"; it is read as text (search,
  scroll, copy). A draft left in its composer still refuses an `oso-restart` (part 2a), whose
  question says to clear it. It goes when its buffer is killed, or once its session is gone,
  and is neither carried over an `oso-restart` nor brought back at oso's next start, as no
  native transcript's rows are: the session comes back a stopped TUI one.
  `Editor::rename_terminal_buffer` now renames a native session's buffer too, which also gives
  ADR 0155's rename of a native session its buffer's new name, which it kept before.
- **A refused start.** A TUI start that is refused (its directory gone, the conversation live
  elsewhere, which offers a fork as a resume does) leaves the session a stopped TUI session:
  `session-resume` starts it again.
- **New public items**, approved by the author through the orchestrator:
  `SessionRequest::SwitchMode(BufferId)`, and `Editor::rename_terminal_buffer` taking a native
  session's buffer (its signature unchanged).
- **Built with:** `sessions/native/switch.rs` (the rules, pure, with their tests),
  `app/sessions/hosting/switch.rs` (the command and the sequence), `native/switching.rs` (what
  the rules are given, the end for the switch and the transcript set aside), the end of a native process in
  `native/end.rs`, a `Handed` on `TerminalOrder` passed by `launch_of` (`sessions/start.rs`),
  `sessions/command.rs` passing a mode, model and effort for any line it is given them for,
  `kill_unreaped` on the stream, and the e2e flows `sessions::native::switch`: a `-p` fake
  that ignores `SIGTERM` and takes 2 s to exit, and a TUI fake that records whether that exit
  came before its start; the transcript set aside refusing its keys and its composer's send;
  and a transcript killed while its `claude -p` ends, which starts no TUI and archives the
  session.
- **Not done:** a test of a switch given up as a reader failed or a stop was asked (rather than
  made) while the process ended, as the app has no harness of its own beside the end-to-end
  flows, which cover a stop made.

### Part 4d-2: a TUI session taken back natively

The author's (a) on Q4-6 and Q4-8 of 2026-11-23, through the space's orchestrator: the other way
of "One session, and the TUI as a hand-off" (Q3).

- **`session-switch-mode`** in a TUI session's terminal: `C-c C-t` there (session mode's key,
  as the Agents menu's head `t` is `C-c a t`), or `C-c a t`. A native session's transcript and
  composer keep `C-c C-t` for the to-do strip (part 3e): session mode's `C-c C-t` is not bound
  there (`NOT_NATIVE`).
- **When.** While the TUI's screen says it is idle, done or ready for review (ADR 0121's
  tracker), or while it is stopped; not while it works or a dialog asks, while its state is not
  known yet (a TUI just started), while it is being started, nor while a resume is held (a stop
  asked, oso waiting to quit or restarting, an orchestrator's replacement). Only where a native
  session may run (Q5): its project's root trusted now, by an open buffer of it or the record of
  trusted projects (part 4a), judged before anything changes, and again once the TUI ended
  (the resume then may be judged by its worktree's claim, "Worktrees' resumes");
  else it says why ("Session NAME stays in the TUI, as a native session runs only at a trusted
  project's root: …"). A running TUI whose process id is not known (its start outlasted the
  start thread's wait) does not switch: its end could not be waited for.
- **Never two writers.** A session keeps its TUI's `claude`'s process id and start time, read
  on the TUI's start thread just after the spawn (`Hosted::tui_process`, in memory only), until
  its terminal's end is seen reaped (`End::Exited`) or a native resume saw it gone. `oso-term`
  says a terminal ended once its pty did and gives up waiting for its child 2 s later, and a
  kill of its buffer stops the session before its hang-up goes, so a `claude` that ignores
  `SIGHUP` may still run after the session shows stopped: on Linux once it closed its
  descriptors of the terminal, as its pty then ends; on macOS, where a session leader's
  controlling terminal stays open until it exits (seen in the e2e flow), only after a kill. So every native resume of such a
  session, the switch's or a later `session-resume`'s, waits first, on its start thread, before
  the directory's and the registry's looks (`sessions::start::gone`): at most 10 s, until no
  process with the id started when it did (a process id is reused only once reaped, and two
  reads of a start time agree, within a second on Linux; on macOS a change of the wall clock
  moves neither; with no start time known, any process with the id counts). A TUI carried
  over an `oso-restart` is waited for by the id and start the restart carried; it is judged
  carried (ADR 0120, step 11) only while that `claude` runs: the native `claude -p` the switch
  starts, as any started in the new instance, is oso's own child, whose bridges are let in as
  any such child's (`Hosted::started_here`). Past the bound the
  resume is refused, saying why, the session stays stopped, and its next native resume waits
  again; meanwhile, as any start does, it holds `oso-restart` and an exit that waits (part
  4b), up to those 10 s. The record says native only once the native process started, so a
  resume refused leaves a TUI session at oso's next start. A switch under way holds
  `oso-restart` too. The security review's first pass found the stopped TUI and the clock
  cases.
- **The hang-up.** A running TUI is hung up as oso's stop hangs a terminal up (a TUI exits 0 in
  1.4 s, measured), but not noted as a stop, so that a stop made meanwhile, by the person, a
  space's close or a worktree's removal, gives the switch up, as a resume held does (a stop
  asked, oso waiting to quit or restarting, an orchestrator's replacement): the session stays
  a stopped TUI one. A kill of the TUI's buffer meanwhile archives the session and ends the
  switch.
- **Where it resumes.** Once the terminal said its program ended, after the event (the end is
  said as the terminal's handle is dropped), its terminal goes (the session's buffer moved
  first, so that the kill archives nothing), and the session resumes natively in the
  transcript set aside (part 4d-1), which takes `*claude: NAME*` back with a row "Back from the
  TUI, in mode M.", or, after an oso restart, which carries no transcript set aside, a new one,
  "Stopped. Its conversation is kept by Claude Code." It resumes as `session-resume` resumes a
  native session (part 2b), the composer selected, in the mode, model and effort oso held for
  it from before the hand-off, which the row names; what the person changed in the TUI is not
  read back, as no stream says it (an item the security review named for the author). Each
  window of the frame's tree that showed the terminal shows the transcript, the one set aside
  before the terminal goes, so that none passes through what a kill would show, or the new
  one after it; the resume shows it in the selected window, as `session-resume` does, its
  composer docked and selected. A TUI the hand-off started that exits of itself comes back
  so too, in the selected window whatever it showed.
- **A TUI the hand-off started that exits of itself** with status 0 (`/exit`), not stopped by
  oso or the person, not failed, and with no resume held, comes back natively at once (Q4-6
  (a)), the same way, its process reaped. Only a TUI started by a hand-off in this instance:
  one carried over an `oso-restart`, or resumed by `session-resume`, stays a TUI session when
  it exits.
- **Built with:** `sessions/native/switch.rs` (the TUI's rules, `TuiAt`, the phases, a native
  start due after the event among them, and the hand-off's TUIs, pure, with their tests),
  `sessions/start/gone.rs` (the wait, with tests over a stand-in for the system's start times:
  gone, another's, still running, a second off, unknown, the clock set back, the bound),
  `Ready::child_started`, `Hosted::tui_process`, `NativeOrder::tui_to_end`,
  `app/sessions/hosting/switch/back.rs`, the transcript taken back in `native/switching.rs`,
  the terminal's start and end in `hosting/terminal.rs`, `take_back_due` after the terminals'
  sync in `app/events.rs`, and the e2e flows `sessions::native::switch`: a TUI fake that
  ignores `SIGHUP` for 3 s, whose exit came before the native `-p` started (the `-p` fake
  counts the TUI exits at its start), the transcript taken back and answering; the same TUI
  stopped with `k` first, shown stopped a second before it ends, then switched; its buffer
  killed while it ends, which starts nothing and archives the session; and a TUI the hand-off
  started that exits 0 of itself. `sessions::native::carried`: a TUI carried over
  `oso-restart`, switched, whose `claude -p` asks the person through its bridge and gets a
  card (its claim was refused before `Hosted::started_here`, so every call was denied), then
  has its composer docked; `sessions::native::split`: a split switched both ways (back by the TUI's
  own exit, which takes it back as `C-c C-t` does), both halves following the session, and
  its composer docked and laid out inside a window that shows the
  transcript, which the orchestrator's switch (ADR 0176) checks too. Which windows follow is
  `tree_windows_showing` in `hosting/switch.rs`, with its test.
- **Not done:** app-level tests of a switch given up by a stop asked or a restart asked, and
  of trust taken back while the TUI ends, as the app has no harness of its own beside the
  end-to-end flows; and an end-to-end flow of a carried TUI resumed as a TUI, whose channel
  bridge `Hosted::started_here` lets in unasked too (the table's test covers the rule). The
  trees kept behind the frame (a zoom's, other spaces', the review's layout) do not follow a
  switch: a window there that showed the session keeps the transcript set aside, or after the
  way back what a kill shows. No flow covers a split whose start is refused at once.

### Part 4e: native is the default

The author's Q4-9 (a), and two answers of 2026-11-23 through the orchestrator:

- **The default.** With no `[sessions] mode`, or `mode = "native"`, a session runs native
  wherever it did under `mode = "native"` before: a new start in a trusted project (the
  person's `claude`, `session-worktree`, an agent's `session_start` and
  `start_worktree_session` with a brief) and a fork; and now an adoption, below. `mode = "tui"`
  opts out: each of them hosts Claude Code's TUI in a terminal, as before step 1. A restricted
  project, a directory below a project's root, and an agent's start with no brief keep the
  terminal (Q5). A space's orchestrator keeps it too, only because a space's directory has no
  `.git` yet and so is no project oso trusts: nothing else excludes it. The author wants
  orchestrators native as well; that needs the space's trust, its directory and the
  orchestrator's tools and flows, and is a follow-up of its own (amended by
  [ADR 0176](0176-native-orchestrators.md): new orchestrators follow `mode` under the space's
  gate since its part O-3). The configuration keeps
  `Sessions::tui` in place of `Sessions::native`, so a configuration not read in time, or
  none, gives native too.
- **A wrong value** (`"chat"`, `"TUI"`, `true`) is said as a problem of the configuration, and
  the default, native, holds, as a wrong value of any other key leaves its default (the
  author's answer). Before this part it kept the TUI.
- **Adopting** (`session-adopt`, and `move_to_space`'s resume of a conversation from outside
  oso, which adopts it): native as a fork is, at the root of a project trusted now, by an open
  buffer of it or by the record of trusted projects (part 4a), in a new buffer, with the
  composer selected when it is shown, and one "Adopted." row for the turns before, which
  `-p --resume` does not write. Its conversation is there, so it may be resumed again. It still
  takes its own baseline (ADR 0121) only where an open buffer gave its trust, as a terminal's
  does, and at its gate, the root the start thread looks at again, so that taking the
  project's trust back stops that run as it stops the start (the security review's finding:
  a second root from the same project would have kept it running); one the record judged
  takes none. Its gate is its tools' trust too (`hosting.trusts`), as a native fork's and a
  new start's are, so its agent's worktree tools judge its project by it, where a terminal
  adoption's tools have none of their own. The row says that the earlier turns are not shown
  here, which the TUI showed: a person may otherwise go on with a conversation they cannot
  read (the security review's finding). Since part 5a the earlier turns are shown, read from
  the transcript, and the row says "Adopted." after them. Which of the two it runs as depends on the configuration and the
  record, so, as a fork does, an adoption now waits whole for the configuration's first look
  and the record's first report (ADR 0173, part 4a); before, its order waited. One that runs
  elsewhere is refused and offered a fork, as before. With no `mode`, a fork or an adoption
  right after oso starts, with no buffer of its project open, so waits for the record's first
  report too, within the held starts' 5 s; under the TUI before, a fork did not.
- **What keeps its mode** (the author's (b) on whether a resume follows the default): a
  session the record holds resumes as it ran, a stopped TUI session in its terminal and a
  native one natively, under `session-resume`, an orchestrator's `session_resume` and
  `move_to_space` of a session oso hosts. Only a session with no mode yet follows the default.
  A TUI session carried over `oso-restart` stays one (ADR 0133). `session-switch-mode`
  moves one by hand, either way (parts 4d-1 and 4d-2).
- **Tests.** The setting's values and wrong values (`config::sessions_tests`). End to end,
  every native flow now runs with no `mode`, and the TUI flows' configurations (in
  `sessions`, `worktrees` and `starts_wait`) say `mode = "tui"`, so their behaviour does not
  change; `sessions::native::adopt` adopts a conversation in a terminal at a restricted
  project's root, natively once it is trusted, and in a terminal below its root. No test
  covers an adoption that waits whole, at the app's level or end to end.
- **The benchmark** (`examples/perf/bench/sessions.rs`) measures terminal sessions. Its project
  is never trusted, so they stayed terminals with no mode, but its configuration now says
  `mode = "tui"` (the author's approval), so that a later step that trusts its project does
  not change what its numbers mean. A benchmark of native sessions, the default path now, is
  wanted in a change of its own.

## Step 5, as built

### Part 5a: earlier turns from the transcript

The author's (a) on Q5-1 to Q5-5, Q5-7 and Q5-8, and the space's orchestrator's (a) on the
technical Q5-6, Q5-9 and Q5-10, on 2026-11-23. A native start whose rows hold none of its
conversation shows its earlier turns, read from Claude Code's transcript, with the row
renderer and folding of live turns: a resume in a new instance, the person's or an agent's
`session_resume`, `oso-restart`'s resume (part 4c), a fork, an adoption (part 4e), which says
"Adopted." after them, and a switch back from the TUI with no transcript set aside, as after an
oso restart (part 4d-2). A stopped buffer brought back at
oso's start keeps its one row until it is resumed (Q5-1). A resume whose rows hold the
conversation already, and a switch back into the transcript set aside, wait for part 5b.

- **Where** (a probe of 2026-11-23, CLI 2.1.330, Haiku, in an empty directory with no oso
  variables): `<config>/projects/<slug>/<id>.jsonl`, `<config>` being `$CLAUDE_CONFIG_DIR` or
  `~/.claude`, the slug the CLI's directory as it resolved it (`/tmp` gave `-private-tmp-…`),
  each character but an ASCII letter or digit a `-`, mode 0600. `--resume` appends to the same
  file. oso looks under the slug of the session's directory as given, then as it resolves, then
  in the first of at most 1024 project directories that holds `<id>.jsonl`.
- **The format.** A line of the conversation starts `{"parentUuid":`; any other is the CLI's
  own record (the account, requests, costs, modes, titles). A user or assistant line's `message`
  is the stream's, with the same ids, one line a block as on the stream; it differs in
  `toolUseResult` (the stream's `tool_use_result`), `thinkingDurationMs` and `compactMetadata`,
  and has no `result` line. So `wire/transcript.rs` reads a line with `wire/messages`, the
  stream's own reader. Attachments, also lines of the conversation, hold the account's email and
  organisation ids and system prompts: they are placed by their top-level fields, read by
  brackets and quotes without decoding (`history::scan`), and never decoded.
- **Which lines** (Q5-6, `sessions::native::past`): the chain from the newest line that shows
  anything, back through each line's `parentUuid`, and across a compaction its
  `logicalParentUuid`, so that a branch a rewind left is not shown. A parent is found only
  among the lines before its line, so the chain ends. Of it, the newest whole turns within
  8 MiB of their lines and 2000 lines, the newest line whatever its length; a cut starts at a
  message of the person's, and a note before the rows says how many turns are left out and that
  `session-search` shows the whole conversation (Q5-2). A chain whose oldest line names a
  parent not read, as one before the part of the file read or one that did not read, is cut
  there too. At most 131,072 lines are placed, four times the longest transcript measured.
- **Whose words** (Q5-3): a message is the person's where the transcript marks it: its
  `promptSource` is `sdk` (oso's sends), `typed`, `queued` or `suggestion_accepted` (the TUI's),
  it is not `isMeta`, and its `origin`, where it gives one, is `human`. Measured over 300 recent
  transcripts, every other user line with text was the CLI's own, a task's notification, a
  wrapped command or a message from another session. A message from another session is a note;
  any other shows what the stream's replay of it would: a command's output a note, a wrapped
  message nothing. As the file is the user's, which a session's own tools can write, the note
  before the rows always says they are as Claude Code's transcript records them.
- **Rows.** Replies, thinking and tool calls are rows as live ones, folded and grouped by the same
  rules; a call whose result is not in the transcript is unfinished. A compaction is a
  `Compacted` row. No turn has a result row, as the transcript keeps none (Q5-9). The notes oso
  said meanwhile ("Resumed.", "Resumed after oso-restart.", "Forked from NAME.") are drawn after
  the earlier turns; a start that does not run draws them at once. The rows added are drawn in
  one change of the buffer. Whole replies are highlighted as live ones (part 2g), the newest
  first.
- **Subagents** (Q5-4): a subagent's lines are not in the main file but in
  `<id>/subagents/agent-<agentId>.jsonl`; the main file has its `Agent` call and its result,
  which is what is shown. Its own rows wait for step 5's subagent rows.
- **A fork** (Q5-5): `--fork-session` writes its own file only at its first message, a copy of
  the parent's lines under the same ids, then its own turn. A fork shows the turns it copies from
  the parent's transcript, then "Forked from NAME.".
- **Where it is read** (Q5-10): once the process is spawned and before its stream is read. The
  plan read it on the start thread itself; the security review found that a transcript on a
  mount that hangs would then hold the spawned process with no owner for as long, so it is read
  on a thread of its own, `oso-native-past`, that the start thread waits for at most 1.25 s, so that a
  transcript on a mount that hangs leaves the process unowned no longer; then nothing is shown,
  and the thread is left. It is handed on first through the stream's own delivery
  (`stream::Out::Past`), so it is drawn before anything the process says, and the read is hidden
  under the CLI's start (0.24 to 0.57 s measured in step 1). At most the transcript's last 128 MiB are read, up to the
  length it had when opened, lines over the stream's 16 MiB limit not read, within 1 s; then only
  the lines shown are read again and decoded. Transcripts on the author's machine: a median of
  1.1 MB, p90 6.2 MB, p99 26 MB, the largest 68 MB (32,000 lines, its longest 1.4 MB).
- **Trust.** It is read only as a regular file of this user's with one name, opened without
  following a link, in a project directory that is not a link, and only if the first line
  naming a directory names the session's, as given or resolved; a transcript naming none is
  not read. Nothing of it is logged; `Debug` gives counts.
- **A missing or malformed file** (Q5-7): the session starts as it would have, and one note says
  why none is shown (not found, not a regular file of this user's, another directory's or none's,
  too slow, too many lines). A line that does not read, its fields or its whole, is left out,
  and one note counts them; an attachment is never decoded, even to be left out.
- **Tests.** `wire::transcript` (placing, the person's marks, side chains, compaction, attachments
  left undecoded), `native::past` (the chain, a rewound branch, a compaction, the budget and its
  cut, the notes, the recorded transcript's rows, an unfinished call), `sessions::past` (the slug,
  the scan, another directory, a link, a second name, a partial and a bad line, the deadline), and
  the fuzz target over the transcript's lines. The fixture `fixtures/2.1.330/transcript.jsonl` is
  this probe's, scrubbed of ids, paths, attachments' contents and the account.
- **Not done:** one reader of the transcripts' format and one way to find a transcript, shared
  with `session-search` (ADR 0145) and the worktree brief: `history::scan`'s field reader and
  `history::held`'s line reader and slug are used from there for now, and a third, narrower
  lookup of `<id>.jsonl` sits in `sessions::past`. A measure of the UI thread's draw of 2000
  rows. Turns after the rows a buffer keeps are part 5b's.
- **New public items:** none.

### Part 5b: the turns after the rows kept

The author's (a) on Q5-8, on 2026-11-23: rows that hold the conversation are given what the
transcript holds after them, when the conversation may have gone on elsewhere since.

- **When.** At a resume of rows that hold the conversation, once its process ended other than to
  resume at once (an interrupt's resume and a mode's, which oso makes, read nothing, unless they
  fail and leave it stopped; so does a restart that stays here and resumes what it ended), and at
  a switch back from the TUI into the transcript set aside (part 4d-2), which then shows the TUI's
  turns. A resume that finds nothing new adds no row. Until a read succeeds, the next start asks
  again: a start that does not run, or a read too slow, loses nothing.
- **Where they follow on.** A user or assistant line's `uuid` is the same on the stream and in
  the transcript (the probe of part 5a), so the rows keep the ids of the session's own 64 newest
  lines, live or read: messages, prompts, notes from other sessions and compactions. An id the
  transcript gives twice is its first line's, so a copy written into the file later does not move
  where the rows end. The chain from the transcript's newest line is walked back only to the
  first of them it finds; the lines after it are shown, within part 5a's limits, after a note
  "Its turns since, as Claude Code's transcript records them". One found that is not the newest
  kept says the conversation went on from an earlier turn; none found, as after a rewind
  elsewhere, gives the newest turns, after a note that the turns above are no longer its
  conversation.
- **Quiet.** Rows that hold the conversation need no word of a transcript not found; one that
  did not read, names another directory, or took too long is said, "Its turns since are not
  shown: …", and lines that did not read are counted only after where the rows end.
- **Tests.** `native::past` (what follows the newest id, nothing new, an earlier id, a rewind, the
  notes, the ids the recorded rows keep), `sessions::past` (only the later turns read, a missing
  transcript said nothing of). End to end (`sessions::native::past`): a session handed to the
  switch fake's TUI, whose transcript gains a turn typed there, comes back with its own rows
  once and the TUI's turns after them, before "Resumed.".
- **Not done:** a local command run in the TUI or a peer's message there shows as its line in the
  transcript says, which the live stream may not have shown; the TUI's to-do list is not read.
  The read places the whole transcript again at each such start, though only what follows is
  decoded: hidden under the CLI's start for the median transcript, not measured at the largest.
- **New public items:** none.

## The rows' look, as built

The author found the transcript very hard to read, and chose on 2026-11-24: the look is ECC's, the
structure stays oso's. ECC was read as a reference; no code of it is copied. This replaces how a
row looked after ADR 0130 and steps 1 and 2; what folds, groups and opens stays. oso's marks
stand in the first two columns, 2 columns left of the author's mock-ups (the space's orchestrator,
for the reason under "Only oso's rows look so").

- **A blank line** sets a prompt, thinking, a reply, a run of tool rows, a note and a compaction apart.
  It is the first line of the row after it (`Row::gap`), so a row is still drawn where it is and
  nothing else. A turn's result follows its turn at once and ends with a blank line. `n`, `p`, `]` and
  `[` go to a row's first line past it, and on a blank line no key acts on a row.
- **A prompt** is after `〉`, its other lines in line with it, on a band to the window's edges ("The
  rows' colours").
- **A reply** has no mark: each line is two columns in.
- **Thinking** is `▸ Thinking · 4.2 s`, `… Thinking` while it streams.
- **A tool row** is its mark, a badge of one plain character, its name, then ` · ` and what it works
  on: `▸ ≡ Read · a.rs  48 lines`. The mark is `▸` folded, `▾` open, `·` with nothing to open, `…`
  while it runs, `✗` failed or denied, which the row says in words too. Calls share a row as before,
  as `Read ×3 · a.rs, b.rs, c.rs`. A call that waits for the person says `waiting for you`, until it is
  answered, also behind another's card.
- **A nested row,** a subagent's or a call of an open row of several, has oso's guide `│ ` in its
  first two columns (the space's orchestrator; not in the mock-ups).
- **A change** is open once it came, after "Added 4 lines, removed 2 lines": at most 40 lines and
  8 KiB, a number before each, the old file's for a removed line, and added and removed lines
  on the review's bands; `⋮` between hunks. Folded, its row says `+4 −2`. A later part's `v` opens the
  review. A hunk with no line is not kept.
- **A result** shows its first 12 lines, at most 8 KiB, where ADR 0130 and step 1 had 200 lines,
  else the first and last 50. `RET` shows a result or a change whole, as before.
- **A turn's result** stands at the right edge of the narrowest window that shows the session, so that
  it wraps in none. When the width changes, those in view are drawn again, the others as they come
  into view.
- **`AskUserQuestion`'s row** shows its first question; its input as JSON is gone. What a request is
  held against is as it was.
- **Only oso's rows look so.** Only oso's marks and guides stand in the first two columns of a line,
  at every depth, whatever a reply, a note, thinking, a result or a change says: a fuzz target holds
  it. On a tool row and in what it shows, what Claude or a tool says is dim and oso's words are not
  (a waiting call's, a failure's and a denial's have colours of their own), and runs of spaces in it
  are one, so it spells no outcome in oso's face. A changed line's number is not in the line's face.
  A band is drawn where a line starts in a highlight that only `sessions::native::render` gives, in
  a session's buffer, never from what a text holds; a tool's name is in one that no parser gives.
  These were `*Spaces*`' highlights, until the session's own ("The rows' colours"). With
  `global-font-lock-mode` off there are no bands or faces, and the columns still hold. A wrapped
  row's columns are "Long lines"'.
- **Measured** (release, this Mac, other builds running, a throwaway test run twice each way in turn;
  10,000 rows, about 1,700 edits of 20 lines among them, 2.0 MB of text where it was 0.4; a 60 by
  100 window that wraps), before and after:
  - a streamed delta folded and drawn: p50 6.5 and 7.4 µs, p99 10 to 19 and 10 µs;
  - an edit's result, drawn open now: p50 9.3 and 13.6 µs, p99 10 to 24 and 16 to 23 µs; at its
    bounds 30 and 87 µs; 1,024 of them in one batch 16 and 23 ms;
  - a change of 20,000 hunks with no line 44 and 33 µs;
  - the window's rows laid out: p50 22 and 23 µs, p99 29 to 34 and 27 to 29 µs.
- **New public items:** none. Bold and the header and footer are later parts.

### The rows' colours, as built

The plan's part E, which the author approved with its mock-ups on 2026-11-24. The colours are oso's
own, in its three themes; none is ECC's.

- **A prompt** is in `SessionPrompt`, the text's colour on a green band of its own, apart from the
  bands of a change; what its lines start with is in `SessionPromptMark`, which the band is drawn
  by. It is drawn as typed: its Markdown is not read.
- **A tool row:** its name and count are in `SessionTool`, a pink; its badge is in one of eight
  `SessionIcon…` classes by what the tool does (reads, writes, runs commands, searches, runs an
  agent, reads the web, keeps tasks, anything else), on a chip over its cell and the space after it.
  The badges stay plain characters, as before. A mark of a call or of thinking under way, `…`, is in
  `SessionRunning`, a green; of a call that waits for the person it stays quiet, and its words
  amber.
- **A reply:** inline code, and a fence's in a language oso does not read, are in `SessionCode`,
  violet (a pale cyan in `dusk`, where no violet reads at 7:1), with no background; what stands for
  a bullet, a list's number, a quote or a rule is in `SessionBullet`.
- **No class of a reply is a row's.** Of what a reply's text can be in, its Markdown's classes and
  every one a parser gives, none is in a row of oso's, a card's frame or a key. A test holds it,
  over `match`es with no wildcard of every class and every face of a row, so that a new one must be
  placed; the fuzz targets hold that each class of a row is on its rows only. The themes' tests hold
  it for colours too: a tool's name, a prompt's and a brief's mark and a running mark are at least
  0.05 apart in OKLab from every syntax colour, code and bullets, and a tool's name 0.09 from inline
  code, which a light theme's looked like. So the dark theme's pink is lighter than the mock-up's,
  which is its functions' own.
- **Contrast:** a prompt's words and inline code at 7:1, code on the current line too; the others at
  4.5:1, a badge on its chip. Escapes, whitespace's and wraps' marks are checked on the prompt's
  band.
- **A chip** is drawn where a badge's class is near the start of a line of a session's buffer, with
  nothing but a row's classes before it on that line, two cells wide, on the line's first row; it
  scrolls sideways with its text. What a text holds is never looked at.
- **An agent's brief** (the author's OK of 2026-11-24): what an agent's tool started the session
  with was drawn as the person's. It is `@  from session NAME`, then its lines three columns in,
  with no `〉`, in `SessionBrief` on a violet band, its mark and heading in `SessionBriefMark`: 0.09
  or more from the person's band. `NAME` is its parent's name as the row is made; else it says
  `another session`. `n` and `p` stop at it as at a prompt.
  - Live, it is the first message of a start by an agent's tool, asked or not (`Hosted` table's
    `brief_from`).
  - A transcript does not say whose a message is, so there it is the conversation's first one, in a
    read that starts there and is not cut, when it starts as oso starts one
    (`sessions::worktree::brief::form`): with oso's line of a start in a directory, whatever the
    session's origin, or with a worktree's header where the origin is an agent's, as the person
    hands off to a worktree too.
  - So a fork of a worktree's session, which oso has as the person's, shows it as a prompt once it
    is read again, as before; and what oso composes for the person's own start, such as an
    orchestrator's take-over, stays after `〉`.
- **Measured** (release, this Mac, a throwaway test run twice each way in turn; the 10,000 rows of
  "The rows' look", a 60 by 100 window that wraps), before and after:
  - a streamed delta folded and drawn: p50 7.3 to 7.5 and 7.3 µs, p99 10 and 10 µs;
  - an edit's result: p50 12.7 and 12.7 µs, p99 14 to 15 and 16 to 18 µs; 1,024 of them in one batch
    21 to 22 and 21 to 22 ms;
  - the window's rows laid out: p50 23 and 23 µs, p99 29 to 31 and 28 µs;
  - a frame's fills, 58 rows in view, every one a tool's: p50 10 and 18 µs, p99 11 to 13 and 21 to
    22 µs.
- **New public items** (the author's OK of 2026-11-24): `oso_core::syntax::Highlight` and
  `oso_render::Face`, each with `SessionPrompt`, `SessionPromptMark`, `SessionTool`, `SessionCode`,
  `SessionBullet`, `SessionRunning`, `SessionBrief`, `SessionBriefMark` and `SessionIcon{Read,
  Write, Shell, Search, Agent, Web, Task, Other}`.

## Long lines, as built

The author found a transcript whose long lines are cut at the window's edge unreadable, and
chose on 2026-11-24:

- **A transcript, its composer and `*result: NAME*` wrap from when they are made**
  ([ADR 0100](0100-soft-wrap.md)), for native sessions only. That covers a start, a resume, a
  fork, a session brought back, an adoption, a switch from the TUI and the earlier turns.
- **`toggle-truncate-lines` still turns each off,** and what the person chose stays for as long
  as that buffer lives: a `*result*` shown again for another call keeps it.
- **Not the card or the strip,** which oso draws to their window's width: a card that wraps takes
  no answer (part 2e-2). Nor a plan's edit, `*plan: NAME*`, which was not asked for.
- **The composer's rows** were counted as wrapped since [ADR 0170](0170-docked-windows-under-a-sessions-transcript.md),
  so its window grew for text that it then cut. They agree now: a composer whose lines the person
  truncates takes as many rows as its lines.
- **A restart** makes a transcript and a composer again, wrapped, whatever the person chose there;
  a result's buffer is made at the next `RET`. Transcripts and results are internal and never
  carried. A composer's entry in the state stands for `*scratch*`, which it must not wrap, so it
  says nothing of it, and an entry that does not say it wraps leaves the buffer as it is.
- **The first text in an empty wrapped buffer,** as a fork's earlier turns or a yank into the
  composer, is wrapped at once only within the budget, and otherwise provisional until the job
  reports, as any edit; it was all wrapped on the UI thread.
- **Measured** (`text_perf -- transcript`, a frame of 64 by 100, the composer docked and selected,
  M-series Mac): a batch that draws the growing last line again, its rewrap, the frame's rows and
  the jobs asked for take 0.07 ms at p50 and 0.15 ms at p99 wrapped, 0.02 and 0.03 ms truncated,
  at 100 rows and at 10,000. On a line that grows to 1 MiB they take 0.40 and 0.53 ms, 0.18 and
  0.31 ms truncated. 10,000 rows of earlier turns, put in at once, take 6.6 ms, 6.8 truncated:
  the text's own cost. Back in the window they wrap in 0.1 ms, the rest on the worker.
- **New public items:** none.

### Wrapped rows line up under their row

The author chose on 2026-11-24, for a native transcript only, that the rows a line goes on in are
indented, so that they read as part of their row:

- **The indent** is the line's leading U+0020 spaces, then up to four times a leader and the
  spaces after it. A leader is one display cluster that starts with neither a letter, a digit nor
  white space, or 1 to 9 digits and `.` or `)`; a space must follow it. So a reply goes on at
  column 2, its `- ` item at 4, its `1. ` item at 5, a `│ • ` one at 6, `▸ ≡ Read · …` at 4, and
  code at its own spaces. No other space counts, as oso writes these, and no list of leaders is
  kept. A space that a combining character joins counts as any.
- **At least 2 columns, whatever a line starts with, and at most half the row's width,** or less
  where the widest cluster of 8 cells ([ADR 0104](0104-soft-wrap-drawn-in-view-rows.md)) would not
  fit after it. So that a call in a nested row keeps its 6, a transcript wraps from 15 text
  columns, where another buffer does from 10. Narrower, it is as a window whose lines are
  truncated; a split or a resize still goes down to 10 ([ADR 0112](0112-window-splits.md)), as it
  goes down to where a window with a wide gutter does not wrap.
- **Such a row holds as many cells fewer.** A row does not break before the column the next one
  stands at.
- **The text is as it was:** the indent is no text. Copying, motion, `C-a`, the cursor, regions,
  matches, the agent channel's and the dev socket's rows have none of it; a region does not fill
  it. The mark `↩` stays in the last column. A preedit would not count in its line's indent, but
  a transcript takes no typing.
- **An edit of what the indent is read from** lays the line out again from its start, and a rewrap
  stops within a line only where its indent is as it was before; a job's, only at a line's start.
- **Code:** in `display_map::wrap_map`, the private `fit.rs` (a window's layout) and `indent.rs` (the
  rule, read from the line's start, at most half a row of it, once a line that wraps is laid out
  and once a line in view a frame). A screen row has its indent, crate-private, and
  `Screen::origin` is its column less that; the renderer lays such a row out in a region that
  starts past its indent.
- **Tests:** the property tests of the WrapMap and of the cursor's place run with rows indented and
  not; the rule's table and that it never gives less than a row's prefix, whatever follows; lines of
  many shapes at widths from 10 to 100; rows not yet wrapped again; a text-grid snapshot of a
  transcript with the cursor, a region and matches; the renderer's cells.
- **Measured** (`text_perf -- transcript`, as above): a batch takes 0.09 ms at p50 and 0.17 ms at
  p99, 0.02 ms more than with no indent, at 100 rows and at 10,000, and 0.40 and 0.54 ms on a line
  that grows to 1 MiB, as with none.
- **New public items:** none.

## Remote Control is off, as amended

Amended on 2026-11-24, the author's choice. Claude Code's Remote Control connects a session to
claude.ai, which then stores the conversation and lets the person's other devices send messages,
change its permission mode and stop it. oso has no part of that yet: its rows, its cards and its
rules about who changes a mode (Q4) know the person at oso's own window only. The CLI connects a
`claude -p` that is asked on its standard input (`remote_control`), which oso never asks. So
that no setting and no plain request connects one, oso starts it turned off until a feature of
its own asks for it, in an ADR of its own.

- **What is blocked.** Every native start, a new one, a resume (after an interrupt, for a mode,
  by `oso-restart`, with a repository added), a fork, an adoption, a switch from the TUI and an
  orchestrator's in a space's directory, has `"disableRemoteControl": true` in its one
  `--settings`, beside the steering hook when `[sessions] steer_worktrees` is on. All build
  their line in `sessions::start`, which sets it for what runs natively, whatever the order.
- **What is not.** A terminal's line never has it, the TUI a native session is handed to among
  them (part 4d): there the person's own settings decide, as `remoteControlAtStartup`, and
  `/remote-control` works. A `claude` that a session itself runs reads its own settings, and
  oso's one-shot runs (a space's name, a dictation's fix) are no sessions and take none. It is
  a setting of the process, and no boundary against other processes of the same user.
- **Measured** (2026-11-24, CLI 2.1.330, Haiku, an empty directory, no variables of oso's or
  Claude Code's; whatever asked to connect ran with no network, so that none could). With the
  setting, `initialize` answers `remote_control_available: false`, and `remote_control` is
  refused: "Remote Control is disabled by your organization's policy", `bridge_state` `failed`.
  Without it the same request went on to make a session, which failed for want of a network. It
  held for a new session, a resume and a fork; over a project's and a local
  `disableRemoteControl: false`; with `remoteControlAtStartup` true in the person's, the
  project's and the local settings; with `--setting-sources=user` over a project's `false`; and
  with the hook beside it. `--remote-control` with it connected nothing. `/remote-control` sent
  as a message "isn't available in this environment", with or without it.
- **What the start held still works,** measured with network and the setting: the steering hook
  of the same JSON denied `git worktree add` on a new session, a resume, a fork and with
  `--setting-sources=user`; a project's own `PreToolUse` hook ran beside it; and a
  `SendMessage` from one such session reached another by name, as it goes between this
  machine's sessions, not over claude.ai.
- **Not measured:** the TUI resuming a conversation that ran with the setting, which would
  connect by the person's own setting; a managed `disableRemoteControl: false`; a later CLI,
  which could change the setting's meaning.
- **Tests.** `sessions::start::line_tests` (every kind of native start, gated or not, has it,
  with the hook beside it; no terminal's has it, handed or not), `steer`'s (the JSON, and that
  it needs no path in UTF-8). End to end: the line `claude` was started with, of a new native
  session, a switch to the TUI and back, and a space's new orchestrator.
  - Erratum (2026-11-24, ADR 0181's part RC-1): the JSON's tests are `sessions::settings`', where
    `steer`'s settings moved.
- **New public items:** none.

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

## Following the end, as amended

Amended on 2026-11-24 (the author's): part 1b-1's "a cursor at the end follows new rows". Only the
selected window's cursor stayed at the end, so nothing followed behind the composer, a card or a
prompt, or in another window; a window that first showed a transcript started at its top, and a
selected cursor in a row drawn again went to that row's end. What holds now:

- **Which windows follow:** each whose primary cursor is at the end of the text, selected or not.
  Its cursor stays at the end as rows come and its view goes with it at once. Any other window is
  not moved: its cursor and its top follow as markers, also in the selected window, as a change of
  a row begins where its text first differs; a cursor that a change above it takes out of view is
  kept in view as any is (ADR 0112). Point says it, so whatever moves point off the end stops it;
  selecting the composer or a card, or a card that appears, moves no point. Point on a queued
  message's row stays on that message as the queue is drawn again, or goes to the end with it
  gone. Beneath a preedit or a prompt, where the wheel leaves the cursor (ADR 0069, ADR 0072), the
  view follows rows and their wrapping only while its last row shows; a frame laid out again
  still shows the cursor.
- **Where a window starts:** one that never showed the transcript at its end, not where another
  window is in it; one that left it at its end, behind another buffer or in a window tree kept
  behind the frame, at the end it has now; any other where it left it. `C-x b`'s preview shows
  what would, in the window that would. `C-g` in an isearch begun at the end goes back to the end
  it has now. A split keeps the place.
- **What starts it again:** point at the end; the wheel or `C-v` that moves the view down to where
  the last row shows, which puts point at the end unless a region shows or there are several
  cursors; and a message
  sent or queued, which puts the window that carries the composer at the end.
- **The bottom of the screen:** the end is the empty row after the last one, the queue's rows
  included. The view moves by the fewest view rows, in the rows its docks leave it: a dock that
  changes lays the frame out again, and a wrap's report follows too.
- **Cost:** a changed row costs its own text and, for each window on its buffer, O(log n) and what
  it wraps in view, within one wrap budget for each batch of the stream, which the selected window
  has first; nothing is done per row of the transcript. A hidden window costs nothing: it keeps
  one flag. In release, a row added with three windows on the transcript and the composer
  selected takes 4 µs at 100, 10,000 and 100,000 rows, and wrapped 13 µs at 100 and 10,000 (p50,
  with the tests' small wrap budget).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0031](0031-claude-code-integration.md), "**Not used:** the `-p` control protocol, the Agent
  SDK, ACP, transcript parsing for the primary view": a native session runs `claude -p` with
  stream-json, and writes the five control requests of "The route" with their fallbacks. The Agent
  SDK and ACP stay unused. A native session's earlier turns are read from its transcript once, at
  a start whose rows hold none of them (part 5a), and the turns after its rows at a start whose
  conversation may have gone on elsewhere (part 5b); what it says live comes from its stream. "The conversation UI is Claude Code's own TUI, not a native oso view.
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
  person chose (Q4), and `--permission-prompt-tool`; a TUI session still passes none, but the TUI
  a native session is handed to (part 4d-1, Q4-7 (a)), which passes the mode the native session
  held, and its `--model` and `--effort`.
  "**`claude` (`Person`)** starts a session … The session's terminal is a buffer named
  `*claude: NAME*`", and "Adopting a session started outside oso, as built", "oso resumes the
  same id as a hosted session": at a trusted project's root, a new session, a fork and an
  adopted conversation, one `move_to_space` brings in among them, are native sessions in
  `*claude: NAME*`, unless `[sessions] mode = "tui"`; elsewhere, and for a TUI session resumed
  or carried over a restart, the terminal stays (part 4e).
- [ADR 0143](0143-agent-keys-on-c-c-a-and-an-agents-menu.md), "Keys planned elsewhere become
  heads of this menu: … ADR 0129's dictation on `t`": `t` is `session-switch-mode`'s (part
  4d-1, the author's Q4-8 (a)), and part 4d-2's `C-c C-t` in a session's terminal is too; ADR
  0129's dictation, deferred, takes another head when it is decided.
- [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md), a worktree's claim is checked
  on the `oso-project` thread for its open buffers: and, by the same check, on a native resume's
  start thread with no buffer open ("Worktrees' resumes").
- [ADR 0131](0131-an-orca-style-sessions-pane.md), "A second line, only for a session that needs
  input or failed (Q2)" and "Where recaps show": and why a stopped native session did not resume
  ("Worktrees' resumes").
- [ADR 0132](0132-a-session-mode-for-hosted-sessions.md), "Every lookup of \"the selected buffer's
  project\" or \"an open buffer of the project\" now counts a session's buffer too": a native
  session's transcript among them ("Transcripts count as open"), as in
  [ADR 0131](0131-an-orca-style-sessions-pane.md), "only under the trust of a buffer in it", and
  [ADR 0153](0153-agents-resume-sessions.md), "the session's own among them".
- [ADR 0133](0133-keeping-terminals-over-an-update.md), step 2, as built: "The host refuses
  as `check-handover` with terminals does …, then stops the agent channel and starts the
  `oso-restart` thread": while a native session has a turn under way, asks the person, is
  starting or is being interrupted, the host waits first, with no question, until they have
  been idle for 2 s, and `C-g` or `C-x C-c` cancels the wait (part 4b). Any session starting,
  ADR 0173's held starts, git's runs and a file operation hold it too; terminals still do
  not.
- [ADR 0133](0133-keeping-terminals-over-an-update.md), step 2, as built, "The order": before
  the thread starts, the native sessions that run are stopped and the state names them; the
  new instance resumes them once the instance going has gone, which `import-terminals` now
  says with no terminal too (a count of 0), and a restart that stays resumes them in the
  instance going (part 4c). "The new instance attaches the masters only once the pair has
  ended": it waits so for the sessions named too.
- [ADR 0120](0120-the-terminal.md), step 11, as built, "The order": `import-terminals {count}`
  goes with the masters, and also with none, a count of 0, when no terminal went but the
  state named native sessions to resume; either way it says that the old instance has gone
  (part 4c).
- [ADR 0133](0133-keeping-terminals-over-an-update.md), step 5, as built, "Waiting.": `w` and
  `quit-when-idle` also cancel a restart that waits, and `oso-restart`, waiting or beginning,
  cancels an exit that waits and resets its guard, so that only one of the two waits (part
  4b).
- [ADR 0130](0130-agent-chat.md), "each tool call one row (`▸ Edit src/foo.rs`, …)", "a result longer
  than 200 lines shows its first and last 50" and "each turn's result as a row": kept by reference, as
  "The rows' look, as built" has them.
- [ADR 0112](0112-window-splits.md), "An edit in one window moves the others' cursors and tops as
  markers": in a native session's transcript a cursor at the end stays there in every window, and
  the view goes with it. "A buffer a window never showed starts where the window selected most
  recently that showed it left it, or its start": a transcript starts at its end ("Following the
  end, as amended").
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md), "moves cursors into view": in a native
  session's transcript, the wheel or `C-v` down to where the last row shows puts point at the end
  ("Following the end, as amended").
- [ADR 0130](0130-agent-chat.md): superseded. Its buffer design holds by reference
  ("The buffers"); its route, its sidecar and its two views do not. "`claude` starts a TUI
  session as before; `C-u C-x a c` asks for the mode; `[sessions] mode = "chat"` makes chat
  the default (Q8)", Q8 (a), "The TUI; chat with `C-u C-x a c` or `[sessions] mode = "chat"`",
  and "the TUI by default": native is the default in a trusted project, and `[sessions] mode = "tui"` keeps the TUI; `"chat"` is no
  value, and no key asks for the mode (part 4e).
- [ADR 0173](0173-starts-wait-for-the-configurations-first-look.md), "Until the
  configuration's first look is done, every start above waits": and a native resume, a fork
  or an adoption with no open buffer of its project waits for the record of trusted projects'
  first report too, within the same 5 s bound (parts 4a and 4e). "`*Messages*` says once: \"The
  configuration was not read in 5 s…\"": only when the configuration was not read; a wait for
  the record alone says nothing there, and its refusal says why.
- [ADR 0173](0173-starts-wait-for-the-configurations-first-look.md), "**A resume or adoption**,
  native or in a terminal: … in its start order only. Whether it is native comes from the
  session's record", and "A resume or adoption makes its buffer at once and marks the session
  starting, as before. Only its start order waits": for a resume only. An adoption chooses
  native or the TUI by `[sessions] mode` and its project's trust, as a fork does, and so waits
  whole as a fork does, before its key is reserved and its buffer made; a second adoption of a
  conversation whose adoption waits is held too, and says "That conversation is being
  adopted" once it goes (part 4e).

- [ADR 0100](0100-soft-wrap.md), "**Off by default, for every buffer (author).**" and "Nothing
  turns it on by file name; modes do that from M3": except a native session's transcript, its
  composer and `*result: NAME*`, which wrap from when they are made ("Long lines, as built").
- ADR 0100, "**A continuation row starts at the left edge (author),** with no indent", and its
  alternative "Continuation rows indented as their line": in a native session's transcript they are
  indented ("Wrapped rows line up under their row"). Everywhere else they start at the edge. "A row's
  start depends on the row before it": and there on what its line starts with.
- [ADR 0104](0104-soft-wrap-drawn-in-view-rows.md), "**Below 10 text columns, nothing wraps**": below
  15 in a native session's transcript. "A row that continues draws its text in the wrap's width,
  and the mark in the column after": after its indent, which is in that width. "**A row draws the
  mark only while it fits** in the wrap's width": with its indent.
- [ADR 0102](0102-soft-wrap-what-building-it-changed.md), "A job rewraps from its start and goes on
  past its end until its rows meet the old ones": in a transcript, until a line starts. An edit's
  rewrap stops within a line there only if its indent is as it was.
- [ADR 0125](0125-a-rewrap-starts-at-a-row-that-reads-the-edit.md), "A rewrap also starts further
  back while a row before reads the edit's start", and its alternative "Rewrap from the logical
  line's start": in a transcript, an edit of what its line's indent is read from, at most half a
  row, starts at the line's start.
- ADR 0112, "fewer than 10 text columns, ADR 0104's narrowest wrap": a transcript between 10 and 14
  columns wide does not wrap.
- ADR 0100, "`oso dev` carries each buffer's setting to the instance that replaces it": not that
  of a buffer whose entry stands for `*scratch*`, as a composer's. ADR 0103's "a buffer that wraps
  says `"wraps": true`" is the same.

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
