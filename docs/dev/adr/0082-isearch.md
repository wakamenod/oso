# 0082. isearch

- Status: Proposed
- Date: 2026-11-08

## Context

M2-20 brings isearch, `C-s` and `C-r` ([roadmap](../roadmap.md)). Earlier ADRs fixed the outline:

- all matches in view are highlighted, the current one emphasized, an "n/m" count is shown and the
  search string is in the echo area ([ADR 0026](0026-m2-ui-layout.md));
- smart case: case-insensitive unless the string holds an uppercase letter, and a key in isearch
  will put cursors on all matches (M2-21) ([ADR 0030](0030-m2-editing-commands.md));
- isearch's exit sets the mark where Emacs does ([ADR 0071](0071-mark-and-region.md));
- isearch is step 5 of the `C-g` chain, with two steps of its own, after the region and the
  innermost prompt, in `Editor::quit` ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md));
- the IME is off while isearch reads keys ([ADR 0069](0069-macos-input-method.md));
- search skips the U+FFFD of an invalid byte ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md));
- while a buffer that may hold secrets is shown, the control socket gives no positions and no match
  counts in it ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md),
  [ADR 0051](0051-m2-plan-decisions.md)), and a search string can say something of the text
  (ADR 0072).

A buffer can be 1 GB, and the UI thread must never wait on it: a `C-s` with no match ahead would scan
all of it, and so would counting the matches. The author chose these on 2026-11-08, through the
coordinator.

## Decision

### Matching

- **`C-s` and `C-r` search for the literal string (chosen).** Regex isearch (`C-M-s`) goes to M3,
  with query-replace. Literal search needs no dependency.
- **Smart case (chosen):** the search is case-sensitive once the string holds a letter for which
  `char::is_uppercase` holds. Otherwise both sides fold one character at a time: a character whose
  lowercase is one character becomes it, and any other stays as it is. Width and characters are not
  folded: `Ａ` does not match `A`. **`M-c` turns case sensitivity around for the rest of this
  search,** and the echo line then says `Case-sensitive` or `Case-insensitive`.
- **A new module, `oso_core::search`, holds the matcher, private to the crate:** a query built from
  the string, searched over a rope's chunks one character at a time with Knuth–Morris–Pratt, so the
  cost is linear in the text whatever the string, and a match may cross chunks. The U+FFFD of an
  invalid byte matches nothing; the invalid bytes are walked alongside the text, so each costs O(1).
  Searching forward from an offset finds the first match that starts there or after; searching
  backward finds the last that ends there or before, and with a limit on its start, the last whose
  start is within it, also one that begins inside a match it passes. A match may begin or end inside
  a display cluster, as in Emacs, and the cursor then goes to the cluster's start. In
  `guardrails/layers.txt` it sits beside `buffer` and `motion`, above `load` and `display_map`,
  which its tests read text with. M3's query-replace and project search decide what of it becomes
  public.
- **What is public:** in `oso_core::editor`, `Isearch` (its string, direction, state, count and
  whether it is around a secret, all read-only), `SearchJob` (`run` alone) and an opaque
  `SearchReport`, as `SaveJob` is, `MatchCells` (the current match's cells and the others'), the
  `Editor::isearch*` methods and `take_isearch_job`, and `Quit::SearchStep` and `Quit::Search`;
  `frame::match_cells`; `SelectionSet::move_primary` and `push_primary_mark`. In `oso_render`,
  `Face::SearchMatch` and `Face::SearchCurrent`, and `TextView::fills` in place of `region_cells`. Nothing
  of `oso_core::search` itself.
- **The string holds at most 1024 characters.** A yank into it is cut there, and copies no more of
  the kill than that, so that what a frame looks at around the view stays small.

### Keys

- **isearch is a mode of the selected window, not a prompt (chosen),** and its line is on the echo
  row: `I-search: foo [3/17]`, with `Failing`, `Wrapped`, `backward` and the case words as in Emacs.
