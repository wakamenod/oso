# 0132. A session mode for hosted sessions

- Status: Proposed
- Date: 2026-11-14

## Context

A hosted session ([ADR 0121](0121-agent-sessions-and-spaces.md)) is a `claude` in one of oso's
terminals ([ADR 0120](0120-the-terminal.md)), in a buffer `*claude: NAME*`. Its keys today are a
terminal's:

- `terminal::route` sends every key to the program before a keymap is looked at, except what
  semi-char mode keeps: `C-x`, `M-x`, the `C-c` prefix, `C-q` and `C-y`
  (`crates/oso/src/terminal/route.rs`).
- After `C-c`, only the terminal's own keymap is read: `C-c C-c` sends `C-c`, `C-c C-k` is char
  mode, and any other key goes to the program with the `C-c` before it
  (`dispatch/terminal.rs`). The global and the user's `C-c` keys never apply in a terminal.
- Session commands sit on the global prefix `C-x a` (ADR 0121, Q7), which works inside a terminal
  because semi-char mode keeps `C-x`. Q7's option (b), `C-c c` as the author's previous setup had
  it, was rejected because a terminal sends `C-c c` to its program.

On 2026-11-14 the author asked for more. A hosted session's buffer is never a general shell, so it
can have a mode of its own: session commands on `C-c` and a key inside a session, shorter than
`C-x a`, and not fighting the terminal's keys. `C-x a` stays the prefix in every other buffer, and
every key Claude Code itself needs must still reach it.

Two more facts shape the decision:

- **A session's buffer has no project.** A terminal has no file, so ADR 0114's project table does
  not point it into one (ADR 0120, "Project terminals"). The commands that use the current
  buffer's project, `C-x a w` `session-worktree`, `worktree-list`, the review menu `C-x v r` and
  `review-session`, refuse from a session's buffer with `Not in a project`; the person has to
  select a file of the project first. `*Sessions*` has none either.
