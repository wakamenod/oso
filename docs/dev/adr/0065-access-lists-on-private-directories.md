# 0065. Access lists on private directories

- Status: Proposed
- Date: 2026-11-07

## Context

[ADR 0062](0062-emergency-save-and-the-panic-hook.md) keeps emergency copies in a private directory, and the control
socket's directory uses the same checks. A mode does not say everything about who may reach a file on a system
with access lists. macOS evaluates a list's entries alongside the mode, a new file or directory takes entries from the
directory it is made in, and whatever mode it was asked for does not change that.

macOS also puts lists on directories by itself: `~` and many directories under it hold `group:everyone deny delete`. So a
rule that refuses any list would refuse every home directory.

`libc` is a release dependency of `oso-platform` on Unix since ADR 0062. It does not bind the list calls, which live in
libSystem.

## Decision

### The rule, on macOS

A `deny` entry takes rights away, so it never counts, wherever it is. What counts is `allow`, or an entry of a kind this
does not know:

- **The directory itself** holds none, for anything, whoever it names and whether it applies to the directory or only to
  what is made in it.
- **A directory on the way to it** holds none that can change what is in it: adding a file or a directory, deleting one,
  deleting itself, changing its list or owner. One that only reads is fine, as mode `0755` is.
- **The deepest directory that exists, when something is to be made under it,** holds none that reaches a new directory.
  It runs before anything is made, as the walk does. A list reaches only what is made after it, and an entry for files
  stops at the directory that holds it, so nothing above this directory matters and neither does an entry for files.
- **A list that cannot be read** is refused, as nothing can be told of it. An owner can be denied reading a list, and that
  gives an error and not an empty list. Having no list, and a file system with no lists, are the same as an empty one.

Symbolic links hold no lists, and what one leads to is walked next. As with a looser mode, a refused directory is not repaired, as
something may already have read what is in it. `PrivateDir::check` finds all of this at startup, which is when a person hears of it,
with what is refused and why.

**An entry naming this very user counts like any other.** Telling them apart needs a directory service, which is more `unsafe` and a
wait in the crash path, for the same reasons [ADR 0062](0062-emergency-save-and-the-panic-hook.md) gives for groups. `chmod -a` is the
remedy, and the startup message says what to look at.

### Each copy

Each of a copy's two temporary files is **sealed** before anything goes in: any list it took from its directory is removed, and read
back as gone. If what it took granted anything, the directory has changed since it was checked, so the copy is refused, nothing is
left, and the report says so without quoting text. An inherited `deny` is removed and the copy goes on, since a file may not be renamed or
removed while it denies this user `delete`.

### The control socket

It follows the same rule, on its directory and its parent, as that directory also holds screenshots. It stays behind the `dev` feature.
As it has only the one parent to look at, both rules for a parent apply whether or not the directory exists yet.

### On other systems

- **Linux:** nothing new to check. The group bits of a mode are a list's mask, so what an access list grants shows in the mode, and the
  existing checks see it. A default list gives a new file or directory a mask no wider than the mode it was asked for, so with `0600` and
  `0700` it grants nothing. A test on Linux keeps both true.
- **Windows** stays as ADR 0062 says, and is left for later, recorded in `open-questions.md`. It needs `windows-sys` as a direct dependency.
- A file system with no lists has nothing to check.

### Where it lives

- `oso_platform::file_meta` is a new public module, in a layer of its own below `privacy` in `guardrails/layers.txt`, as ADR 0062
  expected of the next code to need this. Its public API is `seal` and `Sealed`, with no OS type in them.
- `file_meta::macos` declares the `acl_*` calls itself, with `unsafe extern "C"`, as `libc` does not have them. No dependency is added.
- The rule is three pure functions in `privacy`, over entries that say only what the rule asks. They are tested on every system.

## Consequences

- A directory with an `allow` entry gets no copies, even an entry for this user. The person hears at startup, and can remove it.
- Finding this costs one more call per directory walked, on macOS only, and sealing one or two per file. All of it is off the UI thread.
- `write_copy` takes the seal as a parameter inside, so that a test can see it runs before any text, on any system.
- Tests set real lists on what they made themselves, naming `everyone` by its fixed identifier, so that they need no privileges and no
  directory service. The test helper sets a list by `acl_set_file`, as `acl_set_link_np` fails on a list its owner may not read.
- `acl_from_text` knows only the names of rights for files. They are the same bits as those for directories.
- How a save treats lists stays as [ADR 0059](0059-safe-save-and-the-exit-guard.md) says.

## Alternatives considered

- **Refusing any list:** simple, but it refuses every home directory on macOS.
- **Checking the mode alone:** does not say who may reach what is in the directory on a system with lists.
- **Removing what is found:** a directory found open may already have been read, as ADR 0062 says of a looser mode.
- **Looking up who an entry names:** see above.
- **Reading the list on Linux too:** it would say nothing the mode does not, and refusing a default list would refuse what grants nothing.
- **Checking only the directory and not each copy:** a list added since the check would reach the text. Sealing costs a call, and only then
  does the text go in.
- **Parsing `acl_copy_ext_native`'s bytes, or `acl_to_text`:** no need to read a format the system does not promise, and the text form asks a
  directory service for names.
- **The `exacl` crate:** a dependency for a few calls, with the rule still ours to write.
