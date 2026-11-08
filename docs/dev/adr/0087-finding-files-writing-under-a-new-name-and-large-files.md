# 0087. Finding files, writing under a new name, and large files

- Status: Accepted
- Date: 2026-11-08

## Context

M2-18 builds `C-x C-f` with path-segment completion on a worker thread, `C-x C-w`, and asking
before opening files of 2 GiB or more ([roadmap](../roadmap.md)). Until now files open only as
`oso FILE`, the dev socket's `open-file` and the agent channel's `open`.

What is fixed already:

- [ADR 0030](0030-m2-editing-commands.md): `C-x C-f` completes by path segment, as Emacs's
  `partial-completion` style does: `~/P/S/o` expands segment by segment.
- [ADR 0076](0076-completion-and-m-x.md): the orderless matcher, its order (history,
  then length, then alphabetical), the centred float of 18 candidate rows and the answer's row,
  the keys `C-n`, `C-p`, `TAB`, `RET` and `M-RET`, and the worker path past 5000 candidates, with a
  generation per input and an `Arc<[Candidate]>` snapshot, which M2-18 builds.
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md): only regular files are opened, a
  missing file opens an empty buffer, and a loading thread per file reads it.
- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md): every path on the way counts for
  the secret verdict, a path that cannot be resolved counts as secret, and "saving under a new
  name must resolve the new path".
- [ADR 0051](0051-m2-plan-decisions.md): files of 2 GiB or more ask before opening, as running
  out of memory aborts and no emergency save runs.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md): the order of a write, the baseline, and
  "the text goes only where the file was". A buffer with no file says `Buffer has no file name`
  until `C-x C-w`.
- [ADR 0062](0062-emergency-save-and-the-panic-hook.md): `C-x C-w` changes a buffer's path with
  no text change, and must give its emergency entry a new identity.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md): a question opens only from
  a command, never from a background result, which says what it found and which key asks.
  Commands that change what a window shows refuse inside a prompt.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): every command has a reach, and the channel has
  its own `open`, which opens only files that exist.

The loader checks that a path is a regular file and then opens it. A path swapped for a special
file between the two blocks the loading thread (not the UI), which `open-questions.md` records.

## Decision

The author accepted this ADR on 2026-11-09, through the coordinator.

