# 0182. A native session's requests in its conversation

- Status: Accepted
- Date: 2026-11-24

## Context

A native session asks the person about a call, with questions, or for a plan's approval, on a card
docked between its transcript and its composer
([ADR 0165](0165-native-claude-sessions-the-route.md), part 2e-2 and 3d;
[ADR 0170](0170-docked-windows-under-a-sessions-transcript.md)).
[ADR 0180](0180-a-permission-card-that-stands-out-and-is-read-as-it-is.md) gave it a frame, a
background, pages and keys typed in the empty composer. The author still finds it very hard to
understand:

- **It is a second place.** The call's row is in the conversation and what it asks is in a box
  below, with a width, wrapping, pages and keys of its own.
- **It shows a part.** A call's card has 12 rows: of a 200-line edit, 10. The whole is JSON, in the
  same box. The title says so.
- **Its change is no diff:** every old line, then every new one.
- **A letter answers from two places,** the card and the empty composer.
- **A plan is approved in one place,** by a letter, and edited in another.

What a request shows is Claude's text, which must not pass for oso's words or keys, and what it
answers lets a call run. Those rules of ADR 0165 stay.

## Decision

The author chose on 2026-11-24, from three sketches, to abolish the cards and set requests in the
conversation, and then, from mock-ups: a call is allowed once all of it is read; a band and a tint
as in the mock-ups; point does not go by itself with a Japanese input source. The rest of the
questions were answered through the space's orchestrator, as recommended: the line takes an empty
composer's place; an own answer goes by `C-c C-c`; a plan opens on `RET`, read-only until `e`; an
answered request is one row.

Words: a **request** is a call, questions or a plan that waits for the person. Its **block** is its
text in the transcript. The **line** is the dock that says its keys and takes them. The **choices**
are a question's options to take, and the **answer input** is where an answer of one's own is
written.

### The block

- **The last thing in the transcript,** after its rows and the queue's. Rows that come meanwhile, of
  a subagent or a parallel call, go above it. So nothing stands between a request and its keys, and
  a window that follows has it at its bottom.
- **One tail.** The queue's rows and the block are one tail of the transcript, with its own runs: what
  draws, counts, colours or lays out the transcript's end goes through it. A point or a view in the
  block keeps its place in the block, whatever is put in above.
- **Columns 0 and 1 are oso's,** as in every row (ADR 0165, "The rows' look"): `⚠ ` of the title,
  `$ ` before a command, `+ ` and `- ` of a change, an option's number. All of Claude's text, a
  field's name among it, starts in column 2 or later. A mark that would join oso's goes on `◌`, only
  U+0020 counts as the indent, and what a display would not print is escaped. A transcript's wrapped
  rows are indented by 2 or more, so this holds wrapped, and with highlighting off. A word or a count
  of oso's is on the title's band or starts in column 0.
- **The title** is oso's words on a band: `⚠ Bash asks to run`, `⚠ Edit asks to change a file`,
  `⚠ Claude asks`, `⚠ Claude's plan`, and at the right `adds 60 lines, removes 40`, or `1 of 2`.
  Of Claude's side it names the tool alone, cut, with anything but ASCII letters, digits, `_`, `-` and
  `.` escaped. The title alone is drawn again, for the window's width and the count; what is read and
  what keeps its place are counted from the first byte under it.
- **Under it Claude's text, on a tint** to the window's edges. Band and tint are drawn where a line
  starts in a highlight that only the block's code gives, never from what a text holds. No frame and
  no colour is added: the band, the tint and the chips are ADR 0180's.
- **All of it:** what runs, then why, then every other field of the input (`timeout 600000`). A
  question has its header, its whole text, and each option's whole label and description. No pages
  and no cut. The text under the title is made once from the request, the same at every width, wrapped
  by the display ([ADR 0100](0100-soft-wrap.md)), and does not change until it goes.
- **A change is a diff,** old against new, made by `similar` on the socket's thread, never the UI's,
  under a deadline, past which it is all old then all new. The app makes it there, through what it
  gives `channel::start_sessions`, over `review::diff`; `channel` uses neither `sessions` nor
  `review`, and the fields are read in `sessions::native::wire`, as before. It is on the review's
  bands, with no line numbers, as the request has none. It is exact: a carriage return, a missing last
  newline and a change of white space alone show. `replace_all` stands above it.
- **Bounds:** 64 KiB of input, as a request carries, and 2000 lines of block. A call over either says
  in oso's words how many lines are not shown, or that the input is `not shown at all`; it is
  allowed only unread ("Reading"), and no place says `all read`. Questions over either are denied
  unasked, as are more than 4 questions or 9 options, and, as before, a question over 8 KiB or one
  that cannot be read; a label and a description have no bound of their own. A plan over 64 KiB is
  cut, as before; its block past 2000 lines says how many are not shown, and the plan opened has them.
