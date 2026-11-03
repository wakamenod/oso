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
  helper crates. Clippy `disallowed_types` bans `std::net` sockets.
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
modeline warning ([ADR 0026](adr/0026-m2-ui-layout.md)).

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
  through files.

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
  long lines.
- Parsing runs in the background with cancellation and timeouts.

### Untrusted parsers (T10)

- Image and PDF support (M4 and later) will use pure-Rust decoders, or isolate
  parsing in a separate process.
- Fuzz targets (`cargo-fuzz`) cover file loading, decoding, highlighting and
  layout.

### Development (T9)

- The dev control socket and hot reload exist only behind cargo features that
  release builds never enable. CI will verify this once the features exist
  (M1-7a).
- The control socket lives in a `0700` directory and checks the peer's user
  ID.
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
