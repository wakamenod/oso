# 0070. Windowing and input without winit

- Status: Accepted
- Date: 2026-11-08

## Context

The author decided on 2026-11-08 to stop using winit, on every system. M2-12
([ADR 0069](0069-macos-input-method.md)) ships on winit 0.30 first, and the author starts
real notes with it. This ADR designs what replaces it and how the work is staged, without
blocking that.

What led there is what winit's input method API cannot do, read in winit 0.30.13's macOS
view (`platform_impl/macos/view.rs`):

- **The cursor area is pushed.** `set_ime_cursor_area` stores one rectangle and calls
  `invalidateCharacterCoordinates` every time. `firstRectForCharacterRange` ignores the range
  it is asked about and returns that rectangle. So oso gives the cursor's cell after every
  event that moved it, about 0.1 ms a key at p99 in `just perf` at M2-12, and the candidate
  window cannot follow the segment being converted. A native `NSTextInputClient` answers the
  input method when it asks, for the range it asks about.
- **A composition cannot be committed or discarded.** winit has no call for it, so ADR 0069
  defers turning the IME off until a composition ends, as the OS keeps it otherwise.
- **Text with no marked text is dropped.** `insertText` sends a `Commit` only while text is
  marked, so a full-width space from the input method never arrives, and ADR 0069 adds
  `insert-full-width-space` instead.
- **Option is all or nothing.** `set_option_as_alt` rewrites the event's characters and still
  hands it to the input method. The author found on 2026-11-08 that with `OptionAsAlt::Both`
  an Option key during a Japanese composition makes the input method send an empty preedit and
  no commit: the composed text is lost, and the key never reaches oso. With nothing composing,
  Option keys arrive as Super.
- **Nothing to ask the app before it ends.** winit observes only the notification that the
  app will terminate, so through winit a Dock quit or a logout cannot be refused or waited on
  ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)). Its docs leave
  `applicationShouldTerminate:` to a delegate the app registers itself, which oso has not done.
  winit on Windows does not handle the end of a session (`open-questions.md`).
- **Surrounding text and reconversion** (`attributedSubstringForProposedRange`,
  `characterIndexForPoint`) answer nothing, with no way for the app to answer.

