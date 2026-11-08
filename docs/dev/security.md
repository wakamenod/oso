# Security

This document describes oso's threat model and the measures that follow from
it. oso has no user scripting and no plugins. That removes a large class of
attacks, but it does not remove all of them.

## Threat model

The main scenario is **opening an untrusted repository** (open-source or
third-party code) and having something run because of its contents. Other
threats:

| # | Threat | Examples |
|---|---|---|
| T1 | Content-triggered execution | modelines, file-local variables, project config pointing to executables, `.git/config` `core.fsmonitor`, `.gitattributes` filters, language servers running `build.rs` or proc macros |
| T2 | Supply chain | malicious or compromised crates; `build.rs` and proc macros run on every build, and dev hot reload builds often |
| T3 | Display spoofing | bidirectional control characters (Trojan Source, CVE-2021-42574), invisible characters, homoglyphs |
| T4 | Secrets leaking to disk | auto-save and backup files, session data, crash dumps, persisted undo, clipboard |
| T5 | Unsafe file writes | following symlinks, partial writes, wrong permissions |
| T6 | Process launching | PATH hijacking (on Windows the current directory comes first), URL handlers |
| T7 | Terminal escape sequences | clipboard reads (OSC 52), title or report sequences that echo input back, spoofed hyperlinks (OSC 8) |
| T8 | Denial of service | huge files, very long lines, regex backtracking |
| T9 | Agentic development | prompt injection through read content, casually added dependencies or `unsafe`, dev-only entry points leaking into release |
| T10 | Untrusted parsers | image and PDF decoding (later milestones) |

Out of scope: remote file editing (there is no TRAMP equivalent) and any
network features.

## Principles

1. **Opening never executes.** Opening a file or project triggers no external
   process and no code from the project.
2. **Enforce mechanically.** Rules that a compiler or lint can check are
   checked by tooling, not by convention.
3. **No network code.** oso contains no networking. Network access happens
   only in child processes the user starts explicitly, such as `git push`.
4. **Minimize and audit dependencies.**

## Measures

### Project trust (T1)

- Projects open in **restricted mode**. A modeline indicator shows the
  mode, and no popup asks about it.
- `M-x trust-project` marks a project as trusted. Trust is stored in user
  config, never inside the repository.
- Trust means "I accept running this project's code on my machine".

| Capability | Restricted | Trusted |
|---|---|---|
| View, edit, save | yes | yes |
| Syntax highlighting (in-process tree-sitter) | yes | yes |
| Git status display (in-process `gix`) | yes | yes |
| Terminal (explicit user action) | yes | yes |
| Language servers | no | yes |
| Formatters, format on save | no | yes |
| Git write operations (commit, push, rebase; hooks run) | no | yes |
| Org babel execution (explicit action only) | no | yes |

- Inside repositories, oso reads only EditorConfig. It never reads settings
  that name commands or executables.
- There are no modelines and no file-local variables that execute anything.
  Org links of type `shell:` or `elisp:` are not supported.

### Git (T1)

- Status, branch and diff gutter are read in-process with `gix`. Commands
  defined in config are never run: `core.fsmonitor`, filter and smudge/clean
  drivers, textconv, external diff. Verifying that `gix` can be forced to skip
  all of these is an open item.
- Write operations call the `git` CLI through the process module, and only in
  trusted projects.
- Passphrase and credential prompts never enter minibuffer history, the kill
  ring or keyboard macros.

### Process spawning (T1, T6)

- Only `oso-platform::process` may create processes. Clippy
  `disallowed_methods` bans `std::process::Command::new` (and its async
  counterparts) everywhere else, and `cargo-deny` bans process-spawning
  helper crates. Clippy `disallowed_types` bans `std::net` sockets, and
  Unix domain sockets outside the dev control socket. The one exception is the
  benchmark harness, which never ships
  ([ADR 0046](adr/0046-performance-regression-checks.md)).
