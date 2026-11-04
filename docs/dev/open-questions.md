# Open questions

Topics raised in design discussions that are not decided yet. When one is
decided, record it in an ADR or design doc and remove it from this list. Each
item notes when it must be settled.

## Needs a dedicated discussion

- **UI of each later feature** (before building it). The M2 UI is decided
  ([`ui.md`](ui.md), [ADR 0026](adr/0026-m2-ui-layout.md)); M3 and later
  features (avy, ripgrep results, the menu contents, the git status buffer,
  the terminal) are designed when they come up.

## M1

- Startup misses its budget, in M1-8: the first presented frame comes
  about 175 ms after process start (release, warm), against 100 ms.
  winit's event loop takes 30 to 45 ms to build, `resumed` arrives at
  about 65 ms, the window and GPU take about 50 ms, and the first frame is
  skipped as occluded until macOS shows the window, about 65 ms later. The
  "50 ms after window creation" in ADR 0036 counted that skipped frame.
  Candidates: requesting the GPU adapter and device while the event loop
  builds, and showing the window only once the first frame is ready.
- Text gamma: text is blended in linear space on the sRGB surface, so light
  text on dark looks heavier than in native apps. Decide with the theme.
- Rendering: a multi-row cluster iterator that keeps one cursor across
  rows, instead of a seek per row. Since M1-6 a cached row is reused
  without a seek while the buffer's revision holds, and only rows shaped
  or rechecked after a change seek; a frame costs 0.07 to 0.5 ms of CPU
  (p50 to p99), so this waits until a profile shows it.
- Horizontal scroll extent needs the widest row, and the bidi-control
  modeline warning (`ui.md`) needs a count of bidi controls. The TabMap
  summary tracks neither yet; add them when M2 needs them.
- Large deletions free their chunks on the calling thread. Measure when M2
  editing lands, and drop big subtrees in the background if needed.
- A path swapped for a FIFO between the loader's check and its open blocks
  the loading thread (not the UI). Opening with `O_NONBLOCK` closes this
  and needs `libc` behind an `oso-platform` filesystem helper.
- UI-thread allocations per frame: every `SumTree` seek allocates a cursor
  stack, and every visible glyph hashes its key in the atlas with SipHash.
  A frame costs 0.07 to 0.5 ms of CPU (p50 to p99) today; an inline cursor
  stack and a cheaper hash are the next steps if a profile shows them.
- Segmenting hostile text: a cell holding a 1 KB piece of combining marks
  costs about 1 KB of segmentation, so a screen of it takes about 36 ms on
  first view. Real text is unaffected.
- `unicode-width` 0.2 and 0.1 (through naga) are both in the build.
  Re-check when wgpu is upgraded.
- `ttf-parser` is unmaintained (RUSTSEC-2026-0192, ignored in `deny.toml`).
  It comes from winit's Wayland decorations. Re-check when winit is
  upgraded.

## M2

- Motion functions take display rows (`motion::line_start(display, row)`),
  which are buffer lines while there is no soft wrap. When the WrapMap
  lands, `C-n`, `C-a` and `C-e` must keep moving by logical lines
  (`display-model.md`), so they need line-level conversions then
  ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).
- `kill-buffer` must cancel a load at once (ADR 0022). Today the app drops
  a load's handle only when it next sees the buffer gone; once a command
  can kill a buffer, the app should reconcile its loads right after
  commands.
- Two files with the same name need distinct buffer names (Emacs's
  uniquify); the editor, which knows the other buffers, should name them.
- The invalid-byte table lives in the TabMap and is keyed by buffer
  offsets. When the FoldMap or InlayMap goes below the TabMap, move it to
  the lowest layer that still sees buffer offsets, or to `DisplayMap`.
  Its API (`DisplayMap::invalid_bytes`) does not change.
- `Window::scroll` is a public field, so code can change the top row
  without `Window::set_top`, which moves cursors into view. Make it
  private with a getter before recenter (`C-l`), splits or `goto-line`
  add more writers.

- Editing must invalidate shaped rows by edit range
  ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)). The
  shaping cache checks only the buffer offsets a row covers, which an edit
  that keeps the row's length does not change.
- isearch must not match the U+FFFD of an invalid byte
  (`InvalidBytes`, ADR 0040).
- Horizontal mouse wheel and trackpad scrolling: the view scrolls only
  vertically, and auto-hscroll keeps the cursor in view.
