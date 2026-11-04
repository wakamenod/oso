//! Where a path really leads, for telling whether a file may hold secrets
//! (ADR 0044). It walks the path as `fs::canonicalize` does, but keeps each
//! symbolic link's target, so a chain through a secret path is caught wherever
//! it ends.
//!
//! Must not depend on: `app` or the UI thread; this touches the file system
//! and runs on the loading thread.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The most symbolic links one path may pass through, as in Linux.
const MAX_LINKS: usize = 40;

/// Where a path really leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Each link's target on the way, then the canonical path.
    Paths(Vec<PathBuf>),
    /// Nobody can tell: a link to nothing, a missing directory, an error.
    Unknown,
}

impl Resolved {
    /// The paths, as `Buffer::judge_secret` takes them.
    pub fn paths(&self) -> Option<&[PathBuf]> {
        match self {
            Self::Paths(paths) => Some(paths),
            Self::Unknown => None,
        }
    }
}

/// Where `path` really leads. A file that does not exist yet counts as being
/// where its directory really is; a link to nothing does not, since it may
/// point anywhere.
pub fn resolve(path: &Path) -> Resolved {
    let mut walk = Walk::default();
    let walked = std::path::absolute(path)
        .ok()
        .and_then(|path| walk.follow(&path, true));
    if walked.is_none() {
        return Resolved::Unknown;
    }
    walk.visited.push(walk.current);
    // The system's own answer too: on Windows it also expands short names
    // such as `SSH~1`, which a walk along links never sees.
    match fs::canonicalize(path) {
        Ok(canonical) if walk.visited.last() != Some(&canonical) => walk.visited.push(canonical),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Resolved::Unknown,
    }
    Resolved::Paths(walk.visited)
}

/// A walk along a path, one name at a time, following links. It is the same
/// walk `fs::canonicalize` does, but it keeps the paths it passes through.
#[derive(Default)]
struct Walk {
    /// Where the walk has got to, with no links in it.
    current: PathBuf,
    /// Each link's target, as far as it can be told from where the link is.
    visited: Vec<PathBuf>,
    links: usize,
}

