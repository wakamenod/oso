//! Writing a buffer's text to a new place, for `C-x C-w` (ADR 0087).
//!
//! Must not depend on: `app` or winit; a write reports back like a save.
//!
//! The new path is followed to where it leads, on this thread, and then written as a save
//! writes, in the same order (ADR 0059), with nothing expected there, or the file the person
//! agreed to write over. The path is judged as a load judges it, so that the buffer's verdict
//! comes from where its text now is (ADR 0044).

use std::fs;
use std::path::{Path, PathBuf};

use oso_core::buffer::Baseline;
use oso_core::editor::{Elsewhere, SaveJob};

use super::{SaveError, Written, prepare};
use crate::loader::{Resolved, location, own_spelling, resolve, surely_same_file};

/// Where the text of a write to a new place went, for the buffer to visit.
#[derive(Debug)]
pub struct Moved {
    /// The real place the path led.
    pub(super) location: PathBuf,
    /// What judging the path found.
    pub(super) resolved: Resolved,
    /// The file's own spelling there.
    pub(super) spelling: Option<PathBuf>,
    /// The project the file is in there (ADR 0114).
    pub(super) project: Option<oso_core::project::Found>,
    /// The path written, which the buffer now visits: the one given, or the buffer's file's
    /// name in it if it was a directory.
    pub(super) path: PathBuf,
}

/// Writes `job`'s text to its path, as `elsewhere` says what may be there.
pub(super) fn write(job: &SaveJob, elsewhere: &Elsewhere) -> Result<Option<Written>, SaveError> {
    // A directory takes the buffer's file's name, as Emacs's `write-file` does.
    let path = if fs::metadata(&job.path).is_ok_and(|metadata| metadata.is_dir()) {
        job.path.join(&elsewhere.name)
    } else {
        job.path.clone()
    };
    if fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(SaveError::Io(std::io::Error::other("it is a directory")));
    }
    let location = location(&path)?;
    if is_open_elsewhere(&location, &elsewhere.open) {
        return Err(SaveError::OpenElsewhere);
    }
    // The file agreed to is written over only where the path led when it was found.
    if let Some(over) = &elsewhere.over
        && over.location != location
    {
        return Err(SaveError::Elsewhere);
    }
    let located = SaveJob {
        buffer: job.buffer,
        path: path.clone(),
        text: job.text.clone(),
        bom: job.bom,
        content: job.content,
        baseline: Some(Baseline {
            location: location.clone(),
            file: elsewhere.over.as_ref().map(|over| over.found),
        }),
        only_if_gone: false,
        elsewhere: None,
    };
    let written = prepare(&located).and_then(|prepared| prepared.commit(&located));
    let mut written = match written {
        Err(SaveError::Conflict(Some(found))) => {
            return Err(SaveError::Exists { found, location });
        }
        other => other?,
    };
    written.moved = Some(Box::new(Moved {
        resolved: resolve(&path),
        spelling: own_spelling(&location),
        project: std::panic::catch_unwind(|| crate::project::detect(&location))
            .ok()
            .flatten(),
        location,
        path,
    }));
    Ok(Some(written))
}

/// Whether `location` is the file of one of `open`, another buffer's files, by what the
/// system says of its identity, so that another spelling, a link or a second name of it counts.
fn is_open_elsewhere(location: &Path, open: &[PathBuf]) -> bool {
    if open.iter().any(|other| other == location) {
        return true;
    }
    let Ok(there) = fs::metadata(location) else {
        return false;
    };
    open.iter()
        .any(|other| fs::metadata(other).is_ok_and(|other| surely_same_file(&there, &other)))
}

#[cfg(test)]
mod tests;
