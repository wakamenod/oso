# 0106. The input source turns Japanese on in a file name's prompt

- Status: Accepted
- Date: 2026-11-11

## Context

[ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md) keeps the input
method off when `C-x C-f`'s prompt opens, as in every prompt
([ADR 0072](0072-floating-windows-the-minibuffer-and-questions.md)), and has `かな` turn it on
for that prompt, so that a Japanese file name can be typed, "if a test at the keyboard shows
that `かな` reaches oso while the input method is off".

The test was made on 2026-11-11, on oso's own macOS layer
([ADR 0070](0070-windowing-and-input-without-winit.md)), with a dev build that logged every
`keyDown:` of `英数` and `かな`. Neither reached oso while the prompt kept the input method off:
the system took the key and chose the Japanese input mode, as its menu bar showed, and oso never
heard of it. So `かな` cannot be the trigger as ADR 0087 has it. The author chose to follow the
input source instead, and, as the menu bar would otherwise show Japanese while the prompt types
ASCII, to have oso choose ASCII when the prompt opens and give the person's source back when it
closes, as the Emacs Mac port's `mac-auto-ascii-mode` does. The author made both choices
through the coordinator and accepted this on 2026-11-11, through the coordinator.

## Decision

- **oso follows the input source the person chooses.** On macOS the window layer observes the
  system's notice that the selected keyboard input source changed
  (`com.apple.Carbon.TISNotifySelectedKeyboardInputSourceChanged`, through
  `NSDistributedNotificationCenter`), however it was chosen: `かな`, the menu bar or a
  shortcut. It also looks again when oso comes to the front, for a choice made in another app.
- **What it reads, and what it tells.** On the notice, and only while oso is in front, the
  layer reads the selected source's ID and whether it can type ASCII, from HIToolbox's Text
  Input Sources, and tells the handler `Event::InputSource`: `Japanese` for an input mode that
  cannot type ASCII and whose ID names Japanese (Kotoeri's hiragana and katakana modes,
  Google's, and other input methods' Japanese modes), and `Other` for anything else, its modes
  for Latin letters and keyboard layouts included. It tells only a change of that kind. Nothing
  else runs then: no file is read, no command runs.
- **The prompt for a file's name takes Japanese.** When the person chooses a Japanese input mode
  while `C-x C-f`'s or `C-x C-w`'s prompt is selected, that prompt allows the input method until
  it closes or the person chooses another kind of source. Every other prompt keeps it off, as
  ADR 0072 has it.
- **ASCII while the prompt is open.** When the prompt for a file's name opens while oso is in
  front, and the selected source cannot type ASCII, oso remembers it and chooses the current
  input method's ASCII-capable source (`TISCopyCurrentASCIICapableKeyboardInputSource`), as
  `英数` does: Kotoeri's or Google's mode for Latin letters, so that `かな` goes back to kana as
  usual. When the prompt closes, oso gives the remembered source back, but only if the selected
  one is still the one oso chose: a source the person chose meanwhile, as with `かな`, stays.
  While oso is behind another app, the person's source is theirs: oso gives it back when it goes
  behind, chooses ASCII again when it comes to the front with the prompt still open, and gives
  it back when the app ends, and when the loop stops after a panic. Coming to the front, it
  chooses ASCII before it reads the source, so that the person's source it gave back while
  behind is not taken as a choice made in the prompt. A crash ends the process with no chance
  to give it back, and the person is left with the ASCII mode, which one `英数` or `かな` press
  changes. The notices of oso's own choices read as `Other` while the prompt
  is open, and as the person's kind once it has closed, when no prompt is marked, so they change
  nothing. Only the prompt for a file's name does this; a passphrase has secure input
  ([ADR 0073](0073-secure-input-in-secret-buffers.md)) and no switching.
- **Never with secrets.** No prompt takes Japanese while any window shows a buffer that may
  hold secrets ([ADR 0073](0073-secure-input-in-secret-buffers.md)), and a passphrase or a
  question never does, as only the prompt for a file's name can.
- **Public API:** `oso_platform::window` gains `Event::InputSource(InputSource)`,
  `pub enum InputSource { Japanese, Other }` and, on macOS, `Window::set_ascii_input(&self,
  wanted: bool)`, applied once the handler returns as the other requests are; the author
  approved all three. The app says only whether it wants ASCII, when that changes, and the
  window layer keeps the rest (`gate::AsciiInput`). winit's adapter sends no `InputSource` and
  does nothing for `set_ascii_input`, so on Linux and Windows nothing changes.
- **Dependencies:** `objc2-foundation` gains its `NSDistributedNotificationCenter` feature,
  which the author approved; no crate is added and `Cargo.lock` does not change. The Text Input
  Sources are bound by hand beside secure input's Carbon calls, each `unsafe` with its
  `// SAFETY:` comment, their two property keys as `static`s of an `extern` block, which
  [ADR 0107](0107-foreign-constants-in-extern-blocks.md) allows.

## Changes to accepted ADRs

- **[ADR 0087](0087-finding-files-writing-under-a-new-name-and-large-files.md), "Japanese file
  names":** "`かな` turns it on for that prompt until it closes ... if a test at the keyboard
  shows that `かな` reaches oso while the input method is off" becomes: choosing a Japanese input
  mode, by `かな` or any other way, turns it on for that prompt until it closes or another kind
  of source is chosen. Choice 10's (a) reads the same way.

## Consequences

- A Japanese file name can be typed in `C-x C-f` and `C-x C-w` on macOS: `かな`, then the name,
  and `英数` to type ASCII again. The menu bar and what the prompt types always agree: the
  prompt opens in ASCII, and the person's source comes back once it closes.
- oso chooses an input source for the person, which macOS shows in the menu bar, and gives it
  back; a source the person chose meanwhile is never undone.
- oso hears of every input source change the person makes while it is in front, from any app's
  menu too, but reads only the source's ID and one flag, and acts only in that one prompt.
- The kind is worked out from the source's ID and its ASCII flag. An input method whose
  Japanese mode does not name Japanese in its ID is `Other`, and its user turns the input method
  on as before ADR 0087, outside the prompt.
- On Linux and Windows the prompt keeps the input method off, until their own window layers
  report the input source (ADR 0070, stage 4).

## Alternatives considered

- **Keeping the person's source when the prompt opens:** the menu bar shows Japanese while the
  prompt types ASCII, which the author did not want.
- **Following a source that is already Japanese when the prompt opens:** Japanese at once,
  which a file name usually is not, and one more call for the app to ask.
- **The keyboard layout (`TISCopyCurrentASCIICapableKeyboardLayoutInputSource`), as ABC:** it
  leaves the input method, so `かな` would first have to bring it back.

- **`かな` as a key, as ADR 0087 had it:** the system takes it before `keyDown:`, so oso cannot.
- **A key oso always sees, such as `C-\`, to turn the input method on in the prompt:** smallest,
  but not the key the person reaches for.
- **The input method on in every prompt:** ADR 0072 turned it off so that the next key in a
  prompt is a key; that holds for the other prompts.
- **Matching IDs alone, without the ASCII flag:** misses a third-party input method's Japanese
  mode, and needs every Latin mode's ID listed.
