//! Writing a status's text (ADR 0127): its lines, their highlights, the outline of its
//! headings and the table of what each line is about, from the sections laid out in order.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, git or the file system; [`lay_out`] is a
//! pure function of what was read.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use oso_core::editor::GitHeading;
use oso_core::load::{Builder, LoadSnapshot};
use oso_core::syntax::{Highlight, Highlights};

use super::{Built, File, Head, Hunks, MAX_FILE_TEXT, MAX_FILES, MAX_TEXT, Row, Section, View};
use crate::dired::rows::{decoded_len, push_name};
use crate::vcs::history::Log;
use crate::vcs::sequence::{Doing, Stop};

/// A status's text, highlights, outline and table.
pub(crate) struct Laid {
    pub(crate) text: LoadSnapshot,
    pub(crate) highlights: Highlights,
    pub(crate) outline: Vec<GitHeading>,
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
    let mut built = Built {
        branch: head.branch.clone(),
        stop: head.stop.clone(),
        ..Built::default()
    };
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
    for (section, log) in &head.logs {
        let mut shown = Shown {
            out: &mut out,
            built: &mut built,
            outline: &mut outline,
        };
        shown.log(*section, log, head.branch.upstream.as_deref(), view);
    }
    built.logs.clone_from(&head.logs);
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
    outline: &'a mut Vec<GitHeading>,
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
        outline.push(GitHeading {
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
            Some(Highlight::GitDetail),
        );
        if view.folded.contains(&section) {
            out.part("…".as_bytes(), Some(Highlight::GitDetail));
        }
        out.end_line();
        built.rows.push(Row::Heading(section));
        if view.folded.contains(&section) {
            return cut;
        }
        for (index, file) in files.iter().enumerate().take(MAX_FILES) {
            outline.push(GitHeading {
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
            if !view.is_open(section, &file.path) {
                continue;
            }
            if out.len >= MAX_TEXT {
                cut = true;
                continue;
            }
            let shown = match (file.reads, file.whole_only) {
                (false, Some(reason)) => Hunks::Whole(reason),
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
                Some(Highlight::GitDetail),
            );
            out.end_line();
            built.rows.push(Row::More(section));
        }
        cut
    }

    /// Lays out the commit section `section`'s heading and commits, the unpushed and unpulled
    /// ones naming `upstream`.
    fn log(&mut self, section: Section, log: &Log, upstream: Option<&[u8]>, view: &View) {
        let Shown {
            out,
            built,
            outline,
        } = self;
        if log.commits.is_empty() {
            return;
        }
        out.end_line();
        built.rows.push(Row::Other);
        outline.push(GitHeading {
            line: built.rows.len(),
            depth: 0,
        });
        built.headings.insert(section, built.rows.len());
        out.part(
            section.heading().as_bytes(),
            Some(Highlight::GitSectionHeading),
        );
        let names_upstream = matches!(section, Section::Unpushed | Section::Unpulled);
        if let Some(upstream) = upstream.filter(|_| names_upstream) {
            let mut name = b" ".to_vec();
            push_name(&mut name, upstream);
            out.part(&name, Some(Highlight::GitBranchRemote));
        }
        let more = if log.more { "+" } else { "" };
        out.part(
            format!(" ({}{more})", log.commits.len()).as_bytes(),
            Some(Highlight::GitDetail),
        );
        if view.folded.contains(&section) {
            out.part("…".as_bytes(), Some(Highlight::GitDetail));
        }
        out.end_line();
        built.rows.push(Row::Heading(section));
        if view.folded.contains(&section) {
            return;
        }
        for (index, commit) in log.commits.iter().enumerate() {
            built.file_rows.insert(
                (section, commit.id.clone().into_bytes()),
                (index, built.rows.len()),
            );
            out.part(commit.short.as_bytes(), Some(Highlight::GitHash));
            let mut subject = b" ".to_vec();
            push_name(&mut subject, &commit.subject);
            out.part(&subject, None);
            out.end_line();
            built.rows.push(Row::Commit {
                section,
                commit: index,
            });
        }
        if log.more && section != Section::Recent {
            out.part(
                "… and more not shown".as_bytes(),
                Some(Highlight::GitDetail),
            );
            out.end_line();
            built.rows.push(Row::More(section));
        }
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
        None => out.part(b"(detached)", Some(Highlight::GitDetail)),
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
            out.part(b" (no commits yet)", Some(Highlight::GitDetail));
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
                Some(Highlight::GitDetail),
            );
        }
        out.end_line();
        rows.push(Row::Other);
    }
    if let Some(push) = &head.push {
        out.part(b"Push:     ", None);
        let mut name = Vec::new();
        push_name(&mut name, push);
        out.part(&name, Some(Highlight::GitBranchRemote));
        out.end_line();
        rows.push(Row::Other);
    }
    if let Some(stop) = &head.stop {
        stop_lines(out, rows, stop);
    }
}