Most editors and terminals the author uses are not built on winit: VS Code (Electron), Zed
(GPUI's own platform layer), Emacs's NS port, Sublime Text and Ghostty (a Swift AppKit app on
macOS, GTK on Linux). The research behind this ADR, with sources, is in the pull request that
proposes it.

oso needs much less of a window system than winit offers: one window, keys, text input, the
wheel, resizing, the scale factor, occlusion, focus, a close request and the app's end. Mouse
selection and the clipboard come later. No cursor icons, monitor lists, fullscreen API,
drag and drop, touch, gamepads, web or mobile targets.

## Decision

The author chose on 2026-11-08, from options put to them: what Option, Command and `C-g` do
during a composition, a commit when the IME turns off during one, how quitting is guarded, the
cursor's line as text for the input method, press-and-hold off, Wayland only, IMM32 first, a
cargo feature, `oso_platform::window`, and starting at once. They are marked "(chosen)" where
they appear. Accepting this ADR, the author also chose that a Dock quit let through the guard
counts as agreed, and the fuller account of who reads the cursor's line.

### A window layer in `oso-platform`

- **A new public module, `oso_platform::window` (chosen),** owns the event loop, the window
  and the input method on every system. Its API has no OS types: plain sizes in physical
  pixels, a scale factor, oso's own key and input-method events, and requests as methods.
  `oso-platform` is already the only crate that may use OS bindings
  ([ADR 0038](0038-architecture-guardrails.md)), so no new crate is needed.
- Each system's backend is a `cfg`-gated submodule, split into files under the 600-line limit:
  `window/appkit/`, `window/win32/` and `window/wayland/`. Every
  `unsafe` block has a `// SAFETY:` comment, as now.
- **The window implements `raw-window-handle`'s `HasWindowHandle` and `HasDisplayHandle`,** so
  the tip crate hands `oso-render` the same `Arc<dyn wgpu::DisplayAndWindowHandle>` as today.
  `oso-render` does not change. `raw-window-handle` 0.6 is already in the tree through wgpu; it
  becomes a direct dependency of `oso-platform`.
- **The tip crate keeps the translation of keys** into `KeyPress` (`keys.rs`), the mapping of
  Command to Meta and Option to Super ([ADR 0033](0033-key-presses-and-notation.md)), and all of
  `app/`. The window layer knows no keymap and no editor: `oso-platform` depends on no
  workspace crate.
- **The event types move to `oso_platform::window` at stage 2, as they are:** the key event
  (`KeyInput`, `SystemKey`, `SystemNamed`), the modifiers held (`Held`), the window's events
  (`Event`, `Wheel`) and the input method's (`ImeEvent`), which stage 1 defines in the tip
  crate. `keys.rs` and `app/` then import them from there, and the winit adapter builds them
  too, until it goes. No second copy of them is kept.

### Events and the loop

- **oso runs its own loop on each system,** through one entry point,
  `window::run(start)`, where `start` makes the handler given the waker other threads reach it
  through. It returns when the loop ends, but on macOS when the app
  terminates, where AppKit ends the process after `applicationWillTerminate:`:
  - macOS: `NSApplication.run` with oso's own application delegate, a run loop observer for
    the moment before the loop waits, and a run loop source that other threads signal, both in
    the common modes, so that they also run during a live resize and while a quit waits
    (`NSTerminateLater` runs the loop in the modal panel mode).
  - Since nothing after `run` may run on macOS, what `main` would clean up after the loop,
    such as the dev socket's files, is cleaned up in `exiting`.
  - Windows: a message loop with `MsgWaitForMultipleObjectsEx` for deadlines and wake-ups.
  - Linux: `poll` on the display connection, an `eventfd` for wake-ups and the deadline.
- **A `Handler` trait,** implemented by `App`, receives the events. It replaces winit's
  `ApplicationHandler` one for one, and calls the entry points `app/events.rs` has had since
  stage 1:
  - window events: a key, the modifiers, text, a preedit, a commit, the end of a composition,
    a resize, a new scale factor, occlusion, focus, the wheel, a frame to draw, a close request;
  - a user event from another thread, through a `Waker` that is `Send` and replaces
    `EventLoopProxy`;
  - `before_waiting`, before the loop sleeps;
  - `quit_requested(reason)`, which the app answers (see "Quitting"), and `exiting`, on every
    end.
- **The input method asks, and the handler answers.** `Handler` has a second method,
  `text_input(query) -> answer`, which the backend calls when the OS asks: the rectangle of a
  range of the marked text, the marked and selected ranges, and the text of the cursor's line
  (below). On macOS these are `firstRectForCharacterRange`, `markedRange`, `selectedRange` and
  `attributedSubstringForProposedRange`. The app answers from the editor, with
  `Editor::cursor_area` and the preedit's cells, so the candidate window opens at the cursor
  and follows the segment being converted. Nothing is pushed on a key. The input method asks
  several times for each key of a composition, so the answers come from what the last frame
  laid out: the cursor's cell and the preedit's cells, worked out again only when the editor
  changed since. Whether the OS must
  still be told that earlier rectangles are stale (`invalidateCharacterCoordinates`) when the
  cursor moves with nothing composing is measured at stage 2. Zed tells it once per frame on
  every selection change. If it is needed, it goes after a presented frame in which the cursor's
  cell moved, never on the key path, and not during a composition.
- **Every query checks the selected buffer when it comes.** While the selected buffer may hold
  secrets the IME is off ([ADR 0069](0069-macos-input-method.md)), and a query then gives no
  rectangle, no range and no text, whatever asks. When the selected buffer or its verdict
  changes, the input context drops what it holds (`discardMarkedText` on macOS; text-input-v3
  disabled on Wayland, which clears its surrounding text).
- **What another process sends is checked before it is used.** Ranges, offsets and lengths
  from the input method or a text service are clamped to the text they refer to. A
  replacement range (reconversion) is applied only inside the excerpt last answered, and only
  if the buffer's revision has not changed since and it still takes ordinary text; otherwise
  it is refused.
- **No call into the window from inside the handler re-enters it.** The handler's requests
  (redraw, allow or forbid the IME, commit or discard a composition, show, hide, ask for
  attention, place and size, a deadline, exit) are collected while it runs and applied after it
  returns. An OS callback that comes while the handler is running, such as an input method
  asking during a commit oso started, is answered from what the backend already holds, and is
  logged at `debug`. What it holds is geometry and ranges, never text of a buffer. A quit
  request that comes then is answered "later". A request whose result the system gives at
  once, such as a size Wayland applies with no configure of its own, comes back to the
  handler as the event it stands for, a resize.
- **A panic in the handler stops the loop.** Every OS callback runs the handler inside
  `catch_unwind`, as no unwind may cross an Objective-C or Win32 frame. The panic hook has
  already written the emergency copies, since it runs before unwinding
  ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)). From then on no handler method
  runs, `exiting` included, as the app may be half changed. The backend ends the loop
  (`stop:` on macOS, so that `NSApplication.run` returns) and resumes the panic in `run`,
  where `main` would have seen winit's.
