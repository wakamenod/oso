//! A native session's earlier turns read from Claude Code's transcript of its conversation
//! (ADR 0165, step 5), once, after its process is spawned and before its stream is read: so
//! the rows have them before anything the process says, and the read is hidden under the
//! CLI's own start. It runs on a thread of its own, `oso-native-past`, which the start thread
//! waits for at most [`DEADLINE`] and a little more, so that a file on a mount that hangs
//! holds the process unowned no longer: the read then shows nothing, and its thread is left.
//!
//! Must not depend on: `app` or the UI thread; `native::past` chooses the lines and `wire`
//! reads them.
//!
//! - The transcript is `<projects>/<slug>/<id>.jsonl` under the transcripts' directory
//!   (`transcripts::directory`), the slug made from the session's directory as it is given,
//!   then as it resolves; else the first of at most [`MAX_DIRECTORIES`] project directories
//!   that holds `<id>.jsonl`, the project directory not a link. It is read only as a regular
//!   file of this user's with one name, opened without following a link, and only if the
//!   first line that names a directory names the session's; one that names none is not.
//! - At most the last [`MAX_READ`] bytes are read, up to the length the file had when opened:
//!   what the process writes meanwhile is its own. A line longer than `wire`'s
//!   [`MAX_LINE`](crate::sessions::native::wire::MAX_LINE) does not read, nor does anything
//!   after [`DEADLINE`], nor a transcript of more lines than `native::past` places: then
//!   nothing is shown, and the rows say why.
//! - Each line is placed by its top-level fields only; then only the lines shown are read
//!   again and decoded, at most `native::past`'s limits. Nothing of what was read is logged.

