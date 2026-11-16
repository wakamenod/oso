//! What discarding (`k`, ADR 0127, step 1c) runs for what the cursors name, what it keeps a
//! copy of first, and what its question says.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions of a
//! status's table.
//!
//! - Unstaged: a file is restored from the index (`git restore --worktree`), and its hunks or
//!   lines go back through `git apply -R`.
//! - Staged (the author's choice, as magit's): a file is restored in the index and the work
//!   tree from `HEAD`, a new file taken out of the index and deleted, and hunks or lines go back
//!   from both through `git apply --index -R`.
//! - Untracked files are deleted, at most [`MAX_DELETED`] of them; their size is the thread's to
//!   look at.
//! - Refused: an unmerged path (resolve it first), a submodule ("use a shell"), and a staged
//!   rename or copy (`u` it first).
//! - Every file it touches is copied first, but one that may hold secrets, which the question
//!   names: a loose object can be read more widely than a private file.

use std::collections::{BTreeMap, BTreeSet};

use super::cut;
use super::layout::{Built, File, SECRET, Section};
use super::stage::Target;

/// The most untracked files one discard deletes.
pub(crate) const MAX_DELETED: usize = 100;
/// The most files one discard keeps copies of: past it, nothing is planned.
pub(crate) const MAX_COPIED: usize = 1000;

/// One thing a discard runs, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// `git apply -R` of `patch`, to the work tree, or with `cached` to the index alone: a
    /// staged hunk goes from the index, then the work tree, as magit's does, so that unstaged
    /// changes elsewhere in the file do not stop it.
    Reverse { patch: Vec<u8>, cached: bool },
    /// `git restore --worktree` of these paths, from the index.
    Restore(Vec<Vec<u8>>),
    /// `git restore --staged --worktree --source=HEAD` of these paths.
    RestoreHead(Vec<Vec<u8>>),
    /// `git rm --cached` of these new paths, which are then deleted.
    Unadd(Vec<Vec<u8>>),
    /// Deleting these files of the work tree.
    Delete(Vec<Vec<u8>>),
}

/// What a discard runs: the files to copy first, each with whether it may hold secrets, which
/// gets no copy; the steps; and what the echo area says once it ran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) copies: Vec<(Vec<u8>, bool)>,
    pub(crate) steps: Vec<Step>,
    pub(crate) said: String,
}

/// A file's hunks chosen, each whole (`None`) or the lines of it.
type Picks = BTreeMap<usize, Option<BTreeSet<usize>>>;

/// What discarding `targets` of `built` runs, or why it does not.
pub(crate) fn plan(built: &Built, targets: &[Target]) -> Result<Plan, String> {
    let mut plan = Plan::default();
    let (mut restore, mut head, mut unadd, mut delete) = (vec![], vec![], vec![], vec![]);
    let mut patches: BTreeMap<(Section, usize), Picks> = BTreeMap::new();
    let mut copies: BTreeMap<Vec<u8>, bool> = BTreeMap::new();
    let (mut files, mut hunks) = (0usize, 0usize);
    let mut file = |section: Section, index: usize| -> Result<(), String> {
        let Some(found) = built.file(section, index) else {
            return Ok(());
        };
        refuse(section, found)?;
        files += 1;
        if !found.deleted {
            copies.insert(found.path.clone(), found.whole_only == Some(SECRET));
        }
        match section {
            Section::Untracked => delete.push(found.path.clone()),
            Section::Unstaged => restore.push(found.path.clone()),
            Section::Staged if found.letters[0] == b'A' => unadd.push(found.path.clone()),
            Section::Staged => head.push(found.path.clone()),
            Section::Unmerged | Section::Unpushed | Section::Unpulled | Section::Recent => {}
        }
        Ok(())
    };
    for target in targets {
        match *target {
            Target::Section(section) => {
                for index in 0..built.files(section).len() {
                    file(section, index)?;
                }
            }
            Target::File { section, file: at } => file(section, at)?,
            Target::Hunk {
                section,
                file: at,
                hunk,
            } => pick(built, &mut patches, (section, at, hunk), None)?,
            Target::Lines {
                section,
                file: at,
                hunk,
                first,
                last,
            } => pick(built, &mut patches, (section, at, hunk), Some(first..=last))?,
        }
    }
    if delete.len() + unadd.len() > MAX_DELETED {
        return Err(format!(
            "That deletes more than {MAX_DELETED} files: use a shell"
        ));
    }
    hunks += reverses(built, patches, &mut copies, &mut plan.steps);
    for (paths, step) in [
        (restore, Step::Restore as fn(Vec<Vec<u8>>) -> Step),
        (head, Step::RestoreHead),
        (unadd.clone(), Step::Unadd),
        (delete.into_iter().chain(unadd).collect(), Step::Delete),
    ] {
        if !paths.is_empty() {
            plan.steps.push(step(paths));
        }
    }
    if plan.steps.is_empty() {
        return Err("Nothing here to discard".to_owned());
    }
    if copies.len() > MAX_COPIED {
        return Err(format!(
            "That keeps more than {MAX_COPIED} files: use a shell"
        ));
    }
    plan.copies = copies.into_iter().collect();
    plan.said = said(files, hunks, plan.copies.iter().any(|(_, secret)| !secret));
    Ok(plan)
}

