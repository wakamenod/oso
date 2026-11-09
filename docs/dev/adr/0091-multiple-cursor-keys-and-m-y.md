# 0091. Multiple-cursor keys and `M-y`

- Status: Accepted, amended by 0092 and 0098
- Date: 2026-11-09

## Context

M2-21 brings the keys that add cursors, and `M-y` from the kill ring
([roadmap](../roadmap.md)). Commands already work on the window's selection set, so this step is
about keys, behaviour and the float, not about the core model. Earlier ADRs fixed the outline:

- each window has a selection set, and cursors are added at the next or previous occurrence of the
  word or region, on a rectangle, at all search matches, or with a modifier and a click
  ([ADR 0007](0007-multi-cursor.md));
- the keys are multiple-cursors.el's: `C->` and `C-<` add a cursor at the next or previous
  occurrence of the region or the word at point, `C-c C-<` adds them at all occurrences, `C-x SPC`
  (a rectangle) followed by a command puts a cursor on each line, a key in isearch puts them on all
  matches, and Super with a click adds one; `C-g` removes extra cursors first; `M-y` picks from the
  kill ring in the minibuffer with fuzzy matching, consult-yank-pop style, and inserts the entry, or
  one piece per cursor ([ADR 0030](0030-m2-editing-commands.md));
- each cursor has its own mark, the window shows all regions or none, and regions that overlap
  merge ([ADR 0071](0071-mark-and-region.md));
- each `C-g` does the first thing that applies and says "Quit": a composition, a pending prefix,
  the region, the innermost prompt, isearch's two steps, then extra cursors (M2-21)
  ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md));
- a kill with N cursors stores N pieces as one entry, a yank with as many cursors puts each piece
  at its cursor and joins them with line breaks otherwise, a yank reads the clipboard first and
  waits for it without blocking, the clipboard is read only on a yank, and a secret entry makes
  the buffer it is yanked into secret ([ADR 0074](0074-the-kill-ring-and-the-clipboard.md));
- completion runs in the centred float, and `C-x b` and `C-.` split it into a list and a preview
  that never shows a buffer that may hold secrets ([ADR 0076](0076-completion-and-m-x.md),
  [ADR 0088](0088-switching-and-killing-buffers.md));
- isearch has a literal matcher with smart case in `oso_core::search`, searches 256 KiB on the UI
  thread and the rest on a worker, moves only the primary cursor, binds no prefix key, and leaves
  `M-y` and the key that puts a cursor on every match to M2-21 ([ADR 0082](0082-isearch.md));
- every command declares its reach, the channel refuses `Person`, and it runs nothing while a
  window shows a buffer that may hold secrets ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).

What is still open: the budget is key press to photon at p99 ≤ 8 ms, and typing with 1000 cursors
in a 100 MB file takes 16.75 ms a key at p50 and 25.8 ms at p99 for the edit alone, and undoing it
takes as long. With a mark at every cursor, as after `C->`, it takes 16.2 to 16.9 ms on 100 MB and
15.7 to 19.9 ms on a 10 MB line, mostly from clipping the marks (ADR 0071). `open-questions.md`
asks to batch the edits or cap the cursors before M2-21, and notes that every window clips all its
cursors after each edit. The editor has no mouse clicks yet.

## Decision

