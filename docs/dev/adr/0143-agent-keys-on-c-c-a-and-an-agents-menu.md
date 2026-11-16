# 0143. Agent keys on `C-c a` and an Agents menu

- Status: Accepted, amended by 0138 and 0145
- Date: 2026-11-16

## Context

[ADR 0121](0121-agent-sessions-and-spaces.md) (Q7) put the agent session commands on the prefix
`C-x a`, which works inside a terminal because semi-char mode keeps `C-x`
([ADR 0120](0120-the-terminal.md)). [ADR 0132](0132-a-session-mode-for-hosted-sessions.md) gave a
session's terminal `C-c` and the same letter, and [ADR 0136](0136-issue-spaces.md) added
`C-x a O` and the space menu on `C-x a S`. [ADR 0126](0126-sessions-and-worktrees-in-depth.md)
plans `C-x a B`, `C-x a g`, `C-x a k` and rebinds `C-x a j` and `C-x a w`;
[ADR 0129](0129-dictation.md), still proposed, plans `C-x a v`.

The author, a long-time Emacs user, said on 2026-11-16 that `C-x a r` and its siblings do not feel
like Emacs. The reasons found:

- **`C-x` and a key is the core's.** The Emacs manual lists the standard prefixes as "C-c, C-h, C-x,
  C-x RET, C-x @, C-x a, C-x n, C-x r, C-x t, C-x v, C-x 4, C-x 5, C-x 6, ESC, and M-g"; Emacs adds
  to it itself (`C-x t` in 27, `C-x p` and `C-x x` in 28, `C-x w` in 29). Packages rarely enter it:
  magit's `C-x g` is the known exception, and one key, not a prefix. perspective.el dropped its
  `C-x x` default when Emacs 28 took it. Agent features read as a package, so `C-x a` reads as a
  package in the core's space.
- **`C-x a` is Emacs's abbrev prefix** (`C-x a g`, `C-x a l`, `C-x a i g`, `C-x a e`, `C-x a '`).
- **The letters mix verbs** (`r` resume, `a` adopt, `s` list) with mirrors of core keys (`b` as
  `C-x b`) and capitals (`O`, `S`), which core `C-x` maps almost never use, and three keys show
  nothing after `C-x a`: the which-key list ([ADR 0117](0117-menus.md), step 6) is not built.
- **Session mode's `C-c` and a letter is the user's space.** The elisp manual's "Key Binding
  Conventions": "Sequences consisting of 'C-c' and a letter (either upper or lower case; ASCII or
  non-ASCII) are reserved for users"; `C-c` and a control character or a digit, and `C-c { } < > : ;`,
  are reserved for major modes; `C-c` and any other punctuation is for minor modes.

What well-known packages do, read on 2026-11-16:

- **magit** (`magit-define-global-key-bindings`): by default `C-x g` status, `C-x M-g` dispatch,
  `C-c M-g` file dispatch; `recommended` adds `C-c g` and `C-c f`, "but we cannot use them by
  default, because the C-c <LETTER> namespace is strictly reserved for bindings added by the user."
  Each key opens a transient.
- **claude-code-ide.el** suggests one key, `C-c C-'`, for its transient menu; **claude-code.el**
  suggests `C-c c` as a prefix map with a transient on `C-c c m`; **aidermacs** suggests `C-c a` for
  its transient; **ellama** suggests `C-c e` and says a transient is better than a keymap;
  **gptel** binds no global key. None of these binds a key by default.
- **eat** and **vterm** put a terminal's own commands on `C-c C-<letter>` (`C-c C-c`, `C-c C-k`,
  `C-c C-e`, `C-c C-j`, `C-c C-t`), and keep `C-c` and `C-x` from the program.
- **lsp-mode** defaults its prefix to `s-l`; **projectile** suggests `s-p` on macOS. Emacs's own
  bindings use no Super or Hyper key.

What oso has:

- Menus ([ADR 0117](0117-menus.md)): a key can open one, `config.toml` can rebind it or replace a
  menu whole, a head can open another menu, and a describe function can dim a head with a reason.
  oso's defaults already take one user key, `C-c t` for the toggles (ADR 0117, Q25).
- On macOS, Command is Meta and Option is Super ([ADR 0033](0033-key-presses-and-notation.md)).
  oso has no Hyper modifier: a keyboard's Hyper (Shift, Command, Control and Option together)
  reaches oso as `C-M-s-` and an upper-case letter, `C-M-s-A` for Hyper-a, which `config.toml`
  can bind. In a terminal, semi-char mode sends every key but `C-x`, `M-x`, `C-c`, `C-q` and `C-y`
  to the program (`terminal/route.rs`), so such a key never reaches oso there.

