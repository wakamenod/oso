# 0120. The terminal

- Status: Proposed
- Date: 2026-11-12

## Context

M4 is "develop oso in oso" ([roadmap](../roadmap.md)), and its first item is a terminal. The author
uses no separate terminal app: their Emacs terminal does shell work, `cargo` and `just`, `ssh`,
full-screen TUIs and images. oso's terminal has to replace it, not only host an agent. Claude Code
runs in it as its official TUI ([ADR 0031](0031-claude-code-integration.md)). The author's Emacs
setup stutters because everything runs on one thread, so speed comes first here too.

This ADR is the terminal alone. Sessions of agents, their state and the `claude` command are
ADR 0121's, and reviewing what an agent changed is ADR 0122's, both later. This ADR provides what
they need from a terminal and nothing of theirs.

What is decided already:

- **A terminal is a view of its own** that renders a cell grid directly, and opening one is allowed in
  a restricted project, as an explicit action ([ADR 0015](0015-terminal.md)). ADR 0015 chose
  `alacritty_terminal`; [`open-questions.md`](../open-questions.md) then asked, on 2026-11-07, to build
  with both it and libghostty-vt when M4 starts, and decide in an ADR. The escape-sequence policy so
  far: no OSC 52 reads, no sequence that echoes data back, and an OSC 8 link shows its real target
  before it opens.
- **The agent channel ships in release builds, off by default** ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).
  Each command declares its reach (`Look`, `Change`, `Person`); anything that spawns a process or
  shows what a buffer that may hold secrets withholds is `Person`.
- **Only `oso-platform::process` spawns,** with an allowlisted environment, an executable from an
  absolute path or a filtered `PATH`, and a log line per spawn ([ADR 0010](0010-process-and-network-policy.md),
  [ADR 0047](0047-process-spawning-and-restart-handoff.md)). It is behind the `dev` feature, and
  `just release-safety` checks that its marker is in no release binary. Opening a file or a project
  never executes anything ([ADR 0009](0009-project-trust.md)).
- **oso has no network code** ([ADR 0010](0010-process-and-network-policy.md)). Image decoders are
  pure Rust or isolated ([`security.md`](../security.md), T10).

The forces, in the order of the project's priorities:

- **Speed.** A `cat` of a large log, or a TUI redrawing its whole screen, must not cost the UI thread
  a frame. No work on the UI thread may grow with the bytes a program prints. Key press to photon is
  p99 ≤ 8 ms ([roadmap](../roadmap.md)).
- **Security.** Terminal output is untrusted input: a file `cat`ed, a page an agent fetched, a remote
  host over `ssh`. An escape sequence is T7 in [`security.md`](../security.md), and an agent steered
  by text it read is ADR 0085's threat. A terminal is also the first thing a release build spawns.
- **Self-hosting.** The author will run agents in an oso started by `oso dev`, and each edit to oso's
  source restarts it. A restart that ends the terminals ends the agents mid-turn.

### The two engines

- **`alacritty_terminal` 0.26.0** (Apache-2.0), read from its source:
  - pure Rust; `Term` parses through `vte` 0.15 and keeps a grid with scrollback, damage tracking, the
    kitty keyboard protocol, OSC 8, synchronized updates (mode 2026), bracketed paste and mouse modes;
  - everything that would answer the program or reach outside goes to the embedder as an `Event`
    (`ClipboardStore`, `ClipboardLoad`, `ColorRequest`, `TextAreaSizeRequest`, `PtyWrite`, `Title`,
    `Bell`), so the policy can live in oso's listener;
  - no OSC 7, OSC 133, OSC 9 or 777, no images, and no hook for sequences it ignores;
  - its `tty` and `event_loop` modules spawn the shell with `std::process::Command` themselves, and
    are always compiled.
- **libghostty-vt**, the engine of the author's Emacs terminal (ghostel):
  - has OSC 7, OSC 133, the Kitty graphics protocol and the rest of Ghostty's terminal;
  - is Zig, so a Zig toolchain on three OSes, and has no official Rust binding;
  - parses untrusted output in code that is not memory safe;
  - herdr, a terminal host for coding agents, vendors it with local patches and generated bindings of
    about 465 KB, and runs Claude Code's TUI on it.

