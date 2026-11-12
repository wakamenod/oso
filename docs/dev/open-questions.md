# Open questions

Topics raised in design discussions that are not decided yet. When one is
decided, record it in an ADR or design doc and remove it from this list. Each
item notes when it must be settled.

## Needs a dedicated discussion

- **UI of each later feature** (before building it). The M2 UI is decided
  ([`ui.md`](ui.md), [ADR 0026](adr/0026-m2-ui-layout.md)); M3 and later
  features (the menu contents, the git status buffer,
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
- While the window is occluded, no frame is drawn, and what waits for the first frame of a file
  waits with it. A load stays listed until a frame shows its file. A read of the file again that
  started before then was never taken in, until the wake of the loading threads came to reach
  it whatever is listed. In the end-to-end flows, which start windows behind others, a file's
  first frame came after the first keys. Decide whether a load should end when its text and
  verdict are in rather than at its first frame, which also moves when the first-screen timings
  are taken.
- Key latency leaves out the GPU's work. If a frame's GPU cost starts to
  matter, record GPU completion too, in dev builds. Key samples also end at
  the first frame after a key, even if glyphs fill in over the next ones.
- Rendering: a multi-row cluster iterator that keeps one cursor across
  rows, instead of a seek per row. A cached row is reused without a seek
  while the buffer's revision holds. After a change every visible row seeks
  and is read again, which costs about 0.05 to 0.12 ms for a screen of 60
  rows of ordinary text, so this waits until a profile shows it.
- Rendering, since soft wrap draws view rows: key latency is 4 to 9% above main's at p50 and
  p99 on the macmini (about 0.1 to 0.2 ms), and startup 1 to 7%. A frame builds the rows it
  shows (`window::Screen`) and their cells once: 24 to 40 µs on a wrapped 10 MB line, 38 µs on
  100 MB of source, and 85 to 130 µs while composing (`text_perf -- screen`)
  ([ADR 0104](adr/0104-soft-wrap-drawn-in-view-rows.md)). Ways to cut it, once a profile shows
  which:
  - keep the screen while the buffer's revision, the window's top, size and wrap and the
    preedit hold;
  - read the rows by walking the wrap's tree with one cursor, in place of a seek and a column
    lookup a row;
  - on rows that do not wrap, take a line's start from its row, in place of a seek, for the
    trailing whitespace and the cursors on its line.
- A path swapped for a FIFO between the loader's check and its open blocks
  the loading thread (not the UI). M2-18 closes this: the loader opens through
  `oso_platform::file_meta::open_for_reading`, without waiting on the file, as
  `open_for_writing` does for a save
  ([ADR 0067](adr/0067-access-lists-and-the-quarantine-flag-in-a-save.md),
  [ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- UI-thread allocations per frame: every `SumTree` seek allocates a cursor
  stack, and every visible glyph hashes its key in the atlas with SipHash.
  A frame costs 0.07 to 0.5 ms of CPU (p50 to p99) today; an inline cursor
  stack and a cheaper hash are the next steps if a profile shows them.
- `ttf-parser` is unmaintained (RUSTSEC-2026-0192, ignored in `deny.toml`).
  It comes from winit's Wayland decorations, and goes with winit
  ([ADR 0070](adr/0070-windowing-and-input-without-winit.md)).
- Moving to a display with another scale loads the font synchronously on the UI
  thread (`App::set_scale`), against "never block the UI thread". Measure it, and
  move the load to a background thread.
- After a render error, the comment in `App::draw_frame` says the next frame
  retries, but nothing requests one under `ControlFlow::Wait`, so the window can
  stay stale until another event. Decide on a bounded retry or correct the comment.

## M2

- The visual aids' cost on typing differs by how it is measured. With the aids on by default,
  the benchmark (`perf-remote`) puts `key_p50` 0.28 ms higher (about 17%) on the Mac mini, in
  every pair, the aids alone the difference. Measured key by key on the same machine through
  `take-timings`, they cost about 40 µs at the median, mostly the current line's band, with
  one frame per key and no change in waiting for a drawable or presenting. Find what the
  benchmark's way of driving keys adds ([ADR 0097](adr/0097-visual-aids.md)).
- There is no wall-clock figure for typing with line numbers on: the benchmark runs with them
  off, and its scenarios are in a guard file (`examples/perf/bench.rs`), which waits for the
  author ([ADR 0105](adr/0105-m2-24-part-2-the-flash-and-line-numbers.md)).
- The flash's colour is checked only by eye, as the control socket's screenshots do not include
  the flash ([ADR 0105](adr/0105-m2-24-part-2-the-flash-and-line-numbers.md)).
- An `oso dev` restart from a build whose handoff has visual aids without `line_numbers` is
  refused, as `parse_aids` wants every switch. That fails safe, but ADR 0097 promises
  the defaults only for a handoff with no visual aids at all. Decide whether a missing switch
  takes its default ([ADR 0097](adr/0097-visual-aids.md)).
- Small API tidy-ups in the gutter: `Gutter::rows` gives whether a row is the current line as
  a bare `bool`, `Editor::fit_gutter`'s `bool` result is used only by tests, and text-grid
  snapshots do not mark the current line's number
  ([ADR 0105](adr/0105-m2-24-part-2-the-flash-and-line-numbers.md)).

- Safe save loses a file's extended attributes other than the quarantine flag ([ADR 0067](adr/0067-access-lists-and-the-quarantine-flag-in-a-save.md)). On macOS that includes Finder tags. Decide whether copying them is worth it.
- A control socket's own file is not sealed as a screenshot is. It is a dev tool's, its peer is checked, and an inherited `deny delete` can leave a dead
  socket behind ([ADR 0065](adr/0065-access-lists-on-private-directories.md)).
- Later, before anyone uses oso on Windows: decide what a private directory and a save do about access lists there. It needs `windows-sys` as a direct
  dependency ([ADR 0065](adr/0065-access-lists-on-private-directories.md)).
- A buffer whose load could not tell where its file is still cannot be saved until `C-x C-w`
  (M2-18), and a file owned by another user, or with hard links, cannot be saved at all; decide
  whether a question should offer writing in place
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md), [ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md)).
- By M4, with the first real passphrase prompt: a passphrase's rope chunks are freed without being
  overwritten. Overwriting needs `zeroize` or a small `unsafe` outside `oso-core`
  ([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md)).
- By M2-14: kills from a passphrase prompt reach neither the kill ring nor the OS clipboard, and a
  yank into it works
  ([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md)).
- `C-x C-f`'s look at a row follows a symbolic link to say what it leads to, which can reach an
  automounted or slow file system and hold that listing thread until it answers. Decide whether a
  look stops at links that leave the listed tree
  ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- `file_prompt_type_p99_ms`, typing into `C-x C-f` over 100,000 names, is reported only until a few
  runs exist. Its first run on the Mac mini, on 2026-11-09, gave 4.1–7.1 ms, against 38.6–39.8 ms
  for the same scenario before the worker. Set its threshold from those runs
  ([ADR 0096](adr/0096-completion-on-a-worker-past-5000-candidates.md)).
- A segment of `C-x C-f` that matches several directories lists each, which can reach an
  automounted or cloud-synced folder the person did not name, and wait there. Decide whether
  expansion stops at such mounts.
- `C-x C-f` leaves out names that are not UTF-8, as a path made from a lossy copy of one would
  name another file. Keep the name as the system gives it if they turn out to matter.
- `C-x C-f` reads a path with `/` between its parts and starts an absolute one at `/`, so on
  Windows a path with a drive, such as `C:/Users`, is read as relative. Decide how a drive or a
  UNC share is typed once Windows draws text.
- `C-x C-f` matches file names code point by code point, so a name stored decomposed (NFD), as older
  macOS volumes and zip archives keep them, does not match the composed form an input method types.
  Normalizing needs `unicode-normalization`, a dependency; decide once it is seen to matter
  ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- The emergency save, small things to settle ([ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md)):
  - `write_copy` drops what `unless_unsupported` returns, so its report says nothing when a file system cannot confirm a sync, as a save's message does.
  - A panic that cannot unwind under a command aborts without the copies, as the hook takes the first panic under a command to be caught.
  - A caught panic does not wait for its printing thread, so these threads can pile up while stderr is blocked.
  - `write_copies_with` records what it wrote only if `try_lock` gives `Ok`, while `select` also takes a poisoned lock. At worst a copy is written twice.
- `saver::write_text` and `emergency::write`'s `write_text` are near copies, as are their checks that the whole text was kept. Join them if a third user arrives.
- By M2-14 and M2-18: `Verdict` gains a variant for a tainting yank, which judging a path again must not clear, M2-14 decides whether a kill from an `Unresolved`
  buffer taints, and `C-x C-w` must give an entry in the emergency state a new identity, as its path changes with no text change
  ([ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md); ADR 0087 decides the last for M2-18).
- Tier 2, before Linux or Windows are daily systems: a logout may not reach `exiting` there, so only a panic writes copies ([ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md)).
  winit on Windows does not handle the end of a session, and on Linux one usually comes as a signal, or a close request that the exit guard refuses.
  oso's own Windows layer brings `WM_ENDSESSION` to `exiting` ([ADR 0070](adr/0070-windowing-and-input-without-winit.md)); Linux's signal stays.
- By M2-22: the exit guard counts modified buffers and running saves, and cannot tell that an unmodified
  buffer's file was deleted, since that needs the disk. `C-x C-s` writes such a file again, but
  `C-x C-c` exits at once, and its text is then gone. The watcher in M2-22 can tell it
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)).
- `loader/resolve.rs` has two walkers along symbolic links: `Walk`, which keeps each target for the secret
  verdict, and `follow_links`, which a save uses. They differ on a link to nothing. Decide whether one can do both.
