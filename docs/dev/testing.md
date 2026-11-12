# Testing

## Principles

- Keep logic out of GPU and window code so that most behavior can be tested
  with plain `cargo test`.
- Prefer property-based tests (`proptest`) for positions, offsets and
  coordinate mappings, where boundary cases are easy to miss.
- `just ci` (check, lint, fmt-check, test) must pass before every commit.

## Test pyramid

Decided in [ADR 0037](adr/0037-test-pyramid.md). Lower layers have many fast
tests; higher layers have few.

```
              manual checks (IME, real keys, look on macOS)
            E2E over the control socket       few
          pixel tests (Metal, lavapipe)       few
        text-grid snapshots                   every visible feature
      command scenario tests                  every command
    unit, property, reference-model tests     most
  alongside: fuzzing, performance budgets, mutation testing, coverage
```

### Rules

- A bug fix adds a test at the lowest layer that reproduces the bug.
- Every command gets scenario tests (`add-command` skill).
- Every feature that changes what is drawn gets text-grid snapshots.
- Pixel tests and E2E tests cover seams between components and critical
  flows only, never logic a lower layer can test.
- Snapshot updates are reviewed like code.

### Text-grid snapshots

The screen is rendered as a grid of characters and compared as text with
`insta` ([ADR 0045](adr/0045-text-grid-snapshots.md)). They need no GPU, so
they run on every OS in `just ci`, and both people and agents can read the
diffs. An open prompt's float comes after the frame: where it is, then its cells, with its
ring as a box of `┌─┐` and the window's hollow cursor beneath it marked `o`
([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md)). Decorations will show
up once they exist.

```
keys: C-f
│日本語 a\xFF        │
:<>^^<>  ~~~~        :
│                    │
=                    =
"                    "
```

A frame is its regions from the top ([ADR 0057](adr/0057-frame-layout.md)). Each
row of the text area is a line between `│`, the modeline's is between `=` and the
echo area's between `"`. Those two draw the spans the app draws.
Below a row that needs it, a line between `:` marks its cells:

| Mark | Meaning |
|---|---|
| `^` | the cursor box, over any other mark |
| `<>` | the two cells of a wide cluster |
| `~` | a cell of an escape (`^M`, `\xFF`) |
| `#` | a cell of a boxed character: invisible (`U+202E`), or past a display cluster's limits (`U+0301`) |
| `]` | a boxed character's last cell, so that two boxes side by side read as two ([ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md)) |
| `w` | a whitespace mark's cells, `»`, `□` or `·`, while whitespace is shown ([ADR 0097](adr/0097-visual-aids.md)) |
| `m` | the wrap mark's cell, `↩` in the last column of a row that continues ([ADR 0100](adr/0100-soft-wrap.md)) |

Below the row with an input method's preedit, a line between `_` marks its
underline with `-`, and the segment being converted with `=`
([ADR 0069](adr/0069-macos-input-method.md)).

Below a row with a region, a line between `+` marks the cells it fills with `*`,
from `Editor::region_cells`, as the renderer fills them
([ADR 0071](adr/0071-mark-and-region.md)). The same line marks the current isearch match
with `#` and the other matches in view with `~`, from `Editor::search_cells`
([ADR 0082](adr/0082-isearch.md)), and the current line's band with `l`, trailing
whitespace with `t` and the cursor flash with `f`, from `Editor::current_line_cells`,
`Editor::trailing_cells` and `Editor::flash_cells`
([ADR 0097](adr/0097-visual-aids.md)). The flash shows at full strength when a step's last
key flashes, as `Editor::flashes` says after each key. The visual aids start off in every scenario, so a
snapshot shows only those it turns on.

With line numbers on, each of a window's rows begins with the gutter's text, from
`Editor::gutter_into`, then `┆`, and the lines of marks below it are shifted by as much
([ADR 0097](adr/0097-visual-aids.md)).

