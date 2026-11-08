# 0076. Completion and `M-x`

- Status: Proposed
- Date: 2026-11-08

## Context

M2-17 builds completion in the minibuffer: the orderless matcher, 18 candidate rows, annotations,
history and `M-x` ([roadmap](../roadmap.md)). M2-18 (`C-x C-f`) and M2-19 (`C-x b`, with a preview)
build on it, and so do M2-21's `M-y` from the kill ring, `C-.` (`consult-mark`, ADR 0071) and M3's
consult-like commands. The author uses vertico, orderless, marginalia and consult in Emacs every day,
so the defaults follow them.

What is fixed already:

- [ADR 0008](0008-floating-ui.md) and [ADR 0026](0026-m2-ui-layout.md): completion shows in the
  minibuffer's float, 18 candidate rows by default, configurable; a command with a preview splits the
  popup later, and `M-x` is one column. `ui.md` asks for orderless-style matching and
  marginalia-style annotations on the right.
- [ADR 0034](0034-command-registry-and-keymaps.md): the registry iterates its commands with their docs,
  for completion. Commands are plain functions with no state, `Args` is `Copy`.
- [ADR 0051](0051-m2-plan-decisions.md): the matcher is written in `oso-core`, as `nucleo-matcher` is
  MPL-2.0, which `deny.toml` refuses. Regular expressions come only from the `regex` crate.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), "What this leaves room for":
  candidates are rows below the answer in the same float, which grows down so the answer's row stays
  put; `M-x` comes through a new `ArgSpec`; `C-n`/`C-p` move through candidates; the selected candidate
  needs a background per row; candidates from a worker update the float through the `Status`
  comparison. Histories of 100 answers, walked by `M-p`/`M-n`, exist, and a prompt opened while a buffer
  that may hold secrets is shown reads and records none.
