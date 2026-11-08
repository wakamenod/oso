# 0073. Secure input in secret buffers

- Status: Accepted
- Date: 2026-11-08

## Context

A buffer that may hold secrets already gives the control socket no text and no pixels
([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)), keeps its keys out of timings
([ADR 0060](0060-timings-while-a-secret-buffer-is-shown.md)) and has the input method off
([ADR 0069](0069-macos-input-method.md)). macOS gives password fields one thing more:
secure event input, which keeps keystrokes for the app with the keyboard focus and out of
other processes' event monitoring. The author chose on 2026-11-08 to give secret buffers
the same, with oso's own window layer on macOS
([ADR 0070](0070-windowing-and-input-without-winit.md)).

HIToolbox (Carbon) provides it with three calls: `EnableSecureEventInput`,
`DisableSecureEventInput` and `IsSecureEventInputEnabled`. Enabling is counted and
process-wide: each enable needs its disable, and while any is held, other apps' global
hotkeys and keyboard tools stop working. So it must be held only while it is needed, and
never left on.

## Decision

The author chose on 2026-11-08, from options put to them: an ADR of its own, secure input on
oso's own window layer only, the person's input sources left alone, and a pull request of its
own after the native layer's first one. The author accepted this ADR with those four choices on
2026-11-08, through the coordinator.

### When it is on

- **Secure input is on while all of these hold:** oso is the active app, its window has the
  keyboard, and the selected buffer may hold secrets (`Buffer::is_secret`, the same verdict
  that turns the input method off in ADR 0069). A passphrase prompt
  ([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)) is such a buffer, with
  the verdict `Passphrase`, so typing one gets secure input with no condition of its own.
- **It goes off** when any of them stops: the app deactivates, the window loses the keyboard,
  another buffer is selected, the window hides for an exit, the loop ends, or the app quits.
- **One enable is held while it is wanted.** The window layer counts the enables the system
  holds for oso, calls `EnableSecureEventInput` when it is wanted and none is held, and
  `DisableSecureEventInput` for each one held when it is not. Once the loop has ended nothing
  enables it again. Nothing else in oso calls them.
- **Before the loop ends,** in `exiting` and in `applicationWillTerminate:`, a held enable is
  released. After a panic in the handler is caught on the main thread, it is released
  before the loop stops. The panic hook itself does not call it, as it may run on any
  thread and must stay small ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)). The
  system drops the enable when the process ends, also after a kill, which is checked by hand
  (below).

### Who decides

- **The app decides whether it wants it; the window layer decides when it holds it.** The app
  passes one flag, `Window::set_secure_input(bool)`, from the same hook after every event
  that keeps the input method's state (ADR 0069), computed from the selected buffer's
  verdict. No text, name or position of a buffer crosses to the window layer.
- **The window layer holds it** only while the flag is set and oso is active with its window
  key. It follows `applicationDidBecomeActive:`, `applicationDidResignActive:`,
  `windowDidBecomeKey:` and `windowDidResignKey:`, and checks again when the flag changes.
- The rule is a plain value (`window/appkit/gate.rs`): whether the app wants it, whether oso
  is active and its window key, whether the loop has ended, and how many enables the system
  holds for oso. It says which call to make next, and is tested without a window, over every
  combination of the first four and with calls the system refuses. Only the native layer uses
  it; winit's adapter takes the flag and does nothing with it.
- Whether oso is active and its window key is read from AppKit when the window opens and at
  each notice of a change, so a notice missed cannot leave it wrong.
- Secure input is retried until held: an enable the system refuses is asked for again at the
  next turn of the loop. A refused release never blocks a later enable, and is asked for again
  until the system's count is back to none.
- An enable the system refuses is not held. The flag is set at once, as these calls do not
  call back into the window.

### The calls

- **An `extern "C"` block in `oso_platform::window::appkit::secure`,** linked against the Carbon
  framework, which AppKit loads already. No crate binds these calls, and none is added. Each
  call has a `// SAFETY:` comment: they take no arguments, and are called only on the main
  thread.
- `IsSecureEventInputEnabled` is used only by the dev socket's check (below), never to decide.

### With the input method

- The input method is already off in a buffer that may hold secrets (ADR 0069), so keys
  there arrive as keys whatever secure input does to input sources. Whether macOS changes the
  selected input source while secure input is held is checked at the keyboard, and oso does
  not depend on either answer.

### Tests

- **Unit:** the rule's table, every combination of the four booleans, with no real calls, and
  a sequence that shows enables and disables always pair.
- **Socket:** `get-state` gains `secure_input`: `wanted`, `held` by the window layer, and
  `system_any`, from `IsSecureEventInputEnabled`, which is true while any process holds secure
  input; null on winit, and only in dev builds. It says
  nothing a process of the same user cannot read already
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)). An e2e flow under
  `native-window` (`just e2e-native`) opens a file with a secret name and checks that it is
  wanted, then neither wanted nor held after another buffer is selected. Whether it is held
  depends on oso being in front with the keyboard, which a test run does not control.
- **By hand:** `ioreg -l -w 0 | grep kCGSSessionSecureInputPID` shows oso's process ID while a
  secret buffer is selected and the window has the keyboard, and none after switching to
  another app, another buffer, or quitting, and none after `kill -9` of an instance that held
  it.

## Consequences

- Secret buffers get secure input, as password fields do.
- While one is selected and oso is in front, other apps' global hotkeys and keyboard tools
  do not see keys, as with a password field. They work again as soon as oso goes behind or
  another buffer is selected.
- One `extern` block and three `unsafe` calls join `oso-platform`, on macOS with
  `native-window`.
- winit's layer does not get it; it comes with the native layer.

## Alternatives considered

- **Secure input on the winit layer too,** from winit's focus events: works until winit goes,
  but winit reports no app activation, and the native layer replaces it on macOS.
- **Calling enable and disable on every event, counting on the system's counter:** one
  missed disable leaves it on until oso exits.
- **Disabling it in the panic hook:** the hook may run on another thread and must not grow.
  Releasing it on the main thread once the panic is caught, and at process end, covers it.
- **Allowing only Roman input sources while a secret buffer is selected,** as password fields
  also do: the input method is already off there, and switching the person's input source
  for them is a change of its own.
