//! One file's section of a review buffer (ADR 0122, "The views"): its header, its notes and its
//! hunks as text, the rows that say what each line is, and the highlights each line takes.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; this lays out a reading it is
//! given, on the review's view thread.
//!
//! - Every line ends in `\n`, which this writes itself: a file's own line ending, `\r\n` or
//!   none, is taken off and said in a note, so that a row is always one buffer line.
//! - A path's newline is shown as `␤`, so that no path can make a line of its own. Its other
//!   bytes, and a line's, go in as they are: invalid UTF-8 shows as `\xNN` and controls as the
//!   display map shows them.
//! - What a line is goes in its [`Section`], never read back from its text.
//! - Highlights are counted in the decoded text's bytes, where each invalid byte is a U+FFFD
//!   of three, as the loader decodes it.

use std::fmt::Write as _;
use std::sync::Arc;

use oso_core::review::{Hunk, RowKind, Section, Side};
use oso_core::syntax::Highlight;

use crate::review::list::Status;
use crate::review::{Content, Entry, Reading};

/// What a newline in a path shows as.
const PATH_NEWLINE: &str = "\u{2424}";

/// A file's section: its text, its rows, and its highlights as runs of the decoded text.
pub(crate) struct Rendered {
    pub(crate) bytes: Vec<u8>,
    pub(crate) section: Arc<Section>,
    pub(crate) runs: Vec<(usize, Option<Highlight>)>,
    /// Whether its bytes are all UTF-8, so that its text holds no invalid byte.
    pub(crate) valid: bool,
}

/// Builds a section line by line, keeping its rows and runs with its text.
struct Writer {
    bytes: Vec<u8>,
    section: Section,
    runs: Vec<(usize, Option<Highlight>)>,
    /// The kind of the lines written last and not in the section yet, and how many: a run of
    /// lines of one kind is one run of rows, so that each line's offset counts in it.
    pending: Option<(RowKind, usize)>,
}

impl Writer {
    fn new(path: &[u8]) -> Writer {
        Writer {
            bytes: Vec::new(),
            section: Section::new(path.into()),
            runs: Vec::new(),
            pending: None,
        }
    }

    fn done(mut self) -> Rendered {
        if let Some((kind, lines)) = self.pending.take() {
            self.section.push(kind, lines);
        }
        let valid = std::str::from_utf8(&self.bytes).is_ok();
        Rendered {
            bytes: self.bytes,
            section: Arc::new(self.section),
            runs: self.runs,
            valid,
        }
    }

    /// Writes one line, made of `parts` each with its highlight, as a row of `kind`.
    fn line(&mut self, kind: RowKind, parts: &[(&[u8], Option<Highlight>)]) {
        for (bytes, highlight) in parts {
            self.bytes.extend_from_slice(bytes);
            self.run(decoded_len(bytes), *highlight);
        }
        self.bytes.push(b'\n');
        self.run(1, None);
        match &mut self.pending {
            Some((pending, lines)) if *pending == kind => *lines += 1,
            pending => {
                if let Some((kind, lines)) = pending.replace((kind, 1)) {
                    self.section.push(kind, lines);
                }
            }
        }
    }

    fn run(&mut self, len: usize, highlight: Option<Highlight>) {
        if len == 0 {
            return;
        }
        match self.runs.last_mut() {
            Some((last, kept)) if *kept == highlight => *last += len,
            _ => self.runs.push((len, highlight)),
        }
    }

    /// A note: one line, whatever its text holds, as a reason may come from git.
    fn note(&mut self, text: &str) {
        let text = text.replace(['\n', '\r'], " ");
        self.line(
            RowKind::Note,
            &[(text.as_bytes(), Some(Highlight::Outdated))],
        );
    }
}

/// How many bytes `bytes` take once decoded: each byte of an invalid sequence becomes a
/// U+FFFD, as the loader's decoder makes it.
pub(crate) fn decoded_len(mut bytes: &[u8]) -> usize {
    let mut len = 0;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(text) => return len + text.len(),
            Err(error) => {
                let valid = error.valid_up_to();
                let invalid = error.error_len().unwrap_or(bytes.len() - valid);
                len += valid + invalid * char::REPLACEMENT_CHARACTER.len_utf8();
                bytes = &bytes[valid + invalid..];
            }
        }
    }
}

/// A path as a header shows it: a newline as `␤`, every other byte as it is.
fn shown_path(path: &[u8], out: &mut Vec<u8>) {
    for &byte in path {
        if byte == b'\n' {
            out.extend_from_slice(PATH_NEWLINE.as_bytes());
        } else {
            out.push(byte);
        }
    }
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Added => "added",
        Status::Copied => "copied",
        Status::Deleted => "deleted",
        Status::Modified => "modified",
        Status::Renamed => "renamed",
        Status::TypeChanged => "type changed",
        Status::Unmerged => "conflicted",
        Status::Unknown => "changed",
    }
}

/// A size as a note says it.
fn size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    match bytes {
        b if b < KIB => format!("{b} B"),
        b if b < KIB * KIB => format!("{:.1} KiB", b as f64 / KIB as f64),
        b => format!("{:.1} MiB", b as f64 / (KIB * KIB) as f64),
    }
}

/// The file's header line: its status, its path (and the one it came from), and its counts.
fn header(writer: &mut Writer, entry: &Entry, counts: Option<(usize, usize)>) {
    let mut text = Vec::new();
    text.extend_from_slice(status_word(entry.status).as_bytes());
    text.extend_from_slice(b"  ");
    if let Some(old) = &entry.old_path {
        shown_path(old, &mut text);
        text.extend_from_slice(" → ".as_bytes());
    }
    shown_path(&entry.path, &mut text);
    if let Some((added, removed)) = counts {
        let _ = write!(Bytes(&mut text), "  +{added} \u{2212}{removed}");
    }
    let highlight = Some(Highlight::DiffFileHeader);
    writer.line(RowKind::File, &[(&text, highlight)]);
}