### What others do

- **vterm** (Emacs) wraps libvterm, a C library. Every key goes to the terminal except a list (`C-c`,
  `C-x`, `C-u`, `C-g`, `C-h`, `C-l`, `M-x`, `M-o`, `C-y`, `M-y`); `C-q` sends the next key as it is.
  `vterm-copy-mode` freezes the buffer for Emacs commands. The shell can call Elisp functions the
  user listed (`vterm-eval-cmds`) through an escape sequence.
- **eat** (Emacs) is pure Elisp, with semi-char, char, Emacs and line modes, shell integration and
  Sixel images. **term.el** has char and line modes, escaped by `C-c`, and is slow with heavy output.
- **WezTerm** has its own parser, lets OSC 52 write the clipboard, and shows Kitty, iTerm2 and Sixel
  images. Its `portable-pty` crate does ConPTY on Windows.
- **Ghostty** asks before OSC 52 reads (`clipboard-read = ask`), lets it write
  (`clipboard-write = allow`), warns before a paste that could run a command, and does not report the
  title (`title-report = false`) unless set.
- **Zed** renders `alacritty_terminal`'s grid in its own GPU UI.
- **VS Code** uses xterm.js and node-pty. Its pty host is a separate process, so terminals survive a
  reload of the window. It warns before a multi-line paste, and does shell integration with OSC 633
  and OSC 133.
- **herdr** keeps a background server that owns every PTY, hands live PTYs to a new server on upgrade,
  and counts work per byte × panes × clients as the hot path to benchmark. It reads an agent's state
  from the screen, the title's spinner and OSC 9;4 progress, after its hook-reported state went stale
  (v0.6.7); ADR 0121 builds on that.

## Decision

