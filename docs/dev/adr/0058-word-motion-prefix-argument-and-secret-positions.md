# 0058. Word motion, the prefix argument and positions in secret buffers

- Status: Accepted
- Date: 2026-11-05

## Context

M2-9 brings the motions [ADR 0030](0030-m2-editing-commands.md) decides, and the rest of
what [ADR 0034](0034-command-registry-and-keymaps.md) outlines for `C-u`. Building them left
some choices open:

- Word motion reads clusters until it finds a boundary, and a long line may have none. A
  10 MB line must not stall a key.
- With `C-u`, `Args::count()` reaches commands on the UI thread from a key.
  [ADR 0056](0056-typing.md) left open whether a count needs a limit: a cluster read by seeking
  costs about 0.3 µs, so `C-u 500000000 C-d` in a 1 GB file walks for over 2 minutes.
- [ADR 0051](0051-m2-plan-decisions.md) decides that `get-state` omits positions in a secret
  buffer once word motion exists, and any client can call `export-state` too.

## Decision

### Words

- **A new public module, `word`,** holds pure functions over a display snapshot:
  `forward_word`, `backward_word` and `move_words`. In `guardrails/layers.txt` it sits beside
  `window`, above `motion`. The script table is written by hand, with no dependency.
- **A cluster has a class and a case,** from its first character. The classes are Han, hiragana,
  katakana with `ー`, Latin letters and digits, full-width letters and digits, emoji, other
  letters and digits, marks, and what is not a word: white space, punctuation, symbols such as
  `★` and `♪`, controls, boxed invisible characters and invalid bytes. An emoji is what Unicode
  shows as one by default, or anything with a variation selector or a skin tone.
- **A word breaks** where the class changes, except kanji followed by kana, as ADR 0030 has it, and
  `ー` after hiragana, so that `すごーい` is `すごー|い` and `えーと` is `えー|と`. Katakana words stay one
  word, as `ー` is still katakana. It also breaks at camelCase boundaries as `subword-mode` does going forward: `fooBar` is `foo|Bar`,
  `HTTPServer` is `HTTP|Server`, and `UTF8` is `UT|F8`, since digits stay with what they follow.
  Backward motion stops at the same places, where Emacs's own backward rule differs a little.
  `subword-mode` is always on, since ADR 0030 carries it over and nothing configures it yet.
- **Emoji are words of their own,** so `M-f` stops around them and `M-d` takes them.
- **A mark belongs to what comes before it.** This is a mark with no base of its own, such as `゛` standing
  alone or a boxed one past a cluster's limits ([ADR 0054](0054-limits-on-a-display-cluster.md)). A word breaks as if it
  were not there, so `か゛語` is `か゛|語`, and after what is not a word a lone mark is not a word either. A cursor on
  a mark is in what the mark follows, which needs a look back past the marks, within what a stop may read.
- **Motion follows Emacs:** it passes what is not a word, then goes to the end of the word (or back
  to its start). It does not signal at the buffer's edge, unlike `C-f`. A negative count goes the
  other way. Whether a word breaks depends on two clusters and the one after, so forward and
  backward motion agree on what the words are, with no context to carry.
- **`kill-word` and `backward-kill-word` have their Emacs names and delete** through `apply_edits`,
  as one undo step across all cursors, until the kill ring comes in M2-14. A read-only buffer refuses
  them even with nothing to delete, as `C-d` does.

### What a command may read

- **The limit is on the work, not on the number.** A count as such costs nothing: `C-u 99999 C-n`
  is a few tree lookups. So a prefix argument saturates and is otherwise free, and each command that
  repeats work bounds that work.
- **`WalkBudget`** in `motion` counts what a command reads, in bytes read in order, at about 8 to
  13 ns each. Bytes, because a cluster can hold many characters and the cost per byte stays about the
  same. Going somewhere else in the text costs 512: a line break, a window of a row read backward, the
  start of a word. A command has 4 MiB across all its cursors and its count, about 50 ms at most,
  under the 100 ms that counts as a freeze.
- **Finding one stop has 128 KiB of this,** about 2 ms at most. Past it a motion stops where it got to
  with no signal, and the next key goes on. So a run with no boundary, of text, blanks, marks or empty
  lines, is passed a piece at a time. `M-d` deletes that piece.
- **When a command's budget runs out,** a motion leaves its cursors where they got to and signals
  "Count too large". A deletion leaves that cursor's text alone and signals, after the other cursors
  have deleted, as a deletion that cannot go all the way does (ADR 0056).
- **`C-f`, `C-b`, `C-d` and `DEL` with a count now read in order** instead of seeking per cluster,
  and spend from the budget. A count past the buffer's edge in bytes still needs no walk.
- **Undo crosses at most 256 steps,** then signals. The history is bounded in bytes, but it can hold
  many small steps, and a step of joined typing is 20 small edits. A larger count on a shorter history
  ends with the history, as it did.
