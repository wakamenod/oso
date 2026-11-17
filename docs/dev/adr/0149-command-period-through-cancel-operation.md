# 0149. Command-period through `cancelOperation:`

- Status: Accepted
- Date: 2026-11-17

## Context

On macOS, Command is Meta ([ADR 0033](0033-key-presses-and-notation.md)), so `M-.` is Command-`.`.
In the installed oso, `M-.` did nothing: no jump, no message. `lsp-find-definition` itself
worked when run by name.

oso's own AppKit view takes keys in `keyDown:` ([ADR 0070](0070-windowing-and-input-without-winit.md)).
AppKit's window does not send Command-`.` there. It treats the key as "cancel" and sends
`cancelOperation:` to the first responder instead, which NSResponder ignores, so the key was
dropped before oso saw it. A test with a window that is never shown and key events made by
hand showed which keys this happens to: Command with a character of `.`, whatever key types
it (the numeric keypad's too). Command-Shift-`.`, Command-Control-`.`, Command-Option-`.`,
Command-`,` and Escape all reach `keyDown:`. winit, which oso used before, overrides
`cancelOperation:` for the same reason.

The author chose on 2026-11-17, through the coordinator, to fix this on its own, ahead of a
way to see which keys oso received.

## Decision

- **The view takes `cancelOperation:` as the key it stands for.** It reads the event AppKit is
  dispatching (`NSApplication`'s `currentEvent`) and routes it as `keyDown:` would, so text
  being composed is committed first, as for any Command key.
- **Only a new key-down counts.** The event must be a key-down for the view's own window,
  with Command held. AppKit's current event stays current after its dispatch, so the view
  keeps the last key-down it routed, by its time, key code and window, and never routes that
  one again, and takes none while another event is being handled. Anything else, such as an
  Accessibility action, another sender, or no current event, is ignored. The check is a plain
  function with unit tests.
- **It is checked at the keyboard.** The control socket's keys do not pass through AppKit's
  dispatch, so they cannot show this. The author presses Command-`.` in the installed app.

## Changes to accepted ADRs

ADR 0070's "a key press comes only from the event `keyDown:` is handling" becomes: a key press
comes only from a key-down AppKit dispatches, either the event `keyDown:` is handling or the
one the window sends `cancelOperation:` for in its place. 0070 gains `amended by 0149`.

## Consequences

- `M-.` reaches the keymap, and with it `lsp-find-definition`.
- Text services still give no key. Their `cancelOperation:` can come only with a key-down
  the view already routed, which it refuses, or with one AppKit used for itself and never
  sent to the view. A sender that can cause the latter needs Accessibility permission, which
  already lets it post keys.
- The fix has no automated end-to-end test, since only keys that go through the window server
  reach this path.

## Alternatives considered

- **`performKeyEquivalent:` matching `.` itself:** the event comes in hand, but oso would copy
  AppKit's test of which keys mean cancel, which goes by the character typed and so by the
  layout. `cancelOperation:` gets exactly the keys AppKit diverts.
- **An e2e test that types through System Events:** it needs Accessibility permission for the
  test runner, which CI does not have.
