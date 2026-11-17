//! What a file opened unshown came to, once the loader judged and read it (ADR 0094): whether
//! it is the file asked for, and whether another buffer already holds that very file, by its
//! device and inode. The agent channel's `open` (ADR 0085) and a language server's edit of a
//! file no buffer holds (ADR 0138, step 13b) judge by these.
//!
//! Must not depend on: the loader's internals or threads; these read buffers only.

use std::ffi::OsStr;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::Editor;

/// What another buffer holds of the file `id`'s load opened (ADR 0087, ADR 0094).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Held {
    /// That very file, by its device and inode, read as it is now: this buffer shows it.
    Same(BufferId),
    /// A file of the same device and inode, read at another time or size: a file since
    /// replaced in that inode, or one that changed since, which cannot be told apart.
    Stale,
}

/// The buffer, other than `id` and one the agent channel is still opening, that holds the file
/// `id`'s load opened, if one does. Only by device and inode: time and size alone tell no file
/// apart.
pub(in crate::app) fn held_elsewhere(editor: &Editor, id: BufferId) -> Option<Held> {
    let opened = editor.buffer(id)?.baseline()?.file?;
    opened.identity?;
    let (other, held) = editor.buffers().find_map(|(other, buffer)| {
        let held = buffer.baseline()?.file?;
        (other != id && !buffer.agent_pending() && held.identity == opened.identity)
            .then_some((other, held))
    })?;
    Some(if held.same_content(opened) {
        Held::Same(other)
    } else {
        Held::Stale
    })
}

/// Why a file opened unshown and read is not the one asked for, if it is not: the load found
/// no file there any more, so it would be a new one, which neither the agent channel nor a
/// server's edit starts, or the path led, through a link, to a file of another name.
pub(in crate::app) fn found_elsewhere(
    buffer: &Buffer,
    asked: Option<&OsStr>,
) -> Option<&'static str> {
    let Some(baseline) = buffer.baseline() else {
        return Some("where the path leads could not be told");
    };
    if baseline.file.is_none() {
        return Some("no file is at that path now");
    }
    (baseline.location.file_name() != asked).then_some("the path leads to a file of another name")
}