- **Not every planned key is built.** ADR 0120 also gives semi-char mode `C-s` (the live search),
  `C-k` (copy, then send), `M-p` and `M-n` (send `C-p` and `C-n`) and `` C-` `` (the floating
  terminal). None of them is in `route.rs` yet; this ADR says what they do in a session before they
  are built.

### Claude Code's keys, and where each goes in a session today

From Claude Code's documentation, read on 2026-11-14 ("Interactive mode" and "Keybindings"; the
pages name features up to 2.1.314). "Sent" means semi-char mode sends the key as it is.

| Claude Code's key | What it does | In a session today |
|---|---|---|
| Ctrl-C | interrupt; on an empty prompt clear, twice exit (not rebindable) | `C-c C-c` sends one |
| Ctrl-D | exit after a second press; delete forward with text | sent |
| Esc, Esc Esc | cancel, close a dialog; clear the draft, or the rewind menu | sent |
| Shift-Tab (Alt-M on some Windows) | cycle the permission mode | sent |
| Enter, Tab, arrows, PageUp/Down, Home/End, Space, letters | submit, complete, dialogs, lists | sent |
| Ctrl-O, Ctrl-T, Ctrl-R, Ctrl-B, Ctrl-L, Ctrl-G | transcript, to-dos, history search, background, redraw, external editor | sent |
| Ctrl-V | paste an image, which Claude Code reads from the clipboard itself | sent |
| Ctrl-S | stash the prompt, or restore it | sent; ADR 0120 plans it as oso's live search |
| Ctrl-Z | suspend to the shell, `fg` resumes (Unix) | sent; see below |
| Ctrl-_ | undo in the prompt | sent |
| Ctrl-J, Option-Enter, `\` Enter | a new line | sent (`M-RET` is `ESC CR`) |
| Shift-Enter, Ctrl-Enter | a new line; send now | sent; reach Claude Code as such only with the kitty keyboard flags pushed |
| Alt-P, Alt-T, Alt-O | model picker, thinking, fast mode | sent; ADR 0120 plans `M-p` to send `C-p` |
| Ctrl-A, Ctrl-E, Ctrl-K, Ctrl-U, Ctrl-W, Alt-B, Alt-F, Alt-D, Option-Delete | readline editing | sent (`C-k` will also copy into oso's kill ring, ADR 0120) |
| Ctrl-P, Ctrl-N | history | sent |
| Ctrl-Y, then Alt-Y | yank Claude Code's own kill, then cycle | **oso's** `terminal-yank`; `C-q C-y` sends it |
| Ctrl-X chords: `C-x C-k` stop subagents, `C-x C-e` editor, `C-x RET` queue, `C-x C-s` send now, `C-x C-a`, `C-x TAB`, `C-x C-b`, `C-x b`, `C-x` arrows, `C-x x` | | **oso's** `C-x`; `C-q C-x` and the key send it |
| `/`, `!`, `@`, `:`, `?` at the prompt | commands, shell mode, files and sessions, emoji, help | sent |
| Cmd-K, Cmd-C | clear, copy | oso's (Super); Claude Code sees Cmd only through the kitty protocol |
| Ctrl-H, Ctrl-M, Ctrl-I, Ctrl-[ | backspace, Enter, Tab, Esc | sent |

So today a session can reach every key Claude Code uses, two of them only through `C-q` (`C-y`,
the `C-x` chords). Four things are worth fixing or checking:

- **`C-z` suspends Claude Code with nothing to resume it.** A session runs as
  `$SHELL -l -c 'exec claude …'`, so no shell is left to type `fg` in, and oso's reaper does not
  watch for a stopped child. The terminal then looks frozen until the session is stopped. Not yet
  seen in the running app.
- **ADR 0120's planned `C-s` and `M-p` would take Claude Code's stash and model picker** in a
  session.
- **`C-u` goes to Claude Code**, so no prefix argument can be typed in a terminal, and the `C-u`
  forms of session commands (`C-u C-x a m` sends the place only, `C-u C-x a c` asks a name) cannot
  be given from a session.
- **Shift-Enter and Ctrl-Enter** reach Claude Code only if it pushes the kitty keyboard flags in
  oso's terminal, which has not been checked. Option-Enter, Ctrl-J and `\` Enter make a new line
  either way, and `C-q C-x C-s` sends now.

## Decision

### The mode

- **Session mode is a minor mode of a terminal's buffer that hosts a session.** Its keymap is a
  minor-mode layer above the terminal's keymap (the major mode's), so the order stays ADR 0034's:
  minor modes, then the major mode, then the user's keys and the global ones.
- **It is on from the moment the buffer belongs to a session,** whichever command made it: `claude`,
  `session-worktree`, `session-adopt`, `session-fork`, `session-resume`, and the stopped buffers a
  start of oso brings back. A terminal where the person typed `claude` by hand is not a session and
  has no session mode.
- **It stays on while the buffer lives,** also after `claude` exits: the buffer then shows the last
  screen, and `session-resume` there is what the person wants next. It goes with the buffer, when
  `C-x k` archives the session.
- **Char mode turns its keys off.** Char mode sends every key but `C-c C-k`, in a session too; `C-c`
  and a session key there go to Claude Code, as any other `C-c` key does in char mode.
- **The buffer knows its session.** The host marks the buffer when it makes or adopts it
  (a flag on `oso_core::buffer::Buffer`, set by the tip crate's hosting code), and
  `Keymaps::active` puts the session keymap, and the user's session keys, in the minor-mode slot for
  a buffer so marked. `route_to_terminal` reads the same layers after `C-c`, in place of the
  terminal's keymap alone. No new crate, crate edge, public trait or module.

### The keys (Q1)

One rule: **in a session, `C-c` and a key runs what `C-x a` and the same key runs elsewhere.**
`C-c` and a control key stays the terminal's. Commands on the session itself, which have no `C-x a`
key, take capitals and punctuation.

| Key in a session | Command | Elsewhere |
|---|---|---|
| `C-c c` | `claude`, in this session's project | `C-x a c` |
| `C-c r` | `session-resume` | `C-x a r` |
| `C-c a` | `session-adopt` | `C-x a a` |
| `C-c w` | `session-worktree`, in this session's project (later ADR 0126's `worktree-create`) | `C-x a w` |
| `C-c s` | `sessions`, the pane | `C-x a s` |
| `C-c b` | `session-switch` | `C-x a b` |
| `C-c n` | `session-next-waiting` | `C-x a n` |
| `C-c d` | `session-next-done` | `C-x a d` |
| `C-c j` | `space-switch` (later ADR 0126's `jump-anywhere`) | `C-x a j` |
| `C-c z` | `space-zoom` | `C-x a z` |
| `C-c e` | `session-explain` | `C-x a e` |
| `C-c m` | `session-send` | `C-x a m` |
| `C-c @` | `session-mention`, pasted into this session | `C-x a @` |
| `C-c f` | `session-follow-message` | `C-x a f` |
| `C-c o` | the repository's overview (Q9) | `C-x a o` |
| `C-c k` | ADR 0126's `worktree-cleanup`, once built | `C-x a k` |
| `C-c K` | `session-stop`, this session (Q4) | `k` in `*Sessions*` |
| `C-c P` | `session-pin`, this session (Q4) | `P` in `*Sessions*` |
| `C-c A` | `session-archive`, this session (Q4) | `A` in `*Sessions*` |
| `C-c v` | `review-session`, this session's project since its baseline | `M-x` |
| `C-c V` | the review menu, `review` | `C-x v r` |
| `C-c +` | `terminal-allow-agent-read` | `M-x` |
| `C-c -` | `terminal-deny-agent-read` | `M-x` |
| `C-c C-u` | `universal-argument`, so that `C-c C-u C-c m` is `C-u C-x a m` | `C-u` |
| `C-c ?` | the session mode's keys, as built, user keys included | — |

The terminal keeps its own: `C-c C-c` sends `C-c`, `C-c C-k` char mode, and, when ADR 0120's step 6
builds them, `C-c C-j` copy mode and `C-c RET` the link at point.

- **`C-c ?`** shows the keys in the help float ADR 0131 gives the pane's `?`, built from the
  active keymaps, so a rebinding shows. oso has no `describe-mode` yet; when it comes, it lists
  these too.
- **`C-c C-u`** sets the prefix argument as `C-u` does elsewhere. While it waits, every key is
  oso's (`oso_busy`), so the next `C-c m` reads it. It is the session mode's only, as a plain
  terminal has no command that reads one.

### An unbound `C-c` key (Q2)

- **In a session, `C-c` and a key no layer binds is undefined:** the echo area says
  `C-c g is undefined (C-c C-c sends C-c)`, and nothing goes to Claude Code. A `C-c` sent there
  interrupts a working turn or clears the draft, so a mistyped session key must not send one.
- `C-g` after `C-c` cancels, as anywhere (ADR 0034).
- A plain terminal keeps ADR 0120's rule: both keys go to the program.

### What Claude Code gets (Q6, Q7)

- **Everything in the table above still reaches it,** as it does now; session mode takes only keys
  after `C-c`, and `C-c` itself was oso's already.
- **`C-s`, `M-p` and `M-n` go to Claude Code as they are in a session (Q6):** Ctrl-S stashes the
  prompt and Alt-P opens the model picker. When ADR 0120 builds the live search, a session reaches it
  with `C-c C-s`. `C-k` copies into oso's kill ring and sends, as in any terminal: Claude Code still
  gets its Ctrl-K. `` C-` `` and `C-~` stay oso's, as Claude Code does not use them.
