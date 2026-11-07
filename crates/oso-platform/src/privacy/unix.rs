//! The private directory and the core dump limit on Unix.
//!
//! Must not depend on: `control`, `process` or `text`.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Component, Path, PathBuf};

use super::{
    DirProblem, ancestor_is_safe, check_dir, list_stops_making, list_stops_the_way,
    list_stops_writing, own_list_problem, parent_list_problem, way_list_problem,
};
use crate::file_meta::{AccessEntry, directory_access_list};

/// The most symbolic links the way to a private directory may pass through, as in Linux.
const MAX_LINKS: usize = 40;

/// Sets `RLIMIT_CORE` to 0, both soft and hard, so that nothing in this process can
/// raise it again.
pub(super) fn disable_core_dumps() -> io::Result<()> {
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `none` is an initialized local that outlives the call, and setrlimit only
    // reads it. Lowering a limit is always allowed and touches no memory of ours.
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const none) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The effective user ID of this process.
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions, cannot fail and touches no
    // memory of ours.
    unsafe { libc::geteuid() }
}

/// The per-user directory for state: `~/Library/Application Support` on macOS, otherwise
/// `$XDG_STATE_HOME` or `~/.local/state`. The XDG specification and common sense say a
/// relative path means nothing here.
pub(super) fn state_home_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        var(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if cfg!(target_os = "macos") {
        return Some(
            absolute("HOME")?
                .join("Library")
                .join("Application Support"),
        );
    }
    match absolute("XDG_STATE_HOME") {
        Some(base) => Some(base),
        None => Some(absolute("HOME")?.join(".local").join("state")),
    }
}

/// Whether the directory at `path` could not be used, as far as can be told creating nothing:
/// it would be refused, or this user could not write to it or create it. The modes say that, and an
/// access list can say more on macOS. An access list could also say otherwise, which is why this
/// only warns, and opening does not ask, and goes by what the system says.
pub(super) fn check_private(path: &Path) -> io::Result<()> {
    let deepest = walk_the_way(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            check_itself(path, &meta)?;
            check_denies(path, list_stops_writing)?;
            check_writable(path, &meta)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            check_list(&deepest, parent_list_problem)?;
            check_denies(&deepest, list_stops_making)?;
            let meta =
                fs::metadata(&deepest).map_err(|e| with_path(&e, "cannot inspect", &deepest))?;
            check_writable(&deepest, &meta)
        }
        Err(e) => Err(with_path(&e, "cannot inspect", path)),
    }
}

/// Refuses a directory where this user could not make a file.
fn check_writable(directory: &Path, meta: &fs::Metadata) -> io::Result<()> {
    if can_write_in(meta.mode(), meta.uid(), euid()) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("this user cannot write in {}", directory.display()),
        ))
    }
}

/// Whether `euid` can make a file in a directory with `mode` and `owner`, which needs writing and
/// searching. Whose group it is cannot be told simply, but what leads to a private directory
/// cannot be group-writable unless sticky, so only the other bits are left to look at.
fn can_write_in(mode: u32, owner: u32, euid: u32) -> bool {
    euid == 0
        || if owner == euid {
            mode & 0o300 == 0o300
        } else {
            mode & 0o003 == 0o003
        }
}

/// Makes sure of the directory at `path`, creating it if missing, and what leads to it with
/// mode `0700` too, once nothing on the way lets another user replace it. The way is walked
/// again after that, as what did not exist before has been judged only now.
pub(super) fn open_private(path: &Path) -> io::Result<()> {
    let deepest = check_the_way(path)?;
    if matches!(fs::symlink_metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound) {
        check_list(&deepest, parent_list_problem)?;
    }
    if let Some(parent) = path.parent() {
        DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(parent)
            .map_err(|e| with_path(&e, "cannot create", parent))?;
    }
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(with_path(&e, "cannot create", path)),
    }
    check_the_way(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| with_path(&e, "cannot inspect", path))?;
    check_itself(path, &meta)
}

/// One step of the walk: a part of a path, and whether it came from a link's target.
struct Step {
    part: OsString,
    from_link: bool,
}

