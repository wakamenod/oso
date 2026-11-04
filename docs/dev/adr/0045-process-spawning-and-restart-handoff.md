# 0045. Process spawning and the restart handoff

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0010](0010-process-and-network-policy.md) says only `oso-platform::process`
may spawn, and [ADR 0023](0023-control-socket-protocol-and-restart.md) gives
`oso dev` the job of building and restarting. Neither fixes the module's shape,
how the sources are watched, or what the state holds while nothing can be
unsaved. A response line is limited to 1 MiB
([ADR 0032](0032-control-socket-transport.md)), and
[ADR 0044](0044-control-socket-trust-and-secret-buffers.md) forbids the text of
secret-pattern buffers from leaving through the socket.

## Decision

- **`oso_platform::process`**, a new public module of `oso-platform` behind the
  `dev` feature until a release feature needs it:
  - `Executable::absolute` takes an absolute path. `Executable::from_path`
    searches `PATH` without relative entries, the current directory, the
    project and anything under it.
  - `spawn` clears the environment and passes an allowlist: the filtered `PATH`,
    `HOME`, `USER`, `LOGNAME`, `TMPDIR`, `LANG`, `LC_*`, `TERM`, what Windows
    needs to start a process, and the names the caller gives. So `LD_PRELOAD`,
    `DYLD_*` and tokens stay behind.
  - Standard input is closed. Every spawn is logged, with the names of the
    variables but not their values.
  - `#[expect(clippy::disallowed_methods)]` appears only here, and the
    architecture test fails on it anywhere else. `just release-safety` also
    looks for a second marker string, `oso-dev-process-spawn`, in release
    binaries.
- **Watching polls.** `oso dev` looks at the modification time and size of the
  `.rs` and `Cargo.toml` files under `crates/`, and the root manifest and lock
  file, every 100 ms. The workspace is small enough that this costs well under
  a millisecond, and it needs no dependency.
- **The build** is `cargo build --locked -p oso --features dev` in the
  repository root, with errors going straight to the terminal. Cargo's JSON
  output gives the binary's path, and tells a fresh binary from a new one, so an
  edit that changes nothing in the binary restarts nothing. Until the echo area
  exists (M2), the errors stay in the terminal and are not sent to the instance
  as ADR 0023 says.
- **The state holds paths and positions, never text:** buffers (id, name, path),
  the window (buffer, selections with goals, primary, scroll position), pending
  keys, and the OS window's place and size. It is `version` 1.
  - The new instance reads every file from disk and judges it like any file it
    opens. A secret buffer therefore needs no special case, and neither a
    response line nor `oso ctl export-state` can carry its text.
  - **ADR 0023's "unsaved content" case waits for M2**, since nothing can be
    unsaved before then. A path-less buffer with text, which cannot occur yet,
    makes `export-state` fail, which keeps the old instance rather than losing
    the text. So does a path that is not UTF-8.
  - The state stays 256 bytes under the line limit, so the `import-state`
    request that carries it fits too.
  - Positions wait until the file has loaded that far, then move to the nearest
    place that exists, since the file may have changed.
  - Pending keys are fed back to the key resolver and never run a command. Keys
    that are not a prefix in the new build are dropped.
- **New methods:** `export-state`, `import-state`, and `exit`, which is how the
  supervisor stops an instance without a signal. `import-state` refuses an
  instance that has opened anything, so importing twice cannot duplicate
  buffers.
- **The order of a restart:** export, start the new binary, wait for its window,
  import, and only then stop the old one.
  - A new binary that cannot start in 10 s is stopped, and the old one stays.
  - A failed export starts nothing.
  - A refused import leaves the new instance running clean beside the old one,
    as ADR 0023 says. The next good build closes the clean one and hands over
    from the old one.
  - If the user closes the last instance, `oso dev` exits. If an instance
    crashes, the next good build starts a new one, without state.

## Consequences

- On this machine, an edit reaches a running new instance in about 1.3 s, within
  the 2 to 3 s budget of [ADR 0002](0002-dev-hot-reload.md): about 0.45 s to
  build, 0.7 s to start and hand over, and up to 0.2 s to notice the change.
- The new window appears at its default place and moves to the old one's, and
  briefly both are open. Whether it takes the keyboard focus depends on the
  window system.
- `oso dev` keeps running the code it was started with, so a change to the
  supervisor needs a new `just dev`.
- Stopping the supervisor with a signal other than one the whole terminal group
  gets (Ctrl-C) leaves the instance running.
- A large file shows its top for a moment, until it has loaded as far as the
  cursor.
- Each new variable a build needs has to be named in `oso dev`. A variable that
  is missing shows up as an unexpected rebuild or a failed build, not silently.

## Alternatives considered

- **`notify`:** file events instead of polling. It saves at most one poll
  interval out of a restart that takes over a second, and adds platform backends
  and transitive dependencies.
- **Sending text for non-secret buffers now:** matches ADR 0023's wording, but
  nothing can be unsaved yet, so it would be code that never runs.
- **Hiding the new window until the import:** avoids the jump, but would need a
  way to tell an instance to start hidden, and a hidden window does not draw on
  every platform.
- **A signal to stop the old instance:** needs `unsafe` or a dependency to send,
  and skips the clean exit that removes the socket.
- **Inheriting the whole environment for `oso dev`'s own children:** simpler, but
  the point of the module is that no caller can skip the allowlist.