/// Adds the reverse patch of each file's `patches` to `steps`, and its file to `copies`; says
/// how many hunks they hold.
fn reverses(
    built: &Built,
    patches: BTreeMap<(Section, usize), Picks>,
    copies: &mut BTreeMap<Vec<u8>, bool>,
    steps: &mut Vec<Step>,
) -> usize {
    let mut hunks = 0;
    for ((section, at), picks) in patches {
        let (Some(found), Some(patch)) =
            (built.file(section, at), built.patches.get(&(section, at)))
        else {
            continue;
        };
        let picks: Vec<(usize, Option<Vec<usize>>)> = (picks.into_iter())
            .map(|(hunk, chosen)| (hunk, chosen.map(|set| set.into_iter().collect())))
            .collect();
        let Some(bytes) = cut::file_patch(patch, &picks, true) else {
            continue;
        };
        hunks += picks.len();
        copies.insert(found.path.clone(), found.whole_only == Some(SECRET));
        if section == Section::Staged {
            steps.push(Step::Reverse {
                patch: bytes.clone(),
                cached: true,
            });
        }
        steps.push(Step::Reverse {
            patch: bytes,
            cached: false,
        });
    }
    hunks
}

/// Why `file` of `section` cannot be discarded, if it cannot.
fn refuse(section: Section, file: &File) -> Result<(), String> {
    if section == Section::Unmerged {
        return Err("An unmerged path: resolve it first".to_owned());
    }
    if file.whole_only == Some("a submodule") {
        return Err("A submodule: use a shell".to_owned());
    }
    if section == Section::Staged && file.from.is_some() {
        return Err("A staged rename: u unstages it first, then k".to_owned());
    }
    Ok(())
}

/// Picks hunk `hunk` of a file, whole or the `lines` of it, or says why it cannot be by itself.
fn pick(
    built: &Built,
    patches: &mut BTreeMap<(Section, usize), Picks>,
    (section, file, hunk): (Section, usize, usize),
    lines: Option<std::ops::RangeInclusive<usize>>,
) -> Result<(), String> {
    let found = built.file(section, file).ok_or("That file is gone")?;
    refuse(section, found)?;
    let patch = built.patches.get(&(section, file));
    let why = (found.whole_only)
        .or_else(|| patch.and_then(|patch| patch.whole_only))
        .or_else(|| {
            (patch.and_then(|patch| patch.hunks.get(hunk)))
                .filter(|hunk| !hunk.whole)
                .map(|_| "a hunk git's counts do not match")
        });
    if let Some(why) = why {
        return Err(format!(
            "Not by hunk: {why}; put point on the file's row to discard it whole"
        ));
    }
    let picks = patches.entry((section, file)).or_default();
    match (picks.get_mut(&hunk), lines) {
        (Some(None), _) => {}
        (Some(Some(chosen)), Some(lines)) => chosen.extend(lines),
        (Some(slot), None) => *slot = None,
        (None, lines) => {
            picks.insert(hunk, lines.map(Iterator::collect));
        }
    }
    Ok(())
}

