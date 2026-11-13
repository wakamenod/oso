# 0085. An agent channel in release builds, and the `oso mcp` bridge

- Status: Accepted, amended by 0082, 0087, 0093, 0094, 0112, 0113, 0116, 0118, 0119, 0121 and 0122
- Date: 2026-11-08

## Context

An agent such as Claude Code can drive a person's Emacs today: split the window, open a file
at a line, say what is selected. oso can be driven only through the dev control socket, which
release builds do not contain ([ADR 0016](0016-dev-control-socket.md),
[ADR 0032](0032-control-socket-transport.md)). That socket trusts any process of the same user
with every command and with dev methods such as `screenshot`, `export-state` and `send-keys`
([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)).

[ADR 0031](0031-claude-code-integration.md) plans an `oso mcp` stdio bridge that `claude`
launches, over a Unix socket with a peer user check. It is scheduled for M4, with the terminal
view. The author wants the bridge earlier, without the terminal view, to drive the oso they
use every day.

The threat is not a remote attacker, since oso has no network code. It is:

- an agent steered by text it read (prompt injection) into a harmful command, such as
  exiting without saving, or a command that spawns processes once those exist;
- another process of the same user, which can reach any socket the person's oso opens.

A same-user process can read the person's files anyway. So the channel must not add what such
a process cannot do on its own: act as the person at the keyboard, reach processes, or read
text that oso withholds.

Two facts shape the design. The release build has no JSON parser, since `serde_json` is
dev-only. And `run-command` takes only a number, so no command takes a string over the socket.

## Decision

The author accepted this on 2026-11-08, through the coordinator.

The author chose these on 2026-11-08, through the coordinator.

### Each command says how far the channel may reach it

- **`Command` gains a required field, `reach: Reach`,** with no default
  ([ADR 0034](0034-command-registry-and-keymaps.md)). So every new command has to choose:
  - `Look`: moves, marks, shows or opens, and changes no buffer text and no file;
  - `Change`: edits buffer text or saves;
  - `Person`: needs a person at the keyboard, and the channel refuses it.
- **`Person` covers:**
  - exiting (`save-buffers-kill-terminal`) and `keyboard-quit`;
  - `agent-channel-start` and `agent-channel-stop`;
  - `M-x`, which runs any command by name;
  - answering a prompt: `answer-yes`, `answer-no`, `exit-minibuffer` and the history
    commands;
  - prefix keys (`universal-argument`, `digit-argument`, `negative-argument`) and
    `self-insert-command`, which need a key; the channel passes a count or text instead;
  - `yank`, which reads the OS clipboard;
  - recovering, comparing and deleting emergency copies;
  - commands with a passphrase argument, and dev commands.
- **From later milestones, by rule:** trusting a project ([ADR 0009](0009-project-trust.md)),
  anything that spawns a process ([ADR 0010](0010-process-and-network-policy.md)), anything
  that shows or copies what a buffer that may hold secrets withholds, and anything about
  secure input ([ADR 0073](0073-secure-input-in-secret-buffers.md)).
- **A channel action never starts a process by itself (M3 onward).** What opening or saving a
  file would start, such as a language server or a format on save, waits in a buffer the
  channel opened or saved until the person acts in that buffer from the window.
- `save-buffer` and the kills are `Change`. Undo is `Change`.

### What the channel may write

- **A channel save writes without asking only a buffer the person visited:** one opened from
  the window, from oso's command line, or brought back by a restart of such a buffer.
- **Any other save from the channel asks the person,** as a question the channel opened: with
  the `Agent:` label, from the keyboard only, after 500 ms with no key. This covers a buffer the
  channel opened and, once it exists, saving under a new name.
- **oso's configuration and trust files are never saved from the channel,** whatever the person
  answers (M3 onward, [ADR 0009](0009-project-trust.md), [ADR 0019](0019-configuration.md)).
- Each rule here and above gets a test of its own: a unit test where it is a plain function,
  and an end-to-end flow over the channel's socket where it needs the app.
- A guard test lists the `Person` commands, so a change to the list shows in review.

### What the channel can do

- **`run`**: runs a `Look` or `Change` command by name. Its argument is checked against the
  command's spec before it runs: a count for `Count`, a whole number for `Number`, and a
  string for the new `Text`. A `Number` or `Text` command run without its argument is refused:
  the channel never opens a prompt for an argument. Names are at most 256 bytes and counts at
  most 10,000 either way, as in ADR 0044. Only a registered name is echoed or logged.
