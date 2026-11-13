//! Claude Code's transcripts, for `session-adopt` (ADR 0121, "Adopting a session started outside
//! oso"): which conversations stopped lately, and for each only its id, its name, when it last
//! changed and the directory it ran in. Never what was said in it.
//!
//! Must not depend on: threads, the UI or `app`; the caller reads on a thread of its own, never
//! the UI thread, and gives the time.
//!
//! - Only `<projects>/<slug>/<uuid>.jsonl` is opened: a top-level file whose name is a session
//!   id as oso writes one. Subdirectories (a session's subagents) and other files are not.
//! - At most [`MAX_PROJECTS`] project directories and [`MAX_NAMES`] names in all are looked at;
//!   each file is a regular file of this user's, opened without following a link.
//! - Of a file, at most [`HEAD`] bytes at its start and [`TAIL`] at its end are read. A name is
//!   taken only from a line whose bytes *start* as a title line does, such as
//!   `{"type":"custom-title"`: a message's text can never start a line. The directory is the
//!   top-level `cwd` of the first line in the head that has one, found by a scanner that skips
//!   every other value without decoding it.
//! - What was read is kept by file, length and change time, so a file is read again only when
//!   it changed.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use oso_platform::file_meta;
use oso_platform::privacy::is_this_users;

use super::identity::SessionId;
use super::registry;

/// The most project directories looked at.
pub const MAX_PROJECTS: usize = 1024;

/// The most names looked at in all the project directories, transcripts or not.
pub const MAX_NAMES: usize = 4096;

/// The most conversations listed, the newest.
pub const MAX_LISTED: usize = 100;

/// The most transcripts read in one list, as one with no directory found is read but not
/// listed.
pub const MAX_READS: usize = 2 * MAX_LISTED;

/// How long ago a conversation may have changed and still be listed.
pub const RECENT: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The most bytes read at a transcript's start.
pub const HEAD: u64 = 64 * 1024;

/// The most bytes read at a transcript's end, where a later rename is.
pub const TAIL: u64 = 128 * 1024;

/// The longest name kept, in bytes.
const MAX_TITLE: usize = 256;

/// The longest directory kept, in bytes.
const MAX_CWD: usize = 4096;

/// The longest title line looked at.
const MAX_TITLE_LINE: usize = 4096;

/// What a title line starts with, the field that holds its name, and its rank: the name the
/// person gave first, then the agent's name, then the title Claude Code made itself.
const TITLES: [(&str, &str); 3] = [
    ("{\"type\":\"custom-title\"", "customTitle"),
    ("{\"type\":\"agent-name\"", "agentName"),
    ("{\"type\":\"ai-title\"", "aiTitle"),
];

/// A conversation of Claude Code's that oso may resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: SessionId,
    /// When its transcript last changed.
    pub changed: SystemTime,
    /// The directory it ran in, absolute.
    pub directory: PathBuf,
    /// Its name, by [`TITLES`]' rank, the last of each kind.
    pub title: Option<String>,
}

/// What was read of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Read {
    directory: Option<PathBuf>,
    title: Option<String>,
}

/// What was read of each transcript, by its path, with its length and change time then.
#[derive(Debug, Default)]
pub struct Cache {
    read: HashMap<PathBuf, (u64, SystemTime, Read)>,
}

/// The transcripts' directory: `projects` beside the registry's, so under `$CLAUDE_CONFIG_DIR`,
/// else `~/.claude`.
pub fn directory() -> Option<PathBuf> {
    registry::directory().and_then(|sessions| Some(sessions.parent()?.join("projects")))
}

/// The conversations under `projects` that changed within [`RECENT`] of `now`, newest first, at
/// most [`MAX_LISTED`], leaving out those `left_out` names. Ones whose directory cannot be found
/// are not listed.
pub fn list(
    projects: &Path,
    now: SystemTime,
    left_out: impl Fn(&SessionId) -> bool,
    cache: &mut Cache,
) -> Vec<Conversation> {
    let mut found: Vec<(SystemTime, u64, SessionId, PathBuf)> = Vec::new();
    let mut names = 0;
    let Ok(dirs) = std::fs::read_dir(projects) else {
        return Vec::new();
    };
    'dirs: for dir in dirs.take(MAX_PROJECTS).filter_map(Result::ok) {
        if !dir.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.filter_map(Result::ok) {
            names += 1;
            if names > MAX_NAMES {
                break 'dirs;
            }
            let Some(id) = id_of(&file.file_name()) else {
                continue;
            };
            if left_out(&id) {
                continue;
            }
            let Ok(meta) = file.metadata() else {
                continue;
            };
            let Ok(changed) = meta.modified() else {
                continue;
            };
            let recent = now
                .duration_since(changed)
                .map_or(true, |ago| ago <= RECENT);
            if meta.is_file() && recent {
                found.push((changed, meta.len(), id, file.path()));
            }
        }
    }
    found.sort_by_key(|found| std::cmp::Reverse(found.0));
    let mut kept: HashMap<PathBuf, (u64, SystemTime, Read)> = HashMap::new();
    let mut listed = Vec::new();
    let mut reads = 0;
    for (changed, len, id, path) in found {
        if listed.len() == MAX_LISTED {
            break;
        }
        let read = match cache.read.remove(&path) {
            Some((was_len, was_changed, read)) if was_len == len && was_changed == changed => read,
            _ => {
                reads += 1;
                if reads > MAX_READS {
                    break;
                }
                // One that cannot be read is kept as read too, so that it is not opened again
                // until it changes.
                read_transcript(&path).ok().flatten().unwrap_or(Read {
                    directory: None,
                    title: None,
                })
            }
        };
        if let Some(directory) = read.directory.clone() {
            listed.push(Conversation {
                id,
                changed,
                directory,
                title: read.title.clone(),
            });
        }
        kept.insert(path, (len, changed, read));
    }
    cache.read = kept;
    listed
}

