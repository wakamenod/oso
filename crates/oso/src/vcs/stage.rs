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

use std::collections::{BTreeMap, BTreeSet};

use super::cut;
use super::layout::{Built, Row, Section};

/// What a command means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    /// The lines `first..=last` of a hunk, by their index in it, that a region holds without
    /// its header (ADR 0127, step 1c).
    Lines {
        section: Section,
        file: usize,
        hunk: usize,
        first: usize,
        last: usize,
    },
}

/// The targets `lines` name in `built`, each once, those a section or a file holds folded into
/// it. O(lines) for the rows the regions hold, and O(n log n) for the targets found.
pub(crate) fn targets(built: &Built, lines: &[GitLines]) -> Result<Vec<Target>, &'static str> {
    let mut found = Vec::new();
    for one in lines {
        if one.region && one.last > one.first {
            region(built, one.first, one.last, &mut found)?;
        } else if let Some(row) = built.rows.get(one.first)
            && let Some(target) = target_of(*row, true)
        {
            found.push(target);
        }
    }
    found.sort_unstable();
    found.dedup();
    let set: std::collections::HashSet<Target> = found.iter().copied().collect();
    found.retain(|target| !parents(*target).iter().flatten().any(|p| set.contains(p)));
    Ok(found)
}

/// The headings rows `first..=last` hold, in order: the rows are in the text's order, so a hunk's
/// line is held when its hunk's, its file's or its section's heading came before it in the
/// region.
fn region(
    built: &Built,
    first: usize,
    last: usize,
    found: &mut Vec<Target>,
) -> Result<(), &'static str> {
    let mut held: [Option<Target>; 3] = [None; 3];
    for row in (first..=last).filter_map(|line| built.rows.get(line).copied()) {
        if let Row::Line {
            section,
            file,
            hunk,
            line,
        } = row
        {
            let whole = Target::Hunk {
                section,
                file,
                hunk,
            };
            let mut holders = parents(whole).into_iter().flatten().chain([whole]);
            if holders.any(|holder| held.contains(&Some(holder))) {
                continue;
            }
            // A line of a hunk whose header the region does not hold: its run of lines grows.
            match found.last_mut() {
                Some(Target::Lines {
                    section: s,
                    file: f,
                    hunk: h,
                    last,
                    ..
                }) if (*s, *f, *h) == (section, file, hunk) && *last + 1 == line => *last = line,
                _ => found.push(Target::Lines {
                    section,
                    file,
                    hunk,
                    first: line,
                    last: line,
                }),
            }
            continue;
        }
        let Some(target) = target_of(row, false) else {
            continue;
        };
        match target {
            Target::Section(_) => held = [Some(target), None, None],
            Target::File { .. } => held[1..].copy_from_slice(&[Some(target), None]),
            Target::Hunk { .. } => held[2] = Some(target),
            // A row's target is never lines: a hunk's line names lines only above.
            Target::Lines { .. } => continue,
        }
        found.push(target);
    }
    Ok(())
}

/// What holds `target`: its section's heading, for a hunk its file, and for lines their hunk.
fn parents(target: Target) -> [Option<Target>; 3] {
    match target {
        Target::Section(_) => [None, None, None],
        Target::File { section, .. } => [Some(Target::Section(section)), None, None],
        Target::Hunk { section, file, .. } => [
            Some(Target::Section(section)),
            Some(Target::File { section, file }),
            None,
        ],
        Target::Lines {
            section,
            file,
            hunk,
            ..
        } => [
            Some(Target::Section(section)),
            Some(Target::File { section, file }),
            Some(Target::Hunk {
                section,
                file,
                hunk,
            }),
        ],
    }
}

/// What a row names: a heading its section, a file's row or note the file, a hunk's header
/// the hunk, and, for a cursor, a hunk's line its hunk.
fn target_of(row: Row, cursor: bool) -> Option<Target> {
    match row {
        Row::Heading(section) => Some(Target::Section(section)),
        Row::File { section, file }
        | Row::Detail {
            section,
            file,
            whole: true,
        } => Some(Target::File { section, file }),
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
        Row::Line { .. } | Row::Detail { .. } | Row::Other | Row::More(_) => None,
    }
}

/// What `S` (`stage`) or `U` stages or unstages: every unstaged change of tracked files, or every
/// staged one; or why there is none.
pub(crate) fn all(built: &Built, stage: bool) -> Result<Target, &'static str> {
    let section = if stage {
        Section::Unstaged
    } else {
        Section::Staged
    };
    if built.files(section).is_empty() {
        return Err(if stage {
            "No unstaged changes"
        } else {
            "No staged changes"
        });
    }
    Ok(Target::Section(section))
}