- Executables resolve from absolute paths in user config, or from `PATH` with
  the current directory and the project directory removed.
- The environment is sanitized and every spawn is logged in a visible buffer.

### Dependencies (T2, T8)

- `cargo-deny` (`deny.toml`, `just deny`) enforces:
  - advisories, including yanked crates
  - a minimal permissive license allowlist
    ([ADR 0020](adr/0020-license.md))
  - sources (crates.io only)
  - bans on networking crates (reqwest, hyper and similar)
  - bans on backtracking regex engines (fancy-regex, onig, pcre2, regress)
  - bans on process-spawning helper crates (duct, open and similar)
  - all of the above with every feature enabled
- `cargo-audit` (`just audit`) runs in CI, and `Justfile` builds use
  `--locked`.
- The toolchain is pinned in `rust-toolchain.toml`.
- New dependencies require human approval. Agents must justify them: who
  maintains the crate, how much `unsafe` it has, and how many transitive
  dependencies it adds.
- Only the `regex` crate is used for regular expressions, since it runs in
  linear time.
- `oso-core` uses `#![forbid(unsafe_code)]`. Other crates require
  `// SAFETY:` comments (clippy `undocumented_unsafe_blocks`).
- cargo-vet is not used for now.

### Display (T3)

Bidirectional control characters and invisible characters are always shown
as boxed code points, and a buffer containing bidirectional controls shows a
modeline warning ([ADR 0026](adr/0026-m2-ui-layout.md),
[ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)). Control characters
and invalid bytes are shown as escapes (`^M`, `\xFF`) in an escape color. Labels are
shaped apart from the text around them, so that text cannot change what a label reads.
None of these characters reach the text shaper: their labels are shaped as
ASCII instead ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).
The text system may add characters of its own to what it shapes, such as a ZWNJ
between runs of one direction; they map to no character of the file and draw nothing.
A cluster the renderer shapes only in part, past its budget of characters, shows
in a box and is shaped on its own ([ADR 0068](adr/0068-cut-clusters-short-pieces-the-cursor-box-and-lines-by-direction.md)).
Cells stay in the logical order of the text and no character is shown as its mirrored
form, in every script
([ADR 0066](adr/0066-text-stays-in-logical-order.md)).
A glyph paints only the cells of its cluster, in its row and in narrow bands above
and below the row, and a label's glyphs keep their own cells
([ADR 0052](adr/0052-glyphs-stay-in-their-cells.md)).

### Persistence (T4, T5)

- There is no periodic auto-save, no backup files and no persisted undo
  history.
- On a panic outside a command, or when the OS ends the app on macOS, unsaved buffers are written to a
  private directory (`0700`, files `0600`, refused if it is a symlink, another user's or looser, or if another user can change
  anything on the way to it, through every link and directory it passes, in which case nothing is written, and oso says so at startup). On macOS, access lists count: only `deny` entries pass on the directory, what leads to it may not hold an `allow` that changes what is in it, and each copy holds no list before its text goes in, and at startup oso also says of a `deny` that would stop a copy ([ADR 0065](adr/0065-access-lists-on-private-directories.md)). Files matching secret patterns are excluded
  ([ADR 0028](adr/0028-secrets-handling.md) lists the defaults; users can extend them), and their text
  is not even held where the panic hook can reach it. A path that could not be resolved is saved only if no
  name on it, or on the way to where the walk stopped, matches ([ADR 0051](adr/0051-m2-plan-decisions.md)).
  A confirmed exit writes nothing. A kill, a power cut, a force quit or running out of memory
  writes nothing either. A copy is only ever offered back: nothing in that directory may choose a path to
  write to without the person ([ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md)).