- Memory of very large files: a 1 GB file takes about 1.4 GB, and a binary
  file of random bytes about 3 bytes in the rope and 4 in the invalid-byte
  table per invalid byte. Nothing limits the size of a file opened: decide
  on a threshold that asks first, as Emacs's `large-file-warning-threshold`
  does, once the minibuffer exists. Running out of memory aborts, so
  emergency save would not run.
- Killing a large buffer (and a load that fails midway) frees its trees on
  the UI thread, which takes tens of milliseconds for a 1 GB file. Drop
  them on a background thread.

- Secret-pattern buffers in M2
  ([ADR 0043](adr/0043-control-socket-trust-and-secret-buffers.md)): their
  unsaved edits cannot pass through `export-state`, so decide how `oso dev`
  keeps them (for example, keep the old instance). The emergency save skips
  them too, which loses unsaved work in a file that only could not be
  resolved, so decide whether to tell that case apart. User-defined patterns
  ([ADR 0028](adr/0028-secrets-handling.md)) need the resolved paths kept, or
  resolving again, so config reload can judge open buffers again.

- The panic hook (with emergency save, [ADR 0013](adr/0013-persistence.md)):
  Rust's default hook prints panic messages, which may quote buffer text,
  to stderr even for panics caught at the command boundary. Decide what
  the hook prints, in dev and release builds
  ([ADR 0028](adr/0028-secrets-handling.md),
  [ADR 0034](adr/0034-command-registry-and-keymaps.md)).

- User key bindings from TOML ([ADR 0019](adr/0019-configuration.md)):
  `Keymap::bind` refuses to rebind, so decide between a user layer above
  the global keymap and explicit rebinding and unbinding
  ([ADR 0034](adr/0034-command-registry-and-keymaps.md)).

## M3

- Initial set of tree-sitter grammars, beyond Rust, Markdown and TOML.
- Fuzzing targets and how long they run. The `SumTree`, the rope, the
  display model under random edits, and the streaming decoder and load
  builder (property-tested since M1-6) are the first candidates.
- URL-opening policy: which schemes, and what confirmation.
- Migemo dictionary source and licensing. Common dictionaries derive from GPL
  SKK dictionaries and cannot be bundled under MIT OR Apache-2.0
  ([ADR 0020](adr/0020-license.md)). Options: the user supplies the
  dictionary, a permissively licensed dictionary, or generating one from
  permissive data.

## M4

- Claude Code hooks: which events map to which states, and how
  `oso integration install claude` merges with existing user settings
  ([ADR 0031](adr/0031-claude-code-integration.md)).
- Whether to add native permission dialogs (PermissionRequest hook), a
  read-only transcript viewer, or `claude -p` one-shot features later.
- OSC 7501 is a draft; track its revisions.
- Terminal: PTY implementation per OS (ConPTY on Windows) and the full list of
  allowed and blocked escape sequences.
- If oso is published with the Claude Code integration, check Anthropic's
  terms for third-party clients that launch the user's own `claude`.
- Verify that `gix` can be forced to skip `core.fsmonitor`, filters,
  textconv and external diff drivers.
- Passphrase/credential prompts via `GIT_ASKPASS`: keep the input out of
  history, the kill ring and macros.
- Exact list of git-porcelain commands to implement.

## Later

- An option for East Asian Ambiguous characters to take 2 cells
  ([ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md)). It needs the
  `cjk` feature of `unicode-width`.

- Dev control socket on Windows: a named pipe restricted to the current
  user ([ADR 0032](adr/0032-control-socket-transport.md)). Needed when
  agents test on Windows.
- Linux text-rendering backend: cosmic-text or FreeType + HarfBuzz. Until
  then Linux (and Windows, until DirectWrite) draws no text
  ([ADR 0036](adr/0036-text-system-and-glyph-atlas.md)).
- Exact command subsets for the directory editor and org.
- PDF engine and isolation (pdfium, a pure-Rust renderer, a separate
  process); image decoders.
- OS-level sandboxing of child processes such as language servers.
- `subsecond` hot-patching proof of concept with a plain winit app.
- Generated command reference and the Japanese edition workflow.
- Encodings other than UTF-8 (Shift_JIS, EUC-JP), when they are needed:
  detection, and conversion at the load and save boundary
  ([ADR 0022](adr/0022-text-buffer-and-file-loading.md)).
