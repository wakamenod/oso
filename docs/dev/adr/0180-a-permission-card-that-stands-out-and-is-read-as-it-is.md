# 0180. A permission card that stands out and is read as it is

- Status: Accepted
- Date: 2026-11-24

## Context

A native session asks the person about a call on its permission card
([ADR 0165](0165-native-claude-sessions-the-route.md), part 2e-2), docked between the transcript
and the composer ([ADR 0170](0170-docked-windows-under-a-sessions-transcript.md)). In daily use
on 2026-11-24 the author found it hard to understand:

- **It does not stand out.** The card is drawn on the composer's background, its title and keys
  as rows of plain text.
- **Its keys went elsewhere.** A card asked about a command. The author typed `y` while point
  was in the composer, and `y` became a queued message. Point moves to the card only after
  500 ms with no key.
- **A question could not be read.** The card shows 12 rows. With a question of three paragraphs
  and four options of two lines each, in an 800 by 600 window, it showed the first paragraph and
  a half: not the question itself, and no option. The keys were cut before `[RET] all of it`,
  the only way to the rest. A description's second line was not shown at all, and a question
  over 1 KiB was cut without saying so, after which its answer no longer named it.

What the card shows is Claude's text, which must not pass for oso's words or keys (ADR 0165,
ADR 0170), and what it answers lets a call run.

## Decision

The author decided on 2026-11-24, through the orchestrator session: the look, the composer's keys
and the move at once; then that questions and plans grow and page, and the public items below.

### The look

- **A frame and a background of its own.** The card's title row is the frame's top, `╭─ … ─╮`, on
  a band; its keys' row is the bottom, `╰─ … ─╯`; a row of Claude's is between the sides, `│ … │`,
  or `┊ … │` where it goes on with the row above. The fence is the frame's left side. No row is
  added for it.
- **Keys are chips,** in a menu's key's face ([ADR 0117](0117-menus.md)), beside what each does. Where
  they do not fit one row they take fewer words, and then more rows of oso's, `├─ … ─┤`, so
  that none is left out. In a call's card a second row of them takes one of Claude's.
- **Colour:** `Face::Card` is the background, and `Face::CardTitle` the frame, the title, what the
  keys do and the title's band: amber, apart from the transcript and the other docks.
- **oso's words stay oso's.** The frame and oso's words are in `CardTitle` and a key in `CardKey`.
  Only the code that builds the card gives these, and only to what oso says; a title names the tool,
  after the warning that not all is shown, with nothing a display would not print. A chip and the band are
  drawn where those are, never from what a text holds. A question's option has its number on
  a chip and `[x]` in oso's colour, so a description cannot pass for an option.
- **No band of the current line's** ([ADR 0097](0097-visual-aids.md)) is drawn in a card.
- **Rows are the card's own:** a card's dock takes a row for each of its lines, however wide, as
  its rows are broken at its width and fill it.

### Point

- **At once.** As a card appears, point moves to it, while the person is in that session's transcript
  or in its composer with nothing written, and in the middle of nothing: no key sequence, prompt,
  search, jump, query-replace, menu, composition, keyboard macro or held mouse button, nor input held
  back for the clipboard or an undo. The card's window must show all of it, as before.
- **Otherwise it waits, as before:** point moves once none of that holds and 500 ms passed with no
  key. So clearing a draft is not the moment point goes away.
- **The quiet wait stays:** a key answers the card only after 500 ms with no key and since
  the card became the selected buffer. A key typed as the card appears never answers it. It is measured
  up to when the person pressed the key, and starts again as a window comes to show all of the card,
  shows it at another size, or oso's window comes in front or in sight again: it is measured
  from when the person could see what they answer. After a prompt an agent opened closed,
  wherever it was, a card waits 1.5 s more, as a key meant for the prompt may come just after.
- **A window narrower than 20 cells** cuts each row of a card, which then takes no answer.

### The composer's keys

- **In an empty composer, the card's keys answer it.** On a call's or a plan's card `y`, `n` and
  `N`; on a question's `n`, `N`, `o` and the digit of an option that has been on screen. Not a
  plan's `a`, `A` and `e`: they grant a mode beyond the plan, or open its edit, and begin many a
  sentence. Not `RET`, the composer's line break. `y` is no answer to a question, and is typed.
- **Under the card's own gate:** 500 ms since the card showed or a new call filled it, since the
  composer was last empty and the selected buffer, and since the key before; longer after a
  prompt an agent opened closed; never from a macro; never while the card's window shows
  less than all of it. An agent's look may run while the composer is selected, as the gate looks at
  the card's window at the answer, and waits again once it shows all of it.
- **Where a key would not answer, it is typed.** It is neither an answer nor lost. With text
  in the composer, every key is typed.
