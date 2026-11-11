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

## Consequences

- A user can make `C-x` a command with one line. The warning says what it hides.
- A shifted key under a rebuilt prefix behaves as in Emacs, so the user writes no Shift entries.
- Unbinding a letter does not stop an input method typing it.
- Later steps add their points here.

## Alternatives considered

- **Editing ADR 0108's text:** the author keeps accepted ADRs as they were.
- **D1, refusing a command over a default prefix in the first version:** simpler to explain, but a
  common Emacs habit could not be written.
- **D2, IME text consulting the user's layer:** consistent with keys, at a lookup per character,
  for something few would do.
- **An undefined key that blocks Shift:** `"C-F" = false` would also stop `C-S-f` running `C-f`,
  which Emacs's nil does not.