- By M2-18: two buffers visiting one file can each pass the check while the other writes.
  `C-x C-f` should find the buffer already visiting a file. And `C-x C-w` must decide what writing
  a secret buffer to a new path means over the socket, since `save-buffer` writes only where the
  file was, which is what keeps its text in the editor
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)). M2-18 closes both: one file opens in one
  buffer, found by device and inode, and `write-file` is the person's only, keeping a secret
  verdict wherever it writes ([ADR 0087](adr/0087-finding-files-writing-under-a-new-name-and-large-files.md)).
- By M2-19: a window that shows another buffer loses its marks and its mark ring, as it
  loses its cursors. Decide where a window keeps a buffer's places when `C-x b` comes back
  to it ([ADR 0071](adr/0071-mark-and-region.md)). M2-19 closes this: a window keeps a place
  per buffer ([ADR 0088](adr/0088-switching-and-killing-buffers.md)).
- `kill-buffer` must cancel a load at once (ADR 0022). Today the app drops
  a load's handle only when it next sees the buffer gone; once a command
  can kill a buffer, the app should reconcile its loads right after
  commands. It must also think about a running save: `saving` lives on the
  `Buffer`, so a killed buffer's save no longer counts for the exit guard, a
  guarded `exit` or the wait when leaving
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)). M2-19 closes this: a kill cancels
  the load right after the command, and refuses while a save runs
  ([ADR 0088](adr/0088-switching-and-killing-buffers.md)).

