//! Library entry point.
//!
//! Re-exports the internal modules so that integration tests under `tests/`
//! can reference them via `oso::editor`, `oso::renderer`, etc.
//! The binary (`main.rs`) continues to use `mod` declarations directly.

pub mod editor;
pub mod renderer;
