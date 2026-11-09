# 0093. A `cancel` tool in `oso mcp`

- Status: Accepted
- Date: 2026-11-09

## Context

A `change` an agent makes can open a question for the person: saving a file it opened, or saving
over a file that changed on disk ([ADR 0085](0085-agent-channel-and-oso-mcp.md)). The call answers
at once, with `waiting for the person:` and the question. Until the person answers, the channel
refuses the agent's `run`, `open` and `insert`.

The channel has a method for this, `cancel`, which closes the innermost question the connection
caused. `oso mcp` gives it no tool. So an agent that changed its mind, or that learns the person
will not answer, cannot take its question back. It can only wait, or end the bridge, which closes
every question it caused. `docs/dev/open-questions.md` records this.

MCP's own `notifications/cancelled` is something else: it gives up a call that is still in flight,
and `oso mcp` closes that call's connection. A question the agent caused outlives the call that
caused it.

## Decision

The author accepted this on 2026-11-09, through the coordinator.

- **`oso mcp` gains a seventh tool, `cancel`,** which calls the channel's `cancel` method. It takes
  no arguments, and closes the innermost open question this connection caused.
- **What it answers:** `{"cancelled": true}` when it closed one. When no question this agent
  caused is innermost, it is a tool error with the channel's reason, and nothing is closed.
- **It closes only the agent's own questions,** as the method does. A question the person opened,
  or one another connection caused, is never closed by it.
- **It works while a buffer that may hold secrets is shown,** and while the person is in the
  middle of something, as the method does: it only takes back what the agent asked.
- **Annotations:** not read-only, since it changes what the screen shows. Not destructive: it
  changes no buffer text and no file. So Claude Code's own permission rules can allow it alone.
- **Reach is unchanged:** `cancel` is a channel method, not a command, and runs no command.

## Changes to accepted ADRs

- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "`oso mcp`", the tools `state`, `text`,
  `commands`, `look`, `change` and `open`: and `cancel`.

## Consequences

- An agent whose `change` is waiting for the person can withdraw it, and go on.
- The tool list grows by one. `tools/list` and the bridge's unit tests change with it, and
  `docs/dev/architecture.md` names it when the code lands.
- The open question about a `cancel` tool is settled.

## Alternatives considered

- **No tool; the agent waits or ends the bridge:** ending the bridge also closes the connection,
  and the next call asks the person again.
- **Folding it into `change`, as a `cancel` name:** `change` is marked destructive, so the agent's
  permission rules would ask the person to allow a withdrawal as they do an edit.
- **Cancelling through MCP's `notifications/cancelled`:** that gives up a call in flight. The
  call that caused the question has already answered, so there is nothing to give up.
