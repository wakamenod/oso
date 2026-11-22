# 0171. One buffer order for the editor

- Status: Accepted, amended by 0170
- Date: 2026-11-22

## Context

[ADR 0088](0088-switching-and-killing-buffers.md) ordered `C-x b` by the tiled window's own
places, then the other buffers in the order they opened, the current buffer last. With split
windows (ADR 0112) that is not what consult-buffer does, which the author uses on `C-x b` with
vertico:

- a buffer used in another window does not rise;
- a buffer another window shows is not put after those no window shows.

consult-buffer sorts Emacs's `buffer-list` with `consult--buffer-sort-visibility`. First come the
buffers no window shows, in `buffer-list`'s order. Then come those a window shows. The current
buffer comes last. `buffer-list` is one order for the editor, and these rules move it
(Emacs 32's `window.c`, `buffer.c`, `window.el` and `desktop.el`):

- `select-window` without `norecord` moves its window's buffer to the front (`record_buffer`).
  That covers `switch-to-buffer`, `pop-to-buffer`, `find-file`, `other-window` and a click.
- These do not move it: `display-buffer` into a window that is not selected,
  `previous-buffer` and `next-buffer`, the buffer `quit-window` or a kill puts back in a window,
  and consult's preview (`norecord`).
- When a prompt closes, Emacs puts back its window configuration with `select-window`, so the
  selected window's buffer rises again. The buffer an `M-,` walk stopped on therefore rises at
  the next `C-x b`. The minibuffer's own buffer rises too, but its name starts with a space and
  no list shows it.
- A new buffer joins `buffer-list` at the end (`get-buffer-create`).
- `quit-window` buries the buffer it leaves to the end of `buffer-list`. It then shows the window's
  `prev-buffers`, and after them `buffer-list` (`switch-to-prev-buffer` with `bury`). A kill puts
  back a buffer the same way, and makes `*scratch*` only when there is nothing else.
- `previous-buffer` goes through the window's `prev-buffers`, then through `buffer-list` reversed,
  so that `next-buffer` goes through it the other way.
- desktop saves the buffers in `buffer-list`'s order and restores that order.

The author's Emacs binds `M-,` to `previous-buffer`, `M-/` to `next-buffer` and `C-x k` to plain
`kill-buffer`. Vertico sorts `kill-buffer` by the prompt's history, which oso does not keep for
buffers.

## Decision

The author chose these on 2026-11-22, through the coordinator: Q1 (a), Q2 (a), Q3 (b), Q4 (a)
and Q5 (a), under [Questions](#questions). The author approved the public items under
[Where it lives](#where-it-lives).

- **One order of the buffers for the editor,** the most recently used first, as `buffer-list`.
  A buffer joins it at the end when it is made, and leaves it when it is killed.
- **A buffer rises** when the selected window comes to show it, and when a window that shows it
  is selected (`C-x o`, a click, a switch of spaces, the agent channel's selection). The buffer the
  selected window leaves rises first, as Emacs records it again when a prompt closes.
- **Nothing rises** for these:
  - a window that is not selected (a restart's windows, a review's panes beside the selected
    one);
  - a step of `M-,` or `M-/`;
  - the buffer `q` or a kill puts back in a window;
  - the list of emergency copies shown in place of a copy that goes;
  - `C-x b`'s preview, which shows no buffer in a window;
  - an agent's command whose selection or show is put back because it would give the person's
    keys to a terminal or a list (ADR 0120, ADR 0121): the order is as before the command, so
    an agent cannot choose which of a worktree's sessions counts as the one used last.
- **`C-x b` lists by visibility,** as consult-buffer does:
  - the buffers no window shows, the most recent first;
  - then those another window shows, the most recent first;
  - then the current buffer, so `RET` still goes back to the buffer before;
  - after the buffers, the recent files, as ADR 0148 has them.
- **`C-x k` keeps ADR 0088's order** (Q3 (b)): the current buffer, then those its window showed
  before, the most recent first. The rest follow in the editor's order, no longer in the order
  they opened.
- **`q` (`quit-window`) buries** (Q2 (a)). It shows the buffer the window showed before, or with
  none the most recent other buffer a person goes to. The buffer it leaves goes to the end of the
  order, so `C-x b` offers it last of those no window shows.
- **After a kill,** each window that showed the buffer shows the first of the user's buffers it
  showed before. With none, it shows the most recent of the user's other buffers that
  `*scratch*` could stand for, and only then `*scratch*`, created again if needed. A buffer the
  window never showed is not put there when it is a terminal, whose program the person's next
  keys would reach (ADR 0120), a file the agent channel is still opening (ADR 0087), or one
  that may hold secrets or is not judged yet (ADR 0044). A space's window put back after its
  buffer was killed while it was out falls back the same way.
- **`M-,` and `M-/`** still go through the window's own places first. Then come the buffers it
  never showed, the least recently used first, so that `M-/` from the start reaches the most
  recent first, as `previous-buffer` and `next-buffer` go through `buffer-list`.
- **Other lists by recency** take the editor's order:
  - which hosted session runs in a worktree, the one selected, or shown in the selected window,
    last first (the "shown last" of ADR 0122 and ADR 0127);
  - the buffer the agent channel puts in front when no window has the ordinary keys.
- **A restart carries the order** (Q4 (a)), as desktop does. The state gains `order`, the ids of
  the buffers it hands over, the most recent first. The new instance applies it after the
  windows are shown and selected. A state from before it has no `order`, so its buffers keep the
  order they came in. The version stays, since an older reader passes the field over. A list
  longer than the state's buffers, or an id that is not a number, refuses the state; an id no
  buffer has, or one named twice, is passed over. A review the restart opens again later, in
  the selected window, rises above the carried order, as any show there does.
- Each window's places (cursors, marks and view per buffer) stay as ADR 0088 and ADR 0112 have
  them. So do the preview, which reads the tiled window's places, and the way back of `C-h e`.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. The statuses of ADRs
0088, 0090, 0112 and 0134 gain `0171`.

- [ADR 0088](0088-switching-and-killing-buffers.md), "**The order is recency,** as
  consult-buffer's: the buffer the window showed most recently first, and the current buffer
  last": the editor's order, by visibility, as above.
- ADR 0088, "The window's places are in the order it showed the buffers … That order is
  `C-x b`'s and `M-,`'s": it is `M-,`'s and `C-x k`'s; `C-x b` takes the editor's order.
- ADR 0088, `kill-buffer`, "then the rest in recency": the window's places, then the editor's
  order.
- ADR 0088, "**After a kill,** each window that showed the buffer shows the first buffer in its
  order, the most recent other one, or `*scratch*` when there is none": the window's places, then
  the editor's order, then `*scratch*`.
- ADR 0088, `M-,`: "those the window never showed come last, in the order they opened": the least
  recently used first.
- ADR 0088, "Places stay in memory and are not carried by a restart": they still are not; the
  order of the buffers is.
- [ADR 0112](0112-window-splits.md), "**Killing a buffer** sends each window that showed it back
  through its own places, as today": through its places, then the editor's order.
- [ADR 0090](0090-oso-dev-carries-unsaved-text.md), "**For the editor:** the kill ring … and the
  minibuffer histories": and the order of the buffers.
- [ADR 0134](0134-a-directory-editor.md), "**`q`** puts the window back to the buffer it showed
  before, leaving the listing" and "**`quit-window` shows the buffer shown before,** the most
  recent other one, and keeps the listing": it also buries the listing, and the most recent other
  one is by the editor's order.

## Where it lives

- **`oso-core`:**
  - `editor/order.rs` (private) keeps the order, raises, buries and restores it, and picks
    where a kill sends a window (`Editor::kill_fallback`, which `remove_buffer` and a space's
    `settle_buffer` share).
  - `Editor::window_shows` raises, and `Editor::window_shows_unrecorded` is the show that does
    not, as Emacs's `norecord`. `Editor::make_tiled` and a switch of spaces raise what the
    selected window shows.
  - `Editor::quit_window` is in `editor/walk.rs`, with the walk of `M-,` and `M-/`, moved there
    from `editor/switch.rs`.
- **Public items:**
  - `Editor::buffers_by_recency` keeps its signature; it now gives the editor's order of the
    buffers a person goes to.
  - New: `Editor::quit_window`, and `Editor::restore_buffer_order(&[BufferId])` for the restart.
- **`oso`:**
  - `dispatch/buffers.rs` sorts `C-x b` by visibility and builds `C-x k`'s order.
  - `commands/buffers.rs`'s `quit-window` calls `Editor::quit_window`.
  - `dispatch/agent.rs` puts the order back when it puts back an agent's selection.
  - `carry/order.rs` writes and reads `order`, and `app/handover.rs` applies it.

## Consequences

- With split windows, `C-x b RET` goes to the buffer used most recently that no window shows,
  as in the author's Emacs.
- A rise costs one pass over the buffers, only when a window changes buffer or is selected;
  typing raises nothing. Measured in a release build with 5000 buffers: a show 12 µs, the
  order 0.1 ms, `M-,`'s first step 0.24 ms, and `C-x b`'s list 2.1 ms, most of it each row's
  annotation; 0.34 ms with 1000.
- `q` from a listing or a status no longer leaves it next in `C-x b`.
- Tests: rising on a show and on a selection; nothing for a window not selected, a walk's steps
  and fallbacks; the walk's end rising on the next show; the walk's least-recent-first tail; `q`
  burying; a kill falling back through the order before a new `*scratch*`, past a terminal, a
  pending open and a secret; a fallback that raises nothing; an agent's refused selection
  leaving the order; `C-x b`'s and `C-x k`'s lists; the state's `order` written, read, checked
  and absent; and, end to end, the order through a restart.

## Questions

1. **What makes a buffer rise:**
   - (a) Emacs's rules, above.
   - (b) Every show in the selected window, walks and fallbacks included. Simpler, but after
     `M-, M-, M-,` `C-x b` would list the buffers walked past first, unlike Emacs.
2. **`q`:** (a) bury the buffer left, as Emacs does; (b) show the buffer before, with no bury.
3. **`C-x k`'s order:**
   - (a) the current buffer, then the editor's order;
   - (b) as ADR 0088 had it, the current buffer, then the window's places.
4. **A restart:** (a) carry the order, as desktop does; (b) not.
5. **The record:** (a) this ADR, amending 0088; (b) a section in 0088.

## Alternatives considered

- **A stamp on each buffer instead of a list,** as Emacs's `use_time` on windows: a rise costs
  nothing, but every read sorts. Reads happen at each `C-x b` and a list of a few hundred moves
  in microseconds, so the list stays.
- **Sorting by visibility in `oso-core`:** `dispatch` already reads which buffers the windows
  show, and only `C-x b` sorts so, so the core gives the plain order.
