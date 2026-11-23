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
use std::sync::Arc;
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

/// The longest line read; a longer one is skipped. A message's text keeps at most
/// [`MAX_MESSAGE`] anyway, and the longest lines are tool results, which are never searched; so
/// each reading thread holds at most this and its decoded copy.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// The most bytes of text a message keeps.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// The most bytes a conversation's messages are counted as; past them its later messages are
/// not kept.
pub const MAX_CONVERSATION: usize = 1024 * 1024;

/// What each message is counted as besides its text: its own memory, and the time a search
/// takes to look at it however short it is. So a conversation holds at most
/// [`MAX_CONVERSATION`] / 256 = 4096 messages.
pub const MESSAGE_COST: usize = 256;

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

/// What a conversation is, apart from its messages: what a row and `RET` need. `Debug` gives
/// its id only.
#[derive(Clone, PartialEq, Eq)]
pub struct About {
    pub id: SessionId,
    /// Where it resumes: the first directory it ran in whose name, spelled as Claude Code
    /// spells a project's directory, is the name of the directory its transcript is in. Claude
    /// Code finds a transcript to resume from there, and a line naming another directory is not
    /// believed for a resume.
    pub directory: Option<PathBuf>,
    /// Every directory it ran in, the first first, at most [`MAX_DIRECTORIES_RUN`].
    pub directories: Vec<PathBuf>,
    /// Its best-ranked title, if a title line named one.
    pub title: Option<String>,
    /// The first line of its first prompt, if it has one.
    pub first_prompt: Option<String>,
    pub changed: SystemTime,
    /// The directory it names is no longer a directory.
    pub gone: bool,
}

impl std::fmt::Debug for About {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("About")
            .field("id", &self.id.as_str())
            .finish_non_exhaustive()
    }
}

impl About {
    /// The directory a row names: where it resumes, else the first it ran in.
    pub fn shown_directory(&self) -> Option<&Path> {
        self.directory
            .as_deref()
            .or_else(|| self.directories.first().map(PathBuf::as_path))
    }
}

/// One prompt or reply. `Debug` says nothing of it.
#[derive(Clone, PartialEq, Eq)]
pub struct Message {
    pub prompt: bool,
    pub text: Box<str>,
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Message").finish_non_exhaustive()
    }
}

/// A conversation held: what it is and what was said, or only what its file was, while more
/// than a search holds was held before it. `Debug` gives counts only.
pub struct Conversation {
    pub about: Arc<About>,
    pub messages: Vec<Message>,
    /// What its messages and directories are counted as, by [`MESSAGE_COST`].
    pub bytes: usize,
    /// More was said than it keeps.
    pub cut: bool,
    /// Its messages are held: not a stub left out of the hold.
    pub held: bool,
    /// Each title's rank's latest.
    titles: [Option<String>; scan::TITLES.len()],
    /// What its file was when last read.
    file: FileState,
    /// The transcript's path.
    pub path: PathBuf,
}

