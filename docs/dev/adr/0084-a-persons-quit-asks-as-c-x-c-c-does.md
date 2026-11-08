# 0084. A person's quit asks as `C-x C-c` does

- Status: Proposed
- Date: 2026-11-08

## Context

[ADR 0070](0070-windowing-and-input-without-winit.md) has oso's own macOS layer answer the
OS's requests to end (`applicationShouldTerminate:`). Its "Quitting" rule says a Dock quit
while a buffer is modified "is refused with the guard's message, and the next goes". That
predates [ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md), since which the
exit guard of [ADR 0059](0059-safe-save-and-the-exit-guard.md) is a question:
`C-x C-c` asks "exit without saving? (y or n)". A Dock quit that goes on the second try
would be the one way to exit past it.

[ADR 0077](0077-choices-for-the-native-layers-stage-2.md) set how long the end of the
session waits for saves (choice 2) and that a Dock quit with a save running waits as
`C-x C-c` does (choice 3). How the end of the session is tested was left open. The author
made both choices below on 2026-11-08, through the coordinator.

## Decision

- **A person's quit is oso's own close request.** A quit with no reason, or with any reason
  but the end of the session (the Dock, another app, a script), is answered no to the OS
  (`NSTerminateCancel`). The app then handles it as the window's close button and
  `C-x C-c` are handled: with a modified buffer the exit question opens, `y` exits and
  counts as agreed, so nothing is written ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)),
  and `n` stays. With nothing modified oso exits at once, and with a save running it waits
  with the window hidden, as an exit does (ADR 0077, choice 3).
- **The end of the session** (a logout, a restart or a shutdown, told by the quit Apple
  event's `keyAEQuitReason`) is never refused, as ADR 0070 says. It waits at most 5 s for
  running saves (ADR 0077, choice 2), and `exiting` writes copies of what is at risk.
- **The e2e flows send real quit Apple events.** The end of the session is a quit Apple
  event with `keyAEQuitReason` set to a logout (`kAELogOut`), sent through osascript's
  Objective-C bridge, so the flow goes through `applicationShouldTerminate:` as a logout
  does. A person's quit is the one `NSRunningApplication.terminate` sends, with no reason.
- The window layer's API for this (`Handler::quit_requested`, `QuitReason`, `QuitAnswer`
  and `Control::answer_quit`) follows ADR 0070, and the author approves it with its pull
  request.

## Consequences

- No request to end gets past the exit question but the end of the session, which never
  waits on a person.
- The reason decides only whether the question is asked. Any local process can send a quit
  with a logout reason, and gets what a logout gets: copies of what is at risk, and an end.
- On winit nothing changes: winit does not ask the app, so any request to end ends it and
  writes copies of what is at risk, until the native layer is the default.
- The e2e flow for the end of the session needs osascript to send an Apple event to the
  instance. If a runner's privacy settings refused that, the flow would fail rather than
  pass unchecked.

## Changes to accepted ADRs

ADR 0070's "A Dock quit is guarded as `C-x C-c` is ... the first one is refused
(`NSTerminateCancel`) with the guard's message, and the next goes" becomes: a person's quit
is refused to the OS and opens the exit question, as `C-x C-c` does. The rest of its
"Quitting" section holds.

## Alternatives considered

- **Refusing once with a message, and letting the next quit go,** as ADR 0070 wrote: a second
  Dock quit would exit past the question that `C-x C-c` asks, with no answer.
- **A dev-only socket request standing in for a quit with a reason:** simpler, but it would
  not go through `applicationShouldTerminate:` or read the Apple event, which is what can
  break.
