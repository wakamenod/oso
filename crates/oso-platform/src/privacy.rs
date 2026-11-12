//! Keeping what the editor holds private to its user: no core dumps, and a directory
//! only the user can reach (ADR 0028, ADR 0062, ADR 0065). Also where oso's own state and
//! configuration live, and who else can change a file read as the person's (ADR 0108).
//!
//! Must not depend on: `control`, `process` or `text`. `control` uses the checks here, so
//! that the two kinds of private directory cannot drift apart.

// The checks serve the Unix backend, and are tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "used only by the Unix backend"))]

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::file_meta::AccessEntry;

#[cfg(not(unix))]
mod other;
#[cfg(not(unix))]
use other as backend;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as backend;

#[cfg(unix)]
pub(crate) use unix::{euid, itself_problem, list_problem};

/// Turns core dumps off for this process and whatever it starts, for good: a dump would
/// hold every buffer. Windows has no such limit for a process to set, so there this does
/// nothing and succeeds. A system-wide crash dump setting stays the user's.
///
/// # Errors
///
/// If the system refuses.
pub fn disable_core_dumps() -> io::Result<()> {
    backend::disable_core_dumps()
}

/// The per-user directory for state: `$XDG_STATE_HOME` or `~/.local/state`,
/// `~/Library/Application Support` on macOS, and `%LOCALAPPDATA%` on Windows. Only an absolute
/// value counts. Asks the environment and not the disk.
pub fn state_home() -> Option<PathBuf> {
    backend::state_home_from(&|name| std::env::var_os(name))
}

/// oso's configuration directory, asking the environment and not the disk:
/// `$XDG_CONFIG_HOME/oso` when that is set and absolute, else `~/.config/oso`, on Linux and macOS,
/// and `%APPDATA%\oso` on Windows (ADR 0108). `None` with no absolute base to go by.
pub fn config_home() -> Option<PathBuf> {
    backend::config_home_from(&|name| std::env::var_os(name))
}

/// Who besides this user and root can change a file or directory, by its mode and owner, as
/// [`who_can_change`] tells, fewest first. Access lists are not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Writers {
    /// Only this user, or root.
    Nobody,
    /// The users of its group too.
    Group,
    /// Every user, or it is another user's own.
    Anyone,
}

/// Who besides this user and root can change a file, and who can change the way to it.
#[derive(Debug)]
pub struct WhoCanChange {
    /// The file's own [`Writers`].
    pub file: Writers,
    /// The most of any directory or link on the way to it, through every link: whoever can
    /// change one can put another file in its place.
    pub way: Writers,
    /// The file's metadata, through links, to tell that a file opened later is the one judged.
    pub metadata: std::fs::Metadata,
}

/// Who besides this user and root can change the file at `path`, an absolute one, and the way
/// to it (ADR 0108, ADR 0110): every directory it passes through, as written and through
/// every link, and every link's owner, from the root down, as the system walks it. In a
/// directory every user may write that keeps each entry to its owner (sticky), an entry this
/// user or root owns counts as no one else's. By mode and owner: every user if every user may
/// write it or another user owns it, else a group if its group may.
/// Windows has no modes to go by, and says [`Writers::Nobody`] for all of it.
///
/// # Errors
///
/// If something on the way cannot be looked at, is missing, or links too many times.
pub fn who_can_change(path: &Path) -> io::Result<WhoCanChange> {
    backend::who_can_change(path)
}

/// Whether a file with `metadata` is this user's. Windows has no such simple answer, and
/// relies on the per-user place the caller chose.
pub fn is_this_users(metadata: &std::fs::Metadata) -> bool {
    backend::is_this_users(metadata)
}

/// Whether every user may write the file or directory `metadata` describes, by its mode, as a
/// file read from a repository must not be (ADR 0115). Windows has no modes, and says no.
pub fn others_can_write(metadata: &std::fs::Metadata) -> bool {
    backend::others_can_write(metadata)
}

/// Whether a file with `metadata` has one name, as a hard link is a name for some other file's
/// text. Windows has no such simple answer.
pub fn has_one_name(metadata: &std::fs::Metadata) -> bool {
    backend::has_one_name(metadata)
}

