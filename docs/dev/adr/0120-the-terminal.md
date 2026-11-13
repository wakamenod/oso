# 0120. The terminal

- Status: Accepted
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
- **ghostel** (Emacs), the author's terminal, wraps libghostty-vt. Beside its terminal buffers it
  has project terminals (`ghostel-project`, `ghostel-project-list-buffers`), shell commands that call
  listed Elisp functions (`ghostel-eval-cmds`), and terminals as the backend of `compile` and comint
  (`ghostel-compile`, `ghostel-comint`). The author adds **popterm**, a terminal in a child frame that
  one key shows and hides, at the top, center or bottom.
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

The author accepted this ADR on 2026-11-12, through the coordinator. They chose the first option (a)
of every question under [Questions](#questions) but Q14, where they chose both, and asked for more on
Q5 (project terminals, a floating terminal, shell commands that run oso commands, and the terminal as
the backend of compile and comint), Q12 (`C-s`, `C-k`, `M-p` and `M-n`) and Q18 (the shell and
whether it is a login shell, in `config.toml`). The text below describes all of it. How the extras
are made safe (the shell's commands, the `cd` the floating terminal types, a search while output
arrives, the seam for compile) is this ADR's design of what they asked, sent back to them through
the coordinator with it. The steps are under [Steps](#steps).

### The engine and the crates (Q1, Q2, Q3)

- **The engine is chosen by a time-boxed comparison (Q1),** the first step: both engines behind one
  trait, fed the same recorded and generated output, measured on the criteria below, in at most three
  days of work. The result and the choice are written into this ADR's "As built" at step 1.
  `alacritty_terminal` is kept unless libghostty-vt is clearly better on what the author needs,
  enough to pay for Zig and memory-unsafe parsing.
  - **Correctness:** the Claude Code TUI (recorded from a scratch session, scrubbed), `vim`, `htop`,
    `less`, `lazygit`, wide and combining characters, and `vttest`'s basic screens.
  - **Speed:** parse throughput, the time to build a snapshot, and memory per 10,000 lines of
    scrollback.
  - **Cost:** clean build time and toolchain on macOS and Linux, binary size, and what `cargo-deny`
    and the fuzz target say after an hour each.
  - **Gaps:** what oso must add to each for this ADR: OSC 7, OSC 133, OSC 9, images, and a search
    over scrollback that can stop and go on.
- **A new crate, `oso-term`,** holds the terminal model: the engine, oso's escape-sequence policy,
  the key encoder, the terminal's thread, the snapshots it publishes and the search. It sits beside
  `oso-render` and `oso-syntax`:

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
    (OSC 7, 133, 9, 777 and oso's own), with each OSC string capped at 4 KiB.
- **The PTY is oso's own, in `oso-platform::process::pty` (Q2),** Unix only for now:
  - `openpty` from `libc`; the master is close-on-exec and non-blocking;
  - the child starts through the module's `spawn`, so the executable rules, the environment allowlist
    and the log line hold. A `Spawn` gains `terminal: Option<&Pty>`, and a `pre_exec` hook calls
    `setsid` and `TIOCSCTTY`: the module's one `unsafe` block, with its `// SAFETY:` comment;
  - `TIOCSWINSZ` to resize, `tcgetattr` on the master to see whether the slave echoes, `tcgetpgrp` for
    the foreground process group, `waitpid` for the child's exit status, and `SIGHUP` to that group
    when the terminal closes;
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
  UI's requests (resize, scroll, search, copy mode, close).
  - The UI thread never takes a lock the parser holds, never reads or writes the PTY, never parses and
    never searches.
  - Writes queue on the terminal's thread, at most 16 MiB, so a large paste never blocks the UI.
  - When oso falls behind it stops reading, and the child blocks on a full PTY. Nothing grows without
    bound.
- **A snapshot** holds the visible rows' cells (character, colors, flags, link id), the cursor and its
  shape, which rows changed, the title, whether echo is off, and the search's matches on those rows.
  It is built from the rows the engine marks damaged, and sent through the wake-up path the loader and
  workers use. **At most one snapshot is in flight:** a new one is built only after the UI took the
  last, so bulk output costs the UI one snapshot per frame however fast it comes.
- **Keys are encoded on the terminal's thread,** against the modes current there, so a key never
  misses a mode the program just set.
- **Drawing** puts each cell at its column, as the grid placed it; oso does not measure widths again.
  Only rows marked changed are shaped, through the shaped-row cache text views use. Colors come from
  the theme: 16 new faces, `terminal-color-0` to `terminal-color-15`, in each built-in theme
  ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)). 256-color and true color cells are drawn
  as given. Cells are drawn in grid order, with no reordering
  ([ADR 0066](0066-text-stays-in-logical-order.md)).

### A terminal as a buffer (Q5, Q11)

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
- **Scrollback (Q11):** 10,000 lines by default, set in `config.toml`'s `[terminal] scrollback`, at most
  100,000. A cell takes about 24 bytes, so 10,000 lines of 200 columns take up to 48 MB. Copy mode's
  buffer is dropped when the person leaves it.

### Project terminals (Q5)

As `ghostel-project` and `ghostel-project-list-buffers` are, fitted to
[ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)'s projects.

- **`C-x p m`, `project-terminal`,** switches to the last terminal started in the selected buffer's
  project, or starts one in the project's root. With a prefix argument it always starts a new one.
  Names are `*<project>-terminal*`, `*<project>-terminal*<2>` and so on, the project's name as ADR 0114
  shows it, controls boxed.
- **Which project:** the selected buffer's, as `C-x p f` finds it. From a terminal it is the project of
  the terminal's directory (OSC 7's, else the one it started in). Outside a project, ADR 0114's Q10
  holds: the buffer's directory stands in, and the home directory and `/` say `Not in a project`.
  Where detection has not answered yet, the command waits for it as `C-x p f` does.
- **A terminal keeps the root it started in,** in the tip crate's map of terminals. That is what
  `C-x p m` and `C-x p M` match on; a `cd` inside the terminal does not move it to another project.
  ADR 0114's project table is unchanged: a terminal has no file, so it points into none.
- **`C-x p M`, `project-list-terminals`,** opens the completion float with `C-x b`'s preview
  ([ADR 0088](0088-switching-and-killing-buffers.md)), listing only that project's terminals, most
  recently selected first, each annotated with its directory and whether a process runs. `RET`
  switches to the one chosen.
- Trust does not matter to either: a terminal opens in a restricted project as any terminal does.

### The floating terminal (Q5)

As popterm, with oso's floats ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md),
[ADR 0075](0075-a-float-as-a-panel-attached-to-the-top-edge.md)).

