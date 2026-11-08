//! OS abstractions (ADR 0003, ADR 0004): the text system backends, the window layer's
//! events (ADR 0070), what keeps text private to its user, what decides who may reach a
//! file beyond its mode and, in dev builds, the control socket transport and process
//! spawning.
//!
//! Must not depend on: any workspace crate (`oso-core`, `oso-render`, `oso`),
//! wgpu or winit.
//!
//! IME, clipboard, filesystem and PTY arrive later. `oso_platform::process` is
//! the only place allowed to spawn child processes (ADR 0010).

#[cfg(feature = "dev")]
pub mod control;
pub mod file_meta;
pub mod privacy;
#[cfg(feature = "dev")]
pub mod process;
pub mod text;
pub mod window;