- Killing a large buffer (and a load that fails midway) frees its trees on
  the UI thread, which takes tens of milliseconds for a 1 GB file. Drop
  them on a background thread. M2-19 does this for a kill
  ([ADR 0088](adr/0088-switching-and-killing-buffers.md)); a failed load stays.
- Large deletions free part of their trees on the UI thread
  ([ADR 0053](adr/0053-edit-core.md)). Killing 89 MB of a 100 MB buffer takes about
  1.2 ms, nearly all of it freeing the old display map's items. Its text stays in the
  undo group until the history drops it, on the edit that passes the 64 MiB bound, which
  takes about 2.5 ms (one early run took 17.6 ms, not seen again). Undoing an 89 MB paste
  and then typing frees the redo group in the same way, in 2.4 ms. These scale with the
  text: about 25 ms per GB. Decide before M2-14, which brings kills, whether to send
  dropped groups to the background thread above.
- Recovering a large emergency copy ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)) is O(1) to stage, but what it replaces is freed on
  the UI thread, about 13 ms per GB for the display map, and at the first edit after it a file over 64 MiB frees its old rope and loses the
  undo. Undo and redo each read the whole text back, at about 0.7 ms per MB, with a second copy held for a moment, up to a copy just
  under 2 GiB. Keeping a whole-text state in the undo group would make crossing O(1). Decide with the next two items.