- **One terminal, `*float-terminal*`, shows in a float** that `` C-` ``
  (`terminal-float-toggle`) shows and hides. The first `` C-` `` starts it, in the directory a new
  terminal gets (below). Hiding it leaves its program running. It is an ordinary terminal buffer
  otherwise: `C-x b` reaches it and a tiled window may show it.
- **While the float shows, it is the selected view,** and every terminal key works in it. `` C-` ``
  hides it and selects the window below again; so does `C-x o`. A prompt opened while it shows draws
  over it: the float terminal's layers are below the prompts' (ADR 0072's 16 and 17).
- **Three positions:** attached to the top edge as ADR 0075's float is, at the center, or attached to
  the bottom edge. At the top and bottom it takes the frame's width and 40% of its rows; at the center,
  80% of its columns and 60% of its rows. These sizes are first values, picked from screenshots at the
  step as ADR 0075's were. The PTY takes the float's size.
- **`C-M-p`, `terminal-float-cycle-position`,** moves it top → center → bottom → top. The choice holds
  for the rest of the instance and is carried over `oso dev`'s restart. It starts from
  `config.toml`'s `[terminal] float-position` (`"top"`, `"center"` or `"bottom"`, by default
  `"bottom"`); oso never writes the file. In a terminal, `C-M-p` is oso's only while the float shows,
  and goes to the program otherwise.
- **`C-~`, `terminal-float-toggle-cd`,** shows the float and has its shell change to the directory of
  the buffer in the window selected before (its file's directory, else the directory that buffer's
  project rules give). It types `cd <dir>` and `RET` into the PTY, as if the person had: it is the
  person's command, so it is the person's typing.
  - **The directory is quoted for POSIX shells and fish alike:** the whole path in single quotes, each
    `'` in it written `'\''`. The path is absolute, so it cannot start with `-`.
  - **Refused, with the reason in the echo area,** when the path holds a control character (C0, `DEL`
    or C1), is not UTF-8, or is not a local directory any more.
  - **Refused when the shell is not in the foreground:** the foreground process group is not the one
    oso started (`vim`, `ssh` or `claude` runs there), so the keys would reach that program instead.
  - **Refused when the prompt holds typed text,** where OSC 133 marks say where the prompt's input
    starts and it is not empty. oso never clears what the person typed. Without OSC 133, oso cannot
    tell, and types.
  - If the float shows already, `C-~` hides it, as `` C-` `` does.

### Copy mode and the live search (Q14)

The author chose both.

- **`C-s` searches the live terminal in place,** as isearch does in a buffer
  ([ADR 0082](0082-isearch.md)): the same prompt and keys (`C-s` and `C-r` to the next and previous
  match, `RET` to stay, `C-g` to go back), literal by default, with isearch's regexp and migemo
  toggles ([ADR 0116](0116-migemo-and-jumps.md)). It searches the screen and the scrollback as text,
  soft-wrapped rows joined into their line. The view scrolls back to show the match; matches on the
  rows shown are drawn with isearch's faces.
  - **The search runs on the terminal's thread, never the UI thread.** Each key sends the query; the
    thread searches between reads, in slices of at most 2 ms, and sends the match with the next
    snapshot. A regexp is compiled there, with isearch's limits.
  - **Lines are counted from the start of the terminal's history,** so a line keeps its number when it
    moves into the scrollback. A match is that number and its columns.
  - **When output arrives during a search,** the thread keeps reading and parsing; the view stays
    where the match is, as any scrolled-back view does, and the modeline marks that output came. A
    match in the scrollback stays valid. A match on the live screen is checked against its row again
    with each snapshot: if the program overwrote it, its face goes, the prompt says `[changed]`, and
    the next `C-s` searches on from that line. A match whose line fell off the top of the scrollback
    is dropped the same way.
  - **On the alternate screen** (a full-screen program), the search covers that screen only, which
    has no scrollback. Copy mode reaches the rest.
  - `RET` leaves the view where it is; the next key sent to the program scrolls back to the bottom.
    `C-g` returns to where the search started.
  - A list of matching lines with a preview, as consult-line, comes when oso has one for buffers.
- **Copy mode:** `C-c C-j` freezes the screen and scrollback into a read-only internal buffer, built on
  the terminal's thread, shown at the same place. Every text command works there: isearch, migemo,
  avy, the region, `M-w`. `q` or `C-c C-k` returns to the live terminal, which kept running.

### Input (Q12, Q13, Q6)

- **Semi-char mode is the default (Q12),** with ghostel's keys and the author's own. Every key goes to
  the program, encoded, except:
  - `C-x` and `M-x`, which stay oso's;
  - `C-c` as a prefix: `C-c C-c` sends `C-c`, `C-c C-j` is copy mode, `C-c C-k` is char mode, any
    other key after `C-c` sends both;
  - `C-q`, which sends the next key as it is, oso's keys among them;
  - `C-g` while a prompt or a key sequence of oso's is open;
  - `C-s`, the live search above;
  - `C-k`, which first copies from the terminal's cursor to the end of its line into the kill ring,
    then sends `C-k`. Point in a live terminal is the terminal's cursor. The line is the logical one,
    soft-wrapped rows joined, with trailing blanks dropped; an empty copy adds nothing. Both happen on
    the terminal's thread, in that order, so no output comes between. While echo is off, `C-k` copies
    nothing;
  - `M-p` and `M-n`, which send `C-p` and `C-n`. `C-q M-p` sends `M-p`;
  - `` C-` `` and `C-~`, and `C-M-p` while the floating terminal shows.
- **Char mode** sends every key except `C-c C-k`, which leaves it, and `` C-` ``, so the floating
  terminal can always hide. Line mode is not built.
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
| OSC 7 working directory | Sets the terminal's directory for `C-x C-f`, `C-x p f` and `C-x p m`, if it names a local path. |
| OSC 133 prompt marks | Kept by row: moving between prompts, where output starts for the compile seam, the floating terminal's `cd`, and the host API. |
| OSC 9 and 777 notifications, OSC 9;4 progress | An echo-area message and a modeline mark on a terminal not shown; progress in the modeline; all kept for the host API (Q9). |
| Bell | The modeline flashes, as [ADR 0097](0097-visual-aids.md)'s flash does, and a terminal not shown is marked. |
| oso's shell command sequence | Runs a listed oso command, under the rules in [Shell commands that run oso commands](#shell-commands-that-run-oso-commands). |
| OSC 51 (vterm's and ghostel's "run this editor command") | Denied: it carries no token. |
| Kitty graphics | Shown, from the image step on (Q10). |
| Sixel, iTerm2 images (OSC 1337) | Denied in M4 (Q10). |