/// Walks the way to the directory at `path`, one name at a time and as the system would,
/// through every link and every directory it passes, whether or not they are on the path as
/// written. Each must keep other users out. A name still missing is what `open_private` will
/// create, unless a link led to it, as a link to nothing can be made to lead anywhere. Returns
/// the deepest directory that exists on the way, with no links in it.
fn check_the_way(path: &Path) -> io::Result<PathBuf> {
    walk_the_way(path, false)
}

/// [`check_the_way`], which if `warn` also says what would stop anything going through.
fn walk_the_way(path: &Path, warn: bool) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or(Path::new(""));
    let mut steps: VecDeque<Step> = steps_of(parent, false);
    let mut current = PathBuf::new();
    let mut links = 0;
    while let Some(step) = steps.pop_front() {
        if step.part == ".." {
            current.pop();
            continue;
        }
        let next = current.join(&step.part);
        let meta = match fs::symlink_metadata(&next) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound && !step.from_link => {
                return Ok(current);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(refused(&next, "is missing, where a link leads"));
            }
            Err(e) => return Err(with_path(&e, "cannot inspect", &next)),
        };
        check_ancestor(&next, &meta)?;
        if warn && !meta.file_type().is_symlink() {
            check_denies(&next, list_stops_the_way)?;
        }
        if meta.file_type().is_symlink() {
            links += 1;
            if links > MAX_LINKS {
                return Err(refused(&next, "is one link too many"));
            }
            let target = fs::read_link(&next).map_err(|e| with_path(&e, "cannot read", &next))?;
            for step in steps_of(&target, true).into_iter().rev() {
                steps.push_front(step);
            }
        } else if meta.is_dir() {
            current = next;
        } else {
            return Err(refused(&next, "is not a directory"));
        }
    }
    Ok(current)
}

/// The steps of `path`. A root stays a step, and joining it starts again from there.
fn steps_of(path: &Path, from_link: bool) -> VecDeque<Step> {
    path.components()
        .filter(|component| *component != Component::CurDir)
        .map(|component| Step {
            part: component.as_os_str().to_owned(),
            from_link,
        })
        .collect()
}

/// Checks a directory or a link on the way. A link's own mode means nothing, but its owner can
/// change where it leads.
fn check_ancestor(ancestor: &Path, meta: &fs::Metadata) -> io::Result<()> {
    let mode = if meta.file_type().is_symlink() {
        0o700
    } else {
        meta.mode()
    };
    if !ancestor_is_safe(mode, meta.uid(), euid()) {
        return Err(refused(ancestor, "can be changed by other users"));
    }
    // A link can hold a list of its own, and what it leads to is walked next.
    check_list(ancestor, way_list_problem)
}

/// Checks the directory itself, from its `lstat` results.
fn check_itself(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    match itself_problem(path, meta).map_err(|e| with_path(&e, "cannot inspect", path))? {
        Some(problem) => Err(refused(path, problem.message())),
        None => Ok(()),
    }
}

/// What keeps the directory at `path`, with these `lstat` results, from being private.
///
/// # Errors
///
/// If its access list cannot be read.
pub(crate) fn itself_problem(path: &Path, meta: &fs::Metadata) -> io::Result<Option<DirProblem>> {
    let by_mode = check_dir(
        meta.file_type().is_symlink(),
        meta.is_dir(),
        meta.uid(),
        meta.mode(),
        euid(),
    );
    match by_mode {
        Err(problem) => Ok(Some(problem)),
        Ok(()) => list_problem(path, own_list_problem),
    }
}

/// What `rule` finds wrong with the access list of the directory at `path`.
///
/// # Errors
///
/// If the list cannot be read, which says nothing of what it holds.
pub(crate) fn list_problem(
    path: &Path,
    rule: impl FnOnce(&[AccessEntry]) -> Option<DirProblem>,
) -> io::Result<Option<DirProblem>> {
    Ok(rule(&directory_access_list(path)?))
}

/// Refuses the directory at `path` if `rule` finds anything wrong with its access list, or the
/// list cannot be read.
fn check_list(
    path: &Path,
    rule: impl FnOnce(&[AccessEntry]) -> Option<DirProblem>,
) -> io::Result<()> {
    match list_problem(path, rule).map_err(|e| with_path(&e, "cannot inspect", path))? {
        Some(problem) => Err(refused(path, problem.message())),
        None => Ok(()),
    }
}