- **`ArgSpec::Text { prompt, history }`** is a string, read in a prompt from keys, as `Number`
  is ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)). Over the channel it is
  at most 64 KiB, and it enters no prompt history.
- **`insert`** is a new `Change` command with a `Text` argument. It inserts at every cursor, as
  typing does, and is refused when its text times the cursors comes to more than 1 MiB. There
  is no `send-keys`.
- **`open`**: visits an absolute path of at most 4096 bytes in the selected window, as
  `oso FILE` does, and moves to a line if one is given. It is `Look`. Opening executes nothing.
  - Only a file that already exists opens. A missing path is refused, so the channel cannot
    start a new file.
  - A path that matches the secret patterns as given is refused before anything opens.
  - A file the channel opens is shown only once the loader has judged it ordinary
    ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)). One judged otherwise is
    dropped unshown, and the reply says it was refused.
  - At most 4 of the channel's loads run at once, and one connection opens at most 100 buffers.
- **`state`**: a small schema of its own, with its own version:
  - buffers: id, name, path, read-only, loading, and whether they may hold secrets;
  - windows: id, buffer, whether selected, cursor, selections, region and first row;
  - the rows the selected window shows, at most 64 KiB;
  - the echo area's message;
  - for an open prompt, its kind and whether the person or the channel opened it.

  Everything ADR 0044, 0058, 0063 and 0079 withhold around a buffer that may hold secrets is
  withheld here the same way. The state never has keys in progress, a composition or preedit,
  what is typed in a prompt, the kill ring, the clipboard, secure input or timings.
- **`text`**: whole rows of a buffer by id, 200 rows by default, at most 2,000 rows and
  512 KiB, or the selected window's region, at most 64 KiB. It is checked and copied off the UI
  thread, as `get-text` is. It is refused for a buffer that may hold secrets.
- **`commands`**: the `Look` and `Change` commands, with their docstrings, argument specs and
  reach.
- **`cancel`**: closes a question the channel opened. It works while a buffer that may hold
  secrets is shown too, since it only closes the channel's own question.
- **`version`**: the channel protocol's version, which the bridge asks first, and refuses to go
  on if it does not know it.
- Screenshots, the restart handoff, timings and hot reload stay dev-only.

### Where the channel waits for the person

`run`, `open` and `insert` are refused, with the reason, while any of these holds. `state`,
`text`, `commands`, `version` and `cancel` still answer.

- **Any window shows a buffer that may hold secrets.** The reply says "a buffer that may hold
  secrets is shown; ask the person", and `state` answers with that buffer's fields withheld.
- **The person is in the middle of something:** a prompt the person opened is innermost, keys
  are in progress, a composition is open, or a key came from the window less than 1 s ago.
- **A question the channel opened is still open.**
- **An undo or a redo of a large step goes on** ([ADR 0119](0119-undoing-large-steps-in-slices.md)).
  The reply to the channel's own `undo` waits for it to end.

Questions the channel causes:

- **A question a channel command opens is answered by the person** (for example, saving over a
  file changed on disk). Only keys from the window answer it, and the channel can only `cancel`
  it. The reply says "waiting for the person:" and the question.
- **It takes no answer until 500 ms have passed with no key.** Each key that does not answer
  starts that wait again, so typing that was under way cannot answer it.
- **Its label starts `Agent:`,** so the person can tell it from oso's own questions.
- **Kills from the channel go into oso's kill ring only,** never to the OS clipboard
  ([ADR 0074](0074-the-kill-ring-and-the-clipboard.md)). So text an agent planted does not
  reach a terminal when the person pastes there.

In dev builds, keys from the dev socket's `send-keys` count as keys from the window for these
rules, as the end-to-end flows stand in for the person.

### Turning it on, and who may connect

- **Off by default.** The `Person` command `agent-channel-start` starts it for this session, and
  `agent-channel-stop` stops it and closes its connections. Once configuration loads (M3), a
  setting starts it at launch ([ADR 0019](0019-configuration.md)).
- **Socket:** `<pid>-agent.sock`, in the control socket's private directory, with the same
  checks on the directory, the same mode and the same peer user check on both ends
  ([ADR 0032](0032-control-socket-transport.md), [ADR 0065](0065-access-lists-on-private-directories.md)).
  It is a different socket from the dev one, with only the methods above. So a dev build
  serves the channel exactly as a release build does, and the end-to-end flows test it as
  shipped. Removing stale sockets and files ships in release builds, and counts this socket as
  an instance's.
