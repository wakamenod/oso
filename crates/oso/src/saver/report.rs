//! Taking a save's report in: what changes on the buffer, and what the echo area says, never
//! quoting text (ADR 0059, ADR 0087).
//!
//! Must not depend on: `app` or winit.

use std::path::Path;

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, SaveRefused};

use super::{SaveError, SaveReport, Unsynced, elsewhere};

/// Takes a report in: updates the buffer, and says how it went. Never quotes text.
pub fn apply(editor: &mut Editor, report: SaveReport) {
    let SaveReport {
        buffer,
        path: path_buf,
        outcome,
        elsewhere,
    } = report;
    // A write into a directory went to the buffer's file's name there, which the message names.
    let path_buf = match &outcome {
        Ok(Some(written)) => written
            .moved
            .as_ref()
            .map_or(path_buf, |moved| moved.path.clone()),
        _ => path_buf,
    };
    let path = path_buf.display();
    let message = match outcome {
        Ok(None) => {
            editor.save_unneeded(buffer);
            SaveRefused::Unmodified.to_string()
        }
        Ok(Some(written)) => {
            match written.moved {
                Some(moved) => moved_to(editor, buffer, *moved, written.content, written.stamp),
                None => editor.save_finished(buffer, written.content, written.stamp),
            }
            let lost = written.quarantine_lost.map(|e| {
                format!(
                    "Wrote the file, but its quarantine flag could not be kept, so macOS may not check it when opened: {e}: {path}"
                )
            });
            // What the disk did not confirm is worth more, so it stays on the echo row.
            match (written.unsynced, lost) {
                (None, Some(lost)) => lost,
                (unsynced, lost) => {
                    if let Some(lost) = lost {
                        editor.message(lost);
                    }
                    written_message(unsynced, &path)
                }
            }
        }
        Err(error) if elsewhere => {
            editor.save_failed(buffer);
            elsewhere_failed(editor, buffer, &path_buf, error)
        }
        Err(error) => {
            editor.save_failed(buffer);
            if matches!(error, SaveError::Conflict(_) | SaveError::Elsewhere) {
                editor.file_changed_on_disk(buffer);
            }
            let mut asks = "";
            if let SaveError::Conflict(Some(found)) = error {
                editor.may_save_over(buffer, found);
                asks = ". C-x C-s asks to save over it";
            }
            match error {
                SaveError::Conflict(_) => format!(
                    "Not saved: the file changed on disk since it was read, and was kept{asks}: {path}"
                ),
                SaveError::Elsewhere => format!(
                    "Not saved: the path now leads somewhere other than when it was read, and nothing was written: {path}"
                ),
                SaveError::UnknownLocation => format!(
                    "Not saved: where the path leads could not be told when it was read, so no place is safe: {path}"
                ),
                other => not_saved(other, &path),
            }
        }
    };
    editor.message(message);
}

/// A write to a new place put `content` there as `stamp`: the buffer now visits it, judged as
/// a load judges a file (ADR 0087).
fn moved_to(
    editor: &mut Editor,
    buffer: BufferId,
    moved: elsewhere::Moved,
    content: oso_core::buffer::ContentId,
    stamp: oso_core::buffer::FileStamp,
) {
    let elsewhere::Moved {
        resolved,
        spelling,
        location,
        path,
    } = moved;
    let judge = |buffer: &mut oso_core::buffer::Buffer| {
        resolved.judge(buffer);
        if let Some(spelling) = spelling {
            buffer.set_own_spelling(spelling);
        }
    };
    editor.written_elsewhere(buffer, path, content, location, stamp, judge);
}

/// What a write to a new place that wrote nothing says, and the file found there that the
/// next `C-x C-w` of that path asks to write over.
fn elsewhere_failed(
    editor: &mut Editor,
    buffer: BufferId,
    path_buf: &Path,
    error: SaveError,
) -> String {
    let path = path_buf.display();
    match error {
        SaveError::Exists { found, location } => {
            editor.may_write_over(buffer, path_buf.to_path_buf(), found, location);
            format!("Not saved: a file is there. C-x C-w asks to write over it: {path}")
        }
        SaveError::OpenElsewhere => {
            format!("Not saved: that file is open in another buffer: {path}")
        }
        SaveError::Conflict(None) => format!(
            "Not saved: what is there is no regular file, or it changed during the write: {path}"
        ),
        SaveError::Elsewhere => {
            format!("Not saved: the path led somewhere else during the write: {path}")
        }
        other => not_saved(other, &path),
    }
}

/// What a save or a write says of a failure that comes the same way to both.
fn not_saved(error: SaveError, path: &std::path::Display<'_>) -> String {
    match error {
        SaveError::AccessList(e) => format!(
            "Not saved: its access list could not be kept, and the file is as it was: {e}: {path}"
        ),
        SaveError::ListForbidsReplacing(e) => format!(
            "Not saved: the rename was refused, and its access list denies someone deleting it: {e}: {path}"
        ),
        SaveError::Io(e) => format!("Not saved: {e}: {path}"),
        SaveError::Panicked => format!("Not saved: the save failed: {path}"),
        SaveError::IntoConfiguration => format!(
            "Not saved: the agent channel never saves into oso's configuration directory: {path}"
        ),
        SaveError::Conflict(_)
        | SaveError::Elsewhere
        | SaveError::UnknownLocation
        | SaveError::Exists { .. }
        | SaveError::OpenElsewhere => format!("Not saved: {path}"),
    }
}

/// What a save that wrote the file says of itself.
fn written_message(unsynced: Option<Unsynced>, path: &std::path::Display<'_>) -> String {
    match unsynced {
        None => format!("Wrote {path}"),
        Some(Unsynced::File(e)) => format!(
            "Wrote the file, but the file system cannot confirm the text reached the disk. A power cut could lose it, and any file it replaced: {e}: {path}"
        ),
        Some(Unsynced::Directory(e)) => format!(
            "Wrote the file, but the disk did not confirm the rename. A power cut could bring back what was there: {e}: {path}"
        ),
    }
}