- **Links (Q7):** `http` and `https` targets open with the OS's opener, through `oso-platform::process`,
  after the echo area shows the whole target; `file:` targets on this host and `path:line` open in oso,
  as `C-x C-f` would; any other scheme asks first. This is the URL-opening policy for terminals.
- **Images (Q10):** the Kitty graphics protocol's direct transmission, raw RGB, RGBA and PNG. PNG is
  decoded by a pure-Rust decoder on a worker, never the terminal's or the UI thread; an image is at most
  16 MiB encoded and 4096 × 4096 pixels, and a terminal holds at most 64 MiB of images, dropping the
  oldest. Transmission from a file or shared memory is denied, so a remote program cannot make oso read
  a local file. `oso-render` draws them as textures placed on cells. Sixel and iTerm2 come later.
- The policy is one function in `oso-term` per effect, and each row of the table has a test that feeds
  the sequence and checks the reply, the kill ring, the modeline or what was dropped.

### Shell commands that run oso commands (Q5)

As `ghostel-eval-cmds` and `vterm-eval-cmds`: a shell function asks oso to run a command the person
listed, such as a future `magit-status` in the shell's directory. This reverses the draft, which
denied such sequences. The threat is that any program's output can carry the sequence: a file
`cat`ed, a page fetched, a remote host over `ssh`. So a sequence runs a command only when all of
these hold:

- **It carries the terminal's token.** Each terminal started with a shell gets 128 random bits from
  the OS, as 32 hex digits in `OSO_TERMINAL_TOKEN`. The shell integration script echoes it in each
  request. Output from a file, a page or a remote host cannot know it: it is new for each terminal,
  never shown, and not sent by `ssh` unless the person forwards it. A request with a wrong token is
  dropped silently.
- **The shell is in the foreground** when the thread parses it: the foreground process group is the
  one oso started. A program the shell runs inherits the token, an agent among them; while it runs it
  is in the foreground, so writing the sequence to `/dev/tty` gets it nothing. `ssh` is in the
  foreground for as long as it runs, so a remote host that learned the token gets nothing either.
- **The command is listed** in `config.toml`'s `[terminal] commands-from-shell`, by registered name. It
  is empty by default, which turns the feature off. Listing a `Person` command is the person's
  allowance for it; the docs say that a listed command that starts a process may then be started by
  the shell. Some commands can never be listed, and a config naming one is reported and that name
  dropped, as ADR 0108 reports a wrong name: those that trust or untrust a project, start or stop the
  agent channel, allow an agent to read a terminal, or send keys or text to a terminal. An unknown
  name is reported the same way.
- **The argument is one checked string:** at most 1 KiB of UTF-8, with no control, bidi or invisible
  characters. It goes to the command's first argument only when that reads a string, a file or a
  directory; a path is absolute, or relative to the terminal's directory. A command that reads
  nothing refuses an argument. One that reads more, or another kind, opens its prompts for the person
  as `M-x` would. There is no prefix argument.
- **The terminal is selected,** and no prompt or key sequence of oso's is open. Otherwise the request
  is dropped, and the echo area says so.
- **At most one per terminal each 100 ms;** the rest are dropped and counted in the echo area.

Each command run says `<terminal> ran <command>` in the echo area and writes a log line. The
sequence is an OSC number of oso's own, picked at the step from those no terminal or multiplexer in
use claims, and carries `run`, the token, the name and the argument.

What is left: a program the shell started that writes the sequence and exits before the thread
parses it, or one that runs in the background while the shell waits at its prompt, can pass both
checks. Each is a local program already running as the person, which gains only a listed command.

### The shell integration scripts

- **oso ships scripts for bash, zsh and fish** that emit OSC 7 at each prompt, OSC 133's marks around
  the prompt and each command (with its exit status), and define `oso_run <command> [argument]`, which
  writes the request with the token. `oso shell-integration <shell>` prints one.
- **The person sources it** from their startup file, behind a test of `OSO_TERMINAL_TOKEN`. oso never
  writes a startup file and injects nothing into the shell's start.
- Without the script, the terminal works, with no OSC 7, no prompt marks and no shell commands.

### Compile and comint on a terminal (Q5)

As `ghostel-compile` and `ghostel-comint`. This ADR provides the terminal side; M4's compile buffer
builds on it, and decides its own buffer, keys and error list in its own ADR.

The seam is `oso-term`'s, used by the tip crate:

- **Start a program in a new terminal,** with a shell (the configured shell, `-c` and a command line
  the caller built) or without one (an `Executable` and its arguments, as `process::spawn` takes
  them). Without a shell, no startup file runs, the terminal gets no token, and oso puts the output
  mark at the start itself. The caller gives the directory, the size, the scrollback, the buffer's
  name and extra environment names.
- **Read the output** as rows of the screen and the scrollback by range, or as a stream of lines:
  each logical line once a line feed ends it, as its final text, with its line number. The stream is
  a bounded queue of 64 KiB; a reader that falls behind gets a gap with the numbers it missed, and reads
  those rows instead.
- **Learn the exit:** an event with the child's status (a code or a signal), from `waitpid` on the
  terminal's thread. A child carried over `oso dev`'s restart is no longer oso's to reap, so its exit
  is known only from the PTY's end, with no status.
- **OSC 133 marks:** where each command's output starts and ends, and its exit status, so a compile
  buffer started with a shell finds its output.
- A comint-like buffer is a terminal started without a shell, running the interpreter; nothing more
  is needed of the terminal. Line mode is not built.

Commands built on the seam start a process, so they are `Person`.

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
  bypassing them. oso features the person runs, such as the floating terminal's `cd` or sending review
  comments to a session as a bracketed paste (ADR 0122), are the person's typing.
- **The channel reads no terminal text unless the person allows it, per terminal (Q16).** By default
  `text` is refused for a terminal and `state` lists it by id, name and kind, with whether a process
  runs, but no rows and no title. The `Person` command `terminal-allow-agent-read` allows it for the
  selected terminal until it closes, and the modeline shows `agent reads`; `terminal-deny-agent-read`
  takes it back. Copy mode's buffer follows its terminal, and so do the live search and copy mode for
  the channel: refused where reading is not allowed, as a search is a probe of the text. An echo-area
  message that carries a terminal's text (a link target, a notification) is withheld from `state` the
  same way.
- **Echo off is a secret (Q17).** While the PTY does not echo (a password prompt, `sudo`, `ssh`), keys
  typed there are not recorded into a keyboard macro, `C-k` copies nothing, the modeline says
  `secret`, the channel waits as for a secret buffer shown, its rows are withheld even where reading
  was allowed, and on macOS secure input is on while that terminal is shown and selected
  ([ADR 0073](0073-secure-input-in-secret-buffers.md)).