/// What is stopped, as `Rebasing: main onto abc1234, 2 of 5; stopped at def5678 SUBJECT`, and
/// the keys that go on with it (step 3a).
fn stop_lines(out: &mut Out, rows: &mut Vec<Row>, stop: &Stop) {
    let commit = |out: &mut Out, id: &str| {
        let (short, subject) = stop.named(id);
        out.part(short.as_bytes(), Some(Highlight::GitHash));
        if let Some(subject) = subject {
            let mut words = vec![b' '];
            push_name(&mut words, subject);
            out.part(&words, None);
        }
    };
    out.part(stop.doing.label().as_bytes(), None);
    match stop.doing {
        Doing::Rebase => {
            match &stop.branch {
                Some(branch) => {
                    let mut name = Vec::new();
                    push_name(&mut name, branch);
                    out.part(&name, Some(Highlight::GitBranchLocal));
                }
                None => out.part(b"(detached)", Some(Highlight::GitDetail)),
            }
            if let Some(onto) = &stop.onto {
                out.part(b" onto ", None);
                let (short, _) = stop.named(onto);
                out.part(short.as_bytes(), Some(Highlight::GitHash));
            }
            if let Some((at, of)) = stop.progress {
                out.part(
                    format!(", {at} of {of}").as_bytes(),
                    Some(Highlight::GitDetail),
                );
            }
            if let Some(at) = &stop.at {
                out.part(b"; stopped at ", None);
                commit(out, at);
            }
        }
        Doing::Am => {
            out.part(b"patches", None);
            if let Some((at, of)) = stop.progress {
                out.part(
                    format!(", {at} of {of}").as_bytes(),
                    Some(Highlight::GitDetail),
                );
            }
        }
        Doing::Merge | Doing::CherryPick | Doing::Revert => match &stop.at {
            Some(at) => commit(out, at),
            None => out.part(b"between commits", Some(Highlight::GitDetail)),
        },
        Doing::Bisect => out.part(
            b"git bisect reset in a shell ends it",
            Some(Highlight::GitDetail),
        ),
    }
    out.end_line();
    rows.push(Row::Other);
    if let Some(keys) = stop.doing.keys() {
        out.part(b"          ", None);
        out.part(keys.as_bytes(), Some(Highlight::GitDetail));
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
            Some(Highlight::GitDetail),
        ),
        Some(None) => out.part(b"  binary", Some(Highlight::GitDetail)),
        None => {}
    }
    out.end_line();
}

fn file_hunks(
    out: &mut Out,
    built: &mut Built,
    outline: &mut Vec<GitHeading>,
    section: Section,
    file: usize,
    hunks: &Hunks,
) {
    let detail = |out: &mut Out, rows: &mut Vec<Row>, text: &str, whole: bool| {
        out.part(b"    ", None);
        out.part(text.as_bytes(), Some(Highlight::GitDetail));
        out.end_line();
        rows.push(Row::Detail {
            section,
            file,
            whole,
        });
    };
    let whole = |why: &str| format!("({why}: s and u take it whole)");
    let patch = match hunks {
        Hunks::Note(note) => return detail(out, &mut built.rows, note, false),
        Hunks::Whole(why) => return detail(out, &mut built.rows, &whole(why), true),
        Hunks::Patch(patch) => Arc::clone(patch),
    };
    if patch.hunks.is_empty() {
        let why = patch.whole_only.unwrap_or("no change in its lines");
        return detail(out, &mut built.rows, &whole(why), true);
    }
    let start = out.len;
    let mut headers = Vec::with_capacity(patch.hunks.len());
    let mut left_out = 0usize;
    for (index, hunk) in patch.hunks.iter().enumerate() {
        if out.len - start >= MAX_FILE_TEXT {
            left_out += 1 + hunk.lines.len();
            continue;
        }
        outline.push(GitHeading {
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
            hunk_line(
                out,
                line,
                hunk.refined.get(&line_index).map_or(&[], Vec::as_slice),
            );
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
        let more = format!("… {left_out} more lines not shown");
        detail(out, &mut built.rows, &more, false);
    }
    if patch.cut {
        detail(
            out,
            &mut built.rows,
            "… git wrote more than oso reads",
            false,
        );
    }
    // Hunks shown that stage only with their file, as a new file's, say why under them.
    if let Some(why) = patch.whole_only.filter(|_| !patch.cut) {
        detail(out, &mut built.rows, &whole(why), true);
    }
    built.hunk_rows.insert((section, file), headers);
    built.patches.insert((section, file), patch);
}

/// Writes one of a hunk's lines in its face, the words refinement marked, `refined`, from after
/// its `+` or `-`, in the refined one.
fn hunk_line(out: &mut Out, line: &[u8], refined: &[Range<usize>]) {
    let (face, refine) = match line.first() {
        Some(b'+') => (Some(Highlight::DiffAdded), Highlight::DiffRefineAdded),
        Some(b'-') => (Some(Highlight::DiffRemoved), Highlight::DiffRefineRemoved),
        Some(b'\\') => (Some(Highlight::GitDetail), Highlight::GitDetail),
        _ => (None, Highlight::GitDetail),
    };
    let line = trim_break(line);
    let mut at = 0;
    for span in refined {
        let from = (span.start + 1).min(line.len()).max(at);
        let to = (span.end + 1).min(line.len()).max(from);
        out.part(&line[at..from], face);
        out.part(&line[from..to], Some(refine));
        at = to;
    }
    out.part(&line[at..], face);
}

/// A line without its line break, `\n` or `\r\n`.
fn trim_break(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// The text built so far, with its runs of highlights.
#[derive(Default)]
pub(in crate::vcs) struct Out {
    bytes: Vec<u8>,
    builder: Option<Builder>,
    runs: Vec<(usize, Option<Highlight>)>,
    len: usize,
}

impl Out {
    pub(in crate::vcs) fn part(&mut self, bytes: &[u8], face: Option<Highlight>) {
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

    pub(in crate::vcs) fn end_line(&mut self) {
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

    pub(in crate::vcs) fn finish(mut self) -> (LoadSnapshot, Highlights) {
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
