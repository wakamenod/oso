# 0110. ADR 0108 as built

- Status: Proposed
- Date: 2026-11-11

## Context

[ADR 0108](0108-the-user-configuration-file.md) designed the user configuration file before any
of it was built. It lands in steps, one pull request each. Building them shows where its text
does not say enough, or cannot be built as written. This ADR collects those points as the steps
land, and is accepted with the last one.

## Decision

The author chose D1 and D2 on 2026-11-11, through the coordinator, and the 256 KiB file limit
the same day.

### Step 1: the user's key layer

- **A prefix made where a key was unbound hides the defaults under it,** as Emacs's fresh prefix
  map does. `"C-x" = false` with `"C-x C-k" = "…"` leaves `C-x C-k` the only key under `C-x`.
  Unbinding a prefix that the user's keys already made does the same, so the order of the
  entries, which TOML does not keep, changes nothing. It holds at every level: a prefix made under
  one that hides hides too, so `C-x 4 f` stays hidden beside a user's `C-x 4 o`.
- **An undefined sequence is still taken without Shift,** as Emacs translates a key bound to
  nil ([ADR 0071](0071-mark-and-region.md)). `C-x C-K` runs a user's `C-x C-k` under such a
  prefix. So `"C-F" = false` changes nothing while `C-f` is bound; step 2 warns of such an entry.
- **In a passphrase prompt, every key that types a character is shown as `<char>`:** in the keys
  in progress (`<char>-`) and in "… is undefined". Beside a buffer that may hold secrets, the echo keeps
  the keys, and goes with the buffer, as it does today.
- **The log masks every key that types a character,** not only the last one, as typed text stays
  out of it ([ADR 0028](0028-secrets-handling.md)).
- **`C-g` is never the user's,** whatever the layer holds, Shift or not: under a prefix it
  cancels, and on its own it runs what the defaults bind. `M-x` shows no user binding of it. The
  dispatcher keeps this, so it holds whatever fills the layer.
- **`M-x` shows no default key that a user command on a shorter key cuts short,** as the resolver
  reads keys one at a time, and a command whose default key the user's keys shadow is listed with
  no key.

### D1: a command over a default prefix

- **A user command on a key that is a default prefix needs nothing,** as a plain rebinding does.
  `*Messages*` warns how many default keys it hides. TOML cannot hold `"C-x" = false` beside
  `"C-x" = "…"`, so ADR 0108's rule could not be written for this case.
- **A user prefix over a default command still needs that key unbound in the same table:** `C-a x`
  while `C-a` runs a command needs `"C-a" = false`. Otherwise the entry is refused with a message.
  That is the direction in which a key breaks by accident.

### D2: text from an input method

- **Text an input method commits does not consult the user's keys.** With `"a" = false`, the key
  `a` is undefined, but a commit of "a" still goes in, as in Emacs. A commit is not a key.

### Step 2: parsing the file

- **The dependency:** `toml` 1.1.8, as ADR 0108 says, at the newest release at least two weeks
  old; `serde_spanned` and `toml_datetime` are held at 1.1.2 for the same reason. `toml_parser`
  goes from 1.1.2 to 1.1.5 and `toml_datetime` from 1.1.1 to 1.1.2 for the Android-only proc
  macro too, which shares them. `toml_writer` joins the lock as `toml`'s optional `display`
  dependency and is never built.
- **`config::parse(text, registry, defaults, themes)` is a pure function:** the file's text, the
  registry and default keymap the keys are checked against, and the built-in themes' names, in;
  the user's keymap, the settings and the problems out. It does no I/O and never panics. The
  themes' names come in so that an unknown one is reported with its line, as every problem is.
- **The file is at most 256 KiB,** not 1 MiB. The parser's tokens cost a few dozen bytes for
  each byte of a crafted file: 100 MB at the peak for 1 MiB of syntax errors, about 30 MB for
  256 KiB. A configuration file of 256 KiB is far larger than any written by hand. Step 3's reader
  checks it, before `parse`.
- **A syntax error rejects the whole file,** and every syntax error is reported, for a file of
  up to 64 KiB. A larger one reports its first, and says so: the parser keeps every error it
  finds, a few hundred bytes each, and a crafted file can hold one every other byte; 1 MiB of
  them took 337 MB to report. A key given twice in one table is a syntax error, as TOML says. Nothing in
  a rejected file applies: no keys and no settings.
- **Deep tables need a known stack, not an unbounded one.** `toml`'s default limits refuse a key
  of more than 80 parts, dotted keys and table headers too, and inline tables and arrays nested
  past 80. But a dotted key in each nested inline table multiplies the two: a file of a few KiB
  nests some 6,500 tables, and dropping nested tables recurses. Such a file overflowed the 1 MiB
  stack of a test thread on Windows, and needed 8 MiB in a debug build. So `parse` drops what it
  holds a level at a time. What the parser drops itself, when it rejects a file of any size
  (its first pass runs whatever the size), needs up to 2 MiB in debug and release builds. `config::STACK`, 8 MiB, is four times that:
  step 3's thread is spawned with it, and the test of the deepest file runs on it.