- **Showing a terminal does not stop the channel.** A terminal is not a buffer that may hold secrets in
  ADR 0085's sense, or the channel could never act while an agent's own terminal is on screen.
- **Reach:** `Person` for `terminal`, `project-terminal`, `terminal-float-toggle` (which may start a
  shell), `terminal-float-toggle-cd`, every key and paste sent to a terminal, the mode switches,
  opening a link, allowing reading, and closing or killing a terminal (which asks while a process
  runs); `Look` for switching to a terminal, `project-list-terminals`,
  `terminal-float-cycle-position`, and moving in it, with the search and copy mode as above.
- **Terminal text never reaches the disk:** no auto-save, no emergency copy, no session file. The kill
  ring holds what the person copies.
- The author named the other powers no agent gets, which are not this ADR's: answering its own
  prompts, trusting a project, pushes that cannot be undone, and editing oso's configuration
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md) has the first, second and fourth).

### Restricted projects, the shell and the environment (Q18)

- **A terminal opens in a restricted project,** only by the person's command. Opening a file or a
  project never opens one, and no start of oso brings one back.
- **Its directory** is the project's root, else the buffer's directory, else the home directory.
- **What runs in it is the person's:** their shell, its startup files, its prompt. oso's trust limits
  what oso starts, not what anyone runs in a terminal; the docs say so where they explain trust.
- **The shell (Q18)** is `config.toml`'s `[terminal] shell`, an absolute path to an executable; else
  `$SHELL` if it is absolute; else the account's shell from the user database; else `/bin/sh`. A
  relative or missing `shell` is reported and the next one used.
- **`[terminal] login`,** `true` by default, starts it as a login shell (`-l`), as macOS terminals do.
  With `false` it starts as an interactive shell that is not a login shell.
- **The environment (Q18)** is the module's allowlist plus `SHELL`, `COLORTERM=truecolor`,
  `TERM=xterm-256color`, `TERM_PROGRAM=oso`, `TERM_PROGRAM_VERSION`, `SSH_AUTH_SOCK`,
  `OSO_TERMINAL_TOKEN` (shells only), the names in `config.toml`'s `[terminal] env`, and what the caller
  of the host API adds. The shell's startup files build the rest.
- **Without a login shell,** nothing else is added. The allowlist's `PATH` is oso's own, filtered: from
  a shell (`oso dev`) it is that shell's, but from the Dock it is the GUI's short one
  (`/usr/bin:/bin:/usr/sbin:/sbin`) unless the shell's startup files set it. bash, interactive and not
  a login shell, reads `~/.bashrc` and not `~/.bash_profile` or `~/.profile`; zsh reads `~/.zshenv` and
  `~/.zshrc` but not `~/.zprofile`; fish reads its `config.fish` either way. The docs say so beside the
  setting.

### Restarts under `oso dev` (Q19)

- **A terminal survives a restart, on Unix:** the old instance stops reading and hands each PTY's
  master to the new one with `SCM_RIGHTS`, with the child's process id, the modes, the token, the root
  it started in, and the screen and scrollback re-encoded as escape sequences. The new instance feeds
  that to a new engine, then reads the PTY where the old one stopped; bytes written meanwhile wait in
  the kernel. A resize afterwards makes a full-screen program redraw. The floating terminal's position
  is carried too.
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
| Live search of 10,000 lines × 200 columns | first match ≤ 50 ms, in slices of ≤ 2 ms on the terminal's thread; no UI work but drawing the match |
| Showing or hiding the floating terminal | one frame, its shell already running |

New `just perf` flows measure the first three and the search, with programs that write 100 MB and that
echo keys.

## Steps

One pull request each. Each step updates the docs it changes. Steps 7 to 10 need only step 4 (and 6
for OSC 133), and may go in any order.

1. **The comparison** (at most three days): both engines behind `Engine`, headless, the criteria
   above, and the result and the choice written into this ADR. The losing engine's code is not merged.
2. **The PTY:** `oso-platform::process` in release builds, `process::pty` on macOS and Linux, the
   marker and `just release-safety`, and unit tests with `/bin/sh`.
3. **`oso-term`:** the crate and its `layers.txt` rows, the chosen engine, the terminal's thread,
   snapshots, the policy and every table row's test, the key encoder, the host API, and a fuzz target.
4. **The terminal view:** `terminal`, drawing, the 16 faces, semi-char and char mode, the input method,
   resize, the mouse, paste, echo off, the channel's rules, `[terminal]`'s `shell`, `login`, `env` and
   `scrollback`, end-to-end flows and `just perf` flows.
5. **The live search and the author's keys:** `C-s` on the terminal's thread, `C-k`'s copy, `M-p` and
   `M-n`, and the search's `just perf` flow.
6. **Copy mode,** links, OSC 7 and 133, OSC 52 writes, notifications, progress, the bell, and the shell
   integration scripts for OSC 7 and 133.
7. **The compile and comint seam:** starting without a shell, the line stream, the exit status, and
   tests. Its consumer is M4's compile buffer.
8. **Project terminals:** `C-x p m` and `C-x p M`.
9. **The floating terminal:** the float that shows a terminal, `` C-` ``, `C-~` and its `cd`,
   `C-M-p`, and `[terminal] float-position`. The float is the first to show a buffer's view; its
   public items are named in the pull request for the author.
10. **Shell commands that run oso commands:** the token, the sequence, `[terminal]
    commands-from-shell` and its checks, and `oso_run` in the scripts.
11. **The restart handoff** under `oso dev`.
12. **Kitty graphics.**

Steps 1, 3 and 12 add dependencies, and step 2 adds `log` to release builds of `oso-platform`, each
through the `add-dependency` skill first. The token's random bits come from the OS through `libc`
in `oso-platform`, with no new crate.

## Questions

Put to the author on 2026-11-12, the recommended option first. The author chose (a) on each but Q14,
where they chose both, and asked for more on Q5, Q12 and Q18; each question ends with the answer.

### Engine and crates

1. **The engine.**
   (a) A time-boxed comparison first (step 1, at most three days), keeping `alacritty_terminal` unless
   libghostty-vt is clearly better. The 2026-11-07 plan, with a cap on its cost.
   (b) `alacritty_terminal` now, no comparison. Saves the step; OSC 7, 133, 9 and images are then oso's
   to add, which the comparison would have priced.
   (c) libghostty-vt now. Parity with ghostel and images built in; Zig on three OSes, hand-kept
   bindings, and memory-unsafe parsing of untrusted output in oso's process.
   **Answer:** (a), at most three days, both engines behind `oso_term::Engine`.
2. **The PTY.**
   (a) oso's own `process::pty` over `libc`, one `unsafe` `pre_exec`. No crate; ADR 0010's spawn rules
   hold by construction.
   (b) `portable-pty`. ConPTY comes with it, but it spawns with its own builder, outside the module's
   allowlist and log.
   (c) The engine's own PTY code, where it has any. Already there, but it spawns outside the module.
   **Answer:** (a).
3. **Windows.**
   (a) Deferred until Windows draws text; the interface is shaped for ConPTY.
   (b) ConPTY now. Ready earlier, but nothing can be checked in the app on Windows yet.
   **Answer:** (a).
4. **Threads.**
   (a) One thread per terminal owning the PTY and the engine, at most one snapshot in flight.
   (b) The engine behind a mutex the UI locks to draw, as Alacritty and Zed do. Simpler; bulk output can
   hold the lock for a parse, which is the stutter the author wants gone.
   (c) One thread for all terminals. Fewer threads; one busy terminal slows the rest.
   **Answer:** (a).

### What replaces a terminal app

5. **Many terminals.**
   (a) Each is a buffer in oso's windows and splits; no tab bar until oso has tabs.
   (b) A tab bar and splits inside the terminal, as tmux or herdr. Familiar, but a second window system
   beside oso's.
   **Answer:** (a), "the same as ghostel today": also project terminals (`C-x p m`, `C-x p M`), a
   floating terminal as popterm (`` C-` ``, `C-~` with a `cd`, `C-M-p` for its position), shell
   commands that run listed oso commands, and the terminal as the backend of compile and comint.