- **Only a key the person types there:** one that would be typed. Not one a keymap binds, nor one
  after a prefix key or argument, nor text an input method commits, nor the repeat of a held
  key, nor what a macro replays. A key that answered drops its own repeats, as on the card.

### Questions and plans

- **As tall as they need.** A question's card shows all it asks: the header, the whole question, a
  blank row, then each option, its number and label, its description under it, indented. A
  plan's shows the whole plan. The card grows up to the rows of its window, less the composer's,
  the strip's and 4 rows of transcript, and at most 60. A call's card keeps its 12.
- **Broken to be read:** at a space, or beside a wide character, with no stop at a row's start; a
  longer word at the width. A command and a diff are broken exactly at the width, as before. The card
  breaks its own rows, not the display's wrap ([ADR 0100](0100-soft-wrap.md)): each is fenced,
  measured apart, and starts on `◌` where a mark would join the fence.
- **Pages.** Where it does not fit, the card shows a page, and a row of oso's above the keys says so:
  `├─ ▼ rows 1-24 of 41  SPC  next  DEL  back ─┤`. `SPC` and `DEL` turn it, with a row of the page
  before; turning is no answer, so it does not wait. A page has at least 3 of Claude's rows; a window
  that gives fewer cuts the card, which takes no answer, as before.
- **A digit chooses only an option that has been on screen,** each of its rows, since the card
  showed. Another turns the card to that option and says so. `n`, `o` and a plan's keys answer on any
  page.
- **`RET`** stays "done" for a question that takes several, and a plan's whole view. On a
  question that takes one it opens no view, as the card shows all of it, and says to choose.
- **The title** is `Claude asks 1/2`; the questions of a call are asked one at a time, as before.
- **Bounds:** a question up to 8 KiB. One over that denies its call unasked, as its text is
  its answer's key, which a cut would not match. A description up to 2 KiB with its lines, and a
  label 256 bytes as before, whose option says that it is cut. A card that grows has about 2000 rows
  at most. At most 4 questions of 9 options, as before. Breaking a text costs its length.

### Public API

Approved with this ADR: `oso_core::syntax::Highlight::{CardTitle, CardKey}`,
`oso_render::Face::{Card, CardTitle}` and `oso_core::editor::NativeAction::Page(isize)`, which
the commands `native-card-page-down` and `native-card-page-up` ask, both `Person`.

## Changes to accepted ADRs

- **ADR 0165, part 2e-2,** "Claude's text, fenced": the title and the keys are the frame's top and
  bottom, and the keys chips, where it says "a plain row in oso's own face (not ADR 0075's chips
  …)"; "At most 10 rows are fenced" and the break at the width hold for a call's card alone.
  "Focus": point moves at once, as above, where it says "after 500 ms with no key". The keys that
  answer may come from the empty composer, where an agent's looks run. "`RET`": not for a question
  that takes one option. "`AskUserQuestion`": the options, the bounds and the
  digits are as above. **Part 3d:** `a`, `A` and `e` stay the card's alone.
- **ADR 0170,** "Rows", "Heights": the card takes as many rows as its text has lines, at most 60, its
  host drawing it in the rows it has; the strip's 6 stay. "Drawing", "Telling it apart": the card is
  drawn over its own face, not the float's, and its keys are chips. D2's "A cut card says so on its
  last row" was never so: a cut card loses its last rows, takes no answer and says so as a key asks.
- **ADR 0097:** a card has no current line's band.
- **ADR 0117:** a menu's key's face is also a card's keys', on the card's own background.

## Consequences

- A call that waits is seen, and the key the eye reads on it works from where the person is.
- A `y` meant as a message's first letter, typed in an empty composer half a second or more after the
  card showed, allows the call. The author chose this. A draft's first letter typed sooner is text.
- Point leaves the transcript the moment a card appears. A letter key pressed then goes to the card:
  within the quiet wait it does nothing.
- With an input method composing Japanese, `y` in the composer starts a composition, and is text.
- A long question takes most of the window while it waits.
- `describe-key` in the composer says `self-insert-command` for a key that would answer;
  `view-lossage` shows the card's command that ran.
- A dev build's socket stands for the person's keys, as elsewhere: one `send-keys` key may answer from
  the composer.

## Alternatives considered

- **A frame in pixels,** as a float's panel (ADR 0075): drawn over the cursor, which is on the same
  layer; it would need a layer between the docks and the floats.
- **The card's keys in the composer bound in its keymap:** they would show in `describe-key`, but what a
  key does would depend on a card and a clock a keymap knows nothing of.
- **A letter typed too soon refused, not typed:** the start of a message would be lost.
- **`a`, `A` and `e` from the composer too:** see above.
- **`RET`'s whole view for a question:** JSON, and a second step to read what is asked.
- **A float over the transcript for a long question:** covers what the question is about.