## Decision

The author chose on 2026-11-16, from options put to them:

### `C-c a` opens the Agents menu

- **One key, `C-c a`, opens a built-in menu, `agents`** (ADR 0117), in every buffer, terminals
  included. It is a user key by the manual; the author chose it as oso's default, as `C-c t` is.
- **Inside every terminal, `C-c a` is oso's:** a terminal's own keymap binds it, as it binds
  `C-c C-c` and `C-c C-k`, so a plain terminal no longer sends `C-c a` to its program, and a
  session's terminal opens the menu too. `C-q C-c` then `a` still sends both.
- **The menu keeps today's letters**, `O` and `S` among them, drawn at once with their labels, as
  transient does:

  | Group | Key | Command |
  |---|---|---|
  | Start | `c` | `claude` |
  | | `w` | `session-worktree` |
  | | `r` | `session-resume` |
  | | `a` | `session-adopt` |
  | Go to | `s` | `sessions`, the pane |
  | | `b` | `session-switch` |
  | | `n` | `session-next-waiting` |
  | | `d` | `session-next-done` |
  | | `f` | `session-follow-message` |
  | Talk | `m` | `session-send` |
  | | `@` | `session-mention` |
  | | `e` | `session-explain` |
  | Space | `j` | `space-switch` |
  | | `z` | `space-zoom` |
  | | `O` | `space-new` |
  | | `S` | the space menu (ADR 0136), opened over this one |
  | Session | `K` | `session-stop` |
  | | `P` | `session-pin` |
  | | `A` | `session-archive` |
  | | `v` | `review-session` |
  | | `V` | the review menu (ADR 0122) |

  `q` quits. `C-u C-c a m` gives the head the prefix argument, as ADR 0117 (Q15) says, and `C-u`
  inside the menu does too.
- **Outside a session's terminal, `P` and `A` are dim** and say "This buffer holds no session" when
  pressed, from the menu's describe function; `K` stays live, as `session-stop` also stops the
  session on a row of the pane.
- **Keys planned elsewhere become heads of this menu:** ADR 0126's `B` (board), `g` (grid),
  `k` (`worktree-cleanup`), its `jump-anywhere` on `j` and `worktree-create` on `w`; ADR 0132's
  `o` (overview); ADR 0129's dictation on `t`.

### Session mode moves to `C-c C-<letter>`

In a terminal's buffer that hosts a session (ADR 0132), the session commands move off `C-c` and a
letter, the user's space, to `C-c` and a control key, as eat and vterm put a terminal's commands:

| Key in a session | Command |
|---|---|
| `C-c a` | the Agents menu |
| `C-c C-n` | `session-next-waiting` |
| `C-c C-d` | `session-next-done` |
| `C-c C-b` | `session-switch` |
| `C-c C-p` | `sessions`, the pane |
| `C-c C-r` | `session-resume` |
| `C-c C-a` | `session-adopt` |
| `C-c C-w` | `session-worktree` |
| `C-c C-e` | `session-explain` |
| `C-c C-f` | `session-follow-message` |
| `C-c C-z` | `space-zoom` |
| `C-c C-v` | `review-session` |
| `C-c C-u` | `universal-argument`, so that `C-c C-u C-c a m` is `C-u C-c a m` |
| `C-c @` | `session-mention` |
| `C-c +` | `terminal-allow-agent-read` |
| `C-c -` | `terminal-deny-agent-read` |
| `C-c ?` | `session-help` |

- **The rule:** in a session, `C-c C-<letter>` runs the Agents menu's head of that letter, where the
  terminal and Emacs leave `C-<letter>` free. Signs keep their `C-c` keys: they are the minor
  modes' space.
- **Some heads are reached through `C-c a` only,** as their control keys are taken: `c` (`C-c C-c`
  sends `C-c`), `j` (`C-c C-j` is ADR 0120's copy mode), `m` (`C-c C-m` is `C-c RET`, ADR 0120's
  link at point), `k` (`C-c C-k` is char mode), and `s` (`C-c C-s` is the live search, ADR 0132,
  Q6), whose pane takes `C-c C-p`. `O`, `S`, `K`, `P`, `A` and `V` are capitals or menus, and stop,
  pin and archive are better behind a menu that shows what they do: `C-c a K` stops this session.
- A key that types in a session other than those keeps ADR 0132's rule: `C-c` and a key no layer
  binds is undefined, and nothing goes to Claude Code. `C-c` and a letter is free for the user's
  `[keys.session]`.
