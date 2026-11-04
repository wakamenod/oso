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
  Unix domain sockets outside the dev control socket.
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
modeline warning ([ADR 0026](adr/0026-m2-ui-layout.md)). Control characters
and invalid bytes are shown as escapes (`^M`, `\xFF`) in an escape color.
None of these characters reach the text shaper: their labels are shaped as
ASCII instead ([ADR 0040](adr/0040-buffers-loading-motion-and-scrolling.md)).

### Persistence (T4, T5)

- There is no periodic auto-save, no backup files and no persisted undo
  history.
- On panic, unsaved buffers are written to a private directory (`0700`,
  files `0600`). Files matching secret patterns are excluded
  ([ADR 0028](adr/0028-secrets-handling.md) lists the defaults; users can
  extend them).
- The kill ring syncs with the OS clipboard but is never persisted. Core
  dumps are disabled, and panic messages never include buffer contents
  ([ADR 0028](adr/0028-secrets-handling.md)).
- Saving writes a temporary file in the same directory, fsyncs it, renames it
  over the target and preserves permissions.
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
    write to its parent and it is not sticky
  - checks on both ends that the peer runs as the same user (`getpeereid`,
    `SO_PEERCRED`)
  - limits request and response lines to 1 MiB, clients to 4 (each disconnected after
    60 s idle) and `send-keys` to 4096 keys, and never blocks the UI thread
  - is not available on Windows yet
- Processes spawn only through `oso_platform::process`: executables come from
  an absolute path or from `PATH` without relative entries, the current
  directory or the project, children get an allowlisted environment and a closed
  stdin, and every spawn is logged. It is behind the `dev` feature until a
  release feature needs it, and only `oso dev` uses it
  ([ADR 0045](adr/0045-process-spawning-and-restart-handoff.md)).
- The state handoff holds paths and positions, never text, so it cannot carry a
  secret buffer's content
  ([ADR 0045](adr/0045-process-spawning-and-restart-handoff.md)).
- Dev builds trust the same user: any process running as the same user can
  run any command through the socket. Release builds contain none of this.
  This is reviewed when commands that spawn processes arrive (M3 onward)
  ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)).
- Secret-pattern buffers give no text and no pixels over the socket: while
  one is shown, `screenshot` fails, `get-text` fails and `get-state` returns
  only its metadata. The path as opened, each symlink's target and the
  canonical path all count, so a link to a key or through a secret path is
  caught, and a path that cannot be resolved counts as secret. A file counts as
  secret from the moment it is visited until the loading thread has resolved
  its path ([ADR 0044](adr/0044-control-socket-trust-and-secret-buffers.md)).
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
- CI actions are pinned by commit SHA, the workflow token is read-only, and
  checkout does not persist credentials
  ([ADR 0021](adr/0021-development-tooling.md)).

## What must never be committed

The repository is public. Never commit secrets, personal paths or environment
details, employer or client names, or descriptions of unfixed vulnerabilities.
Report and fix vulnerabilities through GitHub private security advisories, and
document them only after the fix.
