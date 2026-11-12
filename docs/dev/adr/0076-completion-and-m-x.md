# 0076. Completion and `M-x`

- Status: Accepted, amended by 0089, 0096, 0112 and 0116
- Date: 2026-11-08

## Context

M2-17 builds completion in the minibuffer: the orderless matcher, 18 candidate rows, annotations,
history and `M-x` ([roadmap](../roadmap.md)). M2-18 (`C-x C-f`) and M2-19 (`C-x b`, with a preview)
build on it, and so do M2-21's `M-y` from the kill ring, `C-.` (`consult-mark`, ADR 0071) and M3's
consult-like commands. The defaults follow Emacs's vertico, orderless, marginalia and consult, which
ADR 0008 and `ui.md` already name as the model.

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
match and the selected row are picked from screenshots later, as ADR 0075's were. The author
accepted this ADR, and the pull request with it, on 2026-11-09.

After seeing builds in the app, the author changed question 4 (the float), twice: a completion's
float sits at the center of the frame, 62% of its width, always as tall as 18 candidate rows and the
answer's row; it is a plain list on the text's background in a 5-point border, with the count first
on the answer's row, bindings in parentheses, annotations in one column and the selected row a bar
across the panel. Questions and every other prompt keep ADR 0075's panel on the top edge.
[The float](#the-float) describes that, and the rest of question 4's first answer no longer holds.
The four faces of question 8 gain a fifth, for the count and the label.

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
- **Expressions and searchers are compiled once per input,** a failure to compile too, and kept while
  a component's text, style and case stay the same. `regex::RegexBuilder`'s size limit is 1 MiB, which
  admits ordinary Perl classes such as `\w{10}`, and its nest limit 32, so that a component pasted in
  cannot take long to compile; the `regex` crate matches in linear time. A literal is searched with a
  searcher of its own, built once, and skipped for a candidate shorter than it.
- **Perl classes and general categories** (`\w`, `\d`, `\s`, `\b`, `\pL`) compile in the app: `regex`
  is built with `unicode-perl` and `unicode-gencat`, which a test in the app's own crate checks.
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
  alphabetical, and there an empty answer takes a candidate only once the person moved to one, as
  the first of that order is no choice. A prompt a control socket request opened records nothing,
  as its answer is not the person's.

### Where matching runs

- **On the UI thread, up to 5000 candidates,** which every list of M2-17 and M2-19 is: `M-x` has about
  60 commands now and a few hundred later. The budget is 1 ms of matching per key at 5000 candidates,
  which an ignored test in `complete` checks when run by hand in a release build; `just perf`'s
  `mx_type_p99_ms` measures typing into `M-x` in the app.
- **What is matched is bounded:** an answer over 1024 bytes, or of more than 32 components, counted
  before any is compiled, matches
  nothing, and is not copied out of its buffer, so a whole file yanked into the answer costs no key
  more than that. A component's compiled expression is reused while its text and case stay the same.
- **Narrowing reuses the last result, and checks only what changed:** an input that only adds
  characters to the last one, or adds components, filters the
  candidates that matched before, not the whole list.
- **Past 5000, on a worker thread, built with M2-18,** whose directories are the first list that can be
  that long. What is fixed now: each input has a generation, the worker gets the list as an
  `Arc<[Candidate]>` snapshot with the input and its generation, and the UI takes a result only if its
  prompt is still open and its generation is the newest; any other result is dropped. Until a result
  comes the rows shown stay, and the count shows `…`. Results arrive through the `Status` comparison,
  as a load's progress does (ADR 0072). Nothing in M2-17 reaches the worker path.

### The float

A completion's float, unlike every other prompt's:

```
        ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
        ┃ 2/2   M-x forw█                                            ┃ the answer's row
        ┃ forward-char (C-f)  Move point N characters forward (back… ┃
        ┃▌forward-word (M-f)  Move point to the end of the next wor…▐┃ ← selected, a bar
        ┃                                                            ┃ 18 rows, matching or not
        ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

- **At the center of the frame's text area,** both ways, not on its top edge. Questions and the other
  prompts keep ADR 0075's panel on the top edge, which stands for the minibuffer; a question asked
  from inside `M-x` shows there, as the innermost prompt.
- **62% of the text area's columns,** at least 40 and never closer than 2 to an edge, as ADR 0075's
  floats are bounded, with a column of padding at each side.
- **As tall as the answer's row and 18 candidate rows, from the moment it opens,** with no padding rows:
  the list starts on the row after the answer's, and nothing follows it. The float neither grows nor
  shrinks with the matches: rows that nothing fills stay empty. The number is a constant,
  `complete::ROWS`, until configuration (M3) sets it.
- **The answer's row:** the count first, `2/2`: the selected candidate's place and the number of
  matches, as `vertico-count-format` does, kept as wide as the widest count; then the label, cut with
  `…` to a third of the row at most; then what is typed, with a message at its end. Both the count and
  the label are in `Face::CompletionPrompt`, a teal accent. There is no field around the answer.
- **A plain panel:** filled with the text's background, framed by a 5-point border in a muted slate a
  little lighter than it (`PanelStyle::border`, read every frame as the theme's other values are),
  with square corners and no shadow. `Panel::border` asks for it.
- **A row:** the candidate from the inner cells' first column, in the text's color; its binding one
  cell after it, in parentheses, `(C-x C-s)`, in `Face::CompletionBinding`, a light blue; then its annotation,
  in `Face::CompletionAnnotation`, a dim blue-grey.
- **Annotations form a column:** they start at one column, two cells after the longest candidate and
  binding among the rows shown, at most halfway across the row. A row whose own candidate and binding
  reach past it starts its annotation two cells after them.
- **When a row is narrow,** the annotation is cut first, with `…` at the row's end, and goes below 8
  cells; then the binding goes; then the candidate is cut with `…` at the edge. The candidate is never
  cut before the annotation and the binding have gone.
- **The selected row** is a bar across the whole panel, inside the border, one row tall and square, in
  `Face::CompletionSelected`'s background, a medium blue, drawn with ADR 0075's pipeline in `Layer::FLOAT` under
  the text, which keeps its faces on it. So `PaneContent::Lines` needs no background per row, and the
  open question about it closes. The candidates' rows are one pane of `PaneContent::Lines` in
  `Layer::FLOAT_CONTENT`.
- **Italic annotations wait for the text system:** it has one font today, so annotations are only
  dimmed.
- **The list scrolls** to keep the selected row in view, with no margin, as vertico does.
- **A short frame gives up candidate rows.** The float is never taller than the text area.

### Keys

In a prompt that completes, a keymap made of ADR 0072's minibuffer keys and these, in the major-mode
layer in the minibuffer keymap's place:

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
- **Every completion command needs a person** (`Reach::Person`, ADR 0085): `execute-extended-command`,
  the moves through the list, which choose what `RET` runs, and `insert-completion`, as answering a
  prompt does.
- **`run-command execute-extended-command`** is refused, "run the command itself": the socket names a
  command directly. A command marked `ArgSpec::Key` takes a count from `run-command` as one marked
  `Count` does.

### Secrets

- **No history** is read or recorded in a prompt opened while a buffer that may hold secrets is shown,
  or answered with one beneath it (ADR 0072), so the candidates' order says nothing of earlier answers.
- **A passphrase prompt never completes.**
- **Annotations come from the registry,** which holds no text of a buffer. M2-19's buffer names and
  M2-18's paths decide what they show of a secret buffer when they come.
- **`get-state`** gives a prompt that completes the kind `complete`, and, for the innermost,
  `completion`: `count`, `total`, `selected` and `rows`, the names of the candidate rows shown, at most
  18. While a buffer that may hold secrets is shown it is left out with the label and the answer, as the
  set of matches tells the answer.
- **While a prompt that completes is open beside a buffer that may hold secrets,** `run-command` runs
  only `keyboard-quit`, as while a passphrase prompt is open (ADR 0072): whether a command found a
  match would say something of the answer withheld.
- **A prompt that completes opened by a control socket request is answered over the socket,** as a
  question it asked is (ADR 0072): keys from the window say so and leave it open, but `C-g`, so that a
  person's `RET` cannot run a command an agent chose.

### Theme

- **New faces,** with defaults in `pane.rs`'s tables, which M2-23 turns into tokens of a theme that
  switches at run time; no color is cached in an instance or a shader (ADR 0075):

  | Token | Face | Use |
  |---|---|---|
  | `float.selection.background` | `CompletionSelected` | The selected row's rounded box. |
  | `float.match.foreground` | `CompletionMatch` | The characters a component matched. |
  | `float.annotation.foreground` | `CompletionAnnotation` | The annotation and the count. |
  | `float.binding.foreground` | `CompletionBinding` | The binding after a candidate. |
  | `float.list.prompt` | `CompletionPrompt` | The count and the label on the answer's row. |

- **One match face for every component.** Orderless's four faces, one per component, wait for M2-23's
  theme, where the contrast of each is checked.
- **Contrast:** a unit test in `oso-render` checks that every new pair is at least 4.5:1: the text, a
  match, an annotation and a binding, on the text's background and on the selected row, and the count
  and label on the background. So the annotation's blue-grey is lighter than a dimmer one would be. ADR 0027's 7:1 is the
  aim, which M2-23 enforces. The defaults are picked from screenshots by the author, as ADR 0075's were.

### The region in an answer

- `Editor::prompt_region_cells` gives the innermost prompt's region, in its answer's cells, and the
  answer's pane draws it, as the tiled window's does from `Editor::region_cells`. The open question for
  M2-17 closes.

### Where it lives

- **`oso-core`:** the new public module `complete` (the matcher, `Candidate`, `Candidates` with the
  selection, the order and the narrowing), on the lowest line but one of `guardrails/layers.txt`, with
  the modules that use no other, as it uses only `std` and `regex`; `minibuffer::Kind::Complete
  { history }`, `PromptLayout::count`, and the placing of a completion's row and of a candidate's row
  in `minibuffer/completion.rs` and `minibuffer/candidates.rs` (`annotation_column`, `candidate_row`); the float's cells in
  `frame::completion_regions` and `FloatRegions::candidates`; `Editor`'s completion methods in
  `editor/completion.rs`; `ArgSpec::Command` and `ArgSpec::Key`; `Keymap::bindings`;
  `Command::summary`.
- **`oso-render`:** the five faces, `Panel::selected` and `Panel::border`, and the border's width and
  color in `PanelStyle`.
- **`oso`:** `execute-extended-command` and the completion commands in `commands/complete.rs`, their
  keys in `bindings/complete.rs`, the candidates read from the registry and keymaps in
  `dispatch/extended.rs`, what an answer runs in `dispatch/prompts.rs`, the candidate pane in
  `app/draw/float.rs`, and the socket's field in `dev`.
- **One new direct dependency, a gate:** `regex` in `oso-core`, the only regular-expression crate
  the rules allow (ADR 0010). It is in `Cargo.lock` already, 1.12.4 through `env_logger`, so no crate
  is added to the build; `oso-core` takes it with `std`, `unicode-case`, `unicode-perl`, `unicode-gencat`
  and `perf`, for smart case, Perl classes and general categories
  on any script. No new crate, crate edge, `unsafe` or public trait.

### Tests and measures

- `complete`: property tests that a match keeps every component's match, in any order, that narrowing
  gives what a full pass gives, that the order never changes while typing, and that smart case and every
  affix do what the table says; unit tests for a bad expression, the compile limit and an empty input.
- `frame::completion_regions` and `minibuffer::completion_layout`: the rows, the count and the cuts at
  every frame height and width from 0 up, as ADR 0075's are tested, and that no part overlaps another.
- An ignored test in `complete` that matching at 5000 candidates takes under 1 ms a key, run by hand in
  a release build.
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

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. When this ADR is accepted,
the status of ADR 0034, ADR 0072 and ADR 0075 becomes `Accepted, amended by 0076`.

- [ADR 0034](0034-command-registry-and-keymaps.md), "`ArgSpec` is `None` or `Count` and documents what
  a command reads" (with ADR 0072's `Number` and `Passphrase`): `ArgSpec` also has `Command`, a
  command's name read in a prompt that completes, which the dispatcher answers by running that
  command with the prefix argument, and `Key`, for a command that reads the key that ran it, which
  `M-x` leaves out. A `Keymap` lists its bindings (`Keymap::bindings`).
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), "`TAB` is unbound until M2-17 gives
  it completion, and `C-n` and `C-p` move as in any one-row buffer until M2-17 gives them candidates":
  in a prompt that completes, `TAB` puts the selected candidate in the answer, `C-n`, `C-p` and the
  arrows move through the candidates, `M-<`, `M->`, `C-v` and `M-v` go to the ends and by pages, and
  `RET` gives the selected candidate. In other prompts they stay as ADR 0072 says.
- ADR 0072, "The selected candidate needs a background per row in `PaneContent::Lines`": the selected
  candidate is a bar drawn in the float's panel (`Panel::selected`), so `PaneContent::Lines` keeps no
  background per row.
- ADR 0072, "A question a control socket request opened is answered over the socket": so is a prompt
  that completes that a request opened.
- [ADR 0075](0075-a-float-as-a-panel-attached-to-the-top-edge.md), "**Attached to the top edge.**" and
  "**Width as before:** 60% of the columns": a prompt that completes floats at the center of the text
  area, both ways, and takes 62% of its columns, at least 40, never closer than 2 to an edge. Every
  other prompt stays on the top edge at 60%.
- ADR 0075, "**The cells of a float that reads an answer**" (the label's rows, the gap, the answer and
  the padding below) and its field: a prompt that completes has the answer's row and 18 candidate rows
  right below it, with no padding rows, the count, the label and what is typed on the answer's row, in
  that order, and no field.
- ADR 0075, "**One shape per frame** ... a fill in `Face::Float`, its bottom corners rounded, a hairline
  along its sides and bottom, and a shadow": a completion's panel is filled with the text's background,
  framed by a 5-point border in `PanelStyle::border`, drawn as a square box behind it, with square
  corners and no shadow (`Panel::border`).
- ADR 0075, "**Small frames (chosen)**": a completion's float gives up candidate rows; the order there
  holds for every other prompt.

## Consequences

- `M-x` runs any command by name, with its prefix argument, and puts the last one first.
- What works in vertico and orderless works the same: components in any order, `!`, `,` and the
  others, `TAB`, `C-n`, `RET`.
- The order never moves while typing, and nothing is scored, so a typo shows no near miss: flex is one
  `~` away.
- A prompt that completes covers 19 rows at the center of the text, whatever matches, and the text
  above and below it stays in view.
- M2-18 builds the worker path on the generation and snapshot fixed here, and M2-19 the preview beside
  the candidate rows.
- `oso-core`'s public API gains `complete` (`Candidate`, `Candidates` with its reading methods, and
  `Query::highlights`), `Kind::Complete`, `PromptLayout::count`, `minibuffer::candidate_row` and
  `CandidateRow`, `FloatRegions::candidates` and `candidates_padded`, `Prompt::candidates`, `Editor`'s
  completion methods (`open_completion`, `match_completion`, the moves, `insert_completion`,
  `candidate_rows`, `prompt_region_cells`), `ArgSpec::Command`, `ArgSpec::Key`, `Keymap::bindings` and
  `Command::summary`. The matcher's parsing and narrowing stay inside the crate, for M2-18's worker to
  change. `oso-render`'s gains five `Face` variants, `Panel::selected` and `Panel::border`;
  `oso-core`'s also `minibuffer::{annotation_column, BINDING_OPEN, BINDING_CLOSE}`.
- M2-18's worker gives back matches, not where the list is scrolled to, which stays with the prompt.

## Alternatives considered

- **`nucleo-matcher` or `fuzzy-matcher`:** ready and fast, but MPL-2.0 is refused by `deny.toml`
  (ADR 0051), and orderless needs little code.
- **A score, as fzf's or flx's:** finds near misses first, but the order jumps with every key, unlike
  vertico's stable order.
- **Every component tried as literal, initialism and flex at once:** more matches with no affix, but
  short components match most commands, and the list says less.
- **Matching always on a worker:** one path, but a frame with the old list for every key of `M-x`, whose
  60 commands take microseconds.
- **The float on the top edge, as ADR 0075's, growing to fit the list and never shrinking while open
  (the first build):** one place for every prompt, but the panel's bottom edge left most of the panel
  empty once typing narrowed the list, and a long list hangs from the top far from where the eye is.
- **A height that fits the matches at every key:** covers the least, but the panel's edge jumps as the
  list narrows.
- **A background per row in `PaneContent::Lines`:** what ADR 0072 expected, but a flat box beside ADR
  0075's rounded field and chips, and a field every `Lines` pane would carry.
- **Listing every command, as Emacs before 28:** simplest, but `answer-yes` or `digit-argument` from
  `M-x` do nothing useful.
- **`M-x` as a command that runs another itself:** reads naturally, but a command has no registry and
  keeps no state; the dispatcher already runs what a prompt's answer calls for.
- **The count before the label, as vertico's:** familiar, but the end of the answer's row keeps it
  apart from what is typed.
- **The centered float with ADR 0075's look (the second build):** a lighter panel, rounded corners, a
  shadow, padding rows, a field around the answer and the count at the end of its row. The author
  preferred a plain list in a border, with the count first.
- **Annotations aligned to the right edge:** each ends at the same column, but they start at ragged
  columns, which is harder to scan than a column.
