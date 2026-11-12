# 0117. Menus

- Status: Proposed
- Date: 2026-11-12

## Context

The roadmap puts "the menu component, porting the author's hydra menus" in M3
([roadmap](../roadmap.md)). Earlier ADRs fixed its outline and left the rest here:

- [ADR 0008](0008-floating-ui.md): the author uses about nine hydra menus, and git needs
  transient-style ones. Hydra and transient merge into **one menu component** with groups of
  single-key heads, heads that keep the menu open, and toggle-state indicators. Menus are
  **defined declaratively in the config file** and reload on change.
- [ADR 0026](0026-m2-ui-layout.md) and [`ui.md`](../ui.md): menus float at the **center** of the
  frame, as the author's hydra-posframe does today. The IME is off in a menu.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md): a prompt is a window of its own
  over the tiled ones, in `Layer::FLOAT` (16) and `Layer::FLOAT_CONTENT` (17); "M3's menus take
  layers above these". A command never waits for a key: what it reads, the dispatcher reads
  before it runs ([ADR 0034](0034-command-registry-and-keymaps.md)).
- [ADR 0076](0076-completion-and-m-x.md): completion floats at the center too, 62% wide and 18
  candidate rows tall. `M-x` lists every command but those that read their key or belong to a
  keymap not active beneath it.
- [ADR 0056](0056-typing.md): "a menu that must take no text is the only keymap active, or gives
  itself a text command that refuses, as Emacs's `suppress-keymap` does." A question, isearch and
  query-replace each replace the global keymap with their own today (`dispatch/keymaps.rs`); a key
  isearch does not bind ends the search and runs as usual.
- [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md): `C-u`, `M-<digit>` and `M--` build a prefix argument, whose
  digits are a small keymap of their own while it is read. Two more small layers of one or two keys
  exist or are planned: `e` after `C-x e` runs the macro again
  ([ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)), and `indent-rigidly`
  with no count opens a layer of arrows ([ADR 0115](0115-indentation.md), step 3, not built yet).
- [ADR 0108](0108-the-user-configuration-file.md) and [ADR 0110](0110-adr-0108-as-built.md):
  `config.toml` is read, parsed and checked off the UI thread, against the thread's own copy of the
  registry, and swapped in between keys. **Nothing in it is executed:** it names registered
  commands and values, and cannot define a command, a macro or a key sequence to replay. The user
  layer sits just above the global keymap. `C-g` is never configurable. Entries under unknown tables
  warn and are skipped.
- [ADR 0089](0089-built-in-themes-switched-at-run-time.md): every face's pairs pass a contrast check
  in CI for every theme, 4.5 for labels and 3.0 for marks.
- [ADR 0085](0085-agent-channel-and-oso-mcp.md): a command that reads a key is `Person`. The channel
  refuses `run`, `open` and `insert` while the person is in the middle of something: a prompt, keys
  in progress, a composition, or a key in the last second.
- [ADR 0097](0097-visual-aids.md): the visual aids have no keys; "the toggle menu (M3) will reach
  them". [ADR 0030](0030-m2-editing-commands.md) left a multiple-cursor menu to M3.
- [ADR 0112](0112-window-splits.md): windows split; floats cover the whole tiled area whatever the
  layout. The selected window is the one commands act on.
- [ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md): a macro holds the keys
  typed and the text an input method commits, and a replay feeds them back; a key rebound since
  recording does what it does now.
- The author's keyboard sends `英数` for Escape, so a menu cannot count on `ESC` as its way out.
- The author said the magit design waits until M3 is done, so transient's arguments (infixes) have
  no user yet.

The author's own hydras are not in this repository, and this ADR does not guess them: Q1 asks for
them.

### What other editors do

Read on 2026-11-12, from each project's documentation or, where it is silent, its source:

- **Emacs hydra** ([README](https://github.com/abo-abo/hydra),
  [colours](https://github.com/abo-abo/hydra/wiki/Hydra-Colors),
  [nesting](https://github.com/abo-abo/hydra/wiki/Nesting-Hydras), `hydra.el`): a head is a key, a
  command and a hint; a `nil` command only exits. Red, the default, stays after a head, and a key not
  in the hydra quits it and runs; blue exits after a head; amaranth and teal refuse a foreign key with
  a warning and stay; pink runs it and stays. `C-u`, `-` and the digits build a count in the hydra's
  own keymap, so `5 g` repeats a head. The hint is a docstring with `_k_` placeholders, or a table
  built from heads' `:column`. It shows at the bottom (`lv`) by default; hydra-posframe puts it at the
  window's center. Nesting is a head that calls another hydra's body, with `:exit t`, "since only one
  hydra can be active at a time"; a way back is the user's own code. `:pre`, `:post` and `%(expr)`
  in hints run Lisp.
- **Emacs transient** ([manual source](https://raw.githubusercontent.com/magit/transient/main/docs/transient.org)):
  infixes (switches and options with values) stay, suffixes exit, and a command bound outside the
  transient is refused with "Unbound suffix" and does not close it. Sub-prefixes stack, replace or
  recurse; `C-g` quits one level and `C-q` all. Groups lay out as columns, with levels 1 to 7. Values
  can be saved per session or to a file. A prefix argument goes after the prefix key, before the
  suffix. It draws in a bottom side window, at once or after a delay.
- **which-key** ([repository](https://github.com/justbur/emacs-which-key), built into Emacs 30):
  lists a typed prefix's continuations after 1.0 s, in a bottom side window by default, or the
  minibuffer or a frame. No stay heads.
- **Helix** ([keymap](https://docs.helix-editor.com/keymap.html),
  [remapping](https://docs.helix-editor.com/remapping.html), `helix-term/src/keymap.rs`): minor modes
  return to normal mode after one command; sticky ones, such as `Z`, stay until Escape, and an
  unbound key there leaves them on. User keys are nested TOML tables of command names, but labels and
  stickiness are built-in only. The infobox is drawn at the bottom right.
- **Kakoune** ([mapping](https://github.com/mawww/kakoune/blob/master/doc/pages/mapping.asciidoc),
  [commands](https://github.com/mawww/kakoune/blob/master/doc/pages/commands.asciidoc)): user modes
  are declared and filled with `map`, each key with a docstring shown in the info box;
  `enter-user-mode` takes one key, `-lock` stays until Escape, and a count is passed on. An unbound
  key does nothing.
- **VS Code** (`abstractKeybindingService.ts`; [VSpaceCode Which Key](https://vspacecode.github.io/docs/whichkey/reference)):
  a chord's first key shows "Waiting for second key of chord" in the status bar. The Which Key
  extension defines menus as JSON data and shows them in a QuickPick; a transient menu stays until an
  item with `exit: true`, and an unbound key says it is undefined and closes the menu.
- **which-key.nvim** ([repository](https://github.com/folke/which-key.nvim)): Lua specs, a 200 ms
  delay, presets at the bottom, centred at the bottom, or at the bottom right; `loop = true` keeps the
  popup until Escape, a hydra mode. It shows no popup while a macro is recorded or replayed.

So most editors exit after one key and offer a sticky mode, and a foreign key either runs and exits
(hydra red), runs and stays (hydra pink, transient `t`) or is refused (transient's default, hydra
amaranth). Only hydra's hint and Helix's and Kakoune's info boxes are drawn from data a user writes
without code; hydra's needs Lisp for anything past the docstring.

## Decision

Draft, with the recommended option (a) of every question under [Questions](#questions). Nothing here
is decided until the author answers.

### What a menu is (Q2, Q3)

- **A menu is a named key layer with a float that lists it.** It has a title, columns of heads, and a
  default for its heads: `stay` or `exit`. While it is open, its keymap replaces the global one, as
  isearch's does, so no other binding is reached through it.
- **A head is one key press** (any modifiers, in Emacs notation), a label, a command name, and
  optionally its own `stay` or `exit`. A head with no command only closes the menu, as hydra's
  `("q" nil)`. A head may open another menu instead of a command (Q11).
- **`stay` heads run their command and keep the menu open** (hydra's red); **`exit` heads close it
  first and then run** (blue). The menu's default is `stay`, as hydra's is, so a `defhydra` ports
  head for head; a menu of one-shot commands says `exit`.
- **A key the menu does not bind (Q3)** closes it and runs as usual, through every layer, as a key
  ends isearch: hydra's default (`:foreign-keys nil`). A menu may say `foreign-keys = "warn"`, and
  then such a key says `q to quit` (its first head with no command, or `C-g`) and does nothing, as
  hydra's amaranth; or `"run"`, and then the key runs and the menu stays, as hydra's pink. `C-g`
  always closes a menu, first ending a pending prefix argument, in the `C-g` chain of ADR 0072.
- **A head stays only while nothing else takes the keys.** If its command reads an argument in a
  prompt, or opens a prompt, a question, isearch or query-replace, or starts a macro replay, the menu
  closes as that opens, so a menu never sits over a prompt and keys go where the person expects.
- **A failing head keeps a `stay` menu open,** with its error in the echo area, so `n` at the end of
  the buffer in a motion menu does not throw the person out.
- **Text from an input method is a key the menu does not bind:** the IME is off in a menu, but a
  person who turns it on by hand gets the menu's `foreign-keys` rule for the committed text, through
  a text command on the menu's keymap (ADR 0056).
- **Hidden menus (Q16):** a menu may say `show = false`. It is the same key layer with no float, as
  Emacs's `repeat-mode` maps are, for layers such as `indent-rigidly`'s arrows.
- Transient's arguments (infixes: switches and options with values that later heads read) are not
  in this ADR. They come with the git status buffer and its own ADR, which extends the definition
  with a new head kind; nothing here closes that door.

### Where it is drawn (Q4, Q5, Q6, Q12)

- **At the center of the tiled area, fitted to its content (Q4),** as hydra-posframe draws: as wide
  as its columns and as tall as its longest column, plus the title row, a row of padding and the
  ring. It keeps completion's look: the 5-point border a little darker than the fill, all four
  corners rounded (ADR 0076), and never closer than 2 columns or 1 row to an edge.
- **The selected window stays the tiled one, with its cursors drawn as usual (solid),** its region
  and band too, because the heads act on it. A menu is not a window and has no buffer. It may cover
  the cursor; the view does not move for it.
- **It shows at once (Q5),** in the frame after the key that opened it, as hydra's hint does; no
  idle delay. During a macro replay it is not drawn, as which-key.nvim does, so a replay does not
  flash it; a replay that ends with a menu open draws it then.
- **Layout:** the title, then the columns side by side, each with an optional title, its heads one
  per row as `key  label`, 2 columns apart. A head's key is in the `MenuKey` face, a chip like a
  question's (`Face::Key`), and the key of a `stay` head is in `MenuKeyStay`, hydra's red as a hue the
  theme chooses (Q6). Both pass ADR 0089's checks: 4.5 against the float, and 3.0 between the two.
- **A toggle (Q12)** shows `[x]` or `[ ]` before its label. A command that toggles declares how to
  read its state (`Command::state`, a plain `fn(&Editor) -> bool`), and a head naming such a
  command shows it with no word in the menu's definition. The state is read when the menu is
  drawn, which is once after each key.
- **The prefix argument** shows at the end of the title row, `C-u 5-`, as the echo area shows it
  today.
- **A frame too small** gives up the padding, then the column titles, then wraps columns onto a
  second band of columns below, then cuts the longest columns with `…` on their last row, which
  says how many heads are hidden (`… 3 more`). A hidden head still runs from its key.
- **Faces:** `MenuTitle` (the title and column titles), `MenuKey`, `MenuKeyStay`, and the existing
  `Float`, `FloatBorder` and `Key`'s rules. A label is drawn through the same path as buffer text, so
  bidirectional controls and invisible characters in it are boxed (ADR 0026, ADR 0035), and its width is
  counted in cells.
- **Layers:** a menu never coexists with a prompt (Q15), so it uses `Layer::FLOAT` and
  `FLOAT_CONTENT` with pane ids of its own, and the renderer needs no new layer. ADR 0072's "layers
  above" is not needed yet.

### How menus are defined (Q7, Q8, Q9)

- **Built-in menus are Rust tables in `bindings/menus.rs`, user menus are `[menus.<name>]` in
  `config.toml` (Q7), both in one model** (`oso_core::menu::Definition`). A user menu with a built-in
  name replaces it whole. `[menus.<name>] = false` removes a built-in one, as `false` unbinds a key.
- In the file (Q9: columns, not a free-form hint):

  ```toml
  [menus.window]
  title = "Windows"
  default-head = "stay"             # as hydra's red, the default; or "exit"
  foreign-keys = "run"              # "exit" (default), "warn" or "run"

  [[menus.window.columns]]
  title = "Move"
  heads = [
    ["o", "other-window", "next"],
    ["b", "balance-windows", "balance"],
  ]

  [[menus.window.columns]]
  title = "Split"
  heads = [
    ["2", "split-window-below", "below", "exit"],
    ["3", "split-window-right", "right", "exit"],
    ["t", { menu = "toggles" }, "toggles…"],
    ["q", false, "quit"],
  ]
  ```

  A head is `[key, command, label]` with an optional fourth `"stay"` or `"exit"`. The command is a
  registered name, `{ menu = "<name>" }`, or `false` for a head that only closes the menu.

- **The same checks as keys:** a key a person cannot type, `C-g`, an unknown command, a command only
  a dev build has, an unknown menu, a key used twice in one menu, and a value over the limits below
  are refused with their line, and the rest of the menu loads. A menu with a refused head still
  opens, without it.
- **Read with the rest of the file, off the UI thread,** against the thread's registry, and swapped in
  between keys as the user keymap is. **A reload never changes an open menu:** it keeps the
  definition it opened with, and the next opening takes the new one.
- **Reaching a menu (Q8):** a key binds to a menu in `[keys.global]` as `"C-c w" = { menu = "window" }`.
  `Keymap` gains a third kind of entry, `Menu(MenuId)`, beside a command and a prefix; the resolver
  returns `Resolution::Menu`, and the dispatcher opens it. `M-x open-menu` reads a menu's name with
  completion over every menu (`ArgSpec::Menu`). `where-is` and the completion's key column show a
  menu's key for `open-menu`'s candidates.
- **Limits:** 256 menus, 128 heads per menu, 8 columns, labels and titles of 64 cells, menu names of
  64 bytes in kebab case. Past them an entry is refused with its line.

### The prefix argument (Q10)

- **As hydra:** a prefix argument typed before the menu's key goes to the first head run; inside the
  menu, `C-u`, `M-<digit>` and `M--` build one for the next head, and so do plain digits and `-`
  unless a head binds that key. A `stay` head consumes it, as any command does.
- A head that opens another menu passes the prefix argument on.

### Nesting (Q11)

- **A head that opens another menu replaces the current one**, as hydra's heads that call another
  body. No stack: a menu that wants a way back has a head that opens its parent. Since nothing
  stacks, a cycle of menus costs nothing.

### Around the menu (Q13, Q14, Q15)

- **Keyboard macros record the keys (Q13),** as for any key: a replay opens the menu and presses its
  heads, without waiting for frames. A menu redefined since recording does what it does now, as a
  rebound key does (ADR 0113). A replay that ends with a menu open leaves it open.
- **The agent channel (Q14):** `open-menu` is `Person`, since it reads keys, and so is every menu,
  which no channel command can open. While a menu is open the person is in the middle of something,
  so `run`, `open` and `insert` are refused as during a pending key ("the person is in a menu"), and
  a question the channel caused waits until it closes. `get-state` gains `menu`: the open menu's
  name, or null. Heads run commands the person pressed, so a head may name a `Person` command, as a
  key may (ADR 0108).
- **In a prompt (Q15), a menu does not open:** `Not in the minibuffer`, as `C-x b` says there. During
  isearch and query-replace, a menu's key is a key they do not bind, which ends them first.
- **Multiple cursors:** a head's command acts on the selection set, as from any key. Nothing new.
- **The IME** is off while a menu is open: `wants_ime` gains the menu's condition (`app/ime.rs`), as
  open-questions.md asks. A composition open when the menu's key arrives has ended by then, since
  the key went to the dispatcher.
- **Secret buffers:** a menu shows only its definition and the toggle states, which say nothing of
  a buffer's text, so it shows nothing a secret buffer withholds, and opens beside one as anywhere.
- **`oso dev` restart:** an open menu is not carried; the new instance starts with none.
- **Frame resize:** the layout is worked out again, as a prompt's is.

### Where the code lives

- **`oso-core`:** a new public module `menu`: `Definition`, `Head`, `HeadAction` (`Command`,
  `Menu`, `Quit`), `Stay`, `ForeignKeys`, `MenuId`, and `menu::layout`, a pure function from the
  definition, the toggle states and the text area's size to the float's cells, as
  `frame::float_regions` is. `keymap` gains `Entry::Menu`, `Binding::Menu` and `Resolution::Menu`;
  `command` gains `ArgSpec::Menu` and `Command::state`. A head names a `CommandId` and `keymap` names a
  `MenuId`, so in `guardrails/layers.txt` `menu` gets a line of its own between `keymap` and
  `command`: it uses `command`, `key` and `frame`, and only `keymap` uses it.
- **The open menu is the dispatcher's,** as a pending prefix argument is: its definition, the keymap
  built from it when it opens, and a prefix argument being typed. `Editor` does not hold it, so
  `menu` stays below `keymap` and above `command`. The app reads it to draw, to ask `wants_ime`, and
  for the channel's wait and `get-state`.
- **`oso`:** `bindings/menus.rs` (the built-in menus), `config` reads `[menus]` and `{ menu = … }`
  keys, `commands/menus.rs` (`open-menu`), `dispatch/menus.rs` (the open menu and its rules), and
  `app/draw` draws it.
- **`oso-render`:** the three faces and their contrast pairs. No new pipeline.

### Tests and measures

- Unit tests of the layout (sizes, small frames, wrapping columns, `…`), of each `stay`/`exit`/
  `foreign-keys` rule, of the prefix argument, of nesting, and of the configuration's checks and
  limits, in `oso-core` and the dispatcher's tests, with no window.
- An end-to-end flow: open a built-in menu, run a `stay` head twice, an `exit` head, a foreign key,
  and `C-g`, with a screenshot.
- A `just perf` scenario: open a menu and press a `stay` head that moves the cursor, report-only
  (`menu_open_p99_ms`, `menu_head_p99_ms`) until the author sets thresholds (Q17).

### The cost of drawing

A menu of 4 columns of 10 heads is about 40 rows of 30 cells, under 1,500 cells. Its labels do not
change while it is open, so each row's shaped text is kept by its pane id across frames, as a
prompt's label is, and only the toggles and the prefix row are shaped again after a key. The layout
is worked out when the menu opens and when the frame changes size, a pass over at most 128 heads.
Expected: under 0.1 ms on the UI thread for the layout, and a frame no costlier than one with
completion's 18 rows. The perf scenario checks it.

## Questions

To put to the author, the recommended option first.

### What to port

1. **Which hydras to port first.**
   (a) The author pastes their `defhydra` forms (or lists their names, keys and heads), and the first
   step ports them as built-in menus, or as a `config.toml` the author keeps, as they prefer.
   (b) The author lists only names and keys now; the first step ships a toggle menu (ADR 0097's
   aids, highlighting, soft wrap, line numbers, the theme pair) and a window menu (ADR 0112), and
   the rest follows from the list.
   (c) Only the toggle menu now; the author writes the rest in `config.toml` once it loads menus.

### What a menu is

2. **The model.**
   (a) Hydra's: heads that stay or exit, a default per menu, columns; transient's arguments (infixes)
   wait for magit and its own ADR. Small, and what the author's nine hydras need.
   (b) Hydra's and transient's together now: also switches and options with values that later heads
   read, with saved values and levels. Ready for magit, but a large design with no user yet.
   (c) A which-key-style list of a prefix key's continuations, shown after a delay, with no stay
   heads. Cheap, but not a port of hydra.
3. **A key the menu does not bind.**
   (a) By default it closes the menu and runs as usual (hydra's default, as isearch in oso); a menu may
   say `"warn"` (refuse and stay, amaranth) or `"run"` (run and stay, pink).
   (b) Always close and run; no per-menu choice.
   (c) Always refuse until `C-g` or a quit head, as transient does.

### How it looks

4. **Where it is drawn.**
   (a) At the center, fitted to its content, as hydra-posframe draws today (ADR 0026), with
   completion's border; the window beneath stays selected and its cursor solid; the menu may cover
   the cursor.
   (b) Centered across, but in the half of the text area the primary cursor is not in, so the cursor
   stays in view; the menu moves when the cursor crosses the middle.
   (c) A panel along the bottom, above the echo area, full width, as transient and which-key draw.
5. **When it shows.**
   (a) At once, in the next frame, as hydra.
   (b) After an idle delay (`[menus] delay`, default 0), as which-key, so that a quick key after the
   menu's key draws nothing.
6. **Stay and exit heads' look.**
   (a) Two key faces: stay heads' keys in a red-like hue, exit heads' in the usual key colour, each
   checked for contrast in every theme, as hydra colours heads.
   (b) One key face, and a mark after a stay head's label (`↻`).
   (c) No difference.

### How menus are defined

7. **Where menus come from.**
   (a) Both: built-in menus as Rust tables, and `[menus.<name>]` in `config.toml` adding menus or
   replacing a built-in one by name, in one model. The defaults work without a file, and the
   author's menus stay in their file.
   (b) `config.toml` only, with no built-in menus: the toggle menu's heads ship as a sample in the
   docs.
   (c) Rust only: a menu change needs a rebuild, against ADR 0008.
8. **Reaching a menu.**
   (a) A key binds to a menu, `"C-c w" = { menu = "window" }`, as a new kind of keymap entry, and
   `M-x open-menu` completes over menu names.
   (b) Every menu, built-in or configured, is reached only through `open-menu`, with a key bound to it
   by an argument (`"C-c w" = ["open-menu", "window"]`): keys gain arguments, a larger change to
   ADR 0108.
   (c) One registered command per built-in menu (`toggle-menu`), and `open-menu` for configured
   ones; a configured menu cannot be bound to a key directly.
9. **The layout.**
   (a) Columns, each with an optional title and its heads as `key label` rows, as hydra's `:column`
   and transient's groups; oso lays them out.
   (b) (a), and also a free-form hint as hydra's docstring, a multi-line string with `_k_` where a
   head's key goes, drawn as written.
   (c) Only the free-form hint.

### Keys inside a menu

10. **The prefix argument.**
    (a) As hydra: a count typed before the menu's key goes to its first head; inside, `C-u`,
    `M-<digit>`, `M--`, and plain digits and `-` unless a head binds them, build one for the next
    head, shown on the title row.
    (b) Only `C-u` and `M-<digit>` inside; plain digits are always free for heads.
    (c) No prefix argument in a menu.
11. **Nested menus.**
    (a) A head opens another menu in place of this one, with no stack; a way back is a head that opens
    the parent.
    (b) A stack, 8 deep as prompts are, with a key that goes back (`DEL`).
12. **Toggle indicators.**
    (a) A command that toggles declares how to read its state, and any head that names it shows
    `[x]` or `[ ]`, with nothing to write in the menu's definition. Adds `Command::state` to the
    public API.
    (b) A head names a state from a fixed list (`state = "whitespace"`).
    (c) No indicators in this ADR.

### Around the menu

13. **Keyboard macros.**
    (a) Record the keys, as for any key (ADR 0113): a replay opens the menu and presses the heads.
    (b) Record the commands the heads ran, so a menu redefined since recording does not change the
    replay: macros become a mix of keys and commands.
14. **The agent channel.**
    (a) Opening a menu is `Person`; while one is open the channel waits as for a pending key, and
    `get-state` gains `menu`.
    (b) As (a), but the channel may run commands while a menu is open: they would act under a person
    who is choosing a head.
15. **A menu from a prompt.**
    (a) Not in a prompt (`Not in the minibuffer`); a head that opens a prompt closes the menu first.
    So a menu never covers completion, and needs no new layer.
    (b) A menu opens above a prompt, in new layers, and its heads act on the prompt's buffer, as
    embark's actions would.
16. **Hidden menus and the small layers.**
    (a) A menu may have no float (`show = false`), and `indent-rigidly`'s layer (ADR 0115 step 3) is
    built as a hidden built-in menu if this lands first; `C-x e`'s `e` and the prefix argument's
    digits stay as they are.
    (b) As (a), and `C-x e`'s `e` moves onto a hidden menu too.
    (c) No hidden menus; each small layer stays its own code.
17. **The perf scenario.**
    (a) Add `menu_open_p99_ms` and `menu_head_p99_ms` to `just perf`, report-only until the author
    sets thresholds, as ADR 0112 step 6 did. Needs the author's approval to edit `bench.rs`.
    (b) Only unit-level timing in `text_perf`-style tests; no `just perf` scenario.

## Steps

To fill in after the answers.

## Changes to accepted ADRs

- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), "M3's menus take layers above
  these": a menu never sits over a prompt, so it uses the float layers (Q15 (a)).
- [ADR 0008](0008-floating-ui.md), "the minibuffer, menus, completion popups and hover documentation
  are buffers shown in floating windows": a menu is a float with no window and no buffer, since
  nothing is typed or edited in it; its heads act on the window beneath.
- [ADR 0108](0108-the-user-configuration-file.md), "each entry is … either a command name or
  `false`": an entry may also be `{ menu = "<name>" }`, and version 1 gains `[menus]`.

## Consequences

- The author's hydras become data in `config.toml`, reloaded on save, with no code run.
- A menu costs one small float and a keymap built when it opens; nothing runs while none is open.
- Menus are reached only by the person, so the channel gains nothing and loses nothing.
- Transient's arguments and a which-key list of a prefix's continuations stay for later ADRs.

## Alternatives considered

- **Menus as buffers in a floating window (ADR 0008's first idea):** nothing is edited in a menu,
  and a window would take the selection from the window the heads act on.
- **A Lisp-like or scripted definition, as `defhydra`'s `:pre`, `:post` and arbitrary forms:** the
  configuration must not run code (ADR 0108); a head names one command.
- **Key sequences as heads:** hydra allows them, but single keys cover the common case and keep a
  menu's keymap flat; sequences can come later.
- **Commands that read the next key themselves:** a command never waits (ADR 0034).