- Recovery treats that directory as untrusted input ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)). A path in a `.meta` is
  shown and compared, and no file operation ever uses it, so a copy reaches only the buffer of a file the person opened, on their answer,
  and the disk only on `C-x C-s`. A directory refused by the checks above is not read. Each file must be regular, this user's, with one name,
  and the file that was looked at once open. Reads are bounded: 64 KiB of a `.meta`, 1000 rows of copies and temporary files together, 100,000 names, and a look at what is under 2 GiB.
  A view of a copy is withheld from the socket if any path it names would count as secret, if its buffer's verdict says so, if the copy says it was withheld or does not say, or if nothing says
  whose it is, and text recovered from such a view is withheld in the buffer it goes in. Nothing is deleted unasked but a copy found byte for byte equal to the file the person opened, and a recovered copy once a save of it has reached the disk. A temporary file left by a save or a
  copy is listed once over an hour old, and always withheld, as it may hold a secret buffer's text.
- The OS input method is off in a buffer that may hold secrets, as in macOS password
  fields, and gets no cursor area there. A preedit is in no buffer, copy, handoff or log
  ([ADR 0069](adr/0069-macos-input-method.md)).
- With oso's own macOS window layer, a buffer that may hold secrets gets secure input while it
  is selected and oso is in front with the keyboard, as a password field does: keystrokes
  stay with oso. The window layer holds one enable at a time and releases it when any of that
  stops, when the loop ends and after a panic in the handler
  ([ADR 0073](adr/0073-secure-input-in-secret-buffers.md)).
- The kill ring syncs with the OS clipboard but is never persisted. oso reads the
  clipboard only on a yank, on its own thread. A kill from a buffer that may hold secrets
  goes there marked concealed for clipboard managers, never shows over the socket, and
  makes the buffer it is yanked into secret, also after a round trip through another app
  ([ADR 0074](adr/0074-the-kill-ring-and-the-clipboard.md)). Core
  dumps are disabled with `RLIMIT_CORE` on Unix, soft and hard. Windows has no such limit for a process to set.
  The panic hook prints the location and the stack and never the message, also for a panic caught at the
  command boundary ([ADR 0028](adr/0028-secrets-handling.md), [ADR 0062](adr/0062-emergency-save-and-the-panic-hook.md)).
- Saving (T5) writes a temporary file beside the real file, follows symbolic links to
  find it, and renames it over that file, so a crash leaves the old file or the whole
  new one, unless the file system cannot sync, which the message then says. The text goes only to where the buffer's file was: if a link or a directory
  on the path has changed since it was read, nothing is written at the new place. The
  temporary file is created with `create_new` and, on Unix, mode `0600`, so it is never a
  link someone planted and nobody else can read it while it fills. It takes the
  old file's mode, less set-user-ID and set-group-ID, and its group, before the rename.
  If the group cannot be kept, that group gets no more than others do. If the owner
  cannot be kept, the save is refused. A file deleted since it was read gets its mode
  back, narrowed if its group could differ. A file changed on disk since it was read and a file the user may not write are
  left as they are.
  Its access list goes with it on macOS, and a POSIX one on Linux: the temporary file holds none while the text goes in, and
  takes the old file's only after it, or what its directory gives if it is new. If a list cannot be carried, the save is
  refused. The quarantine flag goes with it too on macOS, and if it cannot, the save goes on and says so. Other extended attributes are lost, and hard links break
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md), [ADR 0067](adr/0067-access-lists-and-the-quarantine-flag-in-a-save.md)). A secret buffer saves like
  any other: only the emergency save skips them. Messages name the path and never
  quote the text. A kill or a power cut during a save, or leaving past the 10 minutes it
  may hold an exit, can leave the temporary file behind, `0600` or by then the file's own
  mode, with the text in it, also a secret buffer's.
- A file changed on disk is read again as it was first read: opened, followed through its
  links and judged from the file that was opened. A reload never makes a buffer's verdict less
  strict, as undo can bring back what was read under the old one. Neither a load nor a
  reload logs a file's size. The watcher only looks at visited paths and never opens a file
  ([ADR 0083](adr/0083-reloading-files-changed-on-disk.md)).