The author chose these on 2026-11-08, through the coordinator: the first option of each question
under [Questions](#questions), which the text below describes. The author also approved, in
principle, the change to opening below, through `oso_platform::file_meta`, with no new
dependency. The new public items listed under [Where it lives](#where-it-lives) are named in the
pull request for the author's approval.

### The prompt

- **`find-file` on `C-x C-f`** reads a path in a prompt that completes, in ADR 0076's centred
  float, labelled `Find file:`. The new `ArgSpec::File { prompt, history }` asks for it, and the
  command reads the answer with `ctx.answer()`. Its history is `file`, under ADR 0072's rules.
- **The initial input is a directory:** the directory of the shown buffer's file, or, for a
  buffer with no file, the directory oso was started in, read once at startup. The home
  directory is shown as `~/`. From a row of `*Emergency Copies*`, the input is that row's path
  ([ADR 0064](0064-recovering-an-emergency-copy.md)).
- **The input row reads** `12/80 Find file: ~/notes/` as ADR 0076's does, and `…/…` while the
  worker has not answered.
- `C-x C-f` refuses inside a prompt, "Not in the minibuffer", as `C-x b` does (ADR 0072).
- There is no preview.

### Matching a path

- **The input splits at `/` into segments.** Each segment before the last is a prefix of a name,
  with smart case over that segment: `~/P/S/o` reaches `~/Projects/SideProjects/oso`. A segment
  that is a whole name in its directory matches only that name, so `~/Projects/` lists `Projects`
  and not also `Projects-old`.
- **A segment before the last that matches several directories expands into each of them,** in
  the order of the names, at most 32 directories per segment. Past that, the ones left are not
  listed and the count says `+`.
- **The last segment uses ADR 0076's matcher** over the names in the directories the earlier
  segments reached: components split at spaces, any order, the affixes, and smart case.
- **What the rows show:** the name, with a trailing `/` for a directory. When an earlier segment
  matched several directories, a row shows the path from that segment on, as
  `Projects/SideProjects/oso/`, so two rows with one name can be told apart.
- **Starting over:** `~/` typed right after a `/` starts the path over at the home directory, and
  `/` typed right after a `/` starts it over at the root, as Emacs's shadowed file names do. The
  text before it is drawn dim and ignored, and stays in the answer until deleted.
- **`~` is the home directory** only as the first segment, from `$HOME` read once at startup. With
  no `$HOME`, `~` is a plain name, and the echo area says so once. `~user` is a plain name. `$VAR`
  is never expanded: `$` is a character like any other.
- **Hidden names,** those starting with `.`, are listed only when the last segment starts with
  `.`. `.` and `..` are never listed; typing `../` goes up as the system does.
- **No ignore list** (Emacs's `completion-ignored-extensions`) until configuration (M3).
- **Names are matched as they are stored,** code point by code point. A name stored decomposed
  (NFD), as older macOS volumes and zip archives often keep them, does not match the composed
  form an input method types. This is recorded in `open-questions.md`.

### Listing on a worker

- **The UI thread never touches the file system.** A prompt asks for a directory's names; a
  worker thread lists it with `read_dir`, and the names come back as an `Arc<[Candidate]>`
  snapshot with the input's generation (ADR 0076). The UI takes a result only if its prompt is
  still open and its generation is the newest. Results arrive through the `Status` comparison
  (ADR 0063), as a load's progress does.
- **One lister for the app, at most 4 threads alive at once, and one request in flight per
  directory.** A directory on a file system that hangs stalls only its own thread, and its next
  requests wait without holding up another directory's; the count shows `…`. A newer request for
  a directory replaces one still waiting, so a late generation never waits behind the newest.
  With 4 alive, a new listing waits for one to end. Listings are kept for the prompt's life,
  keyed by directory, and a prompt that closes drops what it left waiting.
- **Bounded:** 100,000 names per directory; past that, the rest are left out and the count says
  `+`. Matching follows ADR 0076: on the UI thread up to 5000 candidates, on a worker past that.
- **Nothing is opened.** A listing reads names and their types, as `read_dir` gives them. The rows
  shown, at most 18, are looked at with `symlink_metadata`, and `metadata` and `read_link` for a
  link, on the worker, for their annotation, their `/` and their size. A listing reports its
  error in the input row's message, such as `[Permission denied]`, and lists nothing.
- **Names are drawn as the echo area draws them** ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md)):
  bidirectional controls and invisible characters boxed, control characters escaped, so a name
  cannot forge a row or hide a part of itself (security.md, T3).

### Rows

- **Order:** ADR 0076's, directories mixed in, as vertico's: paths in the `file` history first,
  the most recent first, then the rest by length, then alphabetically.
- **Annotation,** as marginalia's for files: the mode as `ls` writes it, the size (`1.2k`,
  `3.4M`), and the age (`3 days ago`), with the owner only when it is not the user. Rows not yet
  looked at show no annotation, and fill in when the worker answers. The prompt keeps these
  beside its candidates, not in `Candidate`, so the `&` affix matches no file's annotation.
- **A symbolic link** shows `→ target` in its annotation, as `read_link` gives it. A link to a
  directory ends in `/` and can be entered. A link to nothing shows `→ target (missing)`, and
  opening it visits the target as a new file, judged as ADR 0044 says.

### Keys

ADR 0076's keys hold, with these for a path:

- **`TAB`** puts the selected candidate's whole path in the answer, the expanded segments
  included, and a directory ends in `/`, so its names list next.
- **`RET` on a directory enters it,** as `TAB` does, as there is no directory editor yet.
- **`RET` on a file opens it.** With no match, `RET` opens the path as typed: a new buffer for a
  file that does not exist yet, and the echo area says `(New file)`. A missing directory is not
  created; saving there fails as ADR 0059 says.
- **`M-RET`** gives the text as typed, whatever matches, so `foo` can be created while `foobar`
  matches.