- Undoing a kill puts its text back at about 0.7 ms per MB (63 ms for 89 MB) on the UI
  thread, and for a moment holds several copies of it, since the slice is copied, split
  into chunks and measured again. Running out of memory aborts with no emergency save.
  Redoing takes 1.4 ms. Sharing the slice's chunks with the rope, and keeping display items
  with the undo group, would make it O(log n). Decide before M2-14. `rope.rs` is at its
  size limit, so sharing needs room there.
- Typing at 1000 cursors, which took 17 to 26 ms a key at p99, takes about 3 ms on 100 MB and
  3.3 to 4.7 ms on a 10 MB line of minified JSON since M2-21 edits a leaf in place and clips only
  what is near an edit, in every window (`text_perf -- cursors`,
  [ADR 0092](adr/0092-edits-in-place-for-many-cursors.md)). On a 10 MB line dense with CJK, emoji
  and combining marks it still takes 5.6 to 9.3 ms, over the edit's 4 ms share: each rebuilt
  chunk's summary walks its clusters through the grapheme segmenter. A faster walk over
  characters that cannot join their neighbours is the next step; the author keeps the bound at
  1000 meanwhile. `just perf` has no scenario of 1000 cursors yet, since its thresholds are guard
  files. A command still clones the selection set for its undo group, and an edit a few times
  more; a profile at 1000 cursors does not show it.
- After M2-25: deleting back leaves `left` where it is, as the fewest columns that show the
  cursor ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)), so a row can stay
  scrolled sideways with text hidden to its left. A wrapped buffer never scrolls sideways
  ([ADR 0100](adr/0100-soft-wrap.md)), so this is for unwrapped ones.
- What panes below the frame's top row do with the band above their first row, which they
  lose today: by M3. The modeline and the echo row keep it
  ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)), which `draw_pane` tells as
  a tiled pane of `Lines` with one row for now, and what a float shows keeps both bands in its
  padding, told by its layer ([ADR 0072](adr/0072-floating-windows-the-minibuffer-and-questions.md)).
- Identical messages are not merged in `*Messages*`, so holding a key that fails evicts its older rows in
  about 35 s. Emacs counts repeats on one row.
- With `*Messages*` at its 1000 rows, each new message moves what someone is reading there, as the
  view's top row is not lowered when the oldest row goes.
- The modeline says `UTF-8 LF` before anything of a file has loaded, when neither is known yet.
- `Status::echo_warnings` depends on a flag that could come from `Editor::frame`, which its callers have.
- `Status` is built twice for a key that redraws, in `draw_frame` and in `about_to_wait`
  ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).
- By M2-19: whether `C-x b` lists `*Messages*`, and whether `kill-buffer` may kill it, which today
  it leaves. `C-h e` going back and the `Place` behind it go then. How the modeline shows long names
  ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)). M2-19 decides the first three:
  `C-x b` lists it, nobody kills it, and `C-h e` goes back through the window's places
  ([ADR 0088](adr/0088-switching-and-killing-buffers.md)); long names stay cut, as ADR 0063 says.
- `S-RET`, `S-DEL` and other shifted named keys with no binding do nothing. Emacs falls back to
  the unshifted key, so holding Shift a moment too long still types. Decide whether
  `KeyResolver` does the same, or record that oso differs.

