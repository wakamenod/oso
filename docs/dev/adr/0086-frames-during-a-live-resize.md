# 0086. Frames during a live resize

- Status: Proposed
- Date: 2026-11-08

## Context

[ADR 0070](0070-windowing-and-input-without-winit.md) draws frames on demand, when the run
loop is about to wait, on oso's own macOS layer as on winit. While the person drags the
window's edge, AppKit runs its own tracking loop, and the run loop's hooks run there too, so
frames still come. But each step of the drag resizes the window first, and its frame comes
only when the loop next waits. In between, the Metal layer shows the last frame stretched to
the new size, so text can wobble or smear while the window is dragged.

[ADR 0077](0077-choices-for-the-native-layers-stage-2.md) left the live resize, and the
startup measured again against winit, for the rest of stage 2. The author made the choices
below on 2026-11-08, through the coordinator.

## Decision

- **Each step of a live resize is drawn at once.** Between `viewWillStartLiveResize` and
  `viewDidEndLiveResize`, the view's Metal layer presents with `presentsWithTransaction`, and
  `windowDidResize:` gives the app the new size, then draws the frame it asks for, then and
  there. The frame and the size land in the same Core Animation transaction, so nothing is
  stretched. Outside a live resize, frames come when the loop waits, as before.
- **It is checked at the keyboard, with numbers.** A drag cannot be made from the dev socket
  or a script, as only a mouse on the window's edge starts AppKit's tracking loop. So the
  author drags a corner slowly and fast, before the native layer becomes the default, and
  `get-state`'s `live_resize` (dev builds only) gives the steps of the last live resize and the
  frames drawn for them. The state that decides, `gate::LiveResize`, has unit tests.
- **Startup is measured against winit on the Mac mini.** `just perf-remote` takes the base's
  features apart from the new side's, so that
  `just perf-remote origin/main 5 dev,native-window dev` measures the native layer against
  winit, in 5 pairs.

## Changes to accepted ADRs

ADR 0070's "a redraw request is a flag, drained once before the loop waits" becomes: it is
drained once before the loop waits, and also at each step of a live resize, where the frame
is drawn as the window takes its new size.

## Consequences

- Text keeps its shape while the window is dragged.
- During a live resize, each step waits on the main thread until its frame is scheduled on
  the GPU, which is what `presentsWithTransaction` asks. A drag on a slow GPU may feel heavier
  than winit's, which stretches instead. Nothing waits outside a drag.
- The counts let the keyboard check say whether a step went without a frame.

## Alternatives considered

- **Keeping frames for when the loop waits, with the layer's contents pinned to the top left:**
  no waiting, but a bare strip shows at the new edge until the next frame.
- **Measuring first and choosing after:** a drag's look is what the person sees, and the
  counter alone cannot say whether a frame was stretched.
- **Running the startup comparison on the author's MacBook:** it needs a long window slot on
  a machine in use, and earlier runs there were noisy.
