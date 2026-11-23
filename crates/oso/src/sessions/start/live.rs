//! What holds a session's start back, on its start thread (ADR 0121, "Restore and the fork
//! hazard"): a missing directory, and Claude Code's registry's checks, a resume whose
//! conversation is live elsewhere or a new session where another runs, and the second look
//! after a resume.
//!
//! Must not depend on: `app` or the UI thread; it runs on the session's start thread.

use std::path::{Path, PathBuf};

use oso_platform::process;

use super::super::identity::SessionId;
use super::super::registry;
use super::{Kind, Order, Report};

/// What stops `order` before anything is made: a missing directory, a new session's directory
/// where another live one runs when it is to ask first, or a resume whose conversation is live.
pub(super) fn held_back(order: &Order) -> Option<Report> {
    let token = order.token;
    if !order.directory.is_dir() {
        return Some(Report::Refused {
            token,
            why: "The session's directory is not there any more".into(),
        });
    }
    match &order.kind {
        Kind::Resume(id) => live(id).map(|(pid, directory)| Report::Live {
            token,
            pid,
            directory,
        }),
        Kind::New | Kind::Fork(_) => {
            (order.check_shared && runs_in(&order.directory)).then_some(Report::Shared { token })
        }
    }
}

/// A live process of the session `id`, by the registry: its pid and directory. With no pid,
/// the registry could not tell: its directory is not known, or it was not listed and read in
/// full (`registry::Listing::complete`). A resume is then refused too, as the safe side.
fn live(id: &SessionId) -> Option<(Option<u32>, Option<PathBuf>)> {
    live_in(registry::directory().as_deref(), id)
}

/// [`live`] in the registry `directory`, `None` if it is not known.
fn live_in(directory: Option<&Path>, id: &SessionId) -> Option<(Option<u32>, Option<PathBuf>)> {
    let Some(directory) = directory else {
        return Some((None, None));
    };
    let listing = registry::list(directory);
    let live = registry::live_with(&listing.entries, id, process::start_time);
    match live.first() {
        Some(entry) => Some((Some(entry.pid), entry.cwd.clone())),
        None => (!listing.complete()).then_some((None, None)),
    }
}

/// Whether the registry has a live session whose directory is `directory`.
fn runs_in(directory: &Path) -> bool {
    let Some(dir) = registry::directory() else {
        return false;
    };
    registry::read(&dir).iter().any(|entry| {
        entry
            .cwd
            .as_deref()
            .is_some_and(|cwd| oso_core::project::same_place(cwd, directory))
            && registry::liveness(entry, process::start_time) != registry::Live::No
    })
}

/// How many live processes the registry has for `id`.
pub(super) fn live_count(id: &SessionId) -> usize {
    let Some(directory) = registry::directory() else {
        return 0;
    };
    let entries = registry::read(&directory);
    registry::live_with(&entries, id, process::start_time).len()
}

// Hosted sessions run on Unix only (ADR 0120).
#[cfg(all(test, unix))]
mod tests {
    use super::live_in;
    use crate::sessions::identity::SessionId;

    /// A scratch directory, removed when dropped.
    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_resume_counts_a_registry_not_read_in_full_as_live() {
        use std::os::unix::fs::PermissionsExt as _;
        let id = SessionId::from_random([7; 16]);
        let scratch = Scratch(std::env::temp_dir().join(format!(
            "oso-start-live-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        )));
        let dir = scratch.0.as_path();
        std::fs::create_dir(dir).unwrap();
        // Nothing there: no session wrote to it.
        assert_eq!(live_in(Some(&dir.join("missing")), &id), None);
        assert_eq!(live_in(Some(dir), &id), None);
        assert_eq!(live_in(None, &id), Some((None, None)));
        // An entry file that cannot be read. Root reads it anyway.
        let shut = dir.join("1.json");
        std::fs::write(&shut, r#"{"pid":1}"#).unwrap();
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let want = std::fs::File::open(&shut).is_err().then_some((None, None));
        assert_eq!(live_in(Some(dir), &id), want);
        std::fs::remove_file(&shut).unwrap();
        // One that holds no entry for its process, root or not.
        std::fs::write(&shut, "not json").unwrap();
        assert_eq!(live_in(Some(dir), &id), Some((None, None)));
        std::fs::remove_file(&shut).unwrap();
        // A file where the directory should be cannot be listed.
        std::fs::write(dir.join("file"), "").unwrap();
        assert_eq!(live_in(Some(&dir.join("file")), &id), Some((None, None)));
        for pid in 1..=crate::sessions::registry::MAX_FILES + 1 {
            let entry = format!(r#"{{"pid":{pid}}}"#);
            std::fs::write(dir.join(format!("{pid}.json")), entry).unwrap();
        }
        assert_eq!(live_in(Some(dir), &id), Some((None, None)));
    }
}
