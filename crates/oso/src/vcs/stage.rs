//! What `s`, `u`, `S` and `U` mean in a status buffer (ADR 0127): the sections, files and hunks
//! the cursors' lines name, and the git commands that stage or unstage them.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions of
//! the text shown and its table.
//!
//! - A cursor means what its line is on: a section's heading its files, a file's row the file,
//!   a hunk's header or line the hunk. A region means every heading, file and hunk whose row it
//!   holds; a region that holds a hunk's lines without its header would mean part of a hunk,
//!   which is refused until lines can be staged (ADR 0127, step 1c).
//! - Files go to git as paths on standard input, never on the command line, so that any number
//!   fit; hunks of one file go as one patch, its header once.

use oso_core::editor::GitLines;

use super::layout::{Built, Row, Section};

/// What a command means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Target {
    Section(Section),
    File {
        section: Section,
        file: usize,
    },
    Hunk {
        section: Section,
        file: usize,
        hunk: usize,
    },
}

/// Why a part of a hunk is refused.
pub(crate) const PART_OF_A_HUNK: &str =
    "Staging part of a hunk comes later: put point on its @@ line to stage it whole";

/// The targets `lines` name in `built`, each once, those a section holds folded into it.
pub(crate) fn targets(built: &Built, lines: &[GitLines]) -> Result<Vec<Target>, &'static str> {
    let mut found = Vec::new();
    for one in lines {
        if one.region && one.last > one.first {
            let rows = (one.first..=one.last).filter_map(|line| built.rows.get(line).copied());
            let mut headers = Vec::new();
            for row in rows.clone() {
                if let Some(target) = target_of(row, false) {
                    headers.push(target);
                }
            }
            for row in rows {
                if let Row::Line {
                    section,
                    file,
                    hunk,
                    ..
                } = row
                {
                    let held = headers.iter().any(|target| {
                        holds(
                            *target,
                            Target::Hunk {
                                section,
                                file,
                                hunk,
                            },
                        )
                    });
                    if !held {
                        return Err(PART_OF_A_HUNK);
                    }
                }
            }
            found.extend(headers);
        } else if let Some(row) = built.rows.get(one.first)
            && let Some(target) = target_of(*row, true)
        {
            found.push(target);
        }
    }
    found.sort_unstable();
    found.dedup();
    let outer = found.clone();
    found.retain(|target| {
        !outer
            .iter()
            .any(|other| other != target && holds(*other, *target))
    });
    Ok(found)
}

/// What a row names: a heading its section, a file's row or note the file, a hunk's header
/// the hunk, and, for a cursor, a hunk's line its hunk.
fn target_of(row: Row, cursor: bool) -> Option<Target> {
    match row {
        Row::Heading(section) => Some(Target::Section(section)),
        Row::File { section, file } | Row::Detail { section, file } => {
            Some(Target::File { section, file })
        }
        Row::Hunk {
            section,
            file,
            hunk,
        } => Some(Target::Hunk {
            section,
            file,
            hunk,
        }),
        Row::Line {
            section,
            file,
            hunk,
            ..
        } if cursor => Some(Target::Hunk {
            section,
            file,
            hunk,
        }),
        Row::Line { .. } | Row::Other | Row::More(_) => None,
    }
}

/// Whether `outer` holds `inner`: a section its files and hunks, a file its hunks.
fn holds(outer: Target, inner: Target) -> bool {
    match (outer, inner) {
        (Target::Section(a), Target::File { section, .. } | Target::Hunk { section, .. }) => {
            a == section
        }
        (
            Target::File { section, file },
            Target::Hunk {
                section: s,
                file: f,
                ..
            },
        ) => section == s && file == f,
        _ => outer == inner,
    }
}

/// A git command that stages or unstages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// `git add` of these paths.
    Add(Vec<Vec<u8>>),
    /// `git restore --staged` of these paths, or, before the first commit, `git rm --cached`.
    Unstage(Vec<Vec<u8>>),
    /// `git apply --cached` of this patch, in reverse to unstage it.
    Apply { patch: Vec<u8>, reverse: bool },
}