- [ADR 0075](0075-a-float-as-a-panel-attached-to-the-top-edge.md) (Proposed, PR #58): the float is a
  panel attached to the top edge, with the label on rows of its own, a gap, the answer in a field, and a
  row of padding below, and an order in which a short frame gives them up. Its rounded-box pipeline is
  there for M2-17's selected candidate. Every color goes through a `Face` or `PanelStyle` read every
  frame, so that themes switch at run time (M2-23).
- Budgets: 8 ms from key to photon (p99), and `prompt_type_p99_ms` reported for keys typed into a
  prompt.

## Decision

The author chose, through the coordinator, on 2026-11-08, the first option of each of the ten
questions under [Questions](#questions), which the text below describes. They also approved `regex`
as a direct dependency of `oso-core`, the new public module `complete` and its line in
`guardrails/layers.txt`, `ArgSpec::Command` and `ArgSpec::Key`, `Keymap::bindings`, `Kind::Complete`,
the four `Face` variants, `Panel::selected`, and `get-state`'s `completion` field. The colors of the
match and the selected row are picked from screenshots later, as ADR 0075's were. The ADR stays
Proposed until the pull request.

### The matcher

A new public module, `oso_core::complete`, holds the matcher and a list of candidates. It depends
on nothing of the editor, so M2-18's worker can run it on a snapshot.

- **The input is split at spaces into components,** and a candidate matches when every component
  matches it, in any order, as in orderless. A backslash before a space makes it part of a
  component, as orderless's default separator (`orderless-escapable-split-on-space`) does, and an
  empty input matches everything.
- **Each component matches as a literal substring, or as a regular expression** when it holds a
  character that means something in one, as orderless's default styles (`orderless-literal`,
  `orderless-regexp`) do. A component that is no valid expression, or one over the compile limit
  below, matches as a literal.
- **An affix picks one style,** as orderless 1.x's `orderless-affix-dispatch` does, at the start or
  the end of a component:

  | Affix | Style | `foo` matches |
  |---|---|---|
  | `!` | not: the candidate must not match the rest | `!buf` drops `save-buffer` |
  | `,` | initialism: the letters start words, in order | `,sbk` → `save-buffers-kill-terminal` |
  | `=` | literal only | `=a.b` matches `a.b` and not `axb` |
  | `^` | literal prefix | `^save` |
  | `~` | flex: the letters in order, anything between | `~fwc` → `forward-char` |
  | `&` | matches the annotation instead of the candidate | `&region` → commands whose doc says it |

  `%` (char folding) is left out until migemo (M3) decides how Japanese is matched.
- **Case is smart over the whole input** (`orderless-smart-case`, and isearch in M2-20): an input
  with no upper-case letter ignores case, and one with any matches it exactly.
- **Regular expressions are compiled once per input,** with `regex::RegexBuilder`'s size limit set to
  64 KiB and its nest limit to 32, so that a component pasted in cannot take long to compile; the
  `regex` crate matches in linear time. Literals are matched with a lowercase copy of each candidate
  made once, when the list is given, so a key typed costs no allocation per candidate.
- **What matched is highlighted only in the rows shown,** so highlighting costs 18 rows, never the
  list.

### Ranking

- **The order is fixed when the prompt opens, and filtering keeps it,** as vertico's default
  `vertico-sort-history-length-alpha`: first the candidates in the prompt's history, the most recent
  first, then the rest by length, then alphabetically. Typing only removes candidates, so what stays
  never moves, and nothing is scored.
- **An exact match of the input goes first,** as vertico moves it to the front, so typing a whole name
  selects it.
- **The history is the prompt's own** (ADR 0072): `M-x`'s is `command`. A prompt opened while a buffer
  that may hold secrets is shown neither reads nor records it, so its order is length and then
  alphabetical.

### Where matching runs

- **On the UI thread, up to 5000 candidates,** which every list of M2-17 and M2-19 is: `M-x` has about
  60 commands now and a few hundred later. The budget is 1 ms of matching per key at 5000 candidates on
  the M1, measured in `just perf`, so `prompt_type_p99_ms` stays where it is.
- **Narrowing reuses the last result:** an input that only adds characters to the last one filters the
  candidates that matched before, not the whole list.
- **Past 5000, on a worker thread, built with M2-18,** whose directories are the first list that can be
  that long. What is fixed now: each input has a generation, the worker gets the list as an
  `Arc<[Candidate]>` snapshot with the input and its generation, and the UI takes a result only if its
  prompt is still open and its generation is the newest; any other result is dropped. Until a result
  comes the rows shown stay, and the count shows `…`. Results arrive through the `Status` comparison,
  as a load's progress does (ADR 0072). Nothing in M2-17 reaches the worker path.

### The float

The float of ADR 0075, with the candidate rows below the answer's field:

```
┌──────────────────────────────────────────────────────────┐ ← panel, from the top edge
│  M-x                                              3/57   │ label, with the count at its end
│                                                          │ gap
│  ╭ save█ ───────────────────────────────────────────╮    │ field
│                                                          │ padding
│  save-buffer  C-x C-s            Save the current buffer │ ← selected (a rounded box)
│  save-buffers-kill-terminal  C-x C-c     Offer to save … │
│  ...                                                     │ up to 18 rows
│                                                          │ padding
╰──────────────────────────────────────────────────────────╯
```

- **Up to 18 candidate rows** below the field's padding row, then a row of padding. The number is a
  constant, `complete::ROWS`, until configuration (M3) sets it.
- **The float grows to fit the matches and never shrinks while the prompt is open,** as vertico's
  default `vertico-resize` (`grow-only`): the answer's row stays put, the panel's bottom edge does not
  jump while typing narrows the list, and a prompt with few candidates covers little.
- **The count** shows at the end of the label's first row, `3/57`: the selected candidate's place and
  the number of matches, in the annotations' dim face, as `vertico-count-format` does. Its cells are
  kept for the widest count the list can show, so the label does not break again as the count changes.
  No match shows `0/0`.
- **A row:** the candidate from the inner cells' first column, then the binding one cell after it in
  the key face, then the annotation, aligned to the right edge of the inner cells, in a dim face.
- **When a row is narrow,** the annotation is cut first, with `…`, and goes below 8 cells; then the
  binding goes; then the candidate is cut with `…` at the edge. The candidate is never cut before the
  annotation and the binding have gone.
- **The selected row** is a rounded box from ADR 0075's pipeline, in `Layer::FLOAT` under the text, one
  row tall and 6 points wider than the inner cells on each side, inside the panel, radius 4 points, in
  `Face::Selected`'s background. The text keeps
  its faces on it. So `PaneContent::Lines` needs no background per row, and the open question about it
  closes. The candidates' rows are one pane of `PaneContent::Lines` in `Layer::FLOAT_CONTENT`, which
  keeps no bands into the panel's padding but the bottom one, so no glyph draws over another pane of the
  float (ADR 0072).
- **The list scrolls** to keep the selected row in view, with no margin, as vertico does.
- **A short frame gives up, in this order:** candidate rows down to one; the padding below them; the
  last candidate row; then ADR 0075's order (the padding under the field, the gap, the label's last
  rows, the one-row form). The count stays on the label's first row while it shows, and on the
  one-row form at the end of the row.