/// What the echo area says once `files` files and `hunks` hunks were discarded, and whether a
/// copy was kept.
fn said(files: usize, hunks: usize, kept: bool) -> String {
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("a {one}")),
        n => Some(format!("{n} {many}")),
    };
    let what: Vec<String> = [count(files, "file", "files"), count(hunks, "hunk", "hunks")]
        .into_iter()
        .flatten()
        .collect();
    let back = if kept {
        ": M-x git-discarded brings it back"
    } else {
        ""
    };
    format!("Discarded {}{back}", what.join(" and "))
}

/// Why discarding `targets` is refused before the question, if it is: an untracked section
/// past [`MAX_DELETED`], which costs nothing to tell.
pub(crate) fn too_many(built: &Built, targets: &[Target]) -> Option<String> {
    let deleted: usize = (targets.iter())
        .filter(|target| **target == Target::Section(Section::Untracked))
        .map(|_| built.files(Section::Untracked).len())
        .sum();
    let sections: usize = (targets.iter())
        .filter_map(|target| match target {
            Target::Section(section) => Some(built.files(*section).len()),
            _ => None,
        })
        .sum();
    if deleted > MAX_DELETED {
        Some(format!(
            "That deletes more than {MAX_DELETED} files: use a shell"
        ))
    } else if sections > MAX_COPIED {
        Some(format!(
            "That keeps more than {MAX_COPIED} files: use a shell"
        ))
    } else {
        None
    }
}

/// What the question before discarding `targets` of `built` asks: what goes, with the one
/// file's name if there is one, and a word on files that get no copy.
pub(crate) fn question(built: &Built, targets: &[Target]) -> String {
    let mut files: BTreeSet<(Section, usize)> = BTreeSet::new();
    let (mut whole, mut hunks, mut lines, mut untracked) = (0usize, 0usize, 0usize, 0usize);
    // A section's files are counted, not each put in the set: it may hold very many.
    let (mut in_sections, mut secret) = (0usize, 0usize);
    for target in targets {
        match *target {
            Target::Section(section) => {
                let all = built.files(section);
                in_sections += all.len();
                whole += all.len();
                untracked += if section == Section::Untracked {
                    all.len()
                } else {
                    0
                };
                secret += all
                    .iter()
                    .filter(|file| file.whole_only == Some(SECRET))
                    .count();
            }
            Target::File { section, file } => {
                files.insert((section, file));
                whole += 1;
                untracked += usize::from(section == Section::Untracked);
            }
            Target::Hunk { section, file, .. } => {
                files.insert((section, file));
                hunks += 1;
            }
            Target::Lines { section, file, .. } => {
                files.insert((section, file));
                lines += 1;
            }
        }
    }
    let named: Vec<&File> = (files.iter())
        .filter_map(|(section, index)| built.file(*section, *index))
        .collect();
    secret += (named.iter())
        .filter(|file| file.whole_only == Some(SECRET))
        .count();
    let name = |file: &File| oso_core::editor::row_text(&String::from_utf8_lossy(&file.path));
    let one = (named.len() + in_sections == 1).then(|| {
        named.first().map(|file| name(file)).unwrap_or_else(|| {
            (targets.iter())
                .find_map(|target| match target {
                    Target::Section(section) => built.files(*section).first(),
                    _ => None,
                })
                .map(name)
                .unwrap_or_default()
        })
    });
    let mut ask = if untracked == whole && hunks + lines == 0 {
        match (&one, untracked) {
            (Some(one), 1) => format!("Delete untracked {one}"),
            (_, n) => format!("Delete {n} untracked files"),
        }
    } else {
        let count = |n: usize, one: &str, many: &str| match n {
            0 => None,
            1 => Some(one.to_owned()),
            n => Some(format!("{n} {many}")),
        };
        let what: Vec<String> = [
            count(whole, "the file's changes", "files' changes"),
            count(hunks, "a hunk", "hunks"),
            count(lines, "a hunk's lines", "hunks' lines"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let place = one.map_or_else(String::new, |one| format!(" in {one}"));
        format!("Discard {}{place}", what.join(" and "))
    };
    ask.push('?');
    // Said first, so that no name, however long, can push it out of sight.
    if secret > 0 {
        ask.insert_str(0, "No copy of what may hold secrets. ");
    }
    ask
}

#[cfg(test)]
mod tests;
