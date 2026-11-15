//! Keeping a descriptor out of a child it was not made for (ADR 0133): a gate that every spawn
//! holds shared, and that a descriptor the system makes without close-on-exec holds alone
//! until it is marked.
//!
//! Must not depend on: anything in this crate; `process` and `control` take the gate.
//!
//! macOS cannot make a socket pair, nor take descriptors from a message, close-on-exec at once:
//! each is marked a moment later. A `posix_spawn` on another thread in between would give the
//! child the descriptor, a socket pair's end or a terminal's master. A spawn through
//! `process` waits while the gate is held alone, so none of oso's children can start inside
//! that moment. Linux makes both close-on-exec at once; the gate costs it an uncontended lock.

use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Held shared by each spawn, alone while a descriptor is not yet close-on-exec.
static GATE: RwLock<()> = RwLock::new(());

/// Held for as long as a child is being started.
pub(crate) fn spawning() -> RwLockReadGuard<'static, ()> {
    GATE.read().unwrap_or_else(PoisonError::into_inner)
}

/// Held from making a descriptor that is not yet close-on-exec until it is. Never held across a
/// wait for anything but the system call itself.
pub(crate) fn making() -> RwLockWriteGuard<'static, ()> {
    GATE.write().unwrap_or_else(PoisonError::into_inner)
}