With several tiled windows ([ADR 0112](adr/0112-window-splits.md)), each comes in the
tree's order after a line such as `WindowId(1) at row 6, column 0: 5 x 24, selected`, with
its rows and its modeline. A window with a divider at its right ends each of those lines
with `┃`. The modeline of a window that is not selected is between `-`, its cursor is `o`,
and it has no region, band, flash or isearch match. Beneath a prompt every modeline is between
`-`, but the window the prompt came from keeps its region and band. A frame of one window has
no such line, so it reads as before.

`▌` and `▐` stand for a wide cluster that an edge of the window cuts in half.
Text that reads like an escape has no mark, which is how the two differ. `]`
shows where one box ends and the next begins, as the renderer's 1-pixel gap does. The
padding before the closing `│` shows the row's width whatever font the diff is
read in.

**To write one,** add a test to `crates/oso/src/dispatch/grid_tests.rs`. It
lives there because `oso` is a binary crate, so `tests/` cannot reach the
`Dispatcher`:

```rust
#[test]
fn page_keys_scroll_by_a_screen() {
    let text: String = (0..12).map(|i| format!("line {i:02}\n")).collect();
    // A name, the file's bytes, the window's rows and columns, gutter and text, and the keys of
    // each frame. "" is a frame with no new keys. Keys add up from frame to frame.
    check("page_scrolling", text, (5, 12), &["", "C-v", "M-v"]);
}
```

`check_frame` takes the rows and columns of the whole frame instead, for scenarios
about the frame itself. A step `run:` and a command's name, with a count after a space, such
as `run:enlarge-window 2`, runs that command as the control socket's `run-command` does, for
commands with no keys.

Use a small window, write control and invisible characters as escapes in the
test, and keep a scenario to one idea.

**To update them,** run `just snapshots`, read `git diff` as you would code,
and explain each change in the pull request. `just ci` never accepts a
snapshot: a new or changed one fails it, and so does a `*.snap.new` file that
a plain `cargo test` left behind. `just snapshots` clears those; delete one by
hand if its test is gone.

The grid draws through the code the renderer uses
(`DisplayCluster::for_each_run`, `Window::cursor_point`,
`motion::cursor_cells`, `frame::cursor_rect`). Anything drawn that a test cannot
see this way belongs
in the same code, not in the renderer. What happens after the cells, such as
glyph placement and colors, is left to unit tests of the quads and to pixel tests.

### Guard tests

Some tests check the repository rather than behavior:

- `crates/oso/tests/workspace_lints.rs`: every crate uses the workspace
  lints.
- `crates/oso/tests/architecture`: dependency direction, confined crates,
  file sizes, `static` items, module headers, conflict markers, and where the
  control directory may be made, listened on, cleaned or written to: only the
  dev tooling, until the agent channel's start command joins it
  ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md),
  [ADR 0038](adr/0038-architecture-guardrails.md),
  [ADR 0039](adr/0039-architecture-guardrail-mechanics.md),
  [ADR 0041](adr/0041-module-responsibility-headers.md),
  [ADR 0042](adr/0042-statics-snapshots-and-render-errors.md)).

Each check comes with a unit test on a fixture that shows it catches a
violation, so a broken parser cannot pass silently.

The conflict marker check reads every UTF-8 file in the repository and names each
marker by file and line. It cannot ask git what is tracked, so it keeps its own copy
of what `.gitignore` covers, other sessions' worktrees included. A file not yet added
counts too, such as an editor's backup of a conflicted file. Delete it: adding it to
`.gitignore` does not stop the check. A line of exactly seven `=` fails too, as a
conflict resolved by deleting only the outer markers leaves it behind. That also
fails a Markdown heading underlined at that length, so write headings with `#`. To
show a marker in a doc, indent it.

### Test quality

Three measures ([ADR 0037](adr/0037-test-pyramid.md),
[ADR 0050](adr/0050-test-quality-checks.md)). None of them fails the build for
what it finds.

**Mutation testing** (`just mutants`, weekly in `mutants.yml`) changes
`oso-core` one small edit at a time, such as `<` to `<=`, and runs its tests
against each. Each mutant ends in one of four lists:

