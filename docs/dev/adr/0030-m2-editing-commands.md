# 0030. M2 editing command semantics

- Status: Accepted, amended by 0087, 0091 and 0111
- Date: 2026-11-03

## Context

M2 is where daily editing starts ([`roadmap.md`](../roadmap.md)). Commands
must behave the way the author's current Emacs setup does, adapted to
multiple cursors ([ADR 0007](0007-multi-cursor.md)). Only behavior is decided
here. How key bindings are written in config belongs to M1-3 and
[ADR 0019](0019-configuration.md).

## Decision

### Behavior carried over from the author's setup

| Keys / situation | Behavior |
|---|---|
| `C-w` | Kill the region if active, otherwise delete the previous word |
| `C-k` at line start | Kill the whole line, including its newline (`kill-whole-line`) |
| Typing over an active region | Replace the region (`delete-selection-mode`) |
| `M-f` / `M-b` | Also stop at camelCase boundaries (`subword-mode`) |
| `C-n` / `C-p` | Move by logical line; stay at line end once there (`track-eol`) |
| `M-,` / `M-/` | Previous / next buffer |
| `C-,` | Pop to the previous mark |
| `C-g` | Remove extra cursors first, then deactivate the mark, then quit the minibuffer |
| Saving | Ensure a final newline; indentation never inserts tabs |

### New decisions

- **Undo is linear** (undo-fu style): `C-z` undoes and `C-M-z` redoes. Any
  edit after an undo discards the redo branch.
  - One command is one undo step across all cursors.
  - Consecutive self-inserted characters are grouped into one step, as in
    Emacs.
- **`M-y` picks from the kill ring in the minibuffer**, consult-yank-pop
  style. It uses fuzzy matching and inserts the chosen entry, or one piece per
  cursor for multi-cursor kills.
- **Japanese word motion follows Emacs:**
  - Words break where the script changes: kanji, hiragana, katakana
    (including `ー`), Latin letters and digits, full-width alphanumerics,
    punctuation.
  - Kanji followed by hiragana or katakana stays one word, as Emacs's
    `word-combining-categories` does, so okurigana stays attached: `書く` is
    one word, and `日本語の|文章で` splits as shown.
  - No dictionary-based segmentation.
- **Multiple-cursor keys**, in the style of multiple-cursors.el:
  - `C->` / `C-<`: add a cursor at the next / previous occurrence of the
    region or the word at point.
  - `C-c C-<`: add cursors at all occurrences.
  - `C-x SPC` (rectangle) followed by a command puts a cursor on each line.
  - A key in isearch puts cursors on all matches.
  - Super (Option) + click adds a cursor.
- **`C-a` / `C-e` (mwim):**
  - In M2, `C-a` toggles between line start and the first non-blank
    character. `C-e` toggles between line end and the end before trailing
    whitespace.
  - In M3, tree-sitter makes `C-e` also stop before a trailing comment.
- **`C-x C-f` completes by path segment** (partial-completion style):
  `~/P/S/o` expands segment by segment, with fuzzy matching inside each
  segment.
- **isearch uses smart case:** case-insensitive unless the query contains an
  uppercase letter.

### Placement

These are not in M2. They go to M3:

- query-replace (`M-%`, with a match count)
- keyboard macros
- whitespace cleanup on save
- window splits (already in M3)

## Consequences

- Behavior matches the author's Emacs closely, so switching for daily text
  editing is low-friction.
- Every command above must be implemented and tested for selection sets, not
  single cursors.
- Script-based word motion needs a Unicode script classification table.
  The table can be generated from Unicode data without a new runtime
  dependency.

## Alternatives considered

- **Undo tree or Emacs's undo ring:** keeps every branch, but is harder to
  use than the author's current linear undo.
- **Emacs `M-y` cycling after `C-y`:** less discoverable than picking from a
  list.
- **Dictionary-based Japanese word segmentation:** more accurate, but needs a
  large dictionary and a dependency.
- **Splitting only at punctuation and spaces:** too coarse for Japanese
  prose.
- **A dedicated multiple-cursor menu:** the menu component arrives only in M3.
  It may be added on top of the keys later.
- **VS Code-style `Cmd+D`:** `Command` is Meta, so it would collide with
  `M-d`.
- **Comment-aware `C-e` in M2 using per-mode comment markers:** misfires on
  markers inside strings. Waiting for tree-sitter is more reliable.