- **Ordinary text:** motion, copying, isearch and scrolling as in any row. It is not edited. `TAB`
  and `RET` do nothing on it. The transcript is withheld from agents, so the block is; a test holds
  that no agent reads it.

### The line

- **A dock of one row,** `*request: NAME*`, read-only and withheld, for a call or a plan. The
  order of docks is the to-do strip, the line, the composer.
- **In the composer's place or above it.** The line is always in its own slot. When point goes to it
  and the composer is empty, the composer is undocked, its buffer kept, until the request goes, `i`,
  or something put into it. Otherwise, with a draft, while the person writes after `i`, or while point
  has not gone to it, the line stands above the composer.
- **What it says:** ` y  allow   n  deny   N  deny, saying why   RET  its input   i  write`, and at
  the right `Bash · all read`. Oso's words and the tool's name only, as in the title, never a target
  or a header. Keys are chips. Where they do not fit, they take fewer words, then more rows, so that
  none is left out. Under 20 cells it takes no answer.
- **What it shows is what a key does now.** While an answer would be refused, in the quiet wait, in a
  macro, with oso's window not in front, the chips are dim. Unread: `SPC  read on   n  deny   N  why
  C-u y  unread   i  write`, at the right `34 of 202 rows read`; `y` then goes to the first row not
  read, says so, and answers nothing. Where point is not, it says
  `Bash asks to run   C-c C-j  answers it`.
- **Keys:** `SPC` and `DEL`, `C-v` and `M-v`, and the wheel scroll the transcript; `M-<` and `M->`
  go to the block's ends. `C-c C-o` and `C-g` go to the transcript, and the request waits on. `i`
  goes to the composer. `C-c C-k` is `n`. `RET` shows the input as JSON in `*result: NAME*`, as a
  result's is. `C-c C-j`, in the transcript and the composer, goes to the line or the answer input.
  All are `Person`.

### Reading

In place of "the card's window shows all of it":

- **A row of the block is read** when the window that carries the session's docks showed it, whole,
  in frames that were presented, for 500 ms without its view moving, while:
  - that window or one of its docks was selected, oso's window in front and in sight;
  - no prompt, menu or float was open;
  - the row was wrapped, not scrolled sideways, with no cluster clipped, and laid out at the
    window's width: a row of a provisional wrap is not read.
- **It is kept as ranges of the block's bytes,** so a resize forgets nothing, and is the request's:
  a new block starts unread.
- **What an answer needs:**
  - allow a call: its whole block read, and its last row in view at the key;
  - take or mark an option: the question, and that option's own rows;
  - an own answer: the question;
  - approve a plan: the whole of `*plan: NAME*`, read there by the same rule; that buffer wraps;
  - deny, decline, reject: nothing read, and the quiet wait as any answer.
- **Unread, with `C-u`:** `C-u y` allows a call, and `C-u` before an approval's chord approves a plan,
  after `y` to a question that names the tool and says how many rows are not read, or that the input
  is not shown at all. The question is bound to the call's id and its `y` is under the quiet wait, as
  auto's is (part 3d). Where several apply to a plan (unread, edited, auto), one question says all of
  it. It opens only once the block was in view for 500 ms, and closes as oso's window
  loses the front. No option and no own answer has such a way.
- **The app measures it,** from the laid out rows of the window (`Window::shown_bytes`,
  `Editor::screen_of`); oso-core knows nothing of reading. R1 says if a row's width must become
  public.

### Point

1. **At once, and where the person is.** As a request comes, point goes to the line or the answer
   input, while the person is at the end of that session's transcript or in its empty composer, and in
   the middle of nothing (ADR 0180's list). Who reads earlier rows keeps their place, and the line
   shows.
2. **Never by itself with a Japanese input source.** While the keyboard's source is a Japanese mode,
   point goes by neither this rule nor the next, from the composer or the transcript: `C-c C-j`,
   `C-x o` and a click go. The empty composer keeps its place, the line above it, and typing
   composes there. The input method is off on the line, as in any read-only buffer; the answer input
   takes Japanese. A source oso is not told of, as on Linux and Windows today, counts as not Japanese.
3. **With a draft nothing moves.** The line stands above the composer. Nothing typed in the composer
   answers: no letter, no `RET`, no chord. Point goes as ADR 0180 had it, once the draft is cleared or
   what held it ended and 500 ms passed with no key. But once the person chose the composer while the
   request waits, by `i`, `C-x o` or a click, point does not go by itself for that request, also after
   what they wrote was sent or cleared; `C-c C-j` goes.
4. **A letter typed as it comes is text.** Within 500 ms of point going by itself out of the empty
   composer, a key that types goes into the composer, and point goes back with it; the sentence goes
   on as a draft. Only then: not after `C-c C-j`, a scroll or an answer, not from the transcript, and
   nothing after a prefix key or an argument. There a key too soon is refused, and says so.
5. **The quiet wait is ADR 0180's:** 500 ms since the line became the selected buffer, since the
   block came or was replaced, since the key before, and since oso's window came in front or in
   sight; 1.5 s more after a prompt an agent opened closed. Never from a macro, an agent, or text an
   input method commits. It is measured to when the key was pressed; a chord's or an argument's, to
   its first key. **The repeat of a held key never answers,** `RET` and chords among them. Rows that
   go in above a block do not start it again.
6. **The composer's keys go.** No letter answers from the composer (ADR 0180's `y`, `n`, `N`, `o` and
   digits): an empty one gives its place to the line, and one the person chose is to write in.
7. **A block taller than its window:** a window that follows shows its head at its top, point on its
   first row, once a request and a window, so that it does not follow meanwhile. Answered, it is at
   the end again, unless the person moved.
8. **Agents.** No agent command runs, a look among them, and the channel's `open` shows no file, one
   it is loading among them, while the selected window, or the one beneath the prompts, shows a session
   whose request waits, is docked under one, or shows its plan. While any request waits, an agent
   switches neither highlighting nor wrapping (`global-font-lock-mode`, `toggle-truncate-lines`), and
   nothing an agent does changes what the window that carries a waiting request, or shows its plan,
   shows. Nothing an agent does counts as reading.
9. **Gone otherwise** (an interrupt, its bridge gone, the session ended): the block and the line go,
   the echo area says so, and point that was there goes to the composer. An own answer half written
   goes into that session's history of own answers, in memory only, never one that holds secret text.

### Questions

- **The choices** are a read-only dock over **the answer input,** `*answer: NAME*`, an editable dock
  in the composer's slot, withheld, with no highlighting. A title of oso's on the band says the keys:
  `RET takes, or type your own answer`, `C-c C-k declines` at the right. Then a row an option, its
  number tying it to the block: number, label, the first line of its description, each cut to the row.
  Mark and number are in the first columns, in oso's faces; Claude's text is only after them, on one
  line, escaped and on `◌` as in the block. Neither dock takes an answer under 20 cells, or with the choices
  wrapped or scrolled sideways; a click moves the mark and answers nothing. At most 9 and the input;
  in a short window the list scrolls around the marked one, and the block keeps 4 rows.
- **Until point goes to them,** a question has the line alone, `Claude asks   C-c C-j  answers it`,
  above the composer. Going there puts the choices in the line's slot and the answer input in the
  composer's, over a draft too. The draft's buffer keeps its text, and is docked again as the request
  goes, or as `C-c C-o`, `C-g` or `i` leaves the answer input.
- **`C-n` and `C-p`** (and the arrows) move the mark; as it moves, the transcript shows that option's
  rows, which is how they are read. `RET` takes the marked option. No digit answers: it is text.
- **Where several may be taken,** `TAB` marks (`[x]`) and `RET` gives those marked; with none it says
  so.
- **Typing is an own answer:** the mark goes to the input, the options dim, and the title says
  `C-c C-c answers in your words`. `RET` there is a line break. `C-p` back to an option keeps the
  text, and the title says what `RET` takes; the text then goes into the history. `C-g` clears it,
  then goes to the transcript. `M-p` walks the history. In a text of several lines `C-n` and `C-p`
  move by lines, and to the options from its first. It wraps. An own answer that a yank made secret is
  refused, and none that holds a secret goes into the history.
- **The gate:** `RET` on an option, or giving those marked, is under the whole quiet wait. `C-c C-c`
  gives the text only while the mark is on the input; with none there it says so, and takes nothing;
  options marked are then not given, and the title says so. It needs the question read and 500 ms
  since the input was selected, since this question's block came, since a composition was committed
  and since oso's window came in front or in sight; longer after a prompt an agent opened; never from
  a macro or a held key's repeat; not since the key before, as the person writes the answer and then
  gives it. Until the block is that old, keys that type go to the composer where "Point" 4 says so, and
  are refused otherwise, so that a question is answered by what is typed once it shows. A
  composition's committing `RET` takes nothing. A dictated text never answers, takes an option or
  approves.
- **A call's questions are asked one at a time,** each with its own block and wait, `Claude asks 2/3`.

### Plans

- **The block shows the plan,** its Markdown coloured as a reply's with its marks shown. The line:
  `RET  open the plan   n  keep planning   N  say what   i  write`. **No key of it approves.**
- **`RET` opens `*plan: NAME*`** in the session's window: an ordinary Markdown buffer that wraps, in
  `Mode::Plan`, **read-only until `e`** (a letter from then on). A line of its own, `*plan keys: NAME*`,
  is docked under it, withheld and by the line's rules:
  `C-c C-c  approve   C-c C-a  +edits   C-c C-A  +auto   C-c C-k  reject   e  edit`, at the right
  `read-only`, or `your edit: +2 −1`. `C-c C-o` and `C-x b` leave it waiting, and `RET` on the line
  comes back.
- **Approving** approves what the buffer holds. Auto asks first, as before. An edit that differs asks
  y or n first, with any mode after it, saying its lines, `(+2 −1 lines)`, and its `y` approves the
  edit only as it was when asked.
- **The gate:** while it is read-only, the whole quiet wait, from when the buffer became the selected
  one. Once `e` made it editable, 500 ms since the latest of that, the buffer becoming the selected one
  and oso's window coming in front or in sight; longer after a prompt an agent opened; never from a
  macro or a held key's repeat; not since the key before, as the person writes the edit and then
  approves it. An edit keeps what was read around it, and what it puts in is read as any text. `RET` on
  the line opens the plan under the quiet wait.
- **`C-c C-k` rejects,** asking first if the plan was edited; `C-u C-c C-k` says why.
- **A plan over 64 KiB** opens read-only, has no `e`, and is rejected, or approved only unread (`C-u`),
  the question saying that its end is not shown.
- **What part 3d refuses** is refused as before: a macro, another call's plan, an empty, secret or too
  long edit. A changed plan whose request went stays, stale; there `C-c C-k` gives it up,
  asking first, and answers no request. A later plan's `RET` is refused until then, and says so.

### Answered

- **A call's block goes.** Its own row says how it went, as now (`denied: WHY`).
- **Questions are one row,** `▸ ? AskUserQuestion · Scope → The whole workspace`. `TAB` opens it on
  the whole question, its options and the answer, kept in the row within the request's bounds.
- **A plan's row** says what it says now (` · approved, edited`).

### Several requests

- **One block and one line a session,** the first asked. The others' rows say `waiting for you`, and
  the title `1 of 2`. Answered, the next one's block comes at once, with point where it was and a
  quiet wait and a reading of its own; a Japanese source does not take point off the line then, as
  the person is answering.
- **Two sessions, two windows:** each has its own, and only the selected place answers. A transcript
  in two windows: the one that carries the docks counts (ADR 0170's D1).
- **In no window:** `needs input`, `*Spaces*` and the modeline lead there, as now.

### The TUI, oso-restart and Remote Control

- **`session-switch-mode` is refused** while a request waits (part 4d-1). A TUI's dialogs are its own.
- **`oso-restart` waits** for the turn (part 4b). A changed plan refuses it, as now.
- **A permission mode's change** leaves a request until the CLI takes it back, as now.
- **[ADR 0181](0181-remote-control-of-native-sessions.md) holds, read of requests:** they are answered
  at oso. No line from the phone or the stream answers one, closes one or counts as reading. Its
  session tools' questions are the agent channel's prompts, not requests.

### The route

The bridge, its claim, how a call is found in the stream, what is answered and what an answer may
carry stay as ADR 0165 has them (2e-1, 2e-2, 3d). Answering acts on the request's id, never on what
point is on.

## Public API

Expected, each **approved at its PR; none is approved here:**

- **oso-core:** `Highlight::{CardTitle, CardKey}` become `{RequestTitle, RequestKey}`, and
  `RequestBody`, the tint's, comes; `Mode::Card` becomes `Mode::Request`, and `Mode::Answer` comes;
  `DockSlot::Card` becomes `DockSlot::Request`, after `Strip`; `SessionRequest::CardText` becomes
  `RequestText`. `NativeAction` loses `Choose(u8)`, `Other`, `Page(isize)` and `EditPlan`, and gets
  `Request`, `ReadOn(isize)`, `Choice(isize)`, `Mark`, `Take`, `Edit` and `Unread`.
- **Maybe one argument of `Editor::replace_native_rows`,** for a place kept in the tail: none, R1a
  found.
- **oso-render:** `Face::{Card, CardTitle}` become `{Request, RequestTitle}`.
- **oso-platform:** nothing, if `Event::InputSource` is told as oso starts and comes in front, the app
  keeping the last; else the current input source. R1 checks.
- **R3:** what makes a `Mode::Plan` buffer editable at `e`, and `e` a key only until then.
- **Commands,** all `Person`: `native-card-*` become `native-request-*` (`allow`, `deny`, `note`,
  `input`, `read-on`, `read-back`, `first`, `last`, `leave`, `next-choice`, `previous-choice`, `mark`,
  `take`, `decline`, `answer`, `answer-clear`, `note-text`); `choose`, `other`, `own-text`, `whole`,
  `edit-plan` and `page-*` go. `native-session-request` is `C-c C-j`. The approvals with a mode become
  `native-plan-approve-*`; `native-plan-open`, `native-plan-edit`, `native-plan-reject` and
  `native-plan-reject-note` come, and `native-plan-cancel` is a stale plan's.

## Order

One PR a step, in one train:

- **R0:** this ADR.
- **R1, calls,** in four PRs (the author, 2026-11-24): **R1a,** the agents ("Point" 8), the tail's
  type and the line's place under the strip; **R1b,** the block's text, its diff and what is read, as
  pure functions; **R1c,** calls leave the card, the line above the composer and point not going to
  it by itself; **R1d,** "Point" 1 to 4 and 7, and the line in the composer's place, in one train with
  R1c. In all: the block, the tail, reading, the line, point's rules, the gate, agents, `C-c C-j`.
  Questions and plans keep their cards. A fuzz target holds that only oso stands in columns 0 and 1
  and in its faces, and that the tail's parts stay apart whatever is put in above.
- **R2, questions:** the choices, the answer input, the answered row.
- **R3, plans:** the opening, its line, reject.
- **R4, the cards go:** their code, the renames, ADR 0180's status, the docs.
- **R5,** optional: a shown command's or change's syntax colours.

**Both forms stand on main from R1 to R4, by kind:** a request is a block and a line or a card, never
both. One queue a session holds both kinds, and one shows at a time. ADR 0180 holds for each kind
until its step, its composer's keys and its move of point for a question or a plan among them; "Point"
8 holds for both from R1, and so does the slot's place under the strip. Until R4 the line and the
choices may stand in the card's mode, slot and commands; each PR names the items it adds, and R4
renames. Each step shows its gates' tests failing first: the waits, the held key, reading (width,
sideways, provisional, not presented), and agents, in front and from elsewhere. Each step's docs
(`security.md`, `architecture.md`, `ui.md`, `display-model.md`, `native-sessions-ux.md`, `testing.md`,
`crates/oso/AGENTS.md`, the user's guide) change with it, as what they say now is what is built.

## Changes to accepted ADRs

Each holds from the step that builds it.

- **ADR 0180:** superseded by this one once R4 lands; amended until then. Kept by name here: the
  quiet wait, its list of what one is in the middle of, the chips, the colours, the delayed move.
- **ADR 0165, "Permission questions (Q2)":** the card is a block and a line. **Part 2e-2,** "The
  card", "Claude's text, fenced", "`RET`", "What it says", "Focus", "Keys": as "The block", "The line",
  "Reading" and "Point". "`AskUserQuestion`": the choices and the answer input in place of digits and
  `o`; a digit is text. "`ExitPlanMode`": `y` does not approve.
- **Part 2a, "The composer":** it gives its place to the line and the answer input. **"The queue":**
  its rows are at the transcript's end but for the block.
- **Part 3e, "`*todo: NAME*`":** the strip is above the line, where it says under the card.
- **"Across an oso restart (Q8)":** a waiting request, where it says card. **"The look: tables"** and
  **"The rows' colours":** what is held apart from a card's frame, title and keys is from a block's
  title and marks and a line's keys.
- **Part 3d, "The card's keys", "The edit", "The card's approvals in the edit":** `y`, `a`, `A` and
  `e` of the card go; the plan opens read-only on `RET`; `C-c C-k` rejects, where it gave the edit up.
  "Refused": the card's approvals while an edit differs go with it. The rules of both parts that this
  ADR does not name stay, their amendments of 2026-11-24 among them (an edit asks first and its
  approvals wait, a held key answers once, the channel's `open`, highlighting).
- **"Part 3f":** the line, the answer input and a plan do not bind `C-c C-s`, as the card did not.
- **"The rows' look":** a block is the transcript's last text, its marks in the first two columns;
  an answered question's row says its answer and opens.
- **"Long lines", "Not the card or the strip":** a plan's buffer wraps, and the answer input does; the
  line and the choices do not.
- **"Following the end":** a block taller than the window puts point on its first row, where it says
  that a card that appears moves no point. "The bottom of the screen": the block comes after the
  queue's rows.
- **ADR 0170, "A docked window":** the slot `Card` is `Request`, under the strip; a plan's buffer also
  hosts one, its line. **"Heights":** the line as many rows as its keys need, the choices at most
  10. **D2, a short host:** the strip gives up rows first, then the choices; the line none.
  "Rows": no dock draws a frame. "Agents and secrets": as "Point" 8.
  **"Telling it apart":** the line and the choices' title are on the title's band, and the rest of
  the choices and the answer input over the float's face.
- **[ADR 0117](0117-menus.md):** a menu's key's face is a line's keys', on its band.
- **[ADR 0097](0097-visual-aids.md):** the line and the choices have no current line's band; the block
  has it as a prompt's rows do. A transcript's window with a block marks white space, its other rows'
  too, with `global-whitespace-mode` off.
- **[ADR 0151](0151-moving-a-session-into-a-space.md), [ADR 0155](0155-renaming-sessions.md):** as "R1a,
  as built" says of a session that moves and of a rename.
- **Elsewhere,** as in ADR 0176, ADR 0181 and other parts of ADR 0165, a waiting call's "card" is read
  "request". ADR 0100's "off by default" stands as native transcripts made of it, and ADR 0085's
  `open` and `Look` commands as part 2e-2 did.

## Consequences

- A request is read where it was made, whole, with what led to it above, and searched and copied as
  text.
- **A tall call costs a `SPC` a page,** where one key was enough with 10 rows seen. `C-u y`, `y` is
  the way round.
- **The first rows may be out of view at `y`:** all of it has been on screen, not all at once.
- Being on screen half a second is what oso can see of reading; it is no proof of it.
- The composer is out of sight while the line has its place; `i` brings it back.
- **A `y` typed on the line after the wait allows,** as on a card. Less often: not from the composer,
  not where point came by itself with a Japanese source, not after a letter typed as it came, and no letter approves a plan.
- Text in the answer input is an answer once `C-c C-c` gives it; its title says so.
- With a Japanese source each first request costs a `C-c C-j`.
- Highlighting off, the bands go; the columns and the line's place still tell whose words are whose.
- About 2500 lines go in R4. Measured with a throwaway example (release, one Mac, a wrapped window of
  62 by 100): a block put at the end takes 7 µs at 8 lines, 18 at 200, 70 at 2000 and 210 for one line
  of 64 KiB (p98), the same at 100 and 10,000 rows; what a window shows, under 1 µs, and its rows laid
  out, 20 to 31 µs, only after an event that moved the view of a window in front while its request
  waits. Not measured: the diff of 64 KiB, the first wrap of 2000 long rows.

## R1a, as built (2026-11-24)

- **"Point" 8** holds for cards as they are (`app/agent/requests.rs`). In front, none of an agent's
  commands runs and `open` shows nothing. With a request in a window of the frame, an agent runs no
  command of the windows (`commands::WINDOW_COMMANDS`: a split, a deletion, a size, another window
  selected), nor `global-display-line-numbers-mode`, `global-hl-line-mode`, `next-buffer`,
  `previous-buffer`, `view-echo-area-messages`, `review-files`, `review-quit`, `review-toggle-panes`,
  `session-explain`, `sessions-wider` and `sessions-narrower`;
  and while the side window is selected, from which a buffer goes to a window of the tree, nothing
  runs and `open` shows nothing. While one waits anywhere, not `global-font-lock-mode`,
  `toggle-truncate-lines` or `session-switch`. A test walks every command an agent may run, in
  an ordinary file, and holds that the transcript's window and its docks stay as they are, and that
  what asks the host for something, which it does not see, is held or named there; a command of
  another kind of buffer is judged by its name alone.
- **An explanation** (`session-explain`, `session-explain-file`) that comes while a request waits
  where it would be shown is not put there, unless it was asked for there, which only the person
  can; its job carries which. The person is told.
- **A session that moves into a space** ([ADR 0151](0151-moving-a-session-into-a-space.md)) starts
  behind while a request waits where it would be shown, as one whose space is not in front,
  whatever ended its process: what is shown there then is the person's to choose. An
  agent does not rename a session while that session's request waits
  ([ADR 0155](0155-renaming-sessions.md)).
- **The tail** (`sessions/native/tail.rs`) has three parts: the queue's rows, the block's title and
  its body. The title is a part of its own, as it alone is drawn again, and starts with the row
  between the block and what is above. Until R1c only the queue has text.
- **The order of docks** is the strip, the card, the composer, and so is that of `C-x o` and of
  `windmove`. Public: `DockSlot::Card` is declared
  after `DockSlot::Strip`.
- **"Open", read in the code and not run:** `Event::InputSource` is told as oso first comes in
  front, and each change after, so the app keeps the last and oso-platform needs nothing. oso's own
  switch to ASCII is told too, as `Other`: while oso wants ASCII, and 500 ms after, an `Other` does not
  take the place of a Japanese known before. Linux and Windows tell of no source. No row's width need
  become public, and `Editor::replace_native_rows` no argument: a place under the block's title keeps
  its byte, which the tail's property test holds. For that, a text that goes on from all its range
  held puts its last character in again, so that a place right after the range goes along.

## Open

- What R1a says of the input source, of oso's own switch to ASCII for a prompt
  ([ADR 0158](0158-japanese-in-prompts-and-review-drafts.md)) and of other platforms is from the code:
  R1d runs it.
- Whether [ADR 0175](0175-dictation-through-typewhisper.md)'s dictation writes in the answer input: R2.

## R1b, as built (2026-11-24)

R1 is four PRs: R1a (agents and the tail), R1b (a call's pure parts, `sessions::native::request`,
which nothing shows yet), R1c (the switch for calls) and R1d (point).

- **Every field is shown once** (`wire::request`). One of a known name that does not hold what the
  tool's does is shown as any other. A path is whole, as the request has it; the mock-ups' was not.
- **Text is exact.** A command's, a change's and a file's are as they are, and what the display
  escapes by itself stays so. Oso puts two things in, each told from what was there: the `◌`, and a
  space after a carriage return at a line's end, which the display else takes as part of the line
  break. A text as a field's value is as it is too, unless it could be read as another value, and a
  field's name unless it is not of ASCII letters, digits, `_`, `-` and `.`: these are JSON. A value
  that nests is indented, up to 400 lines, and 64 KiB for all of a call's; else it is on one line. A
  value of more than 40 characters is under its name, further in, as one of several lines is.
- **Oso's words in column 0.** `\ white space at its end` is under a command's, a path's, a field's
  or a changed line that ends so. `\ no line break at its end` is under a last line that has none
  where the other text has one or is empty. `Change 1 of 2` is over each of several. `Why`,
  `The change`, `Its other fields` and `The call's other fields` are over a part only where the one
  before it could be read on into it, as a command of several lines, or why it runs reads as a field
  (`timeout 5`): the mock-ups' scenes have none.
