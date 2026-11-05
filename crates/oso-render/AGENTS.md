# oso-render

The wgpu rendering pipeline: surface handling, the glyph atlas, and panes drawn
in layers with text and shape pipelines (ADR 0005, ADR 0036, ADR 0060). See `docs/dev/display-model.md`.

- May depend on `oso-core` and `oso-platform`. It is the only crate that may
  use wgpu; no winit (the tip crate hands it the window handle and size).
- wgpu types stay out of its public API. Callers pass a list of panes (ADR
  0060), never buffers or pipelines. The one intended exception is the window
  handle the tip crate hands over (`wgpu::DisplayAndWindowHandle`).
- It draws what the display model gives it. Layout and cursor logic belong
  in `oso-core`. What a frame draws is built in `scene` with no GPU, so test
  it there.
- No per-frame work proportional to the buffer; per-frame allocations in
  the hot path need a reason (see `REVIEW.md`, Performance).
- Frame capture and the GPU report are dev-only (`capture`, behind the `dev`
  feature).
- Module layers are in `guardrails/layers.txt` (`[oso-render]`).
- Every Rust file opens with its responsibility and a `Must not depend on:`
  paragraph (ADR 0041). The test checks names in backticks against the layers.
- Anything `pub` is in the API snapshot (`just public-api`).