### Keys

In a prompt that completes, a keymap above ADR 0072's minibuffer keymap:

| Key | Command | |
|---|---|---|
| `C-n`, `<down>` | `next-completion` | The next candidate; at the last it stays (`vertico-cycle` nil). |
| `C-p`, `<up>` | `previous-completion` | The one before. |
| `M-<`, `M->` | `first-completion`, `last-completion` | The first and last, as vertico remaps them. |
| `C-v`, `M-v` | `scroll-completions-up`, `-down` | A page of rows. |
| `TAB` | `insert-completion` | Puts the selected candidate in the answer, as `vertico-insert`. |
| `RET` | `exit-minibuffer` | Gives the selected candidate; with none, says `[No match]` and stays. |

- **`RET` gives the selected candidate,** also with an empty answer, so `M-x RET` runs the command run
  last, as vertico does. A prompt that takes any text (M2-18's new file) gives the text as typed with
  `M-RET` (`vertico-exit-input`); `M-x` takes only a command, so it has no use for it yet.
- **`C-g`** stays ADR 0072's chain: the region in the answer, then the prompt.
- **`M-p`/`M-n`** walk the history as before, and the candidates follow the answer they put.
- A prefix argument typed in the prompt moves by that many candidates, as `C-u 3 C-n` does in Emacs.

### `M-x`

- **`ArgSpec::Command { prompt, history }`** reads a command's name in a prompt that completes over the
  registry. `execute-extended-command`, on `M-x`, is the one command with it. Once the name is given, the
  dispatcher runs that command in its place, through `run_or_read`, with `M-x`'s prefix argument, as
  Emacs passes `current-prefix-arg`: `C-u 3 M-x forward-char` moves 3, and `M-4 M-2 M-x goto-line`
  goes to line 42 with no prompt. A command that reads an answer opens its prompt after `M-x`'s
  closes. The label shows the prefix, `C-u M-x` or `3 M-x`, as Emacs's does.
- **Listed:** every registered command, except those that read the key that ran them
  (`self-insert-command` and `digit-argument`), which `ArgSpec::Key` marks, and those
  bound only in a keymap that is not active beneath the prompt, such as a question's `answer-yes` or the
  copies list's keys outside it: Emacs 28's `command-completion-default-include-p`. Dev commands are
  only in a dev build's registry, so only a dev build lists them.
- **The binding** shown is the shortest key sequence for the command in the layers active beneath the
  prompt, as `where-is` finds it, found once when the prompt opens by walking the keymaps
  (`Keymap::bindings`, new). A command with none shows none.
- **The annotation** is the first line of the docstring.
- **`run-command execute-extended-command`** is refused, "run the command itself": the socket names a
  command directly. A command marked `ArgSpec::Key` takes a count from `run-command` as one marked
  `Count` does.

### Secrets

- **No history** is read or recorded in a prompt opened while a buffer that may hold secrets is shown,
  or answered with one beneath it (ADR 0072), so the candidates' order says nothing of earlier answers.
- **A passphrase prompt never completes.**
- **Annotations come from the registry,** which holds no text of a buffer. M2-19's buffer names and
  M2-18's paths decide what they show of a secret buffer when they come.
- **`get-state`** gives a prompt that completes the kind `complete`, and `completion`: `count`, `total`, `selected` and
  `rows`, the names of the candidate rows shown, at most 18. While a buffer that may hold secrets is
  shown it is left out with the label and the answer, as the set of matches tells the answer.

### Theme

- **New faces,** with defaults in `pane.rs`'s tables, which M2-23 turns into tokens of a theme that
  switches at run time; no color is cached in an instance or a shader (ADR 0075):

  | Token | Face | Use |
  |---|---|---|
  | `float.selection.background` | `Selected` | The selected row's rounded box. |
  | `float.match.foreground` | `Match` | The characters a component matched. |
  | `float.annotation.foreground` | `Annotation` | The annotation and the count. |
  | `float.binding.foreground` | `Binding` | The binding after a candidate. |

- **One match face for every component.** Orderless's four faces, one per component, wait for M2-23's
  theme, where the contrast of each is checked.
- **Contrast:** a unit test in `oso-render` checks that every new pair is at least 4.5:1: the text, a
  match, an annotation and a binding, on the float's fill and on the selected row. ADR 0027's 7:1 is the
  aim, which M2-23 enforces. The defaults are picked from screenshots by the author, as ADR 0075's were.

### The region in an answer

- `Editor::region_cells` gives the selected window's region, the prompt's while one is selected, and the
  answer's pane draws it, as the tiled window does. The open question for M2-17 closes.

### Where it lives

- **`oso-core`:** the new public module `complete` (the matcher, `Candidate`, `Candidates` with the
  selection, the order and the narrowing), on the lowest line but one of `guardrails/layers.txt`,
  with the modules that use no other, as it uses only `std` and `regex`; `minibuffer::Kind::Complete { history }` and the
  candidate rows in `PromptLayout`; `Editor`'s completion methods in `editor/prompts.rs`;
  `ArgSpec::Command` and `ArgSpec::Key`; `Keymap::bindings`.
- **`oso-render`:** the four faces, and `Panel::selected`, the selected row's box.
- **`oso`:** `execute-extended-command` and the completion commands in `commands/minibuffer.rs`, their
  keys in `bindings/minibuffer.rs`, the candidates read from the registry in `dispatch/prompts.rs`, the
  candidate pane in `app/draw/float.rs`, and the socket's field in `dev`.
- **One new direct dependency, a gate:** `regex` in `oso-core`, the only regular-expression crate
  the rules allow (ADR 0010). It is in `Cargo.lock` already, 1.12.4 through `env_logger`, so no crate
  is added to the build; `oso-core` takes it with `std`, `unicode-case` and `perf`, for smart case
  on any script. No new crate, crate edge, `unsafe` or public trait.

### Tests and measures

- `complete`: property tests that a match keeps every component's match, in any order, that narrowing
  gives what a full pass gives, that the order never changes while typing, and that smart case and every
  affix do what the table says; unit tests for a bad expression, the compile limit and an empty input.
- `minibuffer::layout`: the candidate rows, the count and the cuts at every frame height and width
  from 0 up, as ADR 0075's are tested.
- Text-grid snapshots of `M-x`: rows, the selected row, a narrow frame and a short one.
- A flow (`just e2e`, `just e2e-native`): `M-x`, typed, `C-n`, `RET` runs the command, `get-state` on
  the way; `C-u 3 M-x forward-char`; `M-x goto-line` opening its prompt after.
- `just perf` gains `mx_type_p99_ms`, keys typed into `M-x` with every command listed, reported only
  unless the author approves a threshold.

## Questions

Put to the author through the coordinator, the recommended option first. The author chose (a) on
each:

1. **Matching styles:** (a) orderless's defaults: literal or regexp, with its affixes `! , = ^ ~ &`
   and smart case over the whole input; (b) literal, initialism and flex for every component, with no
   affixes; (c) literal only, with `!`. And smart case over (a) the whole input, as isearch, or (b) each
   component.
2. **Ranking:** (a) vertico's: history, then length, then alphabetical, an exact match first, and no
   score; (b) a match score (fzf-like), history breaking ties; (c) history, then the registry's order.
3. **Where matching runs:** (a) on the UI thread to 5000 candidates, 1 ms a key, the worker past that
   built with M2-18; (b) build the worker now too; (c) always on a worker.
4. **The float:** height (a) grows to fit and never shrinks while open, (b) always 18 rows, (c) fits
   the matches each key; the count (a) at the end of the label's row, (b) before the label as
   vertico's, (c) at the end of the field; annotations (a) aligned right, as `ui.md` says, cut first,
   (b) in a column after the longest candidate, marginalia's default (`marginalia-align` `left`).
5. **Keys:** `TAB` (a) puts the selected candidate in the answer, as `vertico-insert`, (b) completes
   the common part, as Emacs's `minibuffer-complete`; `C-n` at the last (a) stays, (b) wraps
   (`vertico-cycle`); `RET` on an empty answer (a) gives the selected candidate, (b) says `[No match]`.
6. **`M-x`'s list:** (a) every command but those that read their key or are bound only in a keymap not
   active, with the shortest binding shown; (b) every command, as Emacs before 28; and the binding
   shown (a) one, the shortest, (b) all of them.
7. **Socket and secrets:** `get-state` shows (a) the count, the selection and the names of the rows
   shown, withheld with the answer beside a secret buffer; (b) the count and the selection only.
8. **Theme:** (a) the four faces above, one match face, a 4.5:1 test now and 7:1 with M2-23, the
   selected row as a rounded box; (b) one face per component, as orderless's four; (c) the selected
   row as a flat background per row in `PaneContent::Lines`.
9. **`regex` in `oso-core`:** (a) add it now, 1.12.4 as locked, for the regexp style; (b) leave the
   regexp style out, and with it the dependency, until a step needs it (isearch's regexp search,
   M2-20 or later); a component is then literal unless an affix says otherwise.
10. **`C-.` (`consult-mark`, ADR 0071):** (a) with M2-19, whose preview it needs; (b) now, without a
   preview.

## Where this differs from accepted ADRs

- [ADR 0034](0034-command-registry-and-keymaps.md): `ArgSpec` gains `Command`, which the dispatcher
  answers by running the named command, and `Key`, for a command that reads the key that ran it; a
  `Keymap` lists its bindings.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md): `TAB`, `C-n` and `C-p` gain their
  completion meanings in a prompt that completes; `RET` there gives the selected candidate; the selected
  row is a rounded box, not a background per row of `PaneContent::Lines`.