| List | Meaning | What to do |
|---|---|---|
| `missed.txt` | the tests passed: nothing noticed the change | triage it |
| `caught.txt` | a test failed | nothing |
| `timeout.txt` | the tests did not finish in 30 s, usually an endless loop | nothing, unless a slower test makes it grow |
| `unviable.txt` | the change does not compile | nothing |

The lists are in `mutants.out/` after a local run, and in the workflow's
`mutants` artifact and job summary. The weekly design health issue shows the
counts and the first 100 missed, from the last run.

**To triage a surviving mutant,** read its line and ask whether any input could
tell the two versions apart:

1. **It can:** this is a gap in the tests. Add a test in `oso-core`, at the lowest
   layer, that fails with the mutant. `cargo mutants -p oso-core -f FILE --re
   'in FUNCTION'` checks it in seconds. A test in the tip crate does not count:
   the run tests `oso-core` alone, as the logic should be testable there.
2. **It cannot, in any input:** the mutant is equivalent. Add a pattern to
   `exclude_re` in `.cargo/mutants.toml`, starting at the mutation's column, and
   prove it in a comment: name what makes the two versions the same. `just
   mutants-exclusions` checks that each pattern matches the one mutant. If the
   proof shows that code is removable, remove it instead. If it comes down to
   "tests happen not to reach it", the mutant is not equivalent. A mutant that
   changes only speed is excluded if the cost stays within a chunk or a constant,
   and stays in the list if a test could pin it, such as a chunk count.
3. **Not worth a test yet:** leave it in the list. A surviving mutant fails
   nothing, and the list is a queue. It is worked through in the refactoring
   pull requests that the weekly report feeds, smallest functions first, and
   anything in the path of a real bug goes first.

**Coverage** (`just coverage`, `coverage.yml`) is in the job summary of each
pull request that changes more than docs: lines, functions and regions per
crate, then per file. Use it to find code no test reaches, not as a target. It measures `cargo test`
on Linux with the dev features, so:

- The window, the event loop and the GPU do not run: `crates/oso/src/app.rs`,
  `crates/oso/src/app/` and much of `oso-render` read at or near 0%. That is
  expected. The end-to-end tests and checks by hand cover them, and this does not
  count those.
- Code for other OSes, such as CoreText on macOS, is not compiled and so not
  counted at all.
- `crates/oso-core` is the number that means the most. It holds the logic, and
  everything in it can run here.

**The time budget** is 30 seconds for the whole test run on the author's Mac.
`just test-time` runs `just test` once to build and times a second run, so
compiling does not count, and neither do coverage and benchmark runs. On the reference Mac
it takes about 4.6 s today. CI runs it on Linux and warns past 60 s, where the
same run takes about twice as long as on the Mac. A warning there never fails
the build, and it is a reason to run `just test-time` on the Mac, which decides.

## Testability by layer

| Testability | Layer | Strategy |
|---|---|---|
| High | `oso-core`: buffers, selections, display model, registry, keymaps | inline unit tests and property tests |
| High | pure coordinate functions | unit tests |
| Medium | command scenarios: key sequences to state | tests in `crates/oso/src/dispatch.rs` with no window |
| Medium | what is drawn: key sequences to the cell grid | text-grid snapshots in `crates/oso/src/dispatch/grid_tests.rs` with no window |
| Medium | the running app | end-to-end tests over the dev control socket: `just e2e` in `crates/oso/tests/e2e` |
| Low | `oso-render` GPU code | logic split out and unit-tested; pixels checked by `just pixel-tests` |

## Test levels

### Level 1: logic tests, no GPU (always)

