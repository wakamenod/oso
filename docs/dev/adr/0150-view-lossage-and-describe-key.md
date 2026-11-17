# 0150. View lossage and describe key

- Status: Accepted
- Date: 2026-11-17

## Context

On macOS, `M-.` did nothing in the installed oso, and nothing in the app could show whether the
key had reached it. AppKit had taken it before oso saw it
([ADR 0149](0149-command-period-through-cancel-operation.md)). The author asked for a way to
check keys themselves, as Emacs's `view-lossage` (`C-h l`) and `describe-key` (`C-h k`) do.
Both keys are unbound in oso. [ADR 0034](0034-command-registry-and-keymaps.md) and the
architecture already name `describe-key` as something the registry drives.

What is fixed already:

- [ADR 0028](0028-secrets-handling.md): typed text stays out of the log, where a key that types a
  character shows as `<char>`.
- [ADR 0113](0113-keyboard-macros.md) and [ADR 0120](0120-terminals.md), Q17: input that reaches
  a buffer that may hold secrets or a password prompt drops a keyboard macro.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): every command has a reach. A buffer can be
  withheld from the agent channel without being secret, as `*Session explain*` is
  ([ADR 0121](0121-agent-sessions-and-spaces.md)).

The author chose what is hidden on 2026-11-17, through the coordinator.

## Decision

- **`view-lossage` (`C-h l`) shows the last 300 keys oso received** in the internal buffer
  `*Lossage*`, oldest first, with the cursor after the newest, as Emacs does. Each line is a key
  and, after `;;`, what it did: the first command it ran, `undefined`, or `sent to the terminal`
  when it went to a terminal's program. A prefix key, or a key a menu, a mode or a terminal kept
  without running a command, has nothing after it.
  - Every key the window or the control socket gives the dispatcher is kept, before anything
    takes it. A key held to run later, while a command waits for the clipboard or `!` or an
    undo runs, or while a keyboard macro replays, is kept as `<held>`, with no key, as where it
    will run is not known yet. It is kept again when it runs, judged where it runs. The
    replay's own keys are not kept, as in Emacs.
  - A key the window layer gives that oso has no name for shows as `<no Emacs name>`. So
    `*Lossage*` tells a key that came but cannot be bound from one that never came. A repeat of
    such a key is not kept.
  - Not kept: text from an input method or from a key that types several characters, which
    is no key; a key that answers the agent channel's question; and a repeat the app drops
    where a letter answers.
- **What is hidden:**
  - A key typed where secrets may be shows as `<hidden>`, with nothing of what it ran: in a
    buffer that may hold secrets, at a passphrase prompt, or wherever the echo area and the
    control socket withhold keys as around such a buffer, such as a prompt, an isearch, a
    query-replace or a jump over it. This holds for every key, named or not.
  - A character sent to a terminal's program shows as `<hidden>`, with `sent to the terminal`.
    It may be a password typed before the terminal said it reads one. Named keys and keys with
    Control or Meta show as they are (`RET`, `C-c`, `M-b`). A terminal reading a password is a
    buffer that may hold secrets, so every key there is hidden.
  - A run of hidden keys is one line, so it does not tell how many were typed, and so is a run
    of held keys.
  - The keys live in the dispatcher's memory only. They are never saved, logged, put in an
    emergency copy or carried over a restart.
- **`describe-key` (`C-h k`) reads one key sequence and runs nothing.**
  - It resolves the keys as a key that runs would. In a terminal, a key that would go to the
    program says `… is sent to the terminal's program`, or what the terminal does with it.
    Otherwise the keymaps active then resolve it, with the same rule for `C-g` as a key that
    runs. A prefix waits for the next key, and the echo area shows it in progress.
  - A command is described in the internal buffer `*Help*`: the keys, the command's name and
    its documentation. A key that runs a command only without Shift says so.
  - An unbound sequence says `… is undefined`, a menu's says `… opens a menu`, and `C-g` under a
    prefix says `Quit`, all in the echo area.
  - Nothing it reads reaches a terminal's program, and the mouse is held from one meanwhile. A
    character it says goes to the program shows as `<char>`, as the log has it.
  - The input method stays off while it reads, so a letter is a key and not a composition.
  - Where secrets may be, it reads no key: it does not start there, and a key that comes where
    they may be ends the read, and the echo area says so.
  - Text ends the read and goes on as it would. A restart carries none of the read over, so the
    next instance never runs it as a prefix.
- **Neither is the agent channel's.** Both commands are `Reach::Person`: one shows what the person
  typed, and the other needs a key. `*Lossage*` is withheld from the channel. `*Help*` holds
  only documentation and is not.
- **The dispatcher answers them.** The commands only check that no prompt is open. The
  dispatcher, which holds the keys and the keymaps, shows `*Lossage*` or starts reading after
  they succeed, as it answers `session-help` (ADR 0132). Nothing is added to `oso-core`.

## Consequences

- The person can see whether a key reached oso, what oso called it, and what it ran, without a
  dev build or a log.
- Every key costs a push onto a ring of 300 small entries.
- `*Lossage*` says nothing of keys typed where secrets may be, so a key problem in such a buffer
  cannot be checked there. The person checks the same key in an ordinary buffer.
- `*Help*` and `*Lossage*` take the selected window, as `*Messages*` does. The person leaves
  them with `C-x b`.

## Alternatives considered

- **Keeping keys in secret contexts and hiding them only when shown:** the keys would then be in
  memory where a dump or a future reader could reach them.
- **Hiding only keys at a password prompt in a terminal, as keyboard macros do:** a password
  typed before the terminal shows its prompt would be kept.
- **Showing what a hidden key ran:** a run of `self-insert-command` and `delete-backward-char`
  tells a secret's length.
- **Keeping the lossage in `oso-core`'s `Editor`:** it would add public items to a library crate
  for state that only the dispatcher fills and reads.