/// A directory only this user can reach, checked when it is opened and created if it
/// is missing. On Unix, that means mode `0700`, this user's own, not a symbolic link, and
/// nothing that leads to it can be changed by another user. On macOS, access lists count too, and
/// only their `deny` entries are always harmless. Windows has no modes, so there it means a real
/// directory, and privacy comes from the per-user place the caller chose.
#[derive(Debug, Clone)]
pub struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    /// Opens the directory at `path`, creating it and what leads to it if missing, once
    /// [`Self::check`] has passed, so that a refused place gets nothing new. A directory
    /// found with a looser mode is refused, and not repaired, as something may already have
    /// read what is in it.
    ///
    /// # Errors
    ///
    /// If it cannot be created, or cannot be made safe. The message names the path and why.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = plain(&path.into())?;
        backend::open_private(&path)?;
        Ok(Self { path })
    }

    /// Whether the directory at `path` could not be used, as far as can be told creating
    /// nothing: [`Self::open`] would refuse it, or by its mode this user could not write to it
    /// or create it. It touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// Why it could not be used. The message names the path and why.
    pub fn check(path: &Path) -> io::Result<()> {
        backend::check_private(&plain(path)?)
    }

    /// The directory at `path`, to read what is in it, if it exists and passes what
    /// [`Self::open`] asks of one that exists. `None` if nothing is there. It creates nothing,
    /// and it does not ask whether this user could write there, so a read-only directory
    /// passes. It touches the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// If it is refused, as another user could have put anything there. The message names the
    /// path and why.
    pub fn existing(path: impl Into<PathBuf>) -> io::Result<Option<Self>> {
        let path = plain(&path.into())?;
        Ok(backend::existing_private(&path)?.then_some(Self { path }))
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// `path`, spelled so that its last name is the directory itself: a final `/` or `/.` would
/// make the system follow a link there. A relative path would be judged by whatever the current
/// directory is, and `..` makes what is written differ from what would be created.
fn plain(path: &Path) -> io::Result<PathBuf> {
    let parent_dir = path.components().any(|c| c == Component::ParentDir);
    if path.is_absolute() && !parent_dir {
        Ok(path.components().collect())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not an absolute path without ..: {}", path.display()),
        ))
    }
}

/// Why a directory cannot be private.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirProblem {
    Symlink,
    NotADirectory,
    WrongOwner,
    LooseMode,
    ListGrants,
    ListLetsChange,
    ListPassesOn,
}

impl DirProblem {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Symlink => "is a symlink",
            Self::NotADirectory => "is not a directory",
            Self::WrongOwner => "is owned by another user",
            Self::LooseMode => "is accessible by other users (mode is not 0700)",
            Self::ListGrants => "has an access list that grants access",
            Self::ListLetsChange => "has an access list that allows changes",
            Self::ListPassesOn => "has an access list subdirectories take",
        }
    }
}

/// Checks a directory's `lstat` results: it must be a real directory, owned
/// by `euid`, with no permissions for group or others.
pub(crate) fn check_dir(
    is_symlink: bool,
    is_dir: bool,
    owner: u32,
    mode: u32,
    euid: u32,
) -> Result<(), DirProblem> {
    if is_symlink {
        Err(DirProblem::Symlink)
    } else if !is_dir {
        Err(DirProblem::NotADirectory)
    } else if owner != euid {
        Err(DirProblem::WrongOwner)
    } else if mode & 0o077 != 0 {
        Err(DirProblem::LooseMode)
    } else {
        Ok(())
    }
}

/// Whether a parent directory with `mode` keeps other users from replacing
/// what is in it: not writable by group or others, or sticky.
pub(crate) fn parent_is_safe(mode: u32) -> bool {
    mode & 0o022 == 0 || mode & 0o1000 != 0
}

/// Whether a directory leading to a private one keeps other users out: safe as a parent, and
/// this user's or root's. Root owns what leads to many home directories, and can replace
/// anything anyway.
pub(crate) fn ancestor_is_safe(mode: u32, owner: u32, euid: u32) -> bool {
    parent_is_safe(mode) && (owner == euid || owner == 0)
}

/// Why the access list of a private directory keeps it from being one. It may grant nothing
/// at all, whoever an entry names: telling this user from others means asking a directory
/// service. A `deny` takes rights away, so it does no harm.
pub(crate) fn own_list_problem(entries: &[AccessEntry]) -> Option<DirProblem> {
    entries
        .iter()
        .any(|entry| entry.allows)
        .then_some(DirProblem::ListGrants)
}