- Run in `cargo test` and `just ci`.
- Cover:
  - cursor and selection motion, including multiple cursors
  - scrolling rules as pure functions (`oso_core::window`)
  - streaming file decoding against a whole-file reference, with every
    split (`oso_core::decode`)
  - buffer edits on Japanese text and emoji
  - display-model mappings in both directions, with folds, inlays and wraps
  - keymap resolution and command dispatch
  - what a frame draws (`oso-render`'s `scene`): pane regions, draw order, what each
    layer holds, clipping, rows per pane and a full atlas, with a fake text system
    and a fake atlas
  - safe save on a scratch directory, with no window and no thread (`saver::run`):
    permissions, links, conflicts and that nothing is left beside the file. Modes that
    need another owner or group are pure functions, as tests cannot make those files
- Reach every command through the registry, as a user would. Key
  sequences run through the tip crate's `Dispatcher` with no window, for
  example `C-f C-f C-b` to a cursor position (`crates/oso/src/dispatch.rs`).

### Level 2: end-to-end tests over the control socket

`just e2e` starts real dev builds and drives them over the control socket
([ADR 0037](adr/0037-test-pyramid.md), layer 5;
[ADR 0049](adr/0049-end-to-end-test-harness.md)). `just e2e 20` repeats the
suite. On macOS it runs on oso's own window layer, and on Linux on winit
([ADR 0070](adr/0070-windowing-and-input-without-winit.md), stage 3). It runs on the author's Mac, on Linux CI with Xvfb and lavapipe, and on macOS
CI, and it is not part of `just ci`, because windows open and take the focus. Leave
the keyboard and mouse alone while it runs: real input reaches its windows too, and
what you type goes into their buffers, which can fail a flow.

| Flow | The seam it checks |
|---|---|
| keys and commands | socket to UI thread to window: `send-keys`, `run-command`, the cursor, scrolling, the visible text, `oso ctl --pid` and its exit status |
| screenshot | window to renderer to PNG: the window's size, the cursor moves by a row of cells, and text pixels (see below) |
| invalid bytes | file to loader to buffer: read-only, with `invalid_bytes` |
| secret files | path judgment to every way out: `get-state` with no positions and nothing an edit or a save changes, `get-text`, `screenshot`, `check-handover` and `export-state` refused for one reason, `take-timings`, `*Messages*`, the echo area and the log, also through a symlink and after a motion and an edit, and `show-buffer` allows them again |
| modeline and echo area | a key that changes only the echo area to a frame; the bidi warning; `*Messages*` holds no restart, and takes no typing |
| restart handoff | `export-state` in one process to `import-state` in another, without cargo; a wrong version leaves the new one clean |
| unsaved text | an edit changes the generation, which a guarded `exit` then refuses, and no `check-handover` or `export-state` quotes it; an ordinary `exit` still goes; an edit queued behind an agreed exit is refused |
| carried text | unsaved text, its region and its kill go across in pieces to a new instance, which saves them to the file; a buffer over 1 MiB goes in many pieces; an unsaved `.env` holds the restart by name; `dev-message` reaches the echo area and `*Messages*` (`handover.rs`) |
| exit | `C-x C-c` and `exit` end with status 0 and remove the socket |
| save | keys to a file read from outside, with nothing left beside it, and a restart then carries nothing and reads the file again |
| emergency save | a panic outside a command, a caught one, an end nobody agreed to, a confirmed exit and, on macOS, a real request to quit (on oso's own layer a quit Apple event with a logout reason), to copies read from outside: the text exactly, the modes, no secret buffer, and no text in the log |
| exit guard | `C-x C-c` asks with unsaved text, and `y` ends the process and leaves the file as it was; after a save it asks nothing, and a save that fails after a confirmed exit keeps the instance and the text. On oso's own macOS layer a person's quit, as another app sends it, asks the same question, `n` stays and `y` goes with nothing written (ADR 0084). What makes it ask again is tested through the `Dispatcher` |
| prompts | `M-g g` reads a line in a float and goes there, `C-g` runs nothing, the IME off in a prompt and on after it, the exit question on `n` and `y`, a passphrase withheld from `get-state`, screenshots and the log, and a restart held while a prompt is open (ADR 0072) |
| cleanup | a failing test leaves no process, socket or scratch directory, and another instance's buffers stay; where `CI` is set, so does the rest of its state |

**What it asserts differs by OS.** Linux has no text backend
([ADR 0036](adr/0036-text-system-and-glyph-atlas.md)). Its screenshots show the
background and the cursor box, so a flow checks the cursor on a file of empty
lines, which looks the same everywhere, and text pixels only on macOS. Linux
having no text pixels is asserted too, so the line changes when it gets a backend.

**To add a flow,** add an ignored test to `crates/oso/tests/e2e/main.rs`, and only
if a component boundary or a critical flow has no test yet. Motion, widths,
decoding and the grid belong in the layers below. Use `Scratch` for files, with
fake secrets only, and `App`, which addresses its own instance by process ID and
stops it even when the test panics. Begin with `App::wait_until_ready`. Creating the
surface can hold the UI thread for longer than the server waits, so until a state has
shown the surface, and only until then, it tries again after a request with no reply.
From then on no reply fails the flow, in that call too, so that a freeze does not go
unnoticed. Wait with `App::wait_for`, never a sleep:
`send-keys` and `run-command` have finished when they return, and anything else
is polled through `get-state`. Failure messages name what was expected and never
quote buffer text, so use `assert_visible_row`, and do not `assert_eq!` on a state
that holds text.

**What is still manual,** through the `verify-in-app` skill or by hand: resizing
the window, the mouse wheel, real OS key events, the window's close button, leaving
while a save runs, IME, and `oso dev`'s build loop, with Ctrl-C and closing the
terminal.

**Layers in real pixels.** No feature draws a pane above another yet, so no flow can see
it. `just pixel-tests` runs five ignored tests in `oso-render`'s `paint`, which draw panes
on a real GPU with a block for a glyph and read the pixels: an opaque pane hides the text
under it, a pane's text goes over its background, both layers draw their own ranges, a
glyph stops at its pane's edges, and the frame's top row keeps its band. They are ignored
because `cargo test` must not need a GPU. `e2e.yml` runs them on Linux with lavapipe and on
macOS. The glyphs come from a fake text system, so they assert the same on both, and a
failure on Linux is real. There are no stored images to compare. Once M2-16 draws floats,
a flow can take over.

**Themes** ([ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md)): a unit test checks
the contrast of every face's pairs in every theme, with no GPU. A pixel test keeps the text
gamma curve. `OSO_THEME_SAMPLES=<dir> cargo test -p oso-render -- --ignored theme_samples` draws
two sample frames of every face in each theme off screen, with the OS's font, and writes their
RGBA pixels and a table of each pair's contrast, for choosing colours.

The socket's methods, for flows and for agents following `verify-in-app`, over
the dev control socket ([ADR 0023](adr/0023-control-socket-protocol-and-restart.md),
[ADR 0032](adr/0032-control-socket-transport.md)):

- `send-keys` sends input in Emacs notation, through the same path as keys from
  the window ([ADR 0033](adr/0033-key-presses-and-notation.md)).
- `run-command` runs a registered command by name, with a numeric argument of at
  most 10,000 if it takes one, or else a prefix argument the keys left. Command errors
  and panics come back as error responses.
- `get-state` reads the state as JSON: every buffer (name, path, whether it may
  hold secrets, read-only reason, whether it is modified or being saved, line ending,
  BOM, invalid-byte count, loading progress, size), every window with its selections, the selected window's
  buffer, cursor and scroll position, its visible text, the keys of a pending
  prefix, what the modeline and the echo area say, and the surface size. The visible
  text stops at 128 KiB and says so with `truncated`. A secret buffer has no text, no
  cursor, no scroll position and no selections, and no size while it has edits
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)). While one is
  shown, `modeline` and `echo` are left out
  ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).
