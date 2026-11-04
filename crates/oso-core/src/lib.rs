//! Editor logic with no GPU, window or OS dependency: buffers, selections,
//! the display model, the command registry and keymaps (ADR 0004).
//! Everything here is testable with plain `cargo test`.
//!
//! Must not depend on: any workspace crate (`oso-platform`, `oso-render`,
//! `oso`), wgpu, winit or OS bindings.

#![forbid(unsafe_code)]
// Panics in core logic must be real errors or justified invariants (ADR 0038).
// Tests may unwrap (`allow-unwrap-in-tests` in clippy.toml).
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod buffer;
pub mod command;
pub mod decode;
pub mod display_map;
pub mod editor;
pub mod key;
pub mod keymap;
pub mod load;
pub mod motion;
pub mod rope;
mod secret;
pub mod selection;
pub mod sum_tree;
pub mod window;