impl Walk {
    /// Walks `path` from `current`. `None` if it leads nowhere. The last name
    /// may be missing only if `may_be_new`, which a link's target never is.
    fn follow(&mut self, path: &Path, may_be_new: bool) -> Option<()> {
        let mut components = path.components().peekable();
        while let Some(component) = components.next() {
            match component {
                // Pushing a root replaces the path.
                Component::Prefix(_) | Component::RootDir => self.current.push(component),
                Component::CurDir => {}
                Component::ParentDir => {
                    self.current.pop();
                }
                Component::Normal(name) => {
                    let next = self.current.join(name);
                    match fs::symlink_metadata(&next) {
                        Ok(metadata) if metadata.is_symlink() => {
                            self.links += 1;
                            if self.links > MAX_LINKS {
                                return None;
                            }
                            let target = fs::read_link(&next).ok()?;
                            self.visited.push(self.current.join(&target));
                            self.follow(&target, false)?;
                        }
                        Ok(_) => self.current = next,
                        Err(e)
                            if e.kind() == io::ErrorKind::NotFound
                                && may_be_new
                                && components.peek().is_none() =>
                        {
                            self.current = next;
                        }
                        Err(_) => return None,
                    }
                }
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for one test, with no links in its path.
    fn scratch_dir(name: &str) -> PathBuf {
        let name = format!("oso-resolve-test-{}-{name}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn resolves_a_new_file_through_its_directory() {
        let directory = scratch_dir("new");
        let new = directory.join("new-file");
        assert_eq!(resolve(&new), Resolved::Paths(vec![new.clone()]));
        assert_eq!(
            resolve(&directory.join("no-dir").join("file")),
            Resolved::Unknown,
            "fails closed"
        );
        // Windows reports this as a missing file, which a new file may be.
        #[cfg(unix)]
        {
            fs::write(&new, b"x").unwrap();
            assert_eq!(
                resolve(&new.join("file")),
                Resolved::Unknown,
                "a file is not a directory"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    mod links {
        use super::*;
        use oso_core::buffer::Buffer;
        use std::os::unix::fs::symlink;

        /// Whether a buffer visiting `path` counts as secret.
        fn is_secret(path: &Path) -> bool {
            let mut buffer = Buffer::visiting(path.to_owned());
            buffer.judge_secret(resolve(path).paths());
            buffer.is_secret()
        }

        #[test]
        fn a_link_is_followed_to_its_target() {
            let directory = scratch_dir("link");
            let target = directory.join("target.txt");
            fs::write(&target, b"x").unwrap();
            let link = directory.join("link");
            symlink(&target, &link).unwrap();
            assert_eq!(
                resolve(&link),
                Resolved::Paths(vec![target.clone(), target.clone()])
            );
            assert!(!is_secret(&link));
            assert_eq!(
                resolve(&link).paths().unwrap().last(),
                Some(&fs::canonicalize(&link).unwrap()),
                "the walk ends where canonicalize does"
            );
            fs::remove_file(&target).unwrap();
            assert_eq!(resolve(&link), Resolved::Unknown, "a link to nothing");
            assert!(is_secret(&link));
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn a_link_to_a_secret_is_secret() {
            let directory = scratch_dir("to-secret");
            fs::create_dir(directory.join(".ssh")).unwrap();
            let key = directory.join(".ssh").join("deploy");
            fs::write(&key, b"x").unwrap();
            let link = directory.join("notes.txt");
            symlink(&key, &link).unwrap();
            assert!(is_secret(&link));
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn a_chain_through_a_secret_path_is_secret_wherever_it_ends() {
            let directory = scratch_dir("chain");
            let target = directory.join("aws-creds");
            fs::write(&target, b"x").unwrap();
            fs::create_dir(directory.join(".aws")).unwrap();
            let middle = directory.join(".aws").join("credentials");
            symlink(&target, &middle).unwrap();
            let link = directory.join("notes.txt");
            symlink(&middle, &link).unwrap();
            assert!(is_secret(&link));
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn a_link_to_a_secret_directory_is_secret() {
            let directory = scratch_dir("to-directory");
            let real = directory.join("dotfiles");
            fs::create_dir(&real).unwrap();
            fs::write(real.join("deploy"), b"x").unwrap();
            let ssh = directory.join(".ssh");
            symlink(&real, &ssh).unwrap();
            let link = directory.join("notes.txt");
            symlink(ssh.join("deploy"), &link).unwrap();
            assert!(is_secret(&link), "through the link named .ssh");
            assert!(is_secret(&ssh.join("deploy")), "opened through it");
            assert!(is_secret(&ssh.join("new-file")), "a new file there");
            assert!(!is_secret(&real.join("deploy")), "nothing tells");
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn relative_targets_and_parents_are_followed_from_the_link() {
            let directory = scratch_dir("relative");
            fs::create_dir(directory.join("a")).unwrap();
            fs::create_dir(directory.join("a").join("b")).unwrap();
            let target = directory.join("a").join("target.txt");
            fs::write(&target, b"x").unwrap();
            let link = directory.join("a").join("b").join("link");
            symlink("../target.txt", &link).unwrap();
            let paths = resolve(&link);
            assert_eq!(paths.paths().unwrap().last(), Some(&target));
            fs::remove_dir_all(directory).unwrap();
        }

        #[test]
        fn a_loop_of_links_fails_closed() {
            let directory = scratch_dir("loop");
            let (a, b) = (directory.join("a"), directory.join("b"));
            symlink(&b, &a).unwrap();
            symlink(&a, &b).unwrap();
            assert_eq!(resolve(&a), Resolved::Unknown);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}
