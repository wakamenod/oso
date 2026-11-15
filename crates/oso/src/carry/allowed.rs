//! Whether an instance may exit for a restart, or take one over, and the generation that
//! names what it holds that no file does (ADR 0047, ADR 0055, ADR 0090).
//!
//! Must not depend on: `app`, `server` or sockets; these are plain functions of the editor.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use oso_core::editor::{Editor, SCRATCH};

use super::EXIT_REFUSED;
use super::{SECRET_SHOWN, shows_judged_secret, under_way};

/// Names what an instance holds that a file does not: which buffers exist, the
/// content of each modified one, and the kill ring's newest entry and size. It is
/// not a counter, as undoing an edit brings the old value back, and only the
/// instance that made it compares it.
///
/// A buffer that is not modified counts only by its id, since each piece of a
/// load gets a new content id and a large file may still be arriving. A save of a
/// modified buffer changes it, as the buffer stops being modified. Marks, cursors
/// and histories do not count: they change all the time in a window in use, and
/// put no text at stake (ADR 0090).
pub fn generation(editor: &Editor) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (id, buffer) in editor.buffers() {
        id.hash(&mut hasher);
        buffer
            .is_modified()
            .then(|| buffer.content_id())
            .hash(&mut hasher);
    }
    let ring = editor.kill_ring();
    ring.len().hash(&mut hasher);
    ring.get(0)
        .map(|newest| (newest.id(), newest.len()))
        .hash(&mut hasher);
    hasher.finish()
}

/// Whether `exit` may go ahead. Without a generation it always may: that is a
/// person or an agent saying so. With one, only if nothing has changed since and
/// no save is running, as a new instance may already have read the old file, and not
/// while a buffer that may hold secrets is shown.
pub fn exit_allowed(editor: &Editor, expected: Option<u64>) -> Result<(), String> {
    match expected {
        Some(_) if shows_judged_secret(editor) => Err(format!("{EXIT_REFUSED}: {SECRET_SHOWN}")),
        Some(_) if editor.buffers().any(|(_, buffer)| buffer.is_saving()) => {
            Err(format!("{EXIT_REFUSED}: a save is running"))
        }
        Some(_) if editor.preedit().is_some() => {
            Err(format!("{EXIT_REFUSED}: a composition is open"))
        }
        Some(_) if editor.prompt().is_some() => Err(format!("{EXIT_REFUSED}: a prompt is open")),
        Some(_) if under_way(editor).is_some() => Err(format!(
            "{EXIT_REFUSED}: {}",
            under_way(editor).unwrap_or_default()
        )),
        Some(expected) if expected != generation(editor) => Err(format!(
            "{EXIT_REFUSED}: the buffers have changed since generation {expected}"
        )),
        _ => Ok(()),
    }
}

/// Whether this instance may take over from another. It may only if it has
/// opened nothing and its `*scratch*` is empty, as the imported buffers would hide
/// any text there. Internal buffers do not count, but one in view does, since the
/// window then has no `*scratch*` to start from.
pub fn import_allowed(editor: &Editor) -> Result<(), String> {
    let (_, scratch) = editor.current();
    if scratch.is_internal() {
        return Err(format!("this instance's window has no {SCRATCH} in view"));
    }
    // A session that came back stopped is a terminal with nothing running: the import may
    // bring its terminal (ADR 0120, step 11).
    let opened = editor
        .buffers()
        .filter(|(_, buffer)| !buffer.is_terminal() || buffer.is_running())
        .count();
    if opened > 1 || scratch.name() != SCRATCH {
        return Err("this instance has already opened something".into());
    }
    if !scratch.text().is_empty() {
        return Err(format!(
            "this instance has text in {SCRATCH}, which the import would hide"
        ));
    }
    Ok(())
}
