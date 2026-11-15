//! Carrying a libghostty-vt terminal over a restart (ADR 0120, step 11; ADR 0133): the bytes
//! that restore it in the next instance, and the engine restored from them. Safe code over
//! `raw`.
//!
//! Must not depend on: the policy or the thread.

use oso_platform::process::pty::Size;

use super::Ghostty;
use super::raw::Vt;
pub(crate) use super::raw::snapshot::{Encoding, NotEncoded};

/// Whether a terminal exported for the same library is written as the library's snapshot. On:
/// oso's patches to the library let a restored terminal reflow as a live one does (ADR 0120,
/// step 11 as built; ADR 0133), so a terminal carried to an instance of the same library
/// travels as its snapshot, which keeps a screen not in use and a sequence its program has not
/// finished; a terminal carried to a different library still travels as escape sequences.
pub(crate) const SNAPSHOTS: bool = true;

/// The lock file the library was built from: its version, oso's patches and their hashes.
const LOCK: &[u8] = include_bytes!("../../../ghostty-vt.lock");

/// What names the library and the snapshot format it writes: two instances built from the
/// same lock restore each other's snapshots. The version and a hash of the lock, whose lines
/// name every patch by its sha256.
pub(crate) fn library() -> String {
    // FNV-1a: a name, not a check of integrity, which the lock's own hashes are.
    let hash = LOCK.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!(
        "libghostty-vt {} {hash:016x}",
        env!("OSO_GHOSTTY_VT_VERSION")
    )
}

impl Ghostty {
    /// This terminal written as `encoding`, its bytes taken from `budget`.
    pub(crate) fn encode(
        &mut self,
        encoding: Encoding,
        budget: &std::sync::atomic::AtomicUsize,
    ) -> Result<Vec<u8>, NotEncoded> {
        self.vt.encode(encoding, budget)
    }

    /// A terminal of `size` restored from `bytes`, as `encode` wrote them, keeping
    /// `scrollback` lines; `None` if the library refuses them. Its first snapshot sends every
    /// row.
    pub(crate) fn restore(
        encoding: Encoding,
        bytes: &[u8],
        size: Size,
        scrollback: usize,
        xtversion: &'static str,
    ) -> Option<Ghostty> {
        let vt = Vt::restore(
            encoding,
            bytes,
            (size.columns, size.rows),
            scrollback,
            xtversion,
        )?;
        Some(Ghostty::around(vt, size))
    }
}
