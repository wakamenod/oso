//! The editor state that commands operate on.
//!
//! The host owns one [`Editor`] and lends it to each command through
//! [`Ctx`](crate::command::Ctx). Today it holds only the cursor. Buffers,
//! windows and their selection sets (ADR 0007) are added here as they come
//! to exist, so commands keep their signature as the state grows.

use crate::cursor::Cursor;

/// All editor state that commands read and change.
#[derive(Default)]
pub struct Editor {
    /// The cursor. Replaced by the selected window's selection set in M1-6.
    pub cursor: Cursor,
}
