---
name: add-command
description: Add an editor command to oso. Write the function, register it with a docstring and argument spec, bind keys, write unit tests and check it end to end over the control socket. Use for any new or changed command or key binding.
---

# Add a command

Commands are plain functions over host-owned state, registered with a name,
a docstring and an argument spec (ADR 0034). They live in the tip crate,
which rebuilds fastest. Read `docs/dev/architecture.md` ("Command registry",
"Keymaps") first if you have not.

## 1. Write the function

In the feature's module under `crates/oso/src/commands/`, such as
`motion.rs`. A new feature gets a new module; its file opens with its
responsibility and a `Must not depend on:` paragraph (ADR 0041):

```rust
fn forward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count());
    Ok(())
}
```

- The signature is always `fn(&mut Ctx<'_>, Args) -> Result<(), CommandError>`.
  State comes from `ctx.editor`. No statics, globals or captured state.
- The command runs once and works on the whole selection set, not once per
  cursor (ADR 0007). Use the "for each cursor" helpers once they exist
  (M1-6).
- Call `ctx.request_redraw()` when something visible changed, and only
  then. Motion that cannot move, or a command that only reports, draws
  nothing.
- Ask the host for effects through `Ctx` (`request_exit()`), never by
  reaching for the window.
- Return `Err(CommandError::new("Beginning of buffer"))` for expected
  failures. A panic is caught at the command boundary (ADR 0014), but state
  the command already changed stays changed, so stage edits first.
- Never block: no I/O, parsing or process waits on the UI thread. Start
  background work and return.
- Name it after the Emacs command when there is one (`forward-char`), in
  lower-case kebab case.

## 2. Register it

Add an entry to the `COMMANDS` table of that module. A new module also adds
one line to `TABLES` in `crates/oso/src/commands/mod.rs`, in alphabetical order:

```rust
Command {
    name: "forward-char",
    doc: "Move point N characters forward (backward if N is negative).",
    args: ArgSpec::Count,
    reach: Reach::Look,
    run: forward_char,
},
```

- `doc`: the first line is a one-sentence summary; details follow after a
  blank line. A missing docstring is refused.
- `args`: `ArgSpec::None`, or `ArgSpec::Count` if the command reads
  `args.count()` (the numeric prefix argument, 1 by default, negative to
  reverse). If a new kind of argument is needed, extend `Args` and
  `ArgSpec` in `oso-core` and propose it in an ADR.
- `reach`: how far the agent channel may reach it (ADR 0085). `Reach::Look`
  if it moves, marks, shows or opens and changes no text or file;
  `Reach::Change` if it edits text or saves; `Reach::Person` if it needs a
  person at the keyboard: it exits, answers a prompt, reads the clipboard,
  needs a key or a passphrase, spawns a process, trusts a project, or shows
  what a buffer that may hold secrets withholds. When in doubt, `Person`. A
  `Person` command also goes in the list in
  `commands::tests::the_agent_channel_refuses_what_needs_a_person`.
- Duplicate names fail `commands::tests::every_command_registers`.

## 3. Bind keys

Add rows to the `BINDINGS` table of the module with the same name under
`crates/oso/src/bindings/`, in Emacs notation (ADR 0033). A new module also adds
one line to `TABLES` in `crates/oso/src/bindings/mod.rs`, in alphabetical order:

```rust
("C-f", "forward-char"),
("C-x C-c", "save-buffers-kill-terminal"),
```

- Follow vanilla Emacs and the author's customizations (ADR 0019,
  ADR 0030).
- Shift is part of a character: write `C-A`, not `C-S-a`, for letters.
- A prefix key cannot also be a command, and a sequence cannot be bound
  twice; `bindings::tests` fails if a row does not bind.
- `C-g` with a prefix pending always cancels it; do not bind under a
  prefix and expect `C-g` to reach it.

## 4. Unit tests

- In the command's module: run the command through the registry on an `Editor`
  and check the state and the `Effects` (redraw only when something
  changed). Cover the boundaries: start and end, a count of 0, negative
  counts, `isize::MIN`.
- In `dispatch.rs`: feed the key sequence to a `Dispatcher` and check the
  result, as `char_motion_through_the_keymap` does.
- Use `proptest` where positions or offsets are involved.
- If the command changes what is drawn, add a text-grid snapshot in
  `crates/oso/src/dispatch/grid_tests.rs`, run `just snapshots` and read the
  `git diff` (ADR 0037, ADR 0045; `docs/dev/testing.md`). Explain every
  snapshot change in the PR.
- Tests follow the test pyramid (ADR 0037): put each check in the lowest
  layer that can express it.
- Run `just ci`.

## 5. Check it in the app

Follow the `verify-in-app` skill: start a dev build, then

```sh
./target/debug/oso ctl send-keys "C-f C-f C-b"
./target/debug/oso ctl get-state     # check the state the command changed
./target/debug/oso ctl screenshot    # when only pixels show the change
```

Quote what you sent and what `get-state` returned in the PR description.
`run-command` (M1-7b) will let you call commands by name as well.

## Later

- Regenerate the command reference once it exists (open questions,
  "Generated command reference").
