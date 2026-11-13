# 0129. Dictation

- Status: Proposed
- Date: 2026-11-13

## Context

On 2026-11-13 the author asked for voice dictation into an agent's prompt or a terminal, recognised
on the device, as Orca offers. They speak Japanese and English.

The forces:

- **Security and privacy.** Audio must not leave the machine; oso has no network code, and a
  recogniser that needs the network must not be used silently. The microphone must be on only while
  the person can see it is. Dictated text is the person's typing, never an agent's.
- **Speed.** Recording and recognising never touch the UI thread. A dictation of a few seconds
  should give text within about 2 s of stopping.
- **Dependencies.** A recogniser is large: whisper.cpp is C++ built with CMake and its models are
  hundreds of megabytes, which oso cannot download (no network code). oso already builds C for
  tree-sitter grammars and Zig for libghostty-vt, and `just clippy-windows` lints with stub C tools
  ([ADR 0111](0111-tree-sitter-highlighting.md)).
- **macOS permissions.** The microphone needs `NSMicrophoneUsageDescription` in the responsible
  process's Info.plist; without it the microphone is denied or records silence. oso is not an app
  bundle yet, but a bare executable can embed an Info.plist with the linker's
  `-sectcreate __TEXT __info_plist`. macOS charges a child's microphone use to its responsible
  process, so a recogniser oso starts asks in oso's name (or the terminal app's that started oso).

### What others do