- `C-c C-y` (attaching an image from the clipboard,
  [ADR 0142](0142-attaching-the-clipboard-s-image-to-a-session.md)) is session mode's own key,
  as `C-c C-u` is: it has no head in the Agents menu.

### `C-x a` is given back

- **`C-x a` is unbound now,** with no alias: oso has no abbrevs, and the prefix stays free for them.
- The user guide shows how to bring it back:

  ```toml
  [keys.global]
  "C-x a" = { menu = "agents" }      # C-x a opens the menu
  ```

### Super keys reach oso in a terminal

- **Semi-char mode keeps every key with Super (`s-`) for oso,** besides `C-x`, `M-x`, the `C-c`
  prefix, `C-q` and `C-y`. A program reads Super as Super, which few do; Claude Code's Alt keys come
  from Command, which is Meta. So a key the user binds with Super, such as Option-a (`s-a`) or a
  keyboard's Hyper-a (`C-M-s-A`), works in a terminal too:

  ```toml
  [keys.global]
  "C-M-s-A" = { menu = "agents" }    # Hyper-a opens the menu
  ```

- A Super key oso does not bind is undefined in a terminal, as anywhere, rather than sent; `C-q` and
  the key still sends it. Char mode is unchanged: it sends every key but `C-c C-k`.
- **No `H-` notation for now.** Hyper is written as the modifiers it sends.

### Messages name the key as bound

Messages that named a key in their text (`C-x a w` in the shared-directory question, `C-x a e
explains` after a start without asking, `C-x a S u` for `space-use-repo`, `C-u C-x a m` for a send
too large) name the key that reaches the command now, as where-is would: the shortest sequence in
the layers active in the current buffer, a menu's key and its head's key included (`C-c a e`, or
`C-c C-e` in a session), or `M-x` and the name when none does. A rebinding in `config.toml` shows.

- A command's own error, which cannot look up keys, names the command as where-is does one bound
  nowhere: `C-u M-x session-send` for a region too large to send. A substitution of `\[name]`
  in errors, as Emacs's `substitute-command-keys` does, was left out: errors quote the person's
  text, which it would rewrite, at a cost per name.
- **`M-x` lists a command a menu's head runs with that key** (`session-worktree  C-c a w`), as it
  lists one a key binds ([ADR 0076](0076-completion-and-m-x.md)). Without it, a command reached
  only from a menu and a session's keys would leave `M-x` in every other buffer.

## Changes to accepted ADRs

- **[ADR 0121](0121-agent-sessions-and-spaces.md), "Keys (Q7):** the prefix `C-x a` …": the keys
  are the Agents menu's heads on `C-c a`, as above. Every other `C-x a` and a letter in it reads as
  `C-c a` and the letter.
- **[ADR 0132](0132-a-session-mode-for-hosted-sessions.md), "The keys (Q1)":** "in a session, `C-c`
  and a key runs what `C-x a` and the same key runs elsewhere" and its table are replaced by the
  session table above. Its `C-c o` and `C-c k` become `C-c a o` and `C-c a k`. "`C-c` and a control
  key stays the terminal's" holds for `C-c C-c`, `C-c C-k`, `C-c C-j` and `C-c RET` only.
- **[ADR 0136](0136-issue-spaces.md):** `C-x a O` and session mode's `C-c O` are `C-c a O`;
  `C-x a S` and `C-c S` are `C-c a S`; `C-x a j` and `C-x a o` are `C-c a j` and `C-c a o`.
- **[ADR 0117](0117-menus.md), "Default keys (Q25)":** the default menu keys are `C-c t` (toggles),
  `C-x v r` (review) and `C-c a` (agents).
- **[ADR 0126](0126-sessions-and-worktrees-in-depth.md):** `C-x a B`, `C-x a g`, `C-x a j`,
  `C-x a w`, `C-x a z` and `C-x a k` are `C-c a` and the same key.
- **[ADR 0131](0131-an-orca-style-sessions-pane.md) and
  [ADR 0135](0135-agents-start-sessions-without-asking.md):** their `C-x a` keys read as `C-c a`
  and the same key; the echo after a start without asking names the key as bound.
- **[ADR 0120](0120-the-terminal.md), "Input (Q12)":** semi-char mode also keeps Super keys, and a
  terminal's keymap binds `C-c a`.
- **[ADR 0140](0140-a-spaces-pane-with-sections-and-guides.md), Q3:** `C-x a s` and session
  mode's `C-c s` are `C-c a s` and `C-c C-p`; `C-u C-c a s` hides the pane. Its modeline's
  "`C-x a s` to enter" names the keys of `sessions` as bound.
