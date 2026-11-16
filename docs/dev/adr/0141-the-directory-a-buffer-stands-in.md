# 0141. The directory a buffer stands in

- Status: Proposed
- Date: 2026-11-16

## Context

`C-x C-f` starts at the buffer's file's directory, a listing's own directory
([ADR 0134](0134-a-directory-editor.md)), or else where oso started
([ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md)). A terminal, a hosted
session's `*claude: NAME*`, `*scratch*`, `*git: NAME*`, a review and the side pane all start where
oso started, wherever they are about. Its first row is selected, so `RET` goes into that row, never
into the directory shown.

The author: "When I press `C-x C-f RET`, open the folder that belongs to the buffer. Run inside a
Session: that directory's root. Run in a source file: the folder the file is in. Behave like the
Emacs I used." In Emacs, each buffer has a `default-directory`. A new buffer takes the current
one's, and every prompt for a path starts there. Vertico's default `vertico-preselect` is
`directory`, so the prompt itself is selected while it is a directory, and `RET` then visits the
directory, which dired lists.

The author answered all five questions with (a) on 2026-11-16, through the coordinator.

## Decision

### One directory for each buffer

- **`Editor::default_directory(buffer)`** in oso-core is the directory a buffer stands in. Every
  prompt for a path starts there.
- **What it is:**

  | Buffer | Directory |
  |---|---|
  | A file | its directory, read again each time, so that a write under a new name or a move is followed |
  | A listing (dired) | the directory it lists |
  | `*git: NAME*`, a review | the project's root (the worktree's own) |
  | `*commit: NAME*`, `*git log: NAME*` | the project's root, given by the host when it makes them |
  | A hosted session's terminal | the session's directory, where it runs; its program's OSC 7 does not move it |
  | A plain terminal | where it started, then the local directory its program last sent (OSC 7, OSC 9;9, OSC 1337 `CurrentDir`) |
  | The side pane | the place of the row at point, as `claude` and `c` there take it ([ADR 0131](0131-an-orca-style-sessions-pane.md)); on a row with no place, the space in front's directory |
  | `*scratch*`, `*Messages*` | the directory oso started in |
  | Any other (`C-x b` to a new name, `*Help*`, `*Session explain*`…) | the directory of the buffer shown when it was made, in the window commands act on, beneath any prompt |

- **A buffer keeps the directory it was made in,** as Emacs's do. A buffer made before any other,
  or beside one that stands where oso started, follows the start directory.
- **The host gives a buffer its own directory** with `Editor::set_buffer_directory`: a terminal's
  start and its program's directory, a session's, the commit and log buffers' root, and the side
  pane's row. A file, a listing, a git status and a review are worked out from their records and
  take no directory of their own.
- **A rename or a move** of a directory (dired's `R`, ADR 0134) moves each buffer that stands in
  it, or under it, to the new place.
- Reading it touches no file system: it is a record's field or a path's parent.

### What uses it

- **Every prompt for a path:** `find-file`, `write-file`, `dired`, `dired-create-directory`,
  `session-explain-file`, the `C-u` directory prompts of `C-x p f`, `C-x p g` and `claude`, and `C`
  and `R` outside a listing.
- **`dired-jump`** in a buffer with no file lists that buffer's directory, as ADR 0134 already
  says ("a terminal's starting directory, `*scratch*`'s current directory").
- **A new terminal and a new session (Q2):** the selected buffer's project root, as before, else
  the directory it stands in. From a plain terminal that is the one the terminal started in, never
  what its program sent (Q3), and the home directory for one carried over a restart, which keeps
  no start. Then, as before, the home directory for a terminal and the start directory for a
  session.
- **`C-x p f` and `C-x p g` (Q5)** in a buffer with no file and no project list the directory it
  stands in, as ADR 0114's Q10 lets a file's directory stand in. The home directory, one that
  holds it, and `/` still say `Not in a project`.

### `C-x C-f RET` lists the directory (Q1)

- **What is typed is selected while it names a directory,** with nothing after its last `/`, as
  vertico's `vertico-preselect` `directory` does. It shows as the selected row on the input row.
  This holds when the prompt opens, after `TAB` or `RET` goes into a directory, and after a name
  typed is deleted.
- **`RET` then gives the directory.** `find-file` lists it, as dired does. `write-file` writes the
  buffer's file name into it, as before for a path ending in `/`. A prompt for a directory gives
  it, as before.
- **`C-n` goes to the first name, and `C-p` back.** Typing a name selects the first match, as
  before. `TAB` puts in the first name, as vertico's `TAB` does from the prompt.
- **A row moved to stays selected** when a listing builds the list again, so a slow directory
  does not take the selection back to the input.
- **A visit of a directory lists it:** `find-file` with a path ending in `/` asks for the listing
  at once. For one without the `/` (`M-RET`), the loader finds a directory on its thread,
  `LoadError::Directory`, and the person's visit then lists it in place of the buffer it made. An
  agent's open of a directory is refused as before, now saying `a directory`.

### A terminal's directory from its program (Q3)

- **The program's OSC 7 `file:` URI** is taken when its host is empty, `localhost`, or this
  machine's name: compared without case, and also as the first label of either (`mac`,
  `mac.local`). The name comes from `oso_platform::host::name()`, `gethostname` on Unix and none
  elsewhere yet, read once at startup. A URI from another machine (after `ssh`) is dropped.