- **`C-z` is held back in a session (Q7):** the echo area says
  `C-z would suspend Claude Code, and nothing here resumes it; C-q C-z sends it`. `C-q C-z` sends it.
  The build checks first, in the running app, that Claude Code really stops; if it does not, `C-z`
  is sent as now and this rule is dropped.
- **`C-y` stays oso's paste,** and `C-q C-y` Claude Code's own yank, as in any terminal. **`C-x`
  stays oso's** (the author's choice), and `C-q C-x` and the key send Claude Code's chords:
  `C-q C-x C-k` twice stops its subagents, `C-q C-x C-s` sends now.
- **Shift-Enter and Ctrl-Enter** are checked in the running app with the first step. If Claude Code
  does not push the kitty flags in oso's terminal, a follow-up decides whether a session encodes
  `S-RET` as `M-RET`; no rule is made before it is seen.

### A session's project (Q8)

- **A session's buffer is in the project of its session's directory:** the repository it started
  in, or the worktree, for a worktree session. The host sets it when the buffer gets its session,
  through the same detection on a worker as a file's ([ADR 0114](0114-projects-trust-finding-files-and-searching-a-project.md)), and the buffer keeps it; a `cd`
  inside Claude Code's shell tool does not move it. ADR 0114's trust is unchanged: the project is
  trusted or restricted as it would be for one of its files.
