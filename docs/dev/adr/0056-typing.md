# 0056. Typing

- Status: Accepted, amended by 0063 and 0101
- Date: 2026-11-05

## Context

[ADR 0051](0051-m2-plan-decisions.md) decided how text gets in: a default binding on a
keymap, and `ctx.input()`. [ADR 0053](0053-edit-core.md) left its undo groups to the
dispatcher. Building the commands left some choices open, mainly where Emacs and
multiple cursors meet, and what to do with counts. This records them. Typing also
replaces half of what [ADR 0046](0046-performance-regression-checks.md) measures.

## Decision

### Text input

- **A keymap has an optional text command.** `Keymap::set_text_command` makes a keymap
  run a command for any key that types a character, unless it binds that key itself.
  `KeyPress::text` says what a key types: a character with no Control, Meta or Super. A
  prefix keymap has none, so `C-x a` is undefined.
- **In the layers, a text command counts as a binding of each key it covers.** This adds to
  [ADR 0034](0034-command-registry-and-keymaps.md)'s "the first keymap that binds it decides":
  a minor mode's text command wins over a major mode's explicit `a`. A layer without one is
  skipped, so it lets typing through to the global keymap. A menu that must take no text is
  the only keymap active, or gives itself a text command that refuses, as Emacs's
  `suppress-keymap` does. The first such layer (M2-20, or menus in M3) decides which.
- **The dispatcher hands the command the key's character** through `ctx.input()`, and an
  empty string for a key that types nothing, so binding `self-insert-command` to `C-q`
  signals an error. `self-insert-command` with no input does too, as through
  `run-command`.
- **`Dispatcher::insert_text`** finds the text command with `Layers::text_command`, so an
  input method's text goes where a key's does, and with no text command nowhere. After a
  pending prefix it is an undefined sequence and inserts nothing.
- **A key that types several characters goes there too,** such as lam-alef on an Arabic layout.
  It is no key press ([ADR 0033](0033-key-presses-and-notation.md)), and it was being dropped with
  nothing to say so. With Control, Meta or Super held it stays dropped, so Option-composed text on
  macOS does too ([ADR 0051](0051-m2-plan-decisions.md)). Input method commits share this path
  from M2-12.
- **Synthetic presses are ignored,** so that a key held when the window gains focus runs
  nothing.

### Commands

- Named after Emacs's, and bound as in [ADR 0030](0030-m2-editing-commands.md):
  `self-insert-command`, `newline` (`RET`), `delete-backward-char` (`DEL`), `delete-char`
  (`C-d`, `<delete>`), `tab-to-tab-stop` (`TAB`), `undo` (`C-z`) and `undo-redo` (`C-M-z`).
- **Counts follow Emacs:** a negative count to `self-insert-command` or `newline` signals,
  and one to a deleting command goes the other way. `tab-to-tab-stop` takes none. A negative
  count to `undo` or `undo-redo` goes the other way too, which is oso's own: motion does so,
  and Emacs's `undo` does nothing useful with one. Keys give no count until `C-u` is read, so
  today only `run-command` does.
- **At most 16 MiB is inserted by one command, across all cursors.** Past that it signals
  and changes nothing. Running out of memory aborts with no emergency save (ADR 0051), and
  that much takes about 40 ms to insert (37 to 45 ms measured, in a 100 MB buffer and an empty
  one), under the 100 ms that counts as a freeze.
- **A deletion that cannot go all the way does nothing,** as Emacs's `delete-char` does:
  `C-u 5 C-d` with 3 characters left signals and leaves them. **With several cursors, this
  holds per cursor.** The ones with room delete and the command then signals, as a motion
  moves its cursors as far as they go.
- **`TAB` counts the display column of each cursor,** so each goes to its own next stop. On a
  row with several cursors it counts what the earlier ones insert, walking what lies between,
  since a tab there starts from a new column. That costs the clusters between two cursors on one
  row, and nothing with one cursor. Wrapped rows (M2-25) must count from the line's start.

### Undo

- Each command runs between `begin_undo_group` and `end_undo_group`, or
  `end_undo_group_after_panic`, in the one function all commands go through. So a command
  that signals still closes its group.
- **Only a single character with a count under 2 asks to join.** Under 2 is Emacs's rule.
- **Text of several characters is its own step,** on either side. Otherwise, 20 commits from
  an input method would undo as one.
- **`RET` is a step of its own.** In Emacs it is `self-insert-command` underneath, but
  it joins only if the previous command was also `newline`. `allow_undo_join` does not look at
  which command came before, and joining `abc RET` would undo a line break with the word
  before it.
- **`DEL` and `C-d` are steps of their own.** Emacs joins them too, up to 20. ADR 0030 and ADR
  0053 speak only of self-inserted characters, so this waits for a request.

### Errors

A failed command's message goes to `Editor::message`, whatever invoked it, and reaches the log
once. A panic's goes there too, and so reaches the log at the `info` level, naming only the
command. An unbound key stays at the `debug` level, since the keys
of a sequence are what the user typed, and a key that types a character shows there as
`<char>`, since it may be text meant for the buffer. The echo area draws messages from M2-7.