- `Editor::set_frame_size` pulls the view back when the selected window gets taller:
  - The limit should be a pure function beside `window::wheel`, used by both.
  - By M3: "taller" compares the tiled window alone, also beneath a prompt. With splits
    ([ADR 0112](adr/0112-window-splits.md)) it compares each window with its own height.
  - Narrowing and widening do not do the same for `left`. With the cursor far to the right,
    a window made very narrow and wide again stays scrolled sideways, with shorter rows blank.
    Decide whether growing sideways follows the same rule.
- Clipboard on Linux and Windows: their backends come with ADR 0070's stage 4, as
  [ADR 0074](adr/0074-the-kill-ring-and-the-clipboard.md) decides (`wl_data_device` on the native
  Wayland window, Win32 through `windows-sys`).
- An Option key pressed during a composition ends it with no commit, so what was being
  composed is lost, and the Super command does not run. With winit oso cannot commit
  first; a native view sees the key before the input method and can. ADR 0070 commits
  the composition, then runs the key ([ADR 0069](adr/0069-macos-input-method.md),
  [ADR 0070](adr/0070-windowing-and-input-without-winit.md)).
- The candidate window stays where a composition started, as winit gives the OS one
  rectangle. Check whether a new area during a conversion moves it at the next key, so
  that it can follow the segment ([ADR 0069](adr/0069-macos-input-method.md)).
- The app still gives the window the cursor's cell on every key that moves the cursor (about
  0.1 ms a key at p99 in `just perf` at M2-12), though oso's own layer only keeps it until
  the input method asks. Measure whether giving it only when a composition may start, or
  when the input method asks, is worth it
  ([ADR 0070](adr/0070-windowing-and-input-without-winit.md),
  [ADR 0077](adr/0077-choices-for-the-native-layers-stage-2.md)).
- `oso_platform::window::Held` names the modifiers as the window system does, beside
  `oso_core::key::Modifiers`, and `keys.rs` maps one to the other, as `oso-platform` cannot
  name `oso-core`'s. That only `app/shell.rs` names winit is said in the tip crate's
  `AGENTS.md` and not checked by the guard tests
  ([ADR 0070](adr/0070-windowing-and-input-without-winit.md)).
- Reconversion: a replacement range outside the marked text is ignored for now, and the text
  goes in at the cursor, though the input method now reads the cursor's line. Decide whether
  to apply one inside the excerpt last given, as ADR 0070 allows
  ([ADR 0077](adr/0077-choices-for-the-native-layers-stage-2.md)).
- On the "Dvorak – QWERTY ⌘" layout, Meta follows Dvorak on oso's own macOS layer: the key
  in the QWERTY `x` position gives `M-q`, where macOS's Command layer would give `M-x`, as a
  printable ASCII reading with Shift alone wins over the Command layer. Decide whether Command
  should follow that layer's QWERTY there ([ADR 0033](adr/0033-key-presses-and-notation.md),
  [ADR 0070](adr/0070-windowing-and-input-without-winit.md)).
- The cursor's line for the input method and typing in one long line: on the native layer,
  typing and deleting in a 10 MB line were about 0.3 ms slower per key once the excerpt was
  given, in every run on a second Mac on 2026-11-09, far under the 5 ms gate. Cutting the
  excerpt there takes about 3 µs, with or without an edit before it, handing it to the window
  is a swap, and motion keys, which give a new excerpt too, did not move. Find where the time
  goes with the same build run against itself with no excerpt given
  ([ADR 0077](adr/0077-choices-for-the-native-layers-stage-2.md)).
- Before ADR 0070's stage 4: show in CI that lavapipe presents under a headless Wayland
  compositor, as Linux CI moves off Xvfb
  ([ADR 0070](adr/0070-windowing-and-input-without-winit.md)).
- By M3: menus add their condition to `wants_ime` in `app/ime.rs`, as prompts and isearch
  have ([ADR 0069](adr/0069-macos-input-method.md)).
- Glyphs under the cursor box take `cursor_color` for their whole quad, including
  pixels outside the box, so ink above or below the row or in a ligature's other
  cells is drawn in the background color. Split the quad at the box
  ([ADR 0052](adr/0052-glyphs-stay-in-their-cells.md)).
