# Open questions

Topics raised in design discussions that are not decided yet. When one is
decided, record it in an ADR or design doc and remove it from this list. Each
item notes when it must be settled.

## Needs a dedicated discussion

- **Claude Code client design** (before M4). Revisit whether a
  `claude -p`-based design is right. Also decide whether a wiki/GTD client
  belongs in oso, and if so when.
- **UI of each feature** (before building it). For example: where the
  minibuffer appears, how completion candidates and previews look, and how
  menus appear.

## M1

- Large-file loading: chunking, progressive display, encoding detection.
- Dev control socket protocol (`send-keys`, `run-command`, `get-state`,
  `screenshot`) and the hot-reload state handoff format.
- Session-restore scope: which state survives a dev restart.
- Dev profile tuning, linker choice, optional cranelift backend, and how to
  measure the 2 s build budget.
- How to measure key-to-photon latency in CI.

## M2

- Default secret-file patterns, used by emergency save.
- Clipboard and crash-dump handling of secrets.
- How bidirectional and invisible characters are rendered.

## M3

- Initial set of tree-sitter grammars, beyond Rust, Markdown and TOML.
- Fuzzing targets and how long they run.
- URL-opening policy: which schemes, and what confirmation.
- Migemo dictionary source and licensing. Common dictionaries derive from GPL
  SKK dictionaries and cannot be bundled under MIT OR Apache-2.0
  ([ADR 0020](adr/0020-license.md)). Options: the user supplies the
  dictionary, a permissively licensed dictionary, or generating one from
  permissive data.

## M4

- Verify that `gix` can be forced to skip `core.fsmonitor`, filters,
  textconv and external diff drivers.
- Passphrase/credential prompts via `GIT_ASKPASS`: keep the input out of
  history, the kill ring and macros.
- Exact list of git-porcelain commands to implement.

## Later

- Linux text-rendering backend: cosmic-text or FreeType + HarfBuzz.
- Terminal: PTY implementation per OS (ConPTY on Windows) and the full list of
  allowed and blocked escape sequences.
- Exact command subsets for the directory editor and org.
- PDF engine and isolation (pdfium, a pure-Rust renderer, a separate
  process); image decoders.
- OS-level sandboxing of child processes such as language servers.
- `subsecond` hot-patching proof of concept with a plain winit app.
- Generated command reference and the Japanese edition workflow.