- Typing keeps its 16 MiB bound (ADR 0056), and commands with no walk keep no limit.
- `run-command`'s limit of 10,000 stays ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)).

### The prefix argument

- **As ADR 0034 outlines:** `universal-argument` (`C-u`), `digit-argument` (`M-0` to `M-9`) and
  `negative-argument` (`M--`) are commands that leave a prefix through `Effects`, and the dispatcher
  holds it next to the pending prefix keys. It takes it out of the effects, so the host never sees it.
- **`Prefix` in `command`** holds Emacs's rules as pure functions. `C-u` is 4, and each repeat multiplies
  by 4. A digit starts a number, and digits go on building it. `-` makes it negative until a number starts, and
  after that it is text: `C-u 5 -` types five of them, and `M--` changes the sign. `C-u` after digits ends the
  number, so that `C-u 5 C-u 3` types five 3s, and one more starts over at 4. Two `M--` in a row cancel, where
  Emacs stays ready for digits.
- **While it reads digits, a keymap of `0` to `9` is active,** and one of `-` while it can read that, first among
  the minor mode layers. Any other key resolves as usual, so `C-u C-x C-c` still works and `C-u a` types four of
  them. The layer is why a digit after `C-x` is not read as part of the count.
- **`Args` holds the `Prefix`.** `Args::numeric` is unchanged, `count` and `prefix` read its number, and the
  new `raw` is for the three commands above, for `C-u C-l`, which treats `C-u` apart from a number, and for
  `M-- C-v`, which scrolls a screen the other way, as in Emacs.
- **`Ctx::invoked` takes an `Invocation`, replacing `Ctx::with_input`:** the text, as before; `key_char`, the character on the key whatever
  modifiers are held, since `M-5` types nothing; and `repeats`, for a command that does something else when
  repeated, since a command keeps no state. `command` still knows nothing of key presses.
- **`C-g` cancels it** by being a command that ignores its argument. An undefined or cancelled sequence drops
  it too. A command that takes no count ignores it, as ADR 0034 says.
- `run-command` with no argument takes a prefix left by the keys before it, and its own argument replaces one.
  `digit-argument` with no key signals.
- **A restart does not carry it,** though it does carry pending keys. It is a moment's state, and carrying it would
  change the state's format for little.

### `C-a`, `C-e` and `track-eol`

- **`mwim-beginning-of-code-or-line` and `mwim-end-of-code-or-line`,** the names of the Emacs package, are on `C-a`
  and `C-e`. As there, from anywhere else `C-a` goes to the first non-blank character, and from there to the line's
  start. `C-e` goes to the end before trailing blanks, and from there to the line's end. A blank is a tab or white
  space, including a full-width space.
- `<home>` and `<end>` stay on `move-beginning-of-line` and `move-end-of-line`, for the plain line ends.
- A count other than 1 moves that many lines less one first, as ADR 0040 has for `C-a`, and goes to the first of the
  two places.
- **`track-eol` follows Emacs on an empty line:** vertical motion from the start of one keeps column 0, unless a
  command went to its end on purpose. So `C-e` now sets the goal, which every other motion still clears.

### `C-l`

`recenter-top-bottom` cycles through the middle, top and bottom row, starting over after any other command. A number
puts the cursor on that row, from the bottom if negative, and a plain `C-u` in the middle. The cycle is
`Ctx::repeats` modulo 3, so nothing is stored. The wheel is no command here, so it does not end the cycle. The view
stops at the first row, as it does everywhere, so near it the cursor may stay above that place, and nothing is drawn if
the view does not move.

### The wheel

- **`WheelRemainder` in `window`** adds up what is not yet worth a whole row or column, for lines and pixels alike, so
  small deltas count. An event counts along the axis it moved furthest on, counting 2 columns as a row, so a trackpad
  gesture a little off straight stays on its axis. The other axis keeps what it holds, so a slow scroll keeps its progress
  through a stray event, and a drag whose main axis alternates still moves.
- **The sideways wheel** moves `left` by 6 columns a notch, about as far as 3 rows, or by trackpad pixels.
  `Editor::scroll_view_sideways` stops once the widest row in view shows its end and a cell for the cursor. That bound
  costs a conversion per visible row at most, and none going left, so no summary needs the widest row in the buffer,
  and it is what the view can show. A view already past it stays.
- **Sideways scrolling leaves the cursors where they are, even out of view,** and the next command brings the view back,
  which `follow_cursor` already does. A cursor moved into view would have no place to go on a row too short to reach it.
- **`Window::set_top` stops at the last row, and follows the primary cursor sideways if it moved that cursor.** It moves a
  cursor left behind to column 0 ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md)), so the view goes to it,
  as Emacs's `auto-hscroll-mode` does. A cursor it did not move stays, so scrolling down keeps a sideways view until the
  cursor leaves the rows in view.

### The control socket

This is a human gate ([`REVIEW.md`](../../../REVIEW.md)). It applies ADR 0051, "Control socket", and goes further; see below.