- **Each new connection asks the person:** `Agent: let claude (pid 812) control oso through
  oso mcp (pid 815)? (y or n)`. It names the connecting process and, when that is `oso mcp`,
  the process that started it, as the system reports them, or says it cannot tell. It takes its
  answer as a channel command's question does: from the keyboard only, and after 500 ms with
  no key. It waits until no prompt is open and no keys are in progress.
- **`n` or `C-g` closes the connection, and refuses new ones for 60 s.** Only one connection
  waits at a time, and another is refused at once. More than 10 connections a minute are
  refused without a question. The channel answers nothing until the person agrees.
- **Each connection asks again,** so does the bridge's after an oso restart.
- **What the person sees:** a modeline segment `Agent`, dim while the channel is on and bright
  while a client is connected ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md)).
  An internal buffer `*Agent*` lists each request: time, method, a registered command's name,
  count or text length, and outcome. It never holds a text argument, a name the registry does
  not know, or buffer text, and keeps the last 1,000 lines.

### Protocol and limits

- **JSON Lines,** as on the dev socket ([ADR 0023](0023-control-socket-protocol-and-restart.md)):
  one JSON object per line of UTF-8, ended by `\n`, with the dev socket's request and response
  shapes. The channel's methods and version are its own.
- At most 2 clients, one request in flight each, and 30 requests a second per connection.
  More is refused, not queued. A hosted session's proven bridge does not count toward the 2
  ([ADR 0121](0121-agent-sessions-and-spaces.md), "Step 1, as built").
- Request and response lines of at most 1 MiB, and 10 s for the UI to reply, as on the dev
  socket. An agreed connection has no idle limit; a waiting one closes if its client goes.
- Background threads read and parse, and requests reach the UI thread as dev requests do. The
  UI thread never touches the socket.

### `oso mcp`

- **A subcommand of the oso binary,** as `oso ctl` is. `claude` launches it, and the docs give
  the `claude mcp add` line. `oso integration install claude` comes with the hooks in M4
  ([ADR 0031](0031-claude-code-integration.md)).
- It speaks JSON-RPC over stdio, written in oso: `initialize`, `ping`, `tools/list` and
  `tools/call`. Lines from stdin are at most 1 MiB. There is no MCP SDK. It logs to stderr
  only.
- **Tools:** `state`, `text`, `commands`, `look` (runs a `Look` command), `change` (runs a
  `Change` command, or inserts text) and `open` (a path and a line). Looking and changing are
  separate tools, so Claude Code's own permission rules can allow one and keep asking for the
  other. `state`, `text` and `commands` are marked read-only; `look` and `open` are not, and
  `change` is marked destructive.
- It connects to the one instance whose channel is on, and holds that connection for its life.
  With several, it refuses and names their process IDs, and `--pid` picks one. After oso
  restarts it connects again, and that connection asks again.
- **`serde_json` becomes a release dependency,** used through `Value` only, without serde's
  derive macros. It brings `serde_core`, `itoa` and `zmij`, which the lock already holds for dev
  builds. `miniz_oxide` stays dev-only.

### Code and checks

- `crates/oso/src/channel/` holds the channel's server, protocol, state, log and the bridge.
  `dev/` builds on its pieces where they are the same. It is a new line in
  `guardrails/layers.txt`.
- `oso-platform::control` is built in every build, not only with `dev`. It gains the peer's
  process ID and process name, and its parent's.
- `just release-safety` no longer lists `serde_json` as dev-only. It keeps its dev markers and
  the `dev` feature check, and a unit test checks that the channel's methods include no dev
  method.
- Windows is not supported yet, as for the dev socket.

### Changes to accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0016](0016-dev-control-socket.md), the socket is behind a dev feature that release
  builds never enable: the dev socket still is, but the agent channel, a separate socket with
  fewer methods, ships in release builds, off by default.
- [ADR 0031](0031-claude-code-integration.md), the `oso mcp` bridge comes in M4 with review and
  worktree tools: it comes now, without the terminal view, with the tools above. Review and
  worktree tools join it in M4.
- [ADR 0032](0032-control-socket-transport.md), `serde_json` is a dev-only crate, the
  transport and the removal of stale sockets exist only with `dev`, and `<pid>.sock` is an
  instance's one socket: all are in release builds now, and `<pid>-agent.sock` is an instance's
  socket too.
