# 0074. The kill ring and the clipboard

- Status: Proposed
- Date: 2026-11-08

## Context

M2-14 brings kills and yanks ([roadmap](../roadmap.md)). Earlier ADRs fixed the outline:

- kills and yanks are per cursor: N cursors store N pieces as one entry, and a yank with a
  different number of cursors joins the pieces with newlines ([ADR 0007](0007-multi-cursor.md));
- kills and yanks sync with the OS clipboard as in Emacs, and the kill ring lives only in
  memory ([ADR 0028](0028-secrets-handling.md));
- `C-w` kills the region if one is active and otherwise the previous word, `C-k` at a line's
  start kills the whole line, and typing over a region replaces it
  ([ADR 0030](0030-m2-editing-commands.md)); `M-y` comes with M2-21;
- yanking text killed from a secret buffer makes the target buffer secret
  ([ADR 0051](0051-m2-plan-decisions.md)), so that a kill and a yank cannot carry text past
  the checks of [ADR 0044](0044-control-socket-trust-and-secret-buffers.md);
- `kill-word` and `backward-kill-word` delete until now, and become kills here with their
  names and keys. Two cursors' ranges can overlap, and `Ctx::repeats` cannot tell that a kill
  follows a different kill ([ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md));
- the region shows only while it is active, and overlapping regions merge into one selection
  while they show, so a kill of regions never takes text twice
  ([ADR 0071](0071-mark-and-region.md));
- `oso-platform` gains `window`, the native window layer, at ADR 0070's stage 2, beside winit
  ([ADR 0070](0070-windowing-and-input-without-winit.md)). Linux will be Wayland only.

What the OS clipboard brings:

- **Reading it can wait on another process.** On Wayland and X11 the text comes from the
  client that owns the selection, through a pipe or a property, and that client may be slow or
  hung. On macOS a pasteboard entry can be a promise that its app fulfils when read. Windows
  blocks while another process holds the clipboard open. None of this may happen on the UI
  thread.
- **Reading it is a privacy event.** Other apps put passwords there, and recent macOS versions
  tell the user when an app reads what another app copied.
- **Writing it publishes text** to every process of the session and to clipboard managers,
  which keep a history, sometimes synced across devices.
- **The clipboard knows nothing of pieces or provenance.** A multi-cursor kill and a kill from
  a secret buffer become plain text there.

## Decision

The author chose on 2026-11-08, through the coordinator, from options put to them: the backend,
reading only on a yank and waiting for it, what a secret kill puts on the clipboard and how its
provenance survives, the three rules for several cursors, appending, the limits and the socket.
Each was the recommended option, and they are marked "(chosen)" where they appear. They accepted
the commands' details below as written with them. The ADR stays Proposed until the pull request.

### Commands