- So every command that uses the current buffer's project works from a session's buffer, with or
  without session mode's keys: `C-x a w`, `worktree-list`, `C-x v r` and the review commands,
  `C-x p f`, `C-x p m`, `trust-project`.
- **`*Sessions*`'s rows:** `w` is bound, as ADR 0121 planned: `session-worktree` in the row's
  session's project, then ADR 0126's `worktree-create` as ADR 0131's table says. `c` (`claude` in the
  row's project) comes with ADR 0131's step. The buffer itself stays outside every project.

### The overview (Q9)

- **ADR 0131's tree is the overview.** ADR 0121's `C-x a o` was `*Sessions*` filtered to one
  repository; ADR 0131's pane shows each repository with its worktrees and sessions. So `C-x a o`
  and `C-c o` show the pane, with point on the current project's row, and unfold it.

### The modeline (Q5)

- **A session's buffer names its mode `Session` in place of `Terminal`.** The rest is as now:
  `char` in char mode, the session's state with its reason and age, `exited N`, the title.

### The user's keys (Q10)

- **`[keys.session]` in `config.toml` rebinds the session keys,** in a user layer just above the
  session keymap, built on the configuration thread as `[keys.global]` is
  ([ADR 0108](0108-the-user-configuration-file.md), [ADR 0110](0110-adr-0108-as-built.md)).
- Only sequences that start with `C-c` are taken, as no other key reaches a keymap in a terminal;
  any other entry is refused with its line: `only C-c keys reach a session's keys`.
- `C-c C-c` and `C-c C-k` cannot be rebound there: they are how `C-c` reaches Claude Code and how
  char mode ends. `C-c C-g` cannot be typed (ADR 0108).
- The other rules of `[keys.global]` hold: a command name or `false`, at most 10,000 entries and 16
  keys a sequence, the swap waits for a pending prefix.

### Agents

- **Nothing changes in reach** ([ADR 0085](0085-agent-channel-and-oso-mcp.md)). Session mode adds keys, which
  only the person types; the agent channel runs commands by name and never types into a terminal
  (ADR 0120, Q15).
- `session-pin` and `session-archive` are `Person`, as `sessions-pin` and `sessions-archive` are;
  `session-stop` stays `Person`.

### Code

- `bindings/agent_sessions.rs` gains the session table beside its `C-x a` table; a
  test checks that every `C-x a` key has its `C-c` twin, so the two cannot drift.
- `dispatch/keymaps.rs`: the session keymap and the user's session keymap, in the minor slot for a
  marked buffer.
- `dispatch/terminal.rs`: after `C-c`, the session layers are read too, and an unbound key is
  undefined in a session.
- `terminal/route.rs`: `State` learns whether the terminal hosts a session, for `C-z` and, once
  built, `C-s`, `M-p` and `M-n`.
