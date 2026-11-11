# 0110. ADR 0108 as built

- Status: Proposed
- Date: 2026-11-11

## Context

[ADR 0108](0108-the-user-configuration-file.md) designed the user configuration file before any
of it was built. It lands in steps, one pull request each. Building them shows where its text
does not say enough, or cannot be built as written. This ADR collects those points as the steps
land, and is accepted with the last one.

## Decision

The author chose D1 and D2 on 2026-11-11, through the coordinator.

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

- **`config::parse(text, registry, defaults, themes)` is a pure function:** the file's text, the
  registry and default keymap the keys are checked against, and the built-in themes' names, in;
  the user's keymap, the settings and the problems out. It does no I/O and never panics. The
  themes' names come in so that an unknown one is reported with its line, as every problem is.
- **A syntax error rejects the whole file,** and every syntax error is reported. A key given
  twice in one table is one, as TOML says. Nothing in a rejected file applies: no keys and no
  settings.
- **A binding cannot start with a key that types a character:** a character or `SPC` with no
  Control, Meta or Super, Shift or not. `"a" = "cmd"` and `"a b" = "cmd"` are refused, with a
  reason that suggests `C-c a`. `"a" = false` is allowed. This is what `KeyPress::text` calls a
  key that types ([ADR 0056](0056-typing.md)): with Super it types nothing (Option-a is `s-a` on
  macOS, [ADR 0033](0033-key-presses-and-notation.md)), so `"s-a" = "cmd"` binds.
- **An entry with `C-g` anywhere in its keys is refused,** with Shift or not (`C-G`), whether it
  binds or unbinds. The dispatcher would ignore it (step 1); the message says why it does
  nothing.
- **`"C-F" = false` while `C-f` is bound warns** that it changes nothing, since an undefined
  key is taken without Shift (step 1). The check reads the user's keys over the defaults, so it
  says nothing when the file unbinds `C-f` too.
- **D1's warning counts the default keys bound to commands under the prefix,** at every level:
  `"C-x" = "cmd"` says "a prefix by default; this hides the N default keys under it". It says
  nothing when the file unbinds that key or a shorter one of it, which hides them anyway.
- **Unbinding comes before binding,** and within each, shorter sequences before longer ones,
  then the spelling in text order. So of two entries that conflict, the longer one is skipped
  (`"C-c x" = "cmd"` beside `"C-c x a" = "cmd"`), and of two spellings of the same keys
  (`"C-c y"`, `"C-c   y"`), the second in text order. The order of the file, which TOML does not
  keep, changes nothing.
- **`true` binds nothing:** an entry is a command's name or `false`, and anything else is skipped
  with its type named.
- **Every `[keys.<name>]` but `global` says "not configurable yet"**, as no mode's name is known
  yet. `[keys.question]` says it never is.
- **A theme pair is two different built-in themes,** and a name that is not one is reported, and
  that setting left unset.
- **At most 10,000 entries in `[keys.global]`, and 16 keys in a sequence.** A table with more
  entries binds none of them, with one message, rather than an arbitrary part of them. A longer
  sequence is skipped. With the 1 MiB file limit, this bounds the time and memory of a build
  and how deep prefix keymaps nest.
- **A problem is `line:column: message`,** the column counted in characters. Messages are
  oso's words: a `BindError` or a key-notation error is never shown as such. The file's text in
  one is quoted, cut at 80 characters and escaped as the display escapes control and invisible
  characters, bidi controls among them. The parser's wording of a syntax error is escaped too,
  and cut at 200. One pass over the text places every problem, so many problems on one long
  line cost no more than the line.
- **Budget:** a 64 KiB file of 3121 bindings parses and builds in 1.2 ms in a release build,
  within ADR 0108's 2 ms; toml's own parse is about 0.45 ms of it. A test that is run by hand
  in release checks it.

## Changes to accepted ADRs

**[ADR 0108](0108-the-user-configuration-file.md)** (The user configuration file):

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
  refused with its line" holds, and also: a binding that starts with a key that types a character,
  an entry with `C-g` in its keys, a sequence of more than 16 keys and `true` are refused, and a
  `[keys.global]` of more than 10,000 entries binds none of them.
- "`C-g` is not configurable in any keymap" holds, and also: an entry with it is refused with a
  message, rather than ignored.

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
