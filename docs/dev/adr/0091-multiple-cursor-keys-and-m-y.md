# 0091. Multiple-cursor keys and `M-y`

- Status: Proposed
- Date: 2026-11-09

## Context

M2-21 brings the keys that add cursors, and `M-y` from the kill ring
([roadmap](../roadmap.md)). Commands already work on the window's selection set, so this step is
about keys, behaviour and the float, not about the core model. Earlier ADRs fixed the outline:

- each window has a selection set, and cursors are added at the next or previous occurrence of the
  word or region, on a rectangle, at all search matches, or with a modifier and a click
  ([ADR 0007](0007-multi-cursor.md));
- the keys are multiple-cursors.el's: `C->` and `C-<` add a cursor at the next or previous
  occurrence of the region or the word at point, `C-c C-<` at all of them, `C-x SPC` (a rectangle)
  followed by a command a cursor on each line, a key in isearch on all matches, and Super with a
  click a cursor; `C-g` removes extra cursors first; `M-y` picks from the kill ring in the
  minibuffer, consult-yank-pop style, and inserts the entry, or one piece per cursor
  ([ADR 0030](0030-m2-editing-commands.md));
- each cursor has its own mark, the window shows all regions or none, and regions that overlap
  merge ([ADR 0071](0071-mark-and-region.md));
- a kill with N cursors stores N pieces as one entry, a yank with as many cursors puts each piece
  at its cursor and joins them with line breaks otherwise, a yank reads the clipboard first and
  waits for it without blocking, and a secret entry makes the buffer it is yanked into secret
  ([ADR 0074](0074-the-kill-ring-and-the-clipboard.md));
- completion runs in the centred float, and `C-x b` and `C-.` split it into a list and a preview
  that never shows a buffer that may hold secrets ([ADR 0076](0076-completion-and-m-x.md),
  [ADR 0088](0088-switching-and-killing-buffers.md));
- isearch has a literal matcher with smart case in `oso_core::search`, searches 256 KiB on the UI
  thread and the rest on a worker, moves only the primary cursor, and leaves `M-y` and the key that
  puts a cursor on every match to M2-21 ([ADR 0082](0082-isearch.md));
- every command declares its reach, and the channel refuses `Person`
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).

What is still open: typing with 1000 cursors takes about 15 ms a key, over the 8 ms budget, and
`open-questions.md` asks to batch the edits or cap the cursors before M2-21. The editor has no
mouse clicks yet.

## Decision