impl std::fmt::Debug for Conversation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conversation")
            .field("messages", &self.messages.len())
            .field("bytes", &self.bytes)
            .field("held", &self.held)
            .finish_non_exhaustive()
    }
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
    // One conversation an id: two transcripts of one id would give a row of one and act on the
    // other. The newest stands.
    let mut seen = std::collections::HashSet::new();
    found.retain(|listed| seen.insert(listed.id.clone()));
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
            held: true,
            about: Arc::new(About {
                id,
                directory: None,
                directories: Vec::new(),
                title: None,
                first_prompt: None,
                changed: SystemTime::UNIX_EPOCH,
                gone: false,
            }),
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

    /// Leaves out its messages, keeping what its file was, so that it is not read again
    /// until it changes.
    pub fn leave_out(&mut self) {
        self.messages = Vec::new();
        self.bytes = 0;
        self.held = false;
    }

    /// Takes in one line's reading.
    fn take(&mut self, line: scan::Line) {
        if let Some(cwd) = line.cwd {
            self.take_directory(cwd);
        }
        if let Some((rank, title)) = line.title {
            self.titles[rank] = Some(title);
            let best = self.titles.iter().flatten().next().cloned();
            Arc::make_mut(&mut self.about).title = best;
        }
        let Some((prompt, texts)) = line.message else {
            return;
        };
        for text in texts {
            if prompt && self.about.first_prompt.is_none() {
                let first = text.trim().lines().next().unwrap_or_default();
                Arc::make_mut(&mut self.about).first_prompt = Some(cut(first, 200).to_owned());
            }
            if self.cut {
                continue;
            }
            let text = cut(&text, MAX_MESSAGE);
            let cost = text.len() + MESSAGE_COST;
            if self.bytes + cost > MAX_CONVERSATION {
                self.cut = true;
                continue;
            }
            self.bytes += cost;
            self.messages.push(Message {
                prompt,
                text: text.into(),
            });
        }
    }

    /// Takes in a directory a line says it ran in: counted, at most [`MAX_DIRECTORIES_RUN`],
    /// and where it resumes if its transcript is in that directory's project directory.
    fn take_directory(&mut self, cwd: PathBuf) {
        if self.about.directories.len() == MAX_DIRECTORIES_RUN
            || self.about.directories.contains(&cwd)
        {
            return;
        }
        let slug = self
            .path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| *name == *slug_of(&cwd));
        self.bytes += cwd.as_os_str().len();
        let about = Arc::make_mut(&mut self.about);
        if slug && about.directory.is_none() {
            about.directory = Some(cwd.clone());
        }
        about.directories.push(cwd);
    }
}

/// The name Claude Code gives the project directory of transcripts that ran in `directory`:
/// its path with each character but an ASCII letter or digit as `-`.
pub fn slug_of(directory: &Path) -> String {
    directory
        .to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
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
            if held.held
                && held.file.identity == now.identity
                && held.file.len <= now.len
                && held.about.id == listed.id =>
        {
            held
        }
        _ => Conversation::new(listed.id.clone(), listed.path.clone()),
    };
    let from = conversation.file.read_to;
    let take = |line| {
        conversation.take(line);
        true
    };
    let read_to = read_lines(&mut file, from, now.len, take).unwrap_or(from);
    conversation.file = FileState { read_to, ..now };
    let about = Arc::make_mut(&mut conversation.about);
    about.changed = now.changed;
    about.gone = about
        .shown_directory()
        .is_some_and(|directory| !directory.is_dir());
    Some(conversation)
}

/// Reads the whole lines of `file` from `from` up to `len` into `conversation`, and gives the
/// offset past the last whole line.
fn read_lines(
    file: &mut File,
    from: u64,
    len: u64,
    mut take: impl FnMut(scan::Line) -> bool,
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
        if !skipped && !line.is_empty() && !take(scan::read(&line)) {
            return Ok(at);
        }
    }
}

/// The most bytes the read-only view of a conversation keeps (ADR 0145, step 2), its text and
/// what the view adds to it counted: [`MESSAGE_COST`] a message and [`VIEW_LINE_COST`] a line.
pub const MAX_VIEW: usize = 8 * 1024 * 1024;

/// What each line of a message is counted as in the view, besides its text: its indent, and
/// the rope's and display model's cost of a line. So a message of line breaks only cannot make
/// a view of millions of lines.
pub const VIEW_LINE_COST: usize = 16;

/// A conversation read whole for its read-only view: its prompts and replies, and whether more
/// was said than [`MAX_VIEW`] keeps. `Debug` gives counts only.
pub struct Whole {
    pub messages: Vec<Message>,
    pub cut: bool,
}

impl std::fmt::Debug for Whole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Whole")
            .field("messages", &self.messages.len())
            .field("cut", &self.cut)
            .finish()
    }
}

