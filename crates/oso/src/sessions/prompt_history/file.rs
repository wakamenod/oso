//! The history of what the person sent, kept in `oso/prompt-history` in the per-user state
//! directory (ADR 0165, step 2, "History"): the first line its version and its last clear, then
//! one JSON object a line, a text and when it was sent, the newest first.
//!
//! Must not depend on: the UI or `app`; the history's own threads read and write it.
//!
//! - It is read and replaced as `sessions` is, with `sessions::record`'s helpers: only in a
//!   private directory, a regular file of this user's with one name, at most [`MAX_BYTES`],
//!   replaced whole through a `.part` of mode `0600`.
//! - A line that holds no whole entry is left out, and only how many were is logged. A file
//!   that may not be read, or of another version, is not replaced: only a clear takes it out.
//! - A write reads the file first and merges it, under `prompt-history.lock`, so that no
//!   instance loses another's messages or brings back what one cleared. Where the lock cannot be
//!   had at all, as on some file systems, it goes on without.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::file_meta;
use oso_platform::privacy::PrivateDir;
use serde_json::{Value, json};

use super::{Entry, History, MAX_ENTRIES, MAX_STAMP};
use crate::sessions::native::queue::MAX_MESSAGE;
use crate::sessions::record::{read_file, replace_file};

const FILE: &str = "prompt-history";
/// Where a write goes first, so that the file is whole or not there.
const PART: &str = "prompt-history.part";
/// Held while a write reads, merges and replaces the file; it holds nothing.
const LOCK: &str = "prompt-history.lock";
/// The most bytes read or written of the file: twice the texts', as JSON escapes some.
pub const MAX_BYTES: u64 = 1024 * 1024;
const VERSION: u64 = 1;
/// How long a write waits for another instance's.
const LOCK_WAIT: Duration = Duration::from_millis(500);

/// What the file in `directory` holds: nothing if there is none.
///
/// # Errors
///
/// If it may not be read, with no word of what it holds.
pub fn read(directory: &Path) -> io::Result<History> {
    match read_file(directory, FILE, MAX_BYTES)? {
        Some(bytes) => parse(&bytes),
        None => Ok(History::default()),
    }
}

/// Merges `ours` with what the file in `directory`, made private if it is missing, holds, and
/// replaces it with that, which it gives. `clears`: all the file holds goes, and a file that may
/// not be read.
///
/// # Errors
///
/// If the file may not be read or replaced, or, as `WouldBlock`, another instance holds it.
pub fn write(directory: &Path, ours: &History, clears: bool) -> io::Result<History> {
    let private = PrivateDir::open(directory)?;
    let _lock = lock(&private)?;
    let theirs = match read(directory) {
        Err(_) if clears => {
            match fs::remove_file(private.path().join(FILE)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            History::default()
        }
        theirs => theirs?,
    };
    let mut merged = ours.clone();
    // A clear takes out all the file has, also what was stamped by a clock ahead.
    match clears {
        true => merged.clear_all_of(&theirs),
        false => merged.merge(&theirs),
    }
    replace_file(&private, FILE, PART, &text(&merged))?;
    Ok(merged)
}

/// Holds the lock in `directory` until the file given is dropped, waiting at most
/// [`LOCK_WAIT`] for another instance; none if it cannot be had at all.
fn lock(directory: &PrivateDir) -> io::Result<Option<File>> {
    let path = directory.path().join(LOCK);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    // There already, or not to be had, which the opening says.
    let _ = options.open(&path);
    let Ok(Some((file, _))) = file_meta::open_regular_here(&path) else {
        return Ok(None);
    };
    let until = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) if Instant::now() < until => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another oso holds it",
                ));
            }
            Err(TryLockError::Error(_)) => return Ok(None),
        }
    }
}

/// The file's text of `history`, at most [`MAX_BYTES`]: the oldest that do not fit go.
fn text(history: &History) -> String {
    let mut text = json!({ "version": VERSION, "cleared": history.cleared() }).to_string();
    text.push('\n');
    let most = usize::try_from(MAX_BYTES).unwrap_or(usize::MAX);
    for entry in history.entries() {
        let line = json!({ "text": &*entry.text, "sent": entry.sent }).to_string();
        if text.len() + line.len() + 1 > most {
            break;
        }
        let _ = writeln!(text, "{line}");
    }
    text
}

/// The history in the file's bytes.
fn parse(bytes: &[u8]) -> io::Result<History> {
    let mut lines = (bytes.split(|&b| b == b'\n'))
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .peekable();
    let mut history = History::default();
    let mut bad = 0;
    let head = (lines.peek()).and_then(|line| serde_json::from_slice::<Value>(line).ok());
    match head.as_ref().and_then(|head| head.get("version")) {
        Some(version) if version.as_u64() == Some(VERSION) => {
            history.cleared = (head.as_ref())
                .and_then(|head| head.get("cleared")?.as_u64())
                .filter(|cleared| *cleared <= MAX_STAMP)
                .unwrap_or(0);
            lines.next();
        }
        Some(_) => return Err(io::Error::other("it is another version's")),
        // A corrupt head is a bad line, as any.
        None => {}
    }
    for line in lines {
        match parse_line(line) {
            Some(_) if history.entries.len() == MAX_ENTRIES => bad += 1,
            Some(entry) => history.entries.push(entry),
            None => bad += 1,
        }
    }
    history.settle();
    if bad > 0 {
        log::warn!("left out {bad} lines of the prompt history that hold no whole entry");
    }
    Ok(history)
}

/// The entry a line holds, if a composer could have sent it.
fn parse_line(line: &[u8]) -> Option<Entry> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let text = value.get("text")?.as_str()?;
    let sent = value.get("sent")?.as_u64()?;
    let kept = text.len() <= MAX_MESSAGE && !text.trim().is_empty() && sent <= MAX_STAMP;
    kept.then(|| Entry {
        text: text.into(),
        sent,
    })
}

// The private directory these tests use is a Unix one.
#[cfg(all(test, unix))]
mod tests;
