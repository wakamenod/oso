//! The conversations `session-search` holds in memory (ADR 0145): which transcripts there are,
//! newest first, and the prompts and replies read from each, read again only from where a file
//! grew.
//!
//! Must not depend on: threads, the UI or `app`; the caller lists and reads on threads of its
//! own and gives the time.
//!
//! - Only `<projects>/<slug>/<uuid>.jsonl` is read, at most [`MAX_DIRECTORIES`] directories
//!   and [`MAX_FILES`] transcripts, the newest. Each is a regular file of this user's with one
//!   name, opened without following a link.
//! - A file is kept by its identity, length, change time and the offset past its last whole
//!   line. A longer file of the same identity is read from that offset; a shorter or another
//!   one is read again whole.
//! - A line longer than [`MAX_LINE`] is skipped. A message keeps at most [`MAX_MESSAGE`] bytes,
//!   a conversation at most [`MAX_CONVERSATION`]; the whole hold is bounded by its caller.

use std::fs::File;
use std::io::{self, BufReader, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use oso_platform::file_meta;
use oso_platform::privacy::{has_one_name, is_this_users};

use super::scan;
use crate::sessions::identity::SessionId;

/// The most project directories looked at.
pub const MAX_DIRECTORIES: usize = 1024;

/// The most transcripts held, the newest.
pub const MAX_FILES: usize = 4096;

/// The most names looked at in all the directories, transcripts or not.
pub const MAX_NAMES: usize = 4 * MAX_FILES;

/// The longest line read; a longer one is skipped.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// The most bytes of text a message keeps.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// The most bytes of text a conversation keeps; past them its later messages are not kept.
pub const MAX_CONVERSATION: usize = 1024 * 1024;

/// The most directories a conversation is known to have run in.
const MAX_DIRECTORIES_RUN: usize = 8;

/// A transcript found by listing: its id, its path and what its file is now.
#[derive(Debug, Clone)]
pub struct Listed {
    pub id: SessionId,
    pub path: PathBuf,
    pub changed: SystemTime,
    pub len: u64,
}

/// What a conversation is, apart from its messages: what a row and `RET` need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct About {
    pub id: SessionId,
    /// The directory it first ran in, where it resumes, if a line named one.
    pub directory: Option<PathBuf>,
    /// Every directory it ran in, the first first, at most [`MAX_DIRECTORIES_RUN`].
    pub directories: Vec<PathBuf>,
    /// Its best-ranked title, if a title line named one.
    pub title: Option<String>,
    /// The first line of its first prompt, if it has one.
    pub first_prompt: Option<String>,
    pub changed: SystemTime,
    /// Its first directory is no longer a directory.
    pub gone: bool,
}

/// One prompt or reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub prompt: bool,
    pub text: Box<str>,
}

/// A conversation held: what it is and what was said.
#[derive(Debug)]
pub struct Conversation {
    pub about: About,
    pub messages: Vec<Message>,
    /// The bytes of text its messages keep.
    pub bytes: usize,
    /// More was said than it keeps.
    pub cut: bool,
    /// Each title's rank's latest.
    titles: [Option<String>; scan::TITLES.len()],
    /// What its file was when last read.
    file: FileState,
    /// The transcript's path.
    pub path: PathBuf,
}

/// What a transcript's file was when last read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileState {
    identity: Option<(u64, u64)>,
    len: u64,
    changed: SystemTime,
    /// The offset past its last whole line read.
    read_to: u64,
}

/// The transcripts under `projects`, newest first, at most [`MAX_FILES`].
pub fn list(projects: &Path) -> Vec<Listed> {
    let mut found = Vec::new();
    let mut names = 0;
    let Ok(dirs) = std::fs::read_dir(projects) else {
        return found;
    };
    'dirs: for dir in dirs.take(MAX_DIRECTORIES).filter_map(Result::ok) {
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
            let Some(id) = file
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
                .and_then(SessionId::parse)
            else {
                continue;
            };
            // Not followed: a link is not a regular file here.
            let Ok(meta) = std::fs::symlink_metadata(file.path()) else {
                continue;
            };
            let Ok(changed) = meta.modified() else {
                continue;
            };
            if meta.is_file() {
                found.push(Listed {
                    id,
                    path: file.path(),
                    changed,
                    len: meta.len(),
                });
            }
        }
    }
    found.sort_by_key(|listed| std::cmp::Reverse(listed.changed));
    found.truncate(MAX_FILES);
    found
}