The author chose on 2026-11-09 the first option of each question under [Questions](#questions),
which the text below describes. The rest are defaults put to them with this ADR. They accepted
this ADR, with the rest as written and the new public items it names, on 2026-11-09, through
the coordinator.

### Adding cursors

| Keys | Command | What it does |
|---|---|---|
| `C->` | `mark-next-like-this` | Adds a cursor at the next occurrence |
| `C-<` | `mark-previous-like-this` | Adds a cursor at the previous occurrence |
| `C-M->` | `skip-to-next-like-this` | Moves the last cursor to the occurrence after it |
| `C-M-<` | `skip-to-previous-like-this` | Moves the first cursor to the occurrence before it |
| `C-c C-<` | `mark-all-like-this` | A cursor at every occurrence in the buffer |
| `C-x SPC` | `edit-lines` | A cursor on each line of the region |

- **What is searched for:** with a region shown, the primary cursor's region, as a literal string,
  case-sensitive, as multiple-cursors.el searches. With no region shown, the word around each
  cursor (ADR 0058's word) is selected first, with the regions shown, and the key then goes on as
  with a region, as multiple-cursors.el's `mc/mark-next-like-this-word` does, so one `C->` on `id`
  marks it and the next `id`. Occurrences of a string taken from a word must be whole words, as
  VS Code's `Cmd+D` does, so `id` does not find `idle`. With no word at the primary cursor it
  signals "No word at point".
- **The window keeps that query** (the string, the case rule and whether it is a whole word) while
  it has more than one cursor, so a run of these keys searches for the same thing. It goes when the
  cursors are back to one or the text changes. A query of more than 1024 characters signals "Region
  too long to match", as isearch's string is bounded (ADR 0082).
- **The query can be secret text,** as isearch's string can: its `Debug` prints no text, nothing
  logs it, and `oso dev` does not carry it (ADR 0082, ADR 0090).
- **Where they search:** `C->` from the end of the last cursor in buffer order, `C-<` back from
  the start of the first. They do not wrap: past the end they say "No more matches". The skips
  start from the same places, and `C-M->` with one cursor moves it, as a plain search would.
- **The new cursor's region covers the match,** in the direction of the primary's, and the new
  cursor becomes the primary, so the view follows it. The others stay. `C-c C-<` keeps the primary
  where it was.
- **A count:** a positive N adds N occurrences, as N presses do; `M--` before `C->` removes the
  last cursor in buffer order, and before `C-<` the first, as multiple-cursors.el's negative
  argument does; `M-0` skips, as `C-M->` does. If the cursor removed was the primary, the one now
  last (or first) becomes it. With one cursor, removing says "Only one cursor".
- **The search is isearch's matcher, within isearch's budget per command.** One command searches
  at most 256 KiB of text on the UI thread, whatever its count; if that does not finish it, the
  rest is searched on the isearch worker, over a snapshot, and the echo area says `Searching…`.
  The cursors are added when the answer comes, if the buffer's revision and the window's cursors
  and regions are still as they were; otherwise the answer is dropped with "Search dropped: the
  buffer changed". A newer search supersedes it. Every `C-g` drops a search still waiting, besides
  its step in the chain below. `C-c C-<` finds its matches on the worker past 256 KiB, the way
  isearch counts them.
- **A buffer still loading:** `C->`, `C-<` and the skips search what has loaded, and a piece the
  load appends does not drop an answer, since it moves no position. `C-c C-<` signals "Buffer is
  still loading", since cursors on part of it would mislead.
- **`edit-lines`** puts one cursor on each line from the mark's line to the primary cursor's line,
  at the primary cursor's goal column, or at a shorter line's end, as multiple-cursors.el's
  `mc/edit-lines` does. It uses the region if one shows, and otherwise the mark, as `M-w` does; with
  no mark it signals "The mark is not set now". It hides the regions and keeps the primary on the
  cursor's line. With several cursors it starts from the primary alone. A rectangle that shows and
  grows before it turns into cursors comes with M3's rectangle commands, on a key of its own, since
  `C-x SPC` is this one's.
- **At most 1000 cursors** in a window. A key that would pass it adds nothing and says "At most
  1000 cursors"; `C-c C-<` with more matches says "1234 matches; at most 1000 cursors" and adds
  none, so a rename never covers only part of the buffer. `edit-lines` over more lines says the
  same. The bound is a constant in `oso-core`, beside the selection set.
- **A cursor with a click** waits for mouse clicks, which oso does not take yet.

### `C-g` with several cursors

- **`C-g` hides the regions first, then removes the extra cursors,** as ADR 0072's chain has it and
  multiple-cursors.el's `mc/keyboard-quit` does: after `C-> C->`, the first `C-g` leaves three
  cursors with no region, so `C-a` and typing work at all of them, and the second leaves the
  primary alone. Each says "Quit", as ADR 0072 says.
- **`Editor::quit`** gains `Quit::Cursors`, after the region, the innermost prompt and isearch's
  steps. So `C-g` in a prompt over a window with several cursors closes the prompt and keeps the
  cursors, and `C-g` during a search ends the search first.

### From isearch

- **`C->` in isearch** ends the search with the current match as the primary's region, and adds a
  cursor at the next match, as `C->` does after it. **`C-<`** adds one at the previous match. **`C-c
  C-<`** ends the search and puts a cursor on every match in the buffer, the primary on the current
  one. Each ends the search as `RET` does, with the string in the history.
- **The query is isearch's:** its string, with its smart case or the case `M-c` chose, and not a
  whole word. Later `C->` presses keep that query.
- **With no current match** (the search is failing, or still running on the worker), they say "No
  current match" and the search goes on. When `C-c C-<` finds more matches than the bound, it says
  so and the search goes on too.
- They are isearch's own commands, `isearch-mark-next`, `isearch-mark-previous` and
  `isearch-mark-all`, in its keymap.
- **isearch gets its first prefix key, `C-c`.** A sequence under it that isearch does not bind ends
  the search and is resolved again, whole, in the other keymaps, as a single unbound key is now; so
  `C-c x` ends the search and runs what `C-c x` runs.

### `M-y` in a buffer

```
        ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
        ┃ 1/5 Yank from kill ring: █                                 ┃
        ┃▌fn main() {⤶    let…  ▐┃fn main() {                         ┃
        ┃ hello world            ┃    let x = 1;                      ┃
        ┃ secret · 1 line        ┃}                                   ┃
        ┃ alpha⤶beta  3 cursors  ┃                                    ┃
        ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

- **`consult-yank-pop` on `M-y`** reads an entry in the centred float, labelled `Yank from kill
  ring:`, with no history. A read-only buffer signals "Buffer is read-only" first, before anything
  is read. Then it reads the clipboard, as `C-y` does (ADR 0074), so another app's newest copy is
  in the ring and at the top, even if `C-g` then closes the float; input from a socket never reads
  the system clipboard. An empty ring signals "Kill ring is empty".
- **The list** is the ring, the newest first. ADR 0076's history, length and alphabetical order do
  not apply. **An entry equal to a newer one is listed once,** at its newest place: entries of at
  most 1 KiB are compared whole, through a hash set, and larger ones only by their fingerprints
  when both have one (ADR 0074). Larger ones without are listed as they are, so opening the float
  never compares more than the ring's small entries.
- **A row** is the entry's text from its start, its pieces joined with line breaks as a yank with
  another count joins them, each line break drawn as `⤶` in the annotation's face, and other
  characters as the echo area draws them (ADR 0063), cut at the row's edge with `…`. Its annotation
  says how many lines when there is more than one (`3 lines`), and how many pieces when there is
  more than one (`3 cursors`).
- **Matching** is ADR 0076's orderless matcher over the row's text: the first 1 KiB of the entry.
  So a key costs the same whatever the entries' sizes.
- **The preview** takes the right 3/5 of the float below the answer's row, as `C-x b`'s does
  (ADR 0088): the selected entry's text from its start, as many lines as the preview has rows, each
  cut at its edge, with the pieces joined by line breaks. It reads no more of the entry than those
  rows show. It changes no buffer. A narrow frame gives it up, as ADR 0088 says.
- **A secret entry is listed but never shown:** its row says `secret` and its line count in the
  annotation's face, and its preview is ADR 0088's `may hold secrets`. Secret rows are dropped
  from the list for any input that is not empty, whatever its operators, so no input can tell
  anything of their text. Picking one with an empty input inserts it, and the buffer becomes
  secret, as ADR 0074 says.
- **`RET`** inserts the entry as `C-y` does: at every cursor, one piece per cursor when the counts
  match and all of them joined otherwise, within the 16 MiB bound, as one undo step, with each mark
  at the start of what it inserted and the region hidden. **Right after a `C-y` or `M-y`,** whose
  text is still there unchanged, it replaces that text instead, as consult-yank-pop does.
- **The entry picked moves to the front of the ring,** as consult's `consult-yank-rotate`, so the
  next `C-y` yanks it. The clipboard is not written, as Emacs's `yank-pop-change-selection` nil.
- `C-g` closes the float and changes nothing in the buffer.

### `M-y` in isearch and in a prompt

- **Right after a yank it cycles,** as Emacs's `yank-pop` always did: `isearch-yank-pop` in isearch
  replaces what the last `C-y` or `M-y` added to the string with the next older entry, its pieces
  joined and cut to 1024 characters as `C-y` there does; `yank-pop` in a prompt replaces the text
  the last yank inserted. Repeating goes further back, and past the oldest entry back to the newest.
- **Cycling skips secret entries,** so their text never shows in `I-search:` or in a prompt, and
  passing by one makes nothing secret. With no other entry it says "No other kill to yank".
- **Otherwise they signal** "Previous command was not a yank". No float opens over isearch or over
  a prompt.
- A secret entry that `C-y` put there keeps ADR 0074's and ADR 0082's rules: in isearch the
  searched buffer becomes secret; in a prompt the prompt's buffer becomes secret, so its answer
  enters no history (ADR 0072).

### Several cursors, kills and yanks

These follow ADR 0007 and ADR 0074 unchanged: a kill with N cursors stores N pieces as one entry,
a yank and `M-y` with as many cursors put each piece back at its cursor, and any other count joins
them with line breaks. Typing over regions and `M-w` work at every cursor.

### Batching the edits

- **Where the time goes now:** each splice costs about 17 µs. `Rope::replace` and the tab map's
  `sync` each split the tree twice and append twice, so one splice copies O(log n) nodes four
  times. After the edit, every window showing the buffer clips each of its cursors and marks with a
  seek of its own, which is most of the cost once every cursor has a mark.
- **A command's splices are applied in one pass.** The rope gains a batched splice and the tab map
  a batched sync, each `pub(crate)`, in its own layer (`rope/` and `display_map/`, as
  `guardrails/layers.txt` puts them), and `buffer/splice.rs` drives both, as it drives one splice
  each now. Undo and redo go through the same pass. Which pass (an edit in place inside a leaf, or
  one walk that keeps the untouched subtrees) is chosen by measuring both.
- **Only cursors and marks near an edit are clipped,** in the same pass: those inside the range of
  chunks an edit rebuilt, since cluster boundaries elsewhere do not move. This holds in every window
  showing the buffer, which settles the open question on windows clipping all their cursors. A
  command clones the selection set once, where an edit now clones it a few times.
- **The target is the budget: key press to photon at p99 ≤ 8 ms** with 1000 cursors. The edit's
  own share, measured by `text_perf` on the UI thread without a frame, aims at p99 ≤ 4 ms, so that
  the frame keeps the rest, in four cases: typing at 1000 bare cursors, typing over 1000 regions
  with their marks, and undoing each, on the 100 MB file and on the 10 MB single line. A single
  cursor must not slow down (10.5 µs at p50 now). Key press to photon is measured with a `just
  perf` scenario of 1000 cursors; its thresholds are guard files, so the implementing pull request
  asks the author before adding it. It reports p50 and p99 before and after; if 1000 cursors miss
  the budget, the bound goes back to the author.
- **No `unsafe`, no new public module:** `oso-core` has none of either for this. `rope.rs` is at
  its size limit, so the rope's part is a private module under `rope/`.

### Reach and the sockets

- **`mark-next-like-this`, `mark-previous-like-this`, the two skips, `mark-all-like-this` and
  `edit-lines` are `Look`** (ADR 0085): they move and mark, and change no text. The channel already
  runs nothing while a window shows a buffer that may hold secrets (ADR 0085), and these act on the
  shown buffer, so they need no refusal of their own. A `run` answers once the command has run; when
  the search went to the worker, the cursors come after it, and `state` shows them.
- **Messages that say a search found something** (`No more matches`, the count of `C-c C-<`) go
  to the echo area only, not to `*Messages*`, while a buffer that may hold secrets is shown, as
  isearch's do (ADR 0082).
- **`consult-yank-pop`, `yank-pop`, `isearch-yank-pop` and the three isearch commands are
  `Person`:** they read the clipboard or pick in a list, or belong to isearch, whose commands are all
  `Person`.
- **`get-state`** gives the `M-y` prompt its kind `complete` and its label, and leaves
  `completion` out entirely: no rows, count, total, selection or preview. The rows would carry
  another app's copy, and the count the number of secret entries, which ADR 0074 leaves out.
  **`screenshot` is refused while the `M-y` float is open,** as it is around secrets, since the
  shot would show what `get-state` leaves out. A dev client can still read ordinary entries by
  sending `M-y RET` and reading the buffer, as `C-u N C-y` already lets it; that is dev only.

### Keys on the way in

- `C->` is Control with `>`, Shift folded into the character (ADR 0033), so the key model holds it.
  The socket's parser builds it from text, so no e2e flow covers the way from the OS: `keys.rs` gets
  tests for `>` and `<` with Control and Shift, and with Control, Meta and Shift, and the author
  checks the keys at the keyboard on winit and on the native layer. If a layer gave the key without
  Shift folded in, `C->` would arrive as `C-.`, `consult-mark`.

### Where it lives

- **`oso-core`:** `editor/cursors.rs`, a private module, with the query a window keeps and the
  `Editor` methods behind the commands; the search through `oso_core::search` (still private) and
  the isearch worker's `SearchJob`; `Quit::Cursors`; `selection::MAX_CURSORS`; the batched pass as
  above; and the `M-y` prompt's candidates, rows and preview in `editor/kill.rs` and `minibuffer`,
  with a `Preview::Text { entry }` beside `Preview::Buffer`, carrying the ring entry's id, since
  `Preview` is `Copy`, and `Preview::Withheld` for a secret one; and an `ArgSpec::KillEntry` that
  reads the clipboard and then the entry, as `CountAndClipboard` reads the clipboard.
- **`oso`:** `commands/cursors.rs` and `bindings/cursors.rs`; `consult-yank-pop` and `yank-pop` in
  `commands/kill.rs`; the isearch commands in `commands/isearch.rs`; the dispatcher's rule for a
  sequence under isearch's prefix; the preview's text pane in `app/draw/float.rs`; the screenshot
  refusal in `dev/state.rs`.
- The new public items, as the API snapshots show, are `Quit::Cursors`, `MAX_CURSORS`,
  `Preview::Text`, `ArgSpec::KillEntry` and the `Editor` methods. No new module is public.

### Tests and measures

- Unit tests for each rule above: the query from a region, a word and isearch; whole words; no
  wrap; counts, skips and removal, also of the primary; the 256 KiB per command across a count; the
  bound for each way of adding; a loading buffer; `edit-lines` with short lines, a hidden region and
  no mark; the `C-g` order with a prompt and with isearch, and a waiting search dropped; isearch
  with no current match and over the bound; `C-c x` ending isearch and running the global binding;
  the `M-y` rows, the `⤶`, the 1 KiB cut, the duplicates, the secret rows with `!` and `&` input,
  the replacement after a yank, the rotation, cycling in isearch and in a prompt past a secret
  entry; the read-only check before the clipboard; `get-state` and `screenshot` with the float open;
  a dropped worker answer; and the keys in `keys.rs`.
- `just e2e` flows: `C-> C->` then typing, `C-g C-g`; isearch `C-c C-<`; `C-x SPC`; `M-y` with
  `--clipboard memory`, picking an older entry with three cursors.
- `text_perf`'s four cases above, before and after the batched pass, and `C-c C-<` on a word with
  1000 matches in 100 MB; the `just perf` scenario once the author agrees to it.

## Questions

Put to the author on 2026-11-09, the recommended option first; each answer was the first.

1. **The order of `C-g`:** (a) the regions, then the extra cursors, as multiple-cursors.el;
   (b) the cursors first, as ADR 0030.
2. **Skipping and removing:** (a) `C-M->` and `C-M-<` skip, `M--` before `C->` or `C-<` removes;
   (b) multiple-cursors.el's counts only (`M-0`, `M--`); (c) neither yet.
3. **A cursor on each line:** (a) `C-x SPC` at once, as `mc/edit-lines`, the rectangle with M3;
   (b) ADR 0030's rectangle mode that turns into cursors before the next command.
4. **From isearch:** (a) the same keys, `C->` and `C-c C-<`, on the search's string; (b) `M-RET` for
   all matches only.
5. **The bound on cursors:** (a) 1000, with the edits batched in M2-21 to fit the budget; (b) 100
   now, raised later; (c) 1000 at 16.75 ms a key.
6. **`M-y`'s preview:** (a) the entry's text in the float; (b) a trial insertion in the buffer, as
   consult does; (c) none.
7. **Secret entries in `M-y`:** (a) listed, with no text and no preview, and picking them works;
   (b) not listed; (c) shown as any other.
8. **`M-y` in isearch and in a prompt:** (a) cycling right after a yank; (b) the float there too,
   over isearch; (c) not yet.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. When this ADR is
accepted, the status of ADR 0030, ADR 0071, ADR 0072, ADR 0074 and ADR 0082 gains
`amended by 0091`.

- [ADR 0030](0030-m2-editing-commands.md), "`C-g`: Remove extra cursors first, then deactivate the
  mark, then quit the minibuffer": `C-g` follows ADR 0072's chain, the region first and the extra
  cursors last.
- ADR 0030, "`C-x SPC` (rectangle) followed by a command puts a cursor on each line": `C-x SPC`
  puts a cursor on each line of the region at once; a rectangle that shows first comes with M3.
- ADR 0030, "Super (Option) + click adds a cursor": it waits for mouse clicks.
- ADR 0030, "It uses fuzzy matching": it uses ADR 0076's orderless matcher, over an entry's first
  1 KiB.
- ADR 0030, under Alternatives, "Emacs `M-y` cycling after `C-y`: less discoverable than picking
  from a list": in a buffer `M-y` picks from a list, but in isearch and in a prompt it cycles.
- [ADR 0071](0071-mark-and-region.md), "`C-g` hides the region now; removing extra cursors first
  comes with the multiple-cursor keys (M2-21)": the region still goes first, and the extra cursors
  go last, after the prompts and isearch.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), "Each `C-g` does one thing,
  the first that applies": it also drops a search for cursors still waiting on the worker.
- [ADR 0074](0074-the-kill-ring-and-the-clipboard.md), "Reading only on a yank": also when `M-y`
  opens its float in a buffer, before any entry is picked, so macOS's notice can appear on an `M-y`
  that `C-g` then closes.
- [ADR 0082](0082-isearch.md), "A key it does not bind ends the search where it is, as `RET` does,
  and then runs as it would have": with `C-c` now a prefix in isearch, an unbound sequence under it
  does the same, resolved whole in the other keymaps.

## Consequences

- The keys are multiple-cursors.el's, so `C->`, `C-<`, `C-c C-<` and `C-g` work as they do there,
  but for whole words when the string came from a word, and a run of `C->` presses ends with `C-g`
  twice.
- A search for the next occurrence never blocks the UI thread, but a far one arrives a moment
  later, and is dropped if the person edits meanwhile.
- `C-c C-<` on a common word in a large file refuses rather than covering part of it, and waits for
  a load to end. The bound is 1000 until the batched pass shows a higher one fits.
- The batched pass changes the edit core: one pass per command for every edit with several
  cursors, which a single cursor must not pay for, and clipping only near edits in every window.
- `M-y` reads the clipboard once, as `C-y` does, so another app's copy is in its list, and macOS
  may show its notice for an `M-y` that inserts nothing.
- `M-y`'s rows show only the first 1 KiB of an entry, and match on it: text deeper in a long entry
  does not find it. Two equal entries over 1 KiB may both be listed.
- A secret entry stays off the screen in `M-y`'s list and out of cycling, but a person can still
  pick it from the list or yank it with `C-y`.
- Over the channel, a client can add cursors in an ordinary buffer but cannot pick from the kill
  ring; the channel's `state` gives a prompt's kind only, and the dev `get-state` leaves `M-y`'s
  completion out.

## Alternatives considered

- **`C-g` removing the cursors first** (ADR 0030): after `C->`, the regions could not be dropped
  without losing the cursors.
- **A first `C->` that only selects the word,** as VS Code's `Cmd+D`: one more press, and not what
  ADR 0030 says.
- **A cursor on the next line for `C->` with no region,** as `mc/mark-next-like-this` does:
  `C-x SPC` covers lines, and ADR 0030 asks for the word.
- **Only multiple-cursors.el's counts for skipping and removing:** fewer keys, but `M-0` and `M--`
  are two-key chords before every skip.
- **The rectangle mode of ADR 0030 now:** it needs a rectangle drawn and grown, and a rule for which
  commands turn it into cursors; M3's rectangle commands need that anyway.
- **`M-RET` in isearch for all matches:** one more key to learn, and no way to add matches one by
  one from a search.
- **Wrapping past the buffer's end,** as VS Code does: with `C-<` beside it, not wrapping keeps the
  cursors in one direction from where the run began.
- **Case folding as isearch's smart case for `C->`:** a rename needs the exact string;
  multiple-cursors.el searches case-sensitively.
- **Adding the first 1000 of more matches:** a rename would cover only part of the buffer, without
  the person seeing where it stopped.
- **100 cursors now:** fits the budget today, but a rename across a large file would not reach.
- **1000 cursors at 16.75 ms a key (25.8 ms at p99):** over the budget the project holds itself to.
- **The edit alone within 8 ms:** it would leave nothing of the budget for the frame.
- **A trial insertion in the buffer for the preview,** as consult does: the centred float usually
  covers the cursor, and an edit that comes and goes meets undo, saving, reloads and the secret
  rules.
- **No preview:** a row shows one line of an entry that may be many.
- **Leaving secret entries out of `M-y`:** safe, but a person who killed a key could not pick it.
- **Showing secret entries as any other:** puts a secret on the screen while browsing, which no
  preview does (ADR 0088).
- **Cycling through secret entries, masked:** the entry that stays would still have to land in the
  string or the prompt, and skipping them is simpler.
- **The float over isearch and prompts:** a prompt opening ends isearch (ADR 0082), and a float over
  a prompt is a second prompt deep; cycling is what Emacs did there for decades.
- **Matching `M-y` on the whole entry:** a key would cost as much as the ring's 64 MiB.
- **Removing duplicates by comparing whole entries:** opening the float could compare tens of MiB
  on the UI thread.
- **The whole ring's rows in `get-state`:** it would carry another app's copy and the count of secret
  entries, which ADR 0074 keeps out.
