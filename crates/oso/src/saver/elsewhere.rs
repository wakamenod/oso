//! Writing a buffer's text to a new place, for `C-x C-w` (ADR 0087).
//!
//! Must not depend on: `app` or winit; a write reports back like a save.
//!
//! The new path is followed to where it leads, on this thread, and then written as a save
//! writes, in the same order (ADR 0059), with nothing expected there, or the file the person
//! agreed to write over. The path is judged as a load judges it, so that the buffer's verdict
//! comes from where its text now is (ADR 0044).

use std::fs;
use std::path::PathBuf;

use oso_core::buffer::Baseline;
use oso_core::editor::{Elsewhere, SaveJob};

use super::{SaveError, Written, prepare};
use crate::loader::{Resolved, location, own_spelling, resolve};

/// Where the text of a write to a new place went, for the buffer to visit.
#[derive(Debug)]
pub struct Moved {
    /// The real place the path led.
    pub(super) location: PathBuf,
    /// What judging the path found.
    pub(super) resolved: Resolved,
    /// The file's own spelling there.
    pub(super) spelling: Option<PathBuf>,
}

/// Writes `job`'s text to its path, as `elsewhere` says what may be there.
pub(super) fn write(job: &SaveJob, elsewhere: Elsewhere) -> Result<Option<Written>, SaveError> {
    if fs::metadata(&job.path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(SaveError::Io(std::io::Error::other("it is a directory")));
    }
    let location = location(&job.path)?;
    let located = SaveJob {
        buffer: job.buffer,
        path: job.path.clone(),
        text: job.text.clone(),
        bom: job.bom,
        content: job.content,
        baseline: Some(Baseline {
            location: location.clone(),
            file: elsewhere.over,
        }),
        only_if_gone: false,
        elsewhere: None,
    };
    let mut written = prepare(&located)?.commit(&located)?;
    written.moved = Some(Moved {
        resolved: resolve(&job.path),
        spelling: own_spelling(&location),
        location,
    });
    Ok(Some(written))
}

#[cfg(test)]
mod tests;
