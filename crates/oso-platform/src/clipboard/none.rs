//! No system clipboard: Linux and Windows until their window layers bring one
//! (ADR 0070, ADR 0074). Kills stay in oso's kill ring.
//!
//! Must not depend on: anything of the system.

use super::{Backend, Found};

/// A clipboard that holds nothing.
pub(super) struct NoClipboard;

impl Backend for NoClipboard {
    fn change_count(&mut self) -> Option<i64> {
        None
    }

    fn write(&mut self, _: &str, _: bool) -> Option<i64> {
        None
    }

    fn read(&mut self, _: usize) -> Found {
        Found::Nothing
    }
}