/// Why the access list of a directory that leads to a private one keeps it from being one. An
/// entry that only reads is fine, as mode `0755` is.
pub(crate) fn way_list_problem(entries: &[AccessEntry]) -> Option<DirProblem> {
    entries
        .iter()
        .any(|entry| entry.allows && entry.lets_change)
        .then_some(DirProblem::ListLetsChange)
}

/// Why a private directory cannot be made under a directory with this access list. Only what
/// reaches a new directory counts: a list reaches only what is made after it, and an entry
/// for files stops at this one.
pub(crate) fn parent_list_problem(entries: &[AccessEntry]) -> Option<DirProblem> {
    entries
        .iter()
        .any(|entry| entry.allows && entry.reaches_new_directories)
        .then_some(DirProblem::ListPassesOn)
}

/// Whether this list on a private directory would stop a file being written in it, though nothing
/// is wrong with what it grants. An entry that the files take counts if it hides the list, as
/// nothing could then be told of what they took. `deny delete` and `deny list` do no such thing.
pub(crate) fn list_stops_writing(entries: &[AccessEntry]) -> bool {
    entries.iter().any(|entry| {
        !entry.allows
            && ((entry.applies_here && entry.names_writing)
                || (entry.reaches_new_files && entry.names_reading_the_list))
    })
}

/// Whether this list on a directory that leads to a private one would stop anyone going through it.
/// Only searching counts: what is in it is not being written there.
pub(crate) fn list_stops_the_way(entries: &[AccessEntry]) -> bool {
    entries
        .iter()
        .any(|entry| !entry.allows && entry.applies_here && entry.names_searching)
}

/// Whether this list on a directory would stop a private directory being made under it and used. A
/// directory takes what reaches it, so that counts as if it were on the new directory.
pub(crate) fn list_stops_making(entries: &[AccessEntry]) -> bool {
    entries.iter().any(|entry| {
        !entry.allows
            && ((entry.applies_here && entry.names_making_directories)
                || (entry.reaches_new_directories
                    && (entry.names_writing || entry.names_reading_the_list)))
    })
}

/// Why a directory cannot be the parent of a private one, whether or not that one exists yet, for
/// the control socket, which looks at one parent and not all the way.
#[cfg(unix)]
pub(crate) fn whole_parent_list_problem(entries: &[AccessEntry]) -> Option<DirProblem> {
    way_list_problem(entries).or_else(|| parent_list_problem(entries))
}

#[cfg(test)]
mod tests {
    use super::{
        AccessEntry, DirProblem, ancestor_is_safe, check_dir, list_stops_making,
        list_stops_the_way, list_stops_writing, own_list_problem, parent_is_safe,
        parent_list_problem, way_list_problem,
    };

    const NOTHING: AccessEntry = AccessEntry {
        allows: false,
        lets_change: false,
        names_writing: false,
        names_searching: false,
        names_making_directories: false,
        names_reading_the_list: false,
        names_deleting: false,
        applies_here: true,
        reaches_new_files: false,
        reaches_new_directories: false,
    };
    /// What macOS puts on `~` and on many of the directories under it.
    const DENY_DELETE: AccessEntry = AccessEntry {
        lets_change: true,
        ..NOTHING
    };
    const ALLOW_READING: AccessEntry = AccessEntry {
        allows: true,
        ..NOTHING
    };
    const ALLOW_CHANGING: AccessEntry = AccessEntry {
        allows: true,
        lets_change: true,
        ..NOTHING
    };
    const ALLOW_FOR_NEW_DIRECTORIES: AccessEntry = AccessEntry {
        allows: true,
        reaches_new_directories: true,
        ..NOTHING
    };
    const DENY_FOR_NEW_DIRECTORIES: AccessEntry = AccessEntry {
        lets_change: true,
        reaches_new_directories: true,
        ..NOTHING
    };
    const DENY_WRITING: AccessEntry = AccessEntry {
        names_writing: true,
        ..NOTHING
    };
    const DENY_SEARCHING: AccessEntry = AccessEntry {
        names_writing: true,
        names_searching: true,
        names_making_directories: true,
        ..NOTHING
    };
    const DENY_MAKING_DIRECTORIES: AccessEntry = AccessEntry {
        names_making_directories: true,
        ..NOTHING
    };
    const HIDES_THE_LIST: AccessEntry = AccessEntry {
        names_reading_the_list: true,
        applies_here: false,
        ..NOTHING
    };

