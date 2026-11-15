//! A status buffer's text, highlights, outline and row table (ADR 0127), in magit's order: the
//! head, then unmerged paths, untracked files, unstaged changes and staged changes, each a
//! section that folds, with a file's hunks under it when it is open.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, git or the file system; [`lay_out`] is a
//! pure function of what was read.
//!
//! - Each line of the text is one row of the table, which says what it is about, so that the
//!   host reads what a command means from the table, never from the text.
//! - Limits, each saying what it hides: [`MAX_FILES`] files a section, [`MAX_FILE_TEXT`] of one
//!   file's hunks and [`MAX_TEXT`] in all, past which files show their row alone.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use super::patch::Patch;
use super::status::{Branch, Counts, Entry, Kind, Status};

/// The most files a section shows; the rest are counted.
pub(crate) const MAX_FILES: usize = 10_000;
/// The most bytes of one file's hunks shown; past it, a line says how many more lines there are.
pub(crate) const MAX_FILE_TEXT: usize = 256 * 1024;
/// The most bytes the text holds before files show their row alone.
pub(crate) const MAX_TEXT: usize = 64 * 1024 * 1024;

/// A section of the status, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Section {
    Unmerged,
    Untracked,
    Unstaged,
    Staged,
}

impl Section {
    pub(crate) const ALL: [Section; 4] = [
        Section::Unmerged,
        Section::Untracked,
        Section::Unstaged,
        Section::Staged,
    ];

    /// Its heading, as magit's.
    fn heading(self) -> &'static str {
        match self {
            Section::Unmerged => "Unmerged paths",
            Section::Untracked => "Untracked files",
            Section::Unstaged => "Unstaged changes",
            Section::Staged => "Staged changes",
        }
    }
}

/// A file as a section shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct File {
    pub(crate) path: Vec<u8>,
    /// The path it was renamed or copied from, in the staged section.
    pub(crate) from: Option<Vec<u8>>,
    /// Its status letter, or letters for an unmerged path.
    pub(crate) letters: [u8; 2],
    /// Its lines added and removed; `Some(None)` for a binary file, `None` when not counted.
    pub(crate) counts: Option<Counts>,
    /// Why its hunks are not staged one by one, if they are not.
    pub(crate) whole_only: Option<&'static str>,
    /// Whether its hunks are read when it is open: not for a file that may hold secrets, an
    /// untracked or unmerged file, or a submodule, which show why instead.
    pub(crate) reads: bool,
    /// Whether it is gone from the work tree.
    pub(crate) deleted: bool,
}

/// Why a file's hunks are not read: it may hold secrets (ADR 0028), as the review's are not.
pub(crate) const SECRET: &str = "a file that may hold secrets";

/// The sections of `status`, each with its files in git's order, counted by `unstaged` and
/// `staged`. `secret` says whether a path may hold secrets.
pub(crate) fn sections(
    status: &Status,
    unstaged: &HashMap<Vec<u8>, Counts>,
    staged: &HashMap<Vec<u8>, Counts>,
    secret: &dyn Fn(&[u8]) -> bool,
) -> Vec<(Section, Vec<File>)> {
    let mut lists: Vec<(Section, Vec<File>)> = Section::ALL
        .iter()
        .map(|section| (*section, Vec::new()))
        .collect();
    let mut push = |section: Section, file: File| {
        if let Some((_, files)) = lists.iter_mut().find(|(of, _)| *of == section) {
            files.push(file);
        }
    };
    for entry in &status.entries {
        let hidden =
            secret(&entry.path) || matches!(&entry.kind, Kind::Moved { from, .. } if secret(from));
        let reason = |own: Option<&'static str>| {
            if hidden {
                Some(SECRET)
            } else if entry.submodule {
                Some("a submodule")
            } else {
                own
            }
        };
        match &entry.kind {
            Kind::Untracked => push(
                Section::Untracked,
                File {
                    path: entry.path.clone(),
                    from: None,
                    letters: [b'?', b' '],
                    counts: None,
                    whole_only: reason(Some("an untracked file")),
                    reads: false,
                    deleted: false,
                },
            ),
            Kind::Unmerged => push(
                Section::Unmerged,
                File {
                    path: entry.path.clone(),
                    from: None,
                    letters: [entry.staged, entry.unstaged],
                    counts: None,
                    whole_only: reason(Some("an unmerged file: resolve it, then s")),
                    reads: false,
                    deleted: false,
                },
            ),
            Kind::Changed | Kind::Moved { .. } => {
                if entry.unstaged != b'.' {
                    let file = changed(entry, unstaged, entry.unstaged, &reason);
                    push(Section::Unstaged, file);
                }
                if entry.staged != b'.' {
                    let mut file = changed(entry, staged, entry.staged, &reason);
                    if let Kind::Moved { from, .. } = &entry.kind {
                        file.from = Some(from.clone());
                        file.whole_only = file.whole_only.or(Some("a renamed file"));
                    }
                    push(Section::Staged, file);
                }
            }
        }
    }
    lists
}