/// What staging (`stage`) or unstaging the targets runs, in order, and what the echo area says
/// once it ran; or why nothing runs.
pub(crate) fn ops(
    built: &Built,
    targets: &[Target],
    stage: bool,
) -> Result<(Vec<Op>, String), String> {
    let mut picked = Picked::default();
    for target in targets {
        picked.pick(built, *target, stage)?;
    }
    let Picked {
        mut paths,
        patches,
        files,
        hunks,
    } = picked;
    let mut ops = Vec::new();
    paths.sort_unstable();
    paths.dedup();
    if !paths.is_empty() {
        ops.push(if stage {
            Op::Add(paths)
        } else {
            Op::Unstage(paths)
        });
    }
    for (section, file, mut list) in patches {
        let Some(patch) = built.patches.get(&(section, file)) else {
            continue;
        };
        list.sort_unstable();
        list.dedup();
        let mut bytes = patch.header.clone();
        for hunk in list {
            if let Some(one) = patch.hunk_patch(hunk) {
                bytes.extend_from_slice(&one[patch.header.len()..]);
            }
        }
        ops.push(Op::Apply {
            patch: bytes,
            reverse: !stage,
        });
    }
    if ops.is_empty() {
        return Err("Nothing here to stage or unstage".to_owned());
    }
    Ok((ops, said(stage, files, hunks)))
}

/// What the echo area says once `files` files and `hunks` hunks were staged or unstaged.
fn said(stage: bool, files: usize, hunks: usize) -> String {
    let verb = if stage { "Staged" } else { "Unstaged" };
    match (files, hunks) {
        (0, 1) => format!("{verb} a hunk"),
        (0, n) => format!("{verb} {n} hunks"),
        (1, 0) => format!("{verb} a file"),
        (n, 0) => format!("{verb} {n} files"),
        (f, h) => format!("{verb} {f} files and {h} hunks"),
    }
}

/// The files and hunks picked so far.
#[derive(Default)]
struct Picked {
    paths: Vec<Vec<u8>>,
    /// Each file's hunks, by section and file.
    patches: Vec<(Section, usize, Vec<usize>)>,
    files: usize,
    hunks: usize,
}

impl Picked {
    /// Picks what `target` names, or says why it cannot be staged (`stage`) or unstaged.
    fn pick(&mut self, built: &Built, target: Target, stage: bool) -> Result<(), String> {
        let fits = |section: Section| (section == Section::Staged) != stage;
        match target {
            Target::Section(section) if fits(section) => {
                for index in 0..built.files(section).len() {
                    self.file(built, section, index, stage);
                }
            }
            Target::File { section, file } if fits(section) => {
                self.file(built, section, file, stage);
            }
            Target::Hunk {
                section,
                file,
                hunk,
            } if fits(section) => {
                let whole_only = built.file(section, file).and_then(|file| file.whole_only);
                let patch = built.patches.get(&(section, file));
                let why = whole_only
                    .or_else(|| patch.and_then(|patch| patch.whole_only))
                    .or_else(|| {
                        patch
                            .and_then(|patch| patch.hunks.get(hunk))
                            .filter(|hunk| !hunk.whole)
                            .map(|_| "a hunk git's counts do not match")
                    });
                if let Some(why) = why {
                    return Err(format!(
                        "Not by hunk: {why}; put point on the file's row to take it whole"
                    ));
                }
                self.hunks += 1;
                match (self.patches.iter_mut()).find(|(s, f, _)| (*s, *f) == (section, file)) {
                    Some((_, _, list)) => list.push(hunk),
                    None => self.patches.push((section, file, vec![hunk])),
                }
            }
            Target::Section(_) | Target::File { .. } | Target::Hunk { .. } => {
                return Err(if stage {
                    "Already staged"
                } else {
                    "Not staged"
                }
                .to_owned());
            }
        }
        Ok(())
    }

    fn file(&mut self, built: &Built, section: Section, index: usize, stage: bool) {
        let Some(file) = built.file(section, index) else {
            return;
        };
        self.files += 1;
        self.paths.push(file.path.clone());
        if !stage && let Some(from) = &file.from {
            self.paths.push(from.clone());
        }
    }
}

#[cfg(test)]
mod tests;