/// A git command that stages or unstages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// `git add` of these untracked paths.
    Add(Vec<Vec<u8>>),
    /// `git add --update` of these tracked paths: only what git tracks under each, so that an
    /// untracked file under a path that became a directory is never added with it.
    Update(Vec<Vec<u8>>),
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
        mut untracked,
        patches,
        files,
        hunks,
        lines,
    } = picked;
    let mut ops = Vec::new();
    for list in [&mut untracked, &mut paths] {
        list.sort_unstable();
        list.dedup();
    }
    if !untracked.is_empty() {
        ops.push(Op::Add(untracked));
    }
    if !paths.is_empty() {
        ops.push(if stage {
            Op::Update(paths)
        } else {
            Op::Unstage(paths)
        });
    }
    for (section, file, picks) in patches {
        let Some(patch) = built.patches.get(&(section, file)) else {
            continue;
        };
        let picks: Vec<(usize, Option<Vec<usize>>)> = (picks.into_iter())
            .map(|(hunk, chosen)| (hunk, chosen.map(|set| set.into_iter().collect())))
            .collect();
        if let Some(bytes) = cut::file_patch(patch, &picks, !stage) {
            ops.push(Op::Apply {
                patch: bytes,
                reverse: !stage,
            });
        }
    }
    if ops.is_empty() {
        return Err("Nothing here to stage or unstage".to_owned());
    }
    Ok((ops, said(stage, files, hunks, lines)))
}

/// What the echo area says once `files` files, `hunks` hunks and `lines` lines of others were
/// staged or unstaged.
fn said(stage: bool, files: usize, hunks: usize, lines: usize) -> String {
    let verb = if stage { "Staged" } else { "Unstaged" };
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("a {one}")),
        n => Some(format!("{n} {many}")),
    };
    let parts: Vec<String> = [
        count(files, "file", "files"),
        count(hunks, "hunk", "hunks"),
        count(lines, "hunk's lines", "hunks' lines"),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!("{verb} {}", parts.join(" and "))
}

/// A file's hunks picked, each whole (`None`) or the lines of it chosen.
type Picks = BTreeMap<usize, Option<BTreeSet<usize>>>;

/// The files and hunks picked so far.
#[derive(Default)]
struct Picked {
    /// Tracked paths, staged with `--update` or unstaged.
    paths: Vec<Vec<u8>>,
    /// Untracked paths, added.
    untracked: Vec<Vec<u8>>,
    /// Each file's hunks, by section and file: each whole (`None`) or the lines chosen of it.
    patches: Vec<(Section, usize, Picks)>,
    files: usize,
    hunks: usize,
    /// The hunks of which lines were chosen.
    lines: usize,
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
            } if fits(section) => self.hunk(built, (section, file, hunk), None)?,
            Target::Lines {
                section,
                file,
                hunk,
                first,
                last,
            } if fits(section) => self.hunk(built, (section, file, hunk), Some(first..=last))?,
            Target::Section(_)
            | Target::File { .. }
            | Target::Hunk { .. }
            | Target::Lines { .. } => {
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

    /// Picks the hunk `hunk` of a file, whole or, with `lines`, those of its lines; or says why
    /// it cannot be taken by itself.
    fn hunk(
        &mut self,
        built: &Built,
        (section, file, hunk): (Section, usize, usize),
        lines: Option<std::ops::RangeInclusive<usize>>,
    ) -> Result<(), String> {
        {
            {
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
            }
        }
        let at = (self.patches.iter()).position(|(s, f, _)| (*s, *f) == (section, file));
        let at = at.unwrap_or_else(|| {
            self.patches.push((section, file, BTreeMap::new()));
            self.patches.len() - 1
        });
        let picks = &mut self.patches[at].2;
        match (picks.get_mut(&hunk), lines) {
            // A hunk taken whole takes every line of it too.
            (Some(None), _) => {}
            (Some(Some(chosen)), Some(lines)) => chosen.extend(lines),
            (Some(slot @ Some(_)), None) => {
                *slot = None;
                self.lines -= 1;
                self.hunks += 1;
            }
            (None, Some(lines)) => {
                picks.insert(hunk, Some(lines.collect()));
                self.lines += 1;
            }
            (None, None) => {
                picks.insert(hunk, None);
                self.hunks += 1;
            }
        }
        Ok(())
    }

    fn file(&mut self, built: &Built, section: Section, index: usize, stage: bool) {
        let Some(file) = built.file(section, index) else {
            return;
        };
        self.files += 1;
        if section == Section::Untracked {
            self.untracked.push(file.path.clone());
        } else {
            self.paths.push(file.path.clone());
        }
        if !stage && let Some(from) = &file.from {
            self.paths.push(from.clone());
        }
    }
}

#[cfg(test)]
mod tests;