fn changed(
    entry: &Entry,
    counts: &HashMap<Vec<u8>, Counts>,
    letter: u8,
    reason: &dyn Fn(Option<&'static str>) -> Option<&'static str>,
) -> File {
    File {
        path: entry.path.clone(),
        from: None,
        letters: [letter, b' '],
        counts: counts.get(&entry.path).copied(),
        whole_only: reason(None),
        reads: reason(None).is_none(),
        deleted: letter == b'D',
    }
}

/// What a file's open hunks are: its patch, or why it has none to show.
#[derive(Clone, Debug)]
pub(crate) enum Hunks {
    Patch(Arc<Patch>),
    /// What was not read, or why it could not be.
    Note(String),
    /// Why its hunks are not read: it stages only whole.
    Whole(&'static str),
}

/// What the head shows: the branch, and the subject of `HEAD`'s commit with its short id.
#[derive(Clone, Debug, Default)]
pub(crate) struct Head {
    pub(crate) branch: Branch,
    pub(crate) commit: Option<(String, Vec<u8>)>,
}

/// What is folded and open: sections hidden, and files whose hunks show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct View {
    pub(crate) folded: BTreeSet<Section>,
    /// The paths, from the root, of files whose open buffer may hold secrets: their hunks are
    /// not read, as a secret name's are not.
    pub(crate) withheld: BTreeSet<Vec<u8>>,
    /// Each section's open files, by path, so that a file is looked up without a copy of it.
    open: BTreeMap<Section, BTreeSet<Vec<u8>>>,
}

impl View {
    /// Shows the hunks of `path` in `section`, as `TAB` on its row does.
    #[cfg(test)]
    pub(crate) fn open_file(&mut self, section: Section, path: Vec<u8>) {
        self.open.entry(section).or_default().insert(path);
    }

    /// Whether the hunks of `path` in `section` show.
    pub(crate) fn is_open(&self, section: Section, path: &[u8]) -> bool {
        self.open
            .get(&section)
            .is_some_and(|paths| paths.contains(path))
    }

    /// What `TAB` on `line` of `built` does: folds or unfolds a section's heading, opens or
    /// closes a file's row, and on a hunk or a note under a file closes the file. Whether it
    /// changed anything.
    pub(crate) fn toggle(&mut self, built: &Built, line: usize) -> bool {
        let row = built.rows.get(line).copied().unwrap_or(Row::Other);
        if let Row::Heading(section) = row {
            if !self.folded.remove(&section) {
                self.folded.insert(section);
            }
            return true;
        }
        let (Some(section), Some(file)) = (row.section(), row.file()) else {
            return false;
        };
        let Some(path) = built.file(section, file).map(|file| file.path.clone()) else {
            return false;
        };
        let paths = self.open.entry(section).or_default();
        let opened = matches!(row, Row::File { .. }) && !paths.remove(&path);
        if opened {
            paths.insert(path);
        } else {
            paths.remove(&path);
        }
        true
    }

    /// Forgets the open files `built` no longer lists, so that their hunks are not read again.
    pub(crate) fn forget_gone(&mut self, built: &Built) {
        for (section, paths) in &mut self.open {
            paths.retain(|path| built.file_rows.contains_key(&(*section, path.clone())));
        }
        self.open.retain(|_, paths| !paths.is_empty());
    }
}

/// What a line is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// The head's lines, a note, or a blank between sections.
    Other,
    Heading(Section),
    /// The `file`th file of the section.
    File {
        section: Section,
        file: usize,
    },
    /// A note under an open file: why it stages only whole (`whole`), which `s` and `u` read as
    /// the file, or what was not shown or could not be read, which they read as nothing.
    Detail {
        section: Section,
        file: usize,
        whole: bool,
    },
    Hunk {
        section: Section,
        file: usize,
        hunk: usize,
    },
    Line {
        section: Section,
        file: usize,
        hunk: usize,
        line: usize,
    },
    /// The count of a section's files not shown.
    More(Section),
}

impl Row {
    fn section(self) -> Option<Section> {
        match self {
            Row::Other => None,
            Row::Heading(section) | Row::More(section) => Some(section),
            Row::File { section, .. }
            | Row::Detail { section, .. }
            | Row::Hunk { section, .. }
            | Row::Line { section, .. } => Some(section),
        }
    }

    fn file(self) -> Option<usize> {
        match self {
            Row::File { file, .. }
            | Row::Detail { file, .. }
            | Row::Hunk { file, .. }
            | Row::Line { file, .. } => Some(file),
            Row::Other | Row::Heading(_) | Row::More(_) => None,
        }
    }

    fn hunk(self) -> Option<usize> {
        match self {
            Row::Hunk { hunk, .. } | Row::Line { hunk, .. } => Some(hunk),
            _ => None,
        }
    }
}

