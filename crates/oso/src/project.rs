//! Finding the project a file is in (ADR 0114), on the thread that loads or writes it.
//!
//! Must not depend on: `app` or the UI thread; this touches the file system.
//!
//! It asks the metadata of `.git`, `.jj` and `.hg` in each directory from the file's up, and
//! reads no file but a `.git` file and the `gitdir` file of the worktree that one names, at
//! most [`MAX_GIT_FILE`] bytes each. Nothing is run, and nothing of a repository's config is
//! read (security.md, T1).

use std::fs::{self, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use oso_core::project::{
    Found, GitMarker, MAX_GIT_FILE, Markers, RootIdentity, git_file_target, nearest_root,
    worktree_backlink, worktree_main,
};

/// The project of the file at `location`, the real place a path led, whether or not a file is
/// there yet: its directory must be its own real path, as `loader::location` gives it. `None`
/// if no directory above it holds a marker.
pub fn detect(location: &Path) -> Option<Found> {
    let start = location.parent()?;
    let (root, markers) = nearest_root(start.ancestors().map(|dir| (dir, markers_in(dir))))?;
    let metadata = fs::metadata(root).ok()?;
    let trust_root = if markers.git == Some(GitMarker::File) {
        worktree_main_of(root)
    } else {
        None
    };
    Some(Found {
        root: identity(root.to_path_buf(), &metadata),
        trust_root,
    })
}

/// Which markers `dir` holds, by their metadata alone. A link counts by where it leads, as git
/// follows one.
fn markers_in(dir: &Path) -> Markers {
    let git = match fs::metadata(dir.join(".git")) {
        Ok(metadata) if metadata.is_dir() => Some(GitMarker::Dir),
        Ok(metadata) if metadata.is_file() => Some(GitMarker::File),
        _ => None,
    };
    Markers {
        git,
        jj: fs::metadata(dir.join(".jj")).is_ok_and(|metadata| metadata.is_dir()),
        hg: fs::metadata(dir.join(".hg")).is_ok_and(|metadata| metadata.is_dir()),
    }
}

/// The main repository of the worktree at `root`, if its `.git` file names
/// `<main>/.git/worktrees/<name>` and that directory's `gitdir` file names this very `.git`
/// file back.
///
/// The link back is followed to where it leads and compared with `root/.git` as written,
/// under a root that is its own real path. So only what the main repository records decides:
/// a `.git` that is a link to a worktree's `.git` is not that worktree, however the link is
/// set up or swapped meanwhile.
fn worktree_main_of(root: &Path) -> Option<RootIdentity> {
    if fs::canonicalize(root).ok()? != root {
        return None;
    }
    let git_file = root.join(".git");
    if !fs::symlink_metadata(&git_file).is_ok_and(|metadata| metadata.is_file()) {
        return None;
    }
    let target = git_file_target(&read_small(&git_file).ok()?)?;
    let gitdir = fs::canonicalize(root.join(target)).ok()?;
    let main = worktree_main(&gitdir)?;
    let back = worktree_backlink(&read_small(&gitdir.join("gitdir")).ok()?)?;
    if fs::canonicalize(gitdir.join(back)).ok()? != git_file {
        return None;
    }
    let metadata = fs::metadata(main).ok()?;
    if !metadata.is_dir() || !fs::metadata(main.join(".git")).is_ok_and(|git| git.is_dir()) {
        return None;
    }
    Some(identity(main.to_path_buf(), &metadata))
}

/// At most one byte more than [`MAX_GIT_FILE`] of the regular file at `path`, so that a
/// longer one is refused by its parser, and never waiting on a pipe or a device.
fn read_small(path: &Path) -> io::Result<Vec<u8>> {
    let Some((file, _)) = oso_platform::file_meta::open_for_reading(path)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    };
    let mut bytes = Vec::new();
    Read::take(file, MAX_GIT_FILE + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn identity(path: PathBuf, metadata: &Metadata) -> RootIdentity {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    };
    // `std` keeps a file's identity unstable here, so the path alone is recorded (ADR 0114).
    #[cfg(not(unix))]
    let identity = {
        let _ = metadata;
        None
    };
    RootIdentity { path, identity }
}

#[cfg(test)]
mod tests;
