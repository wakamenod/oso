# 0117. Menus

- Status: Accepted
- Date: 2026-11-13

## Context

The roadmap puts "the menu component, porting the author's hydra menus" in M3
([roadmap](../roadmap.md)). On 2026-11-13 the author asked for it before
[ADR 0122](0122-agent-review.md)'s unified view, as **one component that combines hydra and
transient**, so that the review's scope menu (`C-x v r`) is built on it. Earlier ADRs fixed its
outline and left the rest here:

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
- [ADR 0075](0075-a-float-as-a-panel-attached-to-the-top-edge.md): prompts are a panel attached to
  the top edge; [ADR 0076](0076-completion-and-m-x.md): completion floats at the center, 62% wide
  and 18 candidate rows tall. `M-x` lists every command but those that read their key or belong to
  a keymap not active beneath it.
- [ADR 0056](0056-typing.md): "a menu that must take no text is the only keymap active, or gives
  itself a text command that refuses, as Emacs's `suppress-keymap` does." A question, isearch and
  query-replace each replace the global keymap with their own today (`dispatch/keymaps.rs`); a key
  isearch does not bind ends the search and runs as usual.
- [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md): `C-u`, `M-<digit>` and
  `M--` build a prefix argument, whose digits are a small keymap of their own while it is read.
  Two more small layers exist: `e` after `C-x e` runs the macro again
  ([ADR 0113](0113-query-replace-keyboard-macros-and-whitespace-cleanup.md)), and
  `indent-rigidly` with no count opens a layer of arrows ([ADR 0115](0115-indentation.md)).
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
- [ADR 0122](0122-agent-review.md): "**The menu** (`C-x v r`, a float of choices as in ADR 0117 once
  it lands, a completing prompt until then) offers each scope with its file count, counted on a
  worker and filled in as each count comes. It remembers the last choice per project." Its step 3
  (the unified view) waits for this ADR.
- The author's keyboard sends `英数` for Escape, so a menu cannot count on `ESC` as its way out.

### The author's menus

Read on 2026-11-13 from the author's Emacs configuration and their previous Emacs setup; nothing
of either is copied here.

- **Twelve hydras, 151 heads.** One plain hydra (red, 5 heads); eleven pretty-hydras that all say
  **blue by default** (a head closes the menu) with `:exit nil` on the heads that stay, **refuse a
  key they do not bind** (`:foreign-keys warn`), quit on `q`, and lay their heads out in **titled
  columns** under a title with an icon. None uses `:pre`, `:post`, `:timeout` or a hint
  docstring. They are drawn by hydra-posframe at the frame's center, with a 5-pixel border and
  8-pixel side padding.
- **What the heads are:** a toggle menu (19 heads, 16 marked as toggles with an on/off
  indicator); a window-and-buffer menu (30 heads, the largest, with arrow heads that resize by a
  **fixed count of 3** and stay); a jump menu (15); a structural-motion menu where every head
  stays (22); a search menu (8); a git-hunk menu (5, red); and menus for one mode each (outline
  notes, merge conflicts, a markup language's toggles, a flash-card mode), a two-head one and a
  one-head one.
- **Some heads are two keys:** `ZZ`, `gm`, `cc`, `df`, `dd`, `C-s C-s`. Some run a small lambda
  (save and bury; switch to a named buffer) or pass a fixed argument. Two menus use a key twice by
  mistake.
- **Of the 151 heads, oso has commands for about 25 today:** the visual aids, line numbers,
  truncation, trailing-whitespace cleanup, splitting, deleting, resizing and balancing windows,
  buffers, finding files, project search, and the avy jumps. Structural motion, git hunks,
  outline folding and the mode-specific ones name commands oso does not have.
- **Transients:** the previous setup's main menu has 47 suffixes in two rows of titled column
  groups and opens four submenus (transient sub-prefixes); two submenus exist only to hold one
  switch each. Its **review menu**, the model for `C-x v r`: eight scope choices whose labels are
  worked out as it draws (a label, a file count or `nothing` or `?`, and `(last)` on the last
  choice); choices that cannot apply are shown but refused (`:inapt-if`), one is hidden without
  `gh` (`:if`); a heading names the session and project; two switches (`-f` only these files, `-e`
  the other diff style, two infixes on one key with only one shown); one suffix that stays and
  changes which session the menu is about; and the cursor starts on the last choice, so `RET`
  repeats it. No option takes a value, nothing is saved with `C-x s`, and no magit transient is
  customised.
- **Also used:** which-key everywhere, embark's prefix help, and one `repeat-mode` map (`o`/`O`
  after the other-frame key).

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
amaranth, and every one of the author's pretty-hydras). Only hydra's hint and Helix's and Kakoune's
info boxes are drawn from data a user writes without code; hydra's needs Lisp for anything past the
docstring, and transient's dynamic labels need Lisp too.

## Decision