- Following the OS's dark or light appearance, as an opt-in entry of `load-theme` that picks
  the pair's dark or light theme. It needs appearance events on both window layers
  ([ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md)).
- Remove `Editor::set_view_size` and `FrameSize::holding` from the public API once
  M2-9 has merged. Production uses only `set_frame_size`, and the rest are test helpers
  ([ADR 0057](adr/0057-frame-layout.md)).
- Any input-method report, even an empty preedit or `Disabled` with no composition open,
  counts as the person typing and restarts the 500 ms wait before a question the agent caused
  takes an answer. It could count only text or the end of a composition that was open. Kept
  for now, as the cautious side ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)).
- The rule that a channel save of a buffer the agent opened asks the person lives in
  `save-buffer`, the only writer the channel reaches. Move it into `Editor::start_save`, as a
  `SaveRefused` the person's `y` gets past, once the channel reaches a second writer, such as a
  `save-some-buffers` ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)).

## M3

- With soft wrap: a fill column to wrap at, narrower than the window, through M3's
  configuration; and scripts that need more than spaces and cluster breaks, such as Thai, which
  wrap at clusters until then ([ADR 0100](adr/0100-soft-wrap.md)).
- A `just perf` scenario with soft wrap on, whose thresholds are guard files and wait for the
  author ([ADR 0100](adr/0100-soft-wrap.md)).
- With soft wrap, a wrapped buffer that loads in many parts, as one carried by `oso dev` or a
  large file turned on before its load ends, is wrapped in full only after its last part, since
  each part starts the wrap over. Following the parts as appends needs the range the rope
  rebuilt, which a load's snapshots do not carry ([ADR 0103](adr/0103-soft-wrap-in-a-window.md)).
- With soft wrap, the input method's rectangle for a preedit that wraps: each character's row,
  so that a candidate window for a segment on a later row sits beside it. Until then the input
  method gets the cursor's cell, and characters' x kept in the text area
  ([ADR 0104](adr/0104-soft-wrap-drawn-in-view-rows.md)).
- A whole wrap is freed on the UI thread when a window drops it or a report replaces it, about
  0.7 ms for 100 MB of prose at 40 columns and 5.5 ms the first time. Hand old trees to a
  thread, as a reload's leftovers are ([ADR 0103](adr/0103-soft-wrap-in-a-window.md)).
- The visual aids' lasting settings: which are on, per buffer or per mode as Emacs's minor
  modes are, and keys or the toggle menu to reach them. Until configuration, they toggle for
  every buffer by `M-x` (`global-…` names, leaving the buffer-local ones for then) and start
  from the configuration's `[aids]`
  ([ADR 0097](adr/0097-visual-aids.md), [ADR 0108](adr/0108-the-user-configuration-file.md)).

- After query-replace, keyboard macros and whitespace cleanup: regexp isearch (`C-M-s`) on
  query-replace's per-line matcher; the kmacro ring, counters, naming and editing; `E`, `C-r`, `Y`
  and `N` in query-replace; whitespace settings per mode once modes exist; and whether to read
  `.editorconfig` ([ADR 0113](adr/0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)).

- Whether users can define their own themes, or override a built-in one's palette entries
  and mappings, and in what format. Themes are data to keep it possible. Configuration
  names the default theme and `<f5>`'s pair
  ([ADR 0089](adr/0089-built-in-themes-switched-at-run-time.md)).

- Which keymaps the configuration reaches after `global`: `minibuffer`, `completion`, `isearch`
  and the major modes' are reserved names, and per-mode settings wait for modes
  ([ADR 0108](adr/0108-the-user-configuration-file.md)).

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
- The count of bidi controls lives in the TabMap's summary. When the FoldMap or InlayMap goes
  below the TabMap, move it to the rope's, which is at its size limit, so that hidden and virtual
  text do not change the warning
  ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)).
- The invalid-byte table lives in the TabMap and is keyed by buffer
  offsets. When the FoldMap or InlayMap goes below the TabMap, move it to
  the lowest layer that still sees buffer offsets, or to `DisplayMap`.
  Its API (`DisplayMap::invalid_bytes`) does not change.