- **`C-p` on the first candidate selects the typed text,** drawn as the selected row on the input
  row, and `RET` then gives it, as vertico does when a match is not required. `C-n` goes back to
  the candidates.
- `DEL` and the other editing keys are unchanged.

### A file already open

- **`C-x C-f` of a file a buffer already visits shows that buffer.** When the answer, made
  absolute without touching the file system, equals a buffer's path, this happens at once.
- **Otherwise the loader tells:** once it has opened the file, it compares the file's device and
  inode with the files open buffers hold (the baseline's, ADR 0059 and ADR 0083). If one holds
  the same file, the new load is dropped, the window shows that buffer, and the echo area says
  `notes.md is already open as notes.md<work>`. So a symbolic link or a second spelling never
  gives two buffers on one file, which could save over each other.
- The agent channel's `open` and the dev socket's `open-file` follow the same rule.

### Files of 2 GiB or more

- **The limit is 2 GiB, 2³¹ bytes,** until configuration sets it.
- **`C-x C-f` asks at once when it knows the size:** a row the worker looked at, selected when
  `RET` is typed, gives it. The question is `big.iso is 3.2 GiB, all held in memory; open it?
  (y or n)`, its name cut to 40 cells as ADR 0072's are. `y` runs `open-large-file`, which opens
  it for at most that size.
- **Otherwise the loader stops before reading:** it finds the size on the file it opened, reads
  nothing, and reports it. The buffer goes, the window shows what it showed before, and the echo
  area says `big.iso is 3.2 GiB; C-x C-f asks to open it`. The editor remembers the path, the
  size and the file's identity, and the next `C-x C-f` of that path asks the question at once, as
  ADR 0072's save over a changed file does.
- **A file larger than the size agreed to is refused again,** with a new message, so a file that
  grew or was replaced after the question is asked about anew.
- **`oso FILE` asks nothing,** as nothing can ask before the window: the load stops, the window
  shows `*scratch*`, and the message says `C-x C-f` asks. **The agent channel's `open` refuses**
  such a file, "2 GiB or more; ask the person to open it". A restart's handoff opens what it carries
  without asking, as the old instance held it already.
- A file that grows past the limit while open reloads with no question
  ([ADR 0083](0083-reloading-files-changed-on-disk.md)).

### Opening never blocks on a special file

- **The loader opens through `oso_platform::file_meta::open_for_reading`,** which looks at the
  path first, so a directory, a device or a pipe is not opened at all, then opens it without
  waiting on the file and without taking a terminal as the controlling one (`O_NONBLOCK` and
  `O_NOCTTY` on Unix), checks from the open file that it is a regular file, clears the flag, and
  gives the file with its metadata. Anything else is refused as not a file. So a path swapped for
  a pipe between the look and the open cannot hold the loading thread. Opening a device can still
  do something of its own, which `O_NONBLOCK` does not stop: the look before the open is what keeps
  an ordinary path off devices. A file under a lease on Linux makes the open
  fail with a message, rather than wait. The emergency copies' readers open the same way.
- `oso-platform` already depends on `libc`, and `file_meta` already opens files with what `std`
  cannot do (`open_for_writing`, ADR 0067). No dependency or crate edge is added, and the `unsafe`
  calls stay in `oso-platform`, each with its `// SAFETY:` comment. Other systems open as `std`
  does, and are decided with their tier.

### Japanese file names

- **The input method is off when the prompt opens,** as in every prompt (ADR 0072). **`かな`
  turns it on for that prompt** until it closes, so a Japanese name can be typed, if a test at
  the keyboard shows that `かな` reaches oso while the input method is off. If it does not, the
  implementation says so in its pull request, and the author decides again. Romaji matching waits
  for M3's migemo.

### `C-x C-w`

- **`write-file` on `C-x C-w`** reads a path in the same prompt, labelled `Write file:`, with the
  same completion, history and initial input.
- **A directory** takes the buffer's file name in it, as Emacs does, or the buffer's name for a
  buffer with no file.
- **An existing file asks** `notes.md exists; write over it? (y or n)`. The question comes at once
  when the listing knows the file; otherwise the save finds it, writes nothing, and says
  `notes.md exists; C-x C-w asks to write over it`, and the next `C-x C-w` of that path asks.
  `y` runs `write-file-over`, which writes only if the file there is still the one asked about
  (ADR 0059's baseline), and asks again otherwise.
- **The same path as the buffer's file** is a `C-x C-s`, with ADR 0059's and ADR 0072's rules.
- **It refuses as `save-buffer` does:** read-only buffers (binary, invalid UTF-8, loading) and a
  running save. An unmodified buffer is still written, as the new file does not have its text.
  An internal buffer is never written, so a prompt's text, a passphrase's among them, never
  reaches a file.
- **A file another buffer has open is refused,** `That file is open in another buffer`: by its
  path at once, and on the save thread by where it leads and by device and inode, so another
  spelling, a link or a second name of it counts. One file is never in two buffers.
- **`y` writes over the file only where the path led when it was found:** a path that leads
  elsewhere since writes nothing.
- **A new file is put in place without replacing anything:** under a second name, which fails if
  something has come there since the look, and then without its own. A file system with no
  second names gets a rename, with the moment between the look and the rename a save has.
- **The write is ADR 0059's,** with the new path resolved on the save thread, links followed, the
  new place's baseline checked as for a file that did not exist or as the question saw it, and no
  directory created. The mode of a new file is what the umask gives, as ADR 0059 says.
- **After the write,** the buffer visits the new path: its name (with ADR 0088's distinct names),
  its path, its baseline, the file information and the watch of ADR 0083 change, and it is
  unmodified. Its emergency entry gets a new identity (ADR 0062). The old file is untouched. A
  failed write changes none of these.
- **A buffer that could not tell where its file was** (no baseline, ADR 0059) can be written under
  a new name, which is how it gets saved.

### Secrets

- **A path's names are not buffer text.** A same-user process can list any directory the user
  can, so rows, annotations and link targets show what `ls -l` would, and a listing of `~/.ssh`
  shows names and sizes only. Nothing in a listed file is read.
- **While a buffer that may hold secrets is shown,** the prompt keeps no history (ADR 0072), and
  `get-state` leaves out its completion, label and answer (ADR 0076).
- **Writing under a new name keeps a verdict that withholds.** The new path is judged as a load
  judges it (ADR 0044). The buffer takes that verdict, unless its old verdict withholds, which then
  stays, as a tainting yank's does (`Verdict::Yanked`, ADR 0051): a secret buffer stays secret
  wherever it is written, until the file is opened again, and an ordinary one written to `.env`
  becomes secret. A secret buffer's text can be written to any path the person types; this is the
  person's choice, from the keyboard only (below).

### Reach and the sockets

- **`find-file`, `write-file`, `open-large-file` and `write-file-over` are `Person`**
  (ADR 0085). The channel opens files only through `open`, with its rules, and has no way to write
  under a new name. ADR 0085's question for "saving under a new name" from the channel has
  nothing to ask about.
- **`get-state`** gives `find-file`'s prompt the kind `complete` and ADR 0076's `completion`, whose
  `rows` are the rows' text as shown, with `directory`, the directory listed (as typed, home as
  `~`), and `pending`, true while the worker has not answered. All are withheld while a buffer that
  may hold secrets is shown.
- **`run-command`** on the dev socket gives a path as its `arg`, made absolute from the selected
  buffer's directory, and refuses `find-file` and `write-file` without one, as ADR 0072 does for a
  number. `send-keys` reaches the prompt as keys do, under ADR 0059's dev trust.

### Where it lives

- **`oso-core`:** `ArgSpec::File { prompt, history }`; the path's segments, the starting over, the
  `~` and the hidden-name rule as pure functions in a new private module `complete/path.rs`; a
  directory's candidate is its name with a trailing `/`; `Editor`'s remembered large file and existing file, in `editor/files.rs`; the
  verdict rule of a write in `secret`.
- **`oso`:** `find-file`, `write-file`, `open-large-file` and `write-file-over` in
  `commands/files.rs`, their keys in `bindings`; the listing workers in a new `lister` module, one
  layer above `loader` in `guardrails/layers.txt`, which uses no other module of the crate; what an
  answer starts in `dispatch/prompts.rs`; the same-file check and the size check in `loader`; the
  write to a new path in `saver`.
- **`oso-platform`:** `file_meta::open_for_reading(&Path) -> io::Result<Option<(File, Metadata)>>`,
  `None` for what is no regular file, the one new public function.
- No new crate, crate edge, dependency or public trait. The new module of the tip crate and its line
  in `layers.txt` are named in the pull request, as ADR 0038 asks.

### Tests and measures

- `complete/path.rs`: property tests that segments, prefixes and the starting over give what a
  plain walk gives, and unit tests for `~`, `~user`, `$VAR`, `//`, `~/` mid-path, hidden names and
  the bounds.
- The lister: a directory of 100,000 names, one that cannot be read, a link to nothing, a FIFO
  listed (never opened), and a generation that arrives late.
- The loader: a FIFO, a device and a directory are refused without blocking, also when swapped in
  after a look; a file of 2 GiB or more stops before reading (a sparse file); the same file through
  a link shows the open buffer.
- `write-file`: a new file, an existing one asked about and changed meanwhile, a directory, a
  secret buffer written to an ordinary name and the other way, and the emergency identity.
- Text-grid snapshots of the prompt: a directory, an expanded segment, the dim part before `~/`,
  annotations, a link, and `…` while pending.
- A flow (`just e2e`): `C-x C-f`, typed segments, `TAB`, `RET` opens the file, `get-state` on the
  way; the same file through a link; `C-x C-w` to a new name and the question over an existing one.
- `just perf` keeps `prompt_type_p99_ms` for keys typed into `C-x C-f`, reported only.

## Questions

Put to the author through the coordinator, the recommended option first. The author chose (a) on
each.

1. **Path matching:** (a) partial completion: earlier segments are smart-case prefixes, a segment
   that matches several directories expands into each, bounded, and the last segment uses ADR
   0076's matcher; (b) the last segment only, earlier ones exact; (c) each segment one orderless
   component.
2. **`~`, `//` and `$VAR`:** (a) `~/` or `/` after a `/` starts over, the text before dim and
   ignored, `$VAR` literal, `~user` not expanded; (b) the same and `$VAR` expanded; (c) `~` only at
   the start, nothing starts over.
3. **Hidden names:** (a) only when the last segment starts with `.`; (b) always; (c) always, after
   the others.
4. **Directories:** (a) mixed in ADR 0076's order; (b) first.
5. **Annotation:** (a) mode, size and age, the owner when not the user; (b) size and age; (c) none.
6. **Links:** (a) `→ target`, a link to a directory entered, a link to nothing listed as missing;
   (b) plain entries.
7. **`RET` and new files:** (a) `RET` enters a directory, opens a file, and with no match opens the
   typed path; `M-RET` gives the typed text; `C-p` from the first candidate selects the typed text;
   (b) the same without selecting the typed text; (c) `RET` on a directory refuses.
8. **A file already open:** (a) its buffer, by the path at once or by device and inode once loaded;
   (b) by the path only.
9. **2 GiB or more:** (a) ask at once when the size is known, else the loader stops and the next
   `C-x C-f` asks; `oso FILE` and the channel's `open` refuse; (b) only the loader's stop; (c) as
   (a), but `oso FILE` opens without asking.
10. **Japanese names:** the input method (a) off, and `かな` turns it on for the prompt; (b) as
    outside the prompt; (c) off, romaji waits for migemo. Normalization (a) none in M2, recorded as
    open; (b) `unicode-normalization`, a dependency.
11. **The verdict after `C-x C-w`:** (a) the worse of the old and the new; (b) the new path's.
12. **Reach:** (a) `find-file`, `write-file` and their confirmations `Person`; (b) `write-file` as
    `Change` with an `Agent:` question each time.

## Changes to accepted ADRs

Each passage below stays in its ADR as written; what holds now follows it. When this ADR is
accepted, the status of ADR 0030, ADR 0040, ADR 0051, ADR 0059 and ADR 0085 gains `amended by 0087`.

- [ADR 0030](0030-m2-editing-commands.md), "`~/P/S/o` expands segment by segment, with fuzzy
  matching inside each segment": the segments before the last match as smart-case prefixes, and
  only the last uses the orderless matcher of ADR 0076, with its affixes, `~` for flex among them.
- [ADR 0040](0040-buffers-loading-motion-and-scrolling.md), "Only regular files are opened,
  checked before opening, since opening a pipe blocks": they are still looked at first, and then
  opened without waiting and checked again from the open file.
- ADR 0040, "a missing file opens an empty buffer": also from `C-x C-f`, which says `(New file)`.
- [ADR 0051](0051-m2-plan-decisions.md), "Files of 2 GiB or more ask before opening, in the
  minibuffer": `C-x C-f` asks, at once or at its next try; `oso FILE` and the agent channel's
  `open` do not open them, and say so; a restart's handoff opens them without asking.
- [ADR 0059](0059-safe-save-and-the-exit-guard.md), "**No file** says `Buffer has no file name`.
  `C-x C-w` comes in M2-18": `C-x C-w` writes such a buffer, and one whose load could not tell
  where its file was, under a new name.
- ADR 0059, "**Rename, then fsync the directory.**" (step 7 of the order of a write): where nothing
  was at the file's place, for `C-x C-w` and for any save of a file that did not exist or has gone,
  the temporary file gets the file's name as a second name, which fails if something has come
  there since step 6, and then loses its own; the directory is then synced. A file system with no
  second names, or a list that denies deleting the file, keeps the rename. A file that is there
  is replaced by the rename as before.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md), "This covers a buffer the channel opened and,
  once it exists, saving under a new name": `write-file` is `Person`, so the channel does not save
  under a new name.

## Consequences

- Files open from inside oso, by path, with completion that never waits on the disk.
- A slow or hung file system stalls a listing's thread, at most 4 of them, never the UI.
- A file of 2 GiB or more costs one question, or one message and a second `C-x C-f`; it never
  aborts the app unasked.
- One file is never in two buffers, so two buffers cannot save over each other.
- `C-x C-w` gives a buffer with no file, or one whose place could not be told, a way to be saved.
- A name stored decomposed does not match what an input method types until normalization is
  decided.
- An agent can open files only through the channel's `open`, and cannot write under a new name.
- `oso-platform`'s public API gains `file_meta::open_for_reading`; `oso-core`'s gains
  `ArgSpec::File` and `Editor`'s methods for the remembered large and existing files and for the
  rows' annotations.

## Alternatives considered

- **Each segment as an orderless component:** fuzzier, but short segments match most directories,
  and each would need a listing.
- **Exact segments before the last:** simplest, but `~/P/S/o` would not work, which ADR 0030 asks
  for.
- **Listing on the UI thread:** fast on a local disk, but a network mount or a hung disk would
  freeze the window.
- **One worker thread for every listing:** fewer threads, but one hung directory would stall every
  later one.
- **Hidden names always listed:** as Emacs, but `.git` and `.DS_Store` would sort near the top of
  every directory, as short names come first.
- **Expanding `$VAR`:** as Emacs's `substitute-in-file-name`, but a `$` in a real name would then
  need escaping, and the environment would shape paths unseen.
- **Asking about the size only after the loader stops:** one path, but always two steps.
- **A command that waits for a `stat` before it asks:** commands never wait (ADR 0072).
- **Opening a large file from `oso FILE` without asking:** the person named it, but other buffers
  from a restart could still be lost.
- **`libc` as a direct dependency of the tip crate:** the loader could open without waiting
  itself, but platform code belongs in `oso-platform` (ADR 0003), which has the crate already.
- **The new path's verdict alone after `C-x C-w`:** simpler, but a secret's text written under an
  ordinary name would reach dev tooling and the emergency save.
- **`write-file` from the channel, asking each time:** useful to an agent, but a path an agent
  types, outside the files the person opened, is what ADR 0085 keeps it from writing.
- **`unicode-normalization` now:** Japanese names stored decomposed would match, at the cost of a
  dependency before such names are seen to matter.
