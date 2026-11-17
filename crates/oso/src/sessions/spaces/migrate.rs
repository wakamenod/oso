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

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use oso_core::project::MAX_GIT_FILE;

use super::dir::identity;
use super::stored::{Member, SpaceId, Stored, last_component, same, slug, unique};
use crate::project::linked_worktree;
use crate::sessions::record::Remembered;

/// How many directories above a worktree are looked at for the repository it names.
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
    let mut repositories: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut spaces: Vec<Stored> = Vec::new();
    for session in sessions.iter_mut() {
        let at_place = place(session);
        let repo = match repositories.get(at_place) {
            Some(repo) => repo.clone(),
            None => {
                let repo = repository_of(at_place, &places);
                repositories.insert(at_place.to_owned(), repo.clone());
                repo
            }
        };
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
/// directories above it and the other `places`; else `place` itself. A worktree's `.git` is
/// read once, for the repository its words name, and that one alone is then checked.
fn repository_of(place: &Path, places: &[PathBuf]) -> PathBuf {
    let git = fs::symlink_metadata(place.join(".git"));
    if !git.is_ok_and(|meta| meta.is_file()) {
        return place.to_owned();
    }
    let Some(claim) = read_git_file(&place.join(".git"))
        .and_then(|bytes| oso_core::project::worktree_claim(place, &bytes))
    else {
        return place.to_owned();
    };
    let holds_repository = super::probe::holds_repository;
    let above = place
        .ancestors()
        .skip(1)
        .take(MAX_UP)
        .find(|dir| same(dir, &claim));
    let named = above
        .map(Path::to_path_buf)
        .or_else(|| places.iter().find(|main| same(main, &claim)).cloned());
    match named {
        Some(main) if holds_repository(&main) && linked_worktree(&main, place).is_some() => main,
        _ => place.to_owned(),
    }
}

/// At most [`MAX_GIT_FILE`] bytes and one more of the `.git` file at `path`, which is no
/// link: more is refused by the claim's reader.
fn read_git_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let (file, _) = oso_platform::file_meta::open_for_reading(path).ok()??;
    let mut bytes = Vec::new();
    file.take(MAX_GIT_FILE + 1).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(all(test, unix))]
mod tests;