/// Lets `write!` append to a byte vector.
struct Bytes<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for Bytes<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}

/// A line's text without its ending, and the ending it had.
fn without_ending(line: &[u8]) -> (&[u8], Ending) {
    match line {
        [rest @ .., b'\r', b'\n'] => (rest, Ending::CrLf),
        [rest @ .., b'\n'] => (rest, Ending::Lf),
        rest => (rest, Ending::None),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Lf,
    CrLf,
    None,
}

/// What the endings of a side's changed lines were.
#[derive(Default)]
struct Endings {
    crlf: bool,
    none: bool,
}

impl Endings {
    fn add(&mut self, ending: Ending) {
        match ending {
            Ending::Lf => {}
            Ending::CrLf => self.crlf = true,
            Ending::None => self.none = true,
        }
    }
}

/// The notes on line endings a text reading needs: a side whose changed lines end in CR LF
/// where the other side's do not, and a side whose last line has no newline.
fn ending_notes(writer: &mut Writer, hunks: &[Hunk]) {
    let (mut old, mut new) = (Endings::default(), Endings::default());
    for hunk in hunks {
        for line in &hunk.removed {
            old.add(without_ending(line).1);
        }
        for line in &hunk.added {
            new.add(without_ending(line).1);
        }
    }
    if old.crlf != new.crlf {
        let side = if old.crlf { "old" } else { "new" };
        writer.note(&format!("line endings differ: CR LF on the {side} side"));
    }
    if old.none {
        writer.note("no newline at the end of the old side");
    }
    if new.none {
        writer.note("no newline at the end of the new side");
    }
}

/// A hunk's header, as git writes one with no context.
fn hunk_header(hunk: &Hunk) -> String {
    let side = |start: u32, count: usize| {
        if count == 0 {
            format!("{},0", start.saturating_sub(1))
        } else if count == 1 {
            format!("{start}")
        } else {
            format!("{start},{count}")
        }
    };
    format!(
        "@@ -{} +{} @@",
        side(hunk.old_start, hunk.removed.len()),
        side(hunk.new_start, hunk.added.len())
    )
}

fn lines(writer: &mut Writer, index: usize, hunk: &Hunk, side: Side) {
    let (lines, first, sign, highlight) = match side {
        Side::Old => (&hunk.removed, hunk.old_start, b"-", Highlight::DiffRemoved),
        Side::New => (&hunk.added, hunk.new_start, b"+", Highlight::DiffAdded),
    };
    let kind = RowKind::Lines {
        hunk: index,
        side,
        first,
    };
    for line in lines {
        let (text, _) = without_ending(line);
        writer.line(kind, &[(sign, Some(highlight)), (text, Some(highlight))]);
    }
}

/// The section of a file listed and not read yet: its header and a note.
pub(crate) fn unread(entry: &Entry) -> Rendered {
    let mut writer = Writer::new(&entry.path);
    header(&mut writer, entry, None);
    writer.note("reading\u{2026}");
    writer.done()
}

/// The section of `reading`; `whole` false shows its header alone, as when the review's text
/// is past its limit.
pub(crate) fn render(reading: &Reading, whole: bool) -> Rendered {
    let entry = &reading.entry;
    let mut writer = Writer::new(&entry.path);
    header(&mut writer, entry, reading.counts());
    if !whole {
        writer.note("not shown: the review's text is past its limit");
        return writer.done();
    }
    match &reading.content {
        Content::Text { shape, .. } => {
            ending_notes(&mut writer, shape.hunks());
            for (index, hunk) in shape.hunks().iter().enumerate() {
                let text = hunk_header(hunk);
                let highlight = Some(Highlight::DiffHunkHeader);
                writer.line(
                    RowKind::Hunk { hunk: index },
                    &[(text.as_bytes(), highlight)],
                );
                lines(&mut writer, index, hunk, Side::Old);
                lines(&mut writer, index, hunk, Side::New);
            }
        }
        Content::Binary { old, new } => {
            writer.note(&format!("binary: {} \u{2192} {}", size(*old), size(*new)));
        }
        Content::TooLarge { old, new } => writer.note(&format!(
            "too large to show: {} \u{2192} {}",
            size(*old),
            size(*new)
        )),
        Content::OutOfTime => writer.note("not read in time: g reads it again"),
        Content::Withheld => writer.note("may hold secrets: not shown"),
        Content::Conflicted => writer.note("conflicted: not shown"),
        Content::Submodule => writer.note(&format!(
            "submodule: {} \u{2192} {}",
            short(entry.old_id.as_str()),
            short(entry.new_id.as_str())
        )),
        Content::Unreadable(why) => writer.note(&format!("cannot be read: {why}")),
    }
    writer.done()
}

/// An object id cut to its first 12 digits, or `none` for no object.
fn short(id: &str) -> &str {
    if id.bytes().all(|digit| digit == b'0') {
        "none"
    } else {
        id.get(..12).unwrap_or(id)
    }
}

/// The review's own section, first in its buffer: its title and what was said of it, each a
/// note.
pub(crate) fn title(title: &str, said: &[String]) -> Rendered {
    let mut writer = Writer::new(b"");
    let title = title.replace(['\n', '\r'], " ");
    let highlight = Some(Highlight::DiffFileHeader);
    writer.line(RowKind::Note, &[(title.as_bytes(), highlight)]);
    for line in said {
        writer.note(line);
    }
    writer.done()
}

#[cfg(test)]
#[path = "section_tests.rs"]
pub(super) mod tests;