- `get-state` also gives `kill_ring`: the number of entries and the newest one's pieces,
  at most 16 of 1 KiB each, leaving out entries killed from a buffer that may hold secrets,
  and null while one is shown; and `clipboard.waiting`, while a yank waits for the
  clipboard ([ADR 0074](adr/0074-the-kill-ring-and-the-clipboard.md)).
- `clipboard` reads the clipboard in memory, or with `text` (and `concealed`) sets it as
  another app would. It works only in an instance started with `--clipboard memory`, which
  the harness always passes, so no flow touches the person's clipboard. Text marked
  concealed reads as null.
- `get-text` reads whole rows, up to 512 KiB and 10,000 rows, and fails clearly
  past that.
- `show-buffer` shows a buffer by id, until `switch-to-buffer` arrives in M2.
- `dev-panic` panics on the UI thread outside the command boundary, and `dev-end-unasked` ends the event loop with nobody having agreed to it. The command `dev-panic`
  panics under the boundary. Each panic's message quotes the buffer's text, to show that it goes nowhere
  ([ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md)). `--emergency-dir DIR`, as a dev build's first argument, sends the copies elsewhere, and the harness always passes it.
- The harness passes `--no-config`, so no flow reads the person's configuration file, or
  `--config FILE`, a dev build's, after `--emergency-dir` and `--clipboard`, for the flows of the
  configuration itself ([ADR 0108](adr/0108-the-user-configuration-file.md)). `just perf` gives
  each instance an empty configuration directory of its own through `XDG_CONFIG_HOME`, or one
  holding the file `OSO_PERF_CONFIG` names.
