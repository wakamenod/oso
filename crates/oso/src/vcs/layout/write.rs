//! Writing a status's text (ADR 0127): its lines, their highlights, the outline of its
//! headings and the table of what each line is about, from the sections laid out in order.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, git or the file system; [`lay_out`] is a
//! pure function of what was read.

use std::collections::HashMap;
use std::sync::Arc;

use oso_core::editor::GitHead;
use oso_core::load::{Builder, LoadSnapshot};
use oso_core::syntax::{Highlight, Highlights};

use super::{Built, File, Head, Hunks, MAX_FILE_TEXT, MAX_FILES, MAX_TEXT, Row, Section, View};
use crate::dired::rows::{decoded_len, push_name};

/// A status's text, highlights, outline and table.
pub(crate) struct Laid {
    pub(crate) text: LoadSnapshot,
    pub(crate) highlights: Highlights,
    pub(crate) outline: Vec<GitHead>,
    pub(crate) built: Arc<Built>,
}

/// Lays out the status: `head`, `notes` under it, then the `sections`, folded and opened as
/// `view` says, each open file's hunks from `hunks`.
pub(crate) fn lay_out(
    head: &Head,
    notes: &[String],
    sections: Vec<(Section, Vec<File>)>,
    hunks: &HashMap<(Section, Vec<u8>), Hunks>,
    view: &View,
) -> Laid {
    let mut out = Out::default();
    let mut built = Built::default();
    let mut outline = Vec::new();
    head_lines(&mut out, &mut built.rows, head);
    for note in notes {
        out.part(note.as_bytes(), None);
        out.end_line();
        built.rows.push(Row::Other);
    }
    let mut cut = false;
    for (section, files) in &sections {
        let mut shown = Shown {
            out: &mut out,
            built: &mut built,
            outline: &mut outline,
        };
        cut |= shown.section(*section, files, hunks, view);
    }
    if cut {
        out.end_line();
        built.rows.push(Row::Other);
        out.part(
            b"Some files' hunks are not shown: the status holds as much text as oso shows",
            None,
        );
        out.end_line();
        built.rows.push(Row::Other);
    }
    built.sections = sections;
    let (text, highlights) = out.finish();
    Laid {
        text,
        highlights,
        outline,
        built: Arc::new(built),
    }
}

/// What a layout writes to.
struct Shown<'a> {
    out: &'a mut Out,
    built: &'a mut Built,
    outline: &'a mut Vec<GitHead>,
}

impl Shown<'_> {
    /// Lays out `section`'s heading and files; whether some file's hunks were left out for the
    /// text's size.
    fn section(
        &mut self,
        section: Section,
        files: &[File],
        hunks: &HashMap<(Section, Vec<u8>), Hunks>,
        view: &View,
    ) -> bool {
        let Shown {
            out,
            built,
            outline,
        } = self;
        let mut cut = false;
        if files.is_empty() {
            return cut;
        }
        out.end_line();
        built.rows.push(Row::Other);
        outline.push(GitHead {
            line: built.rows.len(),
            depth: 0,
        });
        built.headings.insert(section, built.rows.len());
        out.part(
            section.heading().as_bytes(),
            Some(Highlight::GitSectionHeading),
        );
        out.part(
            format!(" ({})", files.len()).as_bytes(),
            Some(Highlight::GitCount),
        );
        if view.folded.contains(&section) {
            out.part("…".as_bytes(), Some(Highlight::GitCount));
        }
        out.end_line();
        built.rows.push(Row::Heading(section));
        if view.folded.contains(&section) {
            return cut;
        }
        for (index, file) in files.iter().enumerate().take(MAX_FILES) {
            outline.push(GitHead {
                line: built.rows.len(),
                depth: 1,
            });
            built
                .file_rows
                .insert((section, file.path.clone()), (index, built.rows.len()));
            file_line(out, file);
            built.rows.push(Row::File {
                section,
                file: index,
            });
            if !view.open.contains(&(section, file.path.clone())) {
                continue;
            }
            if out.len >= MAX_TEXT {
                cut = true;
                continue;
            }
            let shown = match (file.reads, file.whole_only) {
                (false, Some(reason)) => Hunks::Note(format!("({reason}: s and u take it whole)")),
                _ => match hunks.get(&(section, file.path.clone())) {
                    Some(hunks) => hunks.clone(),
                    None => Hunks::Note("reading…".into()),
                },
            };
            file_hunks(out, built, outline, section, index, &shown);
        }
        if files.len() > MAX_FILES {
            out.part(
                format!("… and {} more files not shown", files.len() - MAX_FILES).as_bytes(),
                Some(Highlight::GitCount),
            );
            out.end_line();
            built.rows.push(Row::More(section));
        }
        cut
    }
}

fn head_lines(out: &mut Out, rows: &mut Vec<Row>, head: &Head) {
    out.part(b"Head:     ", None);
    match &head.branch.head {
        Some(branch) => {
            let mut name = Vec::new();
            push_name(&mut name, branch);
            out.part(&name, Some(Highlight::GitBranchLocal));
        }
        None => out.part(b"(detached)", Some(Highlight::GitCount)),
    }
    match &head.commit {
        Some((id, subject)) => {
            out.part(b" ", None);
            out.part(id.as_bytes(), Some(Highlight::GitHash));
            out.part(b" ", None);
            let mut words = Vec::new();
            push_name(&mut words, subject);
            out.part(&words, None);
        }
        None if head.branch.oid.is_none() => {
            out.part(b" (no commits yet)", Some(Highlight::GitCount));
        }
        None => {}
    }
    out.end_line();
    rows.push(Row::Other);
    if let Some(upstream) = &head.branch.upstream {
        out.part(b"Upstream: ", None);
        let mut name = Vec::new();
        push_name(&mut name, upstream);
        out.part(&name, Some(Highlight::GitBranchRemote));
        if let Some((ahead, behind)) = head.branch.ahead_behind {
            out.part(
                format!("  ahead {ahead}, behind {behind}").as_bytes(),
                Some(Highlight::GitCount),
            );
        }
        out.end_line();
        rows.push(Row::Other);
    }
}