    #[test]
    fn what_a_deny_stops_is_told_from_what_is_harmless() {
        // Harmless wherever it is, also on what a new file or directory takes.
        for harmless in [DENY_DELETE, DENY_FOR_NEW_DIRECTORIES, ALLOW_CHANGING] {
            assert!(!list_stops_writing(&[harmless]));
            assert!(!list_stops_the_way(&[harmless]));
            assert!(!list_stops_making(&[harmless]));
        }
        assert!(list_stops_writing(&[DENY_DELETE, DENY_WRITING]));
        assert!(!list_stops_the_way(&[DENY_WRITING]), "not going through");
        assert!(!list_stops_making(&[DENY_WRITING]), "only files");

        assert!(list_stops_writing(&[DENY_SEARCHING]));
        assert!(list_stops_the_way(&[DENY_SEARCHING]));
        assert!(list_stops_making(&[DENY_SEARCHING]));

        assert!(!list_stops_writing(&[DENY_MAKING_DIRECTORIES]));
        assert!(!list_stops_the_way(&[DENY_MAKING_DIRECTORIES]));
        assert!(list_stops_making(&[DENY_MAKING_DIRECTORIES]));
    }

    #[test]
    fn only_applying_or_being_taken_counts() {
        let only_for_what_is_made = AccessEntry {
            applies_here: false,
            ..DENY_SEARCHING
        };
        assert!(!list_stops_writing(&[only_for_what_is_made]));
        assert!(!list_stops_the_way(&[only_for_what_is_made]));
        assert!(!list_stops_making(&[only_for_what_is_made]));

        // A list that hides itself from what takes it.
        assert!(!list_stops_writing(&[HIDES_THE_LIST]), "no file takes it");
        assert!(list_stops_writing(&[AccessEntry {
            reaches_new_files: true,
            ..HIDES_THE_LIST
        }]));
        assert!(
            !list_stops_making(&[HIDES_THE_LIST]),
            "no directory takes it"
        );
        assert!(list_stops_making(&[AccessEntry {
            reaches_new_directories: true,
            ..HIDES_THE_LIST
        }]));

        // A new directory takes what would stop files being written in it.
        assert!(list_stops_making(&[AccessEntry {
            applies_here: false,
            reaches_new_directories: true,
            ..DENY_WRITING
        }]));
    }

    #[test]
    fn a_deny_is_harmless_wherever_it_is() {
        for deny in [DENY_DELETE, DENY_FOR_NEW_DIRECTORIES] {
            assert_eq!(own_list_problem(&[deny]), None);
            assert_eq!(way_list_problem(&[deny]), None);
            assert_eq!(parent_list_problem(&[deny]), None);
        }
    }

    #[test]
    fn no_list_is_harmless() {
        assert_eq!(own_list_problem(&[]), None);
        assert_eq!(way_list_problem(&[]), None);
        assert_eq!(parent_list_problem(&[]), None);
    }

    #[test]
    fn a_private_directory_may_grant_nothing_at_all() {
        for allow in [ALLOW_READING, ALLOW_CHANGING, ALLOW_FOR_NEW_DIRECTORIES] {
            assert_eq!(
                own_list_problem(&[DENY_DELETE, allow]),
                Some(DirProblem::ListGrants)
            );
        }
    }

    #[test]
    fn what_leads_to_it_may_let_others_read_but_not_change() {
        assert_eq!(way_list_problem(&[ALLOW_READING]), None);
        assert_eq!(
            way_list_problem(&[DENY_DELETE, ALLOW_CHANGING]),
            Some(DirProblem::ListLetsChange)
        );
    }

    #[test]
    fn only_what_reaches_a_new_directory_stops_one_being_made() {
        assert_eq!(parent_list_problem(&[ALLOW_READING]), None);
        assert_eq!(parent_list_problem(&[ALLOW_CHANGING]), None);
        assert_eq!(
            parent_list_problem(&[ALLOW_FOR_NEW_DIRECTORIES]),
            Some(DirProblem::ListPassesOn)
        );
    }

    #[test]
    fn parents_must_be_private_or_sticky() {
        assert!(parent_is_safe(0o700));
        assert!(parent_is_safe(0o755));
        assert!(parent_is_safe(0o1777));
        assert!(!parent_is_safe(0o777));
        assert!(!parent_is_safe(0o775));
        assert!(!parent_is_safe(0o757));
    }

