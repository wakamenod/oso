# 0078. Load times and log lines around a secret buffer

- Status: Accepted
- Date: 2026-11-08

## Context

[ADR 0060](0060-timings-while-a-secret-buffer-is-shown.md) keeps key samples and frames out of
`take-timings` while a buffer that may hold secrets is shown, and keeps what happens once per
process: the first frame, and the first load's times. A load's times depend on the file's text,
as its first screen does on what that screen shows. The log has the same kind of lines: how long
a key, a frame, a save or a load took, and which keys were pressed, each with the time it was
written.

## Decision

- **A load of a buffer that may hold secrets is not timed.** The first load's times in
  `take-timings` are those of the first file judged not to hold secrets. A buffer is judged
  before its first content arrives, so the verdict is known when the times start.
- **A first screen drawn while such a buffer is shown is not recorded,** and the load then has
  none, not a later one.
- **No timing line reaches the log while such a buffer is shown,** or for a key that left one:
  how long a key, a text, a frame or a save took. The renderer logs no timings of its own, as it
  does not know what a pane shows. The app's line for each frame stands for them.
- **The loading thread logs no times either.** The UI thread logs a finished load, with its size,
  only for a buffer that cannot hold secrets, as the thread does not know the verdict.
- **Keys pressed while such a buffer is shown are not logged,** as a pending prefix, an undefined
  key or a quit. What the echo area shows is unchanged.
- The first frame of the process stays, as ADR 0060 has it.

### Where this differs from accepted ADRs

ADR 0060 stays as it is; this one changes the following:

- [ADR 0060](0060-timings-while-a-secret-buffer-is-shown.md), "what happens once per process
  stays: … the first load's times": not for a buffer that may hold secrets, nor a first screen
  drawn while one is shown.

## Consequences

- The benchmarks load ordinary files, so their numbers do not change.
- A trace log of a session in such a buffer shows less, and a slow frame there cannot be found in
  it. Measuring one needs an ordinary file with the same text.
- The renderer's own trace lines for preparing and presenting a frame are gone. The app's
  "frame drawn" line covers both.

## Alternatives considered

- **Keeping the first load's times, as ADR 0060 does:** they are once per process, but
  `take-timings` can be read right after a secret file is opened.
- **Dropping the times once a buffer is judged secret, after recording them:** the verdict comes
  before the first content, so there is nothing to wait for.
- **Telling the renderer whether a secret is shown, so it can keep its lines:** a new argument
  for two log lines, which the app's line already covers.
- **A coarser time in the log:** still says when keys were pressed.
