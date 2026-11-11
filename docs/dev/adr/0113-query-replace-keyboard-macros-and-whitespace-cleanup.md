# 0113. query-replace, keyboard macros and whitespace cleanup on save

- Status: Accepted
- Date: 2026-11-12

## Context

The roadmap puts "query-replace (`M-%`), keyboard macros, whitespace cleanup on save" in M3
([roadmap](../roadmap.md)), and [ADR 0030](0030-m2-editing-commands.md) placed them there. They are
small, but each meets rules that earlier ADRs fixed:

- isearch has a literal matcher with smart case in `oso_core::search`, private to the crate. Each
  key searches 256 KiB of text on the UI thread and the rest on a worker over an O(1) snapshot.
  The count is the worker's too. Only the primary cursor moves. ADR 0082 left regexp search and
  what of the matcher becomes public to M3's query-replace ([ADR 0082](0082-isearch.md)).
- Commands act on the selection set. A command's splices are applied in one batched pass. At most
  1000 cursors exist ([ADR 0091](0091-multiple-cursor-keys-and-m-y.md)).
- One command is one undo step across all cursors. Undo is linear ([ADR 0030](0030-m2-editing-commands.md),
  [ADR 0053](0053-edit-core.md)).
- Keys go through layered keymaps and one resolver. A key from the dev socket goes the same way
  ([ADR 0034](0034-command-registry-and-keymaps.md)).
- A `C-g` does the first thing that applies: a composition, a prefix, the region, the innermost
  prompt, isearch's two steps, then extra cursors ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md),
  ADR 0091).
- Every command declares a reach. The channel refuses `Person`, waits while the person is in the
  middle of something, and runs nothing while a buffer that may hold secrets is shown
  ([ADR 0085](0085-agent-channel-and-oso-mcp.md)).
- A passphrase prompt's buffer may hold secrets. It keeps no undo record and enters no history.
  "M3's keyboard macros must not record what is typed there" (ADR 0072). Secret patterns and the
  rules around such a buffer are in [ADR 0028](0028-secrets-handling.md) and
  [ADR 0044](0044-control-socket-trust-and-secret-buffers.md).
- `oso dev` carries the unsaved text of buffers that cannot hold secrets, the kill ring and the
  histories, and no undo history ([ADR 0090](0090-oso-dev-carries-unsaved-text.md)).
- A save makes its final newline an edit and an undo step of its own, before the O(1) snapshot.
  A save of a buffer while one runs is refused ([ADR 0059](0059-safe-save-and-the-exit-guard.md)).
  The write keeps the access list and the quarantine flag ([ADR 0067](0067-access-lists-and-the-quarantine-flag-in-a-save.md)).
- Settings live in `config.toml` and apply when their value changes
  ([ADR 0108](0108-the-user-configuration-file.md), [ADR 0110](0110-adr-0108-as-built.md)).
- Only the prompt for a file's name takes Japanese; every other prompt keeps the input method
  off ([ADR 0106](0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md)).
- Only the `regex` crate is used for regular expressions. `oso-core` already depends on it, for
  completion's regexp style ([ADR 0076](0076-completion-and-m-x.md)). It needs contiguous text.
  A rope is not contiguous.

A buffer can be 1 GB, and the UI thread must never wait on it. So none of the three may scan a whole
buffer on the UI thread.

## Decision

