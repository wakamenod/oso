# 0098. Where `M-y` lives

- Status: Accepted
- Date: 2026-11-09

## Context

[ADR 0091](0091-multiple-cursor-keys-and-m-y.md) placed `M-y`'s rows and preview in
`editor/kill.rs` and its text pane in `app/draw/float.rs`, and named the public items it would add.
Writing it put the code elsewhere, as the files' size limit and their single responsibilities ask,
and the reviews of that work settled four details ADR 0091 left open or put otherwise. Its
`just e2e` flow found that a pick moved past another app's copy did not stay first: the next yank
read the unchanged clipboard as new text, because the ring compared it only with its newest entry
([ADR 0074](0074-the-kill-ring-and-the-clipboard.md)).

## Decision

- **`oso-core`:** the list, its rows, its preview, the pick and the cycling in a prompt are in
  `editor/yank_pop.rs`, a private module; the cycling in isearch and isearch's `C-y` in
  `editor/isearch/yank.rs`, also private. `editor/kill.rs` only notes each yank for them.
- **`oso`:** the preview's panes, the buffer's and the kill's, are in `app/draw/float/preview.rs`,
  a private module beside `float.rs`.
- **Public items** besides those ADR 0091 names: `YankPopError`, which `yank-pop` and
  `isearch-yank-pop` return, and `Candidate::hidden_by_input`, which makes a row shown only while
  nothing is typed, as a secret entry's row is.
- **A secret entry is never compared** with another to list a text once: a plain row it hid would
  tell what it holds. Two equal secret entries are listed twice.
- **A row is cut to 1 KiB twice:** the entry's first 1 KiB, then the row once escaped, which can be
  several times longer. The input matches that row.
- **A `⤶` of the entry's own text is escaped,** so that each `⤶` in a row is a line break.
- **A list closed without an answer picks nothing,** and is forgotten.
- **The ring remembers what the clipboard was last seen to hold,** the fingerprint of the entry it
  got from the ring or gave it, wherever that entry is now, as Emacs compares the selection with
  the last one it saw. The same fingerprint again adds no entry, and marked concealed it makes
  that entry secret. So the next `C-y` after a pick yanks the pick, as ADR 0091 says.

## Changes to accepted ADRs

The passages below stay in their ADR as written; what holds now follows each. The status of
ADR 0091 becomes `Accepted, amended by 0092 and 0098`, and that of ADR 0074 `Accepted, amended
by 0085, 0090, 0091 and 0098`.

- [ADR 0091](0091-multiple-cursor-keys-and-m-y.md), "Where it lives", "the `M-y` prompt's
  candidates, rows and preview in `editor/kill.rs` and `minibuffer`": in `editor/yank_pop.rs` and
  `editor/isearch/yank.rs`, as above.
- ADR 0091, "the preview's text pane in `app/draw/float.rs`": in `app/draw/float/preview.rs`.
- ADR 0091, "The new public items ... are `Quit::Cursors`, `MAX_CURSORS`, `Preview::Text`,
  `ArgSpec::KillEntry` and the `Editor` methods": also `YankPopError` and
  `Candidate::hidden_by_input`.
- ADR 0091, "An entry equal to a newer one is listed once": unless either is secret.
- ADR 0091, "Matching is ADR 0076's orderless matcher over the row's text: the first 1 KiB of the
  entry": over the row's text, cut to 1 KiB again once escaped.
- [ADR 0074](0074-the-kill-ring-and-the-clipboard.md), "Text from another app equal to the newest
  entry adds no entry, but makes it secret if it comes back marked concealed": equal to what the
  clipboard was last seen to hold, the entry it got or gave, which a pick may have moved.

## Consequences

- No file passes the size limit, and `float.rs` keeps only the list.
- A secret entry's row may stand twice in the list.
- A key and a frame read at most 1 KiB of each row, whatever its entry holds.
- A large kill that never reached the clipboard no longer lets the clipboard's older text come
  back as a new entry on the next yank; the yank takes the kill.
- `KillRing::newest_written` and `KillRing::make_newest_secret` leave the public API, for
  crate-private ones that compare with what the clipboard was last seen to hold.

## Alternatives considered

- **Keep ADR 0091's layout**, the rows in `editor/kill.rs` and the pane in `float.rs`: both files
  would pass the size limit, and the kill ring's module would also hold a prompt.
- **Compare secret entries among themselves only,** keyed by whether each is secret: a secret
  entry's text would still go into the set, and the gain is a duplicate row.
- **Write the pick to the clipboard,** as Emacs's `yank-pop-change-selection` t: ADR 0091 keeps the
  clipboard as it is, and another app's copy would be lost from it.
- **Cut only the entry** and leave the escaped row whole: a row of control characters would make
  each key and frame read several times as much.
