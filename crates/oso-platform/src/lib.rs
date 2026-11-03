//! OS abstractions behind traits (ADR 0003, ADR 0004): text shaping and
//! rasterizing backends, IME, clipboard, filesystem and paths, process
//! spawning and PTY.
//!
//! `oso_platform::process` will be the only place allowed to spawn child
//! processes (ADR 0010).

#[cfg(feature = "dev")]
pub mod control;
pub mod text;
