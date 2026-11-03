# 0032. Key presses and Emacs key notation

- Status: Accepted
- Date: 2026-11-03

## Context

Keymaps with prefix keys arrive in M1-3, and the dev control socket's
`send-keys` takes keys in Emacs notation
([ADR 0023](0023-control-socket-protocol-and-restart.md)). Both need one key
type, independent of the window system, so that keys from the window and
from the socket take the same path. winit reports keys with the layout's
shift already applied, and on macOS Option composes characters.

## Decision

- `oso_core::key::KeyPress` is a key and its modifiers. Modifiers are
  Control, Meta, Shift and Super. A key is a character or a named key
  (`RET`, `TAB`, `ESC`, `DEL`, `<delete>`, `<insert>`, arrows, `<home>`,
  `<end>`, `<prior>`, `<next>`, `<f1>` to `<f35>`).
- **Notation** follows Emacs `kbd`: keys are separated by whitespace,
  modifier prefixes are `C-`, `M-`, `S-`, `s-` in any order, and are printed
  in that order. `SPC` is the space character. The aliases `<return>`,
  `<tab>`, `<escape>`, `<backspace>` and `<deletechar>` are accepted and
  printed in the short form.
- **Shift is part of a character.** Shift and `a` is `A`, and Shift and `1`
  is whatever the layout gives (`!`). The Shift modifier is kept only on
  named keys (`S-<right>`). When parsing, `S-` on an ASCII letter gives the
  upper-case letter (`C-S-a` is `C-A`), and on any other character it is an
  error.
- **Translation from winit** happens in the tip crate. On macOS, Command is
  Meta and Option is Super (architecture.md). Characters come from winit's
  logical key; with Option held on macOS, or when Control produced a control
  character, they come from the key without modifiers instead, so Option-a
  is `s-a`. Composed text of more than one character, dead keys and keys
  with no Emacs name are not key presses.
- Parsing and printing round-trip, which property tests check.

## Consequences

- Keymaps compare `KeyPress` values directly; `C-S-a` and `C-A` are the same
  binding.
- `S-SPC` and Shift on non-letter characters cannot be bound, since the
  shifted symbol depends on the layout. This can be revisited if a binding
  needs it.
- Option can no longer type composed characters such as `å`. IME and text
  input (M2) handle text separately from key presses.

## Alternatives considered

- **Keeping Shift as a modifier on characters:** bindings would depend on
  whether the layout reports `A` or Shift-`a`.
- **Using winit's key types directly:** ties `oso-core` and keymaps to
  winit and cannot be tested without it.
- **Emacs's distinction between `RET` and `<return>`:** only matters for
  terminals, which oso does not run in.
