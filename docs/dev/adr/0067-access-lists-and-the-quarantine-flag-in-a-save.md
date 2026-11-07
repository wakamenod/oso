# 0067. Access lists and the quarantine flag in a save

- Status: Proposed
- Date: 2026-11-07

## Context

[ADR 0059](0059-safe-save-and-the-exit-guard.md) writes a new file and renames it into place, so nothing of the old file carries
over by itself. It keeps the owner, the group and the mode. A file's access list decides who may reach it as well, and `std` cannot read one.
[ADR 0065](0065-access-lists-on-private-directories.md) brought `oso_platform::file_meta`, the module ADR 0059 expected this to need.

A new file also takes entries from the list of the directory it is made in, whatever mode it was asked for.

## Decision

### The order of a write

This extends the order in ADR 0059. Only the first of these comes before the text.

1. **Before the file is made,** read what the file being replaced holds: its access list and its quarantine flag.
2. **Right after the temporary file is made,** take off the list it took from its directory, and make sure none is left. So only this user can read it
   while the text goes in, as mode `0600` says.
3. **After the text,** give it the owner and group, as before. Then its quarantine flag, then its list, and read the list back. Then its mode.
   - On macOS a list applies at once. The people it allows could read the old file too, and what it denies is in force before the mode opens anything.
   - On Linux the group bits of a mode are a POSIX list's mask. So the list goes on with the mask and others shut, and the owner's rights alone, and
     setting the mode opens all of it in one step. If the group cannot be kept, the mode narrows the mask as ADR 0059 says, and named users and groups
     with it, which is the safe way to be wrong.
4. **Then the fsync,** after all of this as before, so that the file cannot land without them.
5. **Right before the rename,** read the old file's list again. A list changed during the write counts as a change on disk, as a `chmod` does, since the
   rename would undo it. The quarantine flag does not count: see below.

### When a list cannot be carried

The save is refused. If a list cannot be read, removed, set or confirmed, the temporary file goes, the file on disk is as it was, the buffer stays
modified, and the message says so without quoting text: "Not saved: its access list could not be kept, and the file is as it was". A failed save takes
a confirmed exit back, as ADR 0059 says.

### A new file

A new file, or one deleted since it was read, gets what its directory says. That is what the directory's owner asked for. It goes on after the text, like any other list, and
then the mode.

### What a list can deny

On macOS a rename needs the right to delete both the file it moves and the file it replaces. So a file whose list denies this user deleting it cannot be
replaced, and a new file cannot be put in place if what its directory gives denies that. Both are refused with "Not saved: its access list denies
replacing or moving it". Which user an entry names cannot be told without asking a directory service, so only the system's answer counts: a list that denies
someone else lets the save through.

A temporary file holding such a list cannot be removed either. So when removing it fails, its list is cleared by its path and removing it is tried again.
One a process left behind, listed `left by a save` ([ADR 0064](0064-recovering-an-emergency-copy.md)), has its own list cleared the same way when
the person deletes it there, and only a list that denies deleting it.

### The quarantine flag

- **Kept on macOS,** as it says the file came from elsewhere and that Gatekeeper should check it. It goes on before the list, which may deny setting it.
- **If it cannot be read or set, the save goes on and the flag is lost,** and the message says so: "Wrote the file, but its quarantine flag could not be
  kept". The text reaching the disk counts for more. This is the author's choice.
- **It does not count as a change.** A conflict is a refusal, so a flag changed during the write does not cause one.
- macOS's own table for this, `xattr_preserve_for_intent`, says not to keep it in a save. Here the flag is kept all the same, as what was downloaded still is. That table
  also answers yes for names it does not know, so it is not used for anything here.

### Other attributes

Other extended attributes, resource forks, Finder tags and Finder data do not carry over, as ADR 0059 says. Windows stays as it says too, recorded in `open-questions.md`.
A list on Linux means a POSIX one, and a file system with no lists has nothing to carry.

### Opening a file to find out if this user may write

`file_meta::open_for_writing` opens with `O_NOFOLLOW` and `O_NONBLOCK`, as what is there may have changed since it was looked at. A link is refused and a
pipe with nobody reading does not wait. The loader's own open stays as `open-questions.md` says.

### Where it lives

- `file_meta` gains `AccessList`, `Quarantine`, `clear_access_list_at` and `open_for_writing`. None holds an OS type. A list is bytes as the system gives them, which this process only
  compares and hands back.
- `file_meta::linux` is a new backend, through extended attributes in `libc`, with no library. `file_meta::posix_list` knows the bytes, and is plain arithmetic, tested on every system.
  A list in a form it does not know is refused, as what it would mean cannot be told.
- The macOS calls are declared in `file_meta::macos`, as ADR 0065 says. No dependency is added.
- `saver::attributes` holds what `saver` keeps of the owner, the group and the mode, as `saver` would otherwise pass its size limit.

## Consequences

- A save costs a few more system calls, off the UI thread: reading the list and the flag, taking the temporary file's list off, and reading the list again before the rename. A file with
  a list costs a few more.
- A file whose list denies this user deleting it still cannot be saved, until M2-16 can ask.
- An unexpected error from a file system refuses a save.
- Tests set real lists with `chmod` and `setfacl` through `oso_platform::process`, so they run with the `dev` feature.
- `saver`'s messages gain two refusals and one warning.

## Alternatives considered

- **Refusing any file with a list:** the least code, but such a file could not be saved at all until M2-16 and M2-18.
- **Writing in place when there is a list:** a crash in the middle damages the file, which ADR 0059 is designed against.
- **Warning after the save:** what the list allowed or denied would no longer apply.
- **Putting the list on before the text:** a failed copy would have left text readable. Now nobody but this user can read it before it is whole.
- **`fcopyfile`:** one call, but its man page does not say what it does with what the destination took from its directory, and it needs the old file open to read.
- **Refusing a save when the quarantine flag cannot be kept:** the text would stay out of the file for a flag.
- **Copying every extended attribute, or those `xattr_preserve_for_intent` allows:** it would copy names that must not be copied, such as a stale code signature.
- **Stripping an inherited `deny delete` from a new file:** it would differ from what the directory says.
- **`libacl`, or the `exacl` and `xattr` crates:** a dependency for what `libc` already reaches.
