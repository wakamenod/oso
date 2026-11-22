//! How each buffer a restart carries is handed over (ADR 0023, ADR 0047): by its path, with
//! its text, or not at all, and why one cannot be, naming it and quoting none of its text.
//!
//! Must not depend on: the app or the file system; it looks at a buffer.

use oso_core::buffer::{Buffer, Mode, Verdict};
use oso_core::editor::SCRATCH;

/// How a buffer is handed over.
pub(super) enum Handed<'a> {
    /// An empty `*scratch*`, which needs nothing.
    Nothing,
    /// By the path a new instance reads it from.
    Path(&'a str),
    /// With its text, and the path it visits, if any.
    Carried(Option<&'a str>),
}

/// How `buffer` is handed over, or an error naming it and quoting none of its text if the
/// new instance would not end up with the same text.
pub(super) fn handed(buffer: &Buffer) -> Result<Handed<'_>, String> {
    // An ended terminal holds no text; a running one holds the restart in `held`.
    if buffer.is_terminal() {
        return Ok(Handed::Nothing);
    }
    // A native session's composer is made again with its session, empty: one that holds a
    // message refuses the restart, which would lose it (ADR 0165, step 2).
    if buffer.mode() == Mode::Composer {
        if buffer.text().is_empty() {
            return Ok(Handed::Nothing);
        }
        return Err(format!(
            "cannot hand over {}: it holds a message not sent; send or clear it",
            buffer.name()
        ));
    }
    let path = buffer.path().map(|path| {
        path.to_str().ok_or_else(|| {
            format!(
                "cannot hand over a path that is not UTF-8: {}",
                path.display()
            )
        })
    });
    let scratch = path.is_none() && buffer.name() == SCRATCH;
    // An emptied `*scratch*` holds nothing to carry, though it counts as modified.
    let carried = if scratch {
        !buffer.text().is_empty()
    } else {
        buffer.is_modified()
    };
    match path {
        // The disk may end up holding what this buffer no longer does.
        Some(_) if buffer.is_saving() => Err(format!(
            "cannot hand over {}: it is being saved",
            buffer.name()
        )),
        // A file not judged yet counts too, until it is.
        _ if carried && buffer.is_secret() => Err(format!(
            "cannot hand over {}: it has unsaved edits and may hold secrets",
            buffer.name()
        )),
        _ if carried && buffer.recovery_stands() => Err(format!(
            "cannot hand over {}: it holds an emergency copy not saved yet",
            buffer.name()
        )),
        Some(path) if carried => Ok(Handed::Carried(Some(path?))),
        None if carried && scratch => Ok(Handed::Carried(None)),
        // Read again by its path, it would be judged by its name alone (ADR 0074).
        Some(_) if buffer.verdict() == Verdict::Yanked => Err(format!(
            "cannot hand over {}: it holds a secret kill yanked into it",
            buffer.name()
        )),
        Some(path) => Ok(Handed::Path(path?)),
        None if scratch => Ok(Handed::Nothing),
        None => Err(format!(
            "cannot hand over {}: it has no file to read again",
            buffer.name()
        )),
    }
}
