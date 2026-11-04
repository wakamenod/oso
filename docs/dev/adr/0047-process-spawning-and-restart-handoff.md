# 0047. Process spawning and the restart handoff

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0010](0010-process-and-network-policy.md) says only `oso-platform::process`
may spawn, and [ADR 0023](0023-control-socket-protocol-and-restart.md) gives
`oso dev` the job of building and restarting. Neither fixes the module's shape,
how the sources are watched, or what the state holds while nothing can be
unsaved. This ADR fixes those, and records where the result differs from
ADR 0023. A response line is limited to 1 MiB
([ADR 0032](0032-control-socket-transport.md)), and
[ADR 0044](0044-control-socket-trust-and-secret-buffers.md) forbids the text of
secret-pattern buffers from leaving through the socket.

## Decision

- **`oso_platform::process`**, a new public module of `oso-platform` behind the
  `dev` feature until a release feature needs it:
  - `Executable::absolute` takes an absolute path, from config or from a tool
    that reports one, as `oso dev` does with `current_exe` and cargo's output.
    `Executable::from_path` searches `PATH` without relative or missing entries,
    the current directory, the project and anything under it.
  - `spawn` clears the environment and passes an allowlist: the filtered `PATH`,
    `HOME`, `USER`, `LOGNAME`, `TMPDIR`, `LANG`, `LC_*`, `TERM`, what Windows
    needs to start a process, and the names the caller gives. So `LD_PRELOAD`,
    `DYLD_*` and tokens stay behind. `oso dev` names `CARGO_*`, `RUSTFLAGS`, `CC`
    and the like for cargo, since dropping one that a build script watches would
    rebuild dependencies, but not `LD_LIBRARY_PATH`, which `cargo run` points
    into the project.
  - Standard input is closed. Every spawn is logged, with the names of the
    variables but not their values.
  - `#[expect(clippy::disallowed_methods)]` appears only here, and the
    architecture test fails on it anywhere else. `just release-safety` also
    looks for a second marker string, `oso-dev-process-spawn`, in release
    binaries.
- **Watching polls.** `oso dev` looks at the modification time and size of the
  `.rs`, `.wgsl` and `Cargo.toml` files under `crates/`, and the root manifest
  and lock file, every 100 ms. The workspace is small enough for this to cost
  little, and it needs no dependency. Shaders count because they are compiled in
  until reloading them in a running instance exists
  ([ADR 0002](0002-dev-hot-reload.md)).
- **The build** is `cargo build --locked --offline -p oso --features dev` in the
  repository root, with errors going straight to the terminal.
  - Offline, because a lock file that changes on a pull must not fetch and run
    new crates without a user step. New dependencies need `cargo fetch` first.
  - Cargo's JSON output gives the binary's path, and says whether it rebuilt
    anything, so an edit to an integration test restarts nothing unless no
    instance is running or the last try failed. An edit to a comment does restart.
- **`oso dev` trusts the repository it runs in**, since a build runs that
  repository's code anyway. This includes `rust-toolchain.toml`, which rustup's
  `cargo` follows.
- **The state holds paths and positions, never text:** buffers (id, path), the
  window (buffer, selections with goals, primary, scroll position), pending
  keys, and the OS window's size and, where the window system tells, its place.
  It is `version` 1.
  - The new instance reads every file from disk and judges it like any file it
    opens. A secret buffer therefore needs no special case, and neither a
    response line nor `oso ctl export-state` can carry its text.
  - **ADR 0023's "unsaved content" case waits for M2**, since nothing can be
    unsaved before then. A path-less buffer with text, which cannot occur yet,
    makes `export-state` fail, which keeps the old instance rather than losing
    the text. So does a path that is not UTF-8.
  - The state stays 256 bytes under the line limit, so the `import-state`
    request that carries it fits too.
  - It is read on the client thread, before it reaches the UI. It may open at
    most 1024 different files and hold at most 10,000 selections, and a window
    side is at most 8192 pixels.
  - Positions wait until the file has loaded that far, then move to the nearest
    place that exists, since the file may have changed. If another restart comes
    first, the waiting positions are the ones exported.
  - Pending keys are fed back to the key resolver and never run a command. Keys
    that are not a prefix in the new build are dropped.
- **New methods:** `export-state`, `import-state`, and `exit`, which is how the
  supervisor stops an instance without a signal. `import-state` refuses an
  instance that has opened anything, so importing twice cannot duplicate
  buffers.
- **The order of a restart:** start the new binary, wait for its window, export,
  import, and only then stop the old one.
  - A new binary that cannot start in 10 s is stopped, and the old one stays.
  - A failed export stops the new one, and the old one stays.
  - A refused import leaves the new instance running clean beside the old one,
    as ADR 0023 says. The next good build closes the clean one and hands over
    from the old one.
  - If the user closes the last instance, `oso dev` exits. If an instance
    crashes, the next good build starts a new one, without state.
- **Where this differs from ADR 0023:**
  - The export comes after the new window exists, not before the start, so input
    the old window takes during that time is not lost.
  - A confirmed import means every file has begun to load, not that it has. A
    file that cannot begin refuses the import.
  - The state has its own shape rather than that of `get-state`, since it needs
    a subset and has different consumers. Buffer ids are not kept across a
    restart.
  - Until the echo area exists (M2), build errors stay in the terminal, rather
    than being sent to the old instance as ADR 0002 and ADR 0023 say.

## Consequences

- On this machine, an edit reaches a running new instance in about 1.3 s, within
  the 2 to 3 s budget of [ADR 0002](0002-dev-hot-reload.md): about 0.45 s to
  build, 0.7 s to start and hand over, and up to 0.2 s to notice the change.
- The new window appears at its default place and moves to the old one's, and
  briefly both are open. Whether it takes the keyboard focus depends on the
  window system. Where it applies a size only later, the view can move to keep
  the cursor in sight when the resize arrives.
- Input the old window takes between the export and its exit, a few
  milliseconds, is lost. From M2 this could be a keystroke of text.
- `oso-platform` now depends on `log`, in dev builds only.
- `oso dev` keeps running the code it was started with, so a change to the
  supervisor needs a new `just dev`.
- Stopping the supervisor with a signal other than one the whole terminal group
  gets (Ctrl-C) leaves the instance running.
- A large file shows its top for a moment, until it has loaded as far as the
  cursor.
- Each new variable a build needs has to be named in `oso dev`. A variable that
  is missing shows up as an unexpected rebuild or a failed build, not silently.
- A clean instance left by a refused import is closed by the next good build,
  without asking. It holds nothing until M2 can edit.

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