- **While a window shows a secret buffer, `get-state` leaves out where it is:** `cursor` and `scroll`, and each such
  window's `scroll`, `primary` and `selections`. The rest stays, including the window's size.
- **`bytes` and `lines` of a secret buffer are null while it is modified,** and return once it is back to its saved state.
- **`export-state` holds no position in a secret buffer.** Such a window is exported with one cursor at the start and no
  scroll, which needs no change to the format or its version.
- **Positions still waiting for a file to load are exported as they are,** though the file counts as secret until it is
  judged. They came with the import, and say nothing of its text.
- `generation` and the refusal to hand over stay.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0030](0030-m2-editing-commands.md), a table "generated from Unicode data": it is written by hand. Katakana
  "including `ー`": it stays with hiragana before it, by the author's decision.
- [ADR 0034](0034-command-registry-and-keymaps.md), `Args` holds a numeric argument: it holds a `Prefix`.
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md), "every other motion clears" the goal: `C-e` and `<end>`
  set it. "Horizontal wheel scrolling is not handled yet": it is.
- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md), metadata including "size, positions" stays: positions
  go, as ADR 0051 says, and size goes while a buffer is modified. A secret buffer handed over "by path and position":
  by path only.
- [ADR 0056](0056-typing.md), `undo` with any count ends at the end of its history: past 256 steps it signals.

## Consequences

- One word costs about 1 to 4 µs on real text, and about 1 to 2 ms on 10 MB of one character or a run of marks,
  sequences, empty lines or punctuation, each way, measured by `text_perf`
  (`cargo run --release -p oso-core --example text_perf -- words`). A count that spends all a command may takes up to
  about 45 ms.
- A motion past a stop's limit can stop in the middle of a word, and forward and backward motion do not
  agree there. Only a run longer than about 128 KiB forward, or about 26 KiB backward, gets there. `C-a` and
  `C-e` cannot reach text past that many blanks in one row.
- A count past about 8,000 line breaks or words forward, or about 6,000 words backward, signals "Count too large" on
  any text, since each stop charges for a seek, and going back for a whole window. Measured, this leaves room: a real
  file's count takes about 5 ms when it runs out.
- A restart loses a secret buffer's cursor position, which goes to the start of the file.
- Tests of secret buffers over the socket can check only what remains. `oso ctl` shows no size for one with
  unsaved edits.
- Until M2-7 draws it, nothing shows that a prefix is being typed.
- `Effects`, `Args` and `Ctx` changed, and `Prefix`, `Invocation`, `WalkBudget`, `WheelRemainder` and
  `Recenter` are new, as the API snapshot shows.
- `C-a` and `C-e` read their row one way, so wrapped rows (M2-25) must keep them on logical lines, with the rest
  of [`open-questions.md`](../open-questions.md)'s item on motion.
- The wheel no longer leaves a cursor it moved on a cluster cut by the left edge, since the view follows it. Only the
  sideways wheel can now ([ADR 0057](0057-frame-layout.md) draws the part in view).
- M2-14 turns `delete_words` into kills, and its bindings and names stay. It needs a rule for pieces when cursors'
  ranges overlap, and `Ctx::repeats` will not do for appending kills across different commands.

## Alternatives considered

- **A limit on the prefix number:** simple, but a number that is fine for `C-n` is no bound with a thousand cursors,
  and ADR 0056 turned that down for typing already.
- **Refusing a count up front if it might read too much:** nothing moves, but a word's cost cannot be known before
  reading it, and a refused `C-u 1000 M-f` would be no help. Going as far as it can is what a motion does at an edge.
- **Reading on to the boundary, however far:** agrees with Emacs on any text, but 10 MB takes tens of milliseconds at
  best, and a 1 GB line takes seconds.
- **A summary in the `SumTree` that says a chunk has no boundary:** skips long runs at no cost, but adds to what every
  chunk holds for rare text, and word boundaries depend on classes the display map has no business knowing.
- **A backward cluster iterator:** cleaner for `M-b`, but chunks are scanned forward, so it would need to buffer a chunk
  or a new scan. Windows need neither, and cost one seek each.
- **Treating emoji as what is not a word:** `M-d` on `🎉 next` would take both, and adjacent text with no space would read
  as one word.
- **A dedicated field on `Editor` for the `C-l` cycle:** Emacs's way, but it needs resetting by every other command, which
  the dispatcher already knows about.
- **Separate commands `digit-argument-0` to `-9`:** no new context, but ten commands for one idea, and `describe-key` would
  read oddly.
- **`set_top` always following the cursor sideways:** simple, but a trackpad gesture with a little vertical motion would undo
  sideways scrolling on every event.
- **Moving the cursor into view when scrolling sideways:** matches `C-v`, but a short row has no place in view.
- **Leaving position keys out of `export-state`:** honest about what is there, but needs a new version, with a new
  reader, for no more than what zeros say.
- **Keeping a secret buffer's positions in a state only the supervisor can read:** keeps its cursor through a restart, but
  needs a second way to hand state over, for a dev-only convenience.