The author chose on 2026-11-12 the first option of each question under
[Questions](#questions), which the text below describes. The rest are defaults put to them with
this ADR. They accepted this ADR, with the rest as written, on 2026-11-12, through the
coordinator.

### 1. query-replace

#### Keys and prompts

| Keys | Command | What it does |
|---|---|---|
| `M-%` | `query-replace` | Replaces a string, asking at each match |
| `C-M-%` | `query-replace-regexp` | Replaces a regexp's matches, asking at each |

- **Two prompts,** as Emacs: `Query replace: ` reads the string, then `Query replace foo with: `
  reads the replacement. Both are ADR 0072's prompts with an `ArgSpec::Text`.
- **One history, `query-replace`,** holds both. An empty first answer reuses the last pair, which
  the first prompt shows: `Query replace (default foo → bar): `. With no last pair, an empty
  string signals "Empty search string".
- **The regexp command has its own history pair,** `query-replace-regexp`, so a regexp is never
  reused as a literal string.
- **Both prompts take Japanese, as the prompt for a file's name does (Q5).** A Japanese input mode
  chosen there turns the input method on until the prompt closes, and ASCII is chosen when it
  opens (ADR 0106). Never while a buffer that may hold secrets is shown.
- **A read-only buffer signals "Buffer is read-only" before any prompt opens,** as `M-y` does
  (ADR 0091). A buffer still loading is read-only, and says "Buffer is still loading".

#### Matching

- **`M-%` matches the literal string with isearch's matcher and its smart case.** It is
  case-sensitive once the string holds an uppercase letter.
- **`C-M-%` uses the `regex` crate's syntax (Q1).** The replacement takes `$1`, `${name}` and
  `$0`, and `$$` for a dollar, as `Regex::replace` does. An invalid regexp says why, in the prompt,
  and the prompt stays open.
- **Smart case for a regexp** looks at letters outside escapes, as Emacs does: `\W` and `\p{Lu}`
  hold no uppercase letter, and `Foo` does. A `(?i)` or `(?-i)` in the pattern wins.
- **A regexp matches within one line (Q2).** Each line is the haystack: a slice of a chunk when it
  lies in one, or else a copy into a buffer the search reuses. So a match never crosses a line
  break, `^` and `$` are a line's start and end, and `\n` matches nothing. A line over 64 MiB ends
  the session at it, with "Regexp search stops at line N: it is longer than 64 MiB".
- **An empty match** (`^`, `x*`) is offered as any other. The next search starts one character
  after it, so the session always moves on.
- **Case is preserved as Emacs does,** whenever the search folds case (`case-replace`). The
  replacement is changed by the match it replaces:
  - a match whose letters are all uppercase, with a word of two letters or more, takes the
    replacement in uppercase: `FOO` → `BAR`;
  - otherwise, a match whose words all start with an uppercase letter takes the replacement with
    each word's first letter in uppercase: `Foo` → `Bar`, `Foo Bar` → `Baz Qux`;
  - otherwise the replacement goes in as typed.

  Words are runs of letters and digits. A script with no case, such as Japanese, is never changed.
  A case-sensitive search changes nothing.

#### The session

- **query-replace is a mode of the selected window, as isearch is.** Its line is on the echo row:
  `Query replacing foo with bar: [3/17] (? for help)`. The current match is in isearch's current
  face, and the others in view in its other face, through the same `MatchCells`.
- **The answers are Emacs's:**

  | Key | What it does |
  |---|---|
  | `y`, `SPC` | Replace this match, go to the next |
  | `n`, `DEL` | Skip it, go to the next |
  | `!` | Replace this and every match left in the scope, without asking |
  | `.` | Replace this one and end |
  | `,` | Replace this one and stay; the next `y`, `n` or `SPC` moves on |
  | `^` | Go back to the previous match, without undoing it |
  | `u` | Undo the last replacement, and go back to it |
  | `q`, `RET` | End |
  | `?` | Show the keys on the echo row |

  `E` (edit the replacement), `C-r` (recursive edit) and the multi-buffer `Y` and `N` come later.
- **Any other key ends the session where it is, and then runs as it would have,** as in isearch
  (ADR 0082) and in Emacs.
- **`C-g` ends it too, keeping what was replaced,** as Emacs does. It is `Quit::Replace` in
  `Editor::quit`, after isearch's steps and before the extra cursors.
- **It starts at the primary cursor and goes forward, without wrapping,** as Emacs. When it ends,
  the echo area says `Replaced 12 occurrences (skipped 3)`. If the cursor moved, the place it
  started is pushed onto the primary cursor's mark ring, as Emacs's `push-mark` does, with no
  message.
- **The count** is isearch's: the worker counts the matches in the scope when the session starts,
  and the line shows `[?/…]` meanwhile. The index is the number of matches passed. A replacement
  does not change the total, since the search never goes back into replaced text.

#### Scope, the region and several cursors

- **With a region shown, the scope is the region (Q4),** and with several cursors, every cursor's
  region, in buffer order. Each is a range the session shifts as it replaces. With no region shown,
  the scope runs from the primary cursor to the end of the buffer.
- **Only the primary cursor moves,** as in isearch. The other cursors stay, their regions hidden
  when the session starts, and merge as usual if it meets them. The edits shift them as any edit
  does.

#### Undo

- **One session is one undo step (Q3).** Each answer's replacement is a step while the session
  runs, so `u` undoes the last one. When the session ends, the steps it made are joined into one,
  as Emacs's `undo-amalgamate-change-group` joins them. `C-z` after it puts the whole session back.
- That step returns the cursors to where the session started.

#### Large buffers

- **Each answer searches 256 KiB of text on the UI thread,** as an isearch key does. If that finds
  no match and the scope goes on, the rest is searched on the isearch worker, and the line says
  `[searching…]`.
- **Answer keys typed while the next match is searched for are held,** as `RET` is held for a
  completion worker (ADR 0096), and run in order when the match comes. `C-g` drops them and ends
  the session.
- **`!` replaces on the worker's list, in batches.** The worker finds the remaining matches over a
  snapshot, works out each replacement for a regexp, and sends them back in batches of at most
  1000, over a bounded channel, so it waits while the UI thread has not taken a batch. The UI
  thread applies one batch per turn of the loop with ADR 0091's batched pass, shifting each
  offset by what earlier batches changed. The echo row says `Replacing… 120000/500000`. Keys are
  held meanwhile, and `C-g` stops it, keeping what was replaced. The batch size is tuned by
  measuring that a batch stays within 4 ms on the 100 MB file.
- **A change to the text the session did not make** (a reload, which waits as for isearch, is
  the only one) drops what a worker reports, and the search runs again from the cursor.

#### Secrets and the sockets

- **query-replace works in a buffer that may hold secrets,** as isearch does. Its strings then
  enter no history, its messages go to the echo area only, and `get-state` leaves out the prompts'
  text and the session's strings, as ADR 0072 and ADR 0082 do.
- **Nothing logs the strings.** The worker reports offsets and replacement text only to the UI
  thread. `Debug` of the session prints no string.
- **`get-state` gains `replace`:** `null`, or `regexp`, `searching`, `count` and `replaced`, and
  only `{"active": true}` around a buffer that may hold secrets.
- **A dev restart waits for the session to end,** as for isearch.
- **`query-replace` and `query-replace-regexp` are `Person` (Q6).** They read keys at each match.
  The channel gets no replace command in this ADR: an agent edits with `insert` and the kills, or
  with its own tools on the file.
- **The channel waits while a session is on,** as for isearch: a channel command would end it.

#### As built (step 2, 2026-11-12)

- **`!` replaces a slice a turn on the UI thread,** not from a worker's list over a bounded
  channel: each slice finds the matches within a key's budget of text, at most 1000 of them, and
  replaces them as one edit, and the host asks for the next at its next turn. The offsets stay
  right with no remapping, and isearch's worker, which runs one job at a time, stays free for
  the count. Over 100 MB with 100,000 matches, a slice takes 1.2 ms at p50 and 1.5 ms at p99.
  Undoing that whole session is one step, which takes about 230 ms on the UI thread, once.
- **The count and a search past a key's budget are a scoped job on isearch's worker,** over the
  scope's ranges. The count is of the matches in the scope when the session began.
- **Answers given while the worker looks are kept by the session,** in order, not held in the
  dispatcher, and run once the match comes.
- **The scope is a set of ranges the buffer tracks,** as the cleanup command's are, so it follows
  the session's own edits.
- **One undo step:** each answer's replacement is a step while the session runs, and the
  history joins the steps made since the session began when it ends (`History::join_since`).
- **The answers are commands of their own,** `replace-this`, `replace-skip`, `replace-all`,
  `replace-this-and-end`, `replace-this-and-stay`, `replace-back`, `replace-undo`, `replace-end`
  and `replace-help`, all `Person`, in a keymap that takes the others' place while the session
  is on.
- **The two prompts** are read by `ArgSpec::Replace`, which `Ctx::replace_pair` gives back, and
  take Japanese as the prompt for a file's name does.

### 2. Keyboard macros

#### Keys

| Keys | Command | What it does |
|---|---|---|
| `C-x (`, `<f3>` | `kmacro-start-macro` | Starts recording |
| `C-x )` | `kmacro-end-macro` | Ends recording |
| `C-x e`, `<f4>` | `kmacro-end-and-call-macro` | Ends recording if it is on; runs the last macro |

- **After `C-x e` (or `<f4>`), `e` alone runs the macro again (Q12),** as Emacs's
  `kmacro-call-repeat-key`. Any other key ends that and runs as usual. `<f4>` repeats itself.
- **A count:** `C-u 3 C-x e` runs it 3 times. `C-u 0 C-x e` runs it until something stops it.
  `C-u 3 C-x )` ends the recording and runs it 2 more times, as Emacs.
- **The modeline shows `Def` while recording,** as Emacs.
- `C-x (` while recording says "Already defining a keyboard macro". `C-x e` with no macro says
  "No keyboard macro defined". The kmacro ring, counters, naming, editing and `C-u C-x (`
  (appending) come later.
- **One macro is kept,** the last recorded, in memory only. A new recording replaces it once it
  ends; a recording cancelled keeps the old one.

#### What is recorded

- **Input, as Emacs records it (Q7):** each key the dispatcher takes, and each text an input method
  commits. Not a composition's preedit: the macro holds what was committed, so a replay types the
  same Japanese text whatever the input source is then.
- **Keys from the dev socket's `send-keys` are recorded,** as they stand in for the person's.
  Wheel scrolls are not. Commands the agent channel runs are not, and the channel waits while a
  recording is on (below).
- **The macro commands' own keys are never recorded:** not `C-x (`, nor the `C-x )` or `C-x e`
  that ends it, nor a prefix argument given to them.
- **Keys typed in a prompt are recorded,** as Emacs records them: `M-x name RET`, a file's name, a
  query-replace's strings. A replay types them into the prompt the replay opens.
- **A recording holds at most 100,000 events and 16 MiB of committed text.** Past either, it is
  cancelled with "Keyboard macro too long; not recorded", and the old macro stays.

#### Passphrases

- **A passphrase prompt cancels the recording when it opens:** "Keyboard macro not recorded: a
  passphrase was asked for". Nothing typed in it is recorded, as ADR 0072 asks, and the old macro
  stays.
- **A replay that opens a passphrase prompt stops there,** with the prompt left open for the
  person. It never types into one.

#### Replaying

- **A replay feeds the recorded events through the dispatcher,** as keys from the window: the same
  keymaps, prompts, isearch and query-replace. So the same keys do what they do now, not what they
  did then.
- **It never makes the UI thread wait.** The app feeds events for at most 4 ms of each turn of the
  loop, then draws a frame and goes on at the next.
- **It feeds the next event only when the last one's work is done:** a worker's answer
  (isearch, the cursor keys, query-replace, completion), a clipboard read (ADR 0074), a held key,
  and the load of a file the replay opened. So a replay does what the keys did, not what they did
  before the answer came.
- **A key from the window during a replay:** `C-g` stops the replay first, before anything in the
  `C-g` chain. Other keys are held, and run after it, as Emacs queues them.

#### What stops a replay

- **Anything that would ding in Emacs stops the replay and the iterations left,** and `C-u 0` ends
  this way: a command's error (`End of buffer`, `No more matches`), an undefined key, a failing
  isearch, and a query-replace with no match.
- **A question stops the replay there (Q9),** left open for the person: an exit's, a save over a
  file changed on disk, any `y or n`. A replay never answers a question; the recorded answer and
  what follows are dropped. The channel's `Agent:` questions are never open during a replay,
  as the channel waits.
- **A recording or replay ends with the echo area saying why,** when it stopped early.

#### `C-g` while recording

- **`C-g` takes its step in the chain and is recorded (Q11)** when the chain has something to do:
  the region, a prompt, isearch, the cursors. So a macro can hide a region or leave a prompt.
- **When the chain has nothing to do (`Quit::Nothing`), it cancels the recording,** with "Keyboard
  macro recording cancelled", and the old macro stays, as Emacs's `kmacro-keyboard-quit` keeps it.

#### Undo

- **One replay is one undo step in each buffer it changed (Q8),** with its count. Its commands make
  their steps as usual while it runs, so a recorded `C-z` undoes what it undid then. When it ends,
  or stops, the steps made since it began are joined into one, as for query-replace. In a buffer
  where the replay undid past where it began, nothing is joined.

#### Secrets, restarts and the channel

- **Recording and replaying work in a buffer that may hold secrets (Q10),** as isearch does. A
  macro recorded while such a buffer was shown, or that typed into one, is marked secret. Its
  `Debug` never prints events in any case; nothing logs them.
- **`oso dev` does not carry the macro,** secret or not, and a restart waits while one is recorded
  or replayed, as for a prompt. ADR 0090's handoff stays as it is.
- **`get-state` gains `macro`:** `recording` and `replaying`, booleans. No events, no count.
- **The four macro commands are `Person`.** A replay types keys, which reach every binding,
  `Person` commands among them, so the channel can never start one.
- **The channel waits while a macro is recorded or replayed:** a channel command would change the
  text under the macro without being in it.

### 3. Whitespace cleanup on save

#### What is cleaned

- **Trailing spaces and tabs at the end of a line,** and **blank lines at the end of the buffer,**
  down to the one final newline ADR 0030 keeps. Other whitespace, such as U+3000 or U+00A0, stays:
  a Japanese full-width space at a line's end may be meant.
- **Only on lines edited since the file was read, reloaded or last saved (Q13),** as Emacs's
  `ws-butler` does. So saving a file someone else wrote changes only the lines the person touched,
  and a diff holds no noise. An edit by undo counts. The blank lines at the end are cleaned when
  one of them was edited.
- **Whitespace before a cursor on its own line stays (Q16),** in the buffer and in the file. So
  typing `foo `, saving and typing on gives `foo bar`. The next save after the cursor leaves the
  line cleans it.
- **In a Markdown file, two or more spaces after text stay (Q15):** they are a line break there.
  A line of spaces alone, and a single trailing space, are still cleaned. A file is Markdown by its
  name (`.md`, `.markdown`, ASCII case ignored) until major modes come, and then by its mode.

#### Turning it on

- **Off by default, and on with a setting (Q14):**

  ```toml
  [save]
  trim-whitespace = true            # trailing whitespace on edited lines, blank lines at the end
  ```

  It applies to the next save after the file changes, as ADR 0110's settings do. Per-mode settings
  come with modes. The final newline stays on regardless, as ADR 0030 has it.
- **`delete-trailing-whitespace`** is a new command, `Change`, for a whole buffer or the shown
  regions, with the Markdown rule. It is for a file the person wants cleaned whole, and works with
  the setting off.

#### With the save

- **The cleanup is an edit made before the snapshot, as the final newline is** (ADR 0059), and one
  undo step with it. `C-z` after a save puts the whitespace back, and the buffer is then modified.
- **It runs for `save-buffer` and `C-x C-w`,** not for the save of an unmodified buffer whose file
  has gone (ADR 0059 writes that text as it was), and never for the emergency save, which writes
  the text exactly (ADR 0062).
- **It runs for the agent's saves too,** in a buffer the channel may save: it changes text oso
  already has, starts no process, and ADR 0085's rule about what a save starts does not apply.
- **In a buffer that may hold secrets it runs as anywhere;** nothing leaves the buffer.
- The safe write, its access list and quarantine flag (ADR 0059, ADR 0067) see only the text that
  results.

#### Cost

- **The edited lines are a set of ranges kept beside the buffer,** updated in ADR 0091's batched
  pass, and cleared when the file is read, reloaded or saved. 1000 cursors typing must stay within
  ADR 0091's edit budget, which the step measures with `text_perf`.
- **A save cleans each edited line from its end back,** so it costs the lines' count times a seek,
  plus the whitespace removed, not their length. When the edited ranges hold more than 256 KiB of
  text, as after pasting a large file, the scan runs on a worker over a snapshot, and the save
  starts when its answer comes, if the buffer has not changed since; otherwise it scans again. The
  buffer is marked as saving meanwhile, so a second save is refused, as ADR 0059 refuses it.
- `delete-trailing-whitespace` over more than 256 KiB scans on the worker the same way.
- **`oso dev` does not carry the set.** After a restart, lines edited before it are not cleaned.

### Where it lives

- **`oso-core`:**
  - `search` (still private) gains the per-line regexp matcher beside the literal one, and the
    case rules for regexps, and a private `replace` module the case-preserving rule. No part of
    `search` becomes public.
  - `editor/replace.rs`, a private module, with the session and the `Editor::replace*` methods;
    `Replace`, its read-only state, public as `Isearch` is; `Quit::Replace`; the batches of `!`
    as a job for the isearch worker.
  - `editor/macros.rs`, a private module: the recording, its bounds and its secret mark, the
    `Editor` methods the macro commands call, and the replay the app drains.
  - The undo history gains joining steps since a mark, used by both.
  - `editor/save.rs` gains the cleanup and `buffer` the edited ranges.
- **`oso`:** `commands/replace.rs`, `commands/macros.rs` and `bindings` for both; the dispatcher
  records events and feeds a replay, and holds keys during one; the app drains a replay within its
  budget; `config` reads `[save]`; `dev/state.rs` adds `replace` and `macro`; the channel's wait
  gains the two new states.
- No new crate, crate edge, dependency, public module or public trait. The new public items are
  listed by each implementing pull request, as the API snapshots show them.

### Tests and measures

- Unit tests for each rule above: the case rules for strings and regexps, case preserving, empty
  matches, the scope with a region and with several regions, every answer, holding keys while
  searching, `!` in batches with a change in between, undo as one step and `u`; recording, the
  bounds, passphrases, `C-g` in and out of the chain, every stop of a replay, waiting for a worker,
  undo joined, and secrets; the edited ranges under batched edits, the cursor's line, Markdown,
  the worker scan, and the setting.
- `just e2e` flows: `M-% foo RET bar RET y n !`; `C-M-%` with `$1`; `C-x ( … C-x ) C-x e e`
  with a count and an isearch inside; a save with `trim-whitespace` on.
- `text_perf`: `!` on 100 MB with 100,000 matches, each batch within 4 ms; 1000 cursors typing
  with the edited ranges kept; a save after pasting 100 MB.

## Questions

Put to the author on 2026-11-12, the recommended option first; each answer was the first.

### query-replace

1. **Regexp syntax.**
   (a) The `regex` crate's syntax, `$1` and `${name}` in the replacement. One syntax, documented
   by the crate, the same as completion's regexp style (ADR 0076). An Emacs habit such as
   `\(foo\)` means a literal parenthesis here.
   (b) The crate's syntax, plus Emacs's `\1` and `\&` in the replacement. Easier on Emacs hands,
   but two ways to write one thing.
   (c) Emacs syntax, translated to the crate's. Familiar, but a translator of its own to keep
   right, and Emacs's syntax classes (`\s-`, `\_<`) have no exact match.
2. **Matches across lines.**
   (a) One line at a time, lines up to 64 MiB. Bounded memory, and `^` and `$` work per line; no
   match can span lines.
   (b) Copy the whole scope into one string on the worker. Multi-line matches work, but a 1 GB
   buffer needs 1 GB more memory for each search.
   (c) Drive `regex-automata`'s DFA over the rope's chunks. Multi-line and no copy, but it is a new
   direct dependency, and AGENTS.md allows only the `regex` crate.
3. **Undo.**
   (a) One step per session, with `u` inside it. One `C-z` undoes a whole `!`.
   (b) One step per replacement, as Emacs. Finer, but undoing a long session takes as many `C-z`.
   (c) One step per answer, and `!` one step. In between, but `y y y` takes three `C-z`.
4. **A region and several cursors.**
   (a) Every shown region is the scope; with none, from the primary cursor to the end; only the
   primary moves, as in isearch. Consistent with isearch, and a region limits it as in Emacs.
   (b) Refuse with several cursors: "query-replace works with one cursor". Simple, but `C-g` first.
   (c) Drop the extra cursors when it starts. Quick, but loses them for good.
5. **Japanese in its prompts.**
   (a) Both prompts take Japanese, as the prompt for a file's name does (ADR 0106). Replacing
   Japanese text is a main use.
   (b) The input method stays off, as in every other prompt. Japanese then comes only by a yank.
6. **The agent channel.**
   (a) No replace command for it now; query-replace is `Person`. Nothing new to check.
   (b) Also `replace-string` and `replace-regexp`, `Change`, which replace the whole scope with no
   questions. Needs an argument of two strings, which `ArgSpec` does not have.

### Keyboard macros

7. **What is recorded.**
   (a) Input: keys and committed text, as Emacs. Prompts, isearch and query-replace work in a
   macro; a replay does what the keys do now.
   (b) Commands with their arguments and prompts' answers. Independent of keymaps, but isearch,
   query-replace and completion would each need their own recording.
8. **Undo.**
   (a) One step per replay, joined at its end. One `C-z` undoes `C-u 100 C-x e`.
   (b) One step per command, as Emacs. Simpler, but a long replay takes many `C-z`.
9. **A `y or n` question during a replay.**
   (a) The replay stops there, and the person answers. A replay never says yes to saving over a
   changed file or exiting.
   (b) The recorded answer answers it, as in Emacs. Faithful, but a `y` recorded for one file can
   save over another.
10. **Buffers that may hold secrets.**
    (a) Recording and replaying work there, and the macro is marked secret and never leaves the
    process. As isearch works there.
    (b) The recording is cancelled when such a buffer is shown, and a replay stops there. Safer,
    but no macro for editing a key file.
11. **`C-g` while recording.**
    (a) It takes its step in the chain and is recorded; with nothing else to do, it cancels the
    recording. A macro can hide a region or leave a prompt.
    (b) Any `C-g` cancels the recording, as Emacs. Familiar, but the chain's steps cannot be in a
    macro.
12. **Keys.**
    (a) `C-x (`, `C-x )`, `C-x e` with `e` to repeat, and `<f3>` and `<f4>`. Both Emacs sets.
    (b) The `C-x` keys and `e` only. Leaves the function keys free.

### Whitespace cleanup on save

13. **Which lines.**
    (a) Lines edited since the file was read or saved, as `ws-butler`. No noise in diffs of others'
    files, and cheap on any size.
    (b) The whole buffer, as `delete-trailing-whitespace` in `before-save-hook`. Thorough, but
    every save of a messy file rewrites all of it, and a large one needs the worker each time.
    (c) The whole buffer, only if it had no trailing whitespace when read; edited lines otherwise.
    Keeps clean files clean, but harder to predict.
14. **How it is turned on.**
    (a) `[save] trim-whitespace = true` in `config.toml`, off by default. One setting, per mode
    later.
    (b) A global toggle, `M-x whitespace-cleanup-mode`, which the setting starts, as the aids do.
    More to build, and switchable for a session.
    (c) On by default. Most editors' choice for code, but it changes files the person did not
    ask to change.
15. **Markdown's two-space line breaks.**
    (a) Kept: in a Markdown file, two or more spaces after text stay.
    (b) Cleaned as anywhere. Breaks hard line breaks.
    (c) No cleanup in Markdown at all. Simple, but leaves real trailing spaces.
16. **The cursor's own line.**
    (a) Whitespace before a cursor on its line stays. Typing on after a save keeps its space.
    (b) Cleaned too, and the cursor moves to the line's new end, as Emacs's
    `delete-trailing-whitespace`. Predictable, but `foo ` + save + `bar` gives `foobar`.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. When this ADR is
accepted, the status of each ADR named gains `amended by 0113`.

- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "The person is in the middle of something": also
  while a query-replace session is on, or a keyboard macro is recorded or replayed.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), the `C-g` chain: it gains
  `Quit::Replace` after isearch's steps. With a replay running, a `C-g` from the window stops it
  first. With a recording on and nothing else to do, `C-g` cancels it.
