//! What each line of a review buffer is (ADR 0122, "The views"): a file's header, a note, a
//! hunk's header, lines of a side, or a comment's label or text. The host builds the rows with
//! the text, a section per file, and every key and paint asks the rows, never the text, what a
//! line is, so that no text of a file or of a comment can pass for a header or a label.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.
//!
//! A [`Section`] holds one file's rows as runs of lines of one kind, and the byte spans its
//! refinement marks. [`Rows`] holds the sections in order with the line each starts at, so
//! swapping a section costs the files, not the lines, and a lookup halves twice.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use super::comment::CommentId;
use super::shape::Side;

/// What a line of a review buffer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A file's header: its status, path and counts.
    File,
    /// A note on a file: why it is not shown, or what else changed.
    Note,
    /// The header of the file's hunk of this index.
    Hunk { hunk: usize },
    /// Lines of a side in a hunk, numbered from `first` there.
    Lines { hunk: usize, side: Side, first: u32 },
    /// The label of a comment: who wrote it and its number.
    Label { comment: CommentId },
    /// The text of a comment.
    Card { comment: CommentId },
    /// A row of one pane of two that holds nothing, so that the rows after it line up with the
    /// other pane's (ADR 0122, Q7): motion skips it and no comment goes on it.
    Filler,
}

/// Where a row is in a file's section, so that the same row is found in another layout of the
/// same review, the unified text or a pane: a line of a side, or the row of a kind that comes
/// `nth` among the section's rows of that kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Line { side: Side, number: u32 },
    Row { kind: RowKind, nth: usize },
}

/// One line of a review buffer: the file it is in, what it is, and how far into its run of
/// lines of that kind it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub path: &'a [u8],
    pub kind: RowKind,
    pub offset: usize,
}

impl Row<'_> {
    /// The side and number of the file's line this row shows, for lines of a side.
    pub fn line(&self) -> Option<(Side, u32)> {
        match self.kind {
            RowKind::Lines { side, first, .. } => {
                let offset = u32::try_from(self.offset).ok()?;
                Some((side, first.checked_add(offset)?))
            }
            _ => None,
        }
    }
}

/// A run of lines of one kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    kind: RowKind,
    /// Its first line, from the section's start.
    start: usize,
    lines: usize,
}

/// One file's rows, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    path: Box<[u8]>,
    runs: Vec<Run>,
    lines: usize,
    /// The lines refined, from the section's start, in order, and beside them the spans of
    /// bytes from each line's start that refinement marks.
    refined_lines: Vec<usize>,
    refined: Vec<Range<u32>>,
}

impl Section {
    /// The section of the file at `path`, with no rows yet.
    pub fn new(path: Box<[u8]>) -> Section {
        Section {
            path,
            ..Section::default()
        }
    }

    /// Adds `lines` lines of `kind` after those before. A run of no lines adds nothing.
    pub fn push(&mut self, kind: RowKind, lines: usize) {
        if lines == 0 {
            return;
        }
        self.runs.push(Run {
            kind,
            start: self.lines,
            lines,
        });
        self.lines += lines;
    }

    /// Marks bytes `bytes` from the start of the section's line `line` as refined.
    pub fn refine(&mut self, line: usize, bytes: Range<u32>) {
        // Calls come in order; one that does not goes where it belongs.
        let at = self.refined_lines.partition_point(|&other| other <= line);
        self.refined_lines.insert(at, line);
        self.refined.insert(at, bytes);
    }

    /// The path of its file.
    pub fn path(&self) -> &[u8] {
        &self.path
    }

    /// How many lines it has.
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// The run that line `line` of the section is in.
    fn run(&self, line: usize) -> Option<&Run> {
        let index = self.runs.partition_point(|run| run.start <= line);
        let run = self.runs.get(index.checked_sub(1)?)?;
        (line < run.start + run.lines).then_some(run)
    }

    fn refined(&self, line: usize) -> &[Range<u32>] {
        let from = self.refined_lines.partition_point(|&other| other < line);
        let to = self.refined_lines.partition_point(|&other| other <= line);
        &self.refined[from..to]
    }
}

/// Where a motion goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seek {
    /// A hunk's header.
    Hunk,
    /// A file's header.
    File,
    /// A comment's label.
    Comment,
}

impl Seek {
    fn takes(self, kind: RowKind) -> bool {
        match self {
            Seek::Hunk => matches!(kind, RowKind::Hunk { .. }),
            Seek::File => kind == RowKind::File,
            Seek::Comment => matches!(kind, RowKind::Label { .. }),
        }
    }
}

