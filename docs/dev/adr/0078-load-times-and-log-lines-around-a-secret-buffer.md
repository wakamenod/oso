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
- **While a buffer that may hold secrets is shown, the log keeps warnings and errors only,**
  from every crate. That holds through an event that begins or ends with one shown, and, for
  keys sent together, from the key around one to the end of the batch. One logger does
  this, behind a flag the app sets as it handles each event and each key, so no line has to
  ask, and a new one cannot forget to.
- **The loading thread logs no times.** The UI thread logs a finished load, with its size, only
  for a buffer that cannot hold secrets, as the thread does not know the verdict and its lines
  can come at any time.
- The first frame of the process stays, as ADR 0060 has it.

### Where this differs from accepted ADRs

ADR 0060 stays as it is; this one changes the following:

- [ADR 0060](0060-timings-while-a-secret-buffer-is-shown.md), "what happens once per process
  stays: … the first load's times": not for a buffer that may hold secrets, nor a first screen
  drawn while one is shown.

## Consequences

- The benchmarks load ordinary files, so their numbers do not change.
- A log of a session in such a buffer shows less, and a slow frame there cannot be found in it.
  Measuring one needs an ordinary file with the same text.
- A warning or an error still shows, and must quote no text, as ADR 0028 has it for every line.

## Alternatives considered

- **Keeping the first load's times, as ADR 0060 does:** they are once per process, but
  `take-timings` can be read right after a secret file is opened.
- **Dropping the times once a buffer is judged secret, after recording them:** the verdict comes
  before the first content, so there is nothing to wait for.
- **A check at each line that logs a time or a key:** lines are added all the time, and one
  without the check would say nothing of it.
- **Keeping info lines:** they name files and say what happened, which can say as much.