6. **The mouse.**
   (a) Full reports to programs that ask (clicks, drags, wheel, motion), Shift to override, drag-select
   otherwise. `lazygit` and `htop` then work by mouse.
   (b) The wheel only in M4. Less work; mouse-driven TUIs need the keyboard.
   **Answer:** (a).
7. **Links.**
   (a) Open `http`/`https` with the OS's opener after showing the target, `file:` and `path:line` in oso,
   other schemes ask. Settles the URL policy for terminals.
   (b) Copy the target only, until a URL policy of its own. Nothing opens by accident, but links are
   half a feature.
   **Answer:** (a).
8. **OSC 52 writes.**
   (a) Kill ring always; OS clipboard only within 2 s of the person's key in that terminal. Copy from
   `ssh`'d `vim` or `tmux` works; text a program prints on its own reaches only the kill ring.
   (b) OS clipboard always, as Ghostty and WezTerm. Simplest; anything `cat`ed can replace the clipboard.
   (c) Kill ring only. Safest; copying from a remote host to other apps does not work.
   **Answer:** (a).
9. **Notifications, progress and the bell.**
   (a) In oso: echo area, modeline marks, progress in the modeline, all kept for ADR 0121; OS
   notifications come with ADR 0121, which decides when they are worth it.
   (b) OS notifications now. Needs a macOS notification binding, a new dependency, before sessions
   exist to use it.
   **Answer:** (a).
10. **Images.**
    (a) Kitty graphics in M4 (a step), direct transmission only, PNG through a pure-Rust decoder on a
    worker with size caps; Sixel and iTerm2 later. Parity where ghostel is used most; T10 allows
    pure-Rust decoders.
    (b) After M4, as the roadmap has it. No decoder now; the terminal app replacement waits for images.
    (c) All three formats in M4. Full parity; three parsers of untrusted data.
    **Answer:** (a), with caps of 16 MiB an image, 4096 × 4096 pixels and 64 MiB a terminal.
11. **Scrollback.**
    (a) 10,000 lines by default, at most 100,000, set in `config.toml`; up to about 48 MB a terminal at
    200 columns.
    (b) 2,000 lines. Less memory; a long session scrolls past it.
    **Answer:** (a).

### Input

12. **Keys.**
    (a) Semi-char by default: `C-x`, `M-x` and the `C-c` prefix stay oso's (`C-c C-c` sends `C-c`), `C-q`
    sends any key, `C-c C-k` char mode.
    (b) `C-c` straight to the program, only `C-x` kept. One key for `C-c`; oso's terminal commands need
    another prefix.
    (c) Char mode by default, with one key to leave. Feels like a plain terminal; oso's keys need a mode
    switch.
    **Answer:** (a), with ghostel's defaults and the author's own as defaults too: `C-s` searches the
    terminal's text, `C-k` copies to the end of the line before it is sent, and `M-p` and `M-n` send
    `C-p` and `C-n`.
13. **A multi-line paste without bracketed paste.**
    (a) Ask first, as Ghostty and VS Code. (b) Paste as typed; a copied line break runs a command.
    **Answer:** (a).
14. **Searchable text.**
    (a) Copy mode: a frozen read-only buffer where every text command works.
    (b) isearch and line search on the live grid. No copy; each command needs a grid version, and the
    text moves while one searches.
    **Answer:** both. `C-s` searches the live screen and scrollback in place, on the terminal's thread,
    as the author uses consult-line in ghostel today; copy mode stays for every other text command.

### Agents

15. **Typing.**
    (a) Only a person: the channel never writes to a PTY.
    (b) The channel may type into a terminal the person allows. An agent could then answer another
    agent's permission prompts and run any command.
    **Answer:** (a).
16. **Reading.**
    (a) No terminal text unless the person allows it per terminal, with a modeline mark.
    (b) Never. Simplest; an agent cannot read a build error in the person's shell even when asked to.
    (c) Always, as any buffer. Convenient; whatever was printed, tokens included, is the agent's.
    **Answer:** (a).
17. **Echo off.**
    (a) Treated as a secret buffer shown. (b) Not detected; a password typed at `sudo` can enter a macro.
    **Answer:** (a).
18. **The environment.**
    (a) The allowlist, `SSH_AUTH_SOCK`, the terminal's own names, `[terminal] env` and the host API's,
    with a login shell building the rest.
    (b) Everything oso was started with, as Emacs. Familiar; from `oso dev` that includes `CARGO_*` and
    whatever its shell held.
    **Answer:** (a), with the shell and whether it is a login shell set in `config.toml`, and the docs
    saying what a shell that is not a login shell starts with.

### Restarts

19. **A terminal under `oso dev`'s restart.**
    (a) Handed over with `SCM_RIGHTS`, screen and scrollback carried in memory (a step); until then a
    running terminal holds the restart.
    (b) Always holds the restart; agents run in a second oso that is not rebuilt. No fd passing; the
    agent and the oso it edits are two windows.
    (c) Restarts end terminals. Simple; every edit to oso's source ends every shell and agent in it.
    **Answer:** (a).

### Dependencies (for the `add-dependency` skill, not decided here)