Proposed. The text below takes the first option (a) of each question under
[Questions](#questions); the author's answers may change it.

### The engine and the crates (Q1, Q2, Q3)

- **The engine is chosen by a time-boxed comparison (Q1),** the first step: both engines behind one
  trait, fed the same recorded and generated output, measured on the criteria below, in at most three
  days of work. The result is written into this ADR before it is accepted. `alacritty_terminal` is
  kept unless libghostty-vt is clearly better on what the author needs, enough to pay for Zig and
  memory-unsafe parsing.
  - **Correctness:** the Claude Code TUI (recorded from a scratch session, scrubbed), `vim`, `htop`,
    `less`, `lazygit`, wide and combining characters, and `vttest`'s basic screens.
  - **Speed:** parse throughput, the time to build a snapshot, and memory per 10,000 lines of
    scrollback.
  - **Cost:** clean build time and toolchain on macOS and Linux, binary size, and what `cargo-deny`
    and the fuzz target say after an hour each.
  - **Gaps:** what oso must add to each for this ADR: OSC 7, OSC 133, OSC 9, images.
- **A new crate, `oso-term`,** holds the terminal model: the engine, oso's escape-sequence policy,
  the key encoder, the terminal's thread and the snapshots it publishes. It sits beside `oso-render`
  and `oso-syntax`:

  ```
  [workspace]
  oso
  oso-render oso-syntax oso-term
  oso-core oso-platform
  ```

  - It uses `oso-platform` (the PTY) and `oso-core` (the key type). It does not use `oso-render`; the
    tip crate turns a snapshot into rows `oso-render` draws, as it does for buffers.
  - Only the tip crate uses it. It parses untrusted bytes, so it gets a fuzz target and its own
    `AGENTS.md`.
  - **The engine is behind a trait, `oso_term::Engine`:** feed bytes, resize, publish a snapshot, read
    rows, and report effects (a reply, a clipboard write, a title, a bell, a notification). The policy
    acts on those effects, not inside the engine, so it is the same for either engine.
  - With `alacritty_terminal`, a second `vte` parser fed the same bytes reads the sequences it ignores
    (OSC 7, 133, 9, 777), with each OSC string capped at 4 KiB.
- **The PTY is oso's own, in `oso-platform::process::pty` (Q2),** Unix only for now:
  - `openpty` from `libc`; the master is close-on-exec and non-blocking;
  - the child starts through the module's `spawn`, so the executable rules, the environment allowlist
    and the log line hold. A `Spawn` gains `terminal: Option<&Pty>`, and a `pre_exec` hook calls
    `setsid` and `TIOCSCTTY`: the module's one `unsafe` block, with its `// SAFETY:` comment;
  - `TIOCSWINSZ` to resize, `tcgetattr` on the master to see whether the slave echoes, `tcgetpgrp` for
    the foreground process group, and `SIGHUP` to that group when the terminal closes;
  - the spawn runs on the terminal's thread, never the UI thread, since `fork` of a large process takes
    milliseconds;
  - the engine's own PTY code, where it has any, is never called, and a guard test fails if `oso-term`
    names it.
- **macOS and Linux first; Windows is deferred (Q3)** until Windows draws text
  ([ADR 0036](0036-text-system-and-glyph-atlas.md)). The module's interface is the one ConPTY will
  fill.
- **`oso-platform::process` ships in release builds.** It leaves the `dev` feature, the marker becomes
  `oso-process-spawn`, and `just release-safety` stops looking for it; its other checks stay. The
  architecture test still allows `#[expect(clippy::disallowed_methods)]` only in that module.

### Threads, snapshots and drawing (Q4)

- **Each terminal has one thread** that owns the PTY's master and the engine. It `poll`s the master and
  a wake-up pipe, reads at most 64 KiB at a time, parses, writes what the UI queued, and answers the
  UI's requests (resize, scroll, copy mode, close).
  - The UI thread never takes a lock the parser holds, never reads or writes the PTY, and never parses.
  - Writes queue on the terminal's thread, at most 16 MiB, so a large paste never blocks the UI.
  - When oso falls behind it stops reading, and the child blocks on a full PTY. Nothing grows without
    bound.
- **A snapshot** holds the visible rows' cells (character, colors, flags, link id), the cursor and its
  shape, which rows changed, the title, and whether echo is off. It is built from the rows the engine
  marks damaged, and sent through the wake-up path the loader and workers use. **At most one snapshot
  is in flight:** a new one is built only after the UI took the last, so bulk output costs the UI one
  snapshot per frame however fast it comes.
- **Keys are encoded on the terminal's thread,** against the modes current there, so a key never
  misses a mode the program just set.
- **Drawing** puts each cell at its column, as the grid placed it; oso does not measure widths again.
  Only rows marked changed are shaped, through the shaped-row cache text views use. Colors come from
  the theme: 16 new faces, `terminal-color-0` to `terminal-color-15`, in each built-in theme
  ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)). 256-color and true color cells are drawn
  as given. Cells are drawn in grid order, with no reordering
  ([ADR 0066](0066-text-stays-in-logical-order.md)).

### A terminal as a buffer (Q5, Q11, Q14)

- **A terminal is a buffer of kind `Terminal`,** as [`architecture.md`](../architecture.md)'s table has
  it. `oso-core`'s `Buffer` gains the kind and nothing of the grid; the tip crate maps buffer ids to
  terminals.
- **Many terminals are buffers in oso's windows (Q5).** `terminal` switches to the last terminal or
  starts one; with a prefix argument it always starts a new one. Names are `*terminal*`,
  `*terminal*<2>` and so on. Splits ([ADR 0112](0112-window-splits.md)) and `C-x b` place them, so
  there is no tab bar or split manager inside a terminal; oso's own tabs come later
  ([roadmap](../roadmap.md)). The program's title shows in the modeline, never in the buffer's name.
- **In split windows,** the PTY takes the size of the window that last selected the terminal; others
  showing it draw the same grid, clipped or padded.
- **Copy mode (Q14):** `C-c C-j` freezes the screen and scrollback into a read-only internal buffer,
  built on the terminal's thread, shown at the same place. Every text command works there: isearch,
  migemo, avy, the region, `M-w`. `q` or `C-c C-k` returns to the live terminal, which kept running.