/// Every line of a review buffer, as sections in order. Shared: a clone is O(1), so a command
/// can hold the rows while it moves the cursors.
#[derive(Clone, Debug, Default)]
pub struct Rows {
    sections: Arc<[Arc<Section>]>,
    /// The line each section starts at.
    starts: Arc<[usize]>,
    /// Each section by its path's hash, for a lookup by path in O(1); paths of one hash are
    /// looked for among the sections in order.
    by_path: Arc<HashMap<u64, usize>>,
    lines: usize,
}

/// The hash a path is looked up by.
fn path_hash(path: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

/// What a section's index says of a hash two paths share.
const SHARED: usize = usize::MAX;

impl Rows {
    /// The rows of `sections`, one after another. O(files).
    pub fn new(sections: Vec<Arc<Section>>) -> Rows {
        let mut starts = Vec::with_capacity(sections.len());
        let mut by_path = HashMap::with_capacity(sections.len());
        let mut lines = 0;
        for (index, section) in sections.iter().enumerate() {
            starts.push(lines);
            lines += section.lines;
            by_path
                .entry(path_hash(&section.path))
                .and_modify(|at| *at = SHARED)
                .or_insert(index);
        }
        Rows {
            sections: sections.into(),
            starts: starts.into(),
            by_path: Arc::new(by_path),
            lines,
        }
    }

    /// How many lines the rows describe.
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// The section line `line` is in, and its line there.
    fn section(&self, line: usize) -> Option<(usize, usize)> {
        if line >= self.lines {
            return None;
        }
        let index = self.starts.partition_point(|&start| start <= line) - 1;
        Some((index, line - self.starts[index]))
    }

    /// What line `line` of the buffer is. `None` past the last.
    pub fn at(&self, line: usize) -> Option<Row<'_>> {
        let (index, local) = self.section(line)?;
        let section = &self.sections[index];
        let run = section.run(local)?;
        Some(Row {
            path: &section.path,
            kind: run.kind,
            offset: local - run.start,
        })
    }

    /// The lines of the run of rows of one kind that line `line` is in. O(log of files and
    /// runs).
    pub(crate) fn run_lines(&self, line: usize) -> Option<Range<usize>> {
        let (index, local) = self.section(line)?;
        let run = self.sections[index].run(local)?;
        let start = self.starts[index] + run.start;
        Some(start..start + run.lines)
    }

    /// Where line `line` is in its file's section, and the file's path: `None` for a filler
    /// row and past the last line. O(log of files and runs, and the runs before it).
    pub(crate) fn place(&self, line: usize) -> Option<(&[u8], Place)> {
        let row = self.at(line)?;
        let place = match row.kind {
            RowKind::Filler => return None,
            RowKind::Lines { .. } => {
                let (side, number) = row.line()?;
                Place::Line { side, number }
            }
            kind => {
                let (index, local) = self.section(line)?;
                let before: usize = self.sections[index]
                    .runs
                    .iter()
                    .take_while(|run| run.start + run.lines <= local)
                    .filter(|run| run.kind == kind)
                    .map(|run| run.lines)
                    .sum();
                Place::Row {
                    kind,
                    nth: before + row.offset,
                }
            }
        };
        Some((row.path, place))
    }

    /// The line at `place` in the section of the file at `path`: `None` for a file not shown,
    /// and for a row the section does not hold, as a pane holds only one side's lines and
    /// cards. O(files and the file's runs).
    pub(crate) fn find(&self, path: &[u8], place: Place) -> Option<usize> {
        let index = self.section_of(path)?;
        let start = self.starts[index];
        let mut nth = 0;
        for run in &self.sections[index].runs {
            match (place, run.kind) {
                (
                    Place::Line { side, number },
                    RowKind::Lines {
                        side: at, first, ..
                    },
                ) if side == at && number >= first && ((number - first) as usize) < run.lines => {
                    return Some(start + run.start + (number - first) as usize);
                }
                (Place::Row { kind, nth: wanted }, at) if kind == at => {
                    if wanted < nth + run.lines {
                        return Some(start + run.start + wanted - nth);
                    }
                    nth += run.lines;
                }
                _ => {}
            }
        }
        None
    }

    /// The first line of the section of the file at `path`, its header. O(1).
    pub(crate) fn file_start(&self, path: &[u8]) -> Option<usize> {
        Some(self.starts[self.section_of(path)?])
    }

    /// The row of `side` of the file at `path` whose line number is nearest `number`, the upper
    /// one on a tie: `None` for a file not shown, or one with no row of that side. O(the file's
    /// runs).
    pub(crate) fn nearest(&self, path: &[u8], side: Side, number: u32) -> Option<usize> {
        let index = self.section_of(path)?;
        let start = self.starts[index];
        let mut best: Option<(u32, usize)> = None;
        for run in &self.sections[index].runs {
            let RowKind::Lines {
                side: at, first, ..
            } = run.kind
            else {
                continue;
            };
            if at != side {
                continue;
            }
            let last = first.saturating_add(u32::try_from(run.lines - 1).unwrap_or(u32::MAX));
            let (away, line) = if number < first {
                (first - number, run.start)
            } else if number > last {
                (number - last, run.start + run.lines - 1)
            } else {
                (0, run.start + (number - first) as usize)
            };
            if best.is_none_or(|(nearest, _)| away < nearest) {
                best = Some((away, line));
            }
        }
        best.map(|(_, line)| start + line)
    }

    /// The section of the file at `path`. O(1), or O(files) for a path whose hash another
    /// shares.
    fn section_of(&self, path: &[u8]) -> Option<usize> {
        match self.by_path.get(&path_hash(path)) {
            Some(&SHARED) => self
                .sections
                .iter()
                .position(|section| *section.path == *path),
            Some(&index) => (*self.sections[index].path == *path).then_some(index),
            None => None,
        }
    }

    /// The spans of bytes from line `line`'s start that refinement marks, in order.
    pub fn refined(&self, line: usize) -> &[Range<u32>] {
        match self.section(line) {
            Some((index, local)) => self.sections[index].refined(local),
            None => &[],
        }
    }

    /// The first line after `line` (`forward`) or before it that starts a row `to` goes to.
    /// O(log of files and runs, and the runs passed): the runs of `line`'s own section are
    /// found by halving, so a step costs what it passes, not what lies before it.
    pub fn seek(&self, line: usize, to: Seek, forward: bool) -> Option<usize> {
        if forward {
            let (first, local) = self.section(line)?;
            (first..self.sections.len()).find_map(|index| {
                let runs = &self.sections[index].runs;
                // Only the current section has runs at or before `line`.
                let from = if index == first {
                    runs.partition_point(|run| run.start <= local)
                } else {
                    0
                };
                let start = self.starts[index];
                runs[from..]
                    .iter()
                    .find(|run| to.takes(run.kind))
                    .map(|run| start + run.start)
            })
        } else {
            let (last, local) = match self.section(line) {
                Some(found) => found,
                None => (self.sections.len().checked_sub(1)?, usize::MAX),
            };
            (0..=last).rev().find_map(|index| {
                let runs = &self.sections[index].runs;
                let to_end = if index == last {
                    runs.partition_point(|run| run.start < local)
                } else {
                    runs.len()
                };
                let start = self.starts[index];
                runs[..to_end]
                    .iter()
                    .rev()
                    .find(|run| to.takes(run.kind))
                    .map(|run| start + run.start)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn section(path: &str, runs: &[(RowKind, usize)]) -> Arc<Section> {
        let mut section = Section::new(path.as_bytes().into());
        for &(kind, lines) in runs {
            section.push(kind, lines);
        }
        Arc::new(section)
    }

    fn lines(hunk: usize, side: Side, first: u32) -> RowKind {
        RowKind::Lines { hunk, side, first }
    }

    fn sample() -> Rows {
        Rows::new(vec![
            section(
                "a.rs",
                &[
                    (RowKind::File, 1),
                    (RowKind::Hunk { hunk: 0 }, 1),
                    (lines(0, Side::Old, 4), 2),
                    (lines(0, Side::New, 4), 3),
                    (
                        RowKind::Label {
                            comment: CommentId(7),
                        },
                        1,
                    ),
                    (
                        RowKind::Card {
                            comment: CommentId(7),
                        },
                        2,
                    ),
                    (RowKind::Hunk { hunk: 1 }, 1),
                    (lines(1, Side::New, 20), 1),
                ],
            ),
            section("b.bin", &[(RowKind::File, 1), (RowKind::Note, 1)]),
            section(
                "c.rs",
                &[
                    (RowKind::File, 1),
                    (RowKind::Hunk { hunk: 0 }, 1),
                    (lines(0, Side::Old, 1), 1),
                ],
            ),
        ])
    }

    #[test]
    fn the_nearest_row_of_a_side_is_found_the_upper_one_on_a_tie() {
        let rows = sample();
        // a.rs shows new lines 4 to 6 at rows 4 to 6, and new line 20 at row 11.
        assert_eq!(rows.nearest(b"a.rs", Side::New, 5), Some(5));
        assert_eq!(rows.nearest(b"a.rs", Side::New, 1), Some(4));
        assert_eq!(rows.nearest(b"a.rs", Side::New, 15), Some(11));
        assert_eq!(rows.nearest(b"a.rs", Side::New, 13), Some(6), "7 from each");
        assert_eq!(rows.nearest(b"a.rs", Side::Old, 9), Some(3));
        // c.rs starts at row 14 and shows old line 1 only.
        assert_eq!(rows.nearest(b"c.rs", Side::Old, 5), Some(16));
        assert_eq!(rows.nearest(b"c.rs", Side::New, 1), None);
        assert_eq!(rows.nearest(b"b.bin", Side::New, 1), None);
        assert_eq!(rows.nearest(b"z.rs", Side::New, 1), None);
        assert_eq!(rows.file_start(b"c.rs"), Some(14));
        assert_eq!(rows.file_start(b"z.rs"), None);
    }

    #[test]
    fn every_row_but_a_filler_is_found_again_by_its_place() {
        let mut rows = sample();
        let mut with_filler = Section::new(b"d.rs".as_slice().into());
        for (kind, lines) in [
            (RowKind::File, 1),
            (RowKind::Note, 1),
            (RowKind::Filler, 2),
            (RowKind::Note, 2),
        ] {
            with_filler.push(kind, lines);
        }
        let mut sections: Vec<Arc<Section>> = rows.sections.iter().cloned().collect();
        sections.push(Arc::new(with_filler));
        rows = Rows::new(sections);
        for line in 0..rows.lines() {
            let found = rows.place(line);
            if rows.at(line).unwrap().kind == RowKind::Filler {
                assert_eq!(found, None);
                continue;
            }
            let (path, place) = found.unwrap();
            assert_eq!(rows.find(path, place), Some(line), "line {line}: {place:?}");
        }
        // The notes after the fillers are the second and third of the file's notes.
        assert_eq!(
            rows.place(rows.lines() - 1).unwrap().1,
            Place::Row {
                kind: RowKind::Note,
                nth: 2
            }
        );
        // A line of a side the rows do not hold, and a file not shown.
        let missing = Place::Line {
            side: Side::New,
            number: 1,
        };
        assert_eq!(rows.find(b"c.rs", missing), None);
        assert_eq!(rows.file_start(b"c.rs"), Some(14));
        assert_eq!(rows.find(b"none", missing), None);
    }

    #[test]
    fn each_line_says_what_it_is() {
        // a.rs: file 0, hunk 1, old 2-3, new 4-6, label 7, card 8-9, hunk 10, new 11;
        // b.bin: file 12, note 13; c.rs: file 14, hunk 15, old 16.
        let rows = sample();
        assert_eq!(rows.lines(), 17);
        let row = rows.at(3).unwrap();
        assert_eq!(row.path, b"a.rs");
        assert_eq!(row.kind, lines(0, Side::Old, 4));
        assert_eq!(row.line(), Some((Side::Old, 5)));
        assert_eq!(rows.at(6).unwrap().line(), Some((Side::New, 6)));
        let label = rows.at(7).unwrap();
        assert_eq!(
            label.kind,
            RowKind::Label {
                comment: CommentId(7)
            }
        );
        assert_eq!(label.line(), None);
        assert_eq!(rows.at(14).unwrap().path, b"c.rs");
        assert!(rows.at(17).is_none());
    }

    #[test]
    fn seeking_goes_to_the_next_or_previous_header() {
        let rows = sample();
        assert_eq!(rows.seek(0, Seek::Hunk, true), Some(1));
        assert_eq!(rows.seek(1, Seek::Hunk, true), Some(10));
        assert_eq!(rows.seek(10, Seek::Hunk, true), Some(15));
        assert_eq!(rows.seek(15, Seek::Hunk, true), None);
        assert_eq!(rows.seek(15, Seek::Hunk, false), Some(10));
        assert_eq!(rows.seek(1, Seek::Hunk, false), None);
        assert_eq!(rows.seek(3, Seek::File, true), Some(12));
        assert_eq!(rows.seek(12, Seek::File, false), Some(0));
        assert_eq!(rows.seek(0, Seek::Comment, true), Some(7));
        assert_eq!(rows.seek(7, Seek::Comment, true), None);
        assert_eq!(rows.seek(99, Seek::File, false), Some(14));
    }

    #[test]
    fn a_step_costs_the_runs_it_passes_not_those_before_it() {
        // One file of many one-line hunks, as an agent might write: each step halves to its
        // place, so many steps from far in stay quick.
        let hunks = 100_000;
        let mut big = Section::new(b"big.rs".as_slice().into());
        big.push(RowKind::File, 1);
        for hunk in 0..hunks {
            big.push(RowKind::Hunk { hunk }, 1);
            big.push(lines(hunk, Side::New, 1), 1);
        }
        let rows = Rows::new(vec![Arc::new(big)]);
        let mut line = 0;
        for _ in 0..hunks {
            line = rows.seek(line, Seek::Hunk, true).unwrap();
        }
        assert_eq!(line, 2 * hunks - 1);
        assert_eq!(rows.seek(line, Seek::Hunk, true), None);
        for _ in 1..hunks {
            line = rows.seek(line, Seek::Hunk, false).unwrap();
        }
        assert_eq!(line, 1);
        assert_eq!(rows.seek(line, Seek::File, false), Some(0));
    }

    #[test]
    fn refinement_is_kept_by_line() {
        let mut section = Section::new(b"a".to_vec().into());
        section.push(RowKind::File, 1);
        section.push(lines(0, Side::Old, 1), 2);
        section.refine(2, 4..6);
        section.refine(1, 0..2);
        section.refine(2, 8..9);
        let rows = Rows::new(vec![
            Arc::new(Section::new(b"z".to_vec().into())),
            Arc::new(section),
        ]);
        assert_eq!(rows.refined(0), &[]);
        assert_eq!(rows.refined(1), std::slice::from_ref(&(0..2)));
        assert_eq!(rows.refined(2), &[4..6, 8..9]);
        assert_eq!(rows.refined(3), &[]);
    }

    fn kind() -> impl Strategy<Value = RowKind> {
        prop_oneof![
            Just(RowKind::File),
            Just(RowKind::Note),
            (0..3usize).prop_map(|hunk| RowKind::Hunk { hunk }),
            (0..3usize, any::<bool>(), 1..50u32).prop_map(|(hunk, old, first)| RowKind::Lines {
                hunk,
                side: if old { Side::Old } else { Side::New },
                first
            }),
            (0..3u64).prop_map(|id| RowKind::Label {
                comment: CommentId(id)
            }),
            (0..3u64).prop_map(|id| RowKind::Card {
                comment: CommentId(id)
            }),
        ]
    }

    proptest! {
        /// Whatever the sections, a line's row is the one its section's runs give, a seek lands
        /// on the nearest row of its kind.
        #[test]
        fn rows_agree_with_their_sections(
            given in prop::collection::vec(prop::collection::vec((kind(), 0..4usize), 0..6), 0..5),
        ) {
            let sections: Vec<Arc<Section>> = given
                .iter()
                .enumerate()
                .map(|(index, runs)| {
                    let mut section = Section::new(format!("f{index}").into_bytes().into());
                    for &(kind, lines) in runs {
                        section.push(kind, lines);
                    }
                    Arc::new(section)
                })
                .collect();
            // The reference: every line, in order, with its file, kind, offset and run start.
            let mut flat = Vec::new();
            for (index, runs) in given.iter().enumerate() {
                for &(kind, lines) in runs {
                    for offset in 0..lines {
                        flat.push((index, kind, offset));
                    }
                }
            }
            let rows = Rows::new(sections);
            prop_assert_eq!(rows.lines(), flat.len());
            for (line, &(index, kind, offset)) in flat.iter().enumerate() {
                let row = rows.at(line).unwrap();
                let path = format!("f{index}");
                prop_assert_eq!(row.path, path.as_bytes());
                prop_assert_eq!(row.kind, kind);
                prop_assert_eq!(row.offset, offset);
            }
            prop_assert!(rows.at(flat.len()).is_none());
            for to in [Seek::Hunk, Seek::File, Seek::Comment] {
                let starts: Vec<usize> = (0..flat.len())
                    .filter(|&line| flat[line].2 == 0 && to.takes(flat[line].1))
                    .collect();
                for line in 0..=flat.len() {
                    let next = starts.iter().copied().find(|&start| start > line);
                    let previous = starts.iter().copied().rev().find(|&start| start < line);
                    if line < flat.len() {
                        prop_assert_eq!(rows.seek(line, to, true), next);
                    }
                    prop_assert_eq!(rows.seek(line, to, false), previous);
                }
            }
        }
    }
}