- **An Objective-C exception** from an AppKit call that can throw is caught with `objc2`'s
  exception support and becomes a Rust panic at that call, so the hook writes the copies as
  for any panic.
- Frames stay on demand ([ADR 0063](0063-modeline-echo-area-and-internal-buffers.md)): a
  redraw request is a flag, drained once before the loop waits, as winit does on macOS. The
  surface keeps presenting with `Fifo`.

### macOS (Tier 1)

- **Bindings:** `objc2` 0.6 with `objc2-foundation`, `objc2-app-kit` and `objc2-quartz-core`
  0.3, with only the features used. `objc2` 0.6, `objc2-foundation` and `objc2-quartz-core`
  0.3 are already in the tree through wgpu's Metal backend and the CoreText text system
  ([ADR 0036](0036-text-system-and-glyph-atlas.md)); `objc2-app-kit` 0.3 is new. winit 0.30
  pulls the older 0.5 and 0.2 family, so its removal takes `objc2` 0.5, `objc2-app-kit` 0.2,
  `objc2-foundation` 0.2, `block2` 0.5, `core-graphics` 0.23 and their dependencies out of the
  tree. ADR 0036 turned down `objc2-app-kit` as too large for one font; here it is the window
  system itself.
- **The application:** `NSApplication` with a regular activation policy, no default menu
  (ADR 0046's 55 ms, and Command is Meta), automatic window tabbing and state restoration off.
  Its delegate:
  - `applicationShouldTerminate:` asks the handler, for a Dock quit, a logout, a restart or a
    shutdown, told apart by the quit Apple event's reason (see "Quitting");
  - `applicationWillTerminate:` calls `exiting`, which writes what is at risk, as ADR 0062
    says, unless the app agreed to the end.
- **The window:** an `NSWindow` whose delegate reports a close request (`windowShouldClose:`
  answers no, and the app decides, as ADR 0059 does), resizes, backing scale changes
  (`windowDidChangeBackingProperties:`), a move to another display
  (`windowDidChangeScreen:`), occlusion and key status.
- **The view:** an `NSView` subclass, made with `objc2`'s `define_class!`, layer-backed and the
  window's first responder, implementing `NSTextInputClient`:
  - `keyDown:` decides first, before the input method sees the key (below);
  - `doCommandBySelector:` hands the key being handled in `keyDown:` to oso as a key press;
  - `setMarkedText:selectedRange:replacementRange:` gives a preedit, with its range turned from
    UTF-16 to UTF-8 bytes and clamped, as winit does;
  - `insertText:replacementRange:` gives a commit while text is marked, and otherwise the text
    the key typed: a key press if it is the key's own character, and text if not, such as a
    full-width space from the input method;
  - **a key press comes only from the event `keyDown:` is handling.** Dictation, the
    Character Viewer, Services and accessibility clients call `insertText:` and
    `doCommandBySelector:` with no key event. Their text goes through the same checks as typed
    text, never as a key, so it can run no command, answer no question and pass none of the
    rules that turn the IME off; a selector from them is ignored;
  - `unmarkText` ends a composition;
  - `firstRectForCharacterRange:actualRange:`, `markedRange`, `selectedRange`,
    `hasMarkedText` and `attributedSubstringForProposedRange:actualRange:` ask the handler.
- **The surface:** the view's backing layer is a `CAMetalLayer` (`makeBackingLayer`). wgpu's
  Metal backend (`raw-window-metal`) then draws into that layer as it is, where it would
  otherwise add a sublayer and keep it in step with the view through key-value observing. The
  view sets the layer's `contentsScale` when the backing scale changes and reports the new size
  and scale. `oso-render` still gets the view through `raw-window-handle` and does not change.
- **Keys, in `keyDown:`, before the input method:**
  - With Control or Command held and nothing marked, the key goes to oso as a key press, never
    through the input method, as Emacs's NS port does.
  - With Option held and nothing marked, the key goes to oso as Super, from
    `charactersIgnoringModifiers`, so Option-a is `s-a` whatever the input source.
  - **With text marked, an Option or Command key commits the composition, then runs as `s-`
    or `M-` (chosen).** The marked text as shown is committed, through the same path as a commit
    from the input method, and the input context is told to drop its own (`unmarkText`, then
    `discardMarkedText`). The key never reaches the input method, so composed text is not lost.
  - **With text marked, `C-g` discards the composition (chosen),** as `ESC` does in the input
    method: the preedit goes, nothing is inserted, and the input context discards its own. Other
    Control keys go to the input method, as Japanese input methods bind several of them.
  - Otherwise the key goes to `interpretKeyEvents:`, as now.
  - Both readings of a key, with and without Option, come from the event
    (`characters` and `charactersIgnoringModifiers`), so the tip crate's translation keeps
    its rules ([ADR 0033](0033-key-presses-and-notation.md)). Zed reads both with
    `UCKeyTranslate`, which is a fallback if a layout gives wrong ones.
- **The IME on and off** is oso's: off means `keyDown:` skips the input method, and the
  view gives no input context, so no text service reads it either. **Turning off during a
  composition commits it first (chosen),** as native text views do on a click, so ADR 0069's
  wait for the end of a composition goes. The commit goes where typing goes at that moment, as
  ADR 0069 says of any commit. One case stays as ADR 0069 has it: when a buffer that may hold
  secrets becomes the selected one, the composition is dropped, never committed into it.
- **Press-and-hold is off in oso (chosen),** so a held letter repeats, as in Emacs. oso
  registers `ApplePressAndHoldEnabled` as false at launch with `registerDefaults:`, which
  lives in memory only: no preference is written, and other apps keep the user's setting.
- **Sudden and automatic termination stay off,** so macOS never ends oso without the quit
  event below.

### Text for the input method

- **The input method may read the cursor's line (chosen),** for reconversion and better
  predictions. On macOS this is `attributedSubstringForProposedRange:`, with `selectedRange`
  and `markedRange` in the same line's coordinates; on Wayland `set_surrounding_text`; on
  Windows IMM32's reconversion request.
- **That text goes to the input method's own process,** outside oso, where oso has no say
  over what happens to it. On macOS the system's other text services read a view through the
  same protocol: Dictation, the Character Viewer, Look Up, Services and accessibility clients,
  and some may send it off the device, as may a third-party input method. Writing Tools are
  turned off for the view where macOS offers a way. With the clipboard, this is how buffer
  text reaches other processes while oso runs in a release build.
- **On Wayland the text is pushed,** with each commit of text-input-v3, so oso sends it only
  while a composition is open, and not as the cursor moves.
- **It is bounded:** at most 4000 bytes in all, as text-input-v3 allows: up to 2000 before
  the cursor and 2000 after, cut on grapheme cluster boundaries, and never past the line's
  ends. It holds no preedit. Its ends are found by walking at most those bytes from the
  cursor, never back to the start of the line, and every range the input method is given or
  asks about counts from the excerpt's start, in UTF-16 on macOS, so no query costs more on
  a long line than on a short one.
- **Never from a buffer that may hold secrets:** only a buffer whose verdict is `Ordinary`
  answers ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md),
  [ADR 0062](0062-emergency-save-and-the-panic-hook.md)); any other gives no text, and the IME
  is off there anyway ([ADR 0069](0069-macos-input-method.md)). Internal buffers give none.
