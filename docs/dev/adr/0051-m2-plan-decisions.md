# 0051. M2 plan: scope, safety and display decisions

- Status: Accepted
- Date: 2026-11-04

## Context

M2 is where the author starts using oso for real text and notes. Splitting it
into steps ([`roadmap.md`](../roadmap.md)) needed the M2 topics in
[`open-questions.md`](../open-questions.md) settled, and raised a few more.
Reading the code also showed places where an accepted ADR's plan would not work
as written:

- The shaping cache compares a row's absolute buffer offsets, so one inserted
  character misses the cache for every row below it. Invalidating by edit range
  ([ADR 0040](0040-buffers-loading-motion-and-scrolling.md)) does not fix that.
- macOS can end the app from the Dock or at logout, with no chance to ask.
- `KeyEvent.text` follows the logical key without Control or Command, so an
  unbound `C-q` has the text "q", and keys from `send-keys` have no text at all.

## Decision

### Scope

- **Soft wrap joins M2,** as its last step. Long paragraphs of Japanese prose
  are hard to read by scrolling sideways. `C-n` still moves by logical lines.
- **Loading the configuration file moves to M3,** with the declarative menus
  that need it. M2's defaults are compiled in, including the author's
  customizations ([ADR 0019](0019-configuration.md)), so `toml` is not added in
  M2.
- **User key bindings are a layer above the global keymap.** `Keymap::bind` keeps
  refusing to rebind ([ADR 0034](0034-command-registry-and-keymaps.md)).
- **oso has no macOS menu in M2.** Every Command key stays Meta, and quitting is
  `C-x C-c`.

### Editing

- **Shaped rows are compared by content.** A cached row is reused when the bytes
  it covers and its start column match. Work stays bounded by the screen, and
  undo, reload and several panes need nothing more. This replaces the edit-range
  invalidation in [ADR 0040](0040-buffers-loading-motion-and-scrolling.md).
- **Text input is a default binding on a keymap.** It fires for a character key
  without Control, Meta or Super, and runs `self-insert-command`, which reads
  `ctx.input()`, so the command signature does not change. Text of several
  characters goes through `Dispatcher::insert_text`, which IME commits share.
  `KeyEvent.text` is not used.
- **Option stays Super** ([ADR 0033](0033-key-presses-and-notation.md)). Layouts
  that type symbols with Option are not supported.
- **TAB inserts spaces** to the next tab stop until modes bring indentation (M3)
  ([ADR 0030](0030-m2-editing-commands.md)).
- **Undo** keeps removed text as rope slices, which share chunks and cost
  O(log n), and is bounded in bytes. The newest group always stays. A buffer
  whose history was dropped past its saved state stays modified until it is
  saved.

### Keeping work safe

- **The emergency save also runs when the OS ends the app** (Dock quit, logout),
  from winit's `exiting`. This extends [ADR 0013](0013-persistence.md), which
  covers panics only. It writes the same files, with the same exclusions. A
  kill signal or power loss still loses unsaved work.
- **The panic hook prints the location and the stack,** in dev and release
  builds, and never the panic message, which may quote buffer text
  ([ADR 0028](0028-secrets-handling.md)).
- **An unresolved path is told apart from a secret name.** The emergency save
  writes a buffer whose path could not be resolved if no name on the path matches
  a secret pattern. The control socket still treats it as secret. This settles
  the question left by
  [ADR 0044](0044-control-socket-trust-and-secret-buffers.md).
- **Yanking text killed from a secret buffer makes the target buffer secret,** so
  a kill and a yank cannot carry text past the checks of ADR 0044.
- **Files of 2 GiB or more ask before opening,** in the minibuffer, because
  running out of memory aborts and no emergency save runs. The threshold becomes
  configurable with the configuration file.

### `oso dev` once text can be unsaved

- A modified buffer holds the restart, and the old instance stays, until `oso
  dev` can carry the text. The supervisor checks before it starts a new binary.
  It never closes an instance that has been edited, and an instance that refuses
  to exit is left running, not killed.
- It then carries unsaved text in pieces under the line limit, with the modified
  flag, file information, the modification-time baseline, marks and the kill ring
  without entries from secret buffers.
- **A secret buffer with unsaved edits keeps the old instance,** and the echo area
  says why. Saving or reverting it lets the next build through.
