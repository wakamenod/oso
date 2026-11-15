# 0134. A directory editor

- Status: Accepted
- Date: 2026-11-15

## Context

The roadmap puts a directory editor, like Emacs's dired, after M4, with "only the commands the
author actually uses", and its UI decided before it is built
([`roadmap.md`](../roadmap.md#after-m4)). `open-questions.md` lists its command subset as open.
The author said on 2026-11-15 that the first step only needs what they use: later steps are
listed here, not designed in depth.

What oso has today:

- `C-x C-f` lists a directory's names on worker threads (`lister.rs`, [ADR
  0087](0087-finding-files-writing-under-a-new-name-and-large-files.md)), and `RET` on a
  directory enters it "as there is no directory editor yet".
- `oso_platform::file_meta::Directory` opens a directory and what it holds from its parent by
  name, following no link, as `C-x p f`'s walk does ([ADR
  0114](0114-projects-trust-finding-files-and-searching-a-project.md)).
- Generated, read-only buffers with a mode and a keymap of their own: `*Sessions*` ([ADR
  0131](0131-an-orca-style-sessions-pane.md)), `*Worktrees*` and review buffers ([ADR
  0122](0122-agent-review.md)), filled through `Editor::set_internal_text` and
  `set_internal_highlights`.
- The watcher `stat`s open files once a second and on focus ([ADR
  0083](0083-reloading-files-changed-on-disk.md)).
- `*Sessions*` holds the frame's left side ([ADR 0121](0121-agent-sessions-and-spaces.md), ADR
  0131), which a tree sidebar would compete for.

What the author's previous setup did with dired, as facts to start from:

- Listing switches `-aBhl --group-directories-first`: every file with dotfiles, no backups
  ending in `~`, human sizes, the long format, directories first.
- Copy and rename default to the directory shown in the next window (`dired-dwim-target`),
  unless a prefix argument is given.
- `dired-jump` (`C-x C-j`), and one listing buffer reused while moving between directories
  (`dired-single`).
- Icons per file type (`nerd-icons`).
- A tree sidebar on `F9` (neotree): the project's root, hidden files shown, refreshed on its own.
- A package that runs a shell command on the marked files, in place of dired's `!` and `&`.
- `M-s` left unbound in dired, as it is the global search prefix.

### What others do

- **Emacs dired** runs `ls` with `dired-listing-switches` (or `ls-lisp` in Lisp) and parses its
  output. Keys: `RET`/`f` visit, `^` up, `n`/`p`, `g` refresh, `m`/`u`/`U` marks, `t` toggle
  marks, `d` flag and `x` delete flagged, `D` delete, `C` copy, `R` rename or move, `+` make a
  directory, `w` copy names, `s` sort by name or date, `(` hide details, `!`/`&` shell commands,
  `% m` mark by regexp. `C-x C-j` is in `dired.el` since Emacs 28, and so is
  `dired-kill-when-opening-new-dired-buffer`, the built-in form of `dired-single`.
  `delete-by-moving-to-trash` is off by default. `dired-recursive-deletes` and
  `dired-recursive-copies` default to `top`: ask once per directory given. An existing target is
  asked about file by file. A rename across file systems copies, then deletes. Dired refreshes on
  `g`, or through file notifications on the directory with `auto-revert-mode`.
- **wdired** (`C-x C-q`) makes the names editable text; `C-c C-c` renames every changed name.
- **dirvish** keeps dired's model and adds a preview pane, attribute columns (icons, git state,
  sizes), a side window (`dirvish-side`), and, past a threshold of entries, lists a directory
  with `fd` asynchronously and fills the buffer as output comes.
- **VS Code's explorer** lists a folder when it is first expanded, watches the workspace in a
  separate process, and deletes to the trash with a confirmation (`files.enableTrash`,
  `explorer.confirmDelete`); a permanent delete warns with no opt-out, and a failed trash offers
  one.

oso differs from dired where its rules require it: listing executes nothing, so there is no
`ls`; nothing touches the file system on the UI thread; a process starts only through
`oso-platform::process` (ADR 0010).

## Decision

### The listing buffer

- **A listing is a generated, read-only buffer in a new mode, `Directory`,** with its own keymap,
  as `Review` is. It is named as the directory would be by `C-x b`, with a trailing `/`
  (`crates/`), made unique as file buffers are.
