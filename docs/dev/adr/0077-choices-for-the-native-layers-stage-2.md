# 0077. Choices for the rest of the native window layer's stage 2

- Status: Proposed
- Date: 2026-11-08

## Context

[ADR 0070](0070-windowing-and-input-without-winit.md) replaces winit with oso's own window
layer. Its stage 2, the macOS layer behind `native-window`, landed in parts, and
`open-questions.md` lists what it still owes before the layer becomes the default on macOS.
Five of those items left a choice open, which the author made on 2026-11-08 through the
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

## Consequences

- On the native layer, turning the IME off never leaves a composition behind: the IME goes
  off at once, and the text goes where a typed letter would, or is refused as one would be.
  On winit it stays on until the composition ends.
- A logout is held up by oso for at most 5 s. A save that has not ended by then may or may not
  reach the disk, and its buffer has an emergency copy either way.
- A Dock quit behaves as the quit key does.
- No call to the OS is made for a key that only moves the cursor.

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
