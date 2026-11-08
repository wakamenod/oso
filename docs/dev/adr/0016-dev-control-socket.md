# 0016. Dev control socket

- Status: Accepted, amended by 0085
- Date: 2026-11-03

## Context

Agents cannot see the screen. They need to verify behavior in the real app,
and hot reload needs to hand state from one process to the next
([ADR 0002](0002-dev-hot-reload.md)).

## Decision

- Dev builds expose a local control socket: a Unix domain socket, or a named
  pipe on Windows.
  - `send-keys` sends key input.
  - `run-command` runs a named command.
  - `get-state` returns buffers, cursors and the modeline as JSON.
  - `screenshot` writes a PNG of the current frame.
  - State export and import support hot reload.
- The socket is behind a dev cargo feature that release builds never enable.
  CI verifies this.
- The socket lives in a `0700` directory and checks that the peer runs as the
  same user.

## Consequences

- Agents can run end-to-end tests and check visuals from screenshots.
- One mechanism serves both agent testing and hot reload.

## Alternatives considered

- **Testing only through unit tests:** cannot catch integration and rendering
  bugs.
