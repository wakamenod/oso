# 0088. Switching and killing buffers

- Status: Accepted, amended by 0100
- Date: 2026-11-08

## Context

M2-19 builds `C-x b` with a preview inside the popup, `kill-buffer`, distinct names for files with
one name, and `M-,`/`M-/` ([roadmap](../roadmap.md)). Until now a window changes buffer only when
a file opens, through `C-h e` and `C-h r`, or through the dev socket's `show-buffer`.

What is fixed already:

- [ADR 0030](0030-m2-editing-commands.md): `M-,` and `M-/` go to the previous and the next buffer.
- [ADR 0051](0051-m2-plan-decisions.md): two files with one name are named `notes.md<work>`, as
  Emacs's uniquify does by default.
- [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md): `*Messages*` is an internal buffer
  that `kill_buffer` leaves, `C-h e` goes back to where the window was through a `Place`, and
  M2-19 decides whether `C-x b` lists `*Messages*` and whether a person may kill it.
- [ADR 0071](0071-mark-and-region.md): a window that shows another buffer loses its marks and its
  mark ring, as it loses its cursors, and `C-.` (`consult-mark`) waits for completion.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md): a command that changes which
  buffer a window shows refuses inside a prompt.
- [ADR 0076](0076-completion-and-m-x.md): the centred float, 62% of the columns and 19
  rows, the matcher, the keys, and `C-.` with M2-19, whose preview it needs.
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md): killing a shown buffer shows
  `*scratch*`, created again if needed.
- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md): the dev method `show-buffer` goes
  once `switch-to-buffer` exists.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): every command has a reach, and the channel runs
  nothing while a buffer that may hold secrets is shown.
- `ui.md`: a command with a preview splits the popup, candidates on the left and the preview of the
  selected one on the right; the preview is read-only and renders only buffers already open.
- `open-questions.md`: `kill-buffer` must cancel a load at once, must think about a running save,
  and killing a large buffer frees its trees on the UI thread.

## Decision

The author accepted this ADR on 2026-11-09, through the coordinator.