    #[test]
    fn what_leads_to_a_private_directory_must_be_this_users_or_roots() {
        assert!(ancestor_is_safe(0o755, 501, 501));
        assert!(ancestor_is_safe(0o755, 0, 501));
        assert!(ancestor_is_safe(0o1777, 0, 501));
        assert!(!ancestor_is_safe(0o755, 502, 501), "another user's");
        assert!(!ancestor_is_safe(0o775, 501, 501), "others can write");
    }

    #[test]
    fn accepts_a_private_directory() {
        assert_eq!(check_dir(false, true, 501, 0o700, 501), Ok(()));
        assert_eq!(check_dir(false, true, 501, 0o500, 501), Ok(()));
    }

    #[test]
    fn refuses_a_symlink() {
        assert_eq!(
            check_dir(true, false, 501, 0o777, 501),
            Err(DirProblem::Symlink)
        );
    }

    #[test]
    fn refuses_a_file() {
        assert_eq!(
            check_dir(false, false, 501, 0o600, 501),
            Err(DirProblem::NotADirectory)
        );
    }

    #[test]
    fn refuses_another_owner() {
        assert_eq!(
            check_dir(false, true, 0, 0o700, 501),
            Err(DirProblem::WrongOwner)
        );
        assert_eq!(
            check_dir(false, true, 502, 0o700, 501),
            Err(DirProblem::WrongOwner)
        );
    }

    #[test]
    fn refuses_any_group_or_other_permission() {
        for mode in [0o755, 0o750, 0o701, 0o710, 0o707, 0o1777] {
            assert_eq!(
                check_dir(false, true, 501, mode, 501),
                Err(DirProblem::LooseMode),
                "{mode:o}"
            );
        }
    }

    #[test]
    fn config_home_comes_from_the_environment_and_only_an_absolute_path_counts() {
        use super::backend::config_home_from;
        use std::path::PathBuf;
        let home = |vars: &'static [(&'static str, &'static str)]| {
            config_home_from(&|name| {
                vars.iter()
                    .find(|(var, _)| *var == name)
                    .map(|(_, value)| value.into())
            })
        };
        assert_eq!(home(&[]), None);
        #[cfg(windows)]
        {
            assert_eq!(
                home(&[("APPDATA", r"C:\Users\me\AppData\Roaming")]),
                Some(PathBuf::from(r"C:\Users\me\AppData\Roaming\oso"))
            );
            assert_eq!(home(&[("APPDATA", "relative")]), None);
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                home(&[("XDG_CONFIG_HOME", "/x/conf"), ("HOME", "/home/me")]),
                Some(PathBuf::from("/x/conf/oso"))
            );
            // A relative value does not count, and the home's .config is used, on macOS too.
            assert_eq!(
                home(&[("XDG_CONFIG_HOME", "conf"), ("HOME", "/home/me")]),
                Some(PathBuf::from("/home/me/.config/oso"))
            );
        }
    }

    #[test]
    fn state_home_comes_from_the_environment_and_only_an_absolute_path_counts() {
        use super::backend::state_home_from;
        use std::path::PathBuf;
        let home = |vars: &'static [(&'static str, &'static str)]| {
            state_home_from(&|name| {
                vars.iter()
                    .find(|(var, _)| *var == name)
                    .map(|(_, value)| value.into())
            })
        };
        assert_eq!(home(&[]), None);
        assert_eq!(
            home(&[("HOME", "relative"), ("LOCALAPPDATA", "relative")]),
            None
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            home(&[("HOME", "/h"), ("XDG_STATE_HOME", "/x")]),
            Some(PathBuf::from("/h/Library/Application Support"))
        );
        #[cfg(windows)]
        assert_eq!(
            home(&[("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local")]),
            Some(PathBuf::from("C:\\Users\\a\\AppData\\Local"))
        );
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            assert_eq!(
                home(&[("HOME", "/h"), ("XDG_STATE_HOME", "/x")]),
                Some(PathBuf::from("/x"))
            );
            assert_eq!(
                home(&[("HOME", "/h"), ("XDG_STATE_HOME", "relative")]),
                Some(PathBuf::from("/h/.local/state"))
            );
        }
    }
}