- **A theme pair is two different built-in themes,** and a name that is not one is reported, and
  that setting left unset.
- **At most 10,000 entries in `[keys.global]`, and 16 keys in a sequence.** A table with more
  entries binds none of them, with one message, rather than an arbitrary part of them. A longer
  sequence is skipped. With the 256 KiB file limit, this bounds the time and memory of a build
  and how deep prefix keymaps nest.
- **A load keeps the first 20 problems in the order of the file,** as many as `*Messages*`
  shows, and counts the rest. A message is written only if it is kept.
- **A problem is `line:column: message`,** the column counted in characters. Messages are
  oso's words: a `BindError` or a key-notation error is never shown as such. The file's text in
  one is quoted, cut at 80 characters and escaped as the display escapes control and invisible
  characters, bidi controls among them, and `"` and `\` in it are escaped with a backslash, so it
  cannot close its own quotes. The parser's wording of a syntax error is escaped too,
  and cut at 200. One pass over the text places every problem, so many problems on one long
  line cost no more than the line.
- **Budget:** a 64 KiB file of 3121 bindings parses and builds in 1.2 ms in a release build,
  within ADR 0108's 2 ms; toml's own parse is about 0.45 ms of it. A test that is run by hand
  in release checks it. Measured beside it: a mixed 64 KiB file of short and long sequences,
  unbindings and problems in 0.33 ms. At 256 KiB: unknown names in 4.4 ms; syntax errors in
  5.3 ms and about 30 MB at the peak, nearly all of it the parser's tokens. The largest keymap
  that fits, 5,824 sequences of 16 keys that part early, builds in 4.5 ms and takes 2.1 ms to
  drop, so step 3 must drop a replaced keymap off the UI thread.

### Step 3: reading and watching the file

The author chose the shape of `others_can_write`, `Editor::note` and leaving dev restarts as they
are on 2026-11-11, through the coordinator.

- **`oso-platform::privacy` gains `config_home()`,** which replaces the tip crate's own from
  step 5 with the same rules, **and `others_can_write(&Metadata) -> Writers`:** `Anyone` if every
  user may write it or another user than this one and root owns it, `Group` if its group may,
  else `Nobody`. Windows has no modes, and says `Nobody`, as `is_this_users` says yes there.
- **The thread, `oso-config`, starts first thing in `main`,** as the theme's read does, with
  `config::STACK`, and is given the event loop's waker once there is one. A read done by the first
  frame is applied before it; a later one arrives as an event.
- **The way to the file is judged before it is opened, and the file opened must be the one
  judged.** `privacy::who_can_change`, new in `oso-platform`, walks the path from the root as
  the system does: every directory, as written and through every link, every link's owner, and
  the file. Each entry is judged with the directory holding it; in a sticky directory every user
  may write, an entry this user or root owns counts as no one else's. `Anyone` anywhere refuses
  the file. Then it must be a regular file of at most 256 KiB, opened with
  `file_meta::open_for_reading`, the same file by device and inode, of UTF-8. `Group` reads it
  with a warning, counted as a problem; on macOS a group such as `staff` can hold every account,
  and the warning says so. Any other failure leaves what was in force, and the echo area says
  why ("Configuration not read: other users can change it"). A panic anywhere on the thread is
  caught and said the same way.
- **It is read again when its time, size, mode, device or inode changes,** or it comes or goes,
  as ADR 0083 compares files: looked at once a second, when oso gains the focus, and after any
  save oso makes, so that a save of the file applies at once without telling which file it was.
  A look asked for reads a refused file again whatever its stamp, as fixing its directory's mode
  changes nothing in it. What a look finds is said only when it changes: a refusal again only
  for another reason or a changed file, and a file read again only for other text, so that a
  focus or a save says nothing new. A file that goes leaves the defaults, with a log line. The thread wakes
  once a second for itself, beside the loader's watcher; the UI thread is woken only for news.
- **Each problem is a line of `*Messages*`, `config.toml:3:11: …`, and the echo area says one
  line.** For a file whose name, or where it leads, matches the secret patterns
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)), a problem gives its line and
  column and none of its text.
- **The echo line** is "Configuration: 2 problems, see *Messages*", or "Configuration not
  loaded: …" for a syntax error. `Editor::note`, new in `oso-core`, writes `*Messages*` and the log without the
  echo area; from a command around a buffer that may hold secrets it goes nowhere. The first good
  load after a bad one says "Configuration loaded"; any other only logs. `config::outcome` decides
  all of it, so the app only carries it out. Writing 22 notes and the line into a full
  `*Messages*` took 0.4 ms in a release build, once a load with that many problems.
- **The keymap a reload replaces is dropped on the `oso-free` thread:** `set_user_keymap` returns
  it, and one that a keymap waiting for a prefix key replaces later is taken with
  `take_replaced_keymap` before the event loop waits. Swapping in a 64 KiB file's keys measured
  under a microsecond in a release build, within ADR 0108's 0.1 ms.
- **`--no-config` works in every build, anywhere among the options; `--config FILE`, in a dev
  build, comes first after `--emergency-dir` and `--clipboard`,** as they do, and is made
  absolute. The end-to-end flows pass `--no-config`, or `--config` for the configuration's own.
  `just perf` gives each instance a configuration directory of its own through
  `XDG_CONFIG_HOME` (`APPDATA` on Windows), mode 0700: empty, as for a person with no file, or holding the file `OSO_PERF_CONFIG`
  names. Not an option, so that `perf-remote`'s base, built from before, runs the same.
- **The agent channel never saves into the person's configuration directory, nor the one
  holding the `--config` file:** `config_home()` counts under `--config` and `--no-config` too,
  as the person's next start reads it.
- **A restart under `oso dev` passes `--no-config` on if the first instance had it, and no other
  option,** so a new instance reads the person's file even if the first was given `--config`.
  ADR 0108 says a new instance reads the file itself. `oso dev`'s instances get
  `XDG_CONFIG_HOME` and `APPDATA` from its environment, so they read the file the person's
  shell names.
- **Where the file is comes from the environment oso was started with.** An oso started from
  the Dock or Finder does not see an `XDG_CONFIG_HOME` that only a shell sets, so it reads, and
  keeps the agent channel out of, `~/.config/oso`.
- **The settings are parsed and not yet applied;** step 4 applies them.

## Changes to accepted ADRs

**[ADR 0108](0108-the-user-configuration-file.md)** (The user configuration file):

- "at most 1 MiB of UTF-8" becomes: at most 256 KiB of UTF-8.
- "A save of the file from oso wakes it at once" holds, as any save oso makes wakes it.
- "What it reaches must be a regular file of this user or root, not writable by every user, in a
  directory not writable by every user" holds, and also: a directory another user owns counts as
  writable by every user.
- "Shadowing a default prefix with a command, or a default command with a prefix (binding `C-a x`
  while `C-a` runs a command) needs that key unbound in the same table too" becomes: shadowing a
  default command with a prefix needs that key unbound in the same table; shadowing a default
  prefix with a command needs nothing, and `*Messages*` warns how many default keys it hides.
- "`Keymap` gains an undefined entry (`Keymap::unbind`): it stops the lookup" holds, and also: an
  undefined sequence is still taken without Shift, and a prefix made under an undefined key hides
  the defaults under it.
- "`where-is`, `M-x`'s binding annotations and the keymaps `M-x` judges commands by read the user
  layer over the global one, so a hidden default is never shown" holds, and also: a default key
  that a shorter user command cuts short counts as hidden.
- "A sequence a person cannot type, an unknown command, or a command only a dev build has, is
  refused with its line" holds, and also: a sequence that starts with a key that types a character,
  an entry with `C-g` in its keys, a sequence of more than 16 keys and `true` are refused, and a
  `[keys.global]` of more than 10,000 entries binds none of them.
- "A syntax error rejects the whole file; … `toml`'s recoverable parse gives every syntax error
  with its span" holds for a file of up to 64 KiB; a larger one reports its first syntax error.
- "`C-g` is not configurable in any keymap" holds, and also: an entry with it is refused with a
  message, rather than ignored.
- "A command only a dev build has, is refused" holds in a release build, where it is unknown; a
  dev build binds it.
- "`minibuffer`, `completion`, `isearch` and the major modes' names are reserved, and an entry
  under one says it is not configurable yet" holds, and also: so does every other name under
  `[keys]` but `global`, since modes' names are not known yet. Unknown names elsewhere still warn
  and are skipped.

## Consequences

- A user can make `C-x` a command with one line. The warning says what it hides.
- A shifted key under a rebuilt prefix behaves as in Emacs, so the user writes no Shift entries.
- Unbinding a letter does not stop an input method typing it.
- A user cannot make a letter a prefix of their own, as some Emacs setups do with modal keys.
  `C-c` and the function keys stay free for that.
- A file written for a newer oso loads, with a message for each table this one does not know.
- Later steps add their points here.

## Alternatives considered

- **Editing ADR 0108's text:** the author keeps accepted ADRs as they were.
- **D1, refusing a command over a default prefix in the first version:** simpler to explain, but a
  common Emacs habit could not be written.
- **D2, IME text consulting the user's layer:** consistent with keys, at a lookup per character,
  for something few would do.
- **An undefined key that blocks Shift:** `"C-F" = false` would also stop `C-S-f` running `C-f`,
  which Emacs's nil does not.
- **Super keys counted as typing keys:** they would be refused too, but they type nothing in oso,
  and Emacs users bind them.
- **Ignoring `C-g` entries without a word:** the person would wonder why a binding does nothing.
- **Over the entry limit, binding the first 10,000:** TOML keeps no order, so which ones apply
  would depend on how their keys sort.
