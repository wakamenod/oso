# 0031. Control socket transport, limits and release check

- Status: Accepted
- Date: 2026-11-03

## Context

[ADR 0016](0016-dev-control-socket.md) decided that dev builds expose a
control socket in a `0700` directory, with a check that the peer runs as the
same user, and that CI verifies release builds never contain it.
[ADR 0023](0023-control-socket-protocol-and-restart.md) fixed the protocol
and the `oso ctl` client. Neither says where the directory is, how the peer
is checked, what limits apply, how screenshots are written, or how the
release check works. A named pipe on Windows needs `windows-sys` and
`unsafe` security-descriptor code.

## Decision

- **Directory:** `$XDG_RUNTIME_DIR/oso` when `$XDG_RUNTIME_DIR` is an
  absolute path, otherwise `oso-<uid>` in the temporary directory (per user
  on macOS; the user ID keeps users apart in a shared `/tmp`). The temporary
  directory must be an absolute path.
  - Its parent must not let other users replace it: it is refused if group
    or others can write to it, unless it is sticky like `/tmp`.
  - It is created with mode `0700`. An existing one is checked with `lstat`
    and refused, never repaired, if it is a symlink, is not a directory, is
    owned by another user or grants any permission to group or others.
  - The server and `oso ctl` both check it.
- **Socket:** `<pid>.sock`, mode `0600`. Binding first removes files left
  by an earlier process with the same ID. A background thread then removes
  sockets that refuse connections, and files of instances with no live
  socket; probing a socket can block, so this never delays startup. A clean
  exit removes the instance's own socket and files.
- **Peer check:** both ends compare the peer's user ID with their effective
  user ID: `getpeereid` on macOS and the BSDs, `SO_PEERCRED` on Linux,
  through `libc`. Other Unix systems refuse every peer. A peer running as
  root is refused too, unless oso itself runs as root.
- **Limits:** request lines of at most 1 MiB, string ids of at most 256
  bytes, at most 4 clients at once, at most 4096 keys per `send-keys`, and
  10 s for the UI to reply. Errors quote at most a short prefix of the bad
  input, so responses stay small; `oso ctl` reads at most 1 MiB.
- **Threads:** one thread accepts and one thread per client reads, parses
  JSON and key notation, and waits for the reply. Requests reach the UI
  thread as winit user events, and replies return over a channel. The UI
  thread never touches the socket.
- **Screenshots:** the UI thread only submits an offscreen render and a copy
  to a buffer. Waiting for the GPU, PNG encoding and writing run on a
  background thread, which polls the device without blocking so that it
  never holds wgpu's device locks while the UI thread submits or presents.
  The PNG is written as `<pid>-screenshot-<n>.png`, mode
  `0600`, with `create_new`, so a planted symlink is never followed. The PNG
  container is written by oso; only deflate comes from `miniz_oxide`.
- **Windows is deferred.** The `dev` feature compiles on Windows, and every
  control operation fails with "not supported on this system yet". A named
  pipe restricted to the current user is added when Windows needs agent
  testing.
- **Release check:** `just release-safety`, run by CI on Linux, fails if
  - the release dependency tree of the workspace, for every target, enables
    a `dev` feature of an oso crate, or contains a dev-only crate
    (`serde_json`, `miniz_oxide`). The workspace is checked, as `just
    release` builds it, because features are unified across its crates;
  - the release binary, at the path cargo reports, contains the marker
    string `oso-dev-control-socket`, which every dev binary contains.

  Each check first runs against a dev build and fails if it cannot see the
  dev code there, so it cannot pass vacuously.
- `just check`, `lint` and `test` run both without features and with
  `--all-features`, so dev code is linted and tested, and so is the code
  that ships.
- **Local sockets** (`std::os::unix::net`) are banned by clippy
  `disallowed_types` everywhere except `oso-platform::control`'s Unix
  transport.

## Consequences

- Another user cannot reach the socket: the directory keeps them out, and
  the peer check is a second wall.
- Screenshots put screen contents on disk. This is a dev-only exception to
  "no buffer contents on disk" ([`security.md`](../security.md)): the files
  are private, live in the private directory and are removed on a clean
  exit or by the next instance.
- `libc` is a direct dependency of `oso-platform` with the `dev` feature,
  with three `unsafe` calls (`geteuid`, `getpeereid`, `getsockopt`).
- Agents on Windows cannot drive the app yet.
- The release check costs one release build in CI.

## Alternatives considered

- **Peer check through `rustix` or `nix`:** `rustix` has no `getpeereid`,
  and `nix` is a much larger dependency than the three calls need.
- **No peer check, relying on the directory:** ADR 0016 asks for both.
- **Repairing a directory with a loose mode:** it may have been created by
  someone else to watch the socket; refusing is safer.
- **The `png` crate:** eight more crates, with two versions of
  `miniz_oxide`. The container is about 60 lines.
- **Uncompressed PNGs:** no dependency, but several MB per screenshot.
- **A Windows named pipe now:** `windows-sys` and security-descriptor
  `unsafe` would double the size of this change for a Tier 2 platform.
- **Checking only `cargo tree`:** a dev-only crate that is non-optional by
  mistake would pass; the binary marker checks what actually ships.
