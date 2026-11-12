# 0120. The terminal, and Claude Code in it

- Status: Proposed
- Date: 2026-11-12

## Context

M4 is "develop oso in oso" ([roadmap](../roadmap.md)), and its first item is a terminal that hosts
Claude Code. The author drives Claude Code in a terminal today, and wants to run `claude` in an oso
buffer, with the agent reaching oso through the `oso mcp` bridge.

What is decided already:

- **The engine is `alacritty_terminal`**, a terminal is a view of its own that renders the cell grid
  directly, and opening one is allowed in a restricted project, as an explicit action
  ([ADR 0015](0015-terminal.md)). The escape-sequence policy so far: no OSC 52 reads, no sequence
  that echoes data back, and an OSC 8 link shows its real target before it opens.
- **oso hosts the official `claude` TUI** and integrates through documented extension points only:
  hooks, MCP and `--resume` ([ADR 0031](0031-claude-code-integration.md)).
- **The agent channel and `oso mcp` ship in release builds, off by default** ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).
  Each command declares its reach (`Look`, `Change`, `Person`); anything that spawns a process or
  shows what a buffer that may hold secrets withholds is `Person`. Each connection asks the person.
- **What an agent's open or save starts waits for the person** in that buffer, from M4's first
  program ([ADR 0118](0118-what-an-agents-open-or-save-starts.md)).