- **One listing per window's travel (Q2):** `RET` on a directory and `^` replace the listing in
  place: the same buffer shows the new directory and is renamed. `C-x C-j` and `C-x d` show a
  listing that already shows that directory, else make one. So two windows can show two
  directories, which copy and rename need.
- **Rows are the host's, not parsed back.** The buffer keeps a table from line to entry, by the
  entry's name as bytes. Commands find a row's file from the table, never from the text, so a
  crafted name cannot pass for another row or another directory.
- **The first line** is the directory's path, with the home directory as `~`, ending in `:`, as
  dired's header. The modeline names the sort and the count of marked rows.
- **`.` and `..` come first,** as `ls -a` gives them; `RET` on `..` is `^`.
- **Order:** directories first (with links to directories), then the rest. By name, ignoring
  ASCII case, then by bytes; or by modification time, newest first. `s` toggles the two.
- **Backups ending in `~` are not listed,** as `-B`. Every other dotfile is.

### Rows (Q4, Q5)

A mock at 80 columns, with details shown:

```
  ~/Projects/oso/crates/:
  drwxr-xr-x          -  Nov 15 09:12  .
  drwxr-xr-x          -  Nov 14 21:40  ..
  drwxr-xr-x          -  Nov 15 09:12  oso
* drwxr-xr-x          -  Nov 13 18:02  oso-core
  -rw-r--r--       4.0K  Nov 12 11:30  AGENTS.md
  lrwxr-xr-x         12  Nov  2  2025  latest -> oso-core
  -rw-r--r--  root 1.2M  Oct  1 08:00  dump.bin
```

- **Columns (Q4):** the mark, the mode as `ls -l` writes it, the owner only when it is not the
  person (and the group only when the owner is shown), the size as `ls -h` does (`4.0K`, `1.2M`;
  `-` for a directory), the modification time as `ls` does (time within six months, else the
  year), the name, and `-> target` for a link as `read_link` gives it.
- **`(` hides the details,** leaving the mark and the name, for this listing and those it moves
  to.
