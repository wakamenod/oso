# Open questions

Topics raised in design discussions that are not decided yet. When one is
decided, record it in an ADR or design doc and remove it from this list. Each
item notes when it must be settled.

## Needs a dedicated discussion

- **UI of each later feature** (before building it). The M2 UI is decided
  ([`ui.md`](ui.md), [ADR 0026](adr/0026-m2-ui-layout.md)); M3 and later
  features (avy, ripgrep results, the menu contents, the git status buffer,
  the terminal) are designed when they come up.

## M1

- Control socket, before M1-7b: once `get-state` returns buffer text and
  screenshots show buffers, decide whether secret-pattern files
  ([ADR 0028](adr/0028-secrets-handling.md)) are masked in both. Also, any
  process running as the same user can run editor commands: already every
  bound command through `send-keys`, which shares the keymaps with the
  window ([ADR 0034](adr/0034-command-registry-and-keymaps.md)), and any
  command once `run-command` exists. ADR 0016 accepts same-user trust, and
  this consequence should be stated there, with commands that spawn
  processes in mind.
- Before M1-6: how invalid UTF-8 bytes and binary files are held in the
  rope, which holds only UTF-8, so that they show as `\xNN` escapes and are
  never saved ([ADR 0022](adr/0022-text-buffer-and-file-loading.md)). The
  cell grid already escapes control characters
  ([ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md)).
- M1-6 rendering of buffer text through the text system
  ([ADR 0036](adr/0036-text-system-and-glyph-atlas.md)):
  - shape only the visible column window of a row, so a 10 MB line does not
    block the UI thread
  - key the shaping cache by buffer line rather than screen row, so
    scrolling does not reshape every row
  - cap new rasterizations per frame and batch atlas uploads
  - draw boxed code points and control escapes from the display model
    instead of shaping the raw characters (T3 in `security.md`)
  - measure startup from process start in a release build, cold (fonts not
    cached), and the p99 frame time with a full screen of mixed text
- Text gamma: text is blended in linear space on the sRGB surface, so light
  text on dark looks heavier than in native apps. Decide with the theme.
- M1-6 rendering: a multi-row cluster iterator that keeps one cursor across
  rows, instead of a seek per row (about 25 µs per 60-row frame today).
- Horizontal scroll extent needs the widest row, and the bidi-control
  modeline warning (`ui.md`) needs a count of bidi controls. The TabMap
  summary tracks neither yet; add them when M1-6 or M2 needs them.
- Large deletions free their chunks on the calling thread. Measure when M2
  editing lands, and drop big subtrees in the background if needed.
- `unicode-width` 0.2 and 0.1 (through naga) are both in the build.
  Re-check when wgpu is upgraded.
- `ttf-parser` is unmaintained (RUSTSEC-2026-0192, ignored in `deny.toml`).
  It comes from winit's Wayland decorations. Re-check when winit is
  upgraded.
- Architecture guardrails ([ADR 0039](adr/0039-architecture-guardrail-mechanics.md)),
  by M1-10b:
  - statics with interior mutability (`static X: Mutex<…>`, atomics) are
    global state, but no check catches them yet. Decide whether the
    architecture test rejects them, and how an exception is granted.
  - when to move the pinned nightly and `cargo-public-api`, which must be
    upgraded together.
  - the public API snapshots are Linux-only. On Windows the dev control
    socket stub differs (`ControlStream` has no field, `CleanupGuard` has
    no `Drop`). Revisit when the Windows transport lands.

## M2

- The panic hook (with emergency save, [ADR 0013](adr/0013-persistence.md)):
  Rust's default hook prints panic messages, which may quote buffer text,
  to stderr even for panics caught at the command boundary. Decide what
  the hook prints, in dev and release builds
  ([ADR 0028](adr/0028-secrets-handling.md),
  [ADR 0034](adr/0034-command-registry-and-keymaps.md)).

- User key bindings from TOML ([ADR 0019](adr/0019-configuration.md)):
  `Keymap::bind` refuses to rebind, so decide between a user layer above
  the global keymap and explicit rebinding and unbinding
  ([ADR 0034](adr/0034-command-registry-and-keymaps.md)).

## M3

- Initial set of tree-sitter grammars, beyond Rust, Markdown and TOML.
- Fuzzing targets and how long they run. The `SumTree`, the rope and the
  display model under random edits are the first candidates.
- URL-opening policy: which schemes, and what confirmation.
- Migemo dictionary source and licensing. Common dictionaries derive from GPL
  SKK dictionaries and cannot be bundled under MIT OR Apache-2.0
  ([ADR 0020](adr/0020-license.md)). Options: the user supplies the
  dictionary, a permissively licensed dictionary, or generating one from
  permissive data.

## M4

- Claude Code hooks: which events map to which states, and how
  `oso integration install claude` merges with existing user settings
  ([ADR 0031](adr/0031-claude-code-integration.md)).
- Whether to add native permission dialogs (PermissionRequest hook), a
  read-only transcript viewer, or `claude -p` one-shot features later.
- OSC 7501 is a draft; track its revisions.
- Terminal: PTY implementation per OS (ConPTY on Windows) and the full list of
  allowed and blocked escape sequences.
- If oso is published with the Claude Code integration, check Anthropic's
  terms for third-party clients that launch the user's own `claude`.
- Verify that `gix` can be forced to skip `core.fsmonitor`, filters,
  textconv and external diff drivers.
- Passphrase/credential prompts via `GIT_ASKPASS`: keep the input out of
  history, the kill ring and macros.
- Exact list of git-porcelain commands to implement.

## Later

- An option for East Asian Ambiguous characters to take 2 cells
  ([ADR 0035](adr/0035-sumtree-rope-and-cell-grid.md)). It needs the
  `cjk` feature of `unicode-width`.

- Dev control socket on Windows: a named pipe restricted to the current
  user ([ADR 0032](adr/0032-control-socket-transport.md)). Needed when
  agents test on Windows.
- Linux text-rendering backend: cosmic-text or FreeType + HarfBuzz. Until
  then Linux (and Windows, until DirectWrite) draws no text
  ([ADR 0036](adr/0036-text-system-and-glyph-atlas.md)).
- Exact command subsets for the directory editor and org.
- PDF engine and isolation (pdfium, a pure-Rust renderer, a separate
  process); image decoders.
- OS-level sandboxing of child processes such as language servers.
- `subsecond` hot-patching proof of concept with a plain winit app.
- Generated command reference and the Japanese edition workflow.
- Encodings other than UTF-8 (Shift_JIS, EUC-JP), when they are needed:
  detection, and conversion at the load and save boundary
  ([ADR 0022](adr/0022-text-buffer-and-file-loading.md)).
