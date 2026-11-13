//! The theme shown last, remembered in `oso/theme` in the per-user state directory, beside
//! the emergency copies' directory (ADR 0089): its name on the first line, and on the second
//! the configuration's default theme when it was shown (ADR 0108). It is read on a thread of
//! its own, started first thing and collected as the window opens without waiting, and
//! written on another after a switch, so the UI thread never waits on it. A read not done by
//! then draws the default theme.
//!
//! Must not depend on: the GPU or the window; it reads and writes one file.

use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use oso_platform::file_meta;
use oso_platform::privacy::{PrivateDir, has_one_name, is_this_users};

/// The file's name in its directory.
const FILE: &str = "theme";
/// Where a write goes first, so that the file is whole or not there.
const PART: &str = "theme.part";
/// The most a theme's file is read of: two names and their line breaks, and room to spare.
const MOST_BYTES: u64 = 256;

/// Whether `name` could be a theme's: lower-case letters, digits and `-`, as the
/// built-in ones are named.
fn is_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What the file remembers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Remembered {
    /// The theme shown last.
    pub(super) name: String,
    /// The configuration's default theme when it was, or `None` from a file of one line,
    /// written before the configuration set the theme.
    pub(super) default: Option<String>,
}

/// The remembered theme, being read on a thread of its own.
pub struct ThemeRead {
    name: Option<mpsc::Receiver<Option<Remembered>>>,
}

impl ThemeRead {
    /// Starts reading the name remembered in `directory`. With no directory, there is none.
    pub fn start(directory: Option<PathBuf>) -> Self {
        match directory {
            Some(directory) => Self::spawn(move || read(&directory)),
            None => Self { name: None },
        }
    }

