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

- Startup misses its budget: the first presented frame comes 110 to 130 ms
  after the process starts, usually about 118 ms (optimized, warm, the
  author's Mac), against 100 ms. The budget stays for now. Removing AppKit's
  default menu took it down from about 172 ms
  ([ADR 0046](adr/0046-performance-regression-checks.md)). What remains is
  mostly AppKit and the window server: about 30 to 48 ms to build the event
  loop, 10 ms to `resumed`, 37 ms for `create_window`, and about 33 ms until
  macOS reports the window visible, before which wgpu skips the frame as
  occluded. The GPU takes only about 3.5 ms, and starting it in the
  background, showing the window late and starting from an app bundle made
  no measurable difference. Decide whether to look for more in AppKit or to
  change the budget. The "50 ms after window creation" in ADR 0036 counted
  a skipped frame.
- Key latency leaves out the GPU's work. If a frame's GPU cost starts to
  matter, record GPU completion too, in dev builds. Key samples also end at
  the first frame after a key, even if glyphs fill in over the next ones.
- Rendering: a multi-row cluster iterator that keeps one cursor across
  rows, instead of a seek per row. A cached row is reused without a seek
  while the buffer's revision holds. After a change every visible row seeks
  and is read again, which costs about 0.05 to 0.12 ms for a screen of 60
  rows of ordinary text, so this waits until a profile shows it.
- The bidi-control modeline warning (`ui.md`) needs a count of bidi controls,
  which the TabMap summary does not track yet. Add it when M2 needs it.
- A path swapped for a FIFO between the loader's check and its open blocks
  the loading thread (not the UI). Opening with `O_NONBLOCK` closes this
  and needs `libc` behind an `oso-platform` filesystem helper.
- UI-thread allocations per frame: every `SumTree` seek allocates a cursor
  stack, and every visible glyph hashes its key in the atlas with SipHash.
  A frame costs 0.07 to 0.5 ms of CPU (p50 to p99) today; an inline cursor
  stack and a cheaper hash are the next steps if a profile shows them.
- `unicode-width` 0.2 and 0.1 (through naga) are both in the build.
  Re-check when wgpu is upgraded.
- `ttf-parser` is unmaintained (RUSTSEC-2026-0192, ignored in `deny.toml`).
  It comes from winit's Wayland decorations. Re-check when winit is
  upgraded.
- Moving to a display with another scale loads the font synchronously on the UI
  thread (`App::set_scale`), against "never block the UI thread". Measure it, and
  move the load to a background thread.
- After a render error, the comment in `App::draw_frame` says the next frame
  retries, but nothing requests one under `ControlFlow::Wait`, so the window can
  stay stale until another event. Decide on a bounded retry or correct the comment.

## M2

- By M2-8: once a buffer can be saved, `send-keys` and `run-command` can edit and write any file
  a client opens, secret or not. The trust statement in `security.md` speaks only of reading,
  and a sandboxed client could write outside its sandbox this way.
- By M2-15: the generation that guards `exit` does not change on a save between the
  export and the exit, and does not cover marks, the kill ring or file information
  ([ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md)). Decide what it must cover
  once the state carries them.
- Motion functions take display rows (`motion::line_start(display, row)`),
  which are buffer lines while there is no soft wrap. When the WrapMap
  lands, `C-n`, `C-a` and `C-e` must keep moving by logical lines
  (`display-model.md`), so they need line-level conversions then
  ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)). `TAB` counts
  display columns the same way ([ADR 0056](adr/0056-typing.md)), and word motion
  treats the end of a row as the end of a word
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)).
- `kill-buffer` must cancel a load at once (ADR 0022). Today the app drops
  a load's handle only when it next sees the buffer gone; once a command
  can kill a buffer, the app should reconcile its loads right after
  commands.

- isearch must not match the U+FFFD of an invalid byte
  (`InvalidBytes`, ADR 0040).
- Killing a large buffer (and a load that fails midway) frees its trees on
  the UI thread, which takes tens of milliseconds for a 1 GB file. Drop
  them on a background thread.
- Large deletions free part of their trees on the UI thread
  ([ADR 0053](adr/0053-edit-core.md)). Killing 89 MB of a 100 MB buffer takes about
  1.2 ms, nearly all of it freeing the old display map's items. Its text stays in the
  undo group until the history drops it, on the edit that passes the 64 MiB bound, which
  takes about 2.5 ms (one early run took 17.6 ms, not seen again). Undoing an 89 MB paste
  and then typing frees the redo group in the same way, in 2.4 ms. These scale with the
  text: about 25 ms per GB. Decide before M2-14, which brings kills, whether to send
  dropped groups to the background thread above.