- Initial set of tree-sitter grammars, beyond Rust, Markdown and TOML.
- After window splits ([ADR 0112](adr/0112-window-splits.md)):
  - `C-x 4`: `switch-to-buffer-other-window`, `find-file-other-window` and
    `kill-buffer-and-window`, with rules for where a buffer is shown, as Emacs's
    `display-buffer`;
  - undoing a change of layout, as Emacs's `winner-mode`;
  - repeating `C-x o` and the resize keys with their last key alone, as Emacs's `repeat-mode`;
  - clicks that select a window and drags on dividers and modelines, with mouse focus;
  - one shaped-row cache per buffer shared by its windows, if measuring a buffer shown twice
    says it is needed.
- Fuzzing targets and how long they run. The `SumTree`, the rope, the
  display model under random edits, and the streaming decoder and load
  builder (property-tested since M1-6) are the first candidates.
- URL-opening policy: which schemes, and what confirmation.

## M4

- Claude Code hooks: which events map to which states, and how
  `oso integration install claude` merges with existing user settings, and registers `oso mcp`
  ([ADR 0031](adr/0031-claude-code-integration.md),
  [ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)).
- Which review and worktree tools join `oso mcp`, and the reach of each
  ([ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)).
- Whether to add native permission dialogs (PermissionRequest hook), a
  read-only transcript viewer, or `claude -p` one-shot features later.
- OSC 7501 is a draft; track its revisions.
- Terminal engine: compare `alacritty_terminal` (ADR 0015) and libghostty-vt
  1.0 when M4's terminal work starts, by building with both, and decide in
  an ADR. The bar is parity with the author's current Emacs terminal
  (ghostel, built on libghostty-vt):
  - inline images (Kitty graphics protocol)
  - shell integration: directory tracking (OSC 7), prompt navigation
    (OSC 133), calling whitelisted editor commands from the shell
  - input modes (semi-char, char, line, read-only, copy)
  - the terminal as the backend for compile, async shell commands and
    comint-style buffers

  `alacritty_terminal` lacks OSC 7, OSC 133, OSC 7501 and Kitty graphics.
  It also gives the embedder no way to observe escape sequences it does not
  handle, so OSC 7501 for ADR 0031 would need a second parser over the same
  bytes. Kitty graphics would have to be implemented by oso. libghostty-vt
  has all of these, but needs Zig to build on three OSes, has no official
  Rust binding, and parses untrusted output in code that is not memory safe.
  Also compare rendering of the Claude Code TUI and bulk-output performance.
  Keep the engine behind a trait.
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

- Dev control socket and agent channel on Windows: a named pipe restricted to the current
  user ([ADR 0032](adr/0032-control-socket-transport.md),
  [ADR 0085](adr/0085-agent-channel-and-oso-mcp.md)). Needed when agents test or drive oso on
  Windows.
- Linux text-rendering backend: cosmic-text or FreeType + HarfBuzz. Until
  then Linux (and Windows, until DirectWrite) draws no text
  ([ADR 0036](adr/0036-text-system-and-glyph-atlas.md)). Each must keep cells in logical
  order and show no mirrored form
  ([ADR 0066](adr/0066-text-stays-in-logical-order.md)).
- A right-to-left file in the `just perf` inputs, so its rows are measured
  ([ADR 0066](adr/0066-text-stays-in-logical-order.md)).
- Exact command subsets for the directory editor and org.
- PDF engine and isolation (pdfium, a pure-Rust renderer, a separate
  process); image decoders.
- OS-level sandboxing of child processes such as language servers.
- `subsecond` hot-patching proof of concept with a plain winit app, or with oso's own
  window layer once winit is gone
  ([ADR 0070](adr/0070-windowing-and-input-without-winit.md)).
- Generated command reference and the Japanese edition workflow.
- Encodings other than UTF-8 (Shift_JIS, EUC-JP), when they are needed:
  detection, and conversion at the load and save boundary
  ([ADR 0022](adr/0022-text-buffer-and-file-loading.md)).
