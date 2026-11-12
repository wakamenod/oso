# 0096. Completion on a worker past 5000 candidates

- Status: Accepted, amended by 0114
- Date: 2026-11-09

## Context

[ADR 0076](0076-completion-and-m-x.md) matches a prompt's candidates on the UI thread up to 5000,
and fixes what the worker past 5000 must do: each input has a generation, the worker gets the list
as an `Arc` snapshot with the input and its generation, and the UI takes a result only if its
prompt is still open and its generation is the newest. Until a result comes, the rows shown stay
and the count shows `…`. Results arrive through the `Status` comparison.
[ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md) made `C-x C-f` the first
list that can pass 5000, bounded at 100,000 names, and left the worker unbuilt. Both still run on
the UI thread.

Measured on a MacBook in a release build, with a prompt listing 100,000 names:

| Work | Time |
|---|---|
| Building the list: expansion, candidates, history, length and name order | 23 ms for ASCII names, 57 ms for Japanese and accented ones |
| A key that narrows the input | 1–3 ms |
| A regular-expression component such as `a.*` | up to 10.5 ms a key |
| Deleting a character of one, which matches the whole list again | about 9 ms |
| The empty input | 0.2 ms |

The list is built each time the directory changes, and each time a batch of looks fills in the rows'
annotations. The budget is 8 ms from key to photon (p99), and 1 ms of matching a key
(ADR 0076).

## Decision

The author decided this on 2026-11-09, through the coordinator.

### Where it runs

- **On a thread of its own, `oso-complete`.** It starts with the first job and then runs each in
  turn. A job sent while one runs supersedes it. A job superseded before it runs stops at its
  first look, so running jobs in order costs the newest little, and its snapshots are freed on
  the worker. The host side is `crates/oso/src/app/completions.rs`, built as isearch's
  `oso-search` is ([ADR 0082](0082-isearch.md)). The two threads share nothing: a prompt and
  isearch never run at once, and keeping them apart leaves each one's jobs and generations its
  own.
- **What a long list leaves is freed there too.** A list past 5000 that a new one replaces, the
  list of a prompt that closes, and a report dropped go to the worker as a job that only frees, so
  100,000 names are never dropped on the UI thread. A newer job never takes the place of one that
  frees.
- **A job that panics is reported,** as the lister's and the loader's are caught: the prompt stops
  waiting and says so, a held key is dropped, and the thread goes on. If the thread cannot start,
  the job runs on the UI thread, slower but answering.
- **Past 5000 candidates, decided for each list.** A list of at most 5000 is built and matched on the
  UI thread as now, so `M-x`, `C-x b` and small directories take no thread and no `…`. For `C-x C-f`,
  the size is the sum of the listings the path's segments reach, counted before anything is
  expanded.
- **Both the build and the matching move.** A job to build gets the directories the path's
  segments reach with the `Arc<[Name]>` the lister gave each, the history, the input, and the path
  of the row selected. On the worker, it makes the candidates, orders them, matches the input, and
  finds the row selected in the new list, so it stays selected. Reaching the directories stays on
  the UI thread, as it looks only at the names of directories on the way, without allocating for
  each, and orders only the first past the 32 it keeps. A job to
  match gets the list's `Arc` snapshot, its order, the input, and the matches of the input before it
  when the new input only narrows that one (ADR 0076's narrowing). It gives back the matches.
- **A look patches its rows.** The annotations that looks find, and a link found to lead to a
  directory, are kept beside the list by row. A batch of looks changes how those rows read and
  whether `RET` goes into them, never the list's order or what matches. So it needs no job, and
  costs the UI thread only its rows.
- **The bound stays 100,000 names,** for memory: a worker makes the time no longer the reason, but
  the list and its snapshot are still held twice while a build runs. It holds as the list is made,
  so 32 directories of 100,000 names make no name past it.

### The job and its report

- **`oso_core::complete` makes the job and takes in the report.** The host only runs one and hands
  the other back, as isearch's are (ADR 0082). Two public types go in that public module,
  `CompletionJob` and `CompletionReport`, with `Candidates::pending` for the count. The editor
  gains `Editor::take_completion_job`, `Editor::completion_reported`, `Editor::completion_failed`,
  and `Editor::completion_pending` for the count and the keys held. The job depends on nothing of
  the editor, which ADR 0076 kept `complete` free of for this.
- **The generation is shared with the worker.** The UI raises it with each job, and the worker reads
  it before a job starts, every 1024 candidates of each step, every 1024 comparisons of the sort,
  and once more before it reports. A job superseded midway stops there and reports nothing.
- **A report is taken only if its prompt is still the innermost, its generation is the newest, and
  the list it matched is the one the prompt has.** Any other report is dropped. A report for a
  prompt that another covers is dropped, and that prompt is matched again, or its list built
  again, once it is the innermost, so it never waits for a result that cannot come. A path that
  needs a listing stops the build for the path before, as that list is not its.
- **`Debug` of a job or a report gives its generation only,** and nothing logs what a worker
  reports. A list can name files the person keeps private.

### Keys while a result is pending

- **The rows shown stay, and the count shows `…`** (ADR 0076). `C-n` and `C-p` move over the rows
  shown, and a row moved to stays selected when the result comes if it still matches, as it was
  chosen after the input was typed.
- **`RET` and `TAB`, which take the selected candidate, are held** while a result is pending, and
  run when the newest result comes. With a cached build that is within about 10 ms at 100,000 names.
  `M-RET` gives what is typed, needs no candidate, and is not held.
- **Any other key drops the held key,** as it comes and before it is resolved: an edit asks for a
  new result, a move with `C-n` or `C-p` changes the candidate the key was to take, and a prefix
  or an unbound key begins something else. Text typed drops it too. **`C-g` drops it** and does
  nothing more, as `C-g` does one thing at a time
  ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)); a second `C-g` quits the
  prompt.
