# Roadmap

## Performance budgets

These budgets are measured from M1 onward, and regressions fail CI.

| Metric | Budget |
|---|---|
| Key press to photon | p99 ≤ 8 ms (one frame at 120 Hz) |
| Startup to first frame | ≤ 100 ms |
| Open a 100 MB file | first screen ≤ 50 ms, rest loads in the background |
| 1 GB file, or a 10 MB single line | no freeze while scrolling or typing |
| Dev incremental build of the tip crate | ≤ 2 s |

## Milestones

### M0 — done

A cursor rectangle drawn with wgpu that moves with the arrow keys.

### M1 — Viewer

- Quick fixes to the current code:
  - `ControlFlow::Wait` instead of `Poll`
  - an explicit present mode and frame latency 1
  - one backend per OS
  - a low-power adapter
  - typed surface-error handling
- Workspace split into four crates ([ADR 0004](adr/0004-crate-layout.md)).
- Command registry and keymaps with prefix keys.
- Display model skeleton ([`display-model.md`](display-model.md)).
- OS-native text rendering on macOS.
- Asynchronous, progressive file loading.
- Scrolling and cursor motion: `C-f/b/n/p`, `C-a/e`, `M-<`/`M->`, `C-v`/`M-v`.
- Dev control socket and the restart-and-restore loop.
- Performance measurement in CI.

### M2 — Editor (start using oso for text and notes)

- Insertion and deletion; Japanese input through the macOS IME, switched off
  automatically in the minibuffer and isearch.
- Undo/redo, safe save and emergency save on panic.
- Kill ring, mark and region, multiple cursors.
- isearch (`C-s`/`C-r`).
- Minibuffer with fuzzy matching and preview: `M-x`, `C-x C-f`, `C-x b`, `C-g`.
- The author's key customizations:
  - `C-z` undo
  - `C-w` kills the region, or the previous word when there is no region
  - `M-,`/`M-/` switch buffers
  - `C-a`/`C-e` toggle between code and line start/end
  - Command = Meta, Option = Super

### M3 — Coding

- Tree-sitter highlighting, starting with Rust, Markdown and TOML.
- Window splits, indentation, project trust model.
- Project file finding and asynchronous ripgrep search.
- Migemo-style romaji search for Japanese text, and avy-style jumps.
- The menu component, porting the author's hydra menus.
- Visible bidirectional and invisible characters.

### M4 — Self-hosting (develop oso in oso)

- Language servers: diagnostics, go to definition, completion.
- A git-porcelain subset: status, staging hunks, commit, push/pull, log,
  conflict resolution.
- Compile buffer (running `just ci`), format on save.
- A minimal Claude Code client and an MCP server for reviewing agent changes.
  The client design will be revisited; see
  [`open-questions.md`](open-questions.md).

### After M4

- Directory editor (dired-like), org subset, terminal (`alacritty_terminal`),
  also usable as the backend for compile.
- Tabs and multiple frames.
- Inline images and a PDF viewer.
- Each of these includes only the commands the author actually uses.

The UI of each feature (for example, where the minibuffer appears) is
discussed and decided before that feature is built.
