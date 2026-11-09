# 0079. State and restarts around a secret buffer

- Status: Accepted, amended by 0090
- Date: 2026-11-08

## Context

`get-state` gives no text, positions or marks of a buffer that may hold secrets, and no
modeline or echo area while one is shown
([ADR 0044](0044-control-socket-trust-and-secret-buffers.md),
[ADR 0051](0051-m2-plan-decisions.md), [ADR 0058](0058-word-motion-prefix-argument-and-secret-positions.md),
[ADR 0063](0063-modeline-echo-area-and-internal-buffers.md)). The rest of its metadata stays, and some of
it follows what a person does there: whether it is modified or being saved, and its size.
So do the keys in progress, and the prompts open over it, which can outlast it on the screen.
`check-handover`, `export-state` and a guarded `exit` say whether a buffer is modified too, as
that is what decides a restart ([ADR 0055](0055-oso-dev-never-loses-unsaved-text.md)).

## Decision

- **While any window shows a buffer that may hold secrets, `get-state` gives no `modified`,
  `saving`, `bytes`, `lines` or load progress for such buffers.** Once none is shown, they come
  back, with the size still left out while one is modified, as ADR 0058 has it.
- **`invalid_bytes` is null for such a buffer,** shown or not. `read_only` still says
  `invalid-utf8`.
- **While one is shown, `check-handover`, `export-state` and an `exit` with a generation are
  refused for one reason,** whatever the buffers hold, so the answer is the same before and
  after an edit. An `exit` without a generation still goes. A file not judged yet does not
  count here: it is loading, as after a restart, and its positions are the ones it came in with.
- **A restart does not show such a buffer again from behind an internal buffer:** a window on
  `*Messages*` or the list of copies with one behind it comes back on `*scratch*`, and the
  buffer is handed over unshown, so the next restart is not held by it.
- **Keys in progress typed, or still pending, while such a buffer is shown are left out of
  `get-state`,** in `keys.pending` and the echo area, until they end, and `export-state` hands
  them over as none.
- **A prompt open while one was shown beneath it, or opened then, gives no label, answer,
  cursor, message or region, and its answer enters no history,** also once that buffer is gone.
- **A composition open while one was shown gives no preedit, range or area** until it ends.
- **`screenshot` is refused while any of these is left out,** as the screen shows them.
- **What was around such a buffer is noted where it can change:** each change of what a window
  shows, a change of a shown buffer's verdict, and keys and a composition as they come. So no
  caller has to remember it.

### Where this differs from accepted ADRs

These ADRs stay as they are; this one changes the following:

- [ADR 0044](0044-control-socket-trust-and-secret-buffers.md), a client "can read all state
  except the text": not the fields above.
- [ADR 0055](0055-oso-dev-never-loses-unsaved-text.md), `check-handover` names a buffer that would
  lose text: not while a buffer that may hold secrets is shown.
- [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), what `get-state` shows of a
  prompt and what enters a history: as above.

## Consequences

- `oso dev` does not restart while such a buffer is on the screen. It says why, as for any
  refusal, and the next change restarts once another buffer is shown.
- A test that reads these fields of such a buffer shows another buffer first.

## Alternatives considered

- **Leaving them out always for such a buffer:** `check-handover` would still say whether it
  is modified, and a restart cannot go without knowing.
- **Leaving the fields as they are:** they hold no text, but they follow what a person does in
  such a buffer.
- **Cancelling the keys in progress when `show-buffer` moves away:** changes what the person
  typed, where leaving them out of the state changes nothing they see.