- **A held key runs as it would have, on the new result:** `RET` on a directory goes into it, `RET`
  on no match gives the typed path when the prompt takes any text, and `TAB` puts the candidate in.
  A held key is never run by a result for another prompt, nor once the app is leaving. It also
  runs when the wait ends with no report, as when a listing comes and the list it brings is
  short enough to build at once.
- **A key from the dev socket is never held:** it runs on the rows shown, and a client that wants
  the newest waits until `get-state` says nothing is pending. A key held and run later would open
  what it opens outside the request that sent it. `run-command` of `RET`'s or `TAB`'s command is
  refused while a result is pending.

### Tests and measurement

- **Unit tests run the job synchronously:** they take the job from the editor, run it on the test's
  thread, and hand the report back. The tests cover:
  - a superseded generation dropped, and a superseded job stopping;
  - a closed prompt, and one covered by another and matched again once it is the innermost;
  - a narrowing job that gives what a full pass gives;
  - a held `RET` and `TAB`, and an edit, a move, a prefix, an unbound key or `C-g` dropping them;
  - a row moved to while waiting, kept by the result;
  - a look patching rows without a job;
  - the 5000 line, crossed both ways as the path changes;
  - a list replaced or closed freed by a job of its own;
  - the bound holding as the list is made;
  - no name or input in `Debug`.
- **Ignored tests, run by hand in a release build,** check that the UI thread's share is under
  1 ms at 100,000 names: of a key, which makes and sends the job and takes the report in, and of
  taking in a list in place of another. On a MacBook they measure about 60 µs and 35 µs.
- **`just perf` gains `file_prompt_type_p99_ms`, reported only,** until the author sets a threshold.
  It fills a temporary directory with 100,000 empty files whose names mix ASCII, Japanese and
  accented letters. It then opens `C-x C-f` in that directory and types keys in turn, a
  regular-expression component among them. Each key is timed until the frame that draws it.

## Changes to accepted ADRs

- [ADR 0076](0076-completion-and-m-x.md), "Where matching runs", past 5000 on a worker thread:
  - the list's build runs there too;
  - for `C-x C-f`, the size that decides is counted before the build;
  - `RET` and `TAB` are held while a result is pending.

  What it fixed of generations, snapshots, `…` and the `Status` comparison stands.
- [ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md), "Listing on a worker":
  - a look's annotations, and a link that leads to a directory, are kept beside the list and patch
    its rows without building it again;
  - the bound of 100,000 names stays, for memory.

## Consequences

- One more thread, idle until a list passes 5000, and alone with its own jobs.
- A key in a long list shows its result a few milliseconds later than it would on the UI thread. In
  exchange, the frame for the key, the cursor and the typed text is never late.
- A key held by `RET` or `TAB` shows nothing until the result comes. The count's `…` says why.
- `complete` gains two public types and a method, and the editor four public methods. The host
  needs a `UserEvent` and a file of its own, and the dispatcher a file for the keys it holds.

## Alternatives considered

- **Sharing `oso-search` through the job kinds ADR 0091 adds:** saves no time, as a prompt and
  isearch never run at once. It would tie a key in a prompt to isearch's job and generation, and
  edit the files the cursor keys change.
- **Matching only on the worker:** leaves 23–57 ms on the UI thread each time the directory changes
  or looks come in, which is the larger cost.
- **`RET` on the rows shown:** could open a file the newest input no longer matches.
  **Refusing with `[Matching…]`:** makes the person press `RET` again for a wait of milliseconds.
- **A worker for every file prompt:** a small directory would show `…` for no gain, and its tests
  would need a thread.
- **Matching in parallel on several threads:** needs a dependency or a pool of its own, for a cost a
  single worker already takes off the UI thread.
- **Ordering only the rows shown (a partial sort):** `C-n` can walk the whole list, so the full order
  is needed anyway, and the build's other steps would still cost the UI thread.
