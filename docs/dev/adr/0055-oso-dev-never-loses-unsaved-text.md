# 0055. `oso dev` never loses unsaved text

- Status: Proposed
- Date: 2026-11-05

## Context

[ADR 0047](0047-process-spawning-and-restart-handoff.md) fixed the order of a
restart while nothing could be unsaved, and
[ADR 0051](0051-m2-plan-decisions.md) decided that a modified buffer holds the
restart until M2-15 can carry the text. [ADR 0053](0053-edit-core.md) brought
`Buffer::is_modified`, and typing (M2-4) must not land before this does. Text
could be lost in four ways:

- The state holds a file's path, so its edits were dropped silently.
- ADR 0047 accepted about 30 ms of lost input between the export and the old
  instance's exit.
- `stop` killed an instance that had not exited after 5 s.
- The instance left beside the old one by a refused import was closed by the next
  build without asking, and the import looked only at the buffer count and name.

A new window may also take the focus, so whatever is typed into it counts as much
as what is typed into the old one.

## Decision

- **An instance goes only when it says so itself.** It answers on its UI thread,
  from the text it holds at that moment. The supervisor assumes nothing about
  what an instance holds, and it no longer kills: `Instance` has no way to force
  an exit. This replaces the kill in ADR 0047.
- **A file with unsaved edits makes `export-state` fail,** naming the buffer and
  quoting nothing. A path-less buffer other than an empty `*scratch*` already did.
  An emptied `*scratch*` holds no text, and undo history is not carried (ADR 0051),
  so it hands over. Secret buffers take the same path, and the message gives only
  the name that [ADR 0044](0044-control-socket-trust-and-secret-buffers.md) already
  allows.
- **`check-handover` is a new method:** what `export-state` would refuse, without
  building a state. It costs one look at each buffer and returns the generation.
  The supervisor calls it before it starts anything, so a held restart opens no
  window to take the focus.
- **A generation names what an instance holds that a file does not:** a 64-bit hash
  of each buffer's id and, for a modified buffer, its content id. `export-state`
  returns it as a new field, and the state stays `version` 1, since a reader ignores
  the field.
  - A buffer that is not modified counts only by its id. Each piece of a load gets a
    new content id, so counting those would refuse an exit whenever a large file was
    still arriving.
  - So a save between the export and the exit does not change it. That text is on the
    disk, and the new instance's copy is a stale file like any other (M2-22).
  - Opening a file changes it. Moving the cursor does not: no text is at stake, and a
    window in use would otherwise never hand over.
  - It is a hash so that it says nothing about a secret buffer but that something
    changed, not how many edits there were. It means something only to the instance
    that made it.
- **`exit` takes an optional `generation`.**
  - Without one, it exits unconditionally, unsaved text or not. That is a person or
    an agent saying so, and it is what `oso ctl exit` and `just e2e` keep sending.
  - With one, it exits only if the generation still matches, and otherwise answers
    with an error starting `exit refused`. The check and the end of the event loop
    happen in one step on the UI thread, so no edit can come between them.
  - Anything else in `generation`, null included, is an error. A caller that lost
    track of its generation never gets the unconditional exit by accident.
- **The order of a restart:**
  1. `check-handover` on the old instance. If it refuses, nothing else happens.
  2. Close the instance beside it, if any. If that cannot be closed, nothing else
     happens, so there are never more than two.
  3. Start the new binary and wait for its window.
  4. Export, and import.
  5. `exit` on the old instance with the generation of the export.
- **Closing an instance** always means `check-handover`, then `exit` with the
  generation just reported. One that has been edited refuses at the first step, and
  one edited in between refuses at the second. A new instance is closed this way
  whenever a restart gives up on it.
- **The rule when the old instance refuses the exit:** the old one stays, and the new
  one goes, unless it has been edited too. Then both stay, each with its own text,
  and the next build waits. The old one is asked first because it holds what the user
  was working on. The new one has only had the state for a moment, and everything in
  it that is not an edit can be had again from the old one.
- **The rule when the old instance does not answer or does not exit:** both stay,
  since it may have agreed and be on its way out, and then the new one has to be
  there. If it does exit, the one beside it becomes the current one, as already
  happens when the user closes it. The answer `exit refused` is what tells the two
  cases apart. Other errors, such as the UI thread not replying in time, say nothing
  about what the instance will do.
- **`import-state` also refuses a modified `*scratch*`,** whose text the imported
  buffers would hide. That leaves the new instance beside the old one, and the next
  build holds until the user has dealt with it.
- **`insert-text` is a new dev method,** inserting text at the primary cursor as one
  edit. It exists because a held restart has to be testable over the real socket
  before anything can be typed, as `show-buffer` did for switching buffers. Its reply
  gives only a length. It goes away with M2-4.
- **`oso ctl exit` takes the generation as an optional argument,** so that all of this
  can be driven by hand.

## Consequences

- Nothing is lost at a restart, at the price of restarts that wait. Until M2-15, any
  unsaved edit holds every restart, and the log says which buffer. Saving or undoing
  lets the next build through. Build errors and this message reach the echo area with
  M2-15.
- A clean build after the user saves does not restart by itself. It takes the next
  source change, as after any failed try.
- An instance that cannot be stopped stays until someone ends it. A new binary that
  hangs before its window therefore holds later restarts, and the log gives its
  process ID. Windows that are not closed were the point, but this one holds nothing
  and could have been killed safely. Telling such a case apart is not worth a kill in
  the supervisor.
- Keys the window system still holds for the old window when it exits are lost, as on
  any close. They never reached a buffer. Keys handled before the exit are covered,
  and this window is now a matter of milliseconds of keys queued behind the exit,
  while the new window usually has the focus.
- An `exit` that arrives after the UI thread stalled for more than 10 s can still
  run once it wakes. The server has by then answered that it did not reply in time, so
  both instances stay until then, and the guard still applies when it runs.
- A running `oso dev` keeps its old code until it is restarted, so it does not have
  this protection, and `check-handover` and the guarded `exit` work only from a
  supervisor started after this change.
- M2-4 removes `insert-text` and moves its flow to `send-keys`. M2-15 moves the state
  to version 2 and carries the text, and `export-state` then stops refusing modified
  buffers that are not secret. The generation already covers what it will carry.

## Alternatives considered

- **Checking only that nothing is modified, with no generation:** enough while the
  state holds no text, since nothing modified can be exported. It would not close the
  gap between asking and exiting, and it would have to be redone for M2-15.
- **The content ids themselves, or their sum:** the sum can collide when two buffers
  trade ids through undo, and a list shows how often a secret buffer was edited.
- **Counting content ids of buffers that are not modified:** also catches a save, but
  refuses an exit on every piece of a load.
- **A new method for the guarded exit, leaving `exit` as it is:** one more name for the
  same thing, and the method a caller reaches for first would stay the unguarded one
  either way.
- **Making every `exit` refuse unsaved text, with a `force` for people:** safer for
  agents, but breaks `just e2e` and the harness's cleanup, and M2's `C-x C-c` prompt is
  where that belongs.
- **A result of `{"exiting": false}` instead of an error:** a typed answer rather than
  a matched prefix, but `oso ctl exit G` would then report success for an exit that did
  not happen.
- **Closing the new instance when the old one does not answer:** simple, but if the old
  one was on its way out, both windows go.
- **Killing an instance that never showed a window:** it can hold nothing typed, but a
  same-user client may have reached its socket, and a supervisor that cannot kill is
  easier to trust.
- **A third slot, so that an edited instance beside the old one does not hold the
  restart:** restarts continue, but windows pile up and each holds different text.
