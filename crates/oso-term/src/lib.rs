//! oso's terminal model (ADR 0120): libghostty-vt behind an engine trait, oso's
//! escape-sequence policy, and a thread per terminal that owns its PTY, its child and its
//! engine, and hands the UI one snapshot at a time. Only the tip crate uses it; it never runs
//! on the UI thread but for taking a snapshot. Terminals exist on macOS and Linux; on Windows
//! `Terminal::start` returns `Unsupported` (ADR 0120, Q3).
//!
//! Must not depend on: `oso`, `oso-render` or `oso-syntax`.
//!
//! It parses untrusted output in memory-unsafe code, so all its `unsafe` sits in
//! `engine::ghostty`'s `ffi` and `raw`, each block with its `// SAFETY:` comment, and the
//! library is fuzzed through this crate at each version (`just fuzz-term`).

#![deny(
    unsafe_code,
    unsafe_op_in_unsafe_fn,
    clippy::multiple_unsafe_ops_per_block
)]
// Terminal output is untrusted: failures are values, not panics (ADR 0038).
#![deny(clippy::unwrap_used, clippy::expect_used)]
// On Windows, which has no terminals yet, only the public types and `Terminal::start`'s
// `Unsupported` are built; the engine and the policy are there but unused (ADR 0120, Q3).
#![cfg_attr(
    not(unix),
    allow(dead_code, unused_imports, reason = "terminals exist on Unix only")
)]

mod engine;
mod event;
pub mod limits;
mod policy;
mod snapshot;
mod terminal;

pub use event::{End, Event, Progress, Prompt};
pub use snapshot::{
    Attrs, Cell, Color, Cursor, CursorShape, Modes, Mouse, Palette, Rgb, Row, Snapshot, Underline,
};
pub use terminal::{QueueFull, Start, Terminal};