- **Scrollback (Q11):** 10,000 lines by default, set in `config.toml`'s `[terminal] scrollback`, at most
  100,000. A cell takes about 24 bytes, so 10,000 lines of 200 columns take up to 48 MB. Copy mode's
  buffer is dropped when the person leaves it.

### Input (Q12, Q13, Q6)

- **Semi-char mode is the default (Q12):** every key goes to the program, encoded, except `C-x` and
  `M-x`; `C-c` as a prefix (`C-c C-c` sends `C-c`, `C-c C-j` copy mode, `C-c C-k` char mode, any other
  key after `C-c` sends both); `C-q`, which sends the next key as it is; and `C-g` while a prompt or a
  key sequence of oso's is open.
- **Char mode** sends every key except `C-c C-k`, which leaves it. Line mode is not built.
- **The encoder** writes the xterm forms and, once a program pushes kitty keyboard flags, the kitty
  protocol's `CSI u` forms, so Shift+Enter reaches a program that reads it.
- **The input method:** the preedit is drawn at the terminal's cursor, the committed text goes to the
  program as UTF-8, and the candidate window is placed at the cursor cell even while the program hides
  the cursor, as Claude Code's input box does.
- **Paste (Q13):** `C-y` and the OS paste send text. With bracketed paste on, oso removes `ESC [ 201 ~`
  and other control characters except tab and new line first. With it off, a paste that holds a new
  line asks `Paste N lines into a program that may run them? (y or n)`.
- **The mouse (Q6):** while a program asks for mouse reports (1000, 1002, 1003, 1006), clicks, drags,
  the wheel and motion go to it; with Shift held they are oso's. Otherwise a drag selects cells and the
  wheel scrolls the scrollback. A click selects the window first, as oso's mouse support grows
  ([`open-questions.md`](../open-questions.md)).
- **`ssh` and other programs** need nothing of their own: `TERM=xterm-256color` is in every remote
  terminfo, and `SSH_AUTH_SOCK` passes through (see the environment).

### Escape sequences (Q7, Q8, Q9, Q10)

Each sequence is allowed, denied, or answered from oso's own state. A denied one is dropped without a
reply. Text a sequence carries for oso to show (a title, a notification, a link target) has control,
bidi and invisible characters dropped and is cut at 256 bytes.

| Sequence | What oso does |
|---|---|
| OSC 52 clipboard read | Denied (ADR 0015). |
| OSC 52 clipboard write | Into the kill ring. Into the OS clipboard too when the person pressed a key in that terminal in the last 2 s; the echo area names the terminal and the length (Q8). |
| OSC 0, 1, 2 title | Shown in the modeline, and kept for the host API. |
| Title report (`CSI 21 t`), OSC 50 font, `DECRQSS` | Denied (ADR 0015). |
| Title stack (`CSI 22 t`, `CSI 23 t`) | Allowed. |
| OSC 8 hyperlinks, and URLs and `path:line` found in the text | Underlined; the target shows in the echo area at point. `C-c RET` or a Command-click opens it (Q7). |
| OSC 4, 10, 11, 12 color queries | Answered from the theme. |
| OSC 4, 10, 11, 12 color changes | Applied to that terminal only, until it resets. |
| Device attributes, device status, cursor position, mode reports, kitty keyboard query, text area size | Answered: they report the terminal's own state. |
| Window moves, resizes and iconify (`CSI t` 1–9) | Denied. |
| Bracketed paste, focus reports (1004), synchronized updates (2026), alternate screen, cursor shape, undercurl and underline color | Allowed. |
| Mouse reports, kitty keyboard protocol | Allowed (see Input). |
| OSC 7 working directory | Sets the terminal's directory for `C-x C-f` and `C-x p f`, if it names a local path. |
| OSC 133 prompt marks | Kept by row: moving between prompts, and the host API. |
| OSC 9 and 777 notifications, OSC 9;4 progress | An echo-area message and a modeline mark on a terminal not shown; progress in the modeline; all kept for the host API (Q9). |
| Bell | The modeline flashes, as [ADR 0097](0097-visual-aids.md)'s flash does, and a terminal not shown is marked. |
| Kitty graphics | Shown, from the image step on (Q10). |
| Sixel, iTerm2 images (OSC 1337) | Denied in M4 (Q10). |
| OSC 51 and other "run this editor command" sequences | Denied. A program in a terminal never runs an oso command. |