- [ADR 0034](0034-command-registry-and-keymaps.md), a command is a name, a docstring, an
  argument spec and a function: and a reach. The specs gain `Text`.
- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md), release builds contain no way for
  a same-user process to run commands: they contain the channel, which runs `Look` and `Change`
  commands only after the person agrees to the connection, and none while a buffer that may hold
  secrets is shown. It saves without asking only buffers the person visited.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), a question opens only from
  a command, and one a socket request opened is answered over the socket: the channel's
  connection question opens from a connection, and a question a channel command opened is
  answered from the window. Both take no answer until 500 ms pass with no key. The dev socket's
  rule stays.
- [ADR 0074](0074-the-kill-ring-and-the-clipboard.md), kills sync with the OS clipboard: not
  kills from the channel.

## Consequences

- An agent can move, mark, open, read, edit and save in the person's oso, and each connection
  needs the person's yes. What it can do grows as commands land: `C-x C-f`, `C-x b` and window
  splits come with their steps, each with its reach.
- Through `open` and `text`, an agent reads any file oso can read, outside the agent's own
  rules for reading files, except files that may hold secrets, by the same patterns as
  everywhere. The agent's own permission prompt for each tool still applies.
- The channel adds a local entry point to release builds. It reaches no process, and the dev
  methods are not on it. A same-user process that the person lets connect can edit and save the
  files the person opened. It creates no file, and writes no other file, without asking the
  person, and never writes oso's own configuration or trust.
- An agent's work and the person's do not interleave: the agent waits out a key sequence, a
  prompt, a composition and a second after each key, and retries after a refusal. While a
  buffer that may hold secrets is on the screen, the agent can only read the state.
- Each new command needs a reach, and the `add-command` skill and `REVIEW.md` ask for it.
- `serde_json` and its three crates ship. `just release-safety` loses one of its two dev-only
  crate checks; the marker and feature checks stay.
- Naming the connecting process and its parent adds `unsafe` calls to `oso-platform`'s Unix
  transport, each with its `// SAFETY:` comment.
- The connection question opens from outside a command, which ADR 0072 ruled out to keep
  prose from answering a question. The wait for 500 ms without a key stands in for that rule.
- Follow-up work: the configuration setting (M3), holding processes a channel action would start
  (M3), `oso integration install claude` (M4), and a named pipe on Windows.

## Alternatives considered

- **A deny mark, with commands allowed by default:** a new command would reach the channel
  unless someone remembered to mark it.
- **An allow list in the channel module:** works, but the choice sits far from the command and
  is easy to forget.
- **`send-keys` on the channel:** keys reach every binding, so the mark would have to be checked
  per key, and prompts the person opened could be answered. A `Text` argument and `insert` cover
  typing.
- **The dev rule for questions, answered over the socket:** an agent could answer oso's own
  safety questions, such as saving over a changed file.
- **Refusing any command that would ask:** safe, but the person would then have to do the whole
  action themselves, not just answer.
- **A fixed 500 ms after a question appears:** a person typing steadily would still answer it
  with a key that came after that time.
- **A command-line flag or a marker file to turn it on:** a flag is awkward for an app started
  from the Dock, and a file is a configuration of its own before M3's.
- **No question on connecting, only the indicator:** another same-user process could connect
  quietly once the channel is on.
- **Running commands while a buffer that may hold secrets is shown, refusing only those that
  move or edit in it:** which commands depend on that buffer's text changes as commands land,
  and each would need its own check.
- **Opening a file the patterns match, and withholding it as the dev socket does:** the
  channel would then run nothing while it is shown, so opening it serves no purpose.
- **Text only of buffers the person opened:** "open this file and read it" is a main use.
- **Opening a missing path and saving any buffer, as the person can:** with `insert`, an agent
  could create or replace any file the person can write, such as one that runs at login or on
  a git operation.
- **Asking before every channel save:** the person would answer a question for each change they
  asked the agent to make in a file they have open.
- **One `run` tool for every command:** the agent's permission rules could not tell looking from
  changing.
- **The official MCP SDK (`rmcp`):** it brings an async runtime and many crates for four methods.
- **Hand-written JSON:** no new crate in release, but a parser of input an agent sends, which
  would need fuzzing.
- **A separate `oso-mcp` binary:** it would not link the GUI, but it is a new crate, and
  `oso ctl` already works as a subcommand.
- **Serving the channel on the dev socket's file, with fewer methods per connection:** a dev
  build could not test the channel as shipped, and one mistake in the split would expose dev
  methods.