/// The session id a file called `name` holds the transcript of: `<uuid>.jsonl`.
fn id_of(name: &std::ffi::OsStr) -> Option<SessionId> {
    SessionId::parse(name.to_str()?.strip_suffix(".jsonl")?)
}

/// The directory and name of the transcript at `path`, if it is a regular file of this
/// user's.
fn read_transcript(path: &Path) -> io::Result<Option<Read>> {
    let Some((mut file, meta)) = file_meta::open_regular_here(path)? else {
        return Ok(None);
    };
    if !is_this_users(&meta) {
        return Ok(None);
    }
    let len = meta.len();
    let head = read_at(&mut file, 0, HEAD.min(len))?;
    let tail = if len > HEAD {
        let start = len.saturating_sub(TAIL).max(HEAD);
        read_at(&mut file, start, len - start)?
    } else {
        Vec::new()
    };
    let head_lines = whole_lines(&head, true, len <= HEAD);
    // The tail never starts a line it can tell: its first is cut.
    let tail_lines = whole_lines(&tail, false, true);
    let directory = head_lines
        .iter()
        .find_map(|line| top_level_string(line, "cwd"))
        .filter(|cwd| cwd.len() <= MAX_CWD)
        .map(PathBuf::from)
        .filter(|cwd| cwd.is_absolute());
    let title = title(head_lines.iter().chain(tail_lines.iter()).copied());
    Ok(Some(Read { directory, title }))
}

/// `len` bytes of `file` from `start`.
fn read_at(file: &mut File, start: u64, len: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(len).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The whole lines in `bytes`: the first is cut unless `starts` says `bytes` start a line, the
/// last unless `ends` says they end the file.
fn whole_lines(bytes: &[u8], starts: bool, ends: bool) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    if !ends {
        lines.pop();
    }
    if !starts && !lines.is_empty() {
        lines.remove(0);
    }
    lines.retain(|line| !line.is_empty());
    lines
}

/// The best-ranked name in `lines`, the last of its kind.
fn title<'a>(lines: impl Iterator<Item = &'a [u8]>) -> Option<String> {
    let mut best: [Option<String>; TITLES.len()] = Default::default();
    for line in lines {
        if line.len() > MAX_TITLE_LINE {
            continue;
        }
        let Some(rank) = TITLES
            .iter()
            .position(|(start, _)| line.starts_with(start.as_bytes()))
        else {
            continue;
        };
        if let Some(name) = top_level_string(line, TITLES[rank].1)
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty() && name.len() <= MAX_TITLE)
        {
            best[rank] = Some(name);
        }
    }
    best.into_iter().flatten().next()
}

/// The string value of the key `wanted` of the JSON object `line`, at its top level only. Every
/// other value is skipped by its brackets and quotes, never decoded.
fn top_level_string(line: &[u8], wanted: &str) -> Option<String> {
    if line.first() != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut expect_key = false;
    let mut at = 0;
    while at < line.len() {
        match line[at] {
            b'"' => {
                let end = string_end(line, at)?;
                if depth == 1 && expect_key {
                    expect_key = false;
                    let key = &line[at + 1..end];
                    if key == wanted.as_bytes() {
                        let colon = skip_blanks(line, end + 1);
                        if line.get(colon) != Some(&b':') {
                            return None;
                        }
                        let value = skip_blanks(line, colon + 1);
                        if line.get(value) != Some(&b'"') {
                            return None;
                        }
                        let value_end = string_end(line, value)?;
                        return serde_json::from_slice(&line[value..=value_end]).ok();
                    }
                }
                at = end;
            }
            b'{' => {
                depth += 1;
                expect_key = depth == 1;
            }
            b'[' => depth += 1,
            b'}' | b']' => depth = depth.checked_sub(1)?,
            b',' => expect_key = depth == 1,
            _ => {}
        }
        at += 1;
    }
    None
}

/// The index of the quote that ends the string starting with the quote at `start`.
fn string_end(line: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    while at < line.len() {
        match line[at] {
            b'\\' => at += 2,
            b'"' => return Some(at),
            _ => at += 1,
        }
    }
    None
}

fn skip_blanks(line: &[u8], mut at: usize) -> usize {
    while line.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

#[cfg(all(test, unix))]
#[path = "transcripts_tests.rs"]
mod tests;