- Dev state handoff for hot reload goes process-to-process in memory, never
  through files. Dev screenshots are the only dev-only files; see
  [Development](#development-t9).

### Terminal (T7)

- OSC 52 clipboard reads are disabled.
- Sequences that echo data back, such as title reporting, are disabled.
- OSC 8 hyperlinks show the real target before opening.

### Agent integration (T7, T9)

- Claude Code runs as its official TUI in a terminal view
  ([ADR 0031](adr/0031-claude-code-integration.md)).
- Its MCP tools reach oso only through the `oso mcp` stdio bridge and a Unix
  socket with a peer user check; no TCP port is opened. Tools follow the
  trust model, and there is no code-execution tool.
- Hooks are installed only by an explicit command that shows the settings
  change first.

### Denial of service (T8)

- Files load in the background, and the UI stays responsive on huge files and
  long lines. Only regular files are opened, so a pipe or a device cannot
  block or feed the loader forever. Rendering shapes only the visible
  columns of a row, and rasterizes a bounded number of new glyphs per frame
  ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).
- Parsing runs in the background with cancellation and timeouts.

### Untrusted parsers (T10)

- Image and PDF support (M4 and later) will use pure-Rust decoders, or isolate
  parsing in a separate process.
- Fuzz targets (`cargo-fuzz`) cover file loading, decoding, highlighting and
  layout.

### Development (T9)

- The dev control socket and hot reload exist only behind cargo features that
  release builds never enable. `just release-safety` runs in CI and fails if
  a release build enables a `dev` feature, depends on a dev-only crate or
  contains a dev marker string. Each check first proves it can see the dev
  code in a dev build ([ADR 0032](adr/0032-control-socket-transport.md)).
- The control socket ([ADR 0032](adr/0032-control-socket-transport.md)):
  - lives in a private directory (`$XDG_RUNTIME_DIR/oso`, or `oso-<uid>` in
    the temporary directory), created `0700` and refused if it is a symlink,
    owned by another user or has a looser mode, or if other users can
    write to its parent and it is not sticky, and on macOS by the same rule for
    access lists, on the directory and its parent
    ([ADR 0065](adr/0065-access-lists-on-private-directories.md))
  - checks on both ends that the peer runs as the same user (`getpeereid`,
    `SO_PEERCRED`)
  - limits request and response lines to 1 MiB, clients to 4 (each disconnected after
    60 s idle) and `send-keys` to 4096 keys, and never blocks the UI thread
  - is not available on Windows yet
- Processes spawn only through `oso_platform::process`: executables come from
  an absolute path or from `PATH` without relative or missing entries, the
  current directory or the project, children get an allowlisted environment and a closed
  stdin, and every spawn is logged. It is behind the `dev` feature until a
  release feature needs it, and only `oso dev` and the end-to-end tests use it.
  `oso dev` builds offline,
  so a changed lock file fetches nothing by itself
  ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md)).
- The state handoff holds paths and positions, never text, so it cannot carry a
  secret buffer's content, and no position in one either, so a restart starts it
  from the top ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md))
  ([ADR 0047](adr/0047-process-spawning-and-restart-handoff.md)). A buffer with
  unsaved edits holds the restart, and the refusal names it without quoting it. The
  generation that guards `exit` is not reported while a file holds edits
  ([ADR 0055](adr/0055-oso-dev-never-loses-unsaved-text.md)).
- Dev builds trust the same user: any process running as the same user can
  run any command through the socket. Release builds contain none of this.
  This is reviewed when commands that spawn processes arrive (M3 onward)
  ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)). A sandboxed
  client of the same user is trusted too, so giving it the socket gives it what
  the dev instance can read ([ADR 0051](adr/0051-m2-plan-decisions.md)). `send-keys` also
  edits any buffer it can show, secret or not ([ADR 0056](adr/0056-typing.md)). With
  `open-file` and `save-buffer` a client writes any file the dev instance may write, with no
  prompt of its own tool asking, and outside any sandbox of its own. `save-buffer` writes
  only where the file was, so this gives no way to read a secret buffer
  ([ADR 0059](adr/0059-safe-save-and-the-exit-guard.md)).