As the author chose (see [The author's choices](#the-authors-choices)): option (a) of every
question under [Questions](#questions), but Q1 (d) and Q24, which the author changed.

### What a menu is (Q2, Q3, Q4, Q5)

- **A menu is a named key layer with a float that lists it.** It has a title, groups of heads laid
  out as columns, and a default for its heads: `exit` or `stay`. While it is open, its keymap
  replaces the global one, as isearch's does, so no other binding is reached through it.
- **Three kinds of head (Q2):**
  - a **command head**: a key, a label and a registered command, with its own `stay` or `exit`;
  - a **switch** (transient's infix): a key and a label naming an on/off value the open menu holds,
    drawn as `[x]`/`[ ]`; pressing it flips the value and the menu stays. A command head's command
    reads the open menu's switches when it runs (`Ctx::switch("files")`), as transient's suffixes
    read `transient-args`;
  - a **menu head**: opens another menu (Q16).
  A head with no command only closes the menu, as hydra's `("q" nil)`.
- **`exit` heads close the menu and then run** (hydra's blue, transient's suffixes); **`stay` heads
  run and keep it open** (red). **The default is `exit` (Q3)**, as all of the author's
  pretty-hydras say, and a head says `stay`; a menu may say `default-head = "stay"` for a motion
  menu where every head stays.
- **A key the menu does not bind (Q4)** is refused and the menu stays, with `q to quit` in the echo
  area, as the author's pretty-hydras (`warn`) and transient do. A menu may say
  `foreign-keys = "exit"` (close it and run the key as usual, hydra's default, as a key ends
  isearch) or `"run"` (run it and stay, hydra's pink).
- **Quitting (Q5):** `C-g` always closes a menu (one level, Q16), first ending a pending prefix
  argument, in the `C-g` chain of ADR 0072. `q` closes it too unless a head binds `q`, as
  pretty-hydra's `:quit-key`; it is drawn on the last row as `q quit`.
- **A head stays only while nothing else takes the keys.** If its command reads an argument in a
  prompt, or opens a prompt, a question, isearch or query-replace, or starts a macro replay, the menu
  closes as that opens, so a menu never sits over a prompt and keys go where the person expects. A
  switch's value is read by the command before the menu closes.
- **A failing head keeps a `stay` menu open,** with its error in the echo area, so `n` at the end of
  the buffer in a motion menu does not throw the person out.
- **Text from an input method is a key the menu does not bind:** the IME is off in a menu, but a
  person who turns it on by hand gets the menu's `foreign-keys` rule for the committed text, through
  a text command on the menu's keymap (ADR 0056).
- **Hidden menus (Q21):** a menu may say `show = false`. It is the same key layer with no float, as
  Emacs's `repeat-mode` maps are, for layers such as `indent-rigidly`'s arrows.
- **Options with values** (transient's `--base=main`, read in a prompt and shown in the menu),
  saved values and levels are not in this ADR; a later one adds a fourth head kind. Nothing here
  closes that door.

### Heads (Q13, Q14)

- **A head's key is one key or a sequence of up to three (Q13),** in Emacs notation (`ZZ` is
  written `"Z Z"`, transient's switches `"- f"`), as the author's hydras and transients use. The
  menu's keymap holds the sequences as prefixes; the echo area shows a pending first key as it does
  for `C-x`, and a key that ends no sequence is a foreign key.
- **A head may give a fixed count (Q14):** `count = 3` runs the command as if `C-u 3` had been
  typed, for the author's resize heads. Nothing else: a head names one command, and the small
  lambdas (save and bury, switch to a named buffer) become registered commands when they are
  wanted.

### Live content (Q17, Q18)

- **A built-in menu may have a describe function (Q17),** a plain Rust `fn(&Editor, &Open) -> Notes`
  called when the menu is drawn, once after each key and when a worker's report asks for a redraw.
  For each head it may give a **note** after the label (`12 files`, `…` while a count runs,
  `(last)`), mark it **unavailable** with a reason (drawn dim; its key says the reason and the menu
  stays), or **hide** it (its key is then a foreign key); and it may add a line under the title (the
  review's session and project). It reads only `Editor` and the open menu, so it does no I/O; the
  review's counts come from its worker into the editor's review state, as ADR 0122 already says.
- **A default head:** a menu may name one head that `RET` runs, and a describe function may change
  it; the review menu makes it the last choice, so `RET` repeats it, and draws that head's row
  marked. A menu in `config.toml` may name a fixed one.
- **Configured menus have no describe function,** since the file runs no code; they get the toggle
  indicators (Q10) and switches.
- **A stay head may change what the menu is about:** the review menu's `S` (review another
  session) is a `stay` head whose command sets the subject in the review state the describe
  function reads, so the next draw shows the new session's counts. Nothing new is needed for it.
- **Switch values start off each time a menu opens (Q18).** What a choice should remember is the
  command's: the review remembers its last scope per project (ADR 0122).

### Where it is drawn (Q6, Q7, Q8, Q9, Q10)

- **At the center of the tiled area, fitted to its content (Q6),** as hydra-posframe draws: as wide
  as its columns and as tall as its rows of columns, plus the title, a row of padding and the ring.
  It keeps completion's look: the 5-point border a little darker than the fill, all four corners
  rounded (ADR 0076), and never closer than 2 columns or 1 row to an edge.
- **The selected window stays the tiled one, with its cursors drawn as usual (solid),** its region
  and band too, because the heads act on it. A menu is not a window and has no buffer. It may cover
  the cursor; the view does not move for it.
- **It shows at once (Q7),** in the frame after the key that opened it, as hydra's hint does; no
  idle delay. During a macro replay it is not drawn, as which-key.nvim does, so a replay does not
  flash it; a replay that ends with a menu open draws it then.
- **Layout (Q9):** the title, then **rows of columns**, each column with an optional title over a
  `╌` rule and its heads one per row as `key  label  note`, 2 columns apart. A group says
  `row = 2` to start a second row, as the previous setup's main menu has two.
- **Faces (Q8):** a head's key is in `MenuKey`, a chip like a question's (`Face::Key`); the key of a
  `stay` head in `MenuKeyStay`, hydra's red as a hue the theme chooses; a switch's key in
  `MenuKeySwitch` and its `[x]` in the same face when on; titles in `MenuTitle`; notes and
  unavailable heads in `MenuNote` (dim). All pass ADR 0089's checks: 4.5 against the float, and 3.0
  between the key faces.
- **A toggle (Q10)** shows `[x]` or `[ ]` before its label. A command that toggles declares how to
  read its state (`Command::state`, a plain `fn(&Editor) -> bool`), and a head naming such a
  command shows it with no word in the menu's definition. The state is read when the menu is
  drawn.
- **The prefix argument** shows at the end of the title row, `C-u 5-`, as the echo area shows it
  today.
- **A frame too small** gives up the padding, then the column titles, then moves columns of a row
  onto a new row below, then cuts the longest columns with `…` on their last row, which says how
  many heads are hidden (`… 3 more`). A hidden head still runs from its key.
- A label and a note are drawn through the same path as buffer text, so bidirectional controls and
  invisible characters in them are boxed (ADR 0026, ADR 0035), and widths are counted in cells.
- **Layers:** a menu never coexists with a prompt (Q20), so it uses `Layer::FLOAT` and
  `FLOAT_CONTENT` with pane ids of its own, and the renderer needs no new layer. ADR 0072's "layers
  above" is not needed yet.

### How menus are defined (Q11, Q12)

- **A menu's contents are the user's configuration (Q11).** Built-in menus are only defaults,
  written as Rust tables in `bindings/menus.rs` so that oso works without a file; user menus are
  `[menus.<name>]` in `config.toml`; both are one model (`oso_core::menu::Definition`).
  - A `[menus.<name>]` with a new name **adds** a menu.
  - One with a built-in menu's name **replaces** that menu whole: its title, heads, keys and
    rules are the file's, and nothing of the built-in one is merged in.
  - `[menus.<name>] = false` **removes** a built-in menu, as `false` unbinds a key.
  - Saving the file reloads every menu (below); no rebuild, and no code runs.
- In the file:

  ```toml
  [menus.windows]
  title = "Windows"
  foreign-keys = "warn"             # the default; or "exit" or "run"
  # default-head = "exit"           # the default; or "stay"

  [[menus.windows.groups]]
  title = "Size"
  heads = [
    ["<left>", "shrink-window-horizontally", "narrower", { stay = true, count = 3 }],
    ["<right>", "enlarge-window-horizontally", "wider", { stay = true, count = 3 }],
    ["b", "balance-windows", "balance", { stay = true }],
  ]

  [[menus.windows.groups]]
  title = "Split"
  heads = [
    ["-", "split-window-below", "below"],
    ["/", "split-window-right", "right"],
    ["t", { menu = "toggles" }, "toggles…"],
  ]

  [[menus.windows.groups]]
  title = "Buffer"
  row = 2
  heads = [
    ["b", "switch-to-buffer", "switch"],    # refused: "b" is used twice in this menu
    [",", "previous-buffer", "previous", { stay = true }],
  ]
  ```

  A head is `[key, action, label]` with an optional table of `stay`, `count` and `default`. The
  action is a registered command name, `{ menu = "<name>" }`, `{ switch = "<name>" }`, or `false`
  for a head that only closes the menu.

- **The same checks as keys:** a key a person cannot type, `C-g`, an unknown command, a command only
  a dev build has, an unknown menu, a key used twice in one menu (or a key that is also the start of
  another head's sequence), and a value over the limits below are refused with their line, and the
  rest of the menu loads. A menu with a refused head still opens, without it.
- **Read with the rest of the file, off the UI thread,** against the thread's registry, and swapped in
  between keys as the user keymap is. **A reload never changes an open menu:** it keeps the
  definition it opened with, and the next opening takes the new one.
- **Reaching a menu (Q12):** a key binds to a menu in `[keys.global]` as `"C-c t" = { menu =
  "toggles" }`. `Keymap` gains a third kind of entry, `Menu(MenuId)`, beside a command and a
  prefix; the resolver returns `Resolution::Menu`, and the dispatcher opens it. `M-x open-menu`
  reads a menu's name with completion over every menu (`ArgSpec::Menu`). `where-is` and the
  completion's key column show a menu's key for `open-menu`'s candidates.
- **Limits:** 256 menus, 128 heads per menu, 8 columns per row, 3 rows, keys of 3 key presses,
  counts up to 1,000, labels and titles of 64 cells (and 512 bytes, see [As built](#as-built)),
  menu and switch names of 64 bytes in kebab case. Past them an entry is refused with its line.

### The prefix argument (Q15)

- A prefix argument typed before the menu's key goes to the first head run. **Inside the menu,
  `C-u`, `M-<digit>` and `M--` build one for the next head; plain digits and `-` are ordinary keys**
  for heads to bind (the author's menus bind `0` to `3` and `-`), as in transient. A `stay` head
  consumes it, as any command does; a head's `count` replaces it.
- A head that opens another menu passes the prefix argument on.

### Nesting (Q16)

- **A head that opens another menu pushes it over the current one,** as transient's sub-prefixes:
  the float shows the new menu, `C-g` goes back one level, and an `exit` head closes every level.
  At most 8 levels, as prompts; a menu already on the stack cannot be pushed again. Each level keeps
  its own switches.

### Around the menu (Q19, Q20, Q22)

- **Keyboard macros record the keys (Q19),** as for any key: a replay opens the menu and presses its
  heads, without waiting for frames. A menu redefined since recording does what it does now, as a
  rebound key does (ADR 0113). A replay that ends with a menu open leaves it open.
- **The agent channel (Q22):** `open-menu` is `Person`, since it reads keys, and so is every menu,
  which no channel command can open. While a menu is open the person is in the middle of something,
  so `run`, `open` and `insert` are refused as during a pending key ("the person is in a menu"), and
  a question the channel caused waits until it closes. `get-state` gains `menu`: the open menu's
  name, or null. Heads run commands the person pressed, so a head may name a `Person` command, as a
  key may (ADR 0108).
- **An agent never opens a menu.** It needs none: it runs a head's command directly with `run`,
  under that command's own reach (ADR 0085), so a menu gives an agent nothing it could not do
  without one, and takes nothing away. A switch is the person's; a command that reads switches
  gets none from `run` (all off) unless its `ArgSpec` lets the channel pass them.
- **In a prompt (Q20), a menu does not open:** `Not in the minibuffer`, as `C-x b` says there. During
  isearch and query-replace, a menu's key is a key they do not bind, which ends them first.
- **Multiple cursors:** a head's command acts on the selection set, as from any key. Nothing new.
- **The IME** is off while a menu is open: `wants_ime` gains the menu's condition (`app/ime.rs`), as
  open-questions.md asks. A composition open when the menu's key arrives has ended by then, since
  the key went to the dispatcher.
- **Secret buffers:** a menu shows its definition, the toggle states and its describe function's
  notes. A describe function must not show a buffer's text; the review menu shows counts and
  names only.
- **`oso dev` restart:** an open menu is not carried; the new instance starts with none.
- **Frame resize:** the layout is worked out again, as a prompt's is.

### The key list (Q24, Q26–Q32)

The author needs which-key, and wants it to behave as which-key does, not drawn in the menu's
centred float. It is not a menu: nothing in it is chosen, the keys go on as usual, and it closes
with the next key that is not a paging key.

- **When (Q26):** after a prefix key (`C-x`, `C-c`, `C-x 4`, `C-h`) has waited 1.0 s with no
  next key, as which-key's `which-key-idle-delay`. Once a list is shown, the list of a further
  prefix (`C-x 4` after `C-x`) replaces it at once. `[which-key] delay` in `config.toml` sets the
  delay, and `[which-key] enabled = false` turns it off. It is not shown during a macro replay, and
  not for a prefix argument (`C-u`), whose keys are digits.
- **Where (Q27):** a panel along the bottom of the frame, just above the echo area, the full width
  of the frame, as tall as its content up to a quarter of the frame's rows. It is drawn over the
  bottom rows of the tiled windows, which keep their size and views, so showing it moves no text;
  it may cover the cursor. It uses `Layer::FLOAT` and `FLOAT_CONTENT` with pane ids of its own, and
  never overlaps a prompt, which is at the top edge (ADR 0075). Its look is the float's fill with a
  1-row rule above, in `Float` and `FloatBorder`.
- **What it lists (Q29):** every key that may follow the typed prefix, as the dispatcher would
  resolve it now: the user layer, the buffer's mode keymap and the global one, each key once, with
  the binding that wins. A key bound to `false` is not listed. A key that is itself a prefix shows
  as `+prefix` (or its label, below); a key bound to a menu shows as `+<menu>`. Shift-translated
  keys are not listed apart.
- **How each entry reads (Q30):** `key → name`, the command's registered name, as which-key shows
  them. A prefix may be given a label in `config.toml` (`[which-key.labels] "C-x 4" = "other
  window"`), as which-key's `which-key-add-key-based-replacements`. Entries are sorted by key, plain
  keys first, then `C-`, then `M-`, and laid out in columns down then across,
  each column as wide as its widest entry.
- **Paging (Q28):** a list taller than the panel is split into pages, and the last row says
  `1 of 3 · C-h for more`. While the list is shown, `C-h` asks which-key's question on the echo
  line: `n` next page, `p` previous, `u` undoes the last key of the prefix, `C-g` quits. Any other
  key goes on as usual through the prefix.
- **`C-h` before the list shows (Q31):** `C-h` after a prefix shows the list at once, instead of
  waiting for the delay, as `which-key-show-early-on-C-h` does; a second `C-h` pages as above.
  Where the prefix's keymap binds `C-h` itself, the binding wins and the list waits for the delay.
- **Faces (Q32):** keys in `WhichKeyKey`, prefixes and menus in `WhichKeyPrefix`, the arrow and
  the page line in `WhichKeyDim`, each checked against the float's fill in every theme as ADR 0089
  asks.
- **Cost:** the list is built once when it shows, from the keymaps the dispatcher already holds
  (a few hundred keys at most), and laid out again only when the frame resizes or it pages. A key
  that arrives before the delay costs one timer, as the prefix's echo does.
- **The channel** is unchanged: a pending prefix already makes it wait (ADR 0085), and the list
  shows nothing of a buffer.

### The first menus (Q1, Q25)

The author chose (Q1 (d)) these first menus, built in as Rust tables:

- **`review`** on `C-x v r`, ADR 0122's scope menu, built in ADR 0122 step 3 on this component:
  the scopes as `exit` heads with their counts as notes and `(last)` on the default head,
  unavailable scopes dim with their reason, the PR scope hidden without `gh`, the session and
  project under the title, the switches step 3 settles with the author, and a `stay` head to
  change the session.
- **`toggles`** on `C-c t`: whitespace, current line, cursor flash, line numbers, truncation, syntax
  highlighting, the theme pair, and trailing-whitespace cleanup (an `exit` head), with toggle
  indicators; heads the author's toggle menu has and oso does not stay out until their commands
  exist.
- **`windows`** (the author's window-and-buffer menu, about 15 of its 30 heads): resize by 3,
  balance, split, delete, delete others, the buffer commands and finding files.
- **`jumps`** (the author's jump menu's avy heads and `goto-line`).
- **`search`** (the author's search menu's heads oso has today: project search and finding a
  project's file; the rest join as their commands come).
- The rest wait for their commands; the author may write any of them in `config.toml` meanwhile.
- **Default keys (Q25):** only `C-c t` (toggles) and `C-x v r` (review). The windows, jump and
  search menus have no key by default; the author binds them in `config.toml`, and `M-x open-menu`
  reaches every menu.
- **These are defaults, not the author's menus fixed in code.** Each one is a starting point that
  `config.toml` can replace whole by giving a `[menus.<name>]` of the same name, or remove with
  `[menus.<name>] = false`; any other `[menus.<name>]` adds a menu. Only what a file cannot express
  (the review menu's describe function) stays in Rust, and even that menu's heads and keys can be
  replaced, losing its live notes.

### Where the code lives

- **`oso-core`:** a new public module `menu`: `Definition`, `Group`, `Head`, `HeadAction`
  (`Command`, `Switch`, `Menu`, `Quit`), `Stay`, `ForeignKeys`, `MenuId`, `Notes`, `Note`, `Open`
  (the open menu's switches and default head), and `menu::layout`, a pure function from the
  definition, the toggle states, the notes and the text area's size to the float's cells, as
  `frame::float_regions` is. `keymap` gains `Entry::Menu`, `Binding::Menu` and `Resolution::Menu`;
  `command` gains `ArgSpec::Menu`, `Command::state`, and `Ctx::switch`. A head names a `CommandId`
  and `keymap` names a `MenuId`, so in `guardrails/layers.txt` `menu` gets a line of its own between
  `keymap` and `command`: it uses `command`, `editor`, `key`, `frame` and `minibuffer` (for
  `cells_of`), and only `keymap` uses it. The public items are listed for the author's approval in step 1.
- **The open menus are the dispatcher's,** as a pending prefix argument is: the stack of
  definitions, the keymap built from each when it opens, its switches, and a prefix argument being
  typed. `Editor` does not hold them. The app reads them to draw, to ask `wants_ime`, and for the
  channel's wait and `get-state`.
- **`oso`:** `bindings/menus.rs` (the built-in menus), `config` reads `[menus]` and `{ menu = … }`
  keys, `commands/menus.rs` (`open-menu`), `dispatch/menus.rs` (the open menus and their rules), and
  `app/draw` draws them.
- **`oso-render`:** the five faces and their contrast pairs, and the key list's three. No new
  pipeline.
- **The key list:** `dispatch/which_key.rs` collects a prefix's continuations from the layers the
  dispatcher resolves through and holds the shown list and its page; `app/draw` draws the panel;
  its delay is a timer of the app's loop, as the cursor flash's is.

### Tests and measures

- Unit tests of the layout (sizes, rows, small frames, moving columns, `…`), of each `stay`/`exit`/
  `foreign-keys` rule, of key sequences, switches, counts, the default head, notes and unavailable
  heads, of the prefix argument, of the stack, and of the configuration's checks and limits, in
  `oso-core` and the dispatcher's tests, with no window.
- An end-to-end flow for the key list: type `C-x`, wait, read the panel, page, then a key that
  runs, with a screenshot.
- An end-to-end flow: open a built-in menu, run a `stay` head twice, flip a switch, open a submenu
  and go back with `C-g`, an `exit` head, a foreign key, and `C-g`, with a screenshot.
- A `just perf` scenario: open a menu and press a `stay` head that moves the cursor, report-only
  (`menu_open_p99_ms`, `menu_head_p99_ms`) until the author sets thresholds (Q23).

### The cost of drawing

A menu of 4 columns of 10 heads is about 40 rows of 30 cells, under 1,500 cells. Its labels do not
change while it is open, so each row's shaped text is kept by its pane id across frames, as a
prompt's label is, and only toggles, switches, notes and the prefix row are shaped again after a key
or a report. A describe function reads state the editor already holds and runs once per frame
drawn. The layout is worked out when the menu opens and when the frame changes size, a pass over at
most 128 heads. Expected: under 0.1 ms on the UI thread for the layout, and a frame no costlier than
one with completion's 18 rows. The perf scenario checks it.

## Questions

As put to the author, the recommended option first; the answers are under
[The author's choices](#the-authors-choices).

### What to port

1. **Which menus come first.** The author's Emacs has twelve hydras (151 heads); oso has commands
   for about 25 of those heads today (aids, line numbers, truncation, whitespace cleanup, windows,
   buffers, files, project search, avy). ADR 0122's review needs `C-x v r`.
   (a) Built in: the review scope menu (built in ADR 0122 step 3), a toggle menu (8 heads oso
   has), a windows-and-buffers menu (about 15 of the author's 30 heads), and a jump menu (avy and
   `goto-line`). The other hydras wait for their commands; the author may write any in
   `config.toml` meanwhile.
   (b) Built in: only the review menu and the toggle menu; the author writes the windows, jump and
   search menus in their `config.toml`.
   (c) Only the review menu; every hydra is the author's to write in `config.toml`.
   (d) As (a), plus the search menu (only project search and find file exist today, 2 of its 8
   heads).

### What a menu is

2. **The model: how much of transient.** The review scope menu needs on/off switches (the previous
   setup's "only these files" and "other diff style"); no menu the author uses has an option with
   a value, and nothing is saved.
   (a) Hydra's heads (stay or exit) and transient's **switches**: on/off values the open menu
   holds, drawn `[x]`/`[ ]`, which a head's command reads when it runs. Options with values,
   saved values and levels wait for a menu that needs them (magit's).
   (b) (a), and also **options with values** now (`--base=main`, read in a prompt from a head and
   shown in the menu), with values saved per session. Ready for magit, but larger, with no user
   yet.
   (c) Hydra's heads only; the review's switches become separate heads (`D` and `F` for "session
   changes, only these files") or questions after the choice.
3. **What a head does by default.** All eleven of the author's pretty-hydras close after a head
   (blue) and mark the heads that stay; the plain hydra stays (red); transient's suffixes close.
   (a) Close (exit) by default; a head says `stay`; a menu may say `default-head = "stay"` (for a
   motion menu where every head stays).
   (b) Stay by default (hydra's red); a head says `exit`.
4. **A key the menu does not bind.** The author's pretty-hydras all refuse one (`warn`); transient
   refuses one; hydra's default closes the menu and runs it.
   (a) Refused with `q to quit` in the echo area, and the menu stays; a menu may say `exit` (close
   and run, as a key ends isearch) or `run` (run and stay, hydra's pink).
   (b) Close and run by default (hydra's default, as isearch); a menu may say `warn` or `run`.
   (c) Always refused; no per-menu choice.
5. **Quitting.** The author's keyboard sends `英数` for Escape, so `ESC` cannot be the way out.
   (a) `C-g` always closes one level; `q` closes the menu too unless a head binds `q` (as
   pretty-hydra's quit key), drawn as `q quit` on the last row.
   (b) Only `C-g` and the heads the definition gives (`["q", false, "quit"]`).
   (c) `C-g`, and `q` in every menu, refusing a head on `q`.

### How it looks

6. **Where it is drawn.**
   (a) At the center, fitted to its content, as hydra-posframe draws today (ADR 0026), with
   completion's border; the window beneath stays selected and its cursor solid; the menu may cover
   the cursor.
   (b) Centered across, but in the half of the text area the primary cursor is not in, so the cursor
   stays in view; the menu moves when the cursor crosses the middle.
   (c) The top-edge panel prompts use (ADR 0075), full width.
   (d) A panel along the bottom, above the echo area, full width, as transient and which-key draw.
7. **When it shows.**
   (a) At once, in the next frame, as hydra.
   (b) After an idle delay (`[menus] delay`, default 0), as which-key, so that a quick key after the
   menu's key draws nothing.
8. **How heads' kinds look.** The author's Emacs recolours hydra's red, blue, pink, teal and
   amaranth faces.
   (a) Three key faces: stay heads' keys in a red-like hue, exit heads' in the usual key colour,
   switches' in a third hue that also marks `[x]`; each checked for contrast in every theme.
   (b) One key face, a mark after a stay head's label (`↻`), and `[x]`/`[ ]` for switches.
   (c) No difference but `[x]`/`[ ]`.
9. **The layout.** The author's hydras are one row of titled columns; the previous setup's main
   menu is two rows of titled column groups.
   (a) Rows of columns: each group has an optional title over a `╌` rule and its heads as `key
   label note` rows; a group may start a new row (`row = 2`); oso lays them out.
   (b) One row of columns only; oso wraps them when the frame is narrow.
   (c) (a), and also a free-form hint as hydra's docstring, a multi-line string with `_k_` where a
   head's key goes, drawn as written.
10. **Toggle indicators.** 16 of the 19 heads in the author's toggle menu show on/off.
    (a) A command that toggles declares how to read its state, and any head that names it shows
    `[x]` or `[ ]`, with nothing to write in the menu's definition. Adds `Command::state` to the
    public API.
    (b) A head names a state from a fixed list (`state = "whitespace"`).
    (c) No indicators in this ADR.

### How menus are defined

11. **Where menus come from.**
    (a) Both: built-in menus as Rust tables, and `[menus.<name>]` in `config.toml` adding menus or
    replacing a built-in one by name, in one model. The defaults work without a file, and the
    author's menus stay in their file.
    (b) `config.toml` only, with no built-in menus: the toggle menu's heads ship as a sample in the
    docs. The review menu, which needs live counts, could not be built this way.
    (c) Rust only: a menu change needs a rebuild, against ADR 0008.
12. **Reaching a menu.**
    (a) A key binds to a menu, `"C-c t" = { menu = "toggles" }`, as a new kind of keymap entry, and
    `M-x open-menu` completes over menu names.
    (b) Every menu, built-in or configured, is reached only through `open-menu`, with a key bound to it
    by an argument (`"C-c t" = ["open-menu", "toggles"]`): keys gain arguments, a larger change to
    ADR 0108.
    (c) One registered command per built-in menu (`toggle-menu`), and `open-menu` for configured
    ones; a configured menu cannot be bound to a key directly.
13. **Heads of more than one key.** The author's hydras have six (`ZZ`, `gm`, `cc`, `df`, `dd`,
    `C-s C-s`), and transient's switches are written `-f`, `-e`.
    (a) A head's key is one key or a sequence of up to three; the menu's keymap holds them as
    prefixes, and the echo area shows a pending first key.
    (b) One key only; the ported heads get new single keys, and switches are `F`, `E`.
14. **Heads with a fixed argument.** The author's resize heads pass 3 and some heads pass a fixed
    number; two heads run a small lambda (save and bury; switch to a named buffer).
    (a) A head may give a fixed count (`count = 3`), as if `C-u 3` were typed; nothing else, and the
    lambdas become registered commands when they are wanted.
    (b) A count, and also a fixed answer for a command that reads one text argument
    (`answer = "*scratch*"`), so a head can switch to a named buffer.
    (c) No fixed arguments; resize heads move by one.

### Keys inside a menu

15. **The prefix argument.** The author's heads never take one; two of their menus bind digits
    (`0`–`3`, `1`–`4`) and `-` as heads.
    (a) A count typed before the menu's key goes to its first head; inside, `C-u`, `M-<digit>` and
    `M--` build one for the next head, shown on the title row; plain digits and `-` are ordinary
    keys for heads, as in transient.
    (b) As hydra: also plain digits and `-` build a count unless a head binds them.
    (c) No prefix argument in a menu.
16. **Submenus.** The previous setup's main menu opens four submenus; the hydras do not nest.
    (a) A stack, as transient's sub-prefixes: a head opens a submenu over this one, `C-g` goes back
    one level, an exit head closes every level; 8 deep, each level with its own switches.
    (b) Replace, as hydra: a head opens another menu in place of this one, with no way back but a
    head that opens the parent.

### Live content

17. **Labels worked out as the menu draws.** The review menu shows each scope's file count as a
    worker fills it in, `(last)` on the last choice, scopes that cannot apply dim (and refused with
    a reason), one hidden without `gh`, the session under the title, and starts with `RET` on the
    last choice.
    (a) A built-in menu may have a describe function (Rust, reading only the editor's state) that
    gives each head a note, marks it unavailable with a reason or hides it, adds a line under the
    title, and names the head `RET` runs. A configured menu has none (the file runs no code), but
    may name a fixed `RET` head.
    (b) As (a), and a configured menu may also name notes from a fixed list of sources
    (`note = "file-count"`).
    (c) No live labels; the review shows counts in the echo area as they come, and has no `RET`
    head.
18. **Switch values over time.** The author has saved no transient value.
    (a) Every switch starts off each time its menu opens; a command that should remember a choice
    does so itself, as the review remembers its last scope per project.
    (b) Kept per menu until oso quits, so a switch stays on the next time the menu opens.
    (c) Kept per menu and saved to a file, as transient's `C-x C-s`.

### Around the menu

19. **Keyboard macros.**
    (a) Record the keys, as for any key (ADR 0113): a replay opens the menu and presses the heads.
    (b) Record the commands the heads ran, so a menu redefined since recording does not change the
    replay: macros become a mix of keys and commands.
20. **A menu from a prompt.**
    (a) Not in a prompt (`Not in the minibuffer`); a head that opens a prompt closes the menu first.
    So a menu never covers completion, and needs no new layer.
    (b) A menu opens above a prompt, in new layers, and its heads act on the prompt's buffer, as
    embark's actions would.
21. **Hidden menus and the small layers.** The author uses one repeat map (`o`/`O` after the
    other-frame key).
    (a) A menu may have no float (`show = false`), and `indent-rigidly`'s layer (ADR 0115) moves onto
    a hidden built-in menu; `C-x e`'s `e` and the prefix argument's digits stay as they are.
    (b) As (a), and `C-x e`'s `e` moves onto a hidden menu too.
    (c) No hidden menus; each small layer stays its own code.
22. **The agent channel.**
    (a) Opening a menu is `Person`; while one is open the channel waits as for a pending key, and
    `get-state` gains `menu`.
    (b) As (a), but the channel may run commands while a menu is open: they would act under a person
    who is choosing a head.
23. **The perf scenario.**
    (a) Add `menu_open_p99_ms` and `menu_head_p99_ms` to `just perf`, report-only until the author
    sets thresholds, as ADR 0112 step 6 did. Needs the author's approval to edit `bench.rs`.
    (b) Only unit-level timing in `text_perf`-style tests; no `just perf` scenario.
24. **A list of a prefix key's continuations (which-key).** The author runs which-key everywhere.
    Answered: the author needs it, as which-key behaves (a panel along the bottom that lists a
    waiting prefix's keys), not in the menu's centred float; it is a step of this ADR, and Q26–Q32
    settle it.
25. **Keys for the built-in menus.** In oso, Command is Meta and Option is Super (`s-`); the
    author's Emacs binds its menus to `C-c t`, `s-s`, `s-g`, `s-c` and `M-s`, and oso already uses
    `M-s r` for project search.
    (a) Bind only `C-c t` (toggles) and `C-x v r` (review) by default; the author binds the windows
    and jump menus in `config.toml` to whichever physical keys their `s-` was.
    (b) The author names the keys now, and they become the defaults.
    (c) No default keys but `C-x v r`; everything else through `M-x open-menu` or `config.toml`.

### The key list (which-key)

26. **When it shows.** which-key's default delay is 1.0 s.
    (a) After a prefix has waited 1.0 s; once shown, a further prefix's list replaces it at once;
    `[which-key] delay` and `enabled` in `config.toml`; never during a macro replay or for `C-u`.
    (b) The same with 0.5 s.
    (c) At once after every prefix key, with no delay.
27. **Where it is drawn and how tall.**
    (a) A full-width panel at the bottom of the frame, above the echo area, as tall as its content
    up to a quarter of the frame, drawn over the windows' bottom rows: no window moves or resizes,
    and it may cover the cursor.
    (b) The same place and height, but the tiled windows shrink while it shows, as which-key's
    side window makes Emacs's windows do: nothing is covered, but views jump up and back.
    (c) The same, up to half the frame.
28. **Paging a list taller than the panel.**
    (a) As which-key: the last row says `1 of 3 · C-h for more`; `C-h` asks on the echo line, `n`
    next page, `p` previous, `u` undo the last prefix key, `C-g` quit.
    (b) `C-h` alone goes to the next page and wraps around; no undo.
    (c) No paging: the list is cut with `… N more`, and `C-h` (Q31) opens the whole list in
    completion.
29. **What it lists.**
    (a) Every key that may follow, as the dispatcher would resolve it now (the user layer, the
    buffer's mode keymap, the global one), each key once with the binding that wins; keys bound
    to `false` left out; sub-prefixes as `+prefix`, menus as `+<menu>`.
    (b) Only the global keymap and the user layer, so a mode's keys are not listed.
30. **How an entry reads.**
    (a) `key → command-name`, as which-key; a prefix may get a label in `config.toml`
    (`[which-key.labels] "C-x 4" = "other window"`); sorted by key, columns down then across.
    (b) `key → summary`, the first line of the command's docstring, cut to fit: longer and fewer
    columns.
    (c) The name, and the summary as well when the panel is wide enough.
31. **`C-h` after a prefix.** The author's Emacs sends it to embark's prefix help, a completion
    list of the prefix's keys.
    (a) It shows the list at once, without waiting for the delay, and a second `C-h` pages
    (which-key's `which-key-show-early-on-C-h`). A prefix that binds `C-h` itself keeps it.
    (b) It opens the completion float (ADR 0076) over the prefix's keys and commands; `RET` runs
    the chosen one, as embark's prefix help.
    (c) Nothing new: `C-h` after a prefix stays an undefined key.
32. **Faces.**
    (a) Three: `WhichKeyKey` for keys, `WhichKeyPrefix` for prefixes and menus, `WhichKeyDim` for
    the arrow and the page line, each contrast-checked against the float's fill in every theme.
    (b) Reuse the menu's `MenuKey` and `MenuNote`, with no faces of its own.

## Steps

One PR per step; each passes `just ci` and, where it touches the window, `just e2e`.

1. **The model and the configuration.** `oso_core::menu` (definitions, heads, switches, key
   sequences, counts, limits, and the pure layout), `Entry::Menu`, `Binding::Menu` and
   `Resolution::Menu` in `keymap`, `ArgSpec::Menu`, `Command::state` and `Ctx::switch` in
   `command`; `config` reads `[menus]` and `{ menu = … }` keys with their checks. The public items
   are listed for the author's approval when the step starts, and the `guardrails/layers.txt` line
   for `menu` waits for that approval. Unit tests only; no UI.
2. **The open menu and its float.** The dispatcher's stack and its rules (exit, stay, foreign keys,
   `q` and `C-g`, sequences, the prefix argument, counts, failing heads, closing as a prompt
   opens), `open-menu`, describe functions with notes, unavailable and hidden heads and the
   default head, drawing with the five faces and their contrast pairs, `wants_ime`, the channel's
   wait and `get-state`, macros. An end-to-end flow on a test menu. `ui.md`'s Menus section and
   open-questions.md's `wants_ime` item. **ADR 0122 step 3 builds the review menu on this step.**
3. **The built-in menus:** `toggles` on `C-c t` (with `Command::state` on the toggling commands),
   `windows`, `jumps` and `search`, and a sample `[menus]` in the configuration's docs.
4. **Hidden menus:** `show = false`, and `indent-rigidly`'s arrow layer (ADR 0115) moved onto a
   hidden built-in menu.
5. **The perf scenario:** `menu_open_p99_ms` and `menu_head_p99_ms` in `just perf`, report-only.
   The `bench.rs` registration is a guard-file edit, asked for when the step starts.
6. **The key list (which-key)** as Q26–Q32 (a) settle it: the delay, the bottom panel, what it lists,
   labels from `[which-key.labels]`, paging, `C-h`, its faces, an end-to-end flow. A public way to
   list a prefix's entries in `keymap`, if the dispatcher needs one, is listed for approval when the
   step starts.

Steps 3 to 6 follow step 2 in any order.

## As built

### Step 1: the model and the configuration (2026-11-13)

- **Labels and titles are also capped at 512 bytes**, a change to the limits above that the
  author chose after review, since a count of cells alone does not bound a label's bytes. One
  over either cap is refused as a long label or title is; the bytes are checked first.
- **`RET` is kept for the default head:** a head whose keys begin with `RET` is refused in a menu
  that has a default head, and a head cannot be made the default while another head's keys begin
  with `RET` (`MenuError::RetReserved`). A head on `RET` may itself be the default head.
- **Options that do nothing warn:** `stay` on a switch, on a head that opens a menu or on one that
  only closes it, and `count` on a switch or on a head that only closes the menu, are ignored with
  their line, as an unknown name is. `count` on a head that opens a menu is kept: it passes the
  count on, as a prefix argument typed before it would be.
- **A menu's heads past 128 are not read:** the first one says the limit with its line, and it and
  the rest of that menu's heads and groups are skipped.
- **A column is cut a row at a time, the longest first,** so a short column in another row keeps
  the rows the frame still has, and `… N more` is sized for the number it says.
- `Definition`'s default head is `ret_head` (`set_ret_head`), and its default for heads that say
  nothing is `default_stay` (`set_default_stay`); a describe function's is `Notes::ret_head`. The
  file's names stay `default = true` and `default-head`.

### Step 2: the open menu and its float (2026-11-13)

The author approved the public items and chose (a) for each of step 2's questions (Q1–Q14,
asked through the coordinator). After review, the author added `Column::group` and chose to keep
`Describe`'s signature and the layout's measuring as they are (three more questions, each (a)).

- **The float is completion's panel exactly** (`Panel::border`): the text's background, a 5-point
  border a little lighter than it, square corners and no shadow. This corrects "all four corners
  rounded" and "a little darker" above, which misread completion's look.
- **Chips:** `Panel::keys` gives each chip its face. A menu's chips reach 3 points past the key's
  cells, which hold only the key. Each chip's fill is at least 3:1 against the panel, as a mark,
  and its key at least 4.5:1 on it; the three kinds of key are told apart by their fills, which a
  test keeps apart. This is what "3.0 between the key faces" means. A switch's `[x]` sits on a
  chip in `MenuKeySwitch` while it is on; `[ ]` is plain in `MenuNote`, and a command's toggle
  shows `[x]`/`[ ]` in the text's face. `RET`'s head is marked by a box behind its row in its own
  column (`Panel::marked`).
- **The layout** gives each shown head a `Row` with its key's, check's, label's and note's cells.
  Labels and notes each start in a column of their own per group. A column gives its rule's row
  and the row of `… N more`, and the layout takes the prefix argument's width, whose cells end the
  title's row (`Layout::prefix`), and each column names its group (`Column::group`), so the draw
  repeats none of the layout's rules. The dispatcher cuts a note, its reason and the describe
  function's line to 512 bytes once, as the layout measures them. The layout is worked out again
  only when the menu shown, the tiled area or the prefix argument's width change, or the notes'
  cells, hidden heads, line or default head; a note whose text changes in the same cells is drawn
  in place. A relayout measures every label and note: about 1–2 ms for a crafted menu of 128 heads
  with notes, against about 80 µs for a frame that keeps its layout and at most 15 µs for a key.
  A describe function whose notes change width on every key pays a relayout per key, and a live
  resize pays one per step, so step 3's describe functions keep their notes' widths steady, for
  example by padding counts.
- **The stack keeps the set its first menu came from** (`Arc<Menus>`), so a submenu's id names
  the menu it named when that one opened, whatever a reload has put in since. A menu on the stack
  is not pushed again, and the stack is at most 8 deep; both refusals say so in the echo area.
- **`q` closes one level**, as `C-g` does. `C-g` first ends a half-typed head's keys, then the
  prefix argument, then closes a level.
- **A key the menu does not bind** says `x is not in this menu: q to quit` (`C-g to quit` where a
  head binds `q`) and drops the prefix argument, as an undefined key does. Under
  `foreign-keys = "run"`, a prefix key so run goes on through the usual keymaps, and the menu's
  keys come first again once it ends. Text from an input method follows the same rule and is
  never echoed. A foreign key that types a character is echoed as typed, as `x is undefined` is.
- **Inside a menu, `C-u`, `M-<digit>` and `M--` run the default global keymap's prefix
  commands**, whatever the user's keys bind them to, and the prefix argument shows only on the
  title's row; the echo area shows a half-typed head's keys (`Z-`).
- **The head on `RET` wins** over a describe function's `Notes::ret_head`, which a built-in menu
  names only where no head is on `RET` (checked in debug builds). The default head is marked
  wherever `RET` runs it, a default head whose own keys are `RET` included.
- **No keymap is built for a menu,** in place of "the keymap built from each when it opens" and
  "its keymap replaces the global one" above: `Definition::lookup` resolves its heads ahead of the
  usual keymaps, and the dispatcher applies `foreign-keys` to text, so no text command is needed.
- **An unavailable head is dim with no chip**, and its key says why.
- **A head's command reads its menu's switches** also when it first reads an argument in a prompt
  or waits for the clipboard: the command that waits keeps them.
- **The menu closes** when a prompt or a question opens, a search, a jump or a query-replace
  starts, the layer of `C-x TAB` opens, a command waits for an argument or the clipboard, or a
  macro's replay begins. The window's close button closes it as its question opens. A replay that
  opened the menu presses its heads, and one that ends with a menu open leaves it open, the
  person's then: the next replay closes it as it begins, and the macro's repeat key is the menu's
  while it is open. A menu's key ends isearch, query-replace or the layer of `C-x TAB` and then
  opens the menu; during a jump it ends the jump and does not run, as any key the jump's keymap
  does not bind (ADR 0116).
- **The channel** waits with "the person is in a menu", and a question it causes waits for the
  menu to close. Both the channel's state and the dev socket's `get-state` gain `menu`, the name
  of the menu shown or null; the channel's state version stays 1, as the field is added.
- **The mouse** acts as usual while a menu is open, and the menu stays.
- `Keymap::menu_bindings` lists the keys bound to menus, for `open-menu`'s key column; the
  default keys' count and step 3's unbinding of removed menus' keys will use it. `menu::builds_prefix`
  is public, so the dispatcher and a definition's checks share one rule.
- The menu's panes take ids from `1 << 21`, as the preview's do from `1 << 20`.

## The author's choices

The author answered every question on 2026-11-13, through the coordinator: (a) for each, but
these.

- **Q1: (d).** The built-in menus are the review scope menu, toggles, windows and buffers, jumps,
  and the search menu with the heads oso has today.
- **Q24: changed.** The author needs which-key, behaving as which-key does: a panel at the bottom
  of the frame lists a waiting prefix's keys, and the next key goes on as usual. It is not drawn
  in the menu's float. It is step 6 of this ADR, and Q26–Q32 (all (a)) settle its design: a 1.0 s
  delay, a bottom panel drawn over the windows up to a quarter of the frame, which-key's paging,
  every key the dispatcher would resolve, `key → command-name` with labels, `C-h` showing it at
  once, and three faces.
- On Q11 the author asked whether the menus are the user's configuration: they are (see "How
  menus are defined"); the built-in ones are defaults the file adds to, replaces or removes.
- On Q22 the author asked whether agents can open menus: they cannot, and need not, since an agent
  runs a head's command directly with `run` (see "Around the menu").
- On Q23 the author approves the `bench.rs` registration when step 5 comes.

## Changes to accepted ADRs

- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), "M3's menus take layers above
  these": a menu never sits over a prompt, so it uses the float layers (Q20 (a)).
- [ADR 0008](0008-floating-ui.md), "the minibuffer, menus, completion popups and hover documentation
  are buffers shown in floating windows": a menu is a float with no window and no buffer, since
  nothing is typed or edited in it; its heads act on the window beneath. "Groups of single-key
  heads": a head may be a sequence of up to three keys (Q13 (a)).
- [ADR 0108](0108-the-user-configuration-file.md), "each entry is … either a command name or
  `false`": an entry may also be `{ menu = "<name>" }`, and version 1 gains `[menus]`.
- [ADR 0122](0122-agent-review.md), "a float of choices as in ADR 0117 once it lands": the review
  menu is a built-in menu with a describe function, switches and a default head.

## Consequences

- The author's hydras become data in `config.toml`, reloaded on save, with no code run; menus
  whose labels change as they draw are built in.
- A menu costs one small float and a keymap built when it opens; nothing runs while none is open.
- Menus are reached only by the person, so the channel gains nothing and loses nothing.
- Options with values and saved values stay for a later ADR; the which-key list is step 6.
- A key waiting after a prefix shows what can follow after a delay, without the person asking.

## Alternatives considered

- **Menus as buffers in a floating window (ADR 0008's first idea):** nothing is edited in a menu,
  and a window would take the selection from the window the heads act on.
- **A Lisp-like or scripted definition, as `defhydra`'s `:pre`, `:post`, transient's description
  functions and arbitrary forms:** the configuration must not run code (ADR 0108); a head names one
  command, and labels that change are built in.
- **A transient-style cursor that moves between heads:** the previous setup put the cursor on the
  last choice so `RET` repeats it; a named default head does that without a cursor to draw and
  move.
- **Commands that read the next key themselves:** a command never waits (ADR 0034).