- [ADR 0106](0106-the-input-source-turns-japanese-on-in-a-file-name-prompt.md), "Every other
  prompt keeps it off": query-replace's two prompts take Japanese too.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md), "The final newline is an edit of its own":
  the cleanup joins it in the same step, and a save may wait for a worker's scan before its
  snapshot.
- [ADR 0082](0082-isearch.md), "M3's query-replace and project search decide what of it becomes
  public": none of it does; query-replace's types are in `editor`.

## Consequences

- query-replace works as Emacs's on any buffer size: each answer searches at most 256 KiB on the
  UI thread, and `!` over 1 GB replaces in batches that keep frames coming.
- A regexp never matches across a line break, and a line over 64 MiB ends a regexp session.
- Emacs's regexp habits do not carry over: `\(` is a literal parenthesis.
- A macro replays keys, so a key rebound since recording does what it does now. A completion's
  order, which recency changes, can make a replayed `RET` pick another candidate, as in Emacs.
- A replay waits for every worker and clipboard read it causes, so it is slower than Emacs's on a
  large buffer, but does the same thing each time.
- A macro is lost at an `oso dev` restart and at exit.
- Whitespace cleanup never touches lines the person did not edit, so a file that was messy stays
  messy except where it was edited. `delete-trailing-whitespace` cleans the rest on demand.
