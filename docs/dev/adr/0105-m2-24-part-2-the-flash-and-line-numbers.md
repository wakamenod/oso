# 0105. M2-24 part 2: the flash and line numbers

- Status: Accepted
- Date: 2026-11-10

## Context

[ADR 0097](0097-visual-aids.md) decides the visual aids. Building its cursor flash showed
three things it had not settled:

- what counts as a scroll on purpose;
- whether an agent's jump or an edit flashes;
- what a fading flash costs the keys pressed during it.

The last was measured on the Mac mini with `just perf-remote`. The bench presses its scroll
keys 25 to 41 ms apart.

| The flash's frames | key p99 | Base (main) |
|---|---|---|
| every 16 ms over 0.4 s | 25 ms | 2.7 ms |
| each 33 ms after the last one drawn | 16 ms | 2.7 ms |
| none for 150 ms, then three steps | 2.7 ms | 2.7 ms |

A frame drawn shortly before a key's makes the key's frame wait for the display, about a
refresh. Frames every 16 ms also ran ahead of the 60 Hz display and filled the swap chain.

The author chose each point below (2026-11-10).

## Decision

### The flash

- **A scroll counts as far as it moves the view.** The cursor flashes when the primary cursor
  or the view's top moves 5 or more rows, or another buffer or window is shown.
  - The view follows the cursor by the fewest rows, so following moves the top that far only
    when the cursor moves as far too.
  - So a `C-l` or `C-v` that moves the view less, near a buffer's end or in a window of few
    rows, does not flash.
- **Edits flash too.** An edit that moves the cursor 5 rows or more, such as a yank of many
  lines or `C-w` of a long region, flashes, as Emacs's beacon does. The trigger compares the
  view, not the command.
- **One place asks.** Every input asks in one place, the dispatcher's: keys, typed text, an
  input method's text, a held completion's answer, a yank's clipboard answer, and the dev
  socket's keys, one key at a time.
- **An agent's jump flashes, but ends no flash,** as it is no key of the person's. That is a
  convenience, not a safeguard, so `beacon-mode` stays `Look`.
- **It holds, then steps down.** The band is at full strength for 150 ms, at 2/3 from 150 ms,
  at 1/3 from 275 ms, and gone at 400 ms.
  - That is three frames after the jump's own, and none in its first 150 ms.
  - Keys pressed fast, as when `C-v` is held, end it before any of them.
  - A key pressed 150 to 400 ms after a jump may still wait about a refresh, once.
  - The last step's frame is asked for whether or not the others were drawn, so a hidden
    window still sees it end.

## Consequences

- Key latency during a flash matches main's on the bench.
- The fade is three steps, not a smooth one.
- The flash's view sample counts logical lines. Soft wrap's next step (M2-25 step 4) moves it
  to view rows.

## Alternatives considered

- **Frames every 16 ms, as ADR 0097 said:** key p99 went to 25 ms.
- **Frames paced off the last one drawn, 33 ms apart:** key p99 went to 16 ms. No cadence
  that draws in the first ~50 ms after a jump avoids the wait.
- **No fade, full strength for 0.4 s then gone:** the cheapest, at one frame, but a blink,
  not a fade.
- **A smooth fade, accepting a refresh of latency during it:** misses the 8 ms p99 budget.
- **Only scrolling commands flash, not edits:** it needs the command's name in the trigger,
  and Emacs's beacon flashes on both.

## Changes to accepted ADRs

The passages below stay in ADR 0097 as written; what holds now follows each. ADR 0097's
status becomes `Accepted, amended by 0105`.

- ADR 0097, "the window is scrolled by a command that scrolls on purpose: `C-v`, `M-v`,
  `C-l`, `M-<`, `M->`": a scroll flashes when it moves the view's top 5 rows or more,
  whatever the command.
- ADR 0097, "It does not flash for: … typing and editing": an edit that moves the cursor 5
  rows or more flashes.
- ADR 0097, "its strength falling from full to none over about 0.4 s": full for 150 ms, then
  2/3, 1/3 and none at 150, 275 and 400 ms.
- ADR 0097, "While it fades, the app asks for a frame every 16 ms, about 25 frames": three
  frames, at 150, 275 and 400 ms.
