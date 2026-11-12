# 0124. Three drawables

- Status: Accepted
- Date: 2026-11-13

## Context

The surface has presented with `Fifo` and a frame latency of 1 since M1-1 (the roadmap).
`docs/dev/architecture.md` left `Mailbox` or `Immediate` to be revisited once key-to-photon
latency was measured ([ADR 0025](0025-latency-measurement.md)). wgpu asks for one image more
than the frame latency: two drawables on Metal, and on Vulkan and DX12 two images within the
range the driver allows.

[ADR 0123](0123-thresholds-for-the-report-only-metrics.md) recorded `file_prompt_type_p99_ms`,
typing into `C-x C-f` over 100,000 names, as near the 8 ms budget on the reference Mac: 4.75
to 7.31 ms over 6 runs, with 6 of 30 processes over 8 ms. The app's own spans of each key
(`key_spans_us`) were read on the reference Mac, which has a 60 Hz display, over 3 processes
of 400 keys. They show where the time went:

- The UI thread's own work for a key stayed under 1 ms: handling the key, preparing its frame
  and presenting it.
- Every slow key waited to acquire a drawable, for up to 13.7 ms.
- Each key draws two frames: its own, where the count shows `…`, and then the one with the
  worker's result, a few milliseconds later
  ([ADR 0096](0096-completion-on-a-worker-past-5000-candidates.md)). The next key's frame
  needs the drawable of two frames before, which the compositor may still hold. Plain typing
  draws one frame a key and never waited.
- [ADR 0112](0112-window-splits.md) found the same for `C-x o`: a frame asked for within one
  refresh of the last waited for the next.

The wait is in acquiring the drawable, on the UI thread, so input waits with it.

The same change makes the worker's matching faster, with the same results. A component that
is both a literal and an expression ([ADR 0076](0076-completion-and-m-x.md)) is searched with
one expression that joins the two, where they compile together, have no class and stay small.
A search that need not say where it matched stops at the first match. Matching 100,000 names
took 4 to 5 ms a key before and 1.3 to 2.9 ms after.

The key-to-present p99, in ms, as the benchmark measures it:

| Build | p99 of each of 3 processes | Longest wait for a drawable |
|---|---|---|
| `main` | 6.37, 6.54, 9.33 | 13.7 ms |
| Faster matching only | 6.70, 6.37, 9.92 | 12.3 ms |
| Faster matching, frame latency 2 | 2.15, 2.19, 2.10 | 0.13 ms |

Faster matching alone did not move the 99th percentile.

## Decision

The author chose this on 2026-11-13, through the coordinator.

- **Every surface presents with `Fifo` and a frame latency of 2**, which asks for three
  drawables. The present mode stays `Fifo`.
- `crates/oso-render/src/lib.rs` sets it where the surface is configured, and
  `docs/dev/architecture.md` says so. Reconfiguring and recreating the surface keep it.

## Consequences

- A key that draws twice, as one that leaves a job for a worker does, no longer makes the next
  key wait on the UI thread for a drawable. `C-x C-f` over 100,000 names measured 2.1 to
  2.2 ms at p99 on the reference Mac, down from 6.4 to 9.3 ms.
- The benchmark measures from a key to its frame being presented, not shown. The wait moved
  from the UI thread into the compositor's queue: the frame is handed over at once. When it
  shows was not measured. The drawable two frames back was held long after the frame after it
  was presented (25 ms or more between keys), so it should show no later than before, and
  usually sooner. Measuring when frames show (ADR 0025) is follow-up work.
- One more drawable at the window's size, 4 bytes a pixel: up to about 25 MB for a window of
  1600 × 1000 points at 2x, and 33 MB for a full 4K screen. Metal makes it only when it is
  needed. Vulkan and DX12 allocate it up front, unless the driver already gave three images.
- When frames come faster than the display shows them for a while, up to two can queue
  instead of one, so what shows can lag by one more refresh, 16.7 ms at 60 Hz. Examples are a
  fast scroll, or a key held down in a prompt over 5000 candidates, which draws twice a
  repeat. Those frames used to block the UI thread in acquiring a drawable instead, so fewer
  events were drawn at once. A live resize on macOS is unaffected: each frame is part of the
  resize's transaction ([ADR 0086](0086-frames-during-a-live-resize.md)).
- A frame at least a refresh after the last never waited, and is unchanged. In the runs above,
  the median key and every span but the wait were the same with three drawables as with two.
- [ADR 0046](0046-performance-regression-checks.md) says a frame that costs the GPU more shows
  in key latency only once drawables back up. With three, they back up later, so such a cost
  shows later in `just perf`.
- ADR 0123's note on `file_prompt_type_p99_ms` as a known near-budget metric no longer
  holds. The reference Mac's threshold for it was derived from the old spread, and now allows
  more than three times the new baseline. Setting it again from 6 runs of `main` is the
  author's (`compare.rs` is a guard file).
- ADR 0112's note that a frame asked for within one refresh of the last waits for the next
  holds no longer for two frames in a row. `C-x o` was not measured again here.
- CI's runners have no hardware GPU, so only the reference Mac shows the change.

## Alternatives considered

- **Keep two drawables and draw one frame a key**: hold a key's own frame for a few
  milliseconds while the worker matches, drawing the result in its place if it comes in time.
  It amends ADR 0096, puts the match in every key's latency, and leaves `C-x o` and any other
  key that draws twice as they were.
- **Keep two drawables and accept the near-budget metric**, with only the faster matching. It
  did not move the 99th percentile.
- **`Mailbox` or `Immediate`**: not supported on every backend, and `Immediate` tears.
  `Fifo` with three drawables was enough.
