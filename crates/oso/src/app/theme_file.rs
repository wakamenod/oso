//! The theme chosen last, remembered in one line of `oso/theme` in the per-user state
//! directory, beside the emergency copies' directory, until configuration exists
//! (ADR 0089). It is read on a thread of its own, started first thing and collected as the
//! window opens without waiting, and written on another after a switch, so the UI thread
//! never waits on it. A read not done by then draws the default theme.
//!
//! Must not depend on: the GPU or the window; it reads and writes one file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use oso_platform::privacy::{PrivateDir, has_one_name, is_this_users};

/// The file's name in its directory.
const FILE: &str = "theme";
/// Where a write goes first, so that the file is whole or not there.
const PART: &str = "theme.part";
/// The most a theme's file is read of: a name and a line break, and room to spare.
const MOST_BYTES: u64 = 256;

/// Whether `name` could be a theme's: lower-case letters, digits and `-`, as the
/// built-in ones are named.
fn is_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The remembered name, being read on a thread of its own.
pub struct ThemeRead {
    name: Option<mpsc::Receiver<Option<String>>>,
}

impl ThemeRead {
    /// Starts reading the name remembered in `directory`. With no directory, there is none.
    pub fn start(directory: Option<PathBuf>) -> Self {
        match directory {
            Some(directory) => Self::spawn(move || read(&directory)),
            None => Self { name: None },
        }
    }

    fn spawn(read: impl FnOnce() -> Option<String> + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("oso-theme-read".into())
            .spawn(move || {
                let _ = sender.send(read());
            });
        match spawned {
            Ok(_) => Self {
                name: Some(receiver),
            },
            Err(e) => {
                log::warn!("not reading the remembered theme: {e}");
                Self { name: None }
            }
        }
    }

    /// The name read, if the read is done. It never waits: a read still going leaves the
    /// default theme, and a thread that never ends holds nothing of the app's.
    pub(super) fn take(self) -> Option<String> {
        match self.name?.try_recv() {
            Ok(name) => name,
            Err(mpsc::TryRecvError::Empty) => {
                log::warn!("the remembered theme was not read in time; drawing the default");
                None
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }
}

/// The name remembered in `directory`, if the directory is private and the file there is a
/// small one of this user's that holds one. Anything else gives `None`, and the default
/// theme is drawn.
fn read(directory: &Path) -> Option<String> {
    let directory = match PrivateDir::existing(directory) {
        Ok(directory) => directory?,
        Err(e) => {
            log::warn!("not reading the remembered theme: {e}");
            return None;
        }
    };
    let path = directory.path().join(FILE);
    let meta = fs::symlink_metadata(&path).ok()?;
    let plain = meta.is_file() && is_this_users(&meta) && has_one_name(&meta);
    if !plain || meta.len() > MOST_BYTES {
        log::warn!(
            "not reading the remembered theme: {} is not a small file of its own",
            path.display()
        );
        return None;
    }
    let mut text = String::new();
    File::open(&path)
        .ok()?
        .take(MOST_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    let name = text.strip_suffix('\n').unwrap_or(&text);
    if !is_name(name) {
        log::warn!(
            "not reading the remembered theme: {} holds no theme's name",
            path.display()
        );
        return None;
    }
    Some(name.to_owned())
}

/// Writes `name` in `directory`, which is made private if it is missing. The file is
/// replaced whole, or left as it was.
fn write(directory: &Path, name: &str) -> io::Result<()> {
    let directory = PrivateDir::open(directory)?;
    let part = directory.path().join(PART);
    match fs::remove_file(&part) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&part)?;
    file.write_all(format!("{name}\n").as_bytes())?;
    file.sync_all()?;
    fs::rename(&part, directory.path().join(FILE))
}

/// Writes the names it is given on a thread of its own, only the newest of those waiting.
pub(super) struct Writer {
    names: Option<mpsc::Sender<String>>,
}

impl Writer {
    /// A writer into `directory`, or one that writes nothing with no directory or no thread.
    pub(super) fn start(directory: Option<PathBuf>) -> Self {
        let Some(directory) = directory else {
            return Self { names: None };
        };
        let (names, waiting) = mpsc::channel::<String>();
        let spawned = thread::Builder::new()
            .name("oso-theme".into())
            .spawn(move || {
                while let Ok(mut name) = waiting.recv() {
                    while let Ok(newer) = waiting.try_recv() {
                        name = newer;
                    }
                    if let Err(e) = write(&directory, &name) {
                        log::warn!("could not remember the theme: {e}");
                    }
                }
            });
        match spawned {
            Ok(_) => Self { names: Some(names) },
            Err(e) => {
                log::warn!("the theme will not be remembered: {e}");
                Self { names: None }
            }
        }
    }

    /// Remembers `name`, soon.
    pub(super) fn remember(&self, name: &str) {
        if let Some(names) = &self.names {
            let _ = names.send(name.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A fresh private directory for one test, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let test = thread::current();
            let test = test.name().unwrap_or("test").replace("::", "-");
            let name = format!("oso-theme-{}-{test}", std::process::id());
            let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }

        fn oso(&self) -> PathBuf {
            self.0.join("oso")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_name_written_is_read_back_and_replaces_the_last() {
        let scratch = Scratch::new();
        assert_eq!(read(&scratch.oso()), None, "nothing remembered yet");
        write(&scratch.oso(), "light").unwrap();
        assert_eq!(read(&scratch.oso()).as_deref(), Some("light"));
        write(&scratch.oso(), "dusk").unwrap();
        assert_eq!(read(&scratch.oso()).as_deref(), Some("dusk"));
        assert!(!scratch.oso().join(PART).exists());
    }

    #[test]
    fn what_is_no_name_is_not_read() {
        let scratch = Scratch::new();
        write(&scratch.oso(), "dark").unwrap();
        let path = scratch.oso().join(FILE);
        for text in ["", "\n", "Dark\n", "dark mode\n", "../dark\n", "dark\n\n"] {
            fs::write(&path, text).unwrap();
            assert_eq!(read(&scratch.oso()), None, "{text:?}");
        }
        fs::write(&path, "a".repeat(300)).unwrap();
        assert_eq!(read(&scratch.oso()), None, "too long");
    }

    #[test]
    fn a_read_not_done_gives_the_default_without_waiting_for_it() {
        let started = std::time::Instant::now();
        let slow = ThemeRead::spawn(|| {
            thread::sleep(Duration::from_secs(5));
            Some("light".to_owned())
        });
        assert_eq!(slow.take(), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_name_is_read_on_its_own_thread() {
        let scratch = Scratch::new();
        write(&scratch.oso(), "dusk").unwrap();
        let read = ThemeRead::start(Some(scratch.oso()));
        let name = read.name.unwrap().recv_timeout(Duration::from_secs(5));
        assert_eq!(name, Ok(Some("dusk".to_owned())));
        assert_eq!(ThemeRead::start(None).take(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_or_a_directory_in_its_place_is_not_read() {
        let scratch = Scratch::new();
        write(&scratch.oso(), "dark").unwrap();
        let path = scratch.oso().join(FILE);
        let elsewhere = scratch.0.join("elsewhere");
        fs::write(&elsewhere, "light\n").unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert_eq!(read(&scratch.oso()), None, "a link");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(read(&scratch.oso()), None, "a directory");
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_others_can_reach_is_not_read() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        write(&scratch.oso(), "light").unwrap();
        fs::set_permissions(scratch.oso(), fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(read(&scratch.oso()), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_this_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        write(&scratch.oso(), "dark").unwrap();
        let mode = fs::metadata(scratch.oso().join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