- The engine: `alacritty_terminal` 0.26.0 with `default-features = false` (it brings `vte` 0.15,
  `base64`, `bitflags`, `home`, `parking_lot`, `polling`, `regex-automata`, `unicode-width` 0.2, and on
  Unix `rustix`, `rustix-openpty` and `signal-hook`), and `vte` 0.15 directly for the second parser;
  or libghostty-vt with a Zig toolchain and bindings oso keeps. `regex-automata` is the `regex` crate's
  own engine, used for `alacritty_terminal`'s search, which oso does not call; AGENTS.md's regex rule
  needs a note for it.
- A pure-Rust PNG decoder for the image step.
- `log` in release builds of `oso-platform`, already in the lock.
- No PTY crate under option 2 (a).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0015](0015-terminal.md), "The terminal engine is `alacritty_terminal`": chosen by step 1's
  comparison, which chose libghostty-vt 1.0.1 (see As built); the FFI, memory-safety and toolchain
  costs ADR 0015 rejected it for are taken, under step 1's conditions. "PTYs go through the process module: openpty on Unix, ConPTY on Windows": macOS and Linux
  first. "OSC 8 links show the real target before opening": and open under the rules above. "It exposes
  the content as searchable text, so isearch, line search, copy and selection work as in text
  buffers": the live search covers isearch's keys, and copy mode's buffer the rest. "The terminal may
  also serve as the compile backend": through the seam above.
- [ADR 0047](0047-process-spawning-and-restart-handoff.md), "behind the `dev` feature until a release
  feature needs it" and the marker `oso-dev-process-spawn`: the terminal is that feature, the module
  ships, and the marker is `oso-process-spawn`, no longer checked in release binaries. The state
  handoff carries terminals as above.

## Consequences

- A release build can spawn for the first time, still only through one module with its allowlist and
  log; `just release-safety` loses its spawn check.
- A new crate, `oso-term`, and a new row in `layers.txt`.
- The UI thread's cost of a terminal is one snapshot per frame and the rows that changed, however much a
  program prints, and however many terminals print. Searching costs it nothing but drawing the match.
- Agents cannot type into a terminal, and read or search one only where the person allowed it. What
  they run through their own tools is theirs; oso's trust does not reach inside a terminal.
- A shell can run oso commands the person listed, and only those. A program that runs as the person in
  that shell can do so too in the narrow cases above; it could do more on its own.
- The floating terminal is the first float that shows a buffer's view, below the prompts' layers.
- ADR 0121 gets screen rows, the title, progress, prompts, the directory, bells and the foreground
  program from the host API, and sets a terminal's extra environment through it. M4's compile buffer
  gets the seam.
- Until step 11, a running terminal holds `oso dev`'s restarts.
- Follow-up: ADR 0121 (sessions, `claude`, the MCP config, OS notifications), ADR 0122 (review), the
  compile buffer's ADR, ConPTY, Sixel and iTerm2 images, moving between OSC 133 prompts, and a
  line search with a list, as consult-line, for buffers and terminals.
- `open-questions.md`'s M4 items on the engine, the PTY per OS and the escape-sequence list, and its
  URL-opening item as far as terminals go, are answered here.

## Alternatives considered

- **libghostty-vt without a comparison, and `portable-pty`:** see Q1 and Q2.
- **Mirroring the grid into a rope buffer all the time, as vterm and eat do:** every command works, but
  each line of output costs an edit, and ADR 0015 rejected it for heavy output.
- **A separate pty host process, as VS Code and herdr have:** terminals would survive any restart,
  release ones too, but it is a second process and protocol to keep in step, and `SCM_RIGHTS` covers
  the restart that matters now.
- **Denying shell commands that run oso commands, as the draft did:** safest, but the author uses them
  in ghostel.
- **Only refusing them when the shell is not in the foreground:** a `cat` of a crafted file exits
  before its bytes are parsed, and the shell is in the foreground again by then.
- **Only the token:** every program the shell runs inherits it, an agent among them, which could write
  the sequence to `/dev/tty` while it runs.
- **vterm's OSC 51 as it is, for the scripts people have:** it carries no token.
- **Searching the live terminal on the UI thread over a copy of the grid:** the copy grows with the
  scrollback, up to 100,000 lines, on the thread that must draw each frame.
- **Changing the floating terminal's directory without typing,** by starting a new shell there: its
  history and state would be lost, and a process's directory cannot be changed from outside.
- **Kitty graphics from a file or shared memory:** a remote program could name a local file for oso to
  read and show.

## As built

### Step 1: the engine comparison

**The engine is libghostty-vt 1.0.1,** in oso's process, built `ReleaseFast` from the signed
release. The author chose it on 2026-11-13, through the coordinator, after the comparison below,
with the five conditions at the end of this step.

- **How it ran:** in one day of the three, in a workspace of its own on the branch
  `m4-terminal-engines` (`spikes/term-engines`), which is pushed for the record and never merged.
  - Both engines sat behind one `Engine` trait: feed bytes, resize, publish a snapshot of the rows
    that changed, read rows as text, modes, effects.
  - `alacritty_terminal` 0.26.0 ran with the second `vte` 0.15 parser this ADR planned.
  - libghostty-vt started on a pre-release commit (`83fb956`, 1.4.0-main) and moved to the 1.0.1
    release (2026-11-11) once it was out: Zig 0.17.1, offline from the release's source and
    dependency tarballs, both checked against the published signature and sha256.
  - It is called through hand-written FFI: 23 functions and 49 `unsafe` blocks, each with its
    `// SAFETY:` comment, and a test that checks every struct and constant against the library's own
    type manifest. The third-party `ghostty-vt-sys` crate was not usable: its build script downloads
    the source with `ureq`, which `deny.toml` bans.
  - `vim`, `less`, `htop`, `lazygit`, `vttest` and Claude Code (its start screen, `/help` and typed
    wide text, no prompt sent) were recorded in a PTY at 120 × 40 and replayed in 4 KiB reads. The
    recordings were not committed.
  - The benchmarks ran on an Apple Silicon Mac that other work shared, twice; the ratios held.