- **It is never logged,** and the type that carries it prints no text through `Debug`
  ([ADR 0028](0028-secrets-handling.md)).
- A commit or preedit with a replacement range, as reconversion sends, replaces that part of
  the line through the dispatcher, as one edit and one undo step.

### Quitting (chosen)

- **A Dock quit is guarded as `C-x C-c` is** ([ADR 0059](0059-safe-save-and-the-exit-guard.md)):
  while a buffer is modified the first one is refused (`NSTerminateCancel`) with the guard's
  message, and the next goes. While a save runs it waits (`NSTerminateLater`) and answers once
  the saves have ended, as an exit does.
- **A Dock quit let through counts as agreed,** as a confirmed `C-x C-c` does
  (`saver::Agreement`, [ADR 0062](0062-emergency-save-and-the-panic-hook.md)): nothing is
  written, as the person was told and quit again. One with nothing modified writes nothing
  anyway.
- **A logout, a restart or a shutdown is never refused.** It waits for running saves
  (`NSTerminateLater`) within a bound below what macOS allows an app, set at stage 2, then
  answers yes. `exiting` then writes emergency copies of what is at risk, as nobody agreed to
  the end ([ADR 0062](0062-emergency-save-and-the-panic-hook.md)).
- The reason comes from the quit Apple event (`keyAEQuitReason`). A quit with no reason is
  treated as a Dock quit. Any local process can send that event with any reason, so the
  reason decides only whether a quit is refused, and unlocks nothing else.

