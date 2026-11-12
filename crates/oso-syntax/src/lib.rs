//! Syntax highlighting with tree-sitter: which grammar a mode uses, oso's queries, and
//! documents that follow a buffer's edits, parse and highlight in bounded slices (ADR 0012,
//! ADR 0111). The tip crate's worker drives it; nothing here runs on the UI thread.
//!
//! Must not depend on: `oso` or `oso-render`, and it never runs on the UI thread.

#![forbid(unsafe_code)]
// Parsing runs on untrusted files: failures are values, not panics (ADR 0038).
// Tests may unwrap (`allow-unwrap-in-tests` in clippy.toml).
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod document;
mod edit;
mod language;
mod paint;
mod pending;
mod queries;
mod worker;

pub use document::{Document, Failure, Highlighted, MAX_BYTES, Parsed};
pub use language::Language;
pub use queries::Queries;
pub use worker::{Checkout, Doomed, Job, Outcome, Refused, Report, Scheduler};
