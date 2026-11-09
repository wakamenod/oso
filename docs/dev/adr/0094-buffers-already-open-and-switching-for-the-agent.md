# 0094. Buffers already open, and switching buffers, for the agent channel

- Status: Proposed
- Date: 2026-11-09

## Context

[ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md) gives one buffer per
file: `C-x C-f` of a file a buffer already visits shows that buffer, by the path at once or by
device and inode once loaded. It says the agent channel's `open` follows the same rule. But that
rule shows the other buffer at once, while the channel shows nothing before checking it: the
person may be in the middle of something, and the buffer may hold secrets
([ADR 0085](0085-agent-channel-and-oso-mcp.md)). So the load-time match now leaves out a buffer
the channel is opening, and today the channel's `open` of a file the person has open makes a
second buffer of it.

`switch-to-buffer` is `Look`, but it reads a buffer's name in a prompt, which the channel never
opens. So the channel refuses it, and an agent can show a buffer only by opening its file again.
ADR 0085 lets the channel give text only to a command whose argument is `ArgSpec::Text`.

## Decision

### `open` of a file already open

- **The channel finds the buffer as `C-x C-f` does:** by the path at once, when the absolute path
  equals a buffer's, or by device and inode once its own load has opened the file. Its load still
  checks that the file has one name ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).
- **It shows that buffer, and drops its own load, only if:**
  - the buffer is judged ordinary, not one that may hold secrets, and is not loading;
  - and nothing makes the channel wait: the person in a prompt, a key sequence or a composition,
    within 1 s of a key, or a question the channel caused.
- **Otherwise it refuses, and shows nothing:** "the file is open in a buffer that may hold
  secrets; ask the person", "the file is open and still loading; try again", or the reason it
  waits.
- **The buffer keeps its mark:** one the person visited stays theirs, so the agent's save of it
  writes without asking, as ADR 0085 has it. One the channel opened stays marked, so its save
  asks.
- It goes to the line asked for, as an `open` does. Showing a buffer already open does not count
  toward the 100 buffers a connection opens.
- **The person's `C-x C-f` of a file the channel opened shows that buffer,** as for any buffer,
  so one file never has two buffers. It keeps its mark: the agent's save of it still asks.

### `switch-to-buffer` by name

- **The channel gives a buffer's name as text** to a command whose argument is
  `ArgSpec::Buffer` with the purpose `Switch`, as it gives a `Text` argument:
  `{"name": "switch-to-buffer", "arg": "notes.md", "reach": "look"}`. It stays `Look`.
- **The name must be one `C-x b` lists** ([ADR 0088](0088-switching-and-killing-buffers.md)). A
  name that matches none is refused. For the person it makes an empty buffer; the channel makes no
  buffer, as it starts no file.
- **A buffer that may hold secrets is refused** before anything shows, as `open` refuses one.
- **The internal buffers `C-x b` lists may be shown:** `*Messages*`, `*Emergency Copies*` and
  `*Agent*`. What `text` gives of them is what it gives today.
- `kill-buffer` stays `Person`, and a `Kill` argument is refused. `consult-mark` stays refused.
- Everything else is as for any `run`: the waits, and no prompt opens for an argument.

## Changes to accepted ADRs

- [ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md), "A file already
  open", the agent channel's `open` and the dev socket's `open-file` follow the same rule: the dev
  socket's does. The channel's `open` shows a buffer already open only if it is judged ordinary
  and the channel would not wait, and otherwise refuses. A buffer the channel is opening is left
  out of the load-time match, and the channel makes its own.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "What the channel can do", a string for the new
  `Text`: and a buffer's name for an `ArgSpec::Buffer` argument whose purpose is `Switch`, which
  must name a buffer `C-x b` lists and not one that may hold secrets.

## Consequences

- An agent can go back to a buffer, and open a file the person has open, without a second buffer
  of it.
- An agent that opens a file the person visited edits the person's buffer, and its saves of that
  buffer do not ask. That follows ADR 0085's rule, which a second buffer used to get around by
  accident.
- `dispatch::args` maps a `Buffer` argument for the channel. The dev socket's `run-command` still
  refuses it, as it carries only a number.
- The end-to-end flows gain an `open` of a file the person has open, one of a file open as a
  buffer that may hold secrets, and a switch by name.

## Alternatives considered

- **A second buffer, as today:** two buffers on one file could save over each other, which ADR
  0087 rules out.
- **Showing the buffer the load-time match finds, as `C-x C-f` does:** it would show a buffer that
  may hold secrets, or interrupt the person, before the channel checks.
- **Marking the person's buffer once the agent opens it, so its saves ask:** the person would then
  be asked about saves of a file they opened, against ADR 0085's rule.
- **A name that matches none makes an empty buffer, as for the person:** an agent could make
  buffers no file backs. Nothing needs it yet.
- **A `switch` method of its own on the channel:** the command, its reach and its checks are
  already there; a method would repeat them.
