# 0060. Timings while a secret buffer is shown

- Status: Proposed
- Date: 2026-11-05

## Context

Dev builds keep key latency samples and a count of frames, for the benchmarks, and any client of
the control socket can read them with `take-timings`
([ADR 0025](0025-latency-measurement.md), [ADR 0046](0046-performance-regression-checks.md)). They
hold no text, so [ADR 0044](0044-control-socket-trust-and-secret-buffers.md) did not cover them. But
they do show when keys were pressed and how many, and that includes a person typing in a buffer that
may hold secrets.

## Decision

- **While any window shows a buffer that may hold secrets, timings record no keys and count no frames.**
  A key handled then leaves no sample, and one still waiting for a frame drops when that frame is
  presented.
- **The first frame and the first screen of a load are still recorded,** since a file counts as secret
  until it is judged, and startup and load times are what the benchmarks open files to read. They happen
  once, and say nothing of later keys.
- `take-timings` itself still answers. It only has less to report.
- Samples taken before a secret buffer was shown can still be read, as they say nothing about it.

## Consequences

- A benchmark must not time keys in a file that counts as secret. Those it runs wait for the file to
  load, so it is judged by then, and a run with no samples fails.
- A person's keys in a secret buffer leave no trace in dev timings.
- Latency in such a buffer cannot be measured over the socket, which matches screenshots and text.

## Alternatives considered

- **Refusing `take-timings` while a secret buffer is shown:** a client could switch to another buffer and
  read what was recorded.
- **Recording only the number of keys, or a coarser time:** still says when a person typed.
- **Moving the timings behind a feature of their own:** they are already dev-only, and benchmarks need them
  on ordinary files.
