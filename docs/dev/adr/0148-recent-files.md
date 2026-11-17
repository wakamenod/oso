# 0148. Recent files

- Status: Accepted
- Date: 2026-11-17

## Context

The author asked for recent files in the style of Emacs's recentf and consult-buffer. `C-x C-r`
should open a recently visited file, since many Emacs configurations bind `recentf-open` there.
`C-x b` should list recent files after the buffers, as consult-buffer does. Nothing is bound to
`C-x C-r` today.

What is fixed already:

- [ADR 0088](0088-switching-and-killing-buffers.md), Q1: `C-x b` lists buffers, with "no recent
  files yet". Its alternatives say recent files wait until a history is kept on disk.
- [ADR 0013](0013-persistence.md): no file's text is written to disk but an emergency copy.
- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md) and `oso-core::secret`: a file's
  names decide whether it may hold secrets. The loader judges a visited path from where it
  leads.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): every command has a reach. The channel's
  `state` gives a prompt's kind only.
- [ADR 0118](0118-what-an-agents-open-or-save-starts.md): an agent's open counts as the person's
  from their first command in that buffer (`Editor::note_persons_command`).
- [ADR 0121](0121-agent-sessions-and-spaces.md), "The record at an exit, as built": every exit,
  and `oso-restart` before it starts the new instance, waits at most 1 s for the sessions
  record to be written.
- [ADR 0146](0146-a-state-directory-of-its-own-for-checks.md): `privacy::state_dir()` follows
  `OSO_STATE_DIR`, so checks and verify runs write a scratch directory.

## Decision

