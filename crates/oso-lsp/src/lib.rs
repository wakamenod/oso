//! oso's language server client (ADR 0138). This step has its parts that spawn nothing:
//! the framing of messages, bounded reads of their JSON, `file:` URIs, positions in the
//! encoding agreed, and every bound in one place. Only the tip crate uses it.
//!
//! Must not depend on: `oso`, `oso-render`, `oso-syntax` or `oso-term`.
//!
//! Everything a server sends is untrusted: each reader keeps to a bound in [`limits`], and
//! nothing a server sends can make oso panic or index out of the text.

#![forbid(unsafe_code)]
// Server output is untrusted: failures are values, not panics (ADR 0038).
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod framing;
pub mod json;
pub mod limits;
pub mod position;
pub mod uri;