### Windows (Tier 2)

- **Bindings:** `windows-sys` for Win32 (a window class, the message loop, per-monitor DPI
  awareness v2 and `WM_DPICHANGED`, `WM_QUERYENDSESSION` and `WM_ENDSESSION`), and
  IMM32 for the input method, first (chosen): `WM_IME_STARTCOMPOSITION`, `WM_IME_COMPOSITION`,
  `ImmGetCompositionStringW` and `ImmSetCandidateWindow`, as winit and Zed use. The cursor
  rectangle is pushed there, as now, but only when a composition starts or the cell changes
  during one, since the window procedure sees the composition begin. Text Services Framework,
  which pulls the rectangle as macOS does (`ITextStoreACP::GetTextExt`) and which Windows
  Terminal moved to, needs the `windows` crate's COM support and is left for later. The
  backend keeps the input method behind the same events and queries, so TSF can replace IMM32
  without touching the app.
- `WM_QUERYENDSESSION` is never refused, and `WM_ENDSESSION` reaches `exiting`, which writes
  the copies before the window procedure returns. This closes ADR 0062's gap for a Windows
  logout. The Restart Manager's `ENDSESSION_CLOSEAPP` is treated as a logout.
- The control socket has no Windows transport yet
  ([ADR 0032](0032-control-socket-transport.md)), so `just e2e` and `just perf` do not run
  there; CI builds and tests it, as now.

### Linux (Tier 2)

- **Wayland only (chosen):** `wayland-client` and `wayland-protocols` (xdg-shell, the seat,
  `wp_fractional_scale_v1` with `wp_viewporter`, xdg-decoration and `zwp_text_input_v3`), with
  xkbcommon for the keyboard. `smithay-client-toolkit` would save the boilerplate of windows,
  outputs and the seat, but has no text-input module, so that part is oso's either way; Zed
  binds the globals itself. The cursor rectangle is pushed in text-input-v3, as with winit, and
  takes effect at the next surface commit, so it goes with the frame.
- **Decorations:** GNOME's Mutter has no xdg-decoration, so a window there has no title bar
  unless oso draws one. oso draws a plain bar with its own renderer rather than take
  `sctk-adwaita`, whose font parser is the unmaintained `ttf-parser` that `deny.toml` ignores
  today, or `libdecor`, a C library.
- **No X11 backend.** oso does not run in an X11 session. Linux CI runs `just e2e` and
  `just perf` under Xvfb today; they move to a headless Wayland compositor, once a CI job has
  shown that lavapipe presents there (an early task, below).
- The end of a session on Linux usually comes as a signal, which the window layer does not
  change.

### Dev tools, tests and benchmarks

- **The control socket and `oso dev`** do not change. Requests reach the UI thread through the
  `Waker`, as they reach it through `EventLoopProxy` now, and the restart handoff places and
  sizes the window through the handler's requests. Wayland gives no window position, so a
  handoff there carries none, as `Frame::position` already allows.
- **The socket's `ime` method** keeps standing in for the OS and feeds the same handler, so
  the e2e flows of ADR 0069 run unchanged. `get-state`'s `ime.area` becomes the last rectangle
  the input method asked for, withheld as now while a buffer that may hold secrets is shown,
  and anything the socket gains to check the excerpt is withheld the same way
  ([ADR 0044](0044-control-socket-trust-and-secret-buffers.md)).
- **Tests that fail before the code and pass after** cover each rule of this ADR that a test
  can reach: no excerpt from any buffer but an ordinary one; an excerpt and its queries bounded
  on a 1 MB line; text with no key event never becoming a key; a stale replacement range, or
  one outside the excerpt, refused; no handler method after a panic in a callback; a logout
  with a running save answered within its bound.