- Undoing a kill puts its text back at about 0.7 ms per MB (63 ms for 89 MB) on the UI
  thread, and for a moment holds several copies of it, since the slice is copied, split
  into chunks and measured again. Running out of memory aborts with no emergency save.
  Redoing takes 1.4 ms. Sharing the slice's chunks with the rope, and keeping display items
  with the undo group, would make it O(log n). Decide before M2-14. `rope.rs` is at its
  size limit, so sharing needs room there.
- Typing with 1000 cursors takes about 15 ms per key (1.3 ms with 100), over the 8 ms
  budget by itself. Applying a batch of splices in one tree pass instead of one
  `Rope::replace` each is the next step. Do that or cap the cursors before M2-21. A command
  also clones the selection set for its undo group, and an edit a few times more, which counts
  with that many cursors.
- Every window showing a buffer clips all its cursors after each edit, though only those
  in the edited chunks can change. Limit it to those before splits (M3) bring more windows.
- After M2-25: deleting back leaves `left` where it is, as the fewest columns that show the
  cursor ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)), so a row can stay
  scrolled sideways with text hidden to its left. Check how that reads once rows wrap.
- `Editor::message` has no cap on a message's length. Its callers so far pass short text and
  file names. Add one by M2-7.
- By M2-7: a command that fails leaves a message and asks for no frame, so the echo area would
  not show it until something else draws. "C-x C-z is undefined" has no way to the echo area
  either: `Editor::message` always logs, and typed keys stay out of the `info` log
  ([ADR 0056](adr/0056-typing.md)).
- `S-RET`, `S-DEL` and other shifted named keys with no binding do nothing. Emacs falls back to
  the unshifted key, so holding Shift a moment too long still types. Decide whether
  `KeyResolver` does the same, or record that oso differs.

- By M2-6: with the view sideways, a cursor one column to its left can show in the
  window's margin, since panes are not clipped yet
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)).
- By M2-7: nothing shows that a prefix argument is being typed
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)).
- Clipboard backend: `arboard` or `objc2-app-kit`, chosen with M2-14 through the
  dependency approval.
- By M2-12, from trying the macOS IME by hand (winit 0.30), with its ADR to record
  them:
  - `set_ime_allowed(false)` is enough to switch the IME off, so no new `unsafe` is
    needed. The OS indicator stays on the input source.
  - With the IME on and a Japanese input source, Option-a starts a composition
    instead of arriving as `s-a`. Try `set_option_as_alt` first, then switching the
    IME off while Option is held.
  - A plain letter can start a composition, so the IME goes off while a prefix is
    pending too, as in the minibuffer, isearch and menus. Check this at the keyboard.
  - Space arrives as `SPC`, and winit cannot deliver a full-width one. Space stays
    half-width, and a command, `insert-full-width-space`, inserts the other.
  - Switching off during a composition leaves it with the OS, and the next key brings
    it back. Defer switching off until the composition ends.
  - With several cursors, decide which one shows the preedit.
- Glyphs under the cursor box take `cursor_color` for their whole quad, including
  pixels outside the box, so ink above or below the row or in a ligature's other
  cells is drawn in the background color. Split the quad at the box
  ([ADR 0052](adr/0052-glyphs-stay-in-their-cells.md)).

## M3

- With configurable fonts: `Placer::place` ends a span where the style changes,
  so a shaper cluster crossing a boundary between text and a label is cut to its
  first cell, and the next grapheme's cell stays empty. Menlo forms no such
  cluster: no ASCII pair across such a boundary shaped as one. SF Mono has not
  been checked. Shape labels apart from the text around them, or otherwise keep
  them out of shaping context, before fonts become configurable
  ([ADR 0052](adr/0052-glyphs-stay-in-their-cells.md)).

- User-defined secret patterns ([ADR 0028](adr/0028-secrets-handling.md)) need the
  resolved paths kept, or resolving again, so config reload can judge open buffers
  again.
- The invalid-byte table lives in the TabMap and is keyed by buffer
  offsets. When the FoldMap or InlayMap goes below the TabMap, move it to
  the lowest layer that still sees buffer offsets, or to `DisplayMap`.
  Its API (`DisplayMap::invalid_bytes`) does not change.
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
