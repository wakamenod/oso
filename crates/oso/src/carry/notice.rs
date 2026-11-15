//! Noticing that the binary changed (ADR 0133, step 4): a thread of its own is given the
//! identity of the file `oso-restart` would start, as it was when this instance started, and on
//! each look the window's focus asks for, reports an identity that differs from it, with
//! whether `oso-restart` would refuse that file. [`Notice`] says each new identity once.
//!
//! Must not depend on: `app`; the UI thread asks for looks and is told what was found through
//! the callback it gives.
//!
//! The identity is the file's device and inode, its modification time and its length: an
//! install puts a new file in place, or writes the old one again. The path is the one read at
//! start, resolved ([`super::binary`]), so a new binary installed elsewhere, with a link moved
//! to it, is not seen; that is the restart's limit too. What is said does not bind the
//! restart: `oso-restart` checks and starts the file there when it runs.

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::SystemTime;

use super::binary;

/// A file's identity, as far as telling a changed binary from the one this instance came
/// from needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The device and inode, where the system has them.
    file: Option<(u64, u64)>,
    modified: Option<SystemTime>,
    len: u64,
}

impl Identity {
    /// The identity of the file at `path`, following links.
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::MetadataExt;
            Some((metadata.dev(), metadata.ino()))
        };
        #[cfg(not(unix))]
        let file = None;
        Ok(Self {
            file,
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

/// A file in place of the binary this instance came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub identity: Identity,
    /// Why `oso-restart` would refuse it, if it would.
    pub refused: Option<String>,
}

/// The thread that looks at the binary. It ends once this is dropped.
pub struct Looker {
    asked: SyncSender<()>,
}

impl Looker {
    /// Starts the thread, which tells files at `path` from `own`, the identity the file there
    /// had when this instance started. `post` hands what a look found to the UI thread.
    pub fn start(
        path: PathBuf,
        own: Identity,
        post: impl Fn(Found) + Send + 'static,
    ) -> std::io::Result<Self> {
        // One look waits at most: a look asked for meanwhile is that one.
        let (asked, looks) = sync_channel(1);
        std::thread::Builder::new()
            .name("oso-binary".into())
            .spawn(move || serve(&path, own, &looks, post))?;
        Ok(Self { asked })
    }

    /// Asks for a look, unless one is waiting already.
    pub fn look_again(&self) {
        match self.asked.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => log::debug!("oso-binary: the thread has ended"),
        }
    }
}

fn serve(path: &Path, own: Identity, looks: &Receiver<()>, post: impl Fn(Found)) {
    // The last identity checked, and what the check said, so that each is checked once.
    let mut checked: Option<Found> = None;
    while looks.recv().is_ok() {
        let identity = match Identity::of(path) {
            Ok(identity) => identity,
            // Gone, or not readable now: nothing to say, and a later look tries again.
            Err(e) => {
                log::debug!("oso-binary: cannot look at {}: {e}", path.display());
                continue;
            }
        };
        if identity == own {
            continue;
        }
        let found = match checked.take() {
            Some(found) if found.identity == identity => found,
            _ => Found {
                identity,
                refused: binary::check(Some(path)).err(),
            },
        };
        // Posted on each look, as the UI thread may not say it yet; [`Notice`] says it once.
        post(found.clone());
        checked = Some(found);
    }
}

/// What has been said of the files found, so that each is said once.
#[derive(Default)]
pub struct Notice {
    said: Option<Identity>,
}

