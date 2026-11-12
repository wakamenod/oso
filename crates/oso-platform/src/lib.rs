//! OS abstractions (ADR 0003, ADR 0004): the text system backends, the window layer's
//! events (ADR 0070), what keeps text private to its user, what decides who may reach a
//! file beyond its mode, the control sockets' transport (ADR 0085), and process spawning
//! and pseudo-terminals (ADR 0120).
//!
//! Must not depend on: any workspace crate (`oso-core`, `oso-render`, `oso`),
//! wgpu or winit.
//!
//! `clipboard` reads and writes the OS clipboard off the UI thread (ADR 0074). IME
//! and filesystem arrive later. `oso_platform::process` is
//! the only place allowed to spawn child processes (ADR 0010).

pub mod clipboard;
pub mod control;
pub mod file_meta;
pub mod privacy;
pub mod process;
pub mod text;
pub mod window;
