# 0060. Timings while a secret buffer is shown

- Status: Proposed
- Date: 2026-11-05

## Context

Dev builds keep key latency samples and a count of frames, for the benchmarks, and any client of
the control socket can read them with `take-timings`
([ADR 0025](0025-latency-measurement.md), [ADR 0046](0046-performance-regression-checks.md)). They
hold no text. [ADR 0044](0044-control-socket-trust-and-secret-buffers.md) says a client can read all
state but the text of a buffer that may hold secrets, which leaves timings in. But they show when keys
were pressed and how many, and that includes a person typing in such a buffer.

## Decision

- **A key leaves no sample if a buffer that may hold secrets is shown before or after it.** Keys sent
  together count one by one, so a batch that leaves such a buffer still times the keys after it. A key
  still waiting for a frame drops if that frame is presented while such a buffer is shown.
- **Frames presented while one is shown do not count.**
- **The first frame and the first screen of a load are still recorded,** since a file counts as secret
  until it is judged, and startup and load times are what the benchmarks open files to read. They happen
  once.
- **`Timings` takes this as a required argument** on `keys_handled` and `frame_presented`, so a new way in
  for keys cannot leave it out.
- `take-timings` itself still answers, with less to report. Samples taken before such a buffer was shown can
  still be read.
- A key that needs no frame leaves no sample either way, as before. In a batch this now holds per key, where a
  batch used to count all its keys if any needed a frame.

This covers `take-timings` and nothing else. What `get-state` reports of such a buffer is
[ADR 0051](0051-m2-plan-decisions.md) and [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md).

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md), a client "can read all state except the text": what
  timings record of keys and frames goes while such a buffer is shown.
- [ADR 0025](0025-latency-measurement.md), latency is measured for keys and reported over the socket: not those
  above.

## Consequences

- Latency in such a buffer cannot be measured over the socket, as screenshots and text are not available there.
- A benchmark that sends keys before a file is judged would lose them. `big_file_run` now waits for the verdict before
  its keys while loading, and says so if none of them was timed, as `keys_loading_max_ms` is then left out of the run.
  No threshold changes.

## Alternatives considered

- **Refusing `take-timings` while a secret buffer is shown:** a client could switch to another buffer and
  read what was recorded.
- **Recording only the number of keys, or a coarser time:** still says when a person typed.
- **Moving the timings behind a feature of their own:** they are already dev-only, and benchmarks need them
  on ordinary files.
- **The state after a whole batch deciding for all its keys:** simple, but a key that leaves such a buffer would count.