- **Links (Q7):** `http` and `https` targets open with the OS's opener, through `oso-platform::process`,
  after the echo area shows the whole target; `file:` targets on this host and `path:line` open in oso,
  as `C-x C-f` would; any other scheme asks first. This is the URL-opening policy for terminals.
- **Images (Q10):** the Kitty graphics protocol's direct transmission, raw RGB, RGBA and PNG. PNG is
  decoded by a pure-Rust decoder on a worker, never the terminal's or the UI thread; an image is at most
  16 MiB encoded and 4096 × 4096 pixels, and a terminal holds at most 64 MiB of images, dropping the
  oldest. Transmission from a file or shared memory is denied, so a remote program cannot make oso read
  a local file. `oso-render` draws them as textures placed on cells.
- The policy is one function in `oso-term` per effect, and each row of the table has a test that feeds
  the sequence and checks the reply, the kill ring, the modeline or what was dropped.

### The host API for ADR 0121

`oso-term` gives the tip crate, in-process and off the UI thread where it reads the grid:

- rows of the screen and the scrollback as text, by range or the last N non-empty;
- the title, OSC 9;4 progress, OSC 7's directory and OSC 133's marks, each with when it last changed;
- bell and notification events, and whether the terminal is shown and selected;
- the foreground process group and its program's name, so a host can tell when the program changes;
- names and values of extra environment variables for a terminal's child, given by the caller, logged
  by name only.

These are for oso's own code. What an agent may read through the channel is below.

### Agents and the terminal (Q15, Q16, Q17)

- **Only a person types into a terminal (Q15).** The channel never writes to a PTY: `insert` and every
  `Change` command are refused in a terminal buffer, and copy mode's buffer is read-only. An agent
  typing into a terminal would run anything, and would answer Claude Code's own permission prompts,
  bypassing them. oso features the person runs, such as sending review comments to a session as a
  bracketed paste (ADR 0122), are the person's typing.
- **The channel reads no terminal text unless the person allows it, per terminal (Q16).** By default
  `text` is refused for a terminal and `state` lists it by id, name and kind, with whether a process
  runs, but no rows and no title. The `Person` command `terminal-allow-agent-read` allows it for the
  selected terminal until it closes, and the modeline shows `agent reads`; `terminal-deny-agent-read`
  takes it back. Copy mode's buffer follows its terminal.
- **Echo off is a secret (Q17).** While the PTY does not echo (a password prompt, `sudo`, `ssh`), keys
  typed there are not recorded into a keyboard macro, the modeline says `secret`, the channel waits as
  for a secret buffer shown, its rows are withheld even where reading was allowed, and on macOS secure
  input is on while that terminal is shown and selected
  ([ADR 0073](0073-secure-input-in-secret-buffers.md)).
- **Showing a terminal does not stop the channel.** A terminal is not a buffer that may hold secrets in
  ADR 0085's sense, or the channel could never act while an agent's own terminal is on screen.
- **Reach:** `Person` for `terminal`, every key and paste sent to a terminal, the mode switches, opening
  a link, allowing reading, and closing or killing a terminal (which asks while a process runs); `Look`
  for switching to a terminal, copy mode, and moving and searching in it.
- **Terminal text never reaches the disk:** no auto-save, no emergency copy, no session file. The kill
  ring holds what the person copies.
- The author named the other powers no agent gets, which are not this ADR's: answering its own
  prompts, trusting a project, pushes that cannot be undone, and editing oso's configuration
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md) has the first, second and fourth).

### Restricted projects and the environment (Q18)

- **A terminal opens in a restricted project,** only by the person's command. Opening a file or a
  project never opens one, and no start of oso brings one back.
- **Its directory** is the project's root, else the buffer's directory, else the home directory.
- **What runs in it is the person's:** their shell, its startup files, its prompt. oso's trust limits
  what oso starts, not what anyone runs in a terminal; the docs say so where they explain trust.
