# 0028. Handling secrets in memory and on disk

- Status: Accepted
- Date: 2026-11-03

## Context

Buffers may hold secrets such as `.env` files, keys and credentials.
[ADR 0013](0013-persistence.md) limits disk writes to an emergency save on
panic and excludes "secret-looking files" without defining them. Secrets can
also leak through the clipboard, core dumps and panic messages.

## Decision

- **Default secret patterns.** These are never written by the emergency save:
  - `.env`, `.env.*`
  - `*.pem`, `*.key`, `*.p12`, `*.pfx`
  - `id_*` (SSH keys)
  - `.netrc`
  - `credentials*`
  - anything under `~/.ssh/`, `~/.gnupg/` or `~/.aws/`

  Users can add and remove patterns in TOML config.
- **Clipboard.**
  - Kill and yank sync with the OS clipboard, as in Emacs.
  - The kill ring lives only in memory and is never persisted.
- **Crash data.**
  - Core dumps are disabled for the oso process (for example
    `RLIMIT_CORE = 0` on Unix).
  - Panic and error messages never include buffer contents.

## Consequences

- Copying a secret still puts it on the OS clipboard, where clipboard
  managers may keep it. This is accepted for convenience.
- Crash reports carry stack traces but no memory contents, so some crashes
  will be harder to debug.

## Alternatives considered

- **Exchanging with the OS clipboard only through explicit commands:** safer,
  but inconvenient for daily use.
- **A minimal pattern list (`.env`, `*.pem`, `id_*`):** misses common
  credential files.
- **Emergency-saving only buffers with no file:** safest, but loses work in
  ordinary files.