- Secret-pattern buffers give no text, no screenshot and no positions
  ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md),
  [ADR 0051](adr/0051-m2-plan-decisions.md)). While one is shown, `get-state` gives no
  `modified`, `saving` or size of such buffers, and no restart is handed over. Keys in progress,
  a composition and prompts open then stay out of `get-state` and `screenshot` until they end
  ([ADR 0079](adr/0079-state-and-restarts-around-a-secret-buffer.md)). Any file
  counts as secret until its path is resolved, so wait for `loading` to become
  null before reading it.
- `export-state` and `import-state` hand the state to a new instance, `export-text`
  and `import-text` move the texts it carries in between, and `exit` stops one.
  `just dev` (`oso dev`) drives them on each source change, and says what it has to
  say with `dev-message` ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md),
  [ADR 0090](adr/0090-oso-dev-carries-unsaved-text.md)).
- `check-handover` reports the generation, or why a restart would lose text, and
  `exit` with a `generation` exits only if that is still current. Without one, `exit`
  goes whatever the buffers hold, which is what flows and agents want
  ([ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md)).
- `oso [FILE]` and `open-file` open a file, so test files (generated in a scratch
  directory, never committed) can be driven this way.
- `take-timings` reads key latency, startup and load times for benchmarks. Each key's
  latency, and where it went (`key_spans_us`: handled, waiting for the frame, taking in the
  loads, preparing, waiting for a drawable, presenting), is a sample handed over once, as is
  `dropped_keys`; the next call has only newer ones. `frames` counts since the start, so
  only its difference says how many were drawn between two calls. It records no
  key if a secret-pattern buffer is shown just before or after it or when its frame is
  presented, and no frames while one is shown, but the first frame. A file counts as secret
  until it is judged ([ADR 0060](adr/0060-timings-while-a-secret-buffer-is-shown.md)). The load
  times are of the first file judged ordinary, and its first screen counts only if no such
  buffer is shown then ([ADR 0078](adr/0078-load-times-and-log-lines-around-a-secret-buffer.md)).
- `screenshot` writes a PNG of the current frame and returns its path.

The transport and protocol themselves are tested without a window or GPU
(`oso-platform` control tests, the server tests in the tip crate), and so are
process spawning, the state format, the order of a restart and the source
watcher, so CI runs them on Linux and macOS. Windows is not supported yet
(ADR 0032).

The agent channel's flows (`tests/e2e/agent.rs`, ADR 0085) talk to the channel's own socket
as an agent would, with the dev socket's keys standing in for the person's. The one for
`oso mcp` gives the bridge its requests in a file, with the dev-only `oso mcp --requests FILE`:
a child that `oso_platform::process` starts gets a closed stdin, by design. So the path where
stdin stays open while the person decides is tested only in the bridge's unit tests.

### Level 3: snapshot tests (after text rendering lands)

