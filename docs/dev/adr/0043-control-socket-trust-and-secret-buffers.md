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
- **The matcher** is `oso_core::secret`, pure functions of paths, so the
  emergency save (M2) uses the same rules. It implements the ADR 0028 list and
  is broader in two ways, both toward calling more files secret:
  - ASCII case is ignored, because the default file systems on macOS and
    Windows open `.ENV` as `.env`.
  - `.ssh`, `.gnupg` and `.aws` count anywhere in a path, not only under the
    home directory. This needs no `$HOME`, which can be wrong or unset, and
    also covers other users' homes and backups.
- **Both the path as opened and its canonical path count**, so a symlink to
  `~/.ssh/id_rsa` is caught. A path that cannot be resolved counts as secret.
  That includes a link to nothing, and a new file whose directory does not
  exist.
- **The verdict is a flag on `Buffer`.** `Buffer::visiting` starts as secret.
  The loading thread resolves the path before it reads anything, and the UI
  thread applies the verdict before it applies any content. So the UI thread
  never touches the file system, and a file is withheld from the moment it is
  visited until it is known to be ordinary.
- **Bounded text.**
  - `get-state` returns the rows the selected window shows in full, clipped to
    its columns, so it works on a 1 GB line.
  - The new method `get-text` returns whole rows, at most 512 KiB and 10,000
    rows. It compares the size of the rows with the limit before copying
    anything, and its error says what was asked for.
  - The server turns any response over 1 MiB into an error, so a client never
    sees a cut line. Text with many characters to escape can reach this.
- **`show-buffer`** is a new dev method that shows a buffer by id. It exists
  because no command switches buffers until M2, and leaving a secret buffer
  has to be testable.

## Consequences

- Agents can read the text of ordinary files, and a same-user process can
  drive the editor. Running as the same user already gives a process access to
  the user's files, so this adds little until commands can spawn processes.
- A file is reported as secret for a moment after it opens, even if it is
  ordinary. Clients wait for `loading` to become null, as they already do to
  read its text.
- The check is by name only. A hard link to a key under an ordinary name, or a
  key in a file called `notes.txt`, is not caught.
- The path is resolved once per load, so a symlink changed later keeps the old
  verdict until the file is loaded again. Saving under a new name (M2) must
  resolve the new path.
- Some ordinary files are withheld, such as `.aws/README.md` in a project. They
  still open and edit normally; only dev tooling and the emergency save treat
  them differently.
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
- **Matching `~/.ssh` only, with `$HOME` as a parameter:** closer to ADR 0028's
  wording, but it fails open when `$HOME` is wrong.
- **A `text` parameter on `get-state` instead of `get-text`:** one method fewer,
  but a request for text would also rebuild the whole state.