| | `alacritty_terminal` 0.26.0 | libghostty-vt 1.0.1 |
|---|---|---|
| Recorded programs | The same final screens | The same, and DECCOLM (80 and 132 columns), a flag emoji as one 2-cell grapheme, XTVERSION, in-band resize (mode 2048) |
| Parse, MB/s: plain, SGR-heavy, CJK and emoji, a TUI's redraws | 225, 148, 147, 143, both parsers | 1,789, 326, 453, 163 |
| A snapshot of 300 × 100, p50: every row, one row | 58 µs, 1.2 µs | 135 µs, 3.4 µs |
| Memory, 10,000 lines of 120 and of 200 columns | 36 and 55 MB | 13 and 20 MB |
| Memory, 100,000 lines of 200 columns | 533 MB | 175 MB |
| Binary size added, stripped | 116 KB | 1.27 MB |
| Clean build | 4 s, crates.io only | 35 to 75 s a target; Zig, with bundled C++ (simdutf, highway) and C (Wuffs) |
| Windows (MSVC) | Builds | Needs the MSVC SDK: a Windows runner, or `xwin` and a local patch |
| `cargo deny`, `cargo audit`, licences | Nothing new; Apache-2.0 and MIT; 10 new crates | Not a crate, so neither sees it; MIT, with bundled code under MIT, Apache-2.0 and BSD |
| A control string that never ends | An OSC string is buffered without limit: `oso-term` would need a 4 KiB pre-filter | Bounded |
| Fuzzing with generated escape sequences, two hours or more each | Stopped only in a 1-column terminal; with 2 columns at least, none in 7.7 million inputs | The pre-release commit stopped on crafted output; 1.0.1 did not, in 8.4 million inputs, `ReleaseSafe` included |
| Memory safety | Safe Rust; its `unsafe` is in PTY code oso never calls | Zig built `ReleaseFast`, and C++, in oso's process |
| OSC 7, 133, 9, 777 and 9;4; OSCs it does not know | Through the second parser | Callbacks, and unknown OSCs up to a size oso sets |
| Kitty graphics | oso's to build; `vte` drops APC strings | Built in |

- **Why:** libghostty-vt takes about a third of the memory for the same scrollback and parses
  faster, and it brings what steps 6, 10 and 12 would otherwise build on top of
  `alacritty_terminal`. A 100 MB `cat` is bound by the PTY either way, about 0.75 s on this Mac, and
  a TUI at 60 frames a second uses under a tenth of either engine. The author took the build and
  toolchain cost as solvable. libghostty-vt's 1.0 release gives a stable C ABI, signed source and
  dependency tarballs and an independent release cycle. herdr and ghostel embed it in-process;
  JetBrains' IDEs and KDE's Kwi are built on it, and Neovim's `:terminal` is moving to it.
- **Rejected:** running the engine in a helper process per terminal. Moving a full 300 × 100
  snapshot through a pipe costs about 0.15 ms on the terminal's thread, but the protocol, restarts
  and a sandbox per system were judged two to three weeks more. It stays possible behind `Engine`.