The author chose the first option of each question under [Questions](#questions) on
2026-11-17, through the coordinator, and approved `ArgSpec::RecentFile`.

### What is kept

- **Files the person visits.** A file goes first in the list when:
  - a person's command opens it and its load ends with its text. This covers `C-x C-f`,
    `C-x C-r`, `oso FILE`, dired, `C-x p f`, and grep, xref and server jumps.
  - a person's command leaves its buffer in the selected window, as `C-x b`, `M-,` or
    `C-x o` do.
  - the person's save of it ends, which covers a new file's first save and the new name after
    `C-x C-w`.
- **Not kept:**
  - buffers with no file, internal buffers and terminals;
  - a file still loading, or a new file not yet saved;
  - a buffer by being carried over a restart. A carried buffer goes in only at the person's
    first command in it, as any buffer shown does.
- **Secrets:** a file goes in only once the loader has judged it `Verdict::Ordinary`, that is,
  no name on its path or where it leads matches the patterns. `SecretName`, `Unresolved`,
  `Unjudged` and `Yanked` buffers never go in. The list read back from disk is judged by its
  names again, so a file that later, wider patterns match leaves it.
- **Restricted and untrusted projects** count like any. Trust decides what runs, and a recent
  file opens through `C-x C-f`'s path, which runs nothing.
- **The agent channel moves nothing.** A file an agent opened goes in at the person's first
  command in its buffer, the moment ADR 0118 lets an agent's open go. Each visit a command asks
  for is an `AskedVisit` that says who asked (`Acted`) and whether it was chosen from the list.
  A visit asked for by a command the channel ran is not kept when it loads.
- **Each path once,** absolute as visited (made absolute without touching the disk, as ADR
  0088's distinct names are), never canonicalized. Only paths in UTF-8 of at most 4096 bytes
  with no control character are kept. At most **100** files; the oldest goes first.

### On disk

- **`oso/recent-files`** in the per-user state directory, beside `theme` and `sessions`. It holds
  one JSON object a line, `{"path": "/abs/path", "shown": <ms since the epoch>}`, the most
  recent first. It never holds a file's text.
- **Read** on a thread of its own, started with the app, and taken after the next event once
  it is done; the UI thread never waits for it. At start the window's own first events come
  after it. It is read with `sessions::record`'s helpers, as `sessions` is: only if the
  directory is private and the file a regular one of this user's with one name, at most 256
  KiB. A line with no whole entry, or with a path the editor would not keep (not absolute, too
  long, a control character, a name that may hold secrets), is left out and counted in the
  log. Until the read comes back, the lists hold what this instance has kept since it started;
  what is read goes after those.
- **Written** by a thread of its own, which runs on every OS (the sessions worker runs on Unix
  only). It writes 1 s after the last change, only the newest list. It replaces the file whole,
  through `recent-files.part` with mode `0600` and an fsync, or leaves it as it was. A write
  reads the file first, with the same checks, so it never copies back a line this build would
  not keep. It keeps what another instance wrote, each path at its latest stamp,
  the most recent first, at most 100 and 256 KiB. A file this instance dropped stays only if
  another instance showed it after the drop. A file there that may not be read is not replaced.
- **Flushed** at every exit and before `oso-restart` starts its new instance, together with
  `sessions` and `spaces`: all three writes run at once, and the wait for them is at most 1 s in
  all, as ADR 0121's was for its two (`App::flush_records`). The same gap remains: `oso dev`'s
  supervisor starts its new instance itself.
- **Pruning:** nothing is checked on disk to list the files, so listing never waits on a slow or
  unmounted volume. A file chosen in `C-x C-r` or `C-x b` is asked for with
  `Editor::request_recent_visit`, which marks that one visit. If its load finds the file gone,
  the echo area says `~/notes/a.md no longer exists`, the file leaves the list, and nothing
  opens, where `C-x C-f` would open a new file. The mark goes with the visit's buffer, so a
  later `C-x C-f` of the same path opens a new file as ever. A file found large asks first, as
  `C-x C-f` does, and its `y` visits it as `C-x C-f` would.

### `C-x C-r`

- **`recentf-open` on `C-x C-r`** reads a recent file in ADR 0076's centred float, labelled
  `Open recent file:`, with the history `recent-file` and no preview, as `C-x C-f` has none.
  It reads a new `ArgSpec::RecentFile { prompt, history }`.
- **A row** is the path from the home directory as `~/` (`complete::path::typed_form`),
  annotated `open` when a buffer visits that file.
- **The order is recency,** the current buffer's file last, so `C-x C-r RET` opens the file
  before, as `C-x b RET` goes to the buffer before.
- **Only a listed file is taken:** with no match, `RET` says `[No match]`. A file a buffer visits
  shows that buffer; any other is visited as `C-x C-f` visits one, with its large-file
  question.
- It refuses inside a prompt, as `C-x b` does.

### `C-x b`

- **After the buffers, the recent files no buffer visits,** in the list's order, as
  consult-buffer's file source. A row is the `~/` path annotated `file` in the annotation's
  face; a buffer's annotation says its size and mode, so the two kinds read apart.
- **The preview of a file row says `not open`** in the annotation's face (`Preview::NotOpen`).
  Nothing of the file is read.
- **`RET` on a file row opens it** as `C-x C-r` does. A name typed that matches no buffer and no
  listed file still makes a buffer. `C-x b RET` on an empty input still goes to the buffer
  before. A buffer whose name is a listed file's row shows the buffer.
- `C-x k`, `M-,` and `M-/` list and walk buffers only.

### Agents and the sockets

- **`recentf-open` is `Person`.** It picks from a list, and the agent has `open`.
- **The channel never reads the list.** No tool lists it, `state` gives a prompt's kind only,
  and the channel's `switch-to-buffer` resolves a buffer's name and never opens a file
  (ADR 0094). The command itself also opens no file when it runs for the agent, so it stays
  `Look` should that ever change. Like any message, the echo area's `… no longer exists`, which
  only the person's choice causes, can be read through the channel's `state`, as `Cannot open:`
  can.
- **The dev socket's `get-state`** gives these prompts' rows as it gives `C-x b`'s, and
  `preview_kind` `not-open` for a file row. All of it is withheld while a buffer that may hold
  secrets is shown, as before. Its `open-file` counts as the person's visit, as its `send-keys`
  counts as the person's keys (ADR 0118).

### Where it lives

- **`oso-core`:** `editor/recent_files.rs` keeps the list in memory with no clock or I/O:
  - `Editor::recent_files`, `note_recent_visit`, `remember_recent_files`,
    `forget_recent_file` and `take_recent_changes` (`RecentChanges`);
  - `recent_file_named` and `recent_file_row`;
  - `MAX_RECENT_FILES` and `MAX_RECENT_PATH`.

  `editor/visits.rs` adds `AskedVisit`, `Editor::take_asked_visits` and
  `Editor::request_recent_visit`, and keeps its visits and the large file in one `Visits`.
  `ArgSpec::RecentFile` and `Preview::NotOpen` are new. `note_persons_command` puts the
  selected window's file first.
- **`oso`:**
  - `app/recent_files.rs` reads, stamps, merges and writes the file and holds the person's
    visits still loading;
  - the command is in `commands/files.rs` and its key in `bindings/files.rs`;
  - the candidates are in `dispatch/buffers.rs`, and the prompt, with `C-x b`'s and `C-x k`'s,
    in `dispatch/prompts/buffer.rs`;
  - `sessions::record::{read_file, replace_file}` are shared with the recent files;
  - the preview's pane is in `app/draw/float`.
- No new crate, crate edge, dependency, public module or public trait.

### Tests

- `oso-core`, in `editor/recent_files.rs`:
  - order, a repeat of the first, the cap;
  - what is never kept: secret, unresolved or unjudged verdicts, not on disk, no file,
    relative, control characters, an agent's pending open;
  - the agent moving nothing;
  - the read-back list judged again;
  - a file found gone dropped;
  - rows.
- `oso`, the record:
  - round trip in mode `0600`, newest first;
  - another instance's files kept, and a drop that holds only until another instance shows the
    file again;
  - the cap, bad lines, a path too long;
  - a hard-linked, symbolic-linked or oversized file and a directory others can reach, neither
    read nor replaced;
  - another instance's lines with a name that may hold secrets or a control character, not
    copied back;
  - the writer waiting for quiet and a flush writing at once.
- `oso`, scenarios run by keys:
  - `C-x C-r`'s order and choice, marked as from the list, only a listed file, refused inside
    a prompt;
  - a large recent file asking first, and a `C-x C-f` visit never marked;
  - the current file last, `open`;
  - `C-x b`'s file rows, their `not open` preview and `RET`; `C-x k` with no files;
  - a person's command putting a file first;
  - a file that may hold secrets never kept;
  - the channel neither opening nor listing one.
- Text-grid snapshots of `C-x b` with file rows and of `C-x C-r` (`buffers_recent_files`,
  `recent_files_c_x_c_r`).
- A flow (`just e2e`), in a scratch `OSO_STATE_DIR`:
  - open two files and quit at once; start again, and `C-x C-r` lists them newest first and
    `RET` opens one;
  - `C-x b` shows a file row and opens it;
  - a file removed since says it is gone and leaves the list.

## Questions

Put to the author through the coordinator, the recommended option first. The author chose (a) on
each.

1. **What is kept:** (a) files a person's commands open whose load succeeds, a new file at its
   first save, and the new name after `C-x C-w`; not buffers with no file, internal buffers,
   emergency copies or a restart's carried buffers; (b) also what a restart carries.
2. **Secrets:** (a) only a file judged `Ordinary`, judged again when read back; (b) every path,
   as a path is not text.
3. **Restricted and untrusted projects:** (a) kept like any; (b) trusted projects only.
4. **An agent's open:** (a) kept at the person's first command in its buffer; (b) never; (c) at
   once.
5. **Order and size:** (a) a file goes first when a person's command opens it or shows its
   buffer, 100 kept, no setting; (b) only on open, as recentf; (c) a setting for the count, 0
   for none.
6. **Writes:** (a) 1 s after the last change, and every exit and a restart's start wait at most
   1 s; (b) only at exit.
7. **A missing file:** (a) kept until chosen, then dropped with a word; (b) a look at start drops
   the missing ones.
8. **`C-x C-r`'s rows:** (a) the `~/` path, `open` when a buffer visits it, newest first, the
   current file last, only a listed file taken; (b) paths from the project's root; (c) the name
   with its directory as the annotation.
9. **`C-x b`:** (a) the files no buffer visits after the buffers, marked `file`, previewed as
   `not open`; (b) group titles; (c) (a) with consult's narrowing prefix.
10. **Agents:** (a) `recentf-open` is `Person` and the channel never reads the list; (b) `Look`.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. The status of ADR
0088 gains `amended by 0148`.

- [ADR 0088](0088-switching-and-killing-buffers.md), "Recent files are not listed until a
  history is kept on disk" and Q1 (a), "and no recent files yet": `C-x b` lists, after the
  buffers, the recent files no buffer visits, and opens one on `RET`.
- ADR 0088, "Recent files in `C-x b`: consult-buffer's, but it needs a history on disk, which
  nothing keeps yet": `oso/recent-files` keeps it.
- ADR 0088, "**A buffer that may hold secrets is never previewed**" and the preview's other
  cases: a recent file's row previews as `not open`, and nothing of it is read.

## Consequences

- `C-x C-r` and `C-x b` reach files visited in earlier sessions, the most recent first.
- Paths reach the disk, never text, and no path of a file that may hold secrets. The file is
  this user's alone, as `sessions` is; anyone who can read it learns which files the person
  opened lately.
- A person's command costs the UI thread a look at whether its file is first already; only a
  file that moves is judged and moved, a scan of at most 100 paths. The stamps, the read-back
  list and the files buffers visit are matched through hash sets, and a preview's lookup of a
  row makes no string. Listing reads nothing. Every write and read is off the UI thread, apart
  from a wait of at most 1 s at an exit or a restart, the same as before.
- A file chosen from the list that is gone opens nothing, so a typo in history never makes a
  new file.
- Agents cannot read which files the person visited, beyond the echo area's word on a chosen
  file found gone.
- `oso-core`'s public API gains `RecentChanges`, `AskedVisit`, `MAX_RECENT_FILES`,
  `MAX_RECENT_PATH`, `ArgSpec::RecentFile`, `Preview::NotOpen`, `Editor::take_asked_visits`,
  `Editor::request_recent_visit` and the recent-file methods listed above.

## Alternatives considered

- **Recording only opens, as recentf does:** simpler, but a file worked in all day after a quick
  look elsewhere would come after that look.
- **A setting for the count, or none:** more control, but one more key with no request for it.
  Deleting the file empties the list.
- **Looking at each file at start, dropping the missing:** a cleaner list, but a look per file,
  and a hang on an unmounted volume would hold the read.
- **Group titles in `C-x b`:** consult's look, but rows that cannot be selected are new to
  ADR 0076's completion.
- **Paths from a project's root:** shorter, but ambiguous across projects.
- **Previewing a recent file by reading it:** consult's, but it reads files from a list moved
  through, which may be on a slow volume, and ADR 0088's preview shows open buffers only.
- **`Look` reach for `recentf-open`:** an agent could open a file it does not know of by name,
  but the list says what the person visited, which no tool gives out.
