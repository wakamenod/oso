# 0077. Choices for the rest of the native window layer's stage 2

- Status: Proposed
- Date: 2026-11-08

## Context

[ADR 0070](0070-windowing-and-input-without-winit.md) replaces winit with oso's own window
layer. Its stage 2, the macOS layer behind `native-window`, landed in parts, and
`open-questions.md` lists what it still owes before the layer becomes the default on macOS.
Ten choices were left open there, and the author made them on 2026-11-08 through the
coordinator.

## Decision

The author chose, from options put to them:

1. **A composition is committed when oso turns the IME off, on the native layer only.** winit
   cannot commit one, so on winit oso keeps waiting for the composition to end, as
   [ADR 0069](0069-macos-input-method.md) says. The two layers differ until winit goes.
2. **A logout, a restart or a shutdown waits at most 5 s for running saves**
   (`NSTerminateLater`), then answers yes, and `exiting` writes the emergency copies, the
   buffers being saved included ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)).
3. **A Dock quit with a save running waits as `C-x C-c` does,** up to
   [ADR 0059](0059-safe-save-and-the-exit-guard.md)'s 10 minutes, with the window hidden.
4. **The input method is not told that the cursor moved while nothing is composing**
   (`invalidateCharacterCoordinates`). It asks for the cursor's place when it needs it.
5. **A commit goes where typing goes at that moment,** also when the IME turns off because
   another buffer was selected. A buffer that refuses text refuses it, and one that may hold
   secrets drops the composition, as ADR 0069 has it.
6. **The input method learns where each character of the composition is drawn.** With the
   cursor area, the app gives the x offset of each preedit character on its row, from the
   frame that drew it, so `firstRectForCharacterRange` answers any range inside the marked
   text and the candidate window follows the segment being converted.
7. **A range outside the marked text is answered with the cursor area,** as now, and not
   laid out.
8. **The cursor's line is given to the window layer with each frame in which the cursor or
   its line changed, and only while the IME is on.** It is bounded as ADR 0070 says: up to
   2000 bytes before the cursor and 2000 after, cut on grapheme cluster boundaries, with every
   range counted from the excerpt's start, and only from a buffer whose verdict is
   `Ordinary`.
9. **A replacement range outside the marked text is ignored:** the text goes in at the
   cursor, as now. Reconversion, which ADR 0070 allowed inside the excerpt, waits as an open
   question.
10. **The input method's queries land in two pull requests:** first the rectangles, no input
    context while the IME is off and Writing Tools turned off; then the cursor's line.

## Consequences

- On the native layer, turning the IME off never leaves a composition behind: the IME goes
  off at once, and the text goes where a typed letter would, or is refused as one would be.
  On winit it stays on until the composition ends.
- A logout is held up by oso for at most 5 s. A save that has not ended by then may or may not
  reach the disk, and its buffer has an emergency copy either way.
- A Dock quit behaves as the quit key does.
- No call to the OS is made for a key that only moves the cursor.
- The candidate window can follow the segment being converted, at the cost of a short list of
  offsets with each frame that draws a composition.
- With the IME on, a frame in which the cursor or its line changed copies at most 4000 bytes
  for the input method. Until reconversion lands, an input method that offers it inserts at
  the cursor instead of replacing the text it read.

## Alternatives considered

- **Dropping the wait on winit too:** winit discards the composition, so its text would be
  lost.
- **A longer bound for a logout, or none:** a slow disk could hold up the person's logout.
- **A Dock quit bounded like a logout:** a person who quits from the Dock is waiting for it, as
  with `C-x C-c`, and gets the same rule.
- **Telling the input method after each frame in which the cursor moved:** about one call to
  the OS per key that moves the cursor, for a candidate window that is not open.
- **Committing into the buffer the composition started in:** needs that buffer remembered, and
  an edit outside the selected window.
- **One rectangle for the segment being converted:** less to give, but any other range of the
  marked text would get the start.
- **Laying out the cursor's line to answer any range in it:** positions for up to 4000 bytes,
  for queries that only prediction makes.
- **Giving the cursor's line only while composing:** the input method asks for it at the first
  key, before anything is marked.
- **Reconversion inside the excerpt now:** an edit from another process's range, which needs
  its own checks and tests.