- **Its path** must decode its `%XX` escapes to UTF-8 with no control character, be absolute, and
  have no `.` or `..` segment. OSC 9;9's and OSC 1337 `CurrentDir`'s plain paths are taken as they
  are, under the same rules. `oso-term`'s policy has already dropped any with a control character
  or longer than 4 KiB. Nothing is looked at: the prompt's listing says whether it is there.
- **It moves only where a prompt starts,** which the person sees before `RET`. A terminal or
  session started from that terminal starts where the terminal started, so text the program
  prints (`cat` of a crafted file) cannot choose where a process runs.
- A hosted session's terminal keeps the session's directory, whatever its program sends.
- This is the OSC 7 row of [ADR 0120](0120-the-terminal.md)'s table ahead of its step 6. The shell
  scripts that send OSC 7 at each prompt stay in step 6; until then, OSC 7 comes from the person's
  own shell setup.

### Public items

- `Editor::default_directory` and `Editor::set_buffer_directory`, and `Buffer::directory`, the
  buffer's own directory, in oso-core.
- `oso_platform::host`, a new module with one function, `name()`. It is the module's only item;
  its `unsafe` call has a `// SAFETY:` comment.

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md), "The initial input
  is a directory: the directory of the shown buffer's file, or, for a buffer with no file, the
  directory oso was started in": the directory the buffer stands in, as above.
- ADR 0087, "`RET` on a directory enters it, as `TAB` does, as there is no directory editor yet":
  still so for a directory's row. While what is typed names a directory, it is selected, and `RET`
  gives that directory, which `find-file` lists.
- ADR 0087, "`C-p` on the first candidate selects the typed text": also selected at first while it
  names a directory; `C-n` goes to the first candidate.
- [ADR 0134](0134-a-directory-editor.md), "`C-x C-f`'s `RET` on a directory keeps entering it (ADR
  0087 unchanged)": as above. `RET` on a directory's row enters it; `RET` on the directory typed
  lists it.
- [ADR 0120](0120-the-terminal.md), Q18, "Its directory is the project's root, else the buffer's
  directory, else the home directory": the project's root, else the directory the buffer stands
  in (from a terminal, the one it started in), else the home directory.
- [ADR 0121](0121-agent-sessions-and-spaces.md), "`claude` (`Person`) starts a session in the
  selected buffer's project root, else its file's directory, as a new terminal does, else the
  directory oso started in": the project's root, else the directory the buffer stands in, as a new
  terminal does, else the directory oso started in.
- [ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md), "A buffer with no
  file, the home directory, a directory that holds it … and `/` say `Not in a project`": a buffer
  with no file lists the directory it stands in; the home directory, one that holds it, and `/`
  still say so.

## Consequences

- `C-x C-f RET` lists the buffer's directory everywhere. From a session it lists the worktree the
  session runs in.
- `RET RET` no longer opens the first file of a directory. `C-n RET` does, and typing a name
  works as before. Muscle memory from oso's earlier behaviour changes, as it does when moving from
  vertico with `vertico-preselect 'first` to its default.
- A buffer made from a terminal that has moved stands where that terminal stood then. This is
  Emacs's behaviour, and the person can see it in the prompt.
- A terminal carried over a restart has no start directory of its own, so a terminal started
  from it starts in the home directory. Its program's next OSC 7 gives prompts their place again.
- `oso-platform` gains a small module and one `unsafe` call.
- The tests: oso-core's for each kind of buffer, inheritance, moves and the selection; the
  dispatcher's for `C-x C-f RET` listing the directory, a terminal and `*scratch*`; the host's for
  reading OSC 7 URIs and the loader's for a directory.

## Alternatives considered

- **Keep the directory in the prompt code** (`dispatch/prompts/paths.rs`), as before. Every
  other user (`dired-jump`, terminals, sessions, `C-x p f`) would repeat it, and the rule for each
  kind of buffer would live in four places.
- **Select the input only when the prompt opens** (Q1 (b)). It differs from vertico after `TAB`
  into a subdirectory, which the author uses.
- **Keep `RET` on the first row, and list a directory with `M-RET` only** (Q1 (c)). The author asked
  for `C-x C-f RET`.
- **Let OSC 7 also choose where a process starts** (Q3 (b), as Emacs's `default-directory`
  does). A program's output, not the person, would then pick where the next `claude` runs, and
  Claude Code reads that directory's settings. The prompt shows the path first; a process start
  does not.
- **Leave OSC 7 to ADR 0120's step 6** (Q3 (c)). A terminal's prompts would start where the
  terminal started even after `cd`, which is what the author asked to change.
- **The side pane takes the space's directory always, or the previous buffer's** (Q4 (b), (c)).
  The row at point is what `claude` and `c` already take there, so `C-x C-f` agrees with them.
- **Accept only an empty host or `localhost` in OSC 7.** Most shells' scripts send the machine's
  name, so the directory would rarely be taken.
