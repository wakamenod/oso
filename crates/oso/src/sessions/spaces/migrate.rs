//! The spaces made once from the sessions an earlier oso remembered, when there is no `spaces`
//! file yet (ADR 0136): each repository the sessions ran in becomes a space of that one
//! repository, named after it, with no orchestrator, and each session belongs to its
//! repository's space. Nothing starts, and no directory is made.
//!
//! Must not depend on: threads, the UI or `app`; it touches the disk, so it runs on the
//! sessions worker.
//!
//! A session's place is its project root, else its directory. A place whose `.git` is a file is
//! a worktree: it goes with the repository above it that records it back, as oso's own
//! worktrees under `.claude/worktrees` are, or with another remembered place that does
//! (ADR 0114's check, [`linked_worktree`]). Only paths oso remembered, and the directories
//! above them, are looked at; nothing a repository wrote is followed first.

use std::fs;
use std::path::{Path, PathBuf};

use super::dir::identity;
use super::stored::{Member, SpaceId, Stored, last_component, same, slug, unique};
use crate::project::linked_worktree;
use crate::sessions::record::Remembered;

/// How many directories above a worktree are looked at for its repository.
const MAX_UP: usize = 64;

/// The spaces of `sessions`' repositories, each with an id from `ids`, as of `now`; each
/// session's space is set. A session whose space no id was left for keeps none, and joins one
/// when it is next shown.
pub fn from_sessions(
    sessions: &mut [Remembered],
    ids: &mut dyn FnMut() -> Option<SpaceId>,
    now: u64,
) -> Vec<Stored> {
    let places: Vec<PathBuf> = sessions.iter().map(|s| place(s).to_owned()).collect();
    let mut spaces: Vec<Stored> = Vec::new();
    for session in sessions.iter_mut() {
        let repo = repository_of(place(session), &places);
        let at = match spaces.iter().position(|space| space.has_member(&repo)) {
            Some(at) => at,
            None => {
                let Some(id) = ids() else {
                    continue;
                };
                let taken = |name: &str| spaces.iter().any(|space| space.name == name);
                let name = unique(&slug(&last_component(&repo)), &taken);
                let member = Member {
                    identity: fs::symlink_metadata(&repo).ok().and_then(|m| identity(&m)),
                    root: repo,
                };
                spaces.push(Stored::of_repository(id, name, member, now));
                spaces.len() - 1
            }
        };
        let place = place(session).to_owned();
        let space = &mut spaces[at];
        if !space.has_member(&place) && !space.holds_worktree(&place) {
            space.worktrees.push(place);
        }
        session.space = Some(space.id.clone());
    }
    spaces
}

/// Where `session` runs, for its space: its project root, else its directory.
fn place(session: &Remembered) -> &Path {
    session.root.as_deref().unwrap_or(&session.directory)
}

/// The repository whose main checkout `place` is, or whose worktree it is, among the
/// directories above it and the other `places`; else `place` itself.
fn repository_of(place: &Path, places: &[PathBuf]) -> PathBuf {
    let git = fs::symlink_metadata(place.join(".git"));
    if !git.is_ok_and(|meta| meta.is_file()) {
        return place.to_owned();
    }
    let holds_repository =
        |dir: &Path| fs::symlink_metadata(dir.join(".git")).is_ok_and(|meta| meta.is_dir());
    let above = place
        .ancestors()
        .skip(1)
        .take(MAX_UP)
        .find(|dir| holds_repository(dir) && linked_worktree(dir, place).is_some());
    if let Some(main) = above {
        return main.to_owned();
    }
    places
        .iter()
        .find(|main| !same(main, place) && linked_worktree(main, place).is_some())
        .cloned()
        .unwrap_or_else(|| place.to_owned())
}

#[cfg(all(test, unix))]
mod tests;