- **The conditions** the choice came with:
  1. `oso-term`'s fuzz target runs against the pinned release, and every bump of libghostty-vt is
     fuzzed again before it lands.
  2. `oso_term::Engine` stays, so that the engine can move to a helper process, or back to
     `alacritty_terminal`, if the library ever aborts on crafted output.
  3. Zig is pinned to 0.17; the build is offline from the release's tarballs, checked against
     their signature; `just clippy-windows` builds with a stub in place of the library; Windows
     (MSVC) builds on a Windows runner when Windows terminals come.
  4. The Kitty graphics buffer is capped at 16 MiB (the library's default is larger), and the
     scrollback byte limit is set to `SIZE_MAX`, so that `[terminal] scrollback`'s lines decide: in
     1.0, a NULL byte limit restores a small default instead of removing it.
  5. A remaining cost the author accepted is recorded privately.

What this changes in the steps after it, which were written for `alacritty_terminal`:

- **The engine and the crates, and step 3:** the second `vte` parser and its 4 KiB cap are not
  built. The library reports OSC 7, 133, 9, 777 and 9;4 through callbacks, and an OSC it does not
  know, oso's shell command sequence among them, through its unknown-sequence callback, capped at
  4 KiB. No pre-filter and no 2-column floor are needed.
  - `oso-term` holds the FFI (hand-written, checked against the library's manifest), a `build.rs`
    that runs Zig, and the safe `Engine` over them; all its `unsafe` is there.
  - Each terminal's thread owns one terminal handle and one render state: the library makes no
    threads and its handles are not thread-safe, which fits one thread per terminal. A snapshot is
    built from the render state's dirty rows.
  - The policy acts on the library's callbacks. Two are never installed: the clipboard read, since
    with it even a denied read answers an empty OSC 52, and the window resize. The title report is
    turned off. The theme's colors are set in the library, so that it answers OSC 4, 10, 11 and 12
    from them, and device attributes are answered through their callback; step 3's tests check
    each against the table.
  - The key encoder: oso's own, as Input says, or the library's (`key.h`), which knows the modes
    already. Step 3 decides.
  - The fuzz target feeds `oso-term` through the FFI. libFuzzer's coverage does not reach inside
    Zig, so a generator of escape sequences runs as well; the library is fuzzed upstream with AFL++.
- **Cost and scrollback (Q11):** a cell takes about 9 bytes, not 24, so 10,000 lines of 200 columns
  take about 20 MB. The library frees scrollback a page at a time, so a terminal keeps up to a few
  hundred lines fewer than `[terminal] scrollback` says (9,410 of 10,000 at 80 columns).
- **Step 5:** the live search stays oso's, with isearch's regexp and migemo, on the terminal's
  thread; it reads rows through the FFI. Whether the library's own search (`search.h`) can serve it
  is step 5's to check.
- **Step 6:** OSC 7, 133, the notifications and progress come from callbacks. OSC 133's parameters
  arrive parsed, and the ones it does not know, `aid=` among them, are not passed on; moving between
  prompts needs none of them.
- **Step 11:** the screen and the scrollback re-encoded as escape sequences come from the library's
  formatter.
- **Step 12:** Kitty graphics are the library's: the image store (10 MB by default, set to this
  ADR's 64 MiB a terminal), the 16 MiB cap on what it buffers, and transmission from a file, a
  temporary file or shared memory left off. It decodes PNG with Wuffs, generated C, in oso's process
  while it parses, so on the terminal's thread, not with a pure-Rust decoder on a worker, as Q10's answer and
  [`security.md`](../security.md)'s T10 have it. Step 12 decides between that and building the
  library without Kitty graphics and decoding in oso. `oso-render` draws the images the render
  state lists.

What step 3 needs besides its own crate, each through the `add-dependency` skill or the author first:

- **No new crate:** the FFI is hand-written, with no `bindgen`, `cc` or sys crate. `cargo deny` and
  `cargo audit` do not see libghostty-vt or what it bundles (simdutf, highway, Wuffs, uucode), so
  each version and licence is listed by hand, and read again at each bump, as the grammars' C is
  ([ADR 0111](0111-tree-sitter-highlighting.md)). The MIT licence's notice ships with oso.
- **The source:** the release's two tarballs, about 10 MB, are either committed or fetched and
  checked by a `just` recipe; `build.rs` never downloads. Step 3 asks.
- **The toolchain:** Zig 0.17 on every machine and CI runner that builds `oso-term`, which needs a
  `.github/` change, and a Windows runner later.
- **The guard files:** `build.rs` runs Zig with `std::process::Command` and prints to cargo, which
  `clippy.toml`'s rules forbid elsewhere. The architecture test's list of where they may be
  silenced, `just clippy-windows`'s stub tools and `just release-safety` change with it.
- **`deny.toml`:** nothing to add for the library. As tree-sitter is, `oso-term` alone may use it,
  which the architecture test checks for an FFI module rather than `deny.toml`'s wrappers.
- **`security.md`:** T10 names libghostty-vt as the second exception after the grammars: a
  memory-unsafe parser of untrusted bytes in oso's process, pinned to a signed release and fuzzed at
  each bump. A crash in it ends oso with no emergency save, as one in a grammar does.

### Step 2: the PTY

The author approved the plan on 2026-11-13, through the coordinator, with the first option on each
question, and after the reviews, the restored guards below.

- **`oso-platform::process` is in every build.** `log` is no longer optional in `oso-platform`, whose
  `dev` feature now gates only the window layer's dev code and the memory clipboard, and the spawn
  log line starts with `oso-process-spawn`. Tests that start a process no longer need the `dev`
  feature.
- **The guards that moved with it.** `just release-safety` no longer looks for a spawn marker; it
  looks for `oso-dev-supervisor`, which only `oso dev`'s supervisor holds, so that the code that
  runs cargo cannot reach a release build alone. Clippy's `disallowed-methods` adds
  `CommandExt::pre_exec` and `before_exec`, and libc's `fork`, `vfork`, `forkpty`, `clone`, `exec*`,
  `posix_spawn*`, `system` and `popen`,
  and the architecture test allows silencing them in `process.rs` and in `process/pty/unix.rs`, which
  holds the terminal's child-side hook, only.
- **`Spawn` gains two fields:** `set`, variables set by value over the allowlist; and
  `terminal: Option<&pty::Slave>`. ADR 0122's step 2 then adds only a piped standard input. Names
  in `set` and in `inherit` are refused when empty, holding `=` or NUL, `PATH` (the module's own),
  `LD_*` or `DYLD_*`; names and values come from oso or the person's own configuration, never from a
  project's files, a terminal's output or the agent channel. `set`'s values are neither logged nor
  shown by `Spawn`'s `Debug`. A child in a terminal must leave `stdout` and `stderr` as
  `Sink::Inherit` and `own_process_group` false.
- **`process::pty`:** `open(Size) -> (Master, Slave)`; `Master` reads and writes without blocking,
  `resize`s (`TIOCSWINSZ`, pixels too), says whether the terminal `echoes`, gives its
  `foreground_group` and can `hang_up`; `wake_pair()` gives a `Waker` and a `Wakeup`, and
  `Master::wait` polls the master and the wake-up pipe with a timeout, so step 3's thread needs no OS
  calls of its own. Wakes not yet seen are one byte in the pipe, and a `Wakeup` keeps the pipe open,
  so a dropped `Waker` cannot make the wait spin. A `Slave` is dropped once the child has started,
  so that the master sees the terminal's end; a read then returns 0 on both systems (Linux's `EIO`
  is read as the end). The child's exit status is the standard library's `Child::wait`.
- **For step 3:** one read returns a few KiB, so the thread reads until `WouldBlock` before waiting;
  it asks `wait` for `writable` only while input is queued; and it opens, spawns and drops the slave
  itself, as a spawn with a terminal forks the whole process.
- **`hang_up`** on macOS asks the kernel to signal the foreground group (`TIOCSIG` on the master),
  so no group number is read and then signalled. Linux's `TIOCSIG` sends only `SIGINT`, `SIGQUIT`
  and `SIGTSTP`, so there, and wherever `TIOCSIG` fails, it signals the number `tcgetpgrp` gives,
  never 0, 1 or oso's own group.
- **`posix_openpt` with `O_CLOEXEC`, not `openpty`.** `openpty` opens both ends without
  close-on-exec on macOS and Linux, so a spawn on another thread before they were marked would
  give a child that does not own the terminal its master: it could read what is typed there, and
  the master would never see the end. `posix_openpt` sets it atomically (checked on macOS; glibc
  and musl pass the flag to `open`), the slave's name comes from `TIOCPTYGNAME` on macOS and
  `ptsname_r` on Linux, and the standard library opens the slave with `O_CLOEXEC` and `O_NOCTTY`.
- **The child-side hook** (`pre_exec`, which makes the standard library fork) calls `setsid` and
  `TIOCSCTTY`, sets the job-control and termination signals back to their default actions (an oso
  started with `nohup` would otherwise give its shells an ignored `SIGHUP`), empties the signal
  mask the child has from the spawning thread, and marks every descriptor from 3 up close-on-exec
  (`close_range` on Linux, else one by one up to the soft descriptor limit, which on macOS is
  capped at `kern.maxfilesperproc`), so the long-lived shell keeps no descriptor another thread
  had not marked yet. It allocates nothing and calls only async-signal-safe functions.
- **The unsafe blocks.** "One `unsafe` `pre_exec`" is the one block that runs in the child. Every
  other libc call in the parent has a block of its own, each with its `// SAFETY:` comment:
  `posix_openpt` and taking its descriptor, `grantpt` with `unlockpt`, the slave's name, `fcntl` for
  `O_NONBLOCK`, `TIOCSWINSZ`, `tcgetattr` and reading its result, `tcgetpgrp`, `TIOCSIG`, `getpgrp`,
  `kill`, `poll`, `getrlimit`, `sysctlbyname` on macOS, and `getpwuid_r`.
- **`process::account_shell()`**, in a module of its own, reads the account's shell from the user
  database for step 4's order of shells; it may wait on a directory service, so it never runs on the
  UI thread. The order and `-l` are step 4's.
- **Windows:** `pty` is compiled everywhere, so `Spawn` is the same on every system; `open` and
  `wake_pair` return `Unsupported` there, and no value of its types exists. ConPTY will fill `open`,
  reading, writing, `resize` and `wait`, but has no echo flag, process groups or hang-up: a caller
  treats an error from `echoes` as echo off, and one from `foreground_group` as a shell not in the
  foreground.
- **Tests** run `/bin/sh` in a terminal on macOS and Linux: its size and streams, the controlling
  terminal, the session, a resize reaching a `WINCH` trap, input, echo off, the end, a hang-up
  (also from a thread that blocks `SIGHUP`),
  that no other child inherits either end, that a descriptor left open on `exec` reaches no
  terminal child, the names refused by value and by name, the group a hang-up never signals, and the
  wake-up.
