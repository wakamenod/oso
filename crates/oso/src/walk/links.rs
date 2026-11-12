//! Where a link in a project leads, found by its words (ADR 0114): each name on the way is looked
//! at in the directory before it, opened from that one without following a link, and a link's
//! target is read and taken by its words, only while the way stays under the project's root.
//! Nothing out of the root is looked at.
//!
//! Must not depend on: `app`, winit or the UI thread; this touches the file system.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path};

use oso_platform::file_meta::{Directory, EntryKind};

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
    /// The walk has looked at as many names as it may.
    Spent,
}

/// Where the way stands: the names from the root to the directory it is in, and that
/// directory, open, unless it is the root.
struct Place<'a> {
    root: &'a Directory,
    names: Vec<OsString>,
    here: Option<Directory>,
}

impl Place<'_> {
    fn directory(&self) -> &Directory {
        self.here.as_ref().unwrap_or(self.root)
    }

    /// Goes back to the root.
    fn back_to_root(&mut self) {
        self.names.clear();
        self.here = None;
    }

    /// Goes up a directory: opened again from the root, name by name, each a look. `false`
    /// when the way is gone since.
    fn up(&mut self, looks: &mut usize) -> Result<bool, Leads> {
        self.names.pop();
        let names = std::mem::take(&mut self.names);
        self.here = None;
        for name in names {
            look(looks)?;
            if !self.down(name) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Goes into the directory `name`, opened from here without following a link. `false` if
    /// it is not one now.
    fn down(&mut self, name: OsString) -> bool {
        match self.directory().open_directory(&name) {
            Ok(Some(next)) => {
                self.here = Some(next);
                self.names.push(name);
                true
            }
            _ => false,
        }
    }
}

/// Takes one of `looks`, or says none is left.
fn look(looks: &mut usize) -> Result<(), Leads> {
    *looks = looks.checked_sub(1).ok_or(Leads::Spent)?;
    Ok(())
}

/// Where the link at `link`, a path from the project's root `root_path`, which `root` is open
/// at, leads. Each name on the way is looked at without following it, and each link's target
/// is read and taken by its words, only while the way stays under the root. Each look takes one
/// of `looks`, which the whole walk shares; with none left, it leads to [`Leads::Spent`].
pub(super) fn resolve(root: &Directory, root_path: &Path, link: &Path, looks: &mut usize) -> Leads {
    match follow(root, root_path, link, looks) {
        Ok(leads) | Err(leads) => leads,
    }
}

fn follow(
    root: &Directory,
    root_path: &Path,
    link: &Path,
    looks: &mut usize,
) -> Result<Leads, Leads> {
    let mut ahead: VecDeque<OsString> = VecDeque::new();
    if !push_ahead(&mut ahead, link) {
        return Ok(Leads::Outside);
    }
    let mut place = Place {
        root,
        names: Vec::new(),
        here: None,
    };
    let mut hops = 0;
    while let Some(name) = ahead.pop_front() {
        if name == ".." {
            if place.names.is_empty() {
                return Ok(Leads::Outside);
            }
            if !place.up(looks)? {
                return Ok(Leads::Nowhere);
            }
            continue;
        }
        look(looks)?;
        let Ok(kind) = place.directory().kind_of(&name) else {
            return Ok(Leads::Nowhere);
        };
        match kind {
            EntryKind::Link => {}
            EntryKind::Directory if !ahead.is_empty() => {
                // The look above was this one's.
                if !place.down(name) {
                    return Ok(Leads::Nowhere);
                }
                continue;
            }
            _ if ahead.is_empty() => return Ok(Leads::Inside(kind == EntryKind::File)),
            // A way through something that holds no names.
            _ => return Ok(Leads::Nowhere),
        }
        hops += 1;
        let Ok(target) = place.directory().read_link(&name) else {
            return Ok(Leads::Nowhere);
        };
        if hops > MAX_HOPS {
            return Ok(Leads::Nowhere);
        }
        let target = if target.has_root() {
            // Taken from the root again, if it names a place under it by its words.
            match target.strip_prefix(root_path) {
                Ok(under) => {
                    place.back_to_root();
                    under.to_path_buf()
                }
                Err(_) => return Ok(Leads::Outside),
            }
        } else {
            target
        };
        let mut rest = VecDeque::new();
        if !push_ahead(&mut rest, &target) {
            return Ok(Leads::Outside);
        }
        rest.extend(ahead);
        ahead = rest;
    }
    Ok(Leads::Inside(false))
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