/// The device and inode of a file, where the system gives them.
fn identity(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

impl Conversation {
    fn new(id: SessionId, path: PathBuf) -> Self {
        Self {
            path,
            about: About {
                id,
                directory: None,
                directories: Vec::new(),
                title: None,
                first_prompt: None,
                changed: SystemTime::UNIX_EPOCH,
                gone: false,
            },
            messages: Vec::new(),
            bytes: 0,
            cut: false,
            titles: Default::default(),
            file: FileState {
                identity: None,
                len: 0,
                changed: SystemTime::UNIX_EPOCH,
                read_to: 0,
            },
        }
    }

    /// Whether `listed` is the file as it was last read, so nothing need be read.
    pub fn current(&self, listed: &Listed) -> bool {
        self.file.len == listed.len && self.file.changed == listed.changed
    }

    /// Takes in one line's reading.
    fn take(&mut self, line: scan::Line) {
        if let Some(cwd) = line.cwd
            && self.about.directories.len() < MAX_DIRECTORIES_RUN
            && !self.about.directories.contains(&cwd)
        {
            if self.about.directory.is_none() {
                self.about.directory = Some(cwd.clone());
            }
            self.about.directories.push(cwd);
        }
        if let Some((rank, title)) = line.title {
            self.titles[rank] = Some(title);
            self.about.title = self.titles.iter().flatten().next().cloned();
        }
        let Some((prompt, texts)) = line.message else {
            return;
        };
        for text in texts {
            if prompt && self.about.first_prompt.is_none() {
                let first = text.trim().lines().next().unwrap_or_default();
                self.about.first_prompt = Some(cut(first, 200).to_owned());
            }
            if self.cut {
                continue;
            }
            let text = cut(&text, MAX_MESSAGE);
            if self.bytes + text.len() > MAX_CONVERSATION {
                self.cut = true;
                continue;
            }
            self.bytes += text.len();
            self.messages.push(Message {
                prompt,
                text: text.into(),
            });
        }
    }
}

/// `text` cut to at most `bytes`, at a character's boundary.
fn cut(text: &str, bytes: usize) -> &str {
    &text[..text.floor_char_boundary(bytes.min(text.len()))]
}

/// Reads `listed` into `held`, its conversation as last read if any: only what was added since,
/// if the file only grew, else all of it. Gives the conversation, or none if the file cannot be
/// read as a regular file of this user's with one name.
pub fn read(listed: &Listed, held: Option<Conversation>) -> Option<Conversation> {
    let opened = file_meta::open_regular_here(&listed.path).ok().flatten();
    let (mut file, meta) = opened?;
    if !is_this_users(&meta) || !has_one_name(&meta) {
        return None;
    }
    let now = FileState {
        identity: identity(&meta),
        len: meta.len(),
        changed: meta.modified().unwrap_or(listed.changed),
        read_to: 0,
    };
    let mut conversation = match held {
        Some(held)
            if held.file.identity == now.identity
                && held.file.len <= now.len
                && held.about.id == listed.id =>
        {
            held
        }
        _ => Conversation::new(listed.id.clone(), listed.path.clone()),
    };
    let from = conversation.file.read_to;
    let read_to = read_lines(&mut file, from, now.len, &mut conversation).unwrap_or(from);
    conversation.file = FileState { read_to, ..now };
    conversation.about.changed = now.changed;
    conversation.about.gone = conversation
        .about
        .directory
        .as_deref()
        .is_some_and(|directory| !directory.is_dir());
    Some(conversation)
}

/// Reads the whole lines of `file` from `from` up to `len` into `conversation`, and gives the
/// offset past the last whole line.
fn read_lines(
    file: &mut File,
    from: u64,
    len: u64,
    conversation: &mut Conversation,
) -> io::Result<u64> {
    file.seek(SeekFrom::Start(from))?;
    let mut reader = BufReader::with_capacity(256 * 1024, io::Read::take(file, len - from));
    let mut at = from;
    let mut line = Vec::new();
    loop {
        line.clear();
        let (ended, taken, skipped) = next_line(&mut reader, &mut line)?;
        if taken == 0 {
            return Ok(at);
        }
        if !ended {
            // A line still being written: read from its start next time.
            return Ok(at);
        }
        at += taken;
        if !skipped && !line.is_empty() {
            conversation.take(scan::read(&line));
        }
    }
}

/// Reads one line of `reader` into `line`, without its line break, keeping at most
/// [`MAX_LINE`] bytes of it. Gives whether it ended with a line break, how many bytes were
/// taken, and whether it was too long and is skipped.
fn next_line(reader: &mut impl io::BufRead, line: &mut Vec<u8>) -> io::Result<(bool, u64, bool)> {
    let mut taken = 0u64;
    let mut skipped = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok((false, taken, skipped));
        }
        let (part, ended) = match buffer.iter().position(|&byte| byte == b'\n') {
            Some(at) => (&buffer[..at], true),
            None => (buffer, false),
        };
        if !skipped {
            if line.len() + part.len() > MAX_LINE {
                skipped = true;
                line.clear();
            } else {
                line.extend_from_slice(part);
            }
        }
        let used = part.len() + usize::from(ended);
        taken += used as u64;
        reader.consume(used);
        if ended {
            return Ok((true, taken, skipped));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation() -> Conversation {
        let id = SessionId::parse("6c1b5d1e-2f3a-4b5c-8d9e-0f1a2b3c4d5e").unwrap();
        Conversation::new(id, PathBuf::from("/c/p/x.jsonl"))
    }

    #[test]
    fn a_line_past_the_longest_is_skipped_and_the_next_read() {
        let long = vec![b'x'; MAX_LINE + 10];
        let mut bytes = long.clone();
        bytes.push(b'\n');
        bytes.extend_from_slice(b"next\npartial");
        let mut reader = io::BufReader::with_capacity(4096, bytes.as_slice());
        let mut line = Vec::new();
        let (ended, taken, skipped) = next_line(&mut reader, &mut line).unwrap();
        assert!(ended && skipped);
        assert_eq!(taken, long.len() as u64 + 1);
        line.clear();
        assert_eq!(next_line(&mut reader, &mut line).unwrap(), (true, 5, false));
        assert_eq!(line, b"next");
        line.clear();
        assert_eq!(
            next_line(&mut reader, &mut line).unwrap(),
            (false, 7, false)
        );
    }

    #[test]
    fn titles_rank_and_directories_keep_the_first() {
        let mut held = conversation();
        held.take(scan::read(br#"{"type":"ai-title","aiTitle":"Made"}"#));
        assert_eq!(held.about.title.as_deref(), Some("Made"));
        held.take(scan::read(
            br#"{"type":"custom-title","customTitle":"Mine"}"#,
        ));
        held.take(scan::read(br#"{"type":"ai-title","aiTitle":"Made again"}"#));
        assert_eq!(held.about.title.as_deref(), Some("Mine"));
        held.take(scan::read(
            br#"{"type":"user","cwd":"/w/a","message":{"content":"one\ntwo"}}"#,
        ));
        held.take(scan::read(
            br#"{"type":"user","cwd":"/w/a/.claude/worktrees/b","message":{"content":"three"}}"#,
        ));
        assert_eq!(held.about.directory, Some(PathBuf::from("/w/a")));
        assert_eq!(held.about.directories.len(), 2);
        assert_eq!(held.about.first_prompt.as_deref(), Some("one"));
        assert_eq!(held.messages.len(), 2);
    }

    #[test]
    fn a_conversation_keeps_at_most_its_bytes() {
        let mut held = conversation();
        let text = "y".repeat(MAX_MESSAGE * 2);
        let line = format!(r#"{{"type":"user","message":{{"content":"{text}"}}}}"#);
        for _ in 0..(MAX_CONVERSATION / MAX_MESSAGE + 2) {
            held.take(scan::read(line.as_bytes()));
        }
        assert!(held.cut);
        assert!(held.bytes <= MAX_CONVERSATION);
        assert!(
            held.messages
                .iter()
                .all(|message| message.text.len() == MAX_MESSAGE)
        );
    }
}
