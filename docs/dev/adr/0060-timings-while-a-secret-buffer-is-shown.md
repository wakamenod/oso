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

- **A key leaves no sample if a buffer that may hold secrets is shown just before or just after it.** Keys sent
  together count one by one, so a batch that leaves such a buffer still times the keys after it.
- **Such a key also drops those still waiting for a frame,** so that no sample spans it, and so does a frame presented
  while such a buffer is shown. Neither counts in `dropped_keys`, which would say how many there were.
- **Frames presented while one is shown do not count in `frames`.**
- **What happens once per process stays:** the first frame, since a file counts as secret until it is judged and
  startup is what the benchmarks read, and the first load's times.
- **The rule is in one place,** `SecretWatch`, which every way in for keys uses, and `Timings` takes its answer as a
  required field, so a new way in cannot leave it out by accident.
- `take-timings` itself still answers, with less to report. Samples taken before such a buffer was shown can still be
  read.
- A key that needs no frame leaves no sample either way, as before. In a batch this now holds per key, where a batch
  used to count all its keys if any needed a frame.

This covers `take-timings` and nothing else. What `get-state` reports of such a buffer is
[ADR 0051](0051-m2-plan-decisions.md) and [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md).

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md), a client "can read all state except the text": what
  timings record of keys and frames goes while such a buffer is shown.
- [ADR 0025](0025-latency-measurement.md), latency is measured for keys and reported over the socket: not those
  above.
- [ADR 0046](0046-performance-regression-checks.md), a run fails if "a key gets no frame or a sample is dropped": a
  withheld key is neither. The checks on how many keys drew still catch keys lost this way.

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
- **The state before or after a whole batch deciding for all its keys:** two lines and no new dispatcher method, and the
  same today, since no key changes what is shown yet. But once `switch-to-buffer` arrives (M2), one batch could go in and
  out of such a buffer, and it would go wrong with no sign.