- **Only `oso-platform::process` spawns,** with an allowlisted environment, an executable from an
  absolute path or a filtered `PATH`, and a log line per spawn ([ADR 0010](0010-process-and-network-policy.md),
  [ADR 0047](0047-process-spawning-and-restart-handoff.md)). It is behind the `dev` feature, and
  `just release-safety` checks that its marker is in no release binary. Opening a file or a project
  never executes anything ([ADR 0009](0009-project-trust.md),
  [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
- **oso has no network code** ([ADR 0010](0010-process-and-network-policy.md)).

What [`open-questions.md`](../open-questions.md) leaves for M4:

- the terminal engine, `alacritty_terminal` against libghostty-vt, with the author's current Emacs
  terminal (ghostel, on libghostty-vt) as the bar: inline images, shell integration (OSC 7,
  OSC 133), input modes, and the terminal as the backend for compile;
- the PTY per OS (ConPTY on Windows), and the full list of allowed and blocked escape sequences.

Facts about `alacritty_terminal` 0.26.0, the current release, read from its source:

- `Term` parses through `vte` 0.15 and keeps a grid with scrollback, damage tracking, the kitty
  keyboard protocol's mode stack, OSC 8 hyperlinks, synchronized updates (mode 2026), bracketed paste
  and mouse modes.
- **Everything that would answer the program or reach outside goes to the embedder as an `Event`:**
  `ClipboardStore`, `ClipboardLoad`, `ColorRequest`, `TextAreaSizeRequest`, `PtyWrite` (device
  reports), `Title`, `Bell`. So the policy lives in oso's event listener, not in a fork.
- It has no hook for sequences it does not handle, such as OSC 7, OSC 133, OSC 7501 and the
  notification sequences OSC 9 and OSC 777.
- Its `tty` and `event_loop` modules open the PTY and spawn the shell with `std::process::Command`
  themselves, with `rustix-openpty`, `signal-hook` and `polling` on Unix and `miow` and `windows-sys`
  on Windows. They are always compiled; there is no feature to leave them out.
- It depends on `regex-automata` for its own grid search. `serde` is a default feature that can be
  turned off.

The forces, in the order of the project's priorities:

- **Speed.** A `cat` of a large log, or Claude Code redrawing its whole screen, must not cost the UI
  thread a frame. Key press to photon is p99 ≤ 8 ms ([roadmap](../roadmap.md)).
- **Security.** Terminal output is untrusted input: a file `cat`ed, a web page an agent fetched, a
  build's log. An agent steered by such text is the threat of ADR 0085, and an escape sequence is
  T7 in [`security.md`](../security.md). A terminal is also the first thing a release build spawns.
- **Self-hosting.** The author will run `claude` in an oso started by `oso dev`, and every edit
  the agent makes to oso's source restarts that oso. If a restart ends the terminal, it ends the
  agent in the middle of its turn.

### What others do

- **vterm** (Emacs) wraps libvterm, a C library, in a dynamic module, and mirrors the screen into an
  Emacs buffer. Every key goes to the terminal except a list (`vterm-keymap-exceptions`: `C-c`,
  `C-x`, `C-u`, `C-g`, `C-h`, `C-l`, `M-x`, `M-o`, `C-y`, `M-y`); `C-q` sends the next key as it is.
  `vterm-copy-mode` freezes the buffer so that every Emacs command reads it. The shell can call
  Elisp functions the user listed (`vterm-eval-cmds`) through an escape sequence.
- **eat** (Emacs) is pure Elisp. It has semi-char mode (most keys to the terminal, the Emacs prefix
  keys kept), char mode (every key), Emacs mode (the buffer as text) and line mode. It does shell
  integration (directory, prompts) and Sixel images.
- **term.el** (Emacs) has char mode and line mode, escaped by `C-c`. It is slow with heavy output.
- **WezTerm** has its own parser (`termwiz`), lets OSC 52 write the clipboard, and shows Kitty,
  iTerm2 and Sixel images.
- **Ghostty**, the engine of the author's current terminal, asks before OSC 52 reads the clipboard
  (`clipboard-read = ask`), lets it write (`clipboard-write = allow`), warns before a paste that
  could run a command (`clipboard-paste-protection`), and does not report the title
  (`title-report = false`) unless set.
- **Zed** renders `alacritty_terminal`'s grid in its own GPU UI, and its agent runs commands in
  terminals of its own.
- **VS Code** uses xterm.js and node-pty. Its pty host is a separate process, so terminals survive a
  reload of the window. It warns before a multi-line paste, and does shell integration with OSC 633
  and OSC 133. Claude Code's VS Code extension serves MCP over a WebSocket on localhost, which oso,
  with no network code, cannot do.

## Decision

Proposed. The text below takes the first option (a) of each question under
[Questions](#questions); the author's answers may change it.

### The crates and the layers (Q1, Q2, Q3)

- **The engine stays `alacritty_terminal` (Q1),** as ADR 0015 says, with its default features off.
  It is pure Rust, and an engine that parses untrusted output in memory-unsafe code, and needs Zig on
  three OSes, costs more than what libghostty-vt has and `alacritty_terminal` lacks. What it lacks,
  oso adds: a second scanner for the sequences it ignores (see below), and images later.
- **A new crate, `oso-term`,** holds the terminal model: `alacritty_terminal`, oso's event listener
  with the escape-sequence policy, the second scanner, the key encoder, the terminal's thread, and
  the snapshots it publishes. It sits beside `oso-render` and `oso-syntax`:

  ```
  [workspace]
  oso
  oso-render oso-syntax oso-term
  oso-core oso-platform
  ```

  - `oso-term` uses `oso-platform` (the PTY) and `oso-core` (the key type, so the encoder reads
    oso's keys). It does not use `oso-render`; the tip crate turns a snapshot into the rows
    `oso-render` already draws, as it does for buffers.
  - Like `oso-syntax`, only the tip crate uses it. It parses untrusted bytes, so it gets its own
    fuzz target and its own `AGENTS.md`.
  - **The engine is behind a trait, `oso_term::Engine`** (feed bytes, resize, publish a snapshot,
    read rows for copy mode, read the modes the encoder needs), so libghostty-vt or another engine
    can be built against it later without touching the tip crate. This ADR is the one that adds the
    trait and the crate.
- **The PTY is oso's own, in `oso-platform::process` (Q2):** a submodule `process::pty`, Unix only
  for now. `alacritty_terminal`'s `tty` and `event_loop` modules are compiled but never called, and a
  guard test fails if `oso-term` names them.
  - **`openpty`** from `libc` gives the master and the slave. The master is close-on-exec and
    non-blocking.
  - **The child starts through the module's `spawn`,** so the executable rules, the environment
    allowlist and the log line all hold. A `Spawn` gains `terminal: Option<&Pty>`: the slave becomes
    its standard input, output and error, and a `pre_exec` hook calls `setsid` and `TIOCSCTTY`, so the
    slave is its controlling terminal. The hook is the module's one `unsafe` block, with its
    `// SAFETY:` comment: it calls only async-signal-safe functions.
  - It resizes with `TIOCSWINSZ`, reads whether the slave echoes with `tcgetattr` on the master, and
    sends `SIGHUP` to the child's process group when the terminal closes.
  - **The spawn runs on the terminal's thread,** never the UI thread, since `fork` of a large process
    takes milliseconds.
  - **macOS and Linux first. Windows is deferred (Q3):** ConPTY comes when Windows draws text
    ([ADR 0036](0036-text-system-and-glyph-atlas.md)) and the agent channel has its named pipe. The
    module's interface is the one ConPTY will fill.
- **`oso-platform::process` ships in release builds.** It leaves the `dev` feature, and `log`
  becomes a release dependency of `oso-platform`, as it already is on macOS. The spawn marker becomes
  `oso-process-spawn`, and `just release-safety` stops looking for it; its other checks stay. The
  architecture test still allows `#[expect(clippy::disallowed_methods)]` only in that module.

### The terminal's thread, and what the UI thread does (Q4)

- **Each terminal has one thread** that owns the PTY's master and the `Term`. It `poll`s the master
  and a wake-up pipe, reads at most 64 KiB at a time, parses it, writes what the UI queued, and
  answers the UI's requests (resize, scroll, copy mode, close).
  - The UI thread never takes a lock the parser holds, never reads or writes the PTY, and never
    parses.
  - Writes wait in a queue on the terminal's thread, so a large paste into a program that reads
    slowly never blocks the UI. The queue holds at most 16 MiB; past that, a paste is refused with a
    message.
  - When oso falls behind, it stops reading, so the child blocks on a full PTY. Nothing grows without
    bound.
- **A snapshot** is what the UI draws: the visible rows' cells (character, colors, flags, link id),
  the cursor and its shape, the title, which rows changed, and whether echo is off. The thread builds
  one from the rows `Term`'s damage tracking marks, and sends it through the wake-up path the loader
  and the workers use. **At most one snapshot is in flight:** a new one is built only after the UI has
  taken the last, so bulk output costs the UI one snapshot per frame however fast it comes.
- **Keys are encoded on the terminal's thread,** against the modes current there (application cursor
  keys, the kitty flags, bracketed paste), so a mode the program just set is never missed by a key the
  UI sent before it saw that mode.
- **Rendering** draws each cell at its column, as the grid placed it; oso does not measure widths
  again. A row is shaped only when its snapshot says it changed, through the shaped-row cache text
  views use. Colors come from the theme: 16 new faces, `terminal-color-0` to `terminal-color-15`, in
  each built-in theme ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)), and the theme's
  foreground and background. A cell's 256-color or true color is drawn as given.
- **Logical order:** cells are drawn in the order of the grid, with no reordering, as in text views
  ([ADR 0066](0066-text-stays-in-logical-order.md)).

### The terminal as a buffer (Q5, Q6)

- **A terminal is a buffer of kind `Terminal`,** as [`architecture.md`](../architecture.md)'s table
  has it. `oso-core`'s `Buffer` gains the kind and nothing of the grid: the grid lives in `oso-term`,
  and the tip crate holds a map from buffer id to terminal.
- **Names:** `*terminal*`, `*terminal*<2>` and so on, and `*claude*` for one started by `claude`. The
  program's title (OSC 0 and OSC 2) shows in the modeline, never in the buffer's name.
- **In split windows** ([ADR 0112](0112-window-splits.md)), the PTY takes the size of the window that
  last selected the terminal. Other windows showing it draw the same grid, clipped or padded.
- **Copy mode (Q5):** `C-c C-j` freezes the screen and the scrollback into a read-only internal
  buffer, built on the terminal's thread, and shows it in the window at the same place. Every text
  command then works on it unchanged: isearch, migemo, avy, the region, `M-w`. `q` or `C-c C-k`
  returns to the live terminal, which kept running. This is what ADR 0015 means by searchable text,
  without keeping a second copy in step with the grid.
- **Scrollback (Q6):** 10,000 lines by default, set in `config.toml`'s `[terminal] scrollback`, at
  most 100,000. A cell takes 24 bytes, so 10,000 lines of 200 columns take up to 48 MB. The alternate
  screen keeps none. Copy mode's buffer is dropped when the person leaves it.

### Input (Q7, Q8)

- **Semi-char mode is the default (Q7):** every key goes to the program, encoded, except:
  - `C-x` and `M-x`, so the frame's keys, buffers and windows work from a terminal;
  - `C-c`, a prefix: `C-c C-c` sends `C-c`, `C-c C-j` enters copy mode, `C-c C-k` char mode, and `C-c`
    followed by any other key sends both;
  - `C-q`, which sends the next key as it is, whatever it is;
  - `C-g` while a prompt or a key sequence of oso's is open; otherwise it goes to the program.
- **Char mode** (`C-c C-k`) sends every key except `C-c C-k`, which returns to semi-char mode. It is
  for programs that need `C-x`, such as Emacs in a terminal.
- **Line mode,** as Emacs's term, is not built: Claude Code and shells edit their own line.
- **The encoder** writes the xterm forms (`CSI` and `SS3` keys, `ESC` for Meta) and, once the
  program pushes kitty keyboard flags, the kitty protocol's `CSI u` forms. So Shift+Enter, which
  Claude Code reads as a new line, reaches it where the legacy forms cannot tell it from Enter.
- **The input method:** a preedit is drawn at the terminal's cursor, as in a text view, and the
  committed text goes to the program as UTF-8. The candidate window is placed at the terminal's
  cursor cell, even while the program hides the cursor, as Claude Code's input box does.
- **Paste (Q8):** `C-y` (from the kill ring) and the OS paste send text. With bracketed paste on,
  oso removes any `ESC [ 201 ~` and other control characters except tab and new line from the text
  first, so pasted text cannot end the paste early. With it off, a paste that holds a new line asks
  `Paste N lines into a program that may run them? (y or n)`, as Ghostty and VS Code warn.
- **The mouse:** the wheel scrolls the scrollback, or, while the program has mouse reporting on, goes
  to it as wheel events. Clicks and drags are not sent in M4; oso's mouse support comes later
  ([`open-questions.md`](../open-questions.md)).
- **Echo off is a secret:** while the PTY does not echo (a password prompt, `sudo`, `ssh`), keys typed
  there are not recorded into a keyboard macro, the modeline says `secret`,
  and on macOS secure input is on while that terminal is shown and selected
  ([ADR 0073](0073-secure-input-in-secret-buffers.md)).

### Escape sequences (Q9, Q10)

Each sequence below is allowed, denied, or answered from oso's own state. A denied one is dropped
without a reply, as an unknown sequence is.

| Sequence | What oso does |
|---|---|
| OSC 52 clipboard read | Denied (ADR 0015). |
| OSC 52 clipboard write | Into oso's kill ring only, never the OS clipboard, with an echo-area message that names the terminal and the length (Q9). |
| OSC 0, 1, 2 title | Shown in the modeline. Control, bidi and invisible characters are dropped, and it is cut at 256 bytes. |
| Title report (`CSI 21 t`) and other reports of text the program set | Denied (ADR 0015). |
| Title stack (`CSI 22 t`, `CSI 23 t`) | Allowed. |
| OSC 8 hyperlinks | Drawn underlined. The target shows in the echo area when the cursor or point is on the link. Opening one waits for the URL-opening policy in `open-questions.md`; until then `RET` on a link copies its target into the kill ring. |
| OSC 4, 10, 11, 12 color queries | Answered from the theme, so a program can tell a dark theme from a light one. |
| OSC 4, 10, 11, 12 color changes | Applied to that terminal only, until it resets. |
| OSC 50 font, `DECRQSS` | Denied. |
| Device attributes, device status, cursor position, mode reports, kitty keyboard query | Answered: they report the terminal's own state, not text a program put there. |
| Text area size (`CSI 14 t`, `CSI 18 t`) | Answered. |
| Window moves, resizes and iconify (`CSI t` 1–9) | Denied. |
| Bracketed paste, focus reports (1004), synchronized updates (2026), alternate screen, cursor shape | Allowed. |
| Mouse reporting (1000, 1002, 1003, 1006) | Allowed; the wheel is sent (see Input). |
| Kitty keyboard protocol | Allowed. |
| OSC 7 working directory | Read by the second scanner: it sets the terminal's directory for `C-x C-f` and `C-x p f`, if it names a local path. |
| OSC 133 prompt marks | Read by the second scanner and kept by row, for moving between prompts later. |
| OSC 7501 agent status | Read by the second scanner; its use is the hooks ADR's ([ADR 0031](0031-claude-code-integration.md)). |
| OSC 9, OSC 777 notifications | An echo-area message and a modeline mark on the terminal's buffer while it is not shown, with control and bidi characters dropped. No OS notification yet. |
| Bell | The modeline flashes, as the flash of [ADR 0097](0097-visual-aids.md) does. |
| Kitty graphics, Sixel, iTerm2 images (OSC 1337) | Denied in M4: an image decoder is T10, and images come after M4 (Q10). |
| OSC 51 and other "run this editor command" sequences | Denied. A program in a terminal never runs an oso command. |

- **The second scanner** is a `vte::Parser` with its own `Perform`, fed the same bytes on the
  terminal's thread before `Term` gets them, that acts only on the OSC numbers named above. `vte` is
  the crate `alacritty_terminal` already uses, so the bytes are split the same way. Its OSC string is
  capped at 4 KiB.
- The policy is one function in `oso-term` per `Event` kind, and each row of the table has a test
  that feeds the sequence and checks the reply, the kill ring or the modeline.

### Secrets and the agent channel (Q11, Q12)

- **A terminal's text is withheld from the channel (Q11):** `text` is refused for a terminal buffer,
  and `state` lists it by id, name and kind, with whether a process is running, but no rows and no
  title. Terminal output holds whatever the person's commands printed, such as an environment or a
  token, and no pattern can judge it.
- **Showing a terminal does not stop the channel.** A terminal is not a buffer that may hold secrets
  in ADR 0085's sense, or the channel could never act while Claude Code's own terminal is on screen.
  While a shown terminal has echo off, it is: the channel waits, as for a secret buffer.
- **Nothing reaches a PTY from the channel.** `insert` and every `Change` command are refused in a
  terminal buffer, and copy mode's buffer is read-only. Writing to a PTY is typing into a shell,
  which runs anything.
- **Reach of the new commands:**
  - `Person`: `terminal`, `claude`, every key and paste sent to a terminal, the mode switches, closing
    a terminal, and `kill-buffer` on one (which asks while a process runs);
  - `Look`: switching to a terminal buffer, entering and leaving copy mode, and moving and isearch in
    copy mode's buffer. Copy mode's buffer is withheld from `text` as the terminal is.
- **Terminal text never reaches the disk:** no auto-save, no emergency copy, no session file. A
  restart carries it only in memory, as below. The kill ring holds what the person copies, as from
  any buffer.

### Restricted projects

- **A terminal opens in a restricted project,** as ADR 0009 and ADR 0015 allow, only by the person's
  command. Opening a file or a project never opens one, and no restart outside `oso dev` brings one
  back.
- **Its directory** is the project's root if the buffer has a project, otherwise the buffer's
  directory, otherwise the home directory.
- **What runs in it is the person's:** their shell, its startup files and its prompt, which may run
  `git` in the project. oso's trust limits what oso itself starts. It does not limit what the person
  or an agent runs in a terminal, and the modeline's `restricted` says nothing about it. The docs say
  so where they explain trust.

### The program and its environment (Q13)

- **`terminal`** starts the person's login shell: `$SHELL`, or the account's shell from the user
  database, as an absolute path, with `-l`.
- **The environment is the module's allowlist** plus `SHELL`, `COLORTERM=truecolor`,
  `TERM=xterm-256color`, `TERM_PROGRAM=oso`, `TERM_PROGRAM_VERSION`, `SSH_AUTH_SOCK` and the names
  listed in `config.toml`'s `[terminal] env`. A login shell builds the rest from the person's startup
  files, as macOS Terminal's does.

### Claude Code in a terminal (Q14, Q15, Q16)

- **`claude` (a `Person` command)** opens `*claude*` in the selected window and starts, through the
  login shell, `exec claude --mcp-config <file>`, so `claude` is found on the person's own `PATH`.
  `[claude] program` in `config.toml` names an absolute path instead. A prefix argument asks for more
  arguments, such as `--resume`.
- **The MCP config** is a file `<pid>-mcp.json`, mode 0600, in the control socket's private
  directory ([ADR 0032](0032-control-socket-transport.md)), written when the command runs and removed
  when oso exits:

  ```json
  {"mcpServers": {"oso": {"command": "<oso's own absolute path>", "args": ["mcp", "--pid", "<pid>"]}}}
  ```

  Only paths and a process id go into it. It names the server `oso`, so a session started this way
  uses this instance, whatever `claude mcp add` registered.
- **The channel starts with the command (Q14):** `claude` runs `agent-channel-start` if the channel
  is off, and says so in the echo area. Both are the person's commands, so ADR 0085's "off by
  default" still holds.
- **The connection still asks (Q15),** as every connection does. The question names the terminal:
  `Agent: let claude in *claude* (pid 812) control oso through oso mcp (pid 815)? (y or n)`, when the
  connecting bridge's parent is the process this terminal started.
- **ADR 0118 holds unchanged:** what the agent's `open` or save starts waits for the person in that
  buffer. Claude Code's own tools edit files on disk and run commands outside oso; oso sees those
  edits as changes on disk ([ADR 0083](0083-reloading-files-changed-on-disk.md)), and Claude Code's
  own permission rules govern them.
- **Not in this ADR:** hooks, OSC 7501 states, the session list, notifications and `--resume`
  restore ([ADR 0031](0031-claude-code-integration.md)), and the review and worktree tools on
  `oso mcp`. They get their own ADR after the terminal works (Q16).

### Restarts under `oso dev` (Q17)

- **A terminal survives a restart, on Unix:** the old instance stops reading, and hands each PTY's
  master to the new instance with `SCM_RIGHTS`, with the child's process id, the screen and the
  scrollback re-encoded as escape sequences, and the modes. The new instance feeds that text to a new
  `Term`, then reads the PTY from where the old one stopped: bytes written meanwhile wait in the
  kernel. A resize afterwards makes a full-screen program draw itself again.
  - The child's parent becomes the system's, so the new instance learns of its exit from the PTY's
    end, not from `waitpid`.
  - The carried text travels as ADR 0090's unsaved text does, in memory, and counts toward its 64 MiB.
  - The bridge's connection ends with the old instance. `oso mcp` connects again to the instance that
    now holds its terminal, and that connection asks again (ADR 0085).
- **Until that step lands, a running terminal holds a restart,** as a modified secret buffer does
  ([ADR 0090](0090-oso-dev-carries-unsaved-text.md)), and the echo area says which terminal.
- A release build has no restart handoff, and its terminals end with it.

### Cost

| What | Budget |
|---|---|
| Key press in a terminal to photon, with a child that echoes at once | p99 ≤ 8 ms, as for text |
| UI thread per frame while a terminal prints without pause | p99 ≤ 8 ms per frame, at most one snapshot per frame |
| Parse throughput on the terminal's thread | ≥ 100 MB/s of plain text |
| Snapshot of a 300 × 100 screen | ≤ 0.3 ms on the terminal's thread |
| Rows shaped per frame | only rows the snapshot marks changed |
| Memory | scrollback × columns × 24 bytes per terminal, plus one snapshot |
| Copy mode of 10,000 lines × 200 columns | first screen ≤ 50 ms, built off the UI thread |

New `just perf` flows measure the first three, with a child that writes 100 MB and one that echoes
keys.

## Steps

One pull request each. Each step updates the docs it changes.

1. **The PTY.** `oso-platform::process` in release builds, `process::pty` on macOS and Linux, the
   marker and `just release-safety` change, and unit tests with `/bin/sh`.
2. **`oso-term`.** The crate and its `layers.txt` rows, `alacritty_terminal` behind `Engine`, the
   terminal's thread, snapshots, the policy listener and every table row's test, the second scanner,
   the key encoder, and a fuzz target fed random bytes. No UI.
3. **The terminal view.** `terminal`, drawing snapshots, the 16 faces, semi-char and char mode, the
   input method, resize, the wheel, paste, echo off, the channel's rules for terminal buffers,
   end-to-end flows, and the `just perf` flows.
4. **Copy mode,** OSC 7, OSC 133 marks, links shown, OSC 52 writes and notifications.
5. **`claude`.** The MCP config file, the channel start, the question that names the terminal,
   `oso mcp` finding its instance, and an end-to-end flow with a stand-in program for `claude`.
6. **The restart handoff of terminals** under `oso dev`.

Steps 1 and 2 add dependencies, and each goes through the `add-dependency` skill first.

## Questions

For the author, the recommended option first.

### Engine and crates

1. **The engine.**
   (a) `alacritty_terminal` 0.26 (ADR 0015), behind `oso_term::Engine`, with oso's second scanner for
   OSC 7, 133, 7501, 9 and 777, and images later in oso. Pure Rust; the gaps are oso's to fill.
   (b) Build with both, as `open-questions.md` says, and choose from the Claude Code TUI's rendering
   and bulk output. Measured, but a step of throwaway work, and libghostty-vt needs Zig on three OSes,
   has no official Rust binding, and parses untrusted output in code that is not memory safe.
   (c) libghostty-vt. Images, shell integration and the exact parity with ghostel the author knows,
   at the cost above.
2. **The PTY.**
   (a) oso's own `process::pty` over `libc`'s `openpty`, with one `unsafe` `pre_exec` hook. No new
   crate, and the spawn rules of ADR 0010 hold by construction.
   (b) `portable-pty` (WezTerm's). ConPTY comes with it, but it spawns with its own `CommandBuilder`,
   outside `oso-platform::process`'s allowlist and log, and brings its own dependencies.
   (c) `alacritty_terminal`'s `tty` module. Already compiled in, but it spawns with
   `std::process::Command` outside the module, reads `SHELL` itself and on macOS runs `/usr/bin/login`.
3. **Windows.**
   (a) Deferred until Windows draws text and has the channel's named pipe; the module's interface is
   shaped for ConPTY.
   (b) ConPTY in step 1. Ready earlier, but untestable in the running app on Windows today.
4. **Threads.**
   (a) One thread per terminal that owns the PTY and the `Term`, with at most one snapshot in flight.
   The UI never waits on the parser.
   (b) `Term` behind a mutex the UI thread locks to draw, as Alacritty and Zed do. Simpler, but bulk
   output can hold the lock for a parse.
   (c) One thread for all terminals. Fewer threads, but one busy terminal slows the others.

### The buffer and its input

5. **Searchable text.**
   (a) Copy mode: a frozen read-only buffer of screen and scrollback, where every text command works.
   (b) isearch and line search run on the live grid. No copy, but each command needs a grid version,
   and the text moves while one searches.
6. **Scrollback.**
   (a) 10,000 lines, at most 100,000, set in `config.toml`. Up to about 48 MB a terminal at 200
   columns.
   (b) 2,000 lines, as VS Code's and vterm's defaults are near. Less memory, but a long Claude Code
   session scrolls past it.
7. **Keys.**
   (a) Semi-char by default, keeping `C-x`, `M-x` and `C-c` as a prefix (`C-c C-c` sends `C-c`), `C-q`
   to send any key, and char mode on `C-c C-k`.
   (b) As (a), but `C-c` goes straight to the program and `C-x` is the only prefix kept. `C-c` reaches
   Claude Code with one key, but oso's terminal commands then need another prefix.
   (c) Char mode by default, with one key to leave it, as a plain terminal feels.
8. **Paste of several lines without bracketed paste.**
   (a) Ask first, as Ghostty and VS Code warn. One question for a shell that does not bracket.
   (b) Paste as typed. Fewer questions, but a copied line break runs a command.

### Escape sequences

9. **OSC 52 writes.**
   (a) Into oso's kill ring only, with a message, as channel kills are (ADR 0085). Text a program
   plants never reaches another app's paste.
   (b) Into the OS clipboard, as Ghostty and WezTerm allow. Claude Code's copy then reaches a browser,
   but so does anything printed into the terminal.
   (c) Denied.
10. **Images (Kitty graphics, Sixel, iTerm2).**
    (a) Denied in M4; the pop-up terminal after M4 brings them with a pure-Rust or isolated decoder
    (T10).
    (b) Kitty graphics in M4, for parity with ghostel. A decoder of untrusted data in M4.

### Secrets and the agent

11. **What the channel reads of a terminal.**
    (a) Nothing: `text` is refused and `state` has no rows or title, while showing a terminal does not
    stop the channel; echo off does.
    (b) Its rows, as from any ordinary buffer. The agent could read a build's error in a shell, but also
    whatever else was printed there.
    (c) Terminals count as buffers that may hold secrets. The safest, but the channel would then stop
    whenever Claude Code's own terminal is on screen.
12. **Echo off.**
    (a) Treated as a secret buffer shown: no macro recording, the channel waits, and secure input on
    macOS.
    (b) Not detected. Simpler, but a password typed at `sudo` is recorded into a macro being defined.
13. **The environment.**
    (a) The allowlist, `SSH_AUTH_SOCK`, the terminal's own names and `[terminal] env`, with a login
    shell building the rest.
    (b) The whole environment oso was started with, as Emacs passes `process-environment`. Familiar,
    but on a Dock launch it is the GUI's thin environment anyway, and from `oso dev` it carries
    `CARGO_*` and whatever the person's shell held.
14. **The channel and `claude`.**
    (a) `claude` starts the channel if it is off. One command to get the integration.
    (b) The person starts the channel themselves. One more step each session, unless `config.toml`
    starts it.
15. **The connection question for the bridge `claude` started.**
    (a) Still asked, naming the terminal. ADR 0085 holds unchanged; one `y` per session and per
    restart.
    (b) Not asked when the bridge's parent is the process oso started in that terminal. Smoother, but
    the parent is known only best-effort, and a process id can be reused.
16. **Hooks and the session list.**
    (a) A separate ADR after step 5, with the open items on hooks, OSC 7501 and `oso integration
    install claude`.
    (b) In this ADR. One decision, but a much longer one, made before the terminal can be tried.

### Restarts

17. **A terminal under `oso dev`'s restart.**
    (a) Handed to the new instance over `SCM_RIGHTS`, screen and scrollback carried in memory (step 6);
    until then a running terminal holds the restart.
    (b) A running terminal always holds the restart, and the author runs `claude` in a second oso that
    is not rebuilt. No fd passing, but the agent and the oso it edits are then two windows.
    (c) Restarts end terminals, and `claude --resume` brings the session back. Simple, but the agent
    loses its turn at each edit to oso's source.

### Dependencies (for the `add-dependency` skill, not decided here)

- `alacritty_terminal` 0.26.0, `default-features = false`, in `oso-term` (Apache-2.0). It brings
  `vte` 0.15, `base64`, `bitflags`, `home`, `parking_lot`, `polling`, `regex-automata`,
  `unicode-width` 0.2, and on Unix `rustix`, `rustix-openpty` and `signal-hook`; on Windows `miow`,
  `piper` and `windows-sys`. Some are in the lock already. `regex-automata` is the `regex` crate's
  own engine, used for `alacritty_terminal`'s grid search, which oso does not call; AGENTS.md's
  regex rule needs a note for it.
- `vte` 0.15 as a direct dependency of `oso-term`, for the second scanner; it is the version
  `alacritty_terminal` uses.
- `log` in release builds of `oso-platform`, already in the lock.
- No PTY crate under option 2 (a).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0015](0015-terminal.md), "PTYs go through the process module: openpty on Unix, ConPTY on
  Windows": macOS and Linux first, ConPTY deferred. "OSC 8 links show the real target before
  opening": and opening waits for the URL policy. The searchable text is copy mode's frozen buffer.
- [ADR 0047](0047-process-spawning-and-restart-handoff.md), "behind the `dev` feature until a
  release feature needs it" and the marker `oso-dev-process-spawn`: the terminal is that feature,
  the module ships, and the marker is `oso-process-spawn`, no longer checked in release binaries.
  The state handoff carries terminals as above.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "Off by default. The `Person` command
  `agent-channel-start` starts it": and `claude`, a `Person` command, starts it too.

## Consequences

- The release build can spawn for the first time. Every spawn still goes through one module, with its
  allowlist and log, but `just release-safety` loses its spawn check.
- `alacritty_terminal`'s own PTY code ships unused. A guard test keeps it unused, and an audit of the
  crate covers it anyway.
- A new crate, `oso-term`, and a new row in `layers.txt`.
- The UI thread's cost of a terminal is one snapshot per frame and the rows that changed, however much
  a program prints.
- An agent in a terminal cannot read the terminals through oso, and cannot type into any. It still
  reads and runs whatever its own tools allow; oso's trust does not reach inside a terminal.
- Copy mode makes every text command work on terminal output, at the cost of a copy each time it is
  entered.
- Claude Code reaches oso through `--mcp-config` and the channel, with a question per connection.
  There is no IDE integration over a WebSocket, as oso has no network code.
- Steps 5 and 6 are what make self-hosting work: until step 6, a running `claude` holds `oso dev`'s
  restarts.
- Follow-up: the hooks ADR (Q16), ConPTY, mouse clicks, opening links once the URL policy exists,
  images, and moving between OSC 133 prompts.
- `open-questions.md`'s M4 items on the engine, the PTY per OS and the escape-sequence list are
  answered here once this ADR is accepted.

## Alternatives considered

- **libghostty-vt** and **`portable-pty`:** see Q1 and Q2.
- **Mirroring the grid into a rope buffer all the time, as vterm and eat do:** every command works,
  but each line of output costs an edit, and ADR 0015 rejected it for heavy output. Copy mode builds
  the text only when asked.
- **A separate pty host process, as VS Code has:** terminals would survive any restart, release ones
  too, but it is a second binary to keep in step, and `SCM_RIGHTS` covers the restart that matters.
- **Claude Code's IDE protocol (a WebSocket MCP server and a lock file):** gives the TUI's IDE
  features, but needs a localhost server, which ADR 0010 rules out.
- **`claude` started directly, not through the login shell:** no shell startup, but a Dock launch's
  `PATH` usually lacks `claude`, and the person's shell setup would not apply. `[claude] program`
  covers those who want it.
- **The MCP config inline on the command line:** no file, but the JSON then needs shell quoting, and
  it shows in every process listing.
- **Shell commands that call oso commands through an escape sequence, as `vterm-eval-cmds`:** a
  program's output would then run oso commands. Denied, with OSC 51.
