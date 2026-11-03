# 0034. Command registry, keymaps and key resolution

- Status: Proposed
- Date: 2026-11-03

## Context

Every editing feature from M1-6 on is a command bound to keys, and `M-x`,
`describe-key`, the command reference and the control socket's
`run-command` all go through the registry. Architecture.md fixes the
outline: commands are plain functions over host-owned state,
`fn(&mut Ctx, Args) -> Result<()>`, run once per command across all
cursors (ADR 0007), keymaps are layered (keymap at point > minor modes >
major mode > global), and panics are caught at the command boundary
(ADR 0014). This ADR fixes the details that commands and keymaps are
written against. Today the editor state is only a cursor; buffers, windows
and selection sets come in M1-6.

## Decision

- **Commands.** `oso_core::command::Command` is a name, a docstring, an
  `ArgSpec` and a function
  `fn(&mut Ctx<'_>, Args) -> Result<(), CommandError>`. Names and docs are
  `&'static str`, and the tip crate lists its commands in a `const` table.
  Names are lower-case kebab case, and a command without a docstring is
  refused.
- **Context.** `Ctx` holds `&mut Editor`, the host-owned editor state, and
  the requests a command leaves for the host (`Effects`): redraw and exit.
  New state becomes new fields of `Editor` or new methods on `Ctx`, so the
  signature never changes.
- **Redraws are requested explicitly.** A command calls
  `ctx.request_redraw()` when it changed something visible, and only then.
  Prefix keys, unbound keys, `C-g` and motion that cannot move draw nothing.
- **Arguments.** `Args` is a small `Copy` value built by the caller (the key
  dispatcher, `M-x`, `run-command`). For now it holds an optional numeric
  prefix argument; `Args::count()` gives it or 1. `ArgSpec` is `None` or
  `Count` and documents what a command reads; `run-command` validates
  against it. A prefix argument given to an `ArgSpec::None` command is
  ignored, as in Emacs. `C-u` and `M-<digit>` will be commands that leave a
  prefix for the next command through `Effects`, held by the dispatcher next
  to the pending prefix keys.
- **Registry.** The host owns a `Registry`; there is no global one.
  Registering a name twice is an error, and the tip crate's tests build the
  full registry, so a duplicate cannot ship. Lookup by name serves
  `run-command` and `M-x`; iteration in registration order, with docs,
  serves completion and the command reference. `Registry::execute` runs a
  command under `catch_unwind`; a panic becomes an error that the host logs
  (the echo area, from M2), and the editor keeps running. The error names
  the command but drops the panic message, which may quote buffer text
  ([ADR 0028](0028-secrets-handling.md)).
- **Keymaps.** A `Keymap` maps a `KeyPress` to `Binding::Command(CommandId)`
  or `Binding::Prefix(Keymap)`. Bindings store command ids, resolved from
  names when the keymap is built from a table such as
  `("C-x C-c", "save-buffers-kill-terminal")` in Emacs notation (ADR 0033).
  Building fails on bad notation, unknown commands, a sequence bound twice,
  and prefix conflicts (binding under a key bound to a command, or binding
  a key that is a prefix).
- **Layers.** `Layers` holds the active keymaps: at point, minor modes,
  major mode, global. The whole pending sequence is looked up in each, from
  the highest priority down, and the first keymap that binds it, to a
  command or a prefix, decides. This is Emacs's rule: a higher layer can
  shadow a global prefix with a command and the other way round, and a
  layer that does not bind the sequence is skipped.
- **Resolver.** `KeyResolver` keeps the pending keys in a reused buffer and
  looks the whole sequence up from the top of every layer on each key. It
  holds no references into keymaps, so layers may change between keys. A
  key gives `Command(id)`, `Pending`, `Unbound(keys)` or `Cancelled`. An
  unbound sequence is reported ("C-x C-z is undefined") and clears the
  prefix.
- **`C-g`.** With a prefix pending, `C-g` always cancels it, whatever the
  keymaps bind under the prefix, as in Emacs. With nothing pending it is an
  ordinary key, bound to `keyboard-quit`.

## Consequences

- Commands are testable as plain functions over an `Editor`, and key
  sequences as plain functions of a `Dispatcher` and an `Editor`, with no
  window.
- A key press costs a few hash lookups per layer and no allocation. Typos in
  binding tables and duplicate command names fail tests, not at runtime.
- A forgotten `request_redraw()` leaves a stale frame. Reviews check it, and
  it can be replaced by revision counters once buffers and selections have
  them.
- `Keymap::bind` refuses to rebind. User bindings from TOML (ADR 0019) need
  either a user layer above the global one or explicit rebinding; this is
  decided with configuration loading.
- State that a panicking command touched may be partially updated, as
  ADR 0014 accepts. Commands that edit should stage changes first.
- Rust's default panic hook still prints the panic message to stderr, even
  for panics caught at the boundary. The panic hook that comes with
  emergency save ([ADR 0013](0013-persistence.md)) has to keep buffer text
  out of it.
- Keys from the control socket's `send-keys` go through the same keymaps
  and resolver as keys from the window, so a same-user client can run any
  bound command, and can leave a prefix pending for the user's next key.
  Same-user trust ([ADR 0016](0016-dev-control-socket.md)) accepts this;
  it matters once commands spawn processes or write files.

## Alternatives considered

- **A registry generic over the host's context type:** more flexible, but
  every user of the registry, from `M-x` to the control socket, would carry
  the type parameter, and commands live in one place anyway.
- **Commands as trait objects or closures:** could capture state, which the
  hard rules forbid, and are harder to list in a `const` table.
- **Keymaps storing command names:** keymaps would not need a registry, but
  each key press would look a name up, and unknown names would show up only
  when the key is pressed.
- **A resolver that keeps a reference to the current prefix keymap:** saves
  the walk from the top, but ties the resolver's lifetime to the keymaps and
  breaks when layers change while a prefix is pending.
- **Looking `C-g` up in the prefix keymap:** a mode could make `C-g`
  undefined under a prefix and trap the user.
- **Redraw on every command, or by comparing state before and after:** the
  first draws when nothing changed; the second does not scale to buffers.
- **Arguments as a list of dynamic values now:** more general, but nothing
  needs it yet; `Args` can grow fields as argument kinds arrive.