- **Claude Code's own tools** are named in the title alone. Any other's whole name, escaped as there,
  is the block's first line, as a title cuts it.
- **A row's mark** is in `Highlight::RequestBody`, `$ ` too, as a row has its tint by what it starts
  with; `+ ` and `- ` are in their line's highlight, as a tool's row has them. What runs, its fields
  and a path have no highlight; why, and a change's kept lines, are in the quiet one. The line break
  is in its mark's.
- **A title** says `writes 60 lines` of a `Write`, as what it replaces is not in its request, and
  `1 of 2 · adds 60 lines, removes 40` where both fit, else `1 of 2`.
- **The diff** has 50 ms for all of a call's changes. Two texts that both lack a last line break are
  compared as if both had it. Measured (release, one Mac): a block is made in 2 µs at 8 lines, 11 at
  200, 83 at 2000 and 21 for one line of 64 KiB, and 670 for the worst nesting (p98); a change of
  64 KiB took 13 ms at most, with no deadline.
- **Reading:** a row's cells are to where its text ends, past its window's edge too, against the text
  area's columns, as a space may hang in the mark's; one that is clipped there is not read, and a row of a
  provisional wrap is read only where all of it shows. A view's wait starts as a frame of it was
  presented, and again as anything of it changes, its rows' place in the window too. `34 of 202 rows
  read` counts the body's lines, the same at every width, those not shown among them.
- **The line** where point is has as many rows at a width whatever it says, so that what it says
  moves no row. All read and its last row out of view, `SPC  read on` is in `y`'s place. All chips
  are dim while an answer would be refused, in the quiet face on the band; `y` is, too, until all was
  read, and `C-u y` until some of the block was in view for 500 ms. Once what is shown was read or is
  in view, a call not shown whole has `C-u y` first, and `5 lines not shown` or `not shown at all` at
  the right. Its text takes 13 to 26 µs at 20 to 120 cells, 46 at 2000 lines (p98).

## R1c, as built (2026-11-24)

Calls leave the card: block, band and tint, the line above the composer, reading, the gate, `C-u y`,
`C-c C-j` and the diff on the socket's thread. Questions and plans keep their cards. **Point goes to
the line by itself in no case, and the line always stands above the composer:** a call needs `C-c C-j`,
`C-x o` or a click until R1d. A following window shows a tall block's end until then.

- **A request is a call's** unless its tool is `AskUserQuestion` or `ExitPlanMode`. **A plan that cannot
  be read is denied,** unasked, as such questions are, and said: its row
  `denied: oso could not read its plan`, and the echo area, in oso's words.
- **White space:** a window that shows a transcript with a block draws
  [ADR 0097](0097-visual-aids.md)'s marks, of tabs and the spaces it names, whatever the person's
  setting.
- **The line** (`*request: NAME*`, `Mode::Request`, in the slot `Card`) gives up no row: where the docks
  do not fit with it, none shows, nothing is read and nothing answers. A cell is free at each of its
  ends, the cursor's among them.
- **Presented:** a frame that drew every glyph (`Redraw::NotNeeded`), at the size it was laid out for,
  before and after. **In front** is what the system said, never assumed: until it said so, nothing is
  read or answered. "A float" is a description, a completion, a menu, a jump's labels and an input
  method's composition too.
- **Looked at** after each event that drew no frame, as each frame was presented, when what is in
  view is due to be read, and before a key answers.
- **The wait's start** is the latest of point coming to the line, of the call being shown and of the
  person coming to see oso, worked out as a key comes: what was noted when is no part of it. A key acts
  on the call its session shows; one pressed before that call was shown is too soon. A note and
  `C-u y`'s question keep the call's id.
- **Only a key pressed now answers,** of calls, questions and plans, by one check where the command
  runs (`dispatch/answers.rs`): not a replayed macro, which stops there, nor what it writes for an
  answer; not a key held back while oso waited for the clipboard, checked a paste, matched a
  completion, or went on with a large undo, a replacement or a macro, which says to press it again; not
  the repeat of a key held down, whatever it is bound to. Text an input method commits, and what is
  typed by no key, is text.
- **`C-u y`** is any argument. Its question names the tool and the session
  (`Allow Edit in proj unread: 168 of its 202 rows are not read? (y or n)`) and runs
  `native-plan-confirmed`; asking it drops a plan's question, and the reverse. It closes as oso's window
  loses the front or is hidden, or its call goes.
- **`y` with rows left to read** shows the first of them, or the title where that is the first; all read
  and its end out of view, the end. **`SPC`, with nothing more below,** shows the first row not read.
- **`RET`** shows the input in `*result: NAME*`, at most 256 KiB, and says what it left out. With that
  buffer in front the request counts as in front ("Point" 8).
- **After an answer** the session's next call shows at once, its line where it was. A session's bridge
  holds one request at a time, so the next one comes a moment after and no title says `1 of 2`
  today; point is then in the composer.
- **Public items:** oso-core `Mode::Request` and `NativeAction::{Request, ReadOn(isize), Unread}`, whose
  question's `y` is `Confirmed`. `Mode::Card`, `DockSlot::Card`, `Highlight::{CardTitle, CardKey}` and
  `SessionRequest::CardText`, a call's note too, stand until R4, and so does the code both kinds answer
  through in `permissions/card`. Commands: `native-request-{allow,deny,note,input,read-on,read-back,first,last,leave}`
  and `native-session-request`.
- **Measured** (release, one Mac, 10,000 rows; 20, 62 and 100 columns, p98): a block put and its first
  rows wrapped, 60 to 110 µs at 8 lines, 160 to 370 at 200 and 170 at 2000; under 3 ms at the limits; one
  in place of another, as much; taken out, under 1.2 ms. A look, 25 to 130 µs, of the request in front
  alone, once an event and once a frame: the rows are laid out after each event, not only after one
  that moved the view. A block is 130 KB at most, held once as it waits and twice more as it shows.

## Alternatives considered

- **The block in the call's own row:** rows that stream in after it push it away from its keys.
- **A card that shows more:** still a second place, with a width, pages and keys of its own.
- **`y` at once with a count of what is not read:** what the card did. **The first screen is enough for
  an edit:** what is allowed would depend on the tool.
- **The line always above the composer:** a row more, and two places that look ready to type in.
- **The band alone, the text on no tint:** where the request ends is then less plain.
- **Point by itself with a Japanese source too, after 500 ms:** the line takes keys, not text, so the
  person goes to it.
- **`RET` sends an own answer:** a composition is committed and a line broken by the same key.
- **A plan editable at once, or opened by itself:** a plan is read before it is written in.
- **The answered block kept:** the conversation then holds every command and change twice.
- **`C-u` for every answer while highlighting is off:** the columns hold without it.
