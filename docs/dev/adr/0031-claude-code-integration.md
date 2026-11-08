# 0031. Claude Code integration: host the official TUI

- Status: Accepted, amended by 0085
- Date: 2026-11-03

## Context

M4 includes a Claude Code client and an MCP server for reviewing agent
changes ([`roadmap.md`](../roadmap.md)). The author's current Emacs client
taught several lessons:

- It drives `claude -p` with stream-json and the CLI's control protocol
  (permission callbacks, interrupt, side questions, usage). The control
  protocol's wire format is not published. The client carries about 127
  version-specific workarounds.
- It feels slow. Most of the cost is on the editor side: single-threaded JSON
  parsing, rendering and MCP tools on the UI thread, and CLI restarts.
- In May 2026, a planned billing change would have moved Agent SDK and ACP
  usage out of subscription limits. It was postponed, but it applied to the
  headless and SDK route, not to the interactive TUI.

Other integration styles were considered:

- **Terminal hosts** (herdr, rex) run each agent's own TUI in a PTY and add
  hooks, a status escape sequence (OSC 7501, a draft) and session restore
  around it.
- **ACP clients** (agent-shell, Zed) speak the open Agent Client Protocol.
  Claude Code is reached through a community adapter built on the Agent SDK,
  which is the same headless family as `-p`.
- **Own agent loops** (eca) call model APIs directly and do not use Claude
  Code at all.

Only Claude Code is needed for now. Other agents may come later.

## Decision

- **oso hosts the official interactive `claude` TUI** in a terminal view
  ([ADR 0015](0015-terminal.md)). Claude Code behaves exactly as it does in
  any terminal: every slash command, dialog and new feature works, and
  subscription login applies as usual.
- **Integration uses only documented extension points:**
  - **Hooks**: SessionStart reports the session ID and transcript path.
    Other hook events report state (working, waiting for input, waiting for
    permission, done) to oso over its Unix socket.
  - **OSC 7501** status sequences are accepted by the terminal view, so any
    program that emits them reports state the same way.
  - **MCP**: review, worktree and editor tools are served through an
    `oso mcp` stdio bridge that `claude` launches. The bridge connects to oso
    over a Unix socket with a peer user check. No TCP port is opened.
  - **Resume**: sessions are restored with `claude --resume <id>`.
- **oso owns the surroundings natively:**
  - a session list with state
  - notifications
  - worktree sessions
  - review of changes with comments sent back to the session as a bracketed
    paste
- **Not used:** the `-p` control protocol, the Agent SDK, ACP, transcript
  parsing for the primary view, and direct model API calls.
- **Agent-agnostic structure:** agent-specific parts (hook installation,
  resume command, state mapping) sit behind a small per-agent adapter. Claude
  Code is the only adapter for now.
- **Hook installation is explicit:** `oso integration install claude` shows
  the change to the user's Claude Code settings and writes it only after
  confirmation. oso never edits those settings silently.
- **The terminal moves into M4**, limited to what hosting agents needs:
  - PTY
  - rendering
  - input
  - searchable text
  - the escape-sequence policy

  The pop-up terminal and the compile backend stay after M4.

## Consequences

- No undocumented protocol to track, so CLI updates do not break oso.
- Subscription policy changes aimed at headless or SDK use do not affect the
  main workflow.
- The conversation UI is Claude Code's own TUI, not a native oso view.
  Permission prompts and dialogs appear inside the TUI.
- M4 grows by a scoped terminal implementation.
- The current client's session, worktree, review and notification ideas carry
  over; only the conversation transport is replaced.
- The MCP bridge and hooks add a local entry point that must be secured.
  Tools follow the trust model ([ADR 0009](0009-project-trust.md)), and there
  is no code-execution tool.
- Later options, each needing its own decision:
  - native permission dialogs through the PermissionRequest hook
  - a read-only transcript viewer that may break harmlessly
  - `claude -p` for one-shot features using only documented flags
  - an ACP client

## Alternatives considered

- **`-p` stream-json with a native UI (the current Emacs client's
  approach):** the richest editor integration, but it depends on an
  unpublished control protocol and the headless billing route.
- **An ACP client:** an open protocol with many agents, but Claude Code is
  reached through a community Node adapter on the Agent SDK. Built-in slash
  commands and new features lag, and it shares the headless billing route.
- **Agent SDK sidecar:** an official typed API, but it adds Node, npm and the
  same billing route.
- **Own agent loop calling model APIs (eca style):** violates the no-network
  rule ([ADR 0010](0010-process-and-network-policy.md)), loses Claude Code's
  features, and subscription use is unofficial.
- **A native conversation view built from the transcript file, with the TUI
  hidden:** the transcript format is undocumented. Output appears per message
  instead of streaming. TUI-only dialogs still need the TUI.
