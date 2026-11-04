//! OS abstractions (ADR 0003, ADR 0004): the text system backends and, in dev
//! builds, the control socket transport.
//!
//! Must not depend on: any workspace crate (`oso-core`, `oso-render`, `oso`),
//! wgpu or winit.
//!
//! IME, clipboard, filesystem and PTY arrive later, and
//! `oso_platform::process` will be the only place allowed to spawn child
//! processes (ADR 0010).

#[cfg(feature = "dev")]
pub mod control;
pub mod text;
