# 0023. Control socket protocol and restart-and-restore

- Status: Accepted
- Date: 2026-11-03

## Context

[ADR 0016](0016-dev-control-socket.md) decided that dev builds expose a
control socket, and [ADR 0002](0002-dev-hot-reload.md) decided that code
changes reach the screen through a rebuild and a restart that restores state
in memory. This ADR fixes the protocol, the client, who drives the restart
and what is restored.

## Decision

- **Wire format: JSON Lines.** Each request and response is one JSON object
  on one line.
  - Request: `{"id": 1, "method": "send-keys", "params": {"keys": "C-x C-f"}}`.
  - Response: `{"id": 1, "result": ...}` or
    `{"id": 1, "error": {"message": "..."}}`.
  - Methods: `send-keys` (Emacs key notation), `run-command` (a command name
    and arguments), `get-state`, `screenshot`, `export-state` and
    `import-state`, plus `version` for the protocol version.
  - `screenshot` writes a PNG into the socket's `0700` directory and returns
    its path.
- **Client: `oso ctl`.** Dev builds of the `oso` binary have a `ctl`
  subcommand, for example `oso ctl send-keys "C-n"`. It finds the socket of
  the running instance, or of the one given with `--pid`, and prints the JSON
  response. It works on all platforms.
- **Restart driver: `oso dev`.** A dev-only supervisor subcommand, started by
  `just dev`, drives restart-and-restore. The editor itself never runs cargo.
  1. Watch the Rust sources and run an incremental `cargo build` on change.
     The supervisor spawns processes only through `oso-platform::process`.
  2. If the build fails, the running instance keeps running and shows the
     errors, which the supervisor sends over the socket.
  3. If it succeeds, call `export-state` on the running instance, start the
     new binary and send it the state with `import-state`.
  4. The old instance exits only after the new one confirms the import. If
     the new instance fails, the old one keeps running.
- **Restored state: everything.** Open buffers, including unsaved content
  and modified flags, selections and marks, scroll positions, window layout
  and frame geometry, kill ring, histories and undo history, as each of them
  comes to exist.
  - The state passes through sockets in memory only, never through files
    ([`security.md`](../security.md#persistence-t4-t5)).
  - The state format is versioned and based on the `get-state` JSON. If the
    version does not match or import fails, the new instance starts clean
    and the old one is kept.
- Config and shader reload stay inside the running instance
  ([ADR 0002](0002-dev-hot-reload.md)); the supervisor handles only Rust
  changes.
- Everything here stays behind the dev cargo feature
  ([ADR 0016](0016-dev-control-socket.md)).

## Consequences

- Agents and the supervisor use the same socket and protocol.
- JSON is easy to read and debug for both humans and agents. `serde` and
  `serde_json` become dependencies, which need approval when added.
- Large unsaved buffers make the handoff slower, since their text is sent as
  JSON. Restart time counts toward startup time, so this is measured, and a
  binary encoding can be added later if needed.
- No state is lost when a new build crashes on startup, because the old
  instance is kept until the import succeeds.

## Alternatives considered

- **A custom line-based text protocol:** no dependency, but structured
  results such as `get-state` would need JSON anyway.
- **A binary format (bincode, MessagePack):** faster, but agents cannot read
  it and it is harder to debug.
- **A separate `oso-ctl` binary:** builds faster, but adds a crate.
- **`nc` or `socat`:** nothing to build, but does not work on Windows and
  cannot find the socket by itself.
- **The editor restarting itself:** one process fewer, but puts code that
  runs cargo into the editor.
- **Restoring only files and positions:** simpler, but loses unsaved edits on
  every restart.
