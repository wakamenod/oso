//! The agent channel's `diagnostics` (ADR 0138, What agents may do, step 11): the language
//! servers' diagnostics of a buffer, or of its whole project, as the editor holds them, read like
//! `text`. Also the order `M-g d`'s list shares ([`ordered`]) and the escaping of a server's
//! text that a send to a session shares ([`server_text`]).
//!
//! Must not depend on: sockets or `app`; the caller hands over the servers' reports of files no
//! buffer holds and a reply channel. The UI thread checks the buffer and takes a snapshot, its
//! sets cloned and its text and the reports shared, and a thread of its own makes the answer,
//! as `text` copies its rows (ADR 0085), however many diagnostics the servers keep.
//!
//! - A buffer's own come first, by place, then each other file's by its path from the root and
//!   by line, open in a buffer or not, from every server, as `M-g d` lists them; hints are left
//!   out, as the counts leave them out. At most [`MAX_ENTRIES`], and at most [`ANSWER_BYTES`] of
//!   their text, so that the answer fits a reply line, with `more` saying how many were left out.
//! - An entry gives the path, the line and the column from 1, the severity, the message, the
//!   source and the code, and the server that reported it. An open buffer's column is in
//!   characters; a file no buffer holds gives the server's own, with `column_unit` saying how it
//!   counts, as turning it into characters would read the file.
//! - A server's text, its message, source and code, and the path of a file it reported, may be
//!   written to steer the agent: a message is at most [`MESSAGE_BYTES`], cut at a character, its
//!   line breaks kept, and every other control, bidi or invisible character is written as
//!   `\u{…}`, as a sent path's are (ADR 0121).
//! - Never of a buffer that may hold secrets, or one withheld from agents or still opening, as
//!   `text` refuses them; such buffers, and the servers' reports of their files, are left out of
//!   a project's too. The servers' reports of files whose names may hold secrets were dropped
//!   before they were kept (step 6a).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;

use oso_core::buffer::Buffer;
use oso_core::diagnostic::{Diagnostics, Severity};
use oso_core::display_map::cell::is_invisible;
use oso_core::editor::Editor;
use oso_core::rope::Rope;
use oso_lsp::position::Encoding;
use oso_lsp::protocol::Published;
use serde_json::{Value, json};

use super::protocol::DiagnosticsOf;
use super::server::Reply;
use super::state::unreadable;

/// The most entries one call gives (ADR 0138).
pub const MAX_ENTRIES: usize = 500;

/// The most bytes of a server's message an entry or a send gives (ADR 0138).
pub const MESSAGE_BYTES: usize = 2 << 10;

/// The most bytes of entries' text one answer holds, counted as it is written out: well within
/// a reply line's 1 MiB, its JSON's quotes and escapes included.
pub const ANSWER_BYTES: usize = 512 << 10;

/// The longest path from the root a file's diagnostics are given for, as long as a walk's
/// longest path: a server may name any path.
pub const PATH_BYTES_MAX: usize = 4096;

/// Why a buffer that may hold secrets gives none, or one withheld from agents.
const WITHHELD: &str = "the buffer may hold secrets, so its diagnostics stay in the editor";
const PERSONS: &str = "only the person reads that buffer, so its diagnostics stay in the editor";

/// A server's report of a file no buffer holds, shared with the server's thread, with the
/// encoding of its positions and the server's name.
pub struct FileReport {
    pub path: Arc<Path>,
    pub report: Arc<Published>,
    pub encoding: Encoding,
    pub server: Arc<str>,
}