- `app/sessions/hosting`: marks the buffer and sets its project.
- `config`: `[keys.session]`.
- `commands/agent_sessions`: `session-stop` acts on this buffer's session outside `*Sessions*`;
  `session-pin` and `session-archive` are new.

### Cost

- A key that goes to Claude Code costs what it costs now: the route decides before any keymap.
- A `C-c` key looks in one or two more keymaps, a few hash lookups.
- Setting a session buffer's project is one detection on a worker, once per buffer.

## Steps

1. **The mode and its keys:** the buffer's mark, the keymap, the unbound rule, `C-c ?`, `C-c C-u`,
   the modeline, `session-stop`/`session-pin`/`session-archive` on the buffer, `C-z`, the
   project of a session's buffer and `w` in `*Sessions*`. Checked in the running app with a real
   `claude`: the table above, `C-z`, Shift-Enter and Ctrl-Enter.
2. **`[keys.session]`.**
3. With ADR 0120's step for them: `C-s`, `M-p`, `M-n` in a session and `C-c C-s`. With ADR 0131's
   step: `C-x a o`, `C-c o`, `c` in `*Sessions*`.

## Questions

1. **Which `C-c` keys.**
   (a) `C-c` and a key runs what `C-x a` and the key runs; commands on this session take capitals
   and punctuation; `C-c` and a control key stays the terminal's. One rule to learn; letters are
   quick to type.
   (b) `C-c` and a control key for session commands (`C-c C-s`, `C-c C-n`, …), Emacs's convention
   for a mode, with letters left free. Each control key must avoid the terminal's (`C-c C-c`,
   `C-c C-k`, `C-c C-j`), and the keys no longer match `C-x a`.
   (c) A short set of its own, only the commands used most (`n`, `d`, `s`, `b`, `m`, `@`), the rest
   on `C-x a`. Less to learn; two prefixes for one family.
2. **`C-c` and an unbound key in a session.**
   (a) Undefined: the echo area says so and nothing is sent. A typo never interrupts Claude Code or
   clears its draft; `C-c C-c` is the one way to send Ctrl-C.
   (b) Both go to Claude Code, as in any terminal (ADR 0120). One rule for every terminal; a typo
   sends Ctrl-C.
3. **What the mode is.**
   (a) A minor mode on the terminal buffer of a hosted session, from its start, adopt, fork, resume
   or restore until the buffer is killed, also after `claude` exits; off in char mode.
   (b) A major mode `Session` of its own in place of the terminal's, with the terminal's keys
   copied in. One layer fewer; the terminal's keys live in two tables.
   (c) As (a), but also on in char mode. Session keys always work; char mode no longer sends every
   `C-c` key.
4. **Commands on this session.**
   (a) `C-c K` stop, `C-c P` pin, `C-c A` archive on this buffer's session: `session-stop` works on
   the buffer too, `session-pin` and `session-archive` are new. Renaming waits for its own decision,
   as Claude Code has a name of its own (`/rename`).
   (b) As (a), and `C-c R` `session-rename`, which changes oso's name and the buffer's only. The
   two names may then differ.
   (c) Stop only; pin and archive stay in `*Sessions*`.
5. **The modeline.**
   (a) `Session` in place of `Terminal`, the rest as now.
   (b) `Terminal Session`, a minor mode's word after the major mode's, as Emacs shows them.
   (c) No change: the session's state already says it is one.
6. **ADR 0120's planned `C-s`, `M-p` and `M-n` in a session.**
   (a) They go to Claude Code as they are (stash, model picker); the live search is `C-c C-s` in a
   session.
   (b) As in any terminal: `C-s` is the live search and `M-p` sends `C-p`; Claude Code's own keys
   through `C-q C-s` and `C-q M-p`.
7. **`C-z` in a session.**
   (a) Held back, with a message; `C-q C-z` sends it. Only if the running app shows that it
   suspends Claude Code with nothing to resume it.
   (b) Sent as now.