- Secret-pattern buffers give no text and no pixels over the socket: while
  one is shown, `screenshot` fails, `get-text` fails and `get-state` returns
  only its metadata, without `cursor`, `scroll`, the window's selections or the modeline and
  echo area ([ADR 0063](adr/0063-modeline-echo-area-and-internal-buffers.md)), and later
  match counts ([ADR 0051](adr/0051-m2-plan-decisions.md)), and
  without its size while it is modified
  ([ADR 0058](adr/0058-word-motion-prefix-argument-and-secret-positions.md)), or at all for a
  copy being looked at or compared ([ADR 0064](adr/0064-recovering-an-emergency-copy.md)). What a command
  says while one is shown stays out of the log and `*Messages*`, but for a panic, which names
  only the command, and leaves the echo area once the window moves on, and its modeline shows
  no position. The path as opened, each symlink's target and the
  canonical path all count, so a link to a key or through a secret path is
  caught, and a path that cannot be resolved counts as secret. A file counts as
  secret from the moment it is visited until the loading thread has resolved
  its path ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)).
- While a secret-pattern buffer is shown, `get-state` gives no `modified`, `saving` or size of
  such buffers, and `check-handover`, `export-state` and an `exit` with a generation are refused
  for one reason, whatever the buffers hold. `invalid_bytes` is null for such a buffer. Keys in
  progress, a composition and the prompts that were around one stay out of `get-state` and
  `screenshot` until they end, also once it is gone, and those prompts' answers enter no
  history. A restart does not show such a buffer again from behind an internal one
  ([ADR 0079](adr/0079-state-and-restarts-around-a-secret-buffer.md)).
- `take-timings` records no key if a secret-pattern buffer is shown just before or after
  it or when its frame is presented, and no frames while one is shown, but the first
  frame and the first load's times
  ([ADR 0060](adr/0060-timings-while-a-secret-buffer-is-shown.md)).
- Responses are safe to print: DEL, C1 controls, bidi controls and invisible
  characters from a file go out as `\uXXXX`
  ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)).
- Screenshots are a dev-only exception to keeping screen and buffer
  contents off disk. They are written `0600`, without following symlinks,
  into the private directory, and removed on a clean exit or by the next
  dev instance.
- Human approval is required for:
  - changes to `Cargo.toml`, `Cargo.lock`, `deny.toml`, `.claude/` and the
    files that define the checks (`Justfile`, `clippy.toml`,
    `rust-toolchain.toml`, `.cargo/`, `.github/`, gitleaks config).
    `.claude/settings.json` asks before agents edit them, and the pre-commit
    hook asks again before a commit that touches them. In auto mode these
    prompts do not reach a human, so agents must ask in the conversation and
    PR review is the backstop.
  - new `unsafe`
  - changes to process, file-writing or trust code
- Secret scanning runs through GitHub push protection, a gitleaks scan in the
  Claude Code pre-commit hook, and a full-history gitleaks scan in CI. Both
  scans ignore `gitleaks:allow` comments, and CI rejects gitleaks allowlist
  files.
- CI actions are pinned to commits by the workflow lockfile, which GitHub requires
  of every workflow, the workflow token is read-only, and checkout does not persist
  credentials ([ADR 0021](adr/0021-development-tooling.md),
  [ADR 0080](adr/0080-workflow-dependency-lockfile.md)). Tools a step downloads are
  checked against pinned checksums.

## What must never be committed

The repository is public. Never commit secrets, personal paths or environment
details, employer or client names, or descriptions of unfixed vulnerabilities.
Report and fix vulnerabilities through GitHub private security advisories, and
document them only after the fix.