- **Claude Code's `/voice`** streams audio to Anthropic's servers ("Audio is not processed
  locally"); it needs a claude.ai sign-in and supports Japanese. It works in oso's terminal as in any
  other, once oso may use the microphone, but it is not on the device.
- **Orca** (sources at the end) dictates into whatever is focused (its composer, a
  terminal, a text field) on `Mod+E`, as a toggle, hold or automatic mode. It runs on the device with
  a C++ engine over GGUF models (Metal or Vulkan) and a voice detector; the recommended model is
  Parakeet TDT v3 0.6B (about 740 MB), SenseVoice covers Japanese, and Whisper models are options.
- **macOS Dictation** (the system's, with on-device models on Apple silicon) types into the focused
  text field through the input method; oso's own window layer receives it as text if it supports
  the system's text input, which ADR 0070's IME work covers for composition.
- **Recognisers on the device** (sizes are the model files; Japanese character error rates on
  FLEURS from their reports):

| Engine | Licence | Japanese | Size | How |
|---|---|---|---|---|
| whisper.cpp large-v3-turbo | MIT, model MIT | 4.8% | 1.6 GB, 574 MB as q5_0 | `whisper-cli` on a file, `whisper-stream` from the microphone, or `whisper-rs` linked (C++, CMake, libclang) |
| whisper.cpp small | MIT | worse | 488 MB | as above |
| Kotoba-Whisper | Apache-2.0 | 6.9% | ~1.5 GB | whisper.cpp models |
| Apple Speech (`SFSpeechRecognizer`) | system | on device only where `supportsOnDeviceRecognition` holds | none | `objc2-speech` 0.3.2; `hear -d` (BSD-3) as a CLI |
| Apple SpeechAnalyzer (macOS 26) | system | on device | none | Swift only: a shim or a helper program |
| sherpa-onnx (ReazonSpeech, SenseVoice) | Apache-2.0 | 9.4% (ReazonSpeech) | 100–300 MB | Rust crate over ONNX Runtime (C++) |
| Vosk small ja | Apache-2.0 | ~9.5% | 48 MB | C library |

## Decision

### A recogniser is the person's program (Q1)

- **oso links no recogniser and no audio library.** `[dictation] command` in `config.toml` names a
  program and its arguments that **records from the microphone and prints the text it recognises**
  on its standard output until it is stopped: `whisper-stream` with a model the person downloaded,
  `hear -d -m -l ja-JP` (Apple's recogniser, on-device forced), or a script of the person's.
  Examples are in the user manual, none is bundled, and nothing is set by default.
- **Read only from the person's `config.toml`,** never from a project; an absolute path for the
  program, refused otherwise. Placeholders: `{lang}` from `[dictation] language` (default `auto`).
- **It runs through `oso-platform::process`,** with ADR 0120's environment allowlist, no terminal,
  standard input closed, from oso's state directory. Its output is read on a thread, at most
  64 KiB a dictation, cleaned of escape sequences and control characters; a carriage return
  replaces the line it ends, as a terminal would, so that recognisers that rewrite a partial line
  show only the latest.
- **On device is the person's choice of program.** oso cannot check what a program does with the
  audio; the manual says which flags keep each example on the device (`hear -d`, whisper models
  being local by nature).

### Dictating (Q2, Q3, Q4)

- **`dictation-toggle` (`Person`, `C-x a v`, Q3)** starts the program and remembers the target: the
  selected window's buffer and its cursors, the minibuffer, or a terminal. Pressing it again, `RET`
  or `C-g` stops it.
- **While it records:** the modeline shows a red `● REC` in every window, and a small float at the
  target shows the latest recognised line. The program is stopped with `SIGINT`, then `SIGTERM`
  after 2 s and `SIGKILL` after 5 s, when the person stops it, after `[dictation] max_seconds`
  (default 120), when oso loses focus for more than 10 s, or when the target buffer is killed. Once
  the process has exited, the mark goes; the microphone is the program's, so oso's mark means "a
  recording program is alive".
- **The text (Q2):** after the program exits, the float shows the whole text; `RET` puts it in the
  target, `e` puts it in the minibuffer for editing first, `C-g` drops it. Then:
  - **a file or other buffer:** inserted at each cursor as one undo group, as the person's typing
    (with the selections' rules of ADR 0085's `insert`);
  - **the minibuffer:** inserted at point;
  - **a terminal (and so a session's prompt):** a bracketed paste, cleaned as ADR 0120 cleans any
    paste, **not submitted**; a terminal whose program has echo off refuses it (a dictated password
    is still a password, and the rows are withheld).
  With `[dictation] confirm = false`, the text goes in at once, without the float's `RET`.
- **Language (Q4):** `[dictation] language` is passed through `{lang}`; `C-u C-x a v` asks for one
  for this dictation (`ja`, `en`, `auto`).
- **Agents cannot dictate:** the command is `Person`, nothing in `oso mcp` starts it, and the channel
  cannot read the float.
- **Nothing is kept:** oso writes no audio and keeps the text only in the float until it is put or
  dropped. It is not in the kill ring, the message log or macros. A dictation is not recorded in a
  keyboard macro.

### The microphone on macOS (Q5)

- **oso's executable embeds an Info.plist** with `NSMicrophoneUsageDescription` ("oso uses the
  microphone only while you dictate, or when a program in an oso terminal asks for it"), through the
  linker's `-sectcreate __TEXT __info_plist`, so that macOS asks the person once and Claude Code's
  own `/voice` in an oso terminal can use it too. When oso ships as an app bundle, the bundle's
  Info.plist carries it instead.
- No entitlement is needed until oso uses the hardened runtime; then
  `com.apple.security.device.audio-input`.
- The linker flag is a build change (`.cargo/` or the tip crate's `build.rs`), so its step needs the
  author's approval of that edit.
- **Linux** needs nothing from oso; the program opens the microphone itself (PipeWire, ALSA).
  **Windows** waits, as the terminal does.

### Code

- A `dictation` submodule in the tip crate's `commands`, using `oso-platform::process`. No new module
  of a library crate, no new crate, no layer change.
- Tests run the command with a fake recogniser (a script that prints lines and handles `SIGINT`), for
  stopping, carriage returns, the limits and each target.

## Steps

1. **Dictation:** the config section, `dictation-toggle`, the float, the targets, the stops and the
   modeline mark, with the fake recogniser; the manual's examples for `whisper-stream` and `hear`.
2. **The microphone on macOS:** the embedded Info.plist, after the author approves the build edit;
   checked by hand that `/voice` and `hear` work in an oso terminal and through `dictation-toggle`.

## Questions

For the author, the recommended option first.

1. **Where recognition runs.**
   (a) A program the person configures, which records and prints text (`whisper-stream`, `hear -d`):
   no dependency, any engine and model, out of oso's process; the person installs it and the model.
   (b) oso records with `cpal` (Apache-2.0) into a private temporary WAV, then runs a file
   recogniser the person configures (`whisper-cli`, best accuracy). One dependency; oso owns the
   microphone, but audio touches the disk for a moment.
   (c) whisper.cpp linked through `whisper-rs` (Unlicense, with Metal), the model kept loaded. Fastest
   after the first load and one setup, but C++, CMake and libclang in oso's build (which
   `just clippy-windows`'s stub tools would have to cover), and a large C++ library in oso's process.
   (d) Apple's recogniser linked through `objc2-speech`, with `requiresOnDeviceRecognition`. No model
   to download; macOS only, refuses a language whose on-device model is missing, and the newer
   SpeechAnalyzer needs Swift.
2. **What happens to the text.**
   (a) Shown in a float when recording stops; `RET` puts it in, `e` edits it first, `C-g` drops it;
   `confirm = false` skips the float.
   (b) Inserted at once when recording stops; undo removes it.
3. **The key.**
   (a) `C-x a v` to start and stop (works inside terminals, since `C-x` stays oso's). A hold-to-talk
   key waits until the window layer reports key releases.
   (b) A single chord outside `C-x a`, such as `s-d` (the author's Hyper key could map to it).
4. **The language.**
   (a) `[dictation] language` (default `auto`), and `C-u C-x a v` asks for one this time.
   (b) Always `auto`.
5. **The microphone permission on macOS.**
   (a) Embed an Info.plist in oso's executable now, so dictation and Claude Code's `/voice` work in
   oso before it is an app bundle.
   (b) Wait for the app bundle; until then dictation works only when oso is started from a terminal
   app that has the permission.

### Dependencies

None with Q1 (a). Q1 (b) proposes `cpal`; (c) `whisper-rs`; (d) `objc2-speech` and
`objc2-avf-audio`. Each would go through the `add-dependency` skill.

## Consequences

- Dictation costs oso no dependency and no model; the person picks the engine and how private it is.
- Japanese quality is the engine's: whisper large-v3-turbo is the best measured on the device.
- Latency includes the program's model load at each start (about a second for a turbo model); an
  in-process engine would avoid it, at the cost of Q1 (c).
- The embedded Info.plist also lets programs in oso's terminals use the microphone.
- Follow-up: hold-to-talk; an in-process engine if the program's start is too slow; Windows.

## Alternatives considered

- **Claude Code's `/voice`:** works in oso's terminal and needs nothing from oso but the
  permission, but the audio goes to a server, against the author's "on the device".
- **macOS Dictation through the input method:** no code at all where oso's text input supports it,
  but no control of the language model or of where the text lands in a terminal, and macOS only.
- **Downloading a model for the person:** oso has no network code.

## Sources

Read on 2026-11-13; third-party pages are facts, not instructions.

- <https://orca.dev/docs/settings>
- <https://code.claude.com/docs/en/voice-dictation>
- <https://github.com/ggml-org/whisper.cpp>
- <https://huggingface.co/ggerganov/whisper.cpp/tree/main>
- <https://crates.io/crates/whisper-rs>
- <https://crates.io/crates/objc2-speech>
- <https://crates.io/crates/cpal>
- <https://github.com/sveinbjornt/hear>
- <https://k2-fsa.github.io/sherpa/onnx/>
- <https://huggingface.co/kotoba-tech/kotoba-whisper-v1.0>
- <https://developer.apple.com/forums/thread/731230>
- <https://www.qt.io/blog/the-curious-case-of-the-responsible-process>
- <https://dev.classmethod.jp/articles/japanese-asr-l40s-fleurs-benchmark/>