    fn spawn(read: impl FnOnce() -> Option<Remembered> + Send + 'static) -> Self {
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

    /// What was read, if the read is done. It never waits: a read still going leaves the
    /// default theme, and a thread that never ends holds nothing of the app's.
    pub(super) fn take(self) -> Option<Remembered> {
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

/// What is remembered in `directory`, if the directory is private and the file there is a
/// small one of this user's that holds a name, and on a second line another, or nothing.
/// Anything else gives `None`, and the default theme is drawn.
fn read(directory: &Path) -> Option<Remembered> {
    let directory = match PrivateDir::existing(directory) {
        Ok(directory) => directory?,
        Err(e) => {
            log::warn!("not reading the remembered theme: {e}");
            return None;
        }
    };
    let path = directory.path().join(FILE);
    // Opened only if it is a regular file, without waiting, and judged by what was opened.
    let opened = match file_meta::open_regular_here(&path) {
        Ok(opened) => opened,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("not reading the remembered theme: {e}");
            return None;
        }
    };
    let file = opened.and_then(|(file, meta)| {
        let plain = is_this_users(&meta) && has_one_name(&meta) && meta.len() <= MOST_BYTES;
        plain.then_some(file)
    });
    let Some(file) = file else {
        log::warn!(
            "not reading the remembered theme: {} is not a small regular file of its own",
            path.display()
        );
        return None;
    };
    let mut text = String::new();
    file.take(MOST_BYTES).read_to_string(&mut text).ok()?;
    let lines = text.strip_suffix('\n').unwrap_or(&text);
    let (name, default) = match lines.split_once('\n') {
        Some((name, default)) => (name, Some(default)),
        None => (lines, None),
    };
    if !is_name(name) || !default.is_none_or(is_name) {
        log::warn!(
            "not reading the remembered theme: {} holds no theme's name",
            path.display()
        );
        return None;
    }
    Some(Remembered {
        name: name.to_owned(),
        default: default.map(str::to_owned),
    })
}

/// Writes `remembered` in `directory`, which is made private if it is missing. The file is
/// replaced whole, or left as it was.
fn write(directory: &Path, remembered: &Remembered) -> io::Result<()> {
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
    let mut text = format!("{}\n", remembered.name);
    if let Some(default) = &remembered.default {
        text.push_str(default);
        text.push('\n');
    }
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&part, directory.path().join(FILE))
}

/// Writes what it is given on a thread of its own, only the newest of what waits.
pub(super) struct Writer {
    names: Option<mpsc::Sender<Remembered>>,
}

impl Writer {
    /// A writer into `directory`, or one that writes nothing with no directory or no thread.
    pub(super) fn start(directory: Option<PathBuf>) -> Self {
        let Some(directory) = directory else {
            return Self { names: None };
        };
        let (names, waiting) = mpsc::channel::<Remembered>();
        let spawned = thread::Builder::new()
            .name("oso-theme".into())
            .spawn(move || {
                while let Ok(mut remembered) = waiting.recv() {
                    while let Ok(newer) = waiting.try_recv() {
                        remembered = newer;
                    }
                    if let Err(e) = write(&directory, &remembered) {
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

    /// Remembers `remembered`, soon.
    pub(super) fn remember(&self, remembered: Remembered) {
        if let Some(names) = &self.names {
            let _ = names.send(remembered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// `name`, shown while the configuration's default was `default`.
    fn shown(name: &str, default: Option<&str>) -> Remembered {
        Remembered {
            name: name.to_owned(),
            default: default.map(str::to_owned),
        }
    }

    /// Writes `name`, shown while the configuration's default was the same.
    fn write_name(directory: &Path, name: &str) {
        write(directory, &shown(name, Some(name))).unwrap();
    }

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
    fn what_is_written_is_read_back_and_replaces_the_last() {
        let scratch = Scratch::new();
        assert_eq!(read(&scratch.oso()), None, "nothing remembered yet");
        write(&scratch.oso(), &shown("light", Some("dusk"))).unwrap();
        assert_eq!(read(&scratch.oso()), Some(shown("light", Some("dusk"))));
        assert_eq!(
            fs::read_to_string(scratch.oso().join(FILE)).unwrap(),
            "light\ndusk\n"
        );
        write(&scratch.oso(), &shown("dusk", Some("light"))).unwrap();
        assert_eq!(read(&scratch.oso()), Some(shown("dusk", Some("light"))));
        assert!(!scratch.oso().join(PART).exists());
    }

    #[test]
    fn a_file_of_one_line_from_before_has_no_default() {
        let scratch = Scratch::new();
        write_name(&scratch.oso(), "dark");
        let path = scratch.oso().join(FILE);
        for text in ["light\n", "light"] {
            fs::write(&path, text).unwrap();
            assert_eq!(read(&scratch.oso()), Some(shown("light", None)), "{text:?}");
        }
        fs::write(&path, "light\ndusk").unwrap();
        assert_eq!(read(&scratch.oso()), Some(shown("light", Some("dusk"))));
    }

    #[test]
    fn what_is_no_name_is_not_read() {
        let scratch = Scratch::new();
        write_name(&scratch.oso(), "dark");
        let path = scratch.oso().join(FILE);
        let texts = [
            "",
            "\n",
            "Dark\n",
            "dark mode\n",
            "../dark\n",
            "dark\n\n",
            "dark\nDusk\n",
            "dark\ndusk\nlight\n",
            "\ndusk\n",
        ];
        for text in texts {
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
            Some(shown("light", None))
        });
        assert_eq!(slow.take(), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_name_is_read_on_its_own_thread() {
        let scratch = Scratch::new();
        write_name(&scratch.oso(), "dusk");
        let read = ThemeRead::start(Some(scratch.oso()));
        let name = read.name.unwrap().recv_timeout(Duration::from_secs(5));
        assert_eq!(name, Ok(Some(shown("dusk", Some("dusk")))));
        assert_eq!(ThemeRead::start(None).take(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_or_a_directory_in_its_place_is_not_read() {
        let scratch = Scratch::new();
        write_name(&scratch.oso(), "dark");
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
    fn a_pipe_in_its_place_is_refused_without_waiting() {
        use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
        let scratch = Scratch::new();
        write_name(&scratch.oso(), "light");
        let path = scratch.oso().join(FILE);
        fs::remove_file(&path).unwrap();
        let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
        let status = spawn(&Spawn {
            executable: &mkfifo,
            args: &[path.into_os_string()],
            cwd: None,
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[],
            terminal: None,
        })
        .and_then(|mut child| child.wait())
        .expect("cannot run mkfifo");
        assert!(status.success());
        // Nobody writes to the pipe: a read that waited on it would never end.
        let read = ThemeRead::start(Some(scratch.oso()));
        let name = read.name.unwrap().recv_timeout(Duration::from_secs(5));
        assert_eq!(name, Ok(None), "the default theme is drawn");
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_others_can_reach_is_not_read() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        write_name(&scratch.oso(), "light");
        fs::set_permissions(scratch.oso(), fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(read(&scratch.oso()), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_this_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        write_name(&scratch.oso(), "dark");
        let mode = fs::metadata(scratch.oso().join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