- [ADR 0075](0075-a-float-as-a-panel-attached-to-the-top-edge.md): the float gains the candidate rows
  and their padding, which a short frame gives up first, and `Panel` the selected row.

## Consequences

- `M-x` runs any command by name, with its prefix argument, and puts the last one first.
- What the author types in vertico and orderless works the same: components in any order, `!`, `,` and
  the others, `TAB`, `C-n`, `RET`.
- The order never moves while typing, and nothing is scored, so a typo shows no near miss: flex is one
  `~` away.
- A prompt that completes covers up to 23 rows from the top of the text with a one-row label.
- M2-18 builds the worker path on the generation and snapshot fixed here, and M2-19 the preview beside
  the candidate rows.
- `oso-core`'s public API gains `complete`, `Kind::Complete`, the candidate rows in `PromptLayout`,
  `Editor`'s completion methods, `ArgSpec::Command`, `ArgSpec::Key` and `Keymap::bindings`;
  `oso-render`'s gains four `Face` variants and `Panel::selected`.

## Alternatives considered

- **`nucleo-matcher` or `fuzzy-matcher`:** ready and fast, but MPL-2.0 is refused by `deny.toml`
  (ADR 0051), and orderless needs little code.
- **A score, as fzf's or flx's:** finds near misses first, but the order jumps with every key, and the
  author's habit is vertico's stable order.
- **Every component tried as literal, initialism and flex at once:** more matches with no affix, but
  short components match most commands, and the list says less.
- **Matching always on a worker:** one path, but a frame with the old list for every key of `M-x`, whose
  60 commands take microseconds.
- **A fixed height of 18 rows:** the panel never moves, but a prompt with three candidates covers 18
  rows of text.
- **A height that fits the matches at every key:** covers the least, but the panel's edge jumps as the
  list narrows.
- **A background per row in `PaneContent::Lines`:** what ADR 0072 expected, but a flat box beside ADR
  0075's rounded field and chips, and a field every `Lines` pane would carry.
- **Listing every command, as Emacs before 28:** simplest, but `answer-yes` or `digit-argument` from
  `M-x` do nothing useful.
- **`M-x` as a command that runs another itself:** reads naturally, but a command has no registry and
  keeps no state; the dispatcher already runs what a prompt's answer calls for.
- **The count before the label, as vertico's:** familiar, but in ADR 0075's float the label has a row
  of its own, and its end is free.