impl Notice {
    /// What to say of `found`, unless it was said already. It claims no more than was
    /// checked: that the file changed, and whether `oso-restart` would start it.
    pub fn say(&mut self, found: &Found) -> Option<String> {
        if self.said == Some(found.identity) {
            return None;
        }
        self.said = Some(found.identity);
        Some(match &found.refused {
            None => "The file oso started from has changed: M-x oso-restart would start it, \
                     with terminals and sessions"
                .to_owned(),
            Some(why) => format!(
                "The file oso started from has changed, but oso-restart would refuse it: {why}"
            ),
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
    use std::time::Duration;

    use super::{Found, Identity, Looker, Notice};

    const WAIT: Duration = Duration::from_secs(5);

    /// A directory of the test's own, under the system's temporary one.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oso-notice-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir.canonicalize().unwrap()
    }

    /// Writes `bytes` beside `file` and renames it over `file`, as an install does.
    fn install(file: &std::path::Path, bytes: &[u8], mode: u32) {
        let new = file.with_extension("new");
        fs::write(&new, bytes).unwrap();
        fs::set_permissions(&new, fs::Permissions::from_mode(mode)).unwrap();
        fs::rename(&new, file).unwrap();
    }

    /// A looker over `file` as it is now, and what it posts.
    fn looker(file: &std::path::Path) -> (Looker, Receiver<Found>) {
        let own = Identity::of(file).unwrap();
        let (post, found) = channel();
        let looker = Looker::start(file.to_owned(), own, move |f| post.send(f).unwrap()).unwrap();
        (looker, found)
    }

    #[test]
    fn a_file_put_in_place_is_found_on_each_look_and_the_one_at_start_is_not() {
        let dir = scratch("found");
        let file = dir.join("oso");
        fs::write(&file, b"old").unwrap();
        let (looker, found) = looker(&file);
        // A look at the file as it was posts nothing, so the first post is the new file's.
        looker.look_again();
        install(&file, b"newer", 0o644);
        looker.look_again();
        let first = found.recv_timeout(WAIT).unwrap();
        assert_eq!(first.identity, Identity::of(&file).unwrap());
        // `oso-restart` would refuse a file it cannot run, and the notice says why.
        let why = first.refused.as_deref().unwrap();
        assert!(why.contains("not an executable file"), "{why}");
        // The same file is found again; the notice says it once.
        looker.look_again();
        assert_eq!(found.recv_timeout(WAIT).unwrap(), first);
        // One that others can change is refused for that. A look asked for above may post
        // the file before it once more.
        install(&file, b"newest", 0o757);
        let newest = Identity::of(&file).unwrap();
        looker.look_again();
        let why = std::iter::from_fn(|| found.recv_timeout(WAIT).ok())
            .find(|f| f.identity == newest)
            .and_then(|f| f.refused)
            .unwrap();
        assert!(why.contains("someone else can change"), "{why}");
        // The thread ends with its looker, and lets go of what it posts through, once the looks
        // asked for before have posted.
        drop(looker);
        let end = std::iter::from_fn(|| Some(found.recv_timeout(WAIT))).find_map(Result::err);
        assert_eq!(end, Some(RecvTimeoutError::Disconnected));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_file_says_nothing() {
        let dir = scratch("missing");
        let file = dir.join("oso");
        fs::write(&file, b"old").unwrap();
        let (looker, found) = looker(&file);
        fs::remove_file(&file).unwrap();
        looker.look_again();
        // Nothing for the missing file, so the first post is the one put there after.
        install(&file, b"back", 0o644);
        looker.look_again();
        assert_eq!(
            found.recv_timeout(WAIT).unwrap().identity,
            Identity::of(&file).unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn each_identity_is_said_once_and_a_refusal_says_why() {
        let dir = scratch("said");
        let file = dir.join("oso");
        fs::write(&file, b"a").unwrap();
        let a = Identity::of(&file).unwrap();
        fs::write(&file, b"bb").unwrap();
        let b = Identity::of(&file).unwrap();
        assert_ne!(a, b);
        let mut notice = Notice::default();
        let fine = Found {
            identity: a,
            refused: None,
        };
        let said = notice.say(&fine).unwrap();
        assert!(said.contains("M-x oso-restart would start it"), "{said}");
        assert_eq!(notice.say(&fine), None);
        let refused = Found {
            identity: b,
            refused: Some("someone else can change /x/oso".into()),
        };
        let said = notice.say(&refused).unwrap();
        assert!(
            said.ends_with("would refuse it: someone else can change /x/oso"),
            "{said}"
        );
        assert_eq!(notice.say(&refused), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