The author chose these on 2026-11-08, through the coordinator: the first option of each question
under [Questions](#questions), which the text below describes. The new public items listed under
[Where it lives](#where-it-lives) are named in the pull request for the author's approval.

### `C-x b`

- **`switch-to-buffer` on `C-x b`** reads a buffer in a prompt that completes, in ADR 0076's
  centred float, labelled `Switch to buffer:`, with the history `buffer`. The new
  `ArgSpec::Buffer { prompt, history, purpose }` asks for it, `purpose` being `Switch` or `Kill`.
- **The list:** every buffer of `Editor::buffers`, and the internal buffers a person reads:
  `*Messages*`, `*Emergency Copies*` and `*Agent*`. A prompt's buffer and a copy being looked at
  are not listed.
- **The order is recency,** as consult-buffer's: the buffer the window showed most recently
  first, and the current buffer last. So `C-x b RET` on an empty input goes back to the previous
  buffer. ADR 0076's history, length and alphabetical order does not apply; the exact match of the
  input still goes first. Recent files are not listed until a history is kept on disk.
- **A row:** the name, as the echo area draws it (ADR 0063), then, as marginalia's annotation for
  a buffer: `*` when modified, the size (`12k`), the mode (`Text`), and the file's path with the
  home directory as `~`, or nothing for a buffer with no file. A buffer still loading shows its
  progress for the size.
- **`RET` with no match creates a buffer** with that name and no file, editable, as Emacs does.
  `M-RET` gives the name as typed whatever matches: an existing buffer of that name is shown, and
  any other name is created. An empty name creates nothing. `C-p` from the first candidate selects
  the typed text, as in `C-x C-f` (ADR 0087).
- `C-x b` refuses inside a prompt, as before (ADR 0072).

### The preview

```
        ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
        ┃ 1/4 Switch to buffer: █                                    ┃
        ┃▌todo.md  *  2k  Te…▐┃# Todo                                 ┃
        ┃ notes.md    9k  Te… ┃- budget for Q3                        ┃
        ┃ *Messages*  1k  Me… ┃▯ hiring: two engineers                ┃
        ┃                     ┃                                       ┃
        ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

- **Inside the float, which keeps its size:** 62% of the columns and the answer's row with 18 rows
  (ADR 0076). The answer's row spans the float. Below it, the list takes the left 2/5 of the inner
  columns, a column in the border's colour divides it, and the preview takes the rest. The list's
  annotations are cut first, as ADR 0076 says for a narrow row.
- **What it shows:** the selected buffer's rows, from the top row its window last had for it (the
  places below), or its start, at column 0, cut at the preview's edge. Its cursors are drawn hollow
  at their places; no region and no modeline. It is read-only: keys stay with the prompt.
- **It follows the selection at once,** with no delay, since every buffer is in memory. The
  preview is a pane of its own whose id goes with the previewed buffer, so its shaped rows are
  reused while that buffer stays selected, and moving to another reshapes at most 18 rows of the
  preview's width, within the 8 ms budget.
- **A buffer that may hold secrets is never previewed,** nor one whose verdict is not known yet:
  the preview says `may hold secrets` in the annotation's face, and draws nothing of the buffer. So
  moving through the list never puts such a buffer on the screen, and nothing of ADR 0044, 0063
  and 0079's withholding turns on for it.
- **A narrow frame gives up the preview:** when the list would get fewer than 20 columns, the list
  takes the whole width. A short frame gives up rows as ADR 0076 says.
- `C-x C-f` (ADR 0087) and `M-x` have no preview.

### Where a window was in each buffer

- **A window keeps a place for each buffer it showed:** the selection set with every cursor and its
  goal, the marks, the mark ring, the top row and the left column. Leaving a buffer saves its place,
  and coming back restores it, clipped to cluster boundaries of the text as it is then (a reload or
  an edit may have changed it), with the region hidden.
- **A buffer the window never showed** starts where any window last left it (one window until M3),
  or at its start.
- **The window's places are in the order it showed the buffers,** the most recent first. That order
  is `C-x b`'s and `M-,`'s. A killed buffer's place goes.
- **`C-h e` and `C-h r` go back as before,** through the internal buffers the window went through,
  to the buffer it came from: ADR 0063's way back keeps only which buffers, and the window's places
  say where it was in each, so its own `Place` goes. A window on `*Messages*` hands over, at a
  restart, the buffer it came from and its place, as ADR 0063 says.
- Places stay in memory and are not carried by a restart, beyond what ADR 0063 hands over. Positions
  in a buffer that may hold secrets are withheld from `get-state` as before (ADR 0058).

### Distinct names

- **Two buffers visiting files with one name** are named `name<dir>`, with the shortest suffix of
  the directories that tells them apart, as Emacs's `post-forward-angle-brackets`:
  `notes.md<work>` and `notes.md<home>`, or `notes.md<a/work>` and `notes.md<b/work>`.
- **Names are worked out again** whenever a buffer opens, is killed, or is written under a new
  name (ADR 0087), so the last `notes.md` left takes its plain name back.
- **From the path as visited,** made absolute without touching the file system, never from where
  links lead.
- **A file whose name is a buffer's with no file,** such as a file named `*scratch*`, takes its
  directory in the same way. A buffer created by `C-x b` never shares a name, since that name
  shows the existing buffer.
- The modeline cuts a long name, as ADR 0063 says.

### `kill-buffer`

- **`kill-buffer` on `C-x k`** reads a buffer in the same centred float, labelled `Kill buffer:`,
  with no preview. The current buffer comes first, then the rest in recency, so `C-x k RET` kills
  the current buffer. Only a listed buffer is accepted: with no match `RET` says `[No match]`.
- **A modified buffer asks** `notes.md is modified; kill it anyway? (y or n)`, its name cut to 40
  cells. `y` runs `kill-buffer-anyway`, which kills it if it is still that buffer and no save
  started, and asks again otherwise.
- **A save running refuses,** `notes.md is being saved; kill it when that ends`, as a killed
  buffer's save would no longer count for the exit guard (ADR 0059).
- **A load is cancelled at once:** the app drops its handle right after the command, not when it
  next sees the buffer gone.
- **Internal buffers are kept:** `*Messages*`, `*Emergency Copies*` and `*Agent*` say
  `*Messages* is kept` and stay. `*scratch*` can be killed, and is created again when a window has
  nothing else to show.
- **After a kill,** each window that showed the buffer shows the first buffer in its order, the most
  recent other one, or `*scratch*` when there is none, as Emacs does. A buffer from `oso FILE` that
  is killed leaves `*scratch*` too.
- **What it held is freed on a background thread,** as a save frees its snapshot, so killing a
  large buffer costs the UI thread no more than a small one. A killed buffer's emergency entry goes,
  and an emergency copy of its file becomes `not open` (ADR 0064).

### `M-,` and `M-/`

- **`previous-buffer` on `M-,`** shows the buffer before the current one in the window's order, and
  repeating it goes further back, through every buffer: those the window never showed come last, in
  the order they opened. **`next-buffer` on `M-/`** goes the other way, so `M-/` after `M-,` comes
  back. Both land at the buffer's place, as `C-x b` does.
- **Internal buffers are skipped:** `C-h e` and `C-h r` reach them.
- A count goes that many buffers, negative the other way. With one buffer, they say
  `No other buffer`. Inside a prompt they refuse, as `C-x b` does.

### `C-.`

- **`consult-mark` on `C-.`** (ADR 0071, ADR 0076) lists the marks of the primary cursor's mark ring
  in the same centred float, labelled `Go to mark:`, newest first, with no history. A row is the
  mark's line number and that line's text, cut at the list's edge, and the preview shows the
  buffer around the selected mark, with that mark drawn as a hollow cursor.
- **`RET`** leaves one cursor at the mark, with the region hidden, and puts where the cursor was
  in the ring, as `C-u C-SPC` does.
- It previews the buffer the tiled window shows, but never one that may hold secrets, as no
  preview does; its rows are withheld from `get-state` as every prompt beside such a buffer is
  (ADR 0072, ADR 0076). It refuses inside a prompt, whose own buffer is no list of marks.

### Reach and the sockets

- **`switch-to-buffer`, `previous-buffer` and `next-buffer` are `Look`** (ADR 0085). Over the
  channel, `switch-to-buffer` takes a buffer's exact name as its `Text` argument and never creates
  one. **All three refuse from the channel when the buffer they would show may hold secrets**, "it
  would show a buffer that may hold secrets; ask the person", since the channel would then wait
  until the person moved on, and the person would land on it unasked.
- **`kill-buffer`, `kill-buffer-anyway` and `consult-mark` are `Person`.** A kill can lose unsaved
  text; `consult-mark` picks in a list, and `C-u C-SPC` reaches the ring from the channel.
- **`get-state`** gives these prompts the kind `complete` and ADR 0076's `completion`, whose `rows`
  are the buffers' names, and `preview`, the id of the buffer previewed, or `null` when none is or
  the preview withholds it. The preview's text is never in it; `get-text` reads a buffer by id. All
  of it is withheld while a buffer that may hold secrets is shown. The channel's `state` gives a
  prompt's kind only (ADR 0085).
- **`run-command switch-to-buffer`** on the dev socket takes the name as its `arg`. The dev method
  `show-buffer` goes, as ADR 0044 said, and the tests and flows use `switch-to-buffer`.

### Where it lives

- **`oso-core`:** `ArgSpec::Buffer` and `BufferPurpose`; a window's places and their order in
  `window/places.rs` (`window::Place`, `Window::places`), which `editor/messages.rs`'s `Place`
  gives way to; distinct names as a pure
  function in `buffer/names.rs`; `Editor`'s switching, killing, previous and next buffer in
  `editor/switch.rs`; the preview's cells in `frame::completion_regions` (`FloatRegions::preview`).
- **`oso`:** the commands in `commands/buffers.rs`, their keys in `bindings`, the candidates in
  `dispatch/buffers.rs`, the preview's pane in `app/draw/float.rs`, cancelling a load and freeing a
  killed buffer in `app/loads.rs`, and the socket's field in `dev`.
- No new crate, crate edge, dependency, public module or public trait.

### Tests and measures

- Distinct names: property tests that the names are unique and the shortest that tell the paths
  apart, and that killing one gives the other its plain name back.
- Places: switching away and back restores every cursor, the mark ring and the scroll; a place
  clipped after the text changed; a killed buffer's place gone.
- `kill-buffer`: modified (the question, `y` after a save started), saving, loading, internal,
  `*scratch*`, the buffer shown afterwards.
- `M-,`/`M-/`: the order, a count, skipping internal buffers, `M-/` after `M-,`.
- Text-grid snapshots of `C-x b`: the split, a narrow frame without the preview, a buffer that may
  hold secrets selected, and `C-.`.
- A flow (`just e2e`): `C-x b`, `C-n`, the preview following, `RET`, `get-state` on the way;
  `C-x k` of a modified buffer answered `y`; `M-,` and `M-/`; the channel refusing to switch to a
  buffer that may hold secrets.
- `just perf` reports keys moving the selection in `C-x b` with 100 buffers open, reported only.

## Questions

Put to the author through the coordinator, the recommended option first. The author chose (a) on
each.

1. **What `C-x b` lists:** (a) buffers, the most recent first and the current last, with
   `*Messages*`, `*Emergency Copies*` and `*Agent*`, and no recent files yet; (b) the same without
   internal buffers; (c) ADR 0076's order.
2. **A name that matches nothing:** (a) `RET` creates the buffer, and `M-RET` also when something
   matches; (b) `[No match]`.
3. **A buffer's annotation:** (a) modified, size, mode and path; (b) the path only.
4. **The preview:** (a) inside the 62% float, the list on the left 2/5 and the preview on the right
   3/5, from where its window last was, at once; (b) a float 80% wide while it previews; (c) in the
   window beneath, as consult does.
5. **A buffer that may hold secrets:** (a) never previewed, `may hold secrets` in its place; (b)
   previewed, with all the withholding of a shown one.
6. **Places:** (a) every cursor, the marks, the mark ring and the scroll, per window and buffer;
   (b) the primary cursor and the top row; (c) the start, as today.
7. **`kill-buffer`:** `C-x k` (a) reads a buffer, the current first, (b) kills the current one;
   modified (a) asks, (b) refuses; internal buffers (a) kept, (b) killed and made again.
8. **`M-,` and `M-/`:** (a) the window's order, skipping internal buffers; (b) the same with them;
   (c) `C-x b`'s order.
9. **Reach and `get-state`:** (a) switching `Look`, refused from the channel toward a buffer that
   may hold secrets, killing `Person`; `get-state`'s `preview` an id only; (b) every new command
   `Person`.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. When this ADR is
accepted, the status of ADR 0040, ADR 0044, ADR 0063 and ADR 0071 gains `amended by 0088`.

- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md), "killing a shown buffer shows
  `*scratch*`, created again if needed": it shows the window's most recent other buffer, and
  `*scratch*` only when there is none.
- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md), "`show-buffer` ... goes away once
  `switch-to-buffer` exists": it goes now, and `run-command switch-to-buffer` stands for it.
- [ADR 0063](0063-modeline-echo-area-and-internal-buffers.md), "`kill_buffer` leaves it. M2-19's
  `kill-buffer` decides whether a person may": a person may not; `*Emergency Copies*` and `*Agent*`
  are kept the same way.
- ADR 0063, "the same key goes back to where the window was" through a `Place`: the way back keeps
  only the buffers, and the window's places, which replace the `Place`, say where it was.
- [ADR 0071](0071-mark-and-region.md), "a window that shows another buffer loses its marks and its
  mark ring, as it loses its cursors" (recorded in `open-questions.md`): the window keeps them with
  the buffer's place, and coming back restores them.

## Consequences

- Buffers are switched, killed and cycled from the keyboard, and each comes back where it was.
- Moving through `C-x b` shows what each buffer holds, but never a buffer that may hold secrets.
- `C-x b` is 2/5 of the float for names, so long names and their annotations are cut sooner than
  in `M-x`.
- A window keeps a place for every buffer it showed, a selection set and a mark ring each, which
  is small next to the buffers.
- `C-h e` and `C-h r` keep their way back, and take where the window was from its places.
- An agent can switch to any buffer by name, but not to one that may hold secrets, and cannot kill
  one.
- Killing a buffer of any size costs the UI thread little, and a load it had stops at once.
- `oso-core`'s public API gains `ArgSpec::Buffer`, `BufferPurpose`, `FloatRegions::preview`,
  `window::Place`, `Window::places`, `KillRefused`, and `Editor`'s switching, killing and walking
  methods (`walk_buffers`, `kill_by_person`, `kill_asked`, `take_freed`);
  `Editor::place_behind_internal` reads its place from the window's places.

## Alternatives considered

- **The preview in the window beneath, as consult does:** the real view, but the centred float
  covers most of it, and moving through the list would put each buffer on the screen, a buffer that
  may hold secrets with it.
- **A wider float while it previews:** more room for both, but the float's size would change between
  `M-x` and `C-x b`.
- **Previewing a buffer that may hold secrets, withholding as if shown:** complete, but stepping
  past it in a list would stop the agent channel and quiet the log.
- **ADR 0076's order for buffers:** stable, but the previous buffer would not be one `RET` away.
- **Recent files in `C-x b`:** consult-buffer's, but it needs a history on disk, which nothing keeps
  yet ([ADR 0013](0013-persistence.md)).
- **`C-x k` killing the current buffer at once:** one key less, but Emacs's `kill-buffer` reads one.
- **Refusing to kill a modified buffer:** safe, but saving or undoing first is more work for an
  answer the person can give with `y`.
- **Killing `*Messages*` and making it again empty:** the messages are their only copy.
- **Only the primary cursor and the top row as a place:** simpler, but several cursors and the mark
  ring would be lost by a look at another buffer.
- **`notes.md<2>`:** short, but it does not say which file it is (ADR 0051).
- **Every new command `Person`:** simplest, but an agent asked to show a buffer could not.
