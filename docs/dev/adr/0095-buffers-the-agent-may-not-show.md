# 0095. Buffers the agent channel may not show by name

- Status: Accepted
- Date: 2026-11-09

## Context

[ADR 0094](0094-buffers-already-open-and-switching-for-the-agent.md) lets the agent channel show
a buffer by its name, through `switch-to-buffer`, and lets it show each internal buffer `C-x b`
lists: `*Messages*`, `*Emergency Copies*` and `*Agent*`.

The review of its implementation found two gaps:

- `*Emergency Copies*` takes bare keys: `v`, `=`, `r`, `d`, `g` and `q`
  ([ADR 0064](0064-recovering-an-emergency-copy.md)). `d d` deletes a copy for good, and `r r`
  replaces a buffer's text with one. Each of those is a `Person` action. An agent steered by text
  it read could switch to the list while the person pauses, and the person's next typing would
  then run them.
- A buffer still loading shows text that is not yet all there, and `open` waits until a file is
  read before it shows it ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).

## Decision

The author decided this on 2026-11-09, through the coordinator.

- **The channel shows by name only a buffer whose keys are the ordinary ones**
  (`Mode::Fundamental`): a buffer of the user's, `*Messages*` or `*Agent*`. It refuses
  `*Emergency Copies*`, and any later buffer with keys of its own, with "that buffer's keys are
  the person's; ask the person".
- **It refuses a buffer that is still loading,** with "that buffer is still loading; try again".
- **The name is resolved once,** to the buffer the checks pass, and that buffer is shown: nothing
  on the way can make a buffer for the agent. The reasons quote no name, as the name is the
  agent's text.

## Changes to accepted ADRs

- [ADR 0094](0094-buffers-already-open-and-switching-for-the-agent.md), "`switch-to-buffer` by
  name", the internal buffers `C-x b` lists may be shown: `*Messages*`, `*Emergency Copies*` and
  `*Agent*`: only `*Messages*` and `*Agent*`, as a buffer with keys of its own is refused, and so
  is a buffer still loading.

## Consequences

- The person's keys never land in the list of emergency copies because the agent switched to it.
- A new buffer mode is refused to the channel until someone decides it may be shown.
- An agent that wants a buffer still loading tries again, as it does for `open`.

## Alternatives considered

- **Showing the list, and refusing its keys for a moment after an agent's switch:** the person
  could still type into it after the moment, believing they were elsewhere.
- **Refusing only `*Emergency Copies*` by name:** the next buffer with keys of its own would be
  shown until someone remembered to add it.