/// A source of diagnostics, as `M-g d`'s list and the channel's answer order them: an open
/// buffer's set, of whatever kind the caller keeps, or a file's report.
pub enum Ordered<'a, O> {
    Open(&'a O),
    File(&'a FileReport),
}

/// The sources of the project at `root`, in the order both give them, each with its path from
/// the root: `current` first, then `others` and `files` by those paths. A path that is not
/// UTF-8, or is longer than [`PATH_BYTES_MAX`], gives none, as does a file `skip` says to leave
/// out. The files' paths are judged against the root as a URI reads back, as the servers'
/// threads judged them, so that on Windows a drive letter in either case and a `\\?\` root
/// match.
pub fn ordered<'a, O>(
    root: &Path,
    (current, others): (Option<&'a O>, impl IntoIterator<Item = &'a O>),
    files: &'a [FileReport],
    path_of: impl Fn(&O) -> &Path,
    skip: &dyn Fn(&Path) -> bool,
) -> Vec<(Box<str>, Ordered<'a, O>)> {
    let relative = |path: &Path, root: &Path| -> Option<Box<str>> {
        let relative = path.strip_prefix(root).ok()?.to_str()?;
        (relative.len() <= PATH_BYTES_MAX).then(|| relative.into())
    };
    let server_root = oso_lsp::uri::from_path(root)
        .and_then(|uri| oso_lsp::uri::to_path(&uri))
        .unwrap_or_else(|| root.to_path_buf());
    let first =
        current.and_then(|open| Some((relative(path_of(open), root)?, Ordered::Open(open))));
    let mut rest: Vec<(Box<str>, Ordered<'a, O>)> = Vec::new();
    for open in others {
        if let Some(path) = relative(path_of(open), root) {
            rest.push((path, Ordered::Open(open)));
        }
    }
    for file in files.iter().filter(|file| !skip(&file.path)) {
        if let Some(path) = relative(&file.path, &server_root) {
            rest.push((path, Ordered::File(file)));
        }
    }
    rest.sort_by(|(a, _), (b, _)| a.cmp(b));
    first.into_iter().chain(rest).collect()
}

/// An open buffer's set, with its text and path, as the call found them.
struct OpenSet {
    path: PathBuf,
    shown: Box<str>,
    text: Rope,
    set: Diagnostics,
}

/// What a call reads, taken on the UI thread, which the job's thread answers from.
pub struct DiagnosticsJob {
    buffer: u64,
    root: Option<PathBuf>,
    own: Option<OpenSet>,
    open: Vec<OpenSet>,
    files: Vec<FileReport>,
    /// The paths, as a URI reads back, of the open buffers the channel may not read, whose
    /// files' reports it leaves out.
    unread: HashSet<PathBuf>,
}

/// Checks what `of` asks for and takes a snapshot of it: the buffer's set, and with `project`
/// the project's other open buffers' and what `files` gives of the reports of the project at a
/// root. O(the open buffers' entries), with no answer made here.
///
/// # Errors
///
/// A buffer that is not there, may hold secrets or is withheld from agents, and a project's
/// for a buffer in no project.
pub fn start(
    editor: &Editor,
    files: &dyn Fn(&Path) -> Vec<FileReport>,
    of: DiagnosticsOf,
) -> Result<DiagnosticsJob, String> {
    let (id, buffer) = match of.buffer {
        Some(id) => editor
            .buffers()
            .find(|(own, buffer)| own.get() == id && !buffer.agent_pending())
            .ok_or_else(|| format!("no buffer {id}"))?,
        None => {
            let (window, buffer) = editor.tiled();
            (window.buffer(), buffer)
        }
    };
    refuse(buffer)?;
    let mut job = DiagnosticsJob {
        buffer: id.get(),
        root: None,
        own: open_set(buffer),
        open: Vec::new(),
        files: Vec::new(),
        unread: HashSet::new(),
    };
    if of.project {
        let root = buffer
            .project()
            .and_then(|project| editor.project(project))
            .map(|project| project.root().to_path_buf())
            .ok_or("the buffer is in no project")?;
        for (other, buffer) in editor.buffers() {
            let Some(path) = buffer.path().filter(|path| path.starts_with(&root)) else {
                continue;
            };
            if other == id {
                continue;
            }
            if unreadable(buffer) || buffer.agent_pending() {
                // Its file's reports too, as its server's thread spells the path.
                let read_back = oso_lsp::uri::from_path(path)
                    .and_then(|uri| oso_lsp::uri::to_path(&uri))
                    .unwrap_or_else(|| path.to_path_buf());
                job.unread.insert(read_back);
            } else if let Some(set) = open_set(buffer) {
                job.open.push(set);
            }
        }
        job.files = files(&root);
        job.root = Some(root);
    }
    Ok(job)
}

/// Refuses a buffer whose diagnostics the channel may not read.
fn refuse(buffer: &Buffer) -> Result<(), String> {
    if buffer.is_secret() {
        Err(WITHHELD.into())
    } else if unreadable(buffer) {
        Err(PERSONS.into())
    } else {
        Ok(())
    }
}

/// `buffer`'s set, its text and its path, shared or cloned: at most 2000 small entries.
fn open_set(buffer: &Buffer) -> Option<OpenSet> {
    let set = buffer.diagnostics()?.clone();
    let path = buffer
        .path()
        .map_or_else(|| PathBuf::from(buffer.name()), Path::to_path_buf);
    Some(OpenSet {
        shown: server_text(&path.to_string_lossy(), false).into(),
        path,
        text: buffer.text().clone(),
        set,
    })
}

impl DiagnosticsJob {
    /// Makes the answer on a thread of its own and sends it to `reply`.
    pub fn spawn(self, reply: mpsc::Sender<Reply>) {
        let spawned = thread::Builder::new()
            .name("oso-agent-diagnostics".into())
            .spawn(move || {
                let _ = reply.send(Ok(self.run()));
            });
        if let Err(e) = spawned {
            log::warn!("agent channel: cannot start the diagnostics thread: {e}");
        }
    }

    /// The answer: the buffer's own entries, then the rest by path, within the bounds.
    pub(crate) fn run(&self) -> Value {
        let mut answer = Answer::default();
        if let Some(root) = &self.root {
            let skip = |path: &Path| self.unread.contains(path);
            let sources = ordered(
                root,
                (self.own.as_ref(), &self.open),
                &self.files,
                |open: &OpenSet| &open.path,
                &skip,
            );
            for (_, source) in &sources {
                match source {
                    Ordered::Open(open) => answer.open(open),
                    Ordered::File(file) => answer.file(file),
                }
            }
        } else if let Some(own) = &self.own {
            answer.open(own);
        }
        json!({
            "buffer": self.buffer,
            "project": self.root.as_ref().map(|root| root.to_string_lossy().into_owned()),
            "more": answer.total - answer.entries.len(),
            "entries": answer.entries,
        })
    }
}

/// The entries made, the bytes of their text, and how many there are in all.
#[derive(Default)]
struct Answer {
    entries: Vec<Value>,
    bytes: usize,
    total: usize,
    full: bool,
}

impl Answer {
    /// Whether one more entry of `bytes` fits; past a bound, the answer is full.
    fn room(&mut self, bytes: usize) -> bool {
        if self.full || self.entries.len() == MAX_ENTRIES || self.bytes + bytes > ANSWER_BYTES {
            self.full = true;
            return false;
        }
        self.bytes += bytes;
        true
    }

    /// An open buffer's entries, by place: once full, only counted, in O(1).
    fn open(&mut self, open: &OpenSet) {
        let counts = open.set.counts();
        self.total += [counts.errors, counts.warnings, counts.information]
            .into_iter()
            .map(|count| usize::try_from(count).unwrap_or(usize::MAX))
            .sum::<usize>();
        if self.full {
            return;
        }
        let path = Value::from(&*open.shown);
        for diagnostic in open.set.iter().filter(|d| d.severity() != Severity::Hint) {
            let (message, source, code) = (
                server_text(diagnostic.message(), true),
                server_text(diagnostic.source(), false),
                server_text(diagnostic.code(), false),
            );
            if !self.room(open.shown.len() + message.len() + source.len() + code.len()) {
                return;
            }
            let start = diagnostic.range().start;
            let point = open.text.offset_to_point(start);
            // O(log n): a slice shares the rope's chunks, however long the line.
            let line_start = open.text.line_start(point.row);
            let column = open.text.slice(line_start..start).char_count();
            self.entries.push(json!({
                "path": path,
                "line": point.row + 1,
                "column": column + 1,
                "severity": diagnostic.severity().word(),
                "message": message,
                "source": source,
                "code": code,
                "server": open.set.server(),
            }));
        }
    }

    /// A file's entries, by line and column: once full, only counted.
    fn file(&mut self, file: &FileReport) {
        let kept = || (file.report.diagnostics.iter()).filter(|r| r.severity != Severity::Hint);
        if self.full {
            self.total += kept().count();
            return;
        }
        let mut kept: Vec<_> = kept().collect();
        self.total += kept.len();
        kept.sort_unstable_by_key(|reported| (reported.start.line, reported.start.character));
        // The path is the server's text too.
        let shown = server_text(&file.path.to_string_lossy(), false);
        let path = Value::from(shown.as_str());
        let unit = match file.encoding {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf16 => "utf-16",
        };
        for reported in kept {
            let (message, source, code) = (
                server_text(&reported.message, true),
                server_text(&reported.source, false),
                server_text(&reported.code, false),
            );
            if !self.room(shown.len() + message.len() + source.len() + code.len()) {
                return;
            }
            self.entries.push(json!({
                "path": path,
                "line": reported.start.line as usize + 1,
                "column": reported.start.character as usize + 1,
                "column_unit": unit,
                "severity": reported.severity.word(),
                "message": message,
                "source": source,
                "code": code,
                "server": &*file.server,
            }));
        }
    }
}

/// A server's text as an agent or a session gets it: at most [`MESSAGE_BYTES`] of it, cut at a
/// character, and every control, bidi or invisible character written as `\u{…}`, but line
/// breaks if `keep_breaks`.
pub fn server_text(text: &str, keep_breaks: bool) -> String {
    let kept = &text[..text.floor_char_boundary(MESSAGE_BYTES)];
    let mut out = String::with_capacity(kept.len());
    for c in kept.chars() {
        if (c.is_control() && !(keep_breaks && c == '\n')) || is_invisible(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests;