- **[ADR 0076](0076-completion-and-m-x.md):** `M-x` also lists a command that a menu's head runs,
  from a menu a key in the active layers opens, with the menu's key and the head's as its binding.
- [ADR 0129](0129-dictation.md) is still proposed; its key is now `C-c a t` (Q3), changed in place.

## Consequences

- Every agent key is shown when `C-c a` is pressed, without waiting for the which-key list.
- `C-x` holds only the core's keys again. `C-x a` and the session mode's letters are free for the
  user.
- In a session, the frequent commands are still two keys (`C-c C-n`), and with Control on a key
  held by a thumb they need no change of modifier. The rest are three, as before.
- Muscle memory changes: `C-x a r` is `C-c a r`, and a session's `C-c n` is `C-c C-n`.
- A plain terminal's program no longer receives `C-c a`, nor any Super key, in semi-char mode;
  `C-q` sends them.
- `[keys.global] "C-c a" = false` removes the key outside terminals; a terminal keeps it, as the
  user's global keys do not apply after `C-c` there (ADR 0132). `[keys.session]` can unbind it in a
  session. A Super key bound instead works everywhere.
- **Work:** `bindings/agent_sessions.rs` (no global table; the session table above),
  `bindings/terminal.rs` (the terminal's menu key), `bindings/menus.rs` (the `agents` menu, a head
  that opens a menu, `MENU_KEYS`), the menu's describe function beside the session commands,
  `terminal/route.rs` (Super), a where-is over keymaps and menus in `dispatch`, which `M-x` uses
  too, the four messages, the user guide, `ui.md`, `agent-rules.md`, and the e2e tests that type
  `C-x a`. No new crate, crate edge, public trait or module.
- **[ADR 0140](0140-a-spaces-pane-with-sections-and-guides.md)'s `C-x a s` and `C-c s`** are
  `C-c a s` and `C-c C-p`, and the pane's modeline names the keys of `sessions` as bound in every
  buffer (the user's over the global ones), looked up once and again after the user's keys or
  menus change, not each frame.
- **Follow-up:** `s-SPC` in a terminal runs `insert-full-width-space`, which refuses in a
  terminal's buffer; before, the program got Super-Space. The author chose on 2026-11-16 to keep
  it so here, and to make `s-SPC` send a full-width space (U+3000) to the program as text in a
  later change.

## Alternatives considered

- **`C-c C-'` for the menu,** as claude-code-ide.el suggests and the author's Emacs binds: a major
  mode's key by the manual, so a later oso mode could not use it. The author chose `C-c a`.
- **`C-c M-a`,** the slot magit uses by default because the manual reserves it for no one: correct
  by the manual, but Meta is Command, a change of modifier after `C-c`.
- **One `C-x` key, as magit's `C-x g`** (`C-x j` or `C-x c`, unbound in Emacs): a precedent, but
  still the core's space, which was the complaint.
- **A `C-c '` prefix map with no menu,** the minor modes' space: correct by the manual, but still
  three keys with nothing shown until which-key, and a movable prefix would need a setting of its
  own.
- **A Hyper or Super key by default:** only macOS with a remapped key has Hyper, the author does not
  press Option alone, on Linux and Windows Super is the logo key the desktop often takes, and
  window managers take many Hyper keys. Kept as the user's choice, which the Super rule makes work in
  terminals.
- **Keeping `C-x a` with reworked letters:** the least work, but the core's space and the abbrev
  prefix stay.
- **Keeping session mode's `C-c` and a letter:** shorter to type for letters taken by the terminal
  as control keys, but in the user's space; or `C-c C-<letter>` for every head, re-lettering where
  the terminal collides (stop on `C-c C-x`): one table, but letters unlike the menu's.
- **Keeping `C-x a` as an alias for one release:** oso has no release yet whose users would need
  it; the snippet above brings it back.
- **An `H-` notation:** needs a modifier oso does not have; left until a keyboard sends a real
  Hyper.

## As built

- `C-c C-y` came with ADR 0142 while this was built, and joined the session table as session mode's
  own key, beside `C-c C-u`, `C-c ?` and the agent channel's reads; the test that ties each
  `C-c C-<letter>` to the Agents menu's head of that letter lists it among those.
- The source control flow merged meanwhile (ADR 0127, step 6a) said "`C-x a c` starts one" when no
  session runs in a worktree; it names the keys of `claude` as bound, as the other messages do.
