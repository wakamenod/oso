//! Editor logic with no GPU, window or OS dependency: buffers, selections,
//! the display model, the command registry, keymaps and configuration
//! (ADR 0004). Everything here is testable with plain `cargo test`.

#![forbid(unsafe_code)]

pub mod cursor;
pub mod display_map;
pub mod key;
pub mod rope;
pub mod sum_tree;