- **Names are drawn as any buffer's text:** invalid bytes as `\xNN`, control characters,
  bidirectional and invisible characters shown ([ADR
  0066](0066-text-stays-in-logical-order.md), [`ui.md`](../ui.md#bidirectional-and-invisible-characters)).
  A newline in a name shows as `\n` and never starts a row.
- **Lines do not wrap** in a listing; a long row is cut at the window's edge and scrolls
  sideways, as any unwrapped buffer.
- **Faces (Q5):** `dired.header`, `dired.directory`, `dired.symlink`, `dired.broken-symlink`,
  `dired.executable`, `dired.special` (pipes, sockets, devices), `dired.details` (dim, for the
  mode, owner, size and time), `dired.mark` and `dired.marked`, in the theme table with their
  contrast rules ([ADR 0089](0089-built-in-themes-switched-at-run-time.md)). The face is chosen
  from the entry's kind and mode, never from what the file holds.
- **No icons in step 1 (Q5).** A file-type icon font (Nerd Font) needs a font setting oso does
  not have yet ([ADR 0108](0108-the-user-configuration-file.md) reserves `fonts`), as CoreText's
  cascade list does not cover private-use glyphs. Icons are a later step; they will be chosen
  from the name and kind only.

### Listing on a worker

- **A listing is read on the lister's threads** (`lister.rs`, at most 4, one request per
  directory, the newest request replacing one that waits), as a new request kind beside
  `C-x C-f`'s names. A directory on a file system that hangs stalls only its own thread.
- **The worker opens the directory** with `Directory::open` (a link named by the person is
  followed there) and reads each entry's metadata from the descriptor, following no link: on
  macOS with `getattrlistbulk`, which returns names and attributes in batches and is faster than
  `readdir` and an `lstat` each; on Linux with `getdents64` and an `fstatat` each (`d_type` can be
  unknown on some file systems); on Windows from `FindFirstFileExW`, which carries them. A link's target is read (at most 64 KiB) and looked at
  once, following it, only to say whether it leads to a directory or to nothing.
- **The worker builds the text,** the line table and the highlights as a rope with its display
  map (`oso_core::load::Builder`), as the review's worker does, so the UI thread does no O(n)
  work. It hands over snapshots in the loader's rhythm: the first after the first screen of
  rows, then at most one per 50 ms, then the complete one.
- **Sorting by name** needs only the names, so rows come in order with their details filling in.
  Sorting by time shows `sorting…` in the header until every entry is looked at.
- **At most 100,000 entries** (the lister's `MAX_NAMES`); past that the header says the listing
  is cut, and `g` does not lift it.
- **Nothing listed is opened or read.** No icon, face or column comes from a file's bytes. On
  macOS a file a sync service keeps elsewhere is only `lstat`ed, which does not fetch it.

### Moving, opening and refreshing

- **`RET` on a file** opens it in the window, through the loader's verdict, as `C-x C-f` does; a
  file that may hold secrets opens as such a buffer. `o` opens it in another window.
- **`C-x C-j`** (`dired-jump`) shows the listing of the buffer's file's directory with point on
  the file. In a listing it goes up, as `^`. In a buffer with no file it lists the buffer's
  directory (a terminal's starting directory, `*scratch*`'s current directory).
- **`C-x d`** (`dired`) reads a directory with the file-name prompt (ADR 0087), its default the
  buffer's directory. `C-x C-f`'s `RET` on a directory keeps entering it (ADR 0087 unchanged).
- **Refresh (Q6):** while a listing is shown, the watcher also `stat`s its directory once a second
  and on focus; a change of the directory's modification time or identity lists it again.
  `g` lists it again at once, and so does every operation oso makes in it. A new listing keeps
  point on the same name (or the row where it was) and keeps marks on names that remain.
- **`q`** puts the window back to the buffer it showed before, leaving the listing.

### Marks and operations

- **`m` marks** the row at point and moves down, `u` unmarks and moves down, `DEL` unmarks and
  moves up, `U` unmarks every row, `t` toggles every mark. A mark is a `*` in the first column,
  changed as one character of the buffer's text (O(log n)), never a rebuild.
- **What an operation acts on:** the marked rows; with none, the rows of the window's cursors
  (multiple cursors act on each one's row), or the rows an active region covers; a numeric
  prefix `N` takes the next `N` rows, as dired's. `.` and `..` are never acted on.
- **Every operation runs on a thread of its own, `oso-files`,** one at a time, in order. The echo
  area reports progress at most every 100 ms past 500 ms (`Copying 120/3000…`). `C-g`, when
  nothing else is pending (ADR 0072's order), stops it after the file in hand; what was done
  stays done, and the echo area says how far it got.
- **Every operation acts from the directory as listed:** the worker opens it again by path and
  checks, by device and inode, that it is still the directory listed, or it refuses with "this
  directory changed: `g`". Each source is then reached from that descriptor by its name, following
  no link, so a name swapped for a link between listing and acting cannot redirect it. A target
  the person typed is taken as typed (a link they named is followed).
- **`C` copies,** `R` renames or moves, and both read the target with the file-name prompt:
  - **The default target (Q7)** is the directory of the next window, in `C-x o` order, that shows
    a listing; else this listing's directory. With `C-u` it is this listing's directory.
  - **Copies keep** the mode, the modification time and the extended attributes (so a download's
    quarantine flag travels, as Finder's copy keeps it), through clones where the file system has
    them (`clonefile` on APFS, `copy_file_range` on Linux). A link is copied as a link. A
    directory is copied whole, at most 256 deep; pipes, sockets and devices are skipped and
    named.
  - **A rename** is one `renameat` on the same file system. Across file systems it copies, syncs,
    then deletes the source.
  - **A buffer whose file is renamed** follows it: its path becomes the new one, with unsaved
    edits kept, and its verdict is judged again from the new name, never becoming less secret
    until it is opened again, as a reload never does ([`security.md`](../security.md)).
  - **Targets that exist (Q8)** are found before anything is written: one question names how many
    and the first three (`3 exist (a.txt, b.txt, c.rs): overwrite them? (y or n)`); `n` does
    nothing. A directory is never written over a file or a file over a directory.
- **`D` deletes permanently (Q3),** as Emacs's default, after one question that names the count
  and the first three rows, `Delete 3 files (a.txt, b.txt, notes/)? (y or n)`. A directory among
  them is deleted with what it holds, and the question says so (`notes/ and what it holds`); it
  asks once, not again for each directory.
  - A directory's contents are removed from its descriptor, a name at a time with `unlinkat`,
    following no link (a link is removed, never what it leads to), at most 256 deep. An entry
    that cannot be removed stops the operation, and the echo area names it and how far it got.
  - There is no trash: no system trash call, no process.
  - A buffer whose file was deleted stays, and the watcher reports the file gone, as now.
- **`+`** makes a directory, with its parents, read with the prompt from this listing's
  directory.
- **`w`** copies the names of the rows acted on, separated by spaces, to the kill ring and the
  clipboard; `0 w` copies their absolute paths. These are names, not file text, and copy as any
  kill does.
- **Questions follow ADR 0072:** the command leaves a question whose `y` runs a command of its
  own, which checks again (the directory's identity, the rows) before the worker starts.

### Keys

| Key | Command | Note |
|---|---|---|
| `RET`, `f` | `dired-find-file` | a file opens, a directory replaces the listing |
| `o` | `dired-find-file-other-window` | |
| `^` | `dired-up-directory` | point on the directory left |
| `n` / `p` | `dired-next-line` / `dired-previous-line` | to the name's column |
| `g` | `revert-buffer` | lists again |
| `q` | `quit-window` | |
| `m` / `u` / `DEL` | `dired-mark` / `dired-unmark` / `dired-unmark-backward` | |
| `U` / `t` | `dired-unmark-all-marks` / `dired-toggle-marks` | |
| `C` / `R` | `dired-do-copy` / `dired-do-rename` | |
| `D` | `dired-do-delete` | permanently, after one question |
| `+` | `dired-create-directory` | |
| `w` | `dired-copy-filename-as-kill` | `0 w`: absolute paths |
| `(` | `dired-hide-details-mode` | |
| `s` | `dired-sort-toggle` | name, newest first |
| `C-x C-j` (global) | `dired-jump` | |
| `C-x d` (global) | `dired` | |

- Other keys are global ones (`C-n`, `C-s`, `C-x o`, `M-x`). Nothing in the mode binds under
  `M-s`, so the global search prefix works in a listing.
- The mode keymap is a table in `crates/oso/src/bindings/dired.rs`. `[keys.dired]` in
  `config.toml` rebinds them once per-mode key tables exist, as `[keys.session]` does ([ADR
  0132](0132-a-session-mode-for-hosted-sessions.md)).

### Projects, trust and processes

- Listing a directory, in a trusted project or not, runs nothing and needs no trust: it reads
  names and metadata only. Opening a row goes through the loader as any file.
- No operation of step 1 starts a process. Shell commands on marked files are a later step, for
  the person only, through `oso-platform::process`.

### Agents and secrets (Q9)

- **Reach (ADR 0085):** moving, marking, sorting, hiding details, refreshing, `w` and opening a
  listing are `Look`. `C`, `R`, `D` and `+`, and every later operation that writes the file system
  or starts a process (wdired, shell commands), are `Person`: they change files the agent did not
  write, past what a save covers, and an agent has its own tools for files.
- **A listing's text** is readable by the agent channel as any buffer's, as it holds names and
  metadata only, which the agent could list itself. A listing of a directory whose path matches
  the secret patterns (inside `.ssh`, `.gnupg` or `.aws`) gets the verdict `SecretName` and is
  withheld from the channel and the dev socket, as such a file's buffer is. The agent cannot
  bring a listing in front of the person, as for any mode with keys of its own.

### Restart (Q10)

- **A restart carries each listing** ([ADR 0090](0090-oso-dev-carries-unsaved-text.md), ADR
  0133): its directory's path, the sort, whether details are hidden, the name at point and each
  window's place. The new instance lists the directory again on its worker. Marks are not
  carried. An operation still running holds the restart, and the refusal says so.

### Platforms

- **macOS (Tier 1)** is built and checked first.
- **Linux:** the same Unix code (`getdents64` through `std`, `fstatat`).
- **Windows:** `Directory` is a path there; the listing's attributes come with the names from
  `FindFirstFileExW` (`FindExInfoBasic`, `FIND_FIRST_EX_LARGE_FETCH`), so no `lstat` is needed. The mode column shows PowerShell's `Mode`
  (`d----`, `-a---`, `l----`), and no owner. Built and linted with `just clippy-windows`.

### Code, crates and layers

- **Tip crate:** `crates/oso/src/dired/` for the listing request and rows (`rows.rs`), the order,
  the operations worker (`ops.rs`); `commands/dired.rs`, `bindings/dired.rs`,
  `app/dired.rs` for the glue. Each file stays under 600 lines.
- **`oso-core`:** `Mode::Directory`; a way to swap an internal buffer's whole text for a rope and
  display map built off the UI thread (in O(1), mapping the windows' cursors by line), and to
  replace a range of it keeping the cursors (for marks). No new module or trait.
- **`oso-platform`:** `file_meta::Directory` gains an entry's metadata, rename, unlink, and a copy
  from an opened file. No new module, trait, crate or crate edge.
- **Dependencies:** none. As `D` deletes permanently (Q3), no trash call is needed, so
  `objc2-foundation`'s features and `Cargo.toml` stay as they are.

### Cost

| What | Budget |
|---|---|
| A listing of 1,000 local entries | complete ≤ 20 ms on the worker; shown in the next frame |
| A listing of 100,000 entries | first screen ≤ 50 ms; complete ≤ 1 s on the reference Mac |
| UI thread per snapshot | ≤ 1 ms whatever its size: a swap, no pass over the text |
| A mark, `n`, `p` | within the key-to-photon budget; one character changed |
| Watcher | one `stat` per shown listing per second |
| Memory | ≤ 160 bytes per entry beyond its name and row text |

The `just perf` flows gain one: a listing of 100,000 entries read and refreshed while typing in
another window, whose key-to-photon p99 stays within budget.

## Steps

One pull request each.

1. **The listing (step 1a):** the mode, the worker's listing, rows, faces, sort, details, the
   header, `RET`, `o`, `^`, `n`, `p`, `g`, `q`, `w`, `C-x C-j`, `C-x d`, the watcher's refresh,
   the restart carry, the `just perf` flow.
2. **Operations (step 1b):** marks, `oso-files`, `C`, `R`, `D`, `+`, the dwim target, the questions, buffers following a rename.

Later steps, listed and not designed here; each is proposed when it is next:

- `[keys.dired]` in `config.toml`.
- wdired (`C-x C-q`): names as editable text, committed with `C-c C-c` as one batch of renames.
- Shell commands on the marked files (`!`, `&`), as the previous setup's package did: a command
  line with the files put in, for the person only, through `oso-platform::process`, its output in
  a buffer or a terminal.
- A tree sidebar on `F9`, as neotree: the project's root, hidden files, refreshed on its own; where
  it goes beside `*Sessions*` is decided then.
- File-type icons once oso has a font setting.
- More marks (`% m`, `* /`), `d` and `x`, `M` (mode), `S` (link), a preview as dirvish's.
- Deleting to the system's trash, if the author wants it later.

## Questions

For the author, the recommended option first. Each stands on its own.

1. **The commands of step 1.** Keep, drop or add any.
   (a) The list in Keys: `RET`/`f`, `o`, `^`, `n`/`p`, `g`, `q`, `m`, `u`, `DEL`, `U`, `t`, `C`,
   `R`, `D`, `+`, `w`, `(`, `s`, `C-x C-j`, `C-x d`.
   (b) That list, and `d`/`x` (flag, then delete the flagged), `% m` (mark by regexp) and `C-o`
   (show a file in another window, staying in the listing).
   (c) A smaller list: `RET`, `^`, `n`/`p`, `g`, `m`/`u`, `C`, `R`, `D`, `+`, `C-x C-j`.
2. **Listing buffers.**
   (a) As the previous setup's `dired-single`: moving to another directory replaces the listing in
   place; `C-x C-j` and `C-x d` reuse a listing of that directory, else make one, so two windows
   can show two directories.
   (b) A buffer per directory, as plain dired: those moved away from stay until killed.
   (c) One listing buffer for the whole frame.
3. **Deleting.**
   (a) To the system's trash, after one question naming the count and the first files; permanently
   only with `C-u D`, or where there is no trash, after a question that says so.
   (b) Permanently, as Emacs's default, after one question, and again for each directory with
   files in it.
   (c) To the trash with no question, as the trash can bring them back.
4. **What a row shows.**
   (a) The mode, the size as `ls -h`, the time as `ls`, the name and a link's target; the owner and
   group only when the owner is not you; `(` hides all but the name.
   (b) All of `ls -l`: the mode, the link count, the owner, the group, the size, the time, the name.
   (c) The name and size only; `(` shows the rest.
5. **Icons.**
   (a) None in step 1: faces for directories, links, broken links, executables and special files;
   Nerd Font icons as a later step, once oso has a font setting.
   (b) Nerd Font icons in step 1, with a setting naming a symbols font you install.
   (c) Plain Unicode symbols that every font has (`▸` for a directory, `→` for a link), in step 1.
6. **Refreshing.**
   (a) While shown, the listing's directory is `stat`ed once a second and on focus, and listed
   again when it changed, keeping point and marks; `g` and oso's own operations refresh at once.
   (b) The system's file notifications (FSEvents, inotify, `ReadDirectoryChangesW`): new platform
   code, quicker, and more to get right on each system.
   (c) Only on `g` and after oso's own operations, as dired.
7. **The default target of `C` and `R`.**
   (a) The directory of the next window, in `C-x o` order, that shows a listing, else this
   listing's; with `C-u`, this listing's.
   (b) Always this listing's directory, as Emacs's default; the other listing reachable with
   `M-n` in the prompt.
   (c) As (a), with no `C-u` exception.
8. **When a target exists.**
   (a) One question before anything is written, naming how many exist and the first three; `y`
   overwrites them all, `n` does nothing.
   (b) One question for each, `y`, `n`, `!` (all) or `q`, as dired's: a new kind of question.
   (c) Never overwrite: say which exist and do nothing.
9. **Agents.**
   (a) The agent may read a listing and run `Look` commands in it; copy, rename, delete, making a
   directory and every later writing or process command are `Person`; a listing inside `.ssh`,
   `.gnupg` or `.aws` is withheld.
   (b) Listings withheld from agents entirely, as `*Sessions*`.
   (c) As (a), and copy, rename and making a directory are `Change`, asking you first with the
   `Agent:` question, as a save of a buffer the agent opened does.
10. **Over a restart.**
    (a) Each listing comes back: its directory, sort, details and the name at point, listed again;
    marks are dropped.
    (b) As (a), with the marks, by name.
    (c) Listings do not come back, as internal buffers do not.

### Dependencies

None (after Q3 (b), no trash call and no `Cargo.toml` change).

## The author's choices

The author answered every question on 2026-11-15, through the coordinator: (a) for each, but Q3.

- **Q3: (b).** `D` deletes permanently, as Emacs's default, after one question that names the
  count and the first files, as dired does. There is no trash, so no system trash call and no
  `Cargo.toml` change (`objc2-foundation`'s `NSFileManager` feature is not needed).
- The rest, as recommended: the standard command set in Keys (Q1); one listing per window's
  travel, as `dired-single` (Q2); rows as `ls -lh`, the owner and group only when not the
  person's (Q4); no icons in step 1, faces per type (Q5); a `stat` once a second while shown and
  on focus, keeping point and marks, `g` and oso's own operations at once (Q6); the next window's
  listing as the default target, `C-u` for this one (Q7); one question for existing targets, `y`
  overwrites them all (Q8); agents read listings and run `Look` commands, `C`, `R`, `D` and `+`
  are `Person`, listings inside `.ssh`, `.gnupg` and `.aws` withheld (Q9); listings come back
  after a restart, marks dropped (Q10).

## Step 1a, as built

The listing, without marks or operations. The author approved the plan on 2026-11-15, with the
public items below and three more they chose the same day: a local time offset, group names and a
directory prompt (all (a)).

- **The buffer:** `oso_core::buffer::Mode::Directory`, an internal read-only buffer that does not
  wrap, made by `Editor::add_listing` and filled by `Editor::set_listing_text`, which swaps in the
  text the lister built (O(1), as a load) with a `Listing` record (directory, header lines, name
  column, rows, details, the modeline's note) and the highlights, renames the buffer, judges it
  and puts each window's cursor on the line the host found by name. Unlike other internal
  buffers, `C-x b`, `C-x <left>` and `C-x k` reach a listing. Its modeline says `Dired by name` or
  `Dired by date`, and no encoding. Commands ask the host through `ListingRequest`
  (`Editor::request_listing`, `take_listing_requests`); the row table (name, whether `RET`
  enters it) stays in the tip crate's `app/dired.rs`, never read back from the text.
- **Judging:** a listing is judged by its directory's words the moment it is made, so its
  placeholder (`listing…`) shows nothing of a secret directory, and again by the real place the
  lister found (`canonicalize` on the worker) with each listing. A listing inside `.ssh`,
  `.gnupg` or `.aws` is `SecretName`, withheld from the socket and the agent channel.
- **Listing:** a new request kind on `lister.rs`'s threads, kept per buffer (a newer request for
  the buffer replaces one waiting; a closing prompt no longer drops them). `crate::dired`, a new
  module on the tip crate's lowest layer, reads with `Directory::list_with_metadata` (macOS
  `getattrlistbulk` in 128 KiB batches, its reply read back from a byte slice with every field
  bounds-checked; Linux `readdir` and `fstatat`; Windows `read_dir`), `read_link` for links and
  `target_kind` once whole, sorts and builds the text with `oso_core::load::Builder` and the
  highlights in one pass. While a large directory is read it sends what it has every 50 ms with a
  `reading… N entries so far` line. Owner and group names come from `file_meta::user_name` and
  `group_name`, cached per listing; a name that is not printable ASCII shows as its id, so every
  byte before a row's name is ASCII. Dates use `file_meta::utc_offset_at` (Unix `localtime_r`;
  Windows UTC for now), asked once per hour of time. A directory that cannot be opened shows as a
  listing that says why.
- **Rows:** as the Decision says. Directories show `-` for their size. Names with a line break or
  a carriage return show `\n` and `\r`; invalid bytes go in as bytes, so the display shows them as
  `\xNN` and the highlights follow the decoded text.
- **Refresh:** the watcher (`loader::Watcher`) also `stat`s each shown listing's directory
  (`watch_directories`, `take_directories`). The lister reads the directory's stamp just before it
  lists, and a look that differs from the stamp of the listing shown lists it again, also when the
  watcher saw the change while the listing was being read.
- **Keys:** as the Decision, with `SPC` for `n`, and digits and `-` as a prefix argument, as dired
  binds them, so that `0 w` works.
- **Restart:** `oso dev`'s handoff carries each listing as a buffer entry with no path and a
  `listing` object (directory, details, order, line), and the new instance lists it again.
- **Faces:** seven `Highlight` variants and `oso_render::pane::Face`s. Names take a theme's
  terminal colours as `ls --color` does (directories blue, links cyan, broken links red, files
  someone may run green, pipes, sockets and devices yellow), since a theme with few syntax
  colours, as `dusk`, draws its function and heading colours as its text; the header takes the
  heading colour and the details the prompt's dim one. The theme's face map moved
  to a private child file, `oso-render/src/theme/faces.rs`, as the new faces took `theme.rs` past
  600 lines.
- **Perf:** a report-only flow, `bench/dired.rs`, with `dired_first_rows_ms`, `dired_list_ms` and
  `dired_relist_type_p99_ms`; no thresholds yet. `compare.rs` needed no change, as a metric with
  no threshold never fails.
- **Tests:** unit tests for rows, order, the build, reading scratch directories, the bulk parser,
  the editor's listings, the commands, the handoff; `tests/e2e/dired.rs` for moving, opening, the
  watcher, `s` and `(`, a `.ssh` listing and a restart.

### Changes from the Decision

The author agreed to each on 2026-11-15, through the coordinator.

- **`g` runs `dired-revert`, a `Look` command,** not `revert-buffer`, which is `Person` as it can
  drop unsaved edits.
- **`C-x d` (`dired`) is `Person`,** as `find-file` is: it reads its directory in a prompt
  (`ArgSpec::Directory`, a new argument spec that opens the directory prompt `C-u C-x p f` uses,
  where `RET` gives the directory). `dired-jump` and the listing's own commands are `Look`.
- **A restart carries the line at point,** not the name: the handoff sees only the editor, and the
  directory is listed again at once, so the line lands on the same name unless the directory
  changed.
- **A buffer with no file lists the directory oso started in** for `C-x C-j`; a terminal's own
  directory is a later step.
- **`quit-window` shows the buffer shown before,** the most recent other one, and keeps the listing.
  It is a buffer command (`commands/buffers.rs`), bound to `q` in a listing.
- **The listing's keys are listed in `commands::dired::KEYS`,** beside the commands, and bound by
  `bindings/dired.rs`, as a review's are.
- **Reach:** the commands whose request the host carries out after the command, where the agent
  channel's guards no longer see it, are `Person`: `dired-jump`, `dired-find-file`,
  `dired-find-file-other-window` (they open a file or show a buffer), `dired-copy-filename-as-kill`
  (it writes the clipboard) and `quit-window`. Moving, refreshing, sorting and hiding details
  stay `Look`.
- **Partial listings** show the first 500 rows in order of what has been read, under the
  `reading…` line, and come only for a listing of a directory the buffer did not show whole: a
  refresh replaces the whole listing at once. A listing no longer wanted stops reading. A
  listing with links is handed over before its links are followed, then again once they are; a
  link whose target's words name a UNC or device path, or a place mounted from the network when
  looked at (`/net`, `/Network`), or another volume (`/Volumes` on macOS), is not followed (the
  author's choice).
- **Judging:** a directory whose real place cannot be found, or whose real place is not the
  directory opened by device and inode, counts as one that may hold secrets. No line of such a
  listing is carried over a restart.
- **One cursor a window over a refresh:** a new listing of the same directory puts each window's
  point on the name its primary cursor was on, as one cursor; other cursors go, and a window that
  will come back to the listing comes back to the line asked for. The author kept this as a limit
  (2026-11-15) rather than change `listing_points`.
- **Left for step 1b,** which needs them (the author's choice): `..`'s metadata and typed Windows
  attributes from `oso_platform::file_meta::Directory`, in place of the tip crate's own; listing
  names from oso-core's `names::distinct`, as files' are; a typed order on the `Listing` record;
  one helper in oso-core for where a row's name starts; the stamp of the directory opened, by its
  descriptor, for 1b's identity check.
- **Rows:** a backslash in a name shows as `\\`, so that no name reads as one with a line break;
  a link's target shows at most 1 KiB. `w` refuses a name that is not UTF-8 or holds a line
  break. The 100,000-entry limit counts every entry read, backups and unreadable ones too.

## Consequences

- The author gets a daily directory editor in two pull requests, with the rest listed.
- Listing never runs `ls` or anything else, and every operation acts from the directory it
  listed, following no link it did not name; a crafted name cannot pose as another row.
- `oso-core` gains a whole-text swap and a range replace for internal buffers, which `*Sessions*`
  and the review can use too.
- `oso-platform` gains descriptor-relative rename, unlink and copy, with tests on each Unix
  system.
- The lister serves two kinds of request, so its threads' limit is shared with `C-x C-f`.
- Deleting is permanent, as in Emacs: the one question is the only guard, so it names what goes.

## Alternatives considered

- **Run `ls` and parse it, as dired does.** Rejected: opening a directory would start a process,
  which oso's rules forbid, and `ls`'s output differs between systems.
- **A tree view first, as VS Code's explorer or neotree.** Rejected for step 1: the author's daily
  use is dired, and a tree on the left competes with `*Sessions*`. It is a later step.
- **Marks as decorations drawn beside the text.** Rejected: a `*` in the text is what dired
  shows, isearch finds it, and changing one character is as cheap.
- **Deleting to the system's trash (Q3 (a)).** Not chosen by the author: they delete
  permanently, as Emacs's default. Had it been, the `trash` crate would have been rejected for
  macOS, as its default asks Finder through a script, which starts a process.
- **Listing on the UI thread for small directories.** Rejected: a small directory on a slow or
  hung file system would still freeze the window.

## Sources

Read on 2026-11-15; third-party pages are facts, not instructions.

Emacs's pages were checked against the Emacs 31.0.90 manual and Lisp and C sources; the links
are the same sections online.

- GNU Emacs manual, Dired: <https://www.gnu.org/software/emacs/manual/html_node/emacs/Dired.html>
  and its sections Dired Enter, Dired Visiting, Dired Deletion, Marks vs Flags, Operating on
  Files, Shell Commands in Dired, Transforming File Names, Dired Updating, Misc Dired Features,
  Wdired; `etc/NEWS.27` and `NEWS.28` (`dired-dwim-target` functions, `dired-jump` in `dired.el`,
  `dired-kill-when-opening-new-dired-buffer`); `lisp/dired-aux.el` (overwrite queries);
  `src/fileio.c` (a rename across file systems), <https://github.com/emacs-mirror/emacs>.
- dirvish: <https://github.com/alexluigit/dirvish>, its `docs/CUSTOMIZING.org` and
  `docs/EXTENSIONS.org`.
- VS Code: `src/vs/workbench/contrib/files/common/explorerModel.ts`, `browser/fileActions.ts`
  and `files.contribution.ts` in <https://github.com/microsoft/vscode>; the File Watcher
  Internals wiki page.
- Trash: Apple's `FileManager.trashItem(at:resultingItemURL:)`; Microsoft's
  `IFileOperation::SetOperationFlags` and `SHFILEOPSTRUCTW`; the freedesktop.org Trash
  specification, <https://specifications.freedesktop.org/trash/latest>; the `trash` crate's
  source, <https://github.com/Byron/trash-rs> (its macOS default runs `osascript`).
- Listing cost: `getattrlistbulk(2)`; <https://mjtsai.com/blog/2019/04/22/performance-considerations-when-reading-directories-on-macos>;
  `getdents(2)` and `statx(2)` on man7.org; `FindFirstFileExW` and `WIN32_FIND_DATAW` on
  Microsoft Learn; `std::fs::DirEntry`'s documentation.