- Render headlessly and compare PNGs with a tolerance.
- macOS runs locally on Metal. Linux CI uses Mesa's software Vulkan driver
  (lavapipe), since oso has no GL backend.
- Golden images are used only once the UI is stable enough that false alarms
  stay rare.

## Performance tests

`just perf` builds an optimized binary with the dev feature and runs the
harness in `crates/oso/examples/perf`, which drives the app over the control
socket. Windows open and take the focus, and a hidden window fails the run.
Files are generated in a temporary directory and never committed. It writes
JSON to `target/perf/perf.json`; `just perf OUT --quick` is for trying it out.

`just perf-remote BASE PAIRS FEATURES BASE_FEATURES` runs the same harness on another
Mac, named by the ssh host in `OSO_PERF_HOST`. It builds BASE (by default `origin/main`)
and the working tree here, both with FEATURES (by default `dev`), or the base with
BASE_FEATURES if given, as a base from before ADR 0070's stage 3 needs `dev,native-window`
for oso's own macOS window layer, copies the two binaries and the harness there, and runs them in
alternating pairs, the base first, under `caffeinate -d`. The results and logs come
back to `target/perf/remote/`, each pair is compared as `just perf-compare` does, and
each run says how much the host swapped meanwhile. Nothing runs there but the
benchmark, so builds and tests may go on here during it. The host needs a logged-in
session with a display kept awake, and runs what is built here, so it has the same
OS and architecture. Nothing is built or installed there.

| Metric | What it measures | Runs |
|---|---|---|
| `startup_ms` | just before the process starts, to its first presented frame | 9, plus a first launch reported alone |
| `key_p50_ms`, `key_p99_ms` | key received to first frame presented, on a screen of mixed text | 5, 400 keys each |
| `type_p50_ms`, `type_p99_ms`, `del_p99_ms` | the same while typing there (ASCII, Japanese, an emoji and line breaks), and while `DEL` removes it | 5, 400 keys each |
| `ime_preedit_p99_ms` | an input method's preedit report to the frame that draws it, there, through the dev socket's stand-in, as a word grows, converts and clears; reported only (ADR 0069) | 5, 400 reports each |
| `prompt_type_p99_ms` | digits typed into `goto-line`'s open prompt to the frames that draw them, there; reported only (ADR 0072) | 5, 400 keys each |
| `mx_type_p99_ms` | a letter and `DEL` typed in turn into `M-x` listing every command, to the frames that draw them; reported only (ADR 0076) | 5, 400 keys each |
| `file_prompt_type_p99_ms` | a letter, `.`, `*`, a digit and four `DEL` typed in turn into `C-x C-f` over a directory of 100,000 empty files whose names mix ASCII, accented and Japanese letters, to the frames that draw them, while a worker builds and matches the list; making the files takes about 4 s and removing them about 4 s, once per run of the benchmark; reported only (ADR 0096) | 5, 400 keys each |
| `open_first_screen_ms`, `open_full_load_ms` | opening 100 MB with the window up, to a complete screen and to all of it loaded | 5 |
| `big_*`, `line_*` | a 1 GB file and a 10 MB line: keys during and after loading, an unbound key's trip through the UI thread, typing and `DEL` in the middle of the file and near the end of the line, and edits without a window | 3, 160 keys each |
| `dev_build_ms` | `just build-time`: one changed file in the tip crate, an incremental dev build | 5 |

- Key latency is measured inside the app and ends when the frame is presented
  ([ADR 0025](adr/0025-latency-measurement.md)). The display's refresh and the
  compositor come after that, and real photon latency is checked by hand.
- Typing is measured from key to frame ([ADR 0056](adr/0056-typing.md)). A run fails
  unless each key drew a frame and the text grew by what the keys type. The unbound
  key's trip stays as an alarm for a UI thread that freezes, and the cost of an edit in
  the core structures as a view of that one part.
