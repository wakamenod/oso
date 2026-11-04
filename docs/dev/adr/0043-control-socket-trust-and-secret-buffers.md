# 0043. Control socket trust and secret-pattern buffers

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0016](0016-dev-control-socket.md) gives dev builds a control socket that
only same-user processes can reach. Once `run-command` and a `get-state` with
buffer text exist, that socket can do two more things worth deciding about:

- Any same-user process can run any command, and read what the editor shows.
- `get-state` and `screenshot` could copy a `.env` file or an SSH key to
  whoever asks. [ADR 0028](0028-secrets-handling.md) defines the secret
  patterns, but only for the emergency save, and says neither how a symlink
  counts nor where the check lives.

A response line is limited to 1 MiB ([ADR 0032](0032-control-socket-transport.md)),
and a buffer can be 1 GB, so returning "the text" needs a rule too.

## Decision

- **Dev builds trust the same user.** Any process running as the same user
  can run any registered command through the socket, and can read all state
  except the text below. Release builds contain none of this, which
  `just release-safety` checks. This is reviewed when commands that spawn
  processes arrive (M3 onward), since `run-command` then reaches them.
- **Secret-pattern buffers give no text and no pixels.** While a window shows
  such a buffer, `screenshot` returns an error. `get-state` returns no text
  for it and `get-text` returns an error. The metadata (name, path, size,
  positions) stays.
- **The state handoff never carries their text.** `export-state`
  ([ADR 0023](0023-control-socket-protocol-and-restart.md)) hands a secret
  buffer over by path and position only, and the new instance reads the file
  again and judges it like any file it opens. Otherwise `oso ctl export-state`
  would undo this decision. Nothing unsaved exists until M2. From M2, a secret
  buffer's unsaved edits cannot be handed over, so `oso dev` should keep the
  old instance rather than lose them; this is decided with M2's editing.
- **The matcher** is a private module of `oso-core`, `secret`, made of pure
  functions of paths. Everything else asks `Buffer::is_secret`, so the emergency
  save (M2) uses the same verdict. It implements the ADR 0028 list and is
  broader in two ways, both toward calling more files secret:
  - ASCII case is ignored, because the default file systems on macOS and
    Windows open `.ENV` as `.env`.
  - `.ssh`, `.gnupg` and `.aws` count anywhere in a path, not only under the
    home directory. This needs no `$HOME`, which can be wrong or unset, and
    also covers other users' homes and backups.
- **Every path on the way counts:** the path as opened, each symlink's target
  and the canonical path. So a symlink to `~/.ssh/id_rsa` is caught, and so is
  a chain through `~/.aws/credentials` to `~/dotfiles/aws-creds`, which a
  dotfile manager may set up and the canonical path alone would miss. A path
  that cannot be resolved counts as secret: a link to nothing, a loop of links,
  a new file whose directory does not exist, or any error on the way.
- **The verdict is a flag on `Buffer`.** `Buffer::visiting` starts as secret.
  The loading thread opens the file, then resolves the path, then reads, so a
  file swapped in before the open is what gets judged. The UI thread applies
  the verdict before it applies any content. So the UI thread never touches
  the file system, and a file is withheld from the moment it is visited until
  it is known to be ordinary.
- **Bounded work and text.**
  - `get-state` returns the rows the selected window shows in full, clipped to
    its columns and to 128 KiB, and says so with `truncated`. So it works on a
    1 GB line or a screen of combining marks, and its metadata always gets
    through.
  - The new method `get-text` returns whole rows, at most 512 KiB and 10,000
    rows, and its error says what was asked for. It is checked and copied on a
    background thread from an O(1) snapshot, because 10,000 rows take longer
    than a frame.
  - `run-command` takes names of at most 256 bytes and counts of at most 10,000
    either way, because a command repeats its work on the UI thread.
  - The server turns any response over 1 MiB into an error, so a client never
    sees a cut line.
- **Responses are safe to print.** File text now reaches `oso ctl`'s output.
  JSON leaves some characters raw, so DEL, the C1 controls and what the editor
  draws as boxed code points (bidi controls, invisible tag characters) go out
  as `\uXXXX`. They parse back to the same text.
- **The idle limit covers a whole request.** [ADR 0032](0032-control-socket-transport.md)
  disconnects clients after 60 s without a request. This now holds for a client
  sending a byte now and then, too.
- **`show-buffer`** is a new dev method that shows a buffer by id. It exists
  because no command switches buffers until M2, and leaving a secret buffer
  has to be testable. It goes away once `switch-to-buffer` exists.
- This replaces the consequence in
  [ADR 0040](0040-buffers-loading-motion-and-scrolling.md) that `get-state`
  never returns buffer text.

## Consequences

- Agents can read the text of ordinary files, and a same-user process can
  drive the editor. Running as the same user already gives a process access to
  the user's files, so this adds little until commands can spawn processes.
- A file is reported as secret for a moment after it opens, even if it is
  ordinary. Clients wait for `loading` to become null, as they already do to
  read its text.
- Cursor positions stay, so a client can step through a secret buffer with
  `run-command` and learn the length of each line and the width of each
  character, though no character. This is accepted while motion is all that
  exists, and reviewed when word motion or search arrives, which reveal more.
- The check is by name only. A hard link to a key under an ordinary name, or a
  key in a file called `notes.txt`, is not caught.
- The path is resolved once per load, so a symlink changed later keeps the old
  verdict until the file is loaded again. A file swapped twice around the open,
  on purpose, can also get past it; a process that can do that can read the file
  itself. Saving under a new name (M2) must resolve the new path.
- Some ordinary files are withheld, such as `.aws/README.md` in a project. They
  still open and edit normally, and only dev tooling and the emergency save
  treat them differently. For the emergency save the same caution costs unsaved
  work in such a file, or in one whose path could not be resolved. M2 decides
  whether to tell those two cases apart.
- User-defined patterns ([ADR 0028](0028-secrets-handling.md)) are not supported
  yet: the patterns are constants and the resolved paths are dropped after the
  verdict. Config reload will need to judge open buffers again.
- The checks cover what leaves through the socket's own methods. Commands
  added later that move text, such as ones copying to the clipboard, need their
  own review.

## Alternatives considered

- **Masking the text in place (`***`):** clients could mistake the mask for
  content, and a screenshot would need rendering support for it.
- **No masking, relying on same-user trust:** a same-user process can read the
  file anyway, but agents would copy secrets into logs and transcripts without
  anyone asking them to.
- **Checking the canonical path on the UI thread:** simple, but `canonicalize`
  can block on a slow file system.
- **Checking only the canonical path:** `fs::canonicalize` does the walk for
  free, but it misses a chain that passes through a secret path.
- **Matching `~/.ssh` only, with `$HOME` as a parameter:** closer to ADR 0028's
  wording, but it fails open when `$HOME` is wrong.
- **A `text` parameter on `get-state` instead of `get-text`:** one method fewer,
  but a request for text would also rebuild the whole state.
