//! The editor's commands (ADR 0034).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

mod motion;

#[cfg(test)]
pub use motion::COMMANDS;
pub use motion::registry;