8. **A session's buffer and projects.**
   (a) A hosted session's buffer is in the project of its session's directory, so every
   project-scoped command works from it; `w` in `*Sessions*` uses the row's session's project.
   (b) Every terminal is in the project of the directory it started in, as `C-x p m` already finds
   it. Also a plain terminal; a `cd` there is not followed.
   (c) No project for the buffer; only the session keys find the session's project themselves.
   Plain `C-x a w` and `C-x v r` still refuse in a session.
9. **`C-x a o`, the repository's overview.**
   (a) It shows ADR 0131's pane with point on the current project's row, unfolded; `C-c o` in a
   session.
   (b) ADR 0121's plan: `*Sessions*` filtered to one repository.
   (c) Dropped: the pane is enough.
10. **`[keys.session]` in `config.toml`.**
    (a) Yes, with this step's second part: `C-c` sequences only, `C-c C-c` and `C-c C-k` refused.
    (b) Not yet: it stays reserved, as every name under `[keys]` but `global` is (ADR 0110).

## Changes to accepted ADRs

Each passage stays in its ADR as written; what holds now follows it.

- [ADR 0120](0120-the-terminal.md), "After `C-c`, only the terminal's own keymap is read (`C-c C-c`,
  `C-c C-k`), so any other `C-c` key, the global and the user's among them, goes to the program
  with it": in a session's buffer the session's keymaps are read too, and a key none binds is
  undefined.
- ADR 0120, "`C-s`, the live search above" and "`M-p` and `M-n`, which send `C-p` and `C-n`": not in
  a session's buffer, where they go as they are, and the live search is `C-c C-s`.
- ADR 0120, "ADR 0114's project table is unchanged: a terminal has no file, so it points into none":
  a hosted session's terminal points into its session's project.
- ADR 0120, "The modeline says `Terminal`": `Session` in a session's buffer.
- [ADR 0121](0121-agent-sessions-and-spaces.md), "Keys (Q7): the prefix `C-x a`": also `C-c` and the
  same key inside a session's buffer; `C-x a o` shows ADR 0131's pane at the project's row.
- ADR 0121, "The overview: `*Sessions*` filtered to one repository (`C-x a o`)": the pane at the
  repository's row.
- [ADR 0110](0110-adr-0108-as-built.md), "so does every other name under `[keys]` but `global`":
  but `global` and `session`.

## Consequences

- Session keys are two keys shorter inside a session, and the same letters as `C-x a`.
- A typo after `C-c` no longer interrupts Claude Code.
- Project-scoped commands work from a session's buffer, so a review or a worktree starts from the
  session the person is looking at.
- Claude Code's own keys all still reach it; `C-y` and the `C-x` chords still need `C-q`.
- `C-c` keys behave differently in a session and in a plain terminal: `C-c g` is undefined in one
  and sends `C-c g` in the other.
- `C-c <letter>`, which Emacs keeps for the person, is oso's in a session; `[keys.session]` is
  where the person changes it.
- Follow-ups: `S-RET` if the kitty flags are not pushed; a `session-rename` if Q4 leaves it out;
  `describe-mode`.

## Alternatives considered

- **Keep `C-x a` only.** Nothing to build; the keys stay three long inside the buffer the person
  uses most, which is what the author asked to change.
- **Session commands on a key Claude Code does not use, such as `C-z` or `M-g`.** No prefix to
  share with the terminal, but a single key holds one command, and `C-z` is the global `undo`.
- **Detect `claude` in any terminal by its process name.** Would give a hand-started `claude` the
  mode too, but a terminal's process is the shell's to change, and such a session has none of
  oso's identity for the commands to act on; `session-adopt` brings it in instead.
- **Give the session mode `C-x` and send Claude Code's `C-x` chords with `C-c C-x`.** The author
  keeps `C-x` oso's everywhere; `C-q C-x` already sends the chords.
- **A general minor-mode system for every buffer.** Not needed for one mode; `Layers` already has a
  minor slot, and the buffer's mark is enough until a second mode needs more.
