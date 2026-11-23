//! A native session's note in its modeline (ADR 0165, part 3a): its run's state and its status
//! line, parts joined by ` · `, which a narrow modeline drops whole from the right before the
//! buffer's name would go under its least, the first part never.
//!
//! Must not depend on: the host's window, threads or I/O; plain text the host notes.
//!
//! - The host joins the parts by [`SEPARATOR`], and lets no part hold a dot like it: a part
//!   that held one could pass for two, and keep a mode that is no more in view.

use std::ops::Range;

use super::{MIN_NAME_CELLS, text_cells};

/// The cells a native session's note takes at most.
pub(super) const NATIVE_NOTE_CELLS: usize = 96;
/// What a native session's note joins its parts with.
const SEPARATOR: &str = " · ";

/// The cells the modeline's parts other than the name take, `others`, once the parts of a native
/// session's note at `note` in `state` that the name's least leaves no room for in `columns` are
/// dropped from `state`.
pub(super) fn fit(
    state: &mut String,
    note: Option<Range<usize>>,
    others: usize,
    columns: usize,
) -> usize {
    let over = (others + MIN_NAME_CELLS).saturating_sub(columns);
    let Some(note) = note.filter(|_| over > 0) else {
        return others;
    };
    let before = text_cells(state);
    drop_parts(state, note, over);
    // Counted again whole, as a cluster may straddle a part's start.
    others - (before - text_cells(state)).min(others)
}

/// Drops the parts of the note at `note` in `state` from the right, each whole with the
/// separator before it, until `state` is `over` cells shorter, or one part is left.
fn drop_parts(state: &mut String, note: Range<usize>, over: usize) {
    let mut end = note.end;
    let mut dropped = 0;
    while dropped < over {
        let Some(at) = state[note.start..end].rfind(SEPARATOR) else {
            break;
        };
        let at = note.start + at;
        dropped += text_cells(&state[at..end]);
        end = at;
    }
    state.replace_range(end..note.end, "");
}