/// What a status text shows, kept with it so that a line can be told back into what it is.
#[derive(Debug, Default)]
pub(crate) struct Built {
    pub(crate) sections: Vec<(Section, Vec<File>)>,
    /// The patches of the open files shown, by section and file.
    pub(crate) patches: HashMap<(Section, usize), Arc<Patch>>,
    pub(crate) rows: Vec<Row>,
    headings: HashMap<Section, usize>,
    file_rows: HashMap<(Section, Vec<u8>), (usize, usize)>,
    hunk_rows: HashMap<(Section, usize), Vec<usize>>,
}

/// What `RET` visits: a file of the work tree, by its path from the root, at a line, from 1, and
/// a column, from 0, if a hunk's line named one.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Visit<'a> {
    pub(crate) path: &'a [u8],
    pub(crate) at: Option<(usize, usize)>,
}

/// Where a cursor is, by what it is on, so that it can be found in another text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    row: usize,
    section: Option<Section>,
    path: Option<Vec<u8>>,
    file: Option<usize>,
    hunk: Option<usize>,
    line: Option<usize>,
}

impl Built {
    /// The files of `section`.
    pub(crate) fn files(&self, section: Section) -> &[File] {
        (self.sections.iter())
            .find(|(of, _)| *of == section)
            .map_or(&[], |(_, files)| files.as_slice())
    }

    /// The file a row names, if any.
    pub(crate) fn file(&self, section: Section, file: usize) -> Option<&File> {
        self.files(section).get(file)
    }

    /// What `RET` on `line` at `column` visits: the file's path, and for a hunk's header or line
    /// the line of the file it shows, from 1, and the column, from 0; or why there is none.
    pub(crate) fn visit(&self, line: usize, column: usize) -> Result<Visit<'_>, &'static str> {
        let row = self.rows.get(line).copied().unwrap_or(Row::Other);
        let (Some(section), Some(index)) = (row.section(), row.file()) else {
            return Err("No file on this line");
        };
        let file = self.file(section, index).ok_or("No file on this line")?;
        if file.deleted {
            return Err("That file is deleted");
        }
        let hunk =
            (row.hunk()).and_then(|hunk| self.patches.get(&(section, index))?.hunks.get(hunk));
        let at = match (row, hunk) {
            (Row::Line { line, .. }, Some(hunk)) => {
                Some((hunk.new_line(line), column.saturating_sub(1)))
            }
            (_, Some(hunk)) => Some((hunk.new_line(0), 0)),
            _ => None,
        };
        Ok(Visit {
            path: &file.path,
            at: at.map(|(line, column)| (usize::try_from(line).unwrap_or(usize::MAX), column)),
        })
    }

    /// What `line` is on, to find it again in another text.
    pub(crate) fn place(&self, line: usize) -> Place {
        let row = self.rows.get(line).copied().unwrap_or(Row::Other);
        let section = row.section();
        let file = row.file();
        let path = section
            .zip(file)
            .and_then(|(section, file)| self.file(section, file))
            .map(|file| file.path.clone());
        let line_in_hunk = match row {
            Row::Line { line, .. } => Some(line),
            _ => None,
        };
        Place {
            row: line,
            section,
            path,
            file,
            hunk: row.hunk(),
            line: line_in_hunk,
        }
    }

    /// The line of this text that `place`, from another, is on: the same hunk's line, hunk or
    /// file, else the file now at its place in the section, else the section's heading, else
    /// the same line, as far as the text goes.
    pub(crate) fn line_of(&self, place: &Place) -> usize {
        let last = self.rows.len().saturating_sub(1);
        let Some(section) = place.section else {
            return place.row.min(last);
        };
        if let Some(path) = &place.path
            && let Some(&(file, row)) = self.file_rows.get(&(section, path.clone()))
        {
            let Some(hunk) = place.hunk else {
                return row;
            };
            let Some(hunks) = self.hunk_rows.get(&(section, file)) else {
                return row;
            };
            let Some(&header) = hunks.get(hunk.min(hunks.len().saturating_sub(1))) else {
                return row;
            };
            let Some(line) = place.line.filter(|_| hunk < hunks.len()) else {
                return header;
            };
            let end = hunks.get(hunk + 1).copied().unwrap_or(self.rows.len());
            return (header + 1 + line).min(end.saturating_sub(1));
        }
        let files = self.files(section);
        if let Some(index) = place.file
            && !files.is_empty()
        {
            let file = &files[index.min(files.len() - 1)];
            if let Some(&(_, row)) = self.file_rows.get(&(section, file.path.clone())) {
                return row;
            }
        }
        if let Some(&row) = self.headings.get(&section) {
            return row;
        }
        // The section is gone: the first heading there is, else the same line.
        (self.headings.values().min().copied()).unwrap_or(place.row.min(last))
    }
}

mod write;

pub(crate) use write::{Laid, lay_out};

#[cfg(test)]
mod tests;