- **Undo history is not carried.** This differs from
  [ADR 0023](0023-control-socket-protocol-and-restart.md), which restores
  everything. It would make the handoff much larger, and it only matters during
  development.
- Internal buffers (`*Messages*`, the minibuffer's) are not exported, and do not
  count as unsaved work.

### Control socket

- **Positions in a secret buffer are withheld too.** Once word motion and search
  exist, a client could learn a secret buffer's text from where the cursor lands
  and how many matches a query has. While one is shown, `get-state` omits cursor
  and scroll positions, the modeline and match counts. This narrows the metadata
  that [ADR 0044](0044-control-socket-trust-and-secret-buffers.md) keeps.
- **A sandboxed client of the same user is trusted like any other.** `open-file`
  and `C-x C-f` over `send-keys` let it have an unsandboxed dev instance read a
  file and show it. Dev builds trust the same user, and giving a sandboxed agent
  the socket gives it what the instance can read.

### Display

- **Text gamma is corrected with the theme,** so light text on dark matches native
  apps ([ADR 0027](0027-theme.md)).
- **Boxes of invisible characters have a 1-pixel gap,** and text-grid snapshots
  get a mark for where one ends ([ADR 0045](0045-text-grid-snapshots.md)).
- **IME preedit pushes the text after it,** in the cursor's row only. It is never
  written to the buffer and needs no InlayMap
  ([ADR 0026](0026-m2-ui-layout.md)).
- **Two files with one name** are named as `notes.md<work>`, as Emacs's uniquify
  does by default.

### Dependencies

- **Fuzzy matching is written in `oso-core`.** `nucleo-matcher` is MPL-2.0, which
  `deny.toml` does not allow, and orderless matching needs little code.
- The clipboard crate and `notify` are chosen when their steps start, through the
  normal dependency approval.

## Consequences

- The earliest point at which the author can edit real notes without risking them
  is after M2-12, with files opened as `oso FILE`.
- Agents cannot read cursor positions in a secret buffer over the socket, so such
  tests check only the metadata that remains.
- A yank can make an ordinary file secret. It then gets no emergency save, and
  dev tooling withholds it, until it is opened again.
- The emergency save writes contents to disk on ordinary OS-initiated exits with
  unsaved work, not only in a crash.
- Without a menu there is no Command-H or Command-Q, and the app is hidden and
  quit from the Dock or with `C-x C-c`.
- The first days of use have compiled-in key bindings only.
- A panic costs a little debugging information, since the message is gone.

## Alternatives considered

- **Soft wrap before first use, or in M3:** the first delays real use with a large
  change to the display layer; the second leaves prose hard to read.
- **Configuration in M2:** adds `toml` and reloading while nothing needs them.
- **Explicit rebinding and unbinding in `Keymap`:** no new layer, but no difference
  remains between defaults and user settings.
- **A minimal macOS menu after the first frame:** needs new bindings and `unsafe`,
  and its keys collide with Meta.
- **Panic messages in dev builds, always or behind a variable:** easier debugging,
  but dev instances edit real notes, and logs reach agents.
- **Skipping every unresolved path in the emergency save:** fails closed, but loses
  work in a note whose directory does not exist yet.
- **Emergency save on panic only:** writes less to disk, but the Dock and logout
  would lose work with no warning.
- **Carrying undo history through a restart:** matches ADR 0023, at the cost of a
  larger protocol and a slower restart.
- **Dropping a secret buffer's edits at a restart, with a warning:** never holds a
  restart, but loses work.
- **Asking at 100 MiB or 1 GiB, or never:** the first two interrupt opening files
  oso handles well, and the last can lose other buffers' work.
- **Accepting what positions reveal in a secret buffer:** same-user processes can
  read the file anyway, but an agent could copy derived text into a log unasked.
- **A startup flag limiting which paths the socket opens:** more dev-only code, for
  a limit that only holds if every path through `send-keys` honors it.
- **Drawing preedit over the following text:** simple, as terminals do, but hides
  what comes next while composing in the middle of a sentence.
- **Borders on invisible-character boxes:** read more like boxes, but take four
  rectangles each.
- **`work/notes.md` or `notes.md<2>`:** the first puts the name after the directory,
  and the second does not say which file it is.