The author chose on 2026-11-09 the first option of each question under [Questions](#questions),
which the text below describes. The rest are defaults put to them with this ADR.

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
  case-sensitive, as multiple-cursors.el searches. With no region shown, the first `C->`, `C-<` or
  `C-c C-<` selects the word around each cursor (ADR 0058's word), shows the regions and adds
  nothing (`C-c C-<` then goes on to add); occurrences of a string taken from a word must be whole
  words, as VS Code's `Cmd+D` does, so `id` does not find `idle`. With no word at the primary
  cursor it signals "No word at point".
- **The window keeps that query** (the string, the case rule and whether it is a whole word) while
  it has more than one cursor, so a run of these keys searches for the same thing. It goes when the
  cursors are back to one or the text changes. A query of more than 1024 characters signals "Region
  too long to match", as isearch's string is bounded (ADR 0082).
- **Where they search:** `C->` from the end of the last cursor in buffer order, `C-<` back from
  the start of the first. They do not wrap: past the end they say "No more matches". The skips
  start from the same places, and `C-M->` with one cursor moves it, as a plain search would.
- **The new cursor's region covers the match,** in the direction of the primary's, and the new
  cursor becomes the primary, so the view follows it. The others stay. `C-c C-<` keeps the primary
  where it was.
- **A count:** a positive N adds N occurrences, as N presses do; `M--` before `C->` removes
  the last cursor in buffer order, and before `C-<` the first, as multiple-cursors.el's negative
  argument does; `M-0` skips, as `C-M->` does. With one cursor, removing says "Only one cursor".
- **The search is isearch's matcher.** Each key searches 256 KiB of text on the UI thread; if that
  finds nothing and the text goes on, the rest is searched on the isearch worker, over a snapshot,
  and the echo area says `Searching…`. The cursor is added when the answer comes, if the buffer's
  revision and the window's cursors are still as they were; otherwise the answer is dropped with
  "Search dropped: the buffer changed". `C-c C-<` always finds its matches on the worker past
  256 KiB, the way isearch counts them.
- **`edit-lines`** puts one cursor on each line from the mark's line to the primary cursor's line,
  at the primary cursor's goal column, or at a shorter line's end, as multiple-cursors.el's
  `mc/edit-lines` does. It uses the region if one shows, and otherwise the mark, as `M-w` does; with
  no mark it signals "The mark is not set now". It hides the regions and keeps the primary on the
  cursor's line. With several cursors it starts from the primary alone. A rectangle that shows and
  grows before it turns into cursors comes with M3's rectangle commands.
- **At most 1000 cursors** in a window. A key that would pass it adds nothing and says "At most
  1000 cursors"; `C-c C-<` with more matches says "1234 matches; at most 1000 cursors" and adds
  none, so a rename never covers only part of the buffer. `edit-lines` over more lines says the
  same. The bound is a constant in `oso-core`, beside the selection set.
- **A cursor with a click** waits for mouse clicks, which oso does not take yet.

### `C-g` with several cursors

- **`C-g` hides the regions first, then removes the extra cursors,** as multiple-cursors.el's
  `mc/keyboard-quit` does: after `C-> C->`, the first `C-g` leaves three cursors with no region, so
  `C-a` and typing work at all of them, and the second leaves the primary alone and says nothing.
- **The order in `Editor::quit`** is: the region, the innermost prompt, isearch's two steps, then
  the extra cursors (`Quit::Cursors`). So `C-g` in a prompt over a window with several cursors
  closes the prompt and keeps the cursors, and `C-g` during a search ends the search first.

### From isearch

- **`C->` in isearch** ends the search with the current match as the primary's region, and adds a
  cursor at the next match, as `C->` does after it. **`C-<`** adds one at the previous match. **`C-c
  C-<`** ends the search and puts a cursor on every match in the buffer, the primary on the current
  one. Each ends the search as `RET` does, with the string in the history.
- **The query is isearch's:** its string, with its smart case or the case `M-c` chose, and not a
  whole word. Later `C->` presses keep that query.
- They are isearch's own commands, `isearch-mark-next`, `isearch-mark-previous` and
  `isearch-mark-all`, in its keymap, so they can say they end a search.

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
  ring:`, with no history. First it reads the clipboard, as `C-y` does (ADR 0074), so another app's
  newest copy is in the ring and at the top; input from a socket never reads the system clipboard.
  A read-only buffer signals "Buffer is read-only" before the float opens, and an empty ring "Kill
  ring is empty".
- **The list** is the ring, the newest first, each text once at its newest place. ADR 0076's
  history, length and alphabetical order do not apply.
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
  annotation's face, it holds no text for the matcher, so any input hides it, and its preview says
  `may hold secrets`. Picking it inserts it, and the buffer becomes secret, as ADR 0074 says.
- **`RET`** inserts the entry as `C-y` does: at every cursor, one piece per cursor when the counts
  match and all of them joined otherwise, within the 16 MiB bound, as one undo step, with each mark
  at the start of what it inserted and the region hidden. **Right after a `C-y` or `M-y`,** whose
  text is still there unchanged, it replaces that text instead, as consult-yank-pop does.
- **The entry picked moves to the front of the ring,** as consult's `consult-yank-rotate`, so the
  next `C-y` yanks it. The clipboard is not written, as Emacs's `yank-pop-change-selection` nil.
- `C-g` closes the float and changes nothing.

### `M-y` in isearch and in a prompt

- **Right after a yank it cycles,** as Emacs's `yank-pop` always did: `isearch-yank-pop` in isearch
  replaces what the last `C-y` or `M-y` added to the string with the next older entry, its pieces
  joined and cut to 1024 characters as `C-y` there does; `yank-pop` in a prompt replaces the text
  the last yank inserted. Repeating goes further back, and past the oldest entry back to the newest.
- **Otherwise they signal** "Previous command was not a yank". No float opens over isearch or over
  a prompt.
- A secret entry yanked into isearch makes the searched buffer secret, as `C-y` there does
  (ADR 0082).

### Several cursors, kills and yanks

These follow ADR 0007 and ADR 0074 unchanged: a kill with N cursors stores N pieces as one entry,
a yank and `M-y` with as many cursors put each piece back at its cursor, and any other count joins
them with line breaks. Typing over regions and `M-w` work at every cursor.

### Batching the edits

- **A command's edits at all cursors are applied in one pass over the rope's tree,** in place of
  one `Rope::replace` each, and the display map is updated in the same pass. A command clones
  the selection set once, where an edit now clones it a few times.
- **The target:** typing at 1000 cursors in the 100 MB file of ADR 0071's measures within the 8 ms
  budget at p50, from 15 ms now, without slowing a single cursor. The implementing pull request
  reports the numbers; if 1000 cursors still miss the budget, the bound goes back to the author.
- The pass is a private module beside `rope`, since `rope.rs` is at its size limit.

### Reach and the sockets

- **`mark-next-like-this`, `mark-previous-like-this`, the two skips, `mark-all-like-this` and
  `edit-lines` are `Look`** (ADR 0085): they move and mark, and change no text. **The first five
  refuse from the channel in a buffer that may hold secrets,** "it would search a buffer that may
  hold secrets; ask the person", since where they land and what they say would tell what the
  buffer holds.
- **Messages that say a search found something** (`No more matches`, the count of `C-c C-<`) go
  to the echo area only, not to `*Messages*`, while a buffer that may hold secrets is shown, as
  isearch's do (ADR 0082).
- **`consult-yank-pop`, `yank-pop`, `isearch-yank-pop` and the three isearch commands are
  `Person`:** they read the clipboard or pick in a list, or belong to isearch, whose commands are all
  `Person`.
- **`get-state`** gives the `M-y` prompt the kind `complete` and its label, and no `completion`
  rows and no preview: the rows would carry another app's copy, and their number the count of
  secret entries, which ADR 0074 leaves out. A test picks with `RET` and reads the buffer with
  `get-text`.

### Where it lives

- **`oso-core`:** `editor/cursors.rs`, a private module, with the query a window keeps and the
  `Editor` methods behind the commands; the search through `oso_core::search` (still private) and
  the isearch worker's `SearchJob`; `Quit::Cursors`; `selection::MAX_CURSORS`; the batched pass
  beside `rope`; and the `M-y` prompt's candidates, rows and preview in `editor/kill.rs` and
  `minibuffer`, with a `Preview::Text` beside `Preview::Buffer` and a `ArgSpec::KillEntry` that
  reads the clipboard and then the entry, as `CountAndClipboard` reads the clipboard.
- **`oso`:** `commands/cursors.rs` and `bindings/cursors.rs`; `consult-yank-pop` and `yank-pop` in
  `commands/kill.rs`; the isearch commands in `commands/isearch.rs`; the preview's text pane in
  `app/draw/float.rs`.
- The new public items, as the API snapshots show, are `Quit::Cursors`, `MAX_CURSORS`,
  `Preview::Text`, `ArgSpec::KillEntry` and the `Editor` methods. No new module is public.

### Tests and measures

- Unit tests for each rule above: the query from a region, a word and isearch; whole words; no
  wrap; counts, skips and removal; the bound for each way of adding; `edit-lines` with short lines,
  a hidden region and no mark; the `C-g` order with a prompt and with isearch; the `M-y` rows, the
  `⤶`, the 1 KiB cut, the secret rows, the replacement after a yank, the rotation, cycling in
  isearch and in a prompt; the refusals from the channel; and a dropped worker answer.
- `just e2e` flows: `C-> C->` then typing, `C-g C-g`; isearch `C-c C-<`; `C-x SPC`; `M-y` with
  `--clipboard memory`, picking an older entry with three cursors.
- The typing benchmark with 1000 cursors, before and after the batched pass, and `C-c C-<` on a
  word with 1000 matches in 100 MB.

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
   now, raised later; (c) 1000 at 15 ms a key.
6. **`M-y`'s preview:** (a) the entry's text in the float; (b) a trial insertion in the buffer, as
   consult does; (c) none.
7. **Secret entries in `M-y`:** (a) listed, with no text and no preview, and picking them works;
   (b) not listed; (c) shown as any other.
8. **`M-y` in isearch and in a prompt:** (a) cycling right after a yank; (b) the float there too,
   over isearch; (c) not yet.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. When this ADR is
accepted, the status of ADR 0030 and ADR 0071 gains `amended by 0091`.

- [ADR 0030](0030-m2-editing-commands.md), "`C-g`: Remove extra cursors first, then deactivate the
  mark, then quit the minibuffer": `C-g` hides the regions first, then closes the innermost prompt,
  then takes isearch's steps, and only then removes the extra cursors.
- ADR 0030, "`C-x SPC` (rectangle) followed by a command puts a cursor on each line": `C-x SPC`
  puts a cursor on each line of the region at once; a rectangle that shows first comes with M3.
- ADR 0030, "Super (Option) + click adds a cursor": it waits for mouse clicks.
- [ADR 0071](0071-mark-and-region.md), "`C-g` hides the region now; removing extra cursors first
  comes with the multiple-cursor keys (M2-21)": the region still goes first, and the extra cursors
  go last, after the prompts and isearch.

## Consequences

- The keys are multiple-cursors.el's, so `C->`, `C-<`, `C-c C-<` and `C-g` behave as people used
  to it expect, and a run of `C->` presses ends with `C-g` twice.
- A search for the next occurrence never blocks the UI thread, but a far one arrives a moment
  later, and is dropped if the person edits meanwhile.
- `C-c C-<` on a common word in a large file refuses rather than covering part of it. The bound
  is 1000 until the batched pass shows a higher one fits.
- The batched pass changes the edit core: one tree pass per command for every edit with several
  cursors, which a single cursor must not pay for.
- `M-y` reads the clipboard once, as `C-y` does, so another app's copy is in its list.
- `M-y`'s rows show only the first 1 KiB of an entry, and match on it: text deeper in a long entry
  does not find it.
- A secret entry stays off the screen in `M-y`'s list, but a person can still yank it.
- Over the channel, a client can add cursors in an ordinary buffer but cannot pick from the kill
  ring, and `get-state` says nothing of `M-y`'s rows.

## Alternatives considered

- **`C-g` removing the cursors first** (ADR 0030): after `C->`, the regions could not be dropped
  without losing the cursors.
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
- **1000 cursors at 15 ms a key:** over the budget the project holds itself to.
- **A trial insertion in the buffer for the preview,** as consult does: the centred float usually
  covers the cursor, and an edit that comes and goes meets undo, saving, reloads and the secret
  rules.
- **No preview:** a row shows one line of an entry that may be many.
- **Leaving secret entries out of `M-y`:** safe, but a person who killed a key could not pick it.
- **Showing secret entries as any other:** puts a secret on the screen while browsing, which no
  preview does (ADR 0088).
- **The float over isearch and prompts:** a prompt opening ends isearch (ADR 0082), and a float over
  a prompt is a second prompt deep; cycling is what Emacs did there for decades.
- **Matching `M-y` on the whole entry:** a key would cost as much as the ring's 64 MiB.
- **The whole ring's rows in `get-state`:** it would carry another app's copy and the count of secret
  entries, which ADR 0074 keeps out.