- **`terminal` starts the login shell** (`$SHELL`, or the account's shell, absolute, with `-l`).
- **The environment (Q18)** is the module's allowlist plus `SHELL`, `COLORTERM=truecolor`,
  `TERM=xterm-256color`, `TERM_PROGRAM=oso`, `TERM_PROGRAM_VERSION`, `SSH_AUTH_SOCK`, the names in
  `config.toml`'s `[terminal] env`, and what the caller of the host API adds. The login shell builds
  the rest.

### Restarts under `oso dev` (Q19)

- **A terminal survives a restart, on Unix:** the old instance stops reading and hands each PTY's
  master to the new one with `SCM_RIGHTS`, with the child's process id, the modes, and the screen and
  scrollback re-encoded as escape sequences. The new instance feeds that to a new engine, then reads
  the PTY where the old one stopped; bytes written meanwhile wait in the kernel. A resize afterwards
  makes a full-screen program redraw.
  - The child's parent becomes the system's, so the new instance learns of its exit from the PTY's end.
  - The carried text travels in memory as ADR 0090's unsaved text does, and counts toward its 64 MiB.
- **Until that step lands, a running terminal holds a restart,** as a modified secret buffer does
  ([ADR 0090](0090-oso-dev-carries-unsaved-text.md)), and the echo area says which.
- A release build has no handoff, and its terminals end with it.

### Cost

| What | Budget |
|---|---|
| Key press in a terminal to photon, with a child that echoes at once | p99 ≤ 8 ms |
| UI thread per frame while one terminal, and while 8 terminals, print without pause | p99 ≤ 8 ms, at most one snapshot per terminal per frame |
| Parse throughput on a terminal's thread | ≥ 100 MB/s of plain text |
| Snapshot of a 300 × 100 screen | ≤ 0.3 ms on the terminal's thread |
| Rows shaped per frame | only rows marked changed |
| Memory | scrollback × columns × 24 bytes per terminal, plus one snapshot and its images |
| Copy mode of 10,000 lines × 200 columns | first screen ≤ 50 ms, built off the UI thread |

New `just perf` flows measure the first three, with programs that write 100 MB and that echo keys.

## Steps

One pull request each. Each step updates the docs it changes.

1. **The comparison** (at most three days): both engines behind `Engine`, headless, the criteria
   above, and the result and the choice written into this ADR. The losing engine's code is not merged.
2. **The PTY:** `oso-platform::process` in release builds, `process::pty` on macOS and Linux, the
   marker and `just release-safety`, and unit tests with `/bin/sh`.
3. **`oso-term`:** the crate and its `layers.txt` rows, the chosen engine, the terminal's thread,
   snapshots, the policy and every table row's test, the key encoder, the host API, and a fuzz target.
4. **The terminal view:** `terminal`, drawing, the 16 faces, semi-char and char mode, the input method,
   resize, the mouse, paste, echo off, the channel's rules, end-to-end flows and `just perf` flows.
5. **Copy mode,** links, OSC 7 and 133, OSC 52 writes, notifications, progress and the bell.
6. **Kitty graphics.**
7. **The restart handoff** under `oso dev`.

Steps 1, 3 and 6 add dependencies, each through the `add-dependency` skill first.

## Questions

For the author, the recommended option first.

### Engine and crates

1. **The engine.**
   (a) A time-boxed comparison first (step 1, at most three days), keeping `alacritty_terminal` unless
   libghostty-vt is clearly better. The 2026-11-07 plan, with a cap on its cost.
   (b) `alacritty_terminal` now, no comparison. Saves the step; OSC 7, 133, 9 and images are then oso's
   to add, which the comparison would have priced.
   (c) libghostty-vt now. Parity with ghostel and images built in; Zig on three OSes, hand-kept
   bindings, and memory-unsafe parsing of untrusted output in oso's process.
2. **The PTY.**
   (a) oso's own `process::pty` over `libc`, one `unsafe` `pre_exec`. No crate; ADR 0010's spawn rules
   hold by construction.
   (b) `portable-pty`. ConPTY comes with it, but it spawns with its own builder, outside the module's
   allowlist and log.
   (c) The engine's own PTY code, where it has any. Already there, but it spawns outside the module.
3. **Windows.**
   (a) Deferred until Windows draws text; the interface is shaped for ConPTY.
   (b) ConPTY now. Ready earlier, but nothing can be checked in the app on Windows yet.
4. **Threads.**
   (a) One thread per terminal owning the PTY and the engine, at most one snapshot in flight.
   (b) The engine behind a mutex the UI locks to draw, as Alacritty and Zed do. Simpler; bulk output can
   hold the lock for a parse, which is the stutter the author wants gone.
   (c) One thread for all terminals. Fewer threads; one busy terminal slows the rest.

### What replaces a terminal app

5. **Many terminals.**
   (a) Each is a buffer in oso's windows and splits; no tab bar until oso has tabs.
   (b) A tab bar and splits inside the terminal, as tmux or herdr. Familiar, but a second window system
   beside oso's.
6. **The mouse.**
   (a) Full reports to programs that ask (clicks, drags, wheel, motion), Shift to override, drag-select
   otherwise. `lazygit` and `htop` then work by mouse.
   (b) The wheel only in M4. Less work; mouse-driven TUIs need the keyboard.
7. **Links.**
   (a) Open `http`/`https` with the OS's opener after showing the target, `file:` and `path:line` in oso,
   other schemes ask. Settles the URL policy for terminals.
   (b) Copy the target only, until a URL policy of its own. Nothing opens by accident, but links are
   half a feature.
8. **OSC 52 writes.**
   (a) Kill ring always; OS clipboard only within 2 s of the person's key in that terminal. Copy from
   `ssh`'d `vim` or `tmux` works; text a program prints on its own reaches only the kill ring.
   (b) OS clipboard always, as Ghostty and WezTerm. Simplest; anything `cat`ed can replace the clipboard.
   (c) Kill ring only. Safest; copying from a remote host to other apps does not work.
9. **Notifications, progress and the bell.**
   (a) In oso: echo area, modeline marks, progress in the modeline, all kept for ADR 0121; OS
   notifications come with ADR 0121, which decides when they are worth it.
   (b) OS notifications now. Needs a macOS notification binding, a new dependency, before sessions
   exist to use it.
10. **Images.**
    (a) Kitty graphics in M4 (step 6), direct transmission only, PNG through a pure-Rust decoder on a
    worker with size caps; Sixel and iTerm2 later. Parity where ghostel is used most; T10 allows
    pure-Rust decoders.
    (b) After M4, as the roadmap has it. No decoder now; the terminal app replacement waits for images.
    (c) All three formats in M4. Full parity; three parsers of untrusted data.
11. **Scrollback.**
    (a) 10,000 lines by default, at most 100,000, set in `config.toml`; up to about 48 MB a terminal at
    200 columns.
    (b) 2,000 lines. Less memory; a long session scrolls past it.

### Input

12. **Keys.**
    (a) Semi-char by default: `C-x`, `M-x` and the `C-c` prefix stay oso's (`C-c C-c` sends `C-c`), `C-q`
    sends any key, `C-c C-k` char mode.
    (b) `C-c` straight to the program, only `C-x` kept. One key for `C-c`; oso's terminal commands need
    another prefix.
    (c) Char mode by default, with one key to leave. Feels like a plain terminal; oso's keys need a mode
    switch.
13. **A multi-line paste without bracketed paste.**
    (a) Ask first, as Ghostty and VS Code. (b) Paste as typed; a copied line break runs a command.
14. **Searchable text.**
    (a) Copy mode: a frozen read-only buffer where every text command works.
    (b) isearch and line search on the live grid. No copy; each command needs a grid version, and the
    text moves while one searches.

### Agents

15. **Typing.**
    (a) Only a person: the channel never writes to a PTY.
    (b) The channel may type into a terminal the person allows. An agent could then answer another
    agent's permission prompts and run any command.
16. **Reading.**
    (a) No terminal text unless the person allows it per terminal, with a modeline mark.
    (b) Never. Simplest; an agent cannot read a build error in the person's shell even when asked to.
    (c) Always, as any buffer. Convenient; whatever was printed, tokens included, is the agent's.
17. **Echo off.**
    (a) Treated as a secret buffer shown. (b) Not detected; a password typed at `sudo` can enter a macro.
18. **The environment.**
    (a) The allowlist, `SSH_AUTH_SOCK`, the terminal's own names, `[terminal] env` and the host API's,
    with a login shell building the rest.
    (b) Everything oso was started with, as Emacs. Familiar; from `oso dev` that includes `CARGO_*` and
    whatever its shell held.

### Restarts

19. **A terminal under `oso dev`'s restart.**
    (a) Handed over with `SCM_RIGHTS`, screen and scrollback carried in memory (step 7); until then a
    running terminal holds the restart.
    (b) Always holds the restart; agents run in a second oso that is not rebuilt. No fd passing; the
    agent and the oso it edits are two windows.
    (c) Restarts end terminals. Simple; every edit to oso's source ends every shell and agent in it.

### Dependencies (for the `add-dependency` skill, not decided here)

- The engine: `alacritty_terminal` 0.26.0 with `default-features = false` (it brings `vte` 0.15,
  `base64`, `bitflags`, `home`, `parking_lot`, `polling`, `regex-automata`, `unicode-width` 0.2, and on
  Unix `rustix`, `rustix-openpty` and `signal-hook`), and `vte` 0.15 directly for the second parser;
  or libghostty-vt with a Zig toolchain and bindings oso keeps. `regex-automata` is the `regex` crate's
  own engine, used for `alacritty_terminal`'s search, which oso does not call; AGENTS.md's regex rule
  needs a note for it.
- A pure-Rust PNG decoder for step 6.
- `log` in release builds of `oso-platform`, already in the lock.
- No PTY crate under option 2 (a).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0015](0015-terminal.md), "The terminal engine is `alacritty_terminal`": chosen by step 1's
  comparison. "PTYs go through the process module: openpty on Unix, ConPTY on Windows": macOS and Linux
  first. "OSC 8 links show the real target before opening": and open under the rules above. The
  searchable text is copy mode's buffer.