- Tracking edited lines adds work to every edit, which the batched pass must absorb within budget.
- Follow-up work: regexp isearch (`C-M-s`) can use the new matcher; the kmacro ring, counters and
  editing; `E` and `C-r` in query-replace; per-mode whitespace settings; `.editorconfig`.

## Alternatives considered

- **Regexps over the whole buffer copied to a string on the UI thread:** breaks the rule that it
  never waits.
- **A new rope built by the worker for `!`, swapped in as a reload's text is:** O(1) on the UI
  thread, but the cursors, marks and undo record of a whole-buffer swap lose every position, and
  batches keep them.
- **Wrapping query-replace around the buffer's end:** Emacs does not, and the count would cross
  the starting point.
- **Each cursor running its own query-replace:** the questions of many cursors would interleave.
- **A replace that needs no answers for the agent now:** a two-string argument is new to `ArgSpec`
  and the channel, for a use the agent's own file tools cover.
- **Recording commands rather than keys:** see Q7.
- **Replays that do not wait for workers:** a replayed `C-s` past 256 KiB would act on where the
  cursor was before the match came, and differ from run to run.
- **Feeding a whole replay in one turn of the loop:** `C-u 0 C-x e` would freeze the window until
  it ended.
- **Carrying the macro through `oso dev`:** another item kind in ADR 0090's handoff, and a secret
  one could not go; recording again is cheap.
- **Whitespace cleanup as a mode in `[aids]`:** the aids show things and change no text.
- **Trimming in the written file but not the buffer, as `ws-butler` restores point's whitespace:**
  the buffer would differ from the disk after a save, which ADR 0059 rules out.
- **Treating every Unicode space as whitespace to clean:** a full-width space may be meant in
  Japanese text.