- **What the OS does is tested by hand,** as now: the input method, Option, press-and-hold, a
  Dock quit and a logout, a display with another scale. Translating an `NSEvent`'s fields into
  oso's key event, and the input-method state, are plain functions tested without a window.
- **`just perf`** compares the native layer against winit on `main` with the same keys. Keys
  that move the cursor should lose the cursor area's cost; nothing else should move. Stage 2
  also measures:
  - what one query of the input method costs, while composing at the end of a single line of
    1 MB and of 100 MB, which must not grow with the line;
  - how many queries a composed key brings, and their time together;
  - frames and key-to-photon latency under held keys and trackpad momentum, on both layers,
    since frames are drawn only when the loop is about to wait.

### Stages

The work starts at once (chosen): stage 1 when M2-12 has merged, and stage 2 straight after,
without waiting for notes typed on winit.

1. **On winit, before anything native:** the tip crate's `app/` works on oso's own event types
   (ADR 0069's `ImeEvent` is the first), and only one file, `app/shell.rs`, knows winit. It
   runs the loop and turns winit's events into `events::Event` and `events::Wheel`, defined
   in `app/events.rs`, its real key presses into `keys::KeyInput` with the modifiers as
   `keys::Held`, and wraps the loop, the window and the `Waker`. `keys.rs` translates
   `KeyInput` into `KeyPress` with no winit. Nothing changes in behavior. No new dependency.
2. **macOS native behind a cargo feature, `native-window` (chosen),** off by default, beside
   winit, until it is at parity: every e2e flow, `just perf` within noise, and the author's
   checks at the keyboard. The tip crate has a thin adapter for each, driving the same `App`.
3. **macOS native by default.** winit stays for Windows and Linux only, behind
   `cfg(not(target_os = "macos"))`.
4. **Linux, then Windows,** each behind the same feature until CI passes with it. Linux comes
   first because CI runs `just e2e` and `just perf` there, and Windows has no control socket
   transport yet ([ADR 0032](0032-control-socket-transport.md)).
- **Early, beside stage 2:** a CI job shows that lavapipe presents under a headless Wayland
  compositor, before stage 4 relies on it.

5. **winit goes:** from `Cargo.toml`, `deny.toml` (its `wrappers` entry and the
   `ttf-parser` advisory that came through its Wayland decorations), the architecture test's
   special case and the docs that name it.

### Dependencies and the feature

- Every crate this ADR names is added only after the author approves it through
  `add-dependency`, each at the stage that needs it: `objc2-app-kit`, the direct `objc2`
  crates and `raw-window-handle` at stage 2; `wayland-client`, `wayland-protocols` and
  xkbcommon, and `windows-sys`, at stage 4.
- The `native-window` feature is an edit to `Cargo.toml`, made with the first of those.
- xkbcommon is not yet among the OS bindings the architecture test knows. Adding it there,
  at stage 4, is an edit to a guard test.

### Changes to accepted ADRs

- **[ADR 0069](0069-macos-input-method.md):** `oso-platform` gains the input method behind the
  window layer, where 0069 kept winit's calls as the abstraction and turned down an IME trait
  there. At stage 2, Option as Super comes from `keyDown:`'s routing instead of
  `OptionAsAlt::Both`, a composition is committed when the IME turns off instead of waiting
  for its end, an Option key during one commits it and runs, and the cursor's cell is asked
  for instead of given after each event. `insert-full-width-space` stays, and is no longer
  the only way to a full-width space.
- **[ADR 0062](0062-emergency-save-and-the-panic-hook.md):** a Dock quit no longer reaches
  `exiting` unasked, so it is no longer `Nothing`: it is refused once while a buffer is
  modified, and one let through is agreed. A logout, a restart and a shutdown stay
  `Nothing`, and still write copies.
- **[ADR 0059](0059-safe-save-and-the-exit-guard.md)** and
  **[ADR 0051](0051-m2-plan-decisions.md):** a Dock quit now goes through the exit guard, and
  macOS no longer ends the app with no chance to ask.
- **[ADR 0046](0046-performance-regression-checks.md)** and
  **[ADR 0049](0049-end-to-end-test-harness.md):** Linux CI runs `just perf` and `just e2e`
  under a headless Wayland compositor instead of Xvfb, from stage 4.
- **[ADR 0036](0036-text-system-and-glyph-atlas.md)** turned down `objc2-app-kit` for the
  monospaced system font. It comes in here for the window system; the font choice stays.

### Guardrails

- `guardrails/layers.txt` gains `window` in `[oso-platform]`, on the top line with `control`,
  `process` and `text`. The crate graph does not change: `oso` already depends on
  `oso-platform`.
- `cargo-deny`'s `wrappers` stop naming winit at stage 5. `objc2-app-kit`, `windows-sys` and
  the Wayland crates are OS bindings, confined to `oso-platform` by the architecture test.
- `oso-platform`'s public API gains the `window` module and the `Handler` trait, in its API
  snapshot.

## Consequences

- Keys no longer pay for the cursor area before their frame, and the candidate window follows
  the segment being converted. What is left of the cost, if the OS must still be told after a
  frame, is measured at stage 2.
- Option is Super whatever the input source. An Option or Command key during a composition
  commits it and then runs, and `C-g` discards it, so composed text is never lost to a key.
- A full-width space and other text the input method types with nothing marked arrive, so
  `insert-full-width-space` becomes optional.
- A Dock quit is guarded as `C-x C-c` is, a logout waits for running saves, and Windows
  logouts reach `exiting`.
- The input method's process, and the system's other text services, can read up to 4000
  bytes of the cursor's line in an ordinary buffer. Reconversion and the input method's
  predictions work there.
- A held letter repeats in oso, and accented letters cannot be picked by holding one.
- oso does not run in an X11 session, and Linux CI moves from Xvfb to a headless Wayland
  compositor.
- `oso-platform` gains the most `unsafe` code in the workspace: every AppKit call that
  `objc2-app-kit` does not mark safe, every Win32 call and the Wayland glue. The
  security review covers it as it does the CoreText backend.
- oso takes on what winit's maintainers did: following new OS releases, input methods and
  compositors, and bugs found by many apps. Fixes in winit, such as the clamped range above,
  have to be found again or read from its source.
- Linux and Windows input methods are checked only in CI, which cannot drive an input method,
  until someone uses them daily.
- During stages 2 to 4 two window layers exist, and the tip crate carries an adapter for each.

## Alternatives considered

- **Staying on winit 0.30 and working around it,** as ADR 0069 does: costs nothing now, and
  keeps the push model, the lost composition with Option, and no say over the app's end.
- **winit 0.31,** in beta since 2025-11 (beta.4 on 2026-10-24), with no final release yet. It
  groups the IME calls into `request_ime_update` and adds surrounding text, which only its
  Wayland backend uses. On macOS the cursor area is still one rectangle the app pushes, and
  `firstRectForCharacterRange` still ignores the range. Turning the IME off now discards the
  marked text, where oso would want to choose to commit it. Its `ApplicationHandler` changes
  shape, so moving to it is work of its own.
- **A fork of winit's macOS backend:** fastest to parity, but keeps winit's event model, its
  `unsafe` and its 0.5 `objc2`, and every upstream fix has to be merged by hand.
- **A new crate, `oso-window`:** keeps `oso-platform` small, but needs a new crate and edges,
  and splits the OS bindings rule in two.
- **A runtime switch between winit and the native layer instead of a cargo feature:** lets one
  binary compare both, but builds both into release binaries until winit goes.
- **Swift or Objective-C for the macOS shell,** as Ghostty does: native tooling, but a second
  language and build system, and the editor would cross an FFI boundary on every event.
- **GPUI or another toolkit's platform layer:** brings a UI framework and its model with it.
- **During a composition, dropping an Option or Command key, or giving it to the input
  method:** the first does nothing for `s-SPC` while composing, the second loses the text.
- **Discarding a composition when the IME turns off,** as winit 0.31 does, or ADR 0069's wait
  for its end: the first loses text, the second leaves the OS holding a composition oso cannot
  see.
- **Refusing nothing at quit,** as now, or refusing a logout too: the first lets a Dock quit
  end with modified buffers, the second makes macOS report that oso cancelled the logout.
- **No text for the input method,** as winit gives: no buffer text leaves oso, but
  reconversion does not work.
- **The system's press-and-hold:** accented letters by holding, but no key repeat for letters.
- **X11 beside Wayland, or X11 first:** keeps Xvfb in CI and X11 sessions, at two to four
  weeks more, with XIM as the weakest input method.
- **TSF from the start on Windows:** the pull model there too, at two to four weeks more and
  the `windows` crate's COM support.