/// Says if `stops` finds what would stop a file being written where this list is. This is for
/// warning only, as opening goes by what the system says.
fn check_denies(path: &Path, stops: impl FnOnce(&[AccessEntry]) -> bool) -> io::Result<()> {
    let entries = directory_access_list(path).map_err(|e| with_path(&e, "cannot inspect", path))?;
    if stops(&entries) {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "an access list would stop a file being written: {}",
                path.display()
            ),
        ))
    } else {
        Ok(())
    }
}

fn refused(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("refused: it {why}: {}", path.display()),
    )
}

fn with_path(e: &io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("{what}: {e}: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::io::ErrorKind;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::{can_write_in, disable_core_dumps, euid};
    use crate::privacy::PrivateDir;

    /// A fresh scratch directory for one test, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("osop-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn creates_it_and_what_leads_to_it_with_mode_0700() {
        let scratch = Scratch::new();
        let path = scratch.join("state/oso/emergency");
        let dir = PrivateDir::open(&path).unwrap();
        assert_eq!(dir.path(), path);
        assert_eq!(mode(&path), 0o700);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&scratch.join("state")), 0o700);
        PrivateDir::open(&path).expect("opening it again is fine");
    }

    #[test]
    fn refuses_a_looser_mode_and_does_not_repair_it() {
        let scratch = Scratch::new();
        let path = scratch.join("d");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o750)).unwrap();
        let err = PrivateDir::open(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("mode is not 0700"), "{err}");
        assert_eq!(mode(&path), 0o750);
    }

    #[test]
    fn refuses_a_symlink_to_a_private_directory() {
        let scratch = Scratch::new();
        let target = scratch.join("real");
        PrivateDir::open(&target).unwrap();
        let link = scratch.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = PrivateDir::open(&link).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
    }

    #[test]
    fn refuses_a_file() {
        let scratch = Scratch::new();
        let path = scratch.join("d");
        fs::write(&path, b"").unwrap();
        assert!(PrivateDir::open(&path).is_err());
    }

    #[test]
    fn refuses_another_owner() {
        // Only root can create one, so this runs where `/` is owned by another user,
        // which is everywhere except as root.
        if euid() == 0 {
            return;
        }
        let err = PrivateDir::open("/").unwrap_err();
        assert!(err.to_string().contains("owned by another user"), "{err}");
    }

    #[test]
    fn refuses_a_parent_other_users_can_write_and_creates_nothing_there() {
        let scratch = Scratch::new();
        let parent = scratch.join("shared");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, Permissions::from_mode(0o777)).unwrap();
        let err = PrivateDir::open(parent.join("d")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(!parent.join("d").exists());
        // A sticky one, like /tmp, is fine.
        fs::set_permissions(&parent, Permissions::from_mode(0o1777)).unwrap();
        PrivateDir::open(parent.join("d")).unwrap();
    }

    #[test]
    fn refuses_anything_on_the_way_that_other_users_can_write() {
        let scratch = Scratch::new();
        let shared = scratch.join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, Permissions::from_mode(0o777)).unwrap();
        let path = shared.join("oso").join("emergency");
        let err = PrivateDir::open(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(!shared.join("oso").exists(), "nothing new appears");
        let err = PrivateDir::check(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_link_on_the_way_is_judged_by_all_it_really_leads_through() {
        let scratch = Scratch::new();
        let shared = scratch.join("shared");
        let state = shared.join("state");
        fs::create_dir(&shared).unwrap();
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, Permissions::from_mode(0o700)).unwrap();
        let link = scratch.join("link");
        std::os::unix::fs::symlink(&state, &link).unwrap();
        // The link's target is private, and the written path is fine. Only the real one tells.
        fs::set_permissions(&shared, Permissions::from_mode(0o777)).unwrap();
        let err = PrivateDir::open(link.join("oso").join("emergency")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(!state.join("oso").exists(), "nothing new appears");

        // With nothing wrong on the way, a link is fine.
        fs::set_permissions(&shared, Permissions::from_mode(0o700)).unwrap();
        PrivateDir::open(link.join("oso").join("emergency")).unwrap();
        assert!(state.join("oso").join("emergency").is_dir());
    }

    #[test]
    fn a_link_between_the_written_path_and_the_real_one_counts_too() {
        use std::os::unix::fs::symlink;
        let scratch = Scratch::new();
        let shared = scratch.join("shared");
        let real = scratch.join("real");
        fs::create_dir(&shared).unwrap();
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, Permissions::from_mode(0o700)).unwrap();
        // Neither the written path nor the real one holds `shared`: only this link does.
        symlink(&real, shared.join("middle")).unwrap();
        let link = scratch.join("link");
        symlink(shared.join("middle"), &link).unwrap();
        fs::set_permissions(&shared, Permissions::from_mode(0o777)).unwrap();
        let path = link.join("oso").join("emergency");
        assert!(PrivateDir::check(&path).is_err());
        assert!(PrivateDir::open(&path).is_err());
        assert!(!real.join("oso").exists(), "nothing new appears");
    }

    #[test]
    fn a_link_for_the_directory_is_refused_however_it_is_spelled() {
        let scratch = Scratch::new();
        let target = scratch.join("real");
        PrivateDir::open(&target).unwrap();
        std::os::unix::fs::symlink(&target, scratch.join("link")).unwrap();
        for spelled in ["link/", "link/.", "link/./"] {
            let path = scratch.0.join(spelled);
            let err = PrivateDir::open(&path).unwrap_err();
            assert!(err.to_string().contains("symlink"), "{spelled}: {err}");
            assert!(PrivateDir::check(&path).is_err(), "{spelled}");
        }
    }

    #[test]
    fn a_parent_directory_in_a_links_target_leads_where_it_really_does() {
        let scratch = Scratch::new();
        let shared = scratch.join("shared");
        let real = shared.join("real");
        fs::create_dir(&shared).unwrap();
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(scratch.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink("../shared/real", scratch.join("elsewhere/link")).unwrap();
        fs::set_permissions(&shared, Permissions::from_mode(0o777)).unwrap();
        let err = PrivateDir::open(scratch.join("elsewhere/link/d")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(!real.join("d").exists(), "nothing new appears");
    }

    #[test]
    fn a_relative_link_is_followed_from_where_it_is() {
        let scratch = Scratch::new();
        fs::create_dir(scratch.join("real")).unwrap();
        fs::set_permissions(scratch.join("real"), Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink("real", scratch.join("link")).unwrap();
        PrivateDir::open(scratch.join("link").join("oso").join("d")).unwrap();
        assert!(scratch.join("real").join("oso").join("d").is_dir());
    }

    #[test]
    fn a_link_to_nothing_on_the_way_is_found_by_checking_too() {
        let scratch = Scratch::new();
        let link = scratch.join("oso");
        std::os::unix::fs::symlink(scratch.join("gone"), &link).unwrap();
        let path = link.join("emergency");
        assert!(PrivateDir::check(&path).is_err());
        assert!(PrivateDir::open(&path).is_err());
        assert!(!scratch.join("gone").exists(), "nothing new appears");
    }

    #[test]
    fn a_loop_of_links_on_the_way_is_refused() {
        use std::os::unix::fs::symlink;
        let scratch = Scratch::new();
        symlink(scratch.join("b"), scratch.join("a")).unwrap();
        symlink(scratch.join("a"), scratch.join("b")).unwrap();
        assert!(PrivateDir::check(&scratch.join("a").join("d")).is_err());
    }

    #[test]
    fn a_file_on_the_way_is_refused() {
        let scratch = Scratch::new();
        fs::write(scratch.join("file"), b"").unwrap();
        assert!(PrivateDir::check(&scratch.join("file").join("d")).is_err());
    }

    #[test]
    fn a_path_through_a_parent_directory_is_refused_and_creates_nothing() {
        let scratch = Scratch::new();
        let path = scratch.join("missing").join("..").join("d");
        let err = PrivateDir::open(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(!scratch.join("missing").exists());
    }

    #[test]
    fn checking_says_what_opening_would_and_creates_nothing() {
        let scratch = Scratch::new();
        let path = scratch.join("state/oso/emergency");
        PrivateDir::check(&path).unwrap();
        assert!(!scratch.join("state").exists(), "nothing is created");
        PrivateDir::open(&path).unwrap();
        PrivateDir::check(&path).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o750)).unwrap();
        assert!(PrivateDir::check(&path).is_err());
    }

    #[test]
    fn who_can_make_a_file_in_a_directory_goes_by_the_owners_or_others_bits() {
        // Mode, owner, this user.
        assert!(can_write_in(0o700, 501, 501));
        assert!(!can_write_in(0o500, 501, 501), "no writing");
        assert!(!can_write_in(0o600, 501, 501), "no searching");
        assert!(!can_write_in(0o755, 0, 501), "root's");
        assert!(can_write_in(0o1777, 0, 501), "like /tmp");
        assert!(
            !can_write_in(0o1775, 0, 501),
            "a sticky one only its group can write"
        );
        assert!(can_write_in(0o500, 501, 0), "root writes anywhere");
    }

    #[test]
    fn checking_finds_a_directory_this_user_could_not_write_and_creates_nothing() {
        // Root can write anywhere, so nothing here would be refused.
        if euid() == 0 {
            return;
        }
        let scratch = Scratch::new();
        let closed = scratch.join("closed");
        fs::create_dir(&closed).unwrap();

        fs::set_permissions(&closed, Permissions::from_mode(0o500)).unwrap();
        let err = PrivateDir::check(&closed).unwrap_err();
        assert!(err.to_string().contains("cannot write in"), "{err}");

        let inside = closed.join("oso").join("emergency");
        let err = PrivateDir::check(&inside).unwrap_err();
        assert!(err.to_string().contains("cannot write in"), "{err}");
        assert!(!closed.join("oso").exists(), "nothing new appears");
        // Opening does not ask: the system's answer is the one that counts there.
        assert!(PrivateDir::open(&inside).is_err());

        fs::set_permissions(&closed, Permissions::from_mode(0o700)).unwrap();
        PrivateDir::check(&inside).unwrap();
        // Mode `0500` passes as a control socket's directory: this rule is not in `check_dir`.
        fs::set_permissions(&closed, Permissions::from_mode(0o500)).unwrap();
        assert!(PrivateDir::open(&closed).is_ok());
        fs::set_permissions(&closed, Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn checking_follows_a_link_to_the_directory_it_really_would_write_in() {
        if euid() == 0 {
            return;
        }
        let scratch = Scratch::new();
        let closed = scratch.join("closed");
        fs::create_dir(&closed).unwrap();
        fs::set_permissions(&closed, Permissions::from_mode(0o500)).unwrap();
        std::os::unix::fs::symlink(&closed, scratch.join("link")).unwrap();
        assert!(PrivateDir::check(&scratch.join("link").join("d")).is_err());
        fs::set_permissions(&closed, Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn a_relative_path_is_refused() {
        let err = PrivateDir::open("relative/emergency").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(PrivateDir::check(Path::new("relative")).is_err());
        assert!(!Path::new("relative").exists());
    }

    #[cfg(target_os = "macos")]
    mod lists {
        use std::fs::{self, Permissions};
        use std::os::unix::fs::PermissionsExt;
        use std::path::Path;

        use super::Scratch;
        use crate::file_meta::test_list::{clear, set_for_everyone, set_on_link_for_everyone};
        use crate::privacy::PrivateDir;

        /// A directory on the way, whose mode does not depend on the umask.
        fn plain_directory(path: &Path) {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
        }

        #[test]
        fn a_deny_on_the_directory_or_on_the_way_passes() {
            let scratch = Scratch::new();
            let way = scratch.join("way");
            let path = way.join("d");
            PrivateDir::open(&path).unwrap();
            set_for_everyone(&way, "deny", "delete");
            set_for_everyone(&path, "deny", "delete");
            PrivateDir::check(&path).unwrap();
            PrivateDir::open(&path).unwrap();
            clear(&path);
            clear(&way);
        }

        #[test]
        fn any_allow_on_the_directory_is_refused_and_not_repaired() {
            let scratch = Scratch::new();
            let path = scratch.join("d");
            PrivateDir::open(&path).unwrap();
            set_for_everyone(&path, "allow", "read");
            for err in [
                PrivateDir::open(&path).unwrap_err(),
                PrivateDir::check(&path).unwrap_err(),
            ] {
                assert!(err.to_string().contains("access list"), "{err}");
            }
            assert_eq!(
                crate::file_meta::directory_access_list(&path)
                    .unwrap()
                    .len(),
                1
            );
        }

        #[test]
        fn an_allow_only_for_what_is_made_in_the_directory_is_refused_too() {
            let scratch = Scratch::new();
            let path = scratch.join("d");
            PrivateDir::open(&path).unwrap();
            set_for_everyone(&path, "allow,file_inherit,only_inherit", "read");
            assert!(PrivateDir::check(&path).is_err());
        }

        #[test]
        fn an_allow_that_only_reads_is_fine_on_the_way() {
            let scratch = Scratch::new();
            let way = scratch.join("way");
            plain_directory(&way);
            set_for_everyone(&way, "allow", "read,execute");
            PrivateDir::open(way.join("d")).unwrap();
        }

        #[test]
        fn an_allow_that_changes_what_is_on_the_way_is_refused_and_nothing_appears() {
            for rights in ["write", "append", "delete", "writesecurity", "chown"] {
                let scratch = Scratch::new();
                let way = scratch.join("way");
                plain_directory(&way);
                set_for_everyone(&way, "allow", rights);
                let path = way.join("d");
                let err = PrivateDir::open(&path).unwrap_err();
                assert!(err.to_string().contains("access list"), "{rights}: {err}");
                assert!(PrivateDir::check(&path).is_err(), "{rights}");
                assert!(!path.exists(), "{rights}");
            }
        }

        #[test]
        fn what_would_reach_a_new_directory_is_refused_only_if_one_is_to_be_made() {
            let scratch = Scratch::new();
            let way = scratch.join("way");
            let path = way.join("oso").join("d");
            plain_directory(&way);
            set_for_everyone(&way, "allow,directory_inherit", "read");
            let err = PrivateDir::open(&path).unwrap_err();
            assert!(err.to_string().contains("subdirectories"), "{err}");
            assert!(PrivateDir::check(&path).is_err());
            assert!(!way.join("oso").exists(), "nothing new appears");

            // A directory made before the list did not take it, and nothing more is made.
            clear(&way);
            PrivateDir::open(&path).unwrap();
            set_for_everyone(&way, "allow,directory_inherit", "read");
            PrivateDir::check(&path).unwrap();
            PrivateDir::open(&path).unwrap();
        }

        #[test]
        fn an_entry_only_for_files_stops_at_its_own_directory() {
            let scratch = Scratch::new();
            let way = scratch.join("way");
            plain_directory(&way);
            set_for_everyone(&way, "allow,file_inherit", "read");
            let path = way.join("oso").join("d");
            PrivateDir::check(&path).unwrap();
            PrivateDir::open(&path).unwrap();
            assert_eq!(crate::file_meta::directory_access_list(&path).unwrap(), []);
        }

        #[test]
        fn a_links_own_list_counts_on_the_way() {
            let scratch = Scratch::new();
            let real = scratch.join("real");
            PrivateDir::open(&real).unwrap();
            let link = scratch.join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            PrivateDir::open(link.join("d")).unwrap();

            set_on_link_for_everyone(&link, "allow", "delete");
            let err = PrivateDir::open(link.join("d")).unwrap_err();
            assert!(err.to_string().contains("access list"), "{err}");
            assert!(PrivateDir::check(&link.join("d")).is_err());

            set_on_link_for_everyone(&link, "allow", "read");
            PrivateDir::open(link.join("d")).unwrap();
        }

        #[test]
        fn a_list_that_cannot_be_read_is_refused_as_nothing_can_be_told() {
            let scratch = Scratch::new();
            let way = scratch.join("way");
            let path = way.join("d");
            PrivateDir::open(&path).unwrap();
            for hidden in [&path, &way] {
                set_for_everyone(hidden, "deny", "readsecurity");
                let open = PrivateDir::open(&path);
                let check = PrivateDir::check(&path);
                clear(hidden);
                assert!(open.is_err());
                assert!(check.is_err());
            }
            PrivateDir::open(&path).unwrap();
        }

        #[test]
        fn what_would_stop_a_file_in_the_directory_is_warned_of_and_opening_does_not_ask() {
            for (how, rights) in [
                ("deny", "write"),
                ("deny", "execute"),
                ("deny", "delete_child"),
                ("deny,file_inherit,only_inherit", "readsecurity"),
            ] {
                let scratch = Scratch::new();
                let path = scratch.join("d");
                PrivateDir::open(&path).unwrap();
                set_for_everyone(&path, how, rights);
                let warned = PrivateDir::check(&path);
                let opened = PrivateDir::open(&path);
                clear(&path);
                let warned = warned.unwrap_err().to_string();
                assert!(warned.contains("would stop"), "{rights}: {warned}");
                opened.unwrap();
            }
        }

        #[test]
        fn what_a_copy_does_not_need_is_not_warned_of() {
            for (how, rights) in [
                ("deny", "delete"),
                ("deny", "read"),
                ("deny", "append"),
                ("deny,file_inherit,only_inherit", "delete"),
                ("deny,file_inherit,only_inherit", "write"),
            ] {
                let scratch = Scratch::new();
                let path = scratch.join("d");
                PrivateDir::open(&path).unwrap();
                set_for_everyone(&path, how, rights);
                let warned = PrivateDir::check(&path);
                clear(&path);
                warned.unwrap();
            }
        }

        #[test]
        fn on_the_way_only_what_stops_going_through_is_warned_of() {
            let scratch = Scratch::new();
            let way = scratch.join("way");
            let path = way.join("d");
            plain_directory(&way);
            PrivateDir::open(&path).unwrap();

            set_for_everyone(&way, "deny", "write");
            let writing = PrivateDir::check(&path);
            set_for_everyone(&way, "deny", "execute");
            let searching = PrivateDir::check(&path);
            clear(&way);
            writing.unwrap();
            assert!(searching.is_err());
        }

        #[test]
        fn what_would_stop_a_directory_being_made_and_used_is_warned_of_and_nothing_appears() {
            for (how, rights) in [
                ("deny", "append"),
                ("deny,directory_inherit,only_inherit", "readsecurity"),
                ("deny,directory_inherit,only_inherit", "write"),
            ] {
                let scratch = Scratch::new();
                let way = scratch.join("way");
                plain_directory(&way);
                set_for_everyone(&way, how, rights);
                let warned = PrivateDir::check(&way.join("oso").join("d"));
                let made = way.join("oso").exists();
                clear(&way);
                let warned = warned.unwrap_err().to_string();
                assert!(warned.contains("would stop"), "{rights}: {warned}");
                assert!(!made, "{rights}");
            }

            // What stops at the directory it is on reaches nothing that is to be made.
            let scratch = Scratch::new();
            let way = scratch.join("way");
            plain_directory(&way);
            set_for_everyone(&way, "deny,file_inherit,only_inherit", "readsecurity");
            let warned = PrivateDir::check(&way.join("oso").join("d"));
            clear(&way);
            warned.unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    mod lists {
        use std::ffi::CString;
        use std::fs::{self, File, OpenOptions};
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;
        use std::path::Path;

        use super::{Scratch, mode};
        use crate::privacy::PrivateDir;

        const USER_OBJ: u16 = 0x01;
        const USER: u16 = 0x02;
        const GROUP_OBJ: u16 = 0x04;
        const MASK: u16 = 0x10;
        const OTHER: u16 = 0x20;

        /// `system.posix_acl_*` as the kernel takes it: a version, then tag, rights and ID.
        fn list_bytes(entries: &[(u16, u16, u32)]) -> Vec<u8> {
            let mut bytes = 2u32.to_le_bytes().to_vec();
            for (tag, rights, id) in entries {
                bytes.extend_from_slice(&tag.to_le_bytes());
                bytes.extend_from_slice(&rights.to_le_bytes());
                bytes.extend_from_slice(&id.to_le_bytes());
            }
            bytes
        }

        /// A list that gives another user everything. The ID need not belong to anyone.
        fn list_for_another_user() -> Vec<u8> {
            list_bytes(&[
                (USER_OBJ, 7, u32::MAX),
                (USER, 7, 12345),
                (GROUP_OBJ, 0, u32::MAX),
                (MASK, 7, u32::MAX),
                (OTHER, 0, u32::MAX),
            ])
        }

        /// Sets the list called `name` on `path`, or says the file system has none. Only outside
        /// CI may that pass as nothing to test.
        fn set_list(path: &Path, name: &str, bytes: &[u8]) -> bool {
            let file = File::open(path).unwrap();
            let name = CString::new(name).unwrap();
            // SAFETY: the descriptor is borrowed from `file`, and `name` and `bytes` outlive the
            // call, which only reads them, `bytes` for the length given.
            let set = unsafe {
                libc::fsetxattr(
                    file.as_raw_fd(),
                    name.as_ptr(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    0,
                )
            };
            if set == 0 {
                return true;
            }
            let error = std::io::Error::last_os_error();
            assert_eq!(error.raw_os_error(), Some(libc::EOPNOTSUPP), "{error}");
            assert!(
                std::env::var_os("CI").is_none(),
                "the file system must have ACLs for this test: {error}"
            );
            false
        }

        /// The rights in the mask of the access list on `path`.
        fn mask_of(path: &Path) -> Option<u16> {
            let path = CString::new(path.as_os_str().as_bytes()).unwrap();
            let name = CString::new("system.posix_acl_access").unwrap();
            let mut buffer = [0u8; 256];
            // SAFETY: `path` and `name` outlive the call, which only reads them, and `buffer` is
            // a live local the call writes at most its length of.
            let read = unsafe {
                libc::lgetxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            let read = usize::try_from(read).ok()?;
            buffer[4..read].chunks_exact(8).find_map(|entry| {
                (u16::from_le_bytes([entry[0], entry[1]]) == MASK)
                    .then(|| u16::from_le_bytes([entry[2], entry[3]]))
            })
        }

        #[test]
        fn a_default_list_grants_nothing_on_what_is_made_with_these_modes() {
            let scratch = Scratch::new();
            let shared = scratch.join("shared");
            fs::create_dir(&shared).unwrap();
            if !set_list(
                &shared,
                "system.posix_acl_default",
                &list_for_another_user(),
            ) {
                return;
            }
            let path = shared.join("oso").join("emergency");
            PrivateDir::open(&path).unwrap();
            for directory in [path.parent().unwrap(), &path] {
                assert_eq!(mode(directory), 0o700);
                assert_eq!(mask_of(directory), Some(0));
            }
            let copy = path.join("copy");
            drop(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&copy)
                    .unwrap(),
            );
            assert_eq!(mode(&copy), 0o600);
            assert_eq!(mask_of(&copy), Some(0));
        }

        #[test]
        fn what_an_access_list_grants_shows_in_the_mode_and_is_refused() {
            let scratch = Scratch::new();
            let path = scratch.join("d");
            fs::create_dir(&path).unwrap();
            if !set_list(&path, "system.posix_acl_access", &list_for_another_user()) {
                return;
            }
            assert_eq!(mode(&path), 0o770);
            assert!(PrivateDir::open(&path).is_err());
            assert!(PrivateDir::open(path.join("inner")).is_err());
            assert!(!path.join("inner").exists());
        }
    }

    #[test]
    fn core_dumps_stay_off_for_good() {
        disable_core_dumps().unwrap();
        let mut limit = libc::rlimit {
            rlim_cur: 1,
            rlim_max: 1,
        };
        // SAFETY: `limit` is an initialized local that outlives the call, and getrlimit
        // writes only within it.
        let read = unsafe { libc::getrlimit(libc::RLIMIT_CORE, &raw mut limit) };
        assert_eq!(read, 0);
        assert_eq!((limit.rlim_cur, limit.rlim_max), (0, 0));
    }
}