- **It keeps to that window and its buffer.** When the window shows another buffer, the buffer is
  killed, or a prompt opens over it (the exit question of the window's close button, say), the search
  ends with nothing kept: no history, no mark, the cursors where they are. So nothing it searches or
  moves can be another buffer's.
- **Its keymap replaces the others while it is on.** A key it does not bind ends the search where it
  is, as `RET` does, and then runs as it would have (chosen), so `C-f`, `C-a`, `C-x` and `M-x` all
  work.
- **`DEL` undoes the last isearch step,** the string and the place together, as Emacs's
  `isearch-delete-char` does (chosen), also a string `M-p` or `M-n` put in place of another.
- **`C-s` and `C-r` with an empty string search for the last string** (`C-s C-s`). **`M-p` and `M-n`**
  go through a search history: in memory only, 100 entries, as the prompts' histories, and nothing
  enters it while a buffer that may hold secrets is shown, or from a search that began in one.
- **`C-w` adds the word after the match,** as Emacs's `isearch-yank-word-or-char` does, or the
  character there if no word is, and **`C-y` the newest kill-ring entry**, its pieces joined with
  newlines. `C-y` never waits for the OS clipboard (ADR 0074): it takes what the ring holds. A kill
  from a buffer that may hold secrets makes the searched buffer one, as a yank does (ADR 0074).
  `M-y` waits for M2-21.
- **The IME is off while isearch is on** (`wants_ime`), so a Japanese string cannot be typed. Migemo
  (M3) searches Japanese from romaji.

### Where it goes

- **Typing searches from the current match's start,** forward for a match that starts there or
  after, backward for one that starts there or before, so a match that still matches stays. A
  repeat searches on from the match's end (backward: before its start).
- **Wrap-around as in Emacs (chosen):** with no match ahead the line says `Failing I-search`, and the
  next `C-s` (`C-r`) searches from the buffer's start (end) and says `Wrapped`.
- **Each key searches on the UI thread within a budget of 256 KiB of text.** If that finds nothing
  and the text goes on, the rest is searched on a worker thread over an O(1) snapshot of the rope and
  of its invalid bytes, the line says `[searching…]`, and the cursor goes there when the answer comes,
  unless the string or the text has changed since. A newer search supersedes it, through a generation
  number the worker checks every 64 KiB. One worker thread runs the newest job, and starts with the
  first.
- **The count is the worker's too (chosen):** one job per string and text counts every match, as a
  non-overlapping scan from the buffer's start finds them, and where the current one is among them.
  The line says `[?/…]` meanwhile. A repeat steps the index without a new count, and a wrap takes it
  to the first match (backward: the last); a change of string or case starts one. There is no cap on
  the count: it costs only time, off the UI thread.
- **Only the primary cursor searches and moves (chosen).** The others stay, and merge as usual if it
  meets them. M2-21 adds the key that puts a cursor on every match.

### Ending it

- **`C-g` takes two steps, as ADR 0072 left for isearch (chosen):** with a failing part or a search
  still running, the first takes it away, back to the last string that matched (`Quit::SearchStep`),
  and says nothing, as the search goes on and its line stays; otherwise it ends the search and puts
  every cursor, its mark, the region and the view back as they were before it began (`Quit::Search`),
  and says `Quit`. It comes after the region and the innermost prompt in `Editor::quit`, so a region
  shown during the search goes first.
- **`RET`, or a key isearch does not bind, ends it where it is,** and puts the string in the history.
  If the cursor moved and no region shows, it pushes the place it began onto the primary cursor's mark
  ring and says `Mark saved where search started`, as Emacs does; around a buffer that may hold
  secrets that goes to the echo area alone, since it says the search matched. `run-command` of a
  command isearch's keymap does not hold ends it the same way, once the command's argument is checked.

### Highlighting

- **Rectangles of cells, as the region's (chosen),** not yet the display model's Search layer: the
  current match in a strong amber, the others in view in a muted one, behind the text, which keeps its
  colour. Regions, boxes and the cursor draw over them. M2-23's themes take over the colours.
- **Only what the view shows is searched for them,** each visible row's columns widened by the
  string's longest possible length on both sides, within 256 KiB of text and 2048 matches a frame, so
  a frame's cost is bounded by the view and the string, not the buffer. The renderer takes them, and
  the region's cells, as one list of fills, back to front.

### A change to the text

- **A reload of a file changed on disk (M2-22) waits while isearch is on,** as it waits for a prompt.
- **isearch remembers the buffer's revision,** for what changes it anyway. When the revision has
  changed, what a worker reports is dropped, a search runs again from the cursor as typing does, or a
  repeat the worker went on with is asked again, and so does the count. The places `C-g` and `DEL` go back to are clipped to the text. isearch stays on.
- **A load still under way holds the count until it ends,** with `[?/…]`, rather than starting it
  again at each piece the load takes in.

### Secrets and the control socket

- **isearch works in a buffer that may hold secrets (chosen),** and the screen shows it as anywhere.
  A search that began in one, or saw one, is around a secret: its string enters no history.
- **Nothing of the string reaches a log or `*Messages*`.** A worker's report carries offsets only,
  and nothing logs them. `Debug` of the search state prints no string.
- **`get-state` gains `isearch`:** `null`, or `string`, `backward`, `failing`, `wrapped`,
  `case_sensitive`, `searching` and `count` (`{index, total}` or `null` while counting). While a buffer
  that may hold secrets is shown, or for a search around one, it is only `{"active": true}`, and the
  echo line is left out as it already is.
- **A dev restart waits for isearch to end (chosen):** `check-handover`, `export-state` and a
  generation's `exit` refuse while it is on, as for a prompt.

## Changes to accepted ADRs

One passage of [ADR 0085](0085-agent-channel-and-oso-mcp.md) changes; the rest of its "Where the
channel waits for the person" holds:

- "The person is in the middle of something: a prompt the person opened is innermost, keys are in
  progress, a composition is open, or a key came from the window less than 1 s ago" becomes: also
  while an isearch is on (`Editor::isearch`). A channel command would end the search, as a key
  isearch does not bind does, or move the cursor from under it. The isearch commands are `Person`,
  as they read keys.

## Consequences

- isearch costs a key at most a bounded search and a frame a bounded scan of the view, whatever the
  buffer's size; a far match or a count over 1 GB takes seconds, off the UI thread.
- A count over overlapping matches (`aa` in `aaaa`) counts them as a scan from the start does, and a
  current match found from another place, as backward (`aa` in `aaa` from the end is 1..3), may fall
  between two counted ones: its index is then the number of counted matches that start before it,
  plus one, at most the total.
- The highlights are worked out each frame while isearch is on, from the text in view: no layer and no
  anchors yet. The Search layer of `display-model.md` comes when a second producer needs one.
- The string is not width- or char-folded, and there is no regex search until M3.
- A loading buffer's count shows `[?/…]` until the load ends.

## Alternatives considered

- **Regex isearch now:** the `regex` crate needs contiguous text, so a search over the rope would copy
  it or need a streaming automaton, for a feature M2 does not need.
- **No `M-c`:** smart case alone cannot search for a lowercase-only match of a string with a capital.
- **Folding full-width letters to half-width:** convenient for Japanese text, but Emacs does not by
  default, and it is char folding, not case.
- **Ignoring other keys, so only `RET` and `C-g` leave:** safer against a stray key, but every motion
  would need two keys.
- **Searching every key on the worker:** a frame of latency on every key, also for a match on the next
  line. Searching all on the UI thread, as Emacs does, breaks the rule that it never waits.
- **Counting only the matches in view (`17+`):** cheap, but says nothing of the rest of the buffer,
  which is what a count is for.
- **Wrapping at once, with no failing step:** a person cannot tell that they passed the end.
- **`DEL` as a plain backspace on the string:** loses the place each character found.
- **`C-g` always cancelling in one step:** a typo at the end of a long string would lose the search.
- **Building the Search decoration layer now:** anchors that follow edits are needed by nothing yet,
  since the text does not change during a search but by a reload.
- **Dropping the extra cursors when a search begins, or every cursor searching from itself:** the
  first loses the cursors for good; the second is M2-21's key, made implicit.
- **Refusing isearch in a buffer that may hold secrets:** safe, but a person editing a key file
  could not search it, while the screen already shows what a search would.
- **A restart that ends isearch at its match:** never holds a restart, but takes a search away
  mid-typing.