/// Reads the transcript at `path` whole, as [`read`] reads one, for its read-only view: every
/// prompt and reply, each whole, until [`MAX_VIEW`] bytes, counted as it says; the reading
/// stops there. None if it cannot be read as a regular file of this user's with one name.
pub fn read_whole(path: &Path) -> Option<Whole> {
    let (mut file, meta) = file_meta::open_regular_here(path).ok().flatten()?;
    if !is_this_users(&meta) || !has_one_name(&meta) {
        return None;
    }
    let mut whole = Whole {
        messages: Vec::new(),
        cut: false,
    };
    let mut bytes = 0;
    let take = |line: scan::Line| {
        let Some((prompt, texts)) = line.message else {
            return true;
        };
        for text in texts {
            let lines = text.bytes().filter(|&byte| byte == b'\n').count() + 1;
            let cost = text.len() + MESSAGE_COST + lines * VIEW_LINE_COST;
            if bytes + cost > MAX_VIEW {
                whole.cut = true;
                return false;
            }
            bytes += cost;
            whole.messages.push(Message {
                prompt,
                text: text.into(),
            });
        }
        true
    };
    read_lines(&mut file, 0, meta.len(), take).ok()?;
    Some(whole)
}

/// Reads one line of `reader` into `line`, without its line break, keeping at most
/// [`MAX_LINE`] bytes of it. Gives whether it ended with a line break, how many bytes were
/// taken, and whether it was too long and is skipped.
fn next_line(reader: &mut impl io::BufRead, line: &mut Vec<u8>) -> io::Result<(bool, u64, bool)> {
    next_line_within(reader, line, MAX_LINE)
}

/// [`next_line`] keeping at most `max` bytes of a line: what a native session's earlier turns
/// read (ADR 0165, step 5).
pub(crate) fn next_line_within(
    reader: &mut impl io::BufRead,
    line: &mut Vec<u8>,
    max: usize,
) -> io::Result<(bool, u64, bool)> {
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
            if line.len() + part.len() > max {
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

    /// What makes a test's `/w/...` an absolute path here: a drive on Windows.
    const ROOT: &str = if cfg!(windows) { "C:" } else { "" };

    /// A user line that says it ran in `ROOT` and `cwd`.
    fn ran_in(cwd: &str, text: &str) -> String {
        format!(r#"{{"type":"user","cwd":"{ROOT}{cwd}","message":{{"content":"{text}"}}}}"#)
    }

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
        held.take(scan::read(ran_in("/w/a", "one\\ntwo").as_bytes()));
        held.take(scan::read(
            ran_in("/w/a/.claude/worktrees/b", "three").as_bytes(),
        ));
        assert_eq!(
            held.about.directory, None,
            "not where /c/p holds transcripts"
        );
        let first = format!("{ROOT}/w/a");
        assert_eq!(held.about.shown_directory(), Some(Path::new(&first)));
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

    #[test]
    fn many_short_messages_are_counted_by_what_each_costs() {
        let mut held = conversation();
        let parts: Vec<String> = (0..10_000)
            .map(|_| r#"{"type":"text","text":"x"}"#.to_owned())
            .collect();
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[{}]}}}}"#,
            parts.join(",")
        );
        held.take(scan::read(line.as_bytes()));
        assert!(held.cut);
        assert!(held.messages.len() <= MAX_CONVERSATION / MESSAGE_COST);
    }

    #[test]
    fn a_conversation_resumes_only_where_its_transcript_says() {
        let id = SessionId::parse("6c1b5d1e-2f3a-4b5c-8d9e-0f1a2b3c4d5e").unwrap();
        let resumes = format!("{ROOT}/w/oso/.x");
        let slug = slug_of(Path::new(&resumes));
        let mut held = Conversation::new(id, PathBuf::from(format!("/c/{slug}/x.jsonl")));
        held.take(scan::read(ran_in("/elsewhere/oso", "a").as_bytes()));
        held.take(scan::read(ran_in("/w/oso/.x", "b").as_bytes()));
        assert_eq!(held.about.directory, Some(PathBuf::from(&resumes)));
        assert_eq!(held.about.shown_directory(), Some(Path::new(&resumes)));
        assert_eq!(held.about.directories.len(), 2);
        assert_eq!(
            slug_of(Path::new("/Users/me/a b/.claude")),
            "-Users-me-a-b--claude"
        );
        let debug = format!("{held:?} {:?}", held.about);
        assert!(!debug.contains("oso"), "{debug}");
    }
}
