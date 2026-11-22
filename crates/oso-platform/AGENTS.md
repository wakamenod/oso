# oso-platform

OS abstractions: the text system backends, the clipboard (ADR 0074), and later
IME and filesystem (ADR 0003, ADR 0004). `clipboard` runs on threads of
its own, as reading it can wait on another process. `privacy` holds what keeps text
private to its user: no core dumps, and private directories (ADR 0062), where oso's
state, configuration and cache (ADR 0142) live, the home directory, and who else can change a file by its mode
(ADR 0108), and
`file_meta` what decides who may reach a file beyond its mode (ADR 0065), and opening and
listing a directory relative to itself, with the names of owners and groups and the local time
zone's offset that a directory listing shows (ADR 0114, ADR 0134). The
control sockets' transport lives in `control`, in every build, for the dev socket,
the agent channel (ADR 0085) and the sessions' socket (ADR 0165, step 2); only the dev
tooling and the agent channel listen. Passing descriptors over a connection (`control/unix/rights.rs`) and a PTY master
from a descriptor serve the restart of terminals, `oso dev`'s (ADR 0120, step 11) and
`oso-restart`'s, whose socket pair `ControlStream::pair` makes and whose new instance takes it
as descriptor 3 (`ControlStream::inherited`, `process::PassedFd`, ADR 0133; its number is
`privacy::inheritance`'s). Process spawning lives in `process`, and pseudo-terminals in
`process::pty`, in every build (ADR 0120); a child started in a process group of its own can be
sent `SIGTERM` or `SIGKILL` with what it started (`Child::signal_group`, ADR 0127), and one
started in a session of its own (`Spawn::own_session`, Unix only) has no controlling terminal
to ask a passphrase on (ADR 0127, step 2). On macOS and Linux `process/unix.rs` starts each child
with `posix_spawn`, which gets only the descriptors its file actions name; a terminal's child on
macOS still forks, through `pty/unix.rs`'s hook (ADR 0137). `notify` asks for the person's attention for agent
sessions (ADR 0121). `random` reads the system's random bytes, for ids such as a hosted
session's (ADR 0121). `host` gives the machine's name, for a terminal's OSC 7 directory
(ADR 0141). `window` holds the window layer's
events, which the tip crate's winit adapter builds too, and on macOS the window system itself, in `window/appkit/`
(ADR 0070).

- Depends on no workspace crate. It is the only crate that may use OS
  bindings (`objc2*`, `core-foundation*`, `libc`, `windows*`, ...).
- OS details stay behind its types and traits: no CoreText, Win32 or libc
  types in its public API.
- Every `unsafe` block needs a `// SAFETY:` comment.
- Processes are spawned only through `oso-platform::process` (ADR 0010,
  ADR 0047, ADR 0120), in every build.
  `process::terminate_parent` sends `SIGTERM` to this process's own parent only, while it
  is still its parent (a pidfd on Linux), for `oso mcp` to stop its moved session
  (ADR 0151, step 2).
  `process::open_link` starts the system's opener on an `https` address of plain
  characters (ADR 0127, step 5a); which addresses may open is the caller's to decide.
  There is no network code.
- Each OS backend sits in its own `cfg`-gated module. Linux and Windows
  must keep building and passing tests in CI (Tier 2).
- Module layers are in `guardrails/layers.txt` (`[oso-platform]`).
- Every Rust file opens with its responsibility and a `Must not depend on:`
  paragraph (ADR 0041). The test checks names in backticks against the layers.
- Anything `pub` is in the API snapshot (`just public-api`).
