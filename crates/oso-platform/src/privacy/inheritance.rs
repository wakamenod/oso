//! The descriptor number a child started by `oso-restart` finds its end of the pair at (ADR
//! 0133): what `process` passes and `control` takes.
//!
//! Must not depend on: anything in this crate.
//!
//! Which descriptors a child gets is `process`'s: on Unix only its standard streams and this
//! one, however the rest were made (ADR 0137).

/// The descriptor a child started by `oso-restart` holds its end of the pair at.
#[cfg(unix)]
pub(crate) const INHERITED_FD: std::os::fd::RawFd = 3;