use std::fs::File;
use std::io::{self, BufReader, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use oso_platform::file_meta;
use oso_platform::privacy::{has_one_name, is_this_users};

use super::history::{next_line_within, slug_of};
use super::identity::SessionId;
use super::native::past::{Index, Missing, Past, PastAsk};
use super::native::wire::{self, transcript};

/// How long the read may take.
pub const DEADLINE: Duration = Duration::from_secs(1);
/// The most bytes of a transcript read, its end.
pub const MAX_READ: u64 = 128 << 20;
/// The most project directories looked in for a transcript not where its slug says.
pub const MAX_DIRECTORIES: usize = 1024;
/// How many lines are read between looks at the clock.
const LINES_PER_LOOK: usize = 256;
/// How long past [`DEADLINE`] the start thread waits for the read's thread, which looks at
/// the clock only between lines.
const GRACE: Duration = Duration::from_millis(250);

/// The earlier turns `ask` asks for, of the conversation of a session in `directory`, read
/// on a thread of its own and waited for at most [`DEADLINE`] and [`GRACE`].
pub fn read(ask: &PastAsk, directory: &Path) -> Past {
    Past {
        after_rows: !ask.after.is_empty(),
        ..read_on_its_thread(ask, directory)
    }
}

/// [`read`], on `oso-native-past`.
fn read_on_its_thread(ask: &PastAsk, directory: &Path) -> Past {
    let deadline = Instant::now() + DEADLINE;
    let missing = Past::missing;
    let Some(projects) = super::transcripts::directory() else {
        return missing(Missing::NotFound);
    };
    let (given, directory) = (ask.clone(), directory.to_path_buf());
    let (sent, read) = mpsc::sync_channel(1);
    let reading = std::thread::Builder::new()
        .name("oso-native-past".into())
        .spawn(move || {
            let _ = sent.send(read_in(&projects, &given, &directory, deadline));
        });
    if reading.is_err() {
        return missing(Missing::NotRead);
    }
    read.recv_timeout(DEADLINE + GRACE)
        .unwrap_or_else(|_| missing(Missing::TooSlow))
}

/// [`read`] with the transcripts under `projects`, until `deadline`.
pub(crate) fn read_in(projects: &Path, ask: &PastAsk, directory: &Path, deadline: Instant) -> Past {
    Past {
        after_rows: !ask.after.is_empty(),
        ..read_chosen(projects, ask, directory, deadline)
    }
}

/// The lines `ask` asks for, chosen and read.
fn read_chosen(projects: &Path, ask: &PastAsk, directory: &Path, deadline: Instant) -> Past {
    let id = &ask.id;
    let resolved = std::fs::canonicalize(directory).ok();
    let Some(path) = locate(projects, id, directory, resolved.as_deref(), deadline) else {
        return Past::missing(Missing::NotFound);
    };
    let opened = file_meta::open_regular_here(&path).ok().flatten();
    let Some((mut file, meta)) =
        opened.filter(|(_, meta)| is_this_users(meta) && has_one_name(meta))
    else {
        return Past::missing(Missing::NotRead);
    };
    let ran_here = |cwd: &Path| cwd == directory || resolved.as_deref() == Some(cwd);
    let placed = place(&mut file, meta.len(), deadline, ran_here);
    let index = match placed {
        Ok(Placed::Whole(index)) => index,
        Ok(Placed::Elsewhere) => return Past::missing(Missing::Elsewhere),
        Ok(Placed::TooSlow) => return Past::missing(Missing::TooSlow),
        Ok(Placed::TooMany) => return Past::missing(Missing::TooMany),
        Err(_) => return Past::missing(Missing::NotRead),
    };
    let chosen = index.chosen(&ask.after);
    let mut past = Past {
        since: chosen.since,
        cut: chosen.cut,
        prompts_left_out: chosen.prompts_left_out,
        unreadable: chosen.unreadable,
        ..Past::default()
    };
    let mut line = Vec::new();
    for shown in chosen.lines {
        line.resize(shown.len, 0);
        let read = file
            .seek(SeekFrom::Start(shown.at))
            .and_then(|_| file.read_exact(&mut line));
        if read.is_err() {
            return Past::missing(Missing::NotRead);
        }
        match transcript::said(&line, shown.shows) {
            Ok(said) => past.said.extend(said),
            Err(transcript::Unreadable) => past.unreadable += 1,
        }
    }
    past
}

/// Where session `id`'s transcript is: under the slug of `directory`, then of `resolved`, its
/// path resolved, else in the first project directory that holds it.
fn locate(
    projects: &Path,
    id: &SessionId,
    directory: &Path,
    resolved: Option<&Path>,
    deadline: Instant,
) -> Option<PathBuf> {
    let name = format!("{}.jsonl", id.as_str());
    let is_file = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    let is_dir = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    let slugs = std::iter::once(directory).chain(resolved);
    if let Some(path) = slugs
        .map(|directory| projects.join(slug_of(directory)))
        .filter(|dir| is_dir(dir))
        .map(|dir| dir.join(&name))
        .find(|path| is_file(path))
    {
        return Some(path);
    }
    let dirs = std::fs::read_dir(projects).ok()?;
    for dir in dirs.take(MAX_DIRECTORIES).filter_map(Result::ok) {
        if Instant::now() > deadline {
            return None;
        }
        let path = dir.path().join(&name);
        if dir.file_type().is_ok_and(|kind| kind.is_dir()) && is_file(&path) {
            return Some(path);
        }
    }
    None
}

/// What placing a transcript's lines gave.
enum Placed {
    /// Its lines.
    Whole(Index),
    /// It names another directory than the session's, or none.
    Elsewhere,
    /// It was not read whole by the deadline.
    TooSlow,
    /// It has more lines than are placed.
    TooMany,
}

/// Places the whole lines of `file`'s last [`MAX_READ`] bytes of `len`, until `deadline`,
/// once the first that names a directory names one `ran_here` takes.
fn place(
    file: &mut File,
    len: u64,
    deadline: Instant,
    ran_here: impl Fn(&Path) -> bool,
) -> io::Result<Placed> {
    let from = len.saturating_sub(MAX_READ);
    file.seek(SeekFrom::Start(from))?;
    let mut reader = BufReader::with_capacity(256 << 10, io::Read::take(&mut *file, len - from));
    let mut line = Vec::new();
    let mut at = from;
    // Read from the middle of a line: from the next one.
    if from > 0 {
        at += next_line_within(&mut reader, &mut line, 0)?.1;
    }
    let mut index = Index::default();
    let (mut looked, mut lines) = (false, 0usize);
    loop {
        line.clear();
        let (ended, taken, skipped) = next_line_within(&mut reader, &mut line, wire::MAX_LINE)?;
        // A line still being written is not read.
        if taken == 0 || !ended {
            // Lines that name no directory are not known to be the session's.
            if !looked && !index.is_empty() {
                return Ok(Placed::Elsewhere);
            }
            return Ok(Placed::Whole(index));
        }
        lines += 1;
        if lines % LINES_PER_LOOK == 0 && Instant::now() > deadline {
            return Ok(Placed::TooSlow);
        }
        if skipped {
            index.unreadable_at(at);
        } else if !line.is_empty() {
            if !looked && let Some(cwd) = transcript::cwd(&line) {
                if !ran_here(&cwd) {
                    return Ok(Placed::Elsewhere);
                }
                looked = true;
            }
            if !index.take(at, &line) {
                return Ok(Placed::TooMany);
            }
        }
        at += taken;
    }
}

#[cfg(test)]
mod tests;