- `just perf-compare BASE NEW` applies the thresholds. CI catches large
  regressions. Its runners have no hardware GPU, so the budgets are read by hand
  on the author's Mac, where `just perf` lists them beside the results
  ([ADR 0046](adr/0046-performance-regression-checks.md),
  [ADR 0024](adr/0024-dev-build-profile.md)).
- Key latency ends when `present` returns, so it leaves out the GPU's work as
  well as the display and the compositor.
- `crates/oso-core/examples/text_perf.rs` remains for looking closely at the
  rope and the display model. `-- cursors` runs only typing and undo at 1, 100 and 1000
  cursors, bare, with marks and over regions (ADR 0091).

## Fuzzing

`cargo-fuzz` targets cover file loading and decoding, the display model under
random edits, highlighting and terminal escape-sequence handling. Agents can
run them for long periods.

## CI

GitHub Actions (`.github/workflows/ci.yml`) runs on every pull request and
every push to `main`. See [ADR 0021](adr/0021-development-tooling.md).

A pull request that changes nothing but `docs/` builds nothing: its first job, `changes`,
finds that from the files it changes since it left `main`, and every job below that builds
or checks code is skipped, so it shows as skipped, not failed. The gitleaks scan and
`just markers`, which looks for conflict markers in every tracked file with `git grep`,
still run. A push to `main` always runs everything.

- `just ci` on Linux, macOS and Windows. `check`, `lint` and `test` cover
  the dev features too (`--all-features`). The macOS job also runs
  `just public-api-macos-check`, below, so that a push starts one macOS job.
- `just release-safety` on Linux: release builds contain no dev-only
  features, crates or code ([ADR 0032](adr/0032-control-socket-transport.md))
- `cargo-deny` and `cargo-audit`
- a gitleaks scan of the full history
- `just markers` on Linux: no conflict marker in any tracked file, without a build, as
  the architecture test checks within `just ci`
- `just public-api-check` on Linux: the public API matches
  `guardrails/public-api` ([ADR 0038](adr/0038-architecture-guardrails.md)); and
  `just public-api-macos-check` in the macOS `just ci` job for `oso-platform`'s macOS API,
  with the native window layer ([ADR 0070](adr/0070-windowing-and-input-without-winit.md))
- the structure workflow (`structure.yml`): a structural change needs an
  ADR cited in the pull request description
- weekly, the design health report (`design-health.yml`) as a GitHub issue
- the performance workflow (`perf.yml`), on every pull request and every push
  to `main` that changes more than `docs/`: results in the job summary and in
  JSON artifacts, compared with the last result on `main`
  ([ADR 0046](adr/0046-performance-regression-checks.md)). Pull requests run the
  benchmarks on Linux only; a change to the input, render or load paths is
  measured with `just perf-remote`, or with the workflow started by hand, which
  includes macOS ([ADR 0081](adr/0081-macos-benchmarks-off-pull-requests.md))
- the end-to-end workflow (`e2e.yml`), on pull requests and pushes to `main` that
  change more than `docs/`: `just e2e` on Linux with Xvfb and lavapipe, and on
  macOS, the one runner that checks text pixels
  ([ADR 0049](adr/0049-end-to-end-test-harness.md))
- `just test-time` on Linux: a warning when the test run grows past its budget
- the coverage workflow (`coverage.yml`) on pull requests, in the job summary;
  it cannot fail the build
- weekly, mutation testing of `oso-core` (`mutants.yml`), read by the design
  health report ([ADR 0050](adr/0050-test-quality-checks.md))
- weekly, `oso-syntax`'s property tests with 4096 cases (`syntax-properties.yml`): random
  edits, resumed slices and crafted nesting for every grammar; `just ci` runs them with few
  ([ADR 0111](adr/0111-tree-sitter-highlighting.md))
- CodeQL code scanning (`codeql.yml`), `Analyze (rust)` and `Analyze (actions)`, on pull
  requests, pushes to `main` and weekly. It replaces the repository's default setup, which
  is off. A pull request that changes nothing but `docs/` and Markdown files skips the Rust
  analysis's steps, and the job still reports

Planned:

- snapshot tests on Linux with lavapipe, once they exist
