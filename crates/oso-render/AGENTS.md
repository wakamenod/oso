# oso-render

The wgpu rendering pipeline: surface handling, the glyph atlas, text and
shape passes (ADR 0005, ADR 0036). See `docs/dev/display-model.md`.

- May depend on `oso-core` and `oso-platform`. It is the only crate that may
  use wgpu; no winit (the tip crate hands it the window handle and size).
- wgpu types stay out of its public API where possible: callers pass cells
  and colors, not buffers and pipelines.
- It draws what the display model gives it. Layout and cursor logic belong
  in `oso-core`.
- No per-frame work proportional to the buffer; per-frame allocations in
  the hot path need a reason (see `REVIEW.md`, Performance).
- Frame capture is dev-only (`capture`, behind the `dev` feature).
- Module layers are in `guardrails/layers.txt` (`[oso-render]`).
- Anything `pub` is in the API snapshot (`just public-api`).