### The control socket

`insert-text` stayed out of buffers that may hold secrets. `send-keys` and `run-command` do
not: they go where keys go ([ADR 0034](0034-command-registry-and-keymaps.md)), so a client of
the same user can now change the text of any buffer it can show, and undo it. It still cannot
read one ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)): an edit leaves
`get-state`, `get-text` and `screenshot` withholding it, and a restart refuses it by name. And
nothing can write it until M2-8, which must decide how far this reaches.

### Benchmarks

- **The unbound key is `<f35>`,** so `big_trip_max_ms` still means a trip through the UI
  thread with nothing else in it.
- **Typing is measured directly:** `type_p50_ms`, `type_p99_ms` and `type_max_ms` on a screen
  of mixed text, `big_type_p99_ms` and `big_type_max_ms` in the middle of the 1 GB file, and
  `line_type_p99_ms` and `line_type_max_ms` 2000 characters before the end of the 10 MB line,
  since the very end of a buffer skips most of what a key costs. A run fails unless each key
  drew a frame and the text grew by exactly what the keys type.
- **`DEL` is measured the same way, and only reported:** `del_p99_ms`, `big_del_p99_ms` and
  `line_del_p99_ms`.
- This replaces "two halves" in ADR 0046. The unbound key's trip and the edits without a
  window stay, since they show where a slowdown comes from.
- `type_p99_ms`, `big_type_p99_ms` and `line_type_p99_ms` are listed with the 8 ms budget, and
  `big_type_max_ms` and `line_type_max_ms` with 100 ms. They are still read by hand: CI enforces
  only `key_p99_ms` on a real GPU, as before.
- **Those five can fail a pull request** as regressions, with thresholds in `compare.rs`, under
  the rule of ADR 0046, from 6 runs of one commit on each runner. The `_max_` ones use its freeze
  threshold, and the worst key seen in those runs, moving or typing, was 29 ms. `type_p50_ms` is
  only reported, since typing's p99 is what its budget speaks of.
- **A typing metric takes the wider of its own spread and that of the motion metric beside it** in
  the same runs. Both draw the same frames, and 6 runs may catch a runner's jump in one and not the
  other: `line_keys_p99_ms` jumped once on macOS and `line_type_p99_ms` did not.

  | Metric | Linux | macOS |
  |---|---|---|
  | `type_p99_ms` | 2x, 0.75 ms | 3x, 4 ms |
  | `big_type_p99_ms` | 5.5x, 3.25 ms | 4.5x, 12 ms |
  | `line_type_p99_ms` | 5x, 2.5 ms | 6.75x, 18 ms |
  | `big_type_max_ms`, `line_type_max_ms` | 3x, 100 ms | 3x, 100 ms |

- **`line_keys_p99_ms` on macOS goes from 2.75x, 3.5 ms to 6.75x, 18 ms.** One of the 6 runs took it
  from 3.72 ms on `main` to 12.12 ms with no change to its path, and failed the run. On the author's
  Mac it stayed between 0.5 and 1.5 ms in 12 runs of this branch and `main`, so that is the runner.

## Consequences

- `insert-text` is gone, as ADR 0055 said. Its flows use `send-keys`, which needs a key per
  character. Its reply says the keys ran on the UI thread, and not that they changed anything:
  a key that fails gives the same reply, so a caller reads `bytes` in `get-state`.
- Text can be lost until M2-8 brings saving.
- The session's messages grow by one per failed key, up to their bound of 1000, and a held
  key at the end of the buffer writes a log line per repeat at the `info` level.
- `C-u` (M2) must decide whether a large count to a command that walks clusters needs a limit. A
  cluster takes about 0.3 µs, so a count of 300,000 passes 100 ms and `C-u 500000000 C-d` in a
  1 GB file walks for over 2 minutes on the UI thread, as `C-f` does. A count past the end of the
  buffer in bytes is refused without a walk.
- A command clones the selection set for its group, also motion, and an edit a few times more.
- `Binding` is now a small `Copy` view, and what a keymap stores is private. The API snapshot
  shows it.
- A failed key leaves a message and asks for no frame, which M2-7 must change when it draws
  messages. An undefined key leaves none, since messages reach the log.

## Alternatives considered

- **Reading `KeyEvent.text`:** gives `q` for `C-q`, and keys from `send-keys` have none
  (ADR 0051).
- **A bound on the count alone:** a count of a million is fine for one cursor and a problem
  with a thousand.
- **All or nothing across cursors:** one cursor at the buffer's start would stop the others,
  which multiple-cursors.el does not.
- **Joining `RET` and `DEL` as Emacs does:** needs `allow_undo_join` to know which command came
  before, for little. It can be added without changing a command's signature.
- **Letting several characters join:** a run of commits could undo as a paragraph.
- **`indent-for-tab-command` as the name for `TAB`:** that is what Emacs binds, but it indents,
  which comes with modes in M3.