- [ADR 0047](0047-process-spawning-and-restart-handoff.md), "behind the `dev` feature until a release
  feature needs it" and the marker `oso-dev-process-spawn`: the terminal is that feature, the module
  ships, and the marker is `oso-process-spawn`, no longer checked in release binaries. The state
  handoff carries terminals as above.

## Consequences

- A release build can spawn for the first time, still only through one module with its allowlist and
  log; `just release-safety` loses its spawn check.
- A new crate, `oso-term`, and a new row in `layers.txt`.
- The UI thread's cost of a terminal is one snapshot per frame and the rows that changed, however much a
  program prints, and however many terminals print.
- Agents cannot type into a terminal, and read one only where the person allowed it. What they run
  through their own tools is theirs; oso's trust does not reach inside a terminal.
- ADR 0121 gets screen rows, the title, progress, prompts, the directory, bells and the foreground
  program from the host API, and sets a terminal's extra environment through it.
- Until step 7, a running terminal holds `oso dev`'s restarts.
- Follow-up: ADR 0121 (sessions, `claude`, the MCP config, OS notifications) and ADR 0122 (review),
  ConPTY, Sixel and iTerm2 images, and moving between OSC 133 prompts.
- `open-questions.md`'s M4 items on the engine, the PTY per OS and the escape-sequence list are answered
  here once this ADR is accepted.

## Alternatives considered

- **libghostty-vt without a comparison, and `portable-pty`:** see Q1 and Q2.
- **Mirroring the grid into a rope buffer all the time, as vterm and eat do:** every command works, but
  each line of output costs an edit, and ADR 0015 rejected it for heavy output.
- **A separate pty host process, as VS Code and herdr have:** terminals would survive any restart,
  release ones too, but it is a second process and protocol to keep in step, and `SCM_RIGHTS` covers
  the restart that matters now.
- **Shell commands that call oso commands through an escape sequence, as `vterm-eval-cmds`:** a
  program's output, from any host, would then run oso commands.
- **Kitty graphics from a file or shared memory:** a remote program could name a local file for oso to
  read and show.