fn file_line(out: &mut Out, file: &File) {
    out.part(&file.letters, None);
    out.part(b" ", None);
    let mut path = Vec::new();
    if let Some(from) = &file.from {
        push_name(&mut path, from);
        path.extend_from_slice(b" -> ");
    }
    push_name(&mut path, &file.path);
    out.part(&path, None);
    match file.counts {
        Some(Some((added, removed))) => out.part(
            format!("  +{added} -{removed}").as_bytes(),
            Some(Highlight::GitCount),
        ),
        Some(None) => out.part(b"  binary", Some(Highlight::GitCount)),
        None => {}
    }
    out.end_line();
}

fn file_hunks(
    out: &mut Out,
    built: &mut Built,
    outline: &mut Vec<GitHead>,
    section: Section,
    file: usize,
    hunks: &Hunks,
) {
    let detail = |out: &mut Out, rows: &mut Vec<Row>, text: &str| {
        out.part(b"    ", None);
        out.part(text.as_bytes(), Some(Highlight::GitCount));
        out.end_line();
        rows.push(Row::Detail { section, file });
    };
    let patch = match hunks {
        Hunks::Note(note) => return detail(out, &mut built.rows, note),
        Hunks::Patch(patch) => Arc::clone(patch),
    };
    if patch.hunks.is_empty() {
        let why = patch.whole_only.unwrap_or("no change in its lines");
        detail(
            out,
            &mut built.rows,
            &format!("({why}: s and u take it whole)"),
        );
        return;
    }
    let start = out.len;
    let mut headers = Vec::with_capacity(patch.hunks.len());
    let mut left_out = 0usize;
    for (index, hunk) in patch.hunks.iter().enumerate() {
        if out.len - start >= MAX_FILE_TEXT {
            left_out += 1 + hunk.lines.len();
            continue;
        }
        outline.push(GitHead {
            line: built.rows.len(),
            depth: 2,
        });
        headers.push(built.rows.len());
        out.part(trim_break(&hunk.header), Some(Highlight::DiffHunkHeader));
        out.end_line();
        built.rows.push(Row::Hunk {
            section,
            file,
            hunk: index,
        });
        for (line_index, line) in hunk.lines.iter().enumerate() {
            let face = match line.first() {
                Some(b'+') => Some(Highlight::DiffAdded),
                Some(b'-') => Some(Highlight::DiffRemoved),
                Some(b'\\') => Some(Highlight::GitCount),
                _ => None,
            };
            out.part(trim_break(line), face);
            out.end_line();
            built.rows.push(Row::Line {
                section,
                file,
                hunk: index,
                line: line_index,
            });
        }
    }
    if left_out > 0 {
        detail(
            out,
            &mut built.rows,
            &format!("… {left_out} more lines not shown"),
        );
    }
    if patch.cut {
        detail(out, &mut built.rows, "… git wrote more than oso reads");
    }
    built.hunk_rows.insert((section, file), headers);
    built.patches.insert((section, file), patch);
}

/// A line without its line break, `\n` or `\r\n`.
fn trim_break(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// The text built so far, with its runs of highlights.
#[derive(Default)]
struct Out {
    bytes: Vec<u8>,
    builder: Option<Builder>,
    runs: Vec<(usize, Option<Highlight>)>,
    len: usize,
}

impl Out {
    fn part(&mut self, bytes: &[u8], face: Option<Highlight>) {
        // A part is never a line break of its own but through `end_line`, so that every line of
        // the text is one row of the table.
        let len = decoded_len(bytes);
        for &byte in bytes {
            self.bytes.push(if byte == b'\n' { b' ' } else { byte });
        }
        self.len += len;
        match self.runs.last_mut() {
            Some((run, last)) if *last == face => *run += len,
            _ if len > 0 => self.runs.push((len, face)),
            _ => {}
        }
    }

    fn end_line(&mut self) {
        self.bytes.push(b'\n');
        self.len += 1;
        match self.runs.last_mut() {
            Some((run, None)) => *run += 1,
            _ => self.runs.push((1, None)),
        }
        // The builder takes whole lines, so that no sequence is cut between two pieces.
        if self.bytes.len() >= 64 * 1024 {
            let builder = self
                .builder
                .get_or_insert_with(|| Builder::new(None, Default::default()));
            builder.push(&self.bytes);
            self.bytes.clear();
        }
    }

    fn finish(mut self) -> (LoadSnapshot, Highlights) {
        let mut builder = self
            .builder
            .take()
            .unwrap_or_else(|| Builder::new(None, Default::default()));
        builder.push(&self.bytes);
        builder.finish();
        let text = builder.snapshot();
        let mut highlights = Highlights::new(text.text().len());
        debug_assert_eq!(text.text().len(), self.len);
        highlights.replace(0..self.len, self.runs);
        (text, highlights)
    }
}