| Keys | Command | What it does |
|---|---|---|
| `C-w` | `kill-region-or-backward-word` | Kills the region at every cursor if it shows, otherwise as `backward-kill-word` |
| `M-w` | `kill-ring-save` | Copies the region at every cursor and hides it, as a kill that deletes nothing |
| `C-k` | `kill-line` | Kills to the end of the line; through the line break at its end, or after only blanks, or at its start (`kill-whole-line`) |
| `M-d` | `kill-word` | Kills to the end of the next word (ADR 0058's motion) |
| `M-DEL` | `backward-kill-word` | Kills to the start of the previous word |
| `C-y` | `yank` | Inserts the newest kill, or the clipboard's text, at every cursor |

- **`kill-line` follows Emacs with `kill-whole-line` on:** at a line's start it takes the line
  and its line break; elsewhere it takes the rest of the line, or the line break if only blanks
  follow. A count N takes N lines with their breaks, a negative one goes back, and `C-u 0`
  takes back to the line's start. At the buffer's end it signals "End of buffer". Lines are
  logical lines, as `C-n` moves by (ADR 0030), and soft wrap (M2-25) does not change that. The
  line's end is a lookup in the rope; only the check for trailing blanks walks, within a
  stop's budget (ADR 0058), and a line longer than that counts as not blank.
- **`M-w` with no region shown** copies between each cursor and its mark, as Emacs does, and
  signals "The mark is not set now" for a cursor with none. `C-w` with no region shown never
  uses the mark.
- **`yank`** inserts at every cursor, sets each cursor's mark at the start of what it inserted
  and leaves the region hidden, as Emacs's `push-mark` in `yank` does
  ([ADR 0071](0071-mark-and-region.md)). `C-u C-y` leaves the cursor at the start and the mark
  at the end. `C-u N C-y` yanks the Nth newest entry, as in Emacs; it reads only the ring.
- **A read-only buffer** refuses a kill's deletion but copies the text, and signals "Buffer is
  read-only", as Emacs's `kill-read-only-ok` nil does. So `C-k` copies lines from `*Messages*`.
- **One kill or yank is one undo step** across all cursors. Undo does not change the ring.

### Typing over a region (`delete-selection-mode`)

- **With a region shown, typing, an input method's commit, `RET` and `C-y` replace it** at
  every cursor, in one edit and one undo step. `TAB` does not, as in Emacs, where it
  indents.
- **`DEL`, `C-d` and `<delete>` delete the region,** as Emacs's `delete-active-region`. With a
  count other than 1 they delete characters, as Emacs does, and an empty region is no
  region for them (`use-empty-active-region` nil).
- What a region is replaced with or deleted by does not go to the kill ring, as in Emacs.
- Other commands that edit keep the region's rule from ADR 0071: the edit hides it.

### Appending (chosen)

- **A kill straight after a kill appends to its entry,** as Emacs's `last-command` rule does:
  forward kills at the end, backward kills (`M-DEL`, `C-w` without a region, `C-k` with a
  negative count) at the start. `M-w` after a kill appends too, as in Emacs.
- **"Straight after" means the previous command killed,** whatever it was. The editor keeps a
  flag, as it keeps `join_ready` for undo: a kill sets it, and the end of any other command
  and an undefined or cancelled key sequence clear it. `C-u` and digits before a kill are
  not commands that end it, as they leave a prefix and nothing else. So `M-d C-k` appends and
  `M-d C-f M-d` does not.
- **Pieces append to pieces** when the new kill has as many as the entry. Otherwise the new
  kill starts an entry of its own.

### Several cursors (chosen)

- **Overlapping ranges take their union** (chosen), as `apply_edits` already merges them: each
  merged range is one piece, in buffer order, and the cursors in it merge into one. A kill that
  deletes also joins ranges that touch, as their cursors meet once the text is gone. A cursor
  with nothing to take gives an empty piece. So an entry's pieces match the cursors that remain
  after the kill, and a yank with them puts each piece back where it came from.
- **A yank with as many cursors as pieces** puts each piece at its cursor, in buffer order.
  **Any other count** inserts all the pieces joined with line breaks at every cursor
  (chosen), as ADR 0007 says and multiple-cursors.el does. One piece is the same text at every
  cursor.
- **The clipboard gets the pieces joined with line breaks** (chosen), with `\n`, as
  multiple-cursors.el does, so another app gets all the text. oso keeps the pieces in its own
  entry and finds them again when the clipboard still holds what it wrote (below).
- Typing over regions and `M-w` work on every cursor's region.

### The ring

- **`KillRing` in a new public module of `oso-core`, `kill`,** owned by `Editor`, as the undo
  history is. An entry holds its pieces as rope slices, which share chunks with the buffer
  and its undo history, so a kill of a large region copies no text on the UI thread, and
  appending is O(log n). An entry also holds where it came from (below) and whether it is
  secret.
- **Bounds (chosen):** at most 120 entries, as Emacs's `kill-ring-max`, and 64 MiB of text
  counted as the entries' lengths, the newest always kept whatever its size, as the undo
  history bounds itself. The oldest go first.
- **A yank inserts at most 16 MiB across all cursors** (chosen), the bound typing has
  ([ADR 0056](0056-typing.md)), and otherwise signals "Too much text to yank". Inserting
  copies the text, about 40 ms for 16 MiB. A kill has no bound, as it copies nothing.
- `C-x h C-w` on a 1 GB file therefore kills at once, yanks back with `C-z` only, and puts
  nothing on the clipboard (below).

### The clipboard (chosen)

- **A worker thread owns the clipboard,** in `oso_platform::clipboard`, a new public module
  beside `window`, on the same layer line. Its API has no OS types: plain text in, plain text
  out, a concealed flag, and a change token. The tip crate's `app/clipboard.rs` talks to it, as
  `app/saves.rs` talks to the saving thread. The UI thread never calls into the OS clipboard.
- **Backends (chosen): oso's own code, no new crate.**
  - **macOS:** `NSPasteboard` through `objc2-app-kit` 0.3, which ADR 0070 approved for the window
    layer. It becomes a macOS dependency of `oso-platform` with or without `native-window`, with the
    `NSPasteboard` and `NSPasteboardItem` features, beside `objc2` (`std`) and `objc2-foundation`
    (`NSString`, `NSArray`) as direct dependencies at the versions stage 2 uses. One pasteboard item carries the text and, for a secret entry, the markers below. Every
    `unsafe` call has a `// SAFETY:` comment.
  - **Linux:** the core `wl_data_device` on the native window's own Wayland connection, at ADR 0070's
    stage 4, as the Wayland docs ask of an app with a window. A selection is set with the serial of the
    key that killed, and read through a pipe with `poll` and a timeout.
  - **Windows:** Win32 through `windows-sys`, at stage 4 with the window layer that adds it.
  - **Until stage 4, Linux and Windows keep the ring in oso only:** a kill and a yank work there as in
    macOS, with no clipboard: a backend that holds nothing stands in. Both are Tier 2
    ([ADR 0003](0003-platform-tiers.md)), and CI builds and tests them as now.
- **Writing:** after a command that killed, the host takes the new or grown entry from the
  editor (`Editor::take_clipboard_write`, as `take_save_jobs`), and hands the rope slices to
  the worker, which joins them into one string and writes it. A write not yet started when a
  newer one comes is dropped, so a run of kills costs one write, and a read waits behind one
  at most. An entry over 16 MiB is not written, and the echo area says "Not copied to the
  clipboard: too large"; it stays in the ring.
- **Reading only on a yank** (chosen): never on focus, never in the background, never on a
  timer. `C-y` with no prefix asks the worker for the clipboard; `C-u N C-y` reads only the
  ring.
- **A yank waits for its answer without blocking** (chosen), as a command that reads an argument
  waits for its prompt ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)): `yank`
  declares that it reads the clipboard (`ArgSpec::CountAndClipboard`), and the dispatcher keeps it
  waiting with its prefix argument, asks the host to read, and runs it through the same `execute`
  once the answer comes, with the answer in `ctx.clipboard()`. The command stays a plain function
  and never waits. Keys, text and commits that arrive meanwhile are held, in order, and go on
  once it has run. `run-command yank` answers at once, and the yank runs when the clipboard
  answers; `run-command` while a yank waits is refused, and `get-state`'s `clipboard.waiting`
  says when it is over. A plain `C-u` reads the clipboard too; a number reads only the ring. The
  worker first asks whether the clipboard still holds what oso last wrote, through the
  cheapest signal the system has (macOS `changeCount`, Windows `GetClipboardSequenceNumber`,
  Wayland whether the selection's source is oso's own), and only reads the text if not. The
  answer comes back as a `UserEvent`.
  - **Its own text:** the yank inserts the ring's newest entry, with its pieces and its
    provenance.
  - **Text from another app:** it becomes a new entry of one piece, at most 16 MiB (more is
    not read, and the yank takes the ring's newest with a message), and the yank inserts it, as
    Emacs's `interprogram-paste-function` does. The same text again, by its fingerprint, adds no
    second entry.
  - **No text, no answer within 500 ms, or a read still running from an earlier yank:** the
    yank inserts the ring's newest entry, and the echo area says "Clipboard not read" if there
    was no answer. Two threads keep this bound: one owns the clipboard and may wait on another
    process, and one answers the host in time and drops a late answer. On macOS a stuck read
    ends when the pasteboard server gives up, and after 5 s, as Emacs's `x-selection-timeout`,
    oso leaves that worker behind; the Wayland backend (stage 4) reads its pipe with `poll` and
    the same timeout.
- **Tests and dev runs:** `--clipboard memory` (dev builds only) gives the worker an in-memory
  clipboard, so tests and `just e2e` never touch the author's clipboard, and CI needs none;
  `just perf` sends no kill. The socket's new `clipboard` method reads and sets it, standing in
  for another app, as the `ime` method stands in for the input method. A release build always
  uses the system's, and has no other.
- **Input from the socket never reads the system clipboard.** A yank that `send-keys`,
  `run-command` or `ime` causes reads only a clipboard in memory; with the system's it takes
  the ring's newest entry, so no client reads the author's clipboard through oso. Where input
  came from travels with it: the waiting yank and each input held behind it keep it.
- **A yank lands where it was asked for, or not at all.** If the selected window, its buffer or
  the open prompts change before the clipboard answers (a close request, a dev request), the
  yank is dropped with a message, and the held input goes on.
- **A read that never returns is left behind.** After 5 s the worker on it is abandoned and a
  new one takes over, and the echo area says once "The clipboard is not answering". Writes wait
  in the front while a read is out, never behind it.

### Secret buffers (chosen)

- **A kill from a buffer that may hold secrets** (any verdict but `Ordinary`) is a secret entry.
- **A kill from a passphrase prompt** (`Verdict::Passphrase`, ADR 0072) reaches neither the ring
  nor the clipboard, as ADR 0072 asks; it deletes, and a yank into the prompt works.
  `Verdict::may_be_killed` is false for it, and names every verdict, so a new one decides this
  where it is added.
- **What the clipboard gets for it: the text, marked concealed** (chosen). On macOS the item also
  carries `org.nspasteboard.ConcealedType` (and `TransientType`), which clipboard managers that
  follow nspasteboard.org skip; on Windows `ExcludeClipboardContentFromMonitorProcessing` and
  `CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard` as 0, which keep it out of
  the clipboard history and the cloud; on Wayland `x-kde-passwordManagerHint` as `secret`,
  which Klipper honours. A key can still be pasted into a browser, as password
  managers allow.
- **Where an entry came from survives the round trip** (chosen):
  - the clipboard's change token, recorded when oso wrote an entry, tells oso that the
    clipboard still holds that entry, and the yank then uses the entry, secret flag and all;
  - text from another app counts as secret if it equals the text of a secret entry still in
    the ring (compared by length and a hash the entry keeps, on the worker), so copying a key
    through another app and back does not wash it, or if the clipboard marks it concealed, as
    a password manager does;
  - anything else is ordinary.
- **Yanking a secret entry makes the target buffer secret,** as ADR 0051 says: a flag on the
  buffer, beside its verdict, which nothing that judges the text clears, read as a verdict,
  `Yanked`, withheld from the socket and from the emergency save as `SecretName` is, until the
  file is opened again in a new buffer. A name that matches and a passphrase keep their own
  verdicts, so a passphrase stays one. `oso dev` refuses to hand such a buffer over by its path,
  so the old instance stays until it is closed. The echo area says "Buffer now treated as
  secret".
- **The ring remembers what secret entries went out as:** the fingerprints of the last 64, apart
  from the entries, so a key copied out and back still counts as secret after more was appended
  to its entry, or after the ring dropped it. Text from another app equal to the newest entry
  adds no entry, but makes it secret if it comes back marked concealed.
- **A secret entry is never shown over the socket and never handed over by `oso dev`**
  (ADR 0051), and nothing logs an entry's text.

### The control socket

This is a human gate ([`REVIEW.md`](../../../REVIEW.md)).

- **`get-state`** gains `kill_ring`: how many entries oso killed from ordinary buffers, and the
  newest one's pieces, at most 16 pieces of at most 1 KiB each, with `truncated`. **Secret
  entries and text taken from the clipboard are left out entirely,** as if absent: no count, no
  length, so no client reads another app's copy through oso, even after its yank is undone. While a buffer that may
  hold secrets is shown, `kill_ring` is null, as its positions are.
- **`clipboard`** (dev, with `--clipboard memory` only) reads or sets the in-memory clipboard,
  with or without the concealed flag. Text marked concealed reads as null, as it may be a
  secret kill's. With the system clipboard it returns an error, so no client reads the
  author's clipboard through oso.
- `get-state` gains `clipboard.waiting`, true while a yank waits for the clipboard.
- `export-state` does not carry the ring until M2-15, which carries ordinary entries only.

### What it is made of

- `oso_core::kill`: `KillRing`, `KillEntry` (pieces, whether secret, fingerprints), `Piece`,
  `Fingerprint`, `FromClipboard` and `kill_ranges`, the pure function that merges the ranges of
  several cursors. The yank's text per cursor is worked out in the editor (`editor/kill.rs`).
- `Editor`: the ring, the append flag, `kill`, `yank`, `take_clipboard_write` and
  `take_from_clipboard`, which decides whether text from another app is secret; and a verdict
  `Yanked`.
- `ArgSpec::CountAndClipboard` and `Ctx::clipboard`: the clipboard's answer for the command that
  waited for it, only `yank` so far.
- `oso_platform::clipboard`: the worker, with a backend per system, `cfg`-gated in files of its
  own, and the in-memory one.
- `oso`: `commands/kill.rs` and `bindings/kill.rs`; `kill-word` and `backward-kill-word` move
  there from `edit`; `app/clipboard.rs`; the dispatcher's pending yank.
- `guardrails/layers.txt`: `kill` in `[oso-core]` beside `undo`, below `editor`; `clipboard` in
  `[oso-platform]` on the top line, beside `window` from stage 2.
- `oso::dispatch::clipboard`, a private module, holds the waiting command and the held input.
  ADR 0072's `run_or_read` decides that a yank waits, as it decides what a command reads in a
  prompt, so one place says what a command waits for. A prompt takes the keys while it waits;
  the clipboard holds them,
  and `--clipboard memory` is a dev build's option, after `--emergency-dir` if both are given.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0028](0028-secrets-handling.md), "copying a secret still puts it on the OS clipboard,
  where clipboard managers may keep it": it is marked concealed, which managers that honour the
  markers skip.
- [ADR 0030](0030-m2-editing-commands.md), `C-w` "delete the previous word": it kills it.
- [ADR 0071](0071-mark-and-region.md), "until M2-14, typing with a region inserts and ends it":
  it replaces the region.
- [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md): `kill-word` and
  `backward-kill-word` kill, and move from `commands/edit.rs` to `commands/kill.rs`.

## Consequences

- Killing and copying are O(log n) per cursor on the UI thread, whatever the size; joining the
  text for the clipboard is the worker's.
- A yank waits for the clipboard, and keys typed in that moment wait with it. Usually this is
  well under a frame; a hung clipboard owner costs at most 500 ms once.
- oso reads the clipboard only when asked to yank, so macOS's notice about reading another
  app's text appears then and only then.
- Keys from a password manager or a secret buffer reach the clipboard marked concealed. A
  manager that ignores the markers still keeps them.
- A yank can make an ordinary file secret, as ADR 0051 says.
- Entries over 16 MiB stay out of the clipboard, and a yank over 16 MiB is refused until a
  yank inserts rope slices without copying.
- Tests and dev runs need `--clipboard memory` to stay off the author's clipboard; the e2e
  harness passes it.
- On Linux and Windows the clipboard comes only with the native window layer (stage 4); until then
  their kills stay in oso. On Wayland, what oso put on the clipboard goes when oso exits, unless a
  clipboard manager took it.
- `NSPasteboard` documents no rule for threads. arboard and others use it off the main thread; the
  worker is its only user in oso, and the e2e flows and the author's checks exercise it.
- On macOS 15.4 and later the system may ask the person once whether oso may read what other apps
  copied. Reading only on `C-y` keeps that to a yank.
- `objc2-app-kit` becomes a macOS dependency of every build, not only with `native-window`; it is in
  the tree from stage 2 either way.
- `NSPasteboard` has no cheap way to ask a text's size, so text from another app is read whole on
  the clipboard's thread before the 16 MiB bound applies.
- The public API gains `oso_core::kill`, `oso_platform::clipboard`, a verdict and `Ctx`'s
  answer, as the API snapshots show.

## Alternatives considered

- **`arboard` 3.7 on every system:** the least code, and it marks secret text on all three. But on
  Windows it needs `clipboard-win` and `error-code`, under BSL-1.0, which `deny.toml` does not allow;
  its Wayland mode is `wl-clipboard-rs` over the data-control protocols, which GNOME's Mutter does not
  have and which its own docs leave to tools with no window, and otherwise it falls back to X11, which
  ADR 0070 leaves; and a read on Linux waits up to 4 s on X11 and with no limit on Wayland. Its macOS
  backend adds only itself once `objc2-app-kit` 0.3 is in the tree.
- **Own code on macOS now and `arboard` on Linux and Windows until stage 4:** the clipboard everywhere
  at once, at the cost of the licence exception and code that stage 4 replaces.
- **`copypasta`, `clipboard-rs`, `window_clipboard`:** each needs `clipboard-win` too; the first is on
  the older `objc2` 0.5, the second brings the `windows` crate, the third is tied to iced's helper
  crates.
- **Reading on the UI thread:** simple, but the owner of the selection can hang it.
- **A copy kept up to date off the key path** (polling `changeCount`, or the Wayland selection event on
  focus): a yank never waits, but oso would read everything another app copies, passwords included,
  and on macOS each read is an access the privacy notice counts.
- **Reading on focus,** as some editors do: the same reads, for every switch of apps.
- **Waiting with no bound on the clipboard:** Emacs's `x-selection-timeout` waits 5 s with the whole
  editor stopped; oso's keys would wait as long.
- **Nothing on the clipboard from a secret buffer:** safest, but a key could not be pasted into a
  browser, and people would copy it from another editor instead.
- **The usual text from a secret buffer,** as ADR 0028 accepted: clipboard managers keep it, and some
  sync it to other devices.
- **Clearing a secret entry from the clipboard after a while,** as password managers do: Emacs has no
  such thing, and it can be added later on top of the markers.
- **Provenance by the change token only:** a key copied out through another app and back would come
  back ordinary.
- **Each piece to a line when an app's text has as many lines as there are cursors,** as VS Code's
  `multiCursorPaste` does: handy, but text that happens to have N lines would be split.
- **Only the primary cursor's piece on the clipboard:** other apps would see a part of the kill.
- **For overlapping ranges, each byte to the first cursor's piece,** keeping N pieces, some empty:
  pieces would no longer match the cursors left after the kill.
- **Appending only when the same command repeats** (`Ctx::repeats`): `M-d C-k` would not append, unlike
  Emacs.
- **No bound on a yank,** as Emacs: 1000 cursors could insert gigabytes and run out of memory, which
  aborts with no emergency save (ADR 0051).
- **Inserting rope slices for a yank without copying:** no bound needed, but it changes the edit core
  and the display map's updates; it can come later.
- **The whole ring over the socket:** more to test with, but a large ring makes `get-state` large, and
  the top entry is enough for the flows.
