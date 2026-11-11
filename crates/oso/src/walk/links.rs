//! Where a link in a project leads, found by its words (ADR 0114): each name on the way is looked
//! at without following it, and a link's target is read and taken by its words, only while the
//! way stays under the project's root. Nothing out of the root is looked at.
//!
//! Must not depend on: `app`, winit or the UI thread; this touches the file system.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path};

/// The most links a link's path goes through before it is taken for one that leads nowhere, as
/// the system's own bound.
const MAX_HOPS: usize = 40;

/// Where a link leads, found by its words.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Leads {
    /// Under the root, to a file (`true`) or to something else.
    Inside(bool),
    /// Out of the root, at some hop: nothing there is looked at.
    Outside,
    /// Nowhere: a link of too many hops, or one that cannot be read.
    Nowhere,
}

/// Where the link at `link`, under `root`, leads. Each name on the way is looked at without
/// following it, and each link's target is read and taken by its words, only while the way
/// stays under `root`.
pub(super) fn resolve(root: &Path, link: &Path) -> Leads {
    let Ok(relative) = link.strip_prefix(root) else {
        return Leads::Outside;
    };
    let mut ahead: VecDeque<OsString> = VecDeque::new();
    let mut at = root.to_path_buf();
    if !push_ahead(&mut ahead, relative) {
        return Leads::Outside;
    }
    let mut hops = 0;
    while let Some(name) = ahead.pop_front() {
        if name == ".." {
            if at == root {
                return Leads::Outside;
            }
            at.pop();
            continue;
        }
        let next = at.join(&name);
        let Ok(metadata) = fs::symlink_metadata(&next) else {
            return Leads::Nowhere;
        };
        if !metadata.is_symlink() {
            if ahead.is_empty() {
                return Leads::Inside(metadata.is_file());
            }
            at = next;
            continue;
        }
        hops += 1;
        let Ok(target) = fs::read_link(&next) else {
            return Leads::Nowhere;
        };
        if hops > MAX_HOPS {
            return Leads::Nowhere;
        }
        let mut rest = VecDeque::new();
        let target = if target.has_root() {
            // Taken from the root again, if it names a place under it by its words.
            match target.strip_prefix(root) {
                Ok(under) => {
                    at = root.to_path_buf();
                    under.to_path_buf()
                }
                Err(_) => return Leads::Outside,
            }
        } else {
            target
        };
        if !push_ahead(&mut rest, &target) {
            return Leads::Outside;
        }
        rest.extend(ahead);
        ahead = rest;
    }
    Leads::Inside(false)
}

/// Puts the names of `path`, relative, at the end of `ahead`: `..` as one, `.` as none. `false`
/// for a path with a root or a prefix, which leaves the place it is taken from.
fn push_ahead(ahead: &mut VecDeque<OsString>, path: &Path) -> bool {
    for component in path.components() {
        match component {
            Component::Normal(name) => ahead.push_back(name.to_os_string()),
            Component::ParentDir => ahead.push_back(OsString::from("..")),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}
