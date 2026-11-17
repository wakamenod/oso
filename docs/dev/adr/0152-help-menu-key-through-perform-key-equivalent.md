# 0152. The Help menu key through `performKeyEquivalent:`

- Status: Accepted
- Date: 2026-11-17

## Context

On macOS, Command is Meta ([ADR 0033](0033-key-presses-and-notation.md)), so `M-?`
(`lsp-find-references`, [ADR 0138](0138-language-servers.md)) is Command-Shift-`/`. In the
installed oso the key did nothing, and `view-lossage` ([ADR 0150](0150-view-lossage-and-describe-key.md))
showed that it never reached oso.

oso's AppKit view takes keys in `keyDown:` ([ADR 0070](0070-windowing-and-input-without-winit.md)),
and Command-`.` also in `cancelOperation:` ([ADR 0149](0149-command-period-through-cancel-operation.md)).
Command-Shift-`/` reaches neither. A test app with an offscreen window showed where it goes. Its
key events were made in the process from key codes and handed to `NSApplication`'s `sendEvent:`,
never posted to the window server. For a key-down with Command, `sendEvent:` offers the key in turn
to:

1. the key window's `performKeyEquivalent:`, which reaches the view;
2. the other windows;
3. the main menu (oso has none);
4. the system's symbolic hot keys.

The last step matches the event against the system's table of symbolic hot keys. Command-Shift with
the key code 44 is "Show Help menu" (number 98). It is on by default, as Command-Shift-`/`. AppKit counts that key as handled whether or not the app has a Help menu.
`sendEvent:` returns, and the window never sends the key to `keyDown:`.

The match goes by key code and the exact modifiers held: Command-`/`,
Command-Option-Shift-`/`, Control-Command-Shift-`/` and Hyper-`/` all reach `keyDown:`. Of
the 61 first keys with Meta that oso binds, the test found only `M-?` lost this way. `M-.`
came through `cancelOperation:`, and every other key reached `keyDown:`. Hot keys that the
window server handles itself, such as screenshots, never reach the app, and this test
cannot show them.

ADR 0149 turned down `performKeyEquivalent:` for Command-`.`, as oso would have copied
AppKit's test of which characters mean cancel, which depends on the layout. Here the test
goes by key code and modifiers alone, which do not depend on the layout, and nothing comes
after it that could take the key in its place, as `cancelOperation:` does for Command-`.`.

The author chose on 2026-11-17, through the coordinator, to take only this key, and accepted
that a key "Show Help menu" is moved to stays AppKit's.

## Decision

- **The view takes the Help menu key in `performKeyEquivalent:`.** That is the last step
  oso sees before AppKit takes the key. The key is Command and Shift alone, on the key with
  code 44, as the system's default says and AppKit matches it, whatever the layout types
  there. The view routes it as `keyDown:` would, so text being composed is committed first,
  as for any Command key, and answers that it handled it. Every other key equivalent goes on
  to NSView's own method, as before.
- **Only a new key-down for the view's window counts**, as for `cancelOperation:`: the view
  never routes the key-down it routed last again, and takes none while another event is being
  handled. The check is a plain function with unit tests, beside ADR 0149's in the same
  module.
- **It is checked at the keyboard.** The control socket's keys do not pass through AppKit's
  dispatch. The author presses Command-Shift-`/` in the installed app, and `C-h l` shows `M-?`.

## Changes to accepted ADRs

ADR 0070's "a key press comes only from the event `keyDown:` is handling", as ADR 0149 amended it,
becomes: a key press comes only from a key-down AppKit dispatches: the event `keyDown:` is
handling, the one the window sends `cancelOperation:` for in its place, or the Help menu key
offered to `performKeyEquivalent:`. 0070 gains `amended by 0152`.

## Consequences

- `M-?` reaches the keymap, and with it `lsp-find-references`. On a layout where the key with
  code 44 types something other than `/`, such as German's `-`, the key taken is what
  Command-Shift types there (`M-_`), which AppKit took before; `M-?` there comes from another
  key and reaches `keyDown:` as it did.
- Command-Shift-`/` no longer opens the Help menu's search in oso. oso has no Help menu, so
  nothing is lost.
- A person who moves "Show Help menu" to another key in System Settings loses that key to
  AppKit, as with any app, and oso does not follow. With the default turned off, the key
  would reach `keyDown:` anyway, and taking it earlier changes nothing: the view answers that
  it handled the key, so the key is never routed twice.
- Text services give no key here. They do not send key equivalents to the view, and a key-down
  the view routed already is refused.
- The fix has no automated end-to-end test. Only keys that go through the window server reach
  this path.

## Alternatives considered

- **Every Command key in `performKeyEquivalent:`:** copies nothing of AppKit's test and would catch
  any key AppKit takes after the view. But it moves every Meta key off `keyDown:`, and with it
  ADR 0149's path, to fix one key.
- **Unticking "Show Help menu" in System Settings:** no code, but every person would have to do
  it, and nothing in oso would say so.
- **Reading the hot key table:** the call that names a hot key's number is private. The public
  call lists keys without numbers, so oso could not tell "Show Help menu" from the others.
- **Taking keys AppKit dropped after the event:** oso would have to remember each key offered to
  `performKeyEquivalent:` and route it once the dispatch ends. The key would come late, after
  any key behind it in the same pass of the loop.
