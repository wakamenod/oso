//! Images the person attaches to a hosted session from the clipboard (`session-yank-image`,
//! `C-c C-y` in session mode, ADR 0142): where they are kept, what they are called, the PNG a
//! TIFF becomes, the `@path` reference pasted into the session, and how long they stay.
//!
//! Must not depend on: `app`, the clipboard's threads or the UI thread. [`save`], [`sweep`] and
//! the [`Keeper`]'s jobs touch the disk, and [`save`] may run `sips`: they run on threads of their own,
//! never the sessions worker's, which judges the sessions' screens.
//!
//! - Each session has a private directory (`0700`) under the per-user cache,
//!   `oso/images/<session id>`, and each image is a new file in it (`0600`, made with
//!   `create_new`, so never through a link), named by the time in UTC as
//!   `YYYYMMDD-HHMMSS-mmm`, with `-2` to `-9` after it if that name is taken.
//! - The image goes by path, never inline: Claude Code reads the file the reference names when
//!   the prompt is sent, and base64 in the prompt would sit in the conversation's recording.
//! - A TIFF is converted to PNG by macOS's `/usr/bin/sips`, in a child process with a time
//!   limit, so that decoding another app's image never runs in oso's own process, which holds
//!   every buffer. The other types are copied as they came and never decoded.
//! - A session's directory goes when the session is archived or dropped from the record, and
//!   files older than [`KEEP`] when oso starts and after each image is kept.
//! - Nothing is removed through a link: a directory is removed only once it and the way to it
//!   pass [`PrivateDir::existing`].

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_platform::clipboard::{Image, ImageKind};
use oso_platform::privacy::PrivateDir;
use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::identity::SessionId;

/// How long an image is kept at most: files older than this go when oso starts.
pub const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long `sips` may take to convert a TIFF: it takes a few hundredths of a second for a
/// screenshot.
const CONVERT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a conversion is looked at while it runs.
const CONVERT_POLL: Duration = Duration::from_millis(10);

/// macOS's image tool, at the one path it ships at.
const SIPS: &str = "/usr/bin/sips";

/// How many entries a sweep looks at in all, so that directories filled by something else
/// cost a bounded walk.
const MAX_SWEPT: usize = 4096;

/// Names tried for one image: the time, then `-2` to `-9` after it.
const NAMES_TRIED: u32 = 9;

/// Where the images are kept: `oso/images` in the per-user cache directory. `None` with no
/// cache directory to go by.
pub fn root() -> Option<PathBuf> {
    oso_platform::privacy::cache_home().map(|cache| cache.join("oso").join("images"))
}

/// The directory of session `id`'s images under `root`. `None` for an id that is not a plain
/// name, which no session made by oso has.
pub fn session_directory(root: &Path, id: &SessionId) -> Option<PathBuf> {
    plain_name(id.as_str()).then(|| root.join(id.as_str()))
}

/// Whether `name` is a session id as oso makes them, safe as one path component.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// What is pasted into the session for the image at `path`: `@path`, with a space before it,
/// as oso cannot tell what is left of Claude Code's cursor and a mention must start a word, and
/// one after it, so that what is typed next is not part of the path. Refused for a path an
/// `@` reference cannot hold: one with a space or a control character, or not UTF-8.
pub fn reference(path: &Path) -> Result<String, String> {
    let Some(text) = path.to_str() else {
        return Err(format!(
            "The image directory's path is not UTF-8, which an @ reference cannot hold: {}",
            path.display()
        ));
    };
    if text.chars().any(|c| {
        c.is_whitespace() || c.is_control() || oso_core::display_map::cell::is_invisible(c)
    }) {
        return Err(format!(
            "The image directory's path has a space or a control character, which an @ \
             reference cannot hold: {}",
            text.escape_debug()
        ));
    }
    Ok(format!(" @{text} "))
}

/// The name of an image kept at `at`, without its extension: `YYYYMMDD-HHMMSS-mmm` in the time
/// zone `offset` seconds east of UTC.
pub fn stem(at: SystemTime, offset: i32) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = i64::try_from(since.as_secs()).unwrap_or(i64::MAX - 86_400) + i64::from(offset);
    let (days, of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = crate::dired::civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{:03}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60,
        since.subsec_millis()
    )
}

/// The thread that keeps and removes the images, one job after another, so that a session's
/// directory is never removed while an image is written into it. It starts with its first job,
/// and a new one takes over from one that ended. What goes wrong is logged without paths.
#[derive(Default)]
pub struct Keeper(Option<Sender<Work>>);

impl std::fmt::Debug for Keeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keeper").finish_non_exhaustive()
    }
}

/// What is told how keeping an image went, on the keeper's thread.
pub type Done = Box<dyn FnOnce(Result<Kept, String>) + Send>;

/// A job of the [`Keeper`]'s.
enum Work {
    /// Keep `image` for session `id` under `root`, say how it went, then sweep `root`.
    Save {
        root: PathBuf,
        id: SessionId,
        image: Image,
        done: Done,
    },
    /// Remove the images older than [`KEEP`] under this root.
    Sweep(PathBuf),
    /// Remove session `id`'s images under `root`: it was archived or dropped.
    Session { root: PathBuf, id: SessionId },
    /// Remove this image, which no paste refers to.
    File(PathBuf),
}

impl Keeper {
    /// Keeps `image` for session `id` under `root`, then calls `done` with how it went, on the
    /// keeper's thread. Hands the image back if there is no thread to do it.
    pub fn save(
        &mut self,
        root: PathBuf,
        id: SessionId,
        image: Image,
        done: Done,
    ) -> Result<(), Image> {
        let work = Work::Save {
            root,
            id,
            image,
            done,
        };
        match self.send(work) {
            Ok(()) => Ok(()),
            Err(Work::Save { image, .. }) => Err(image),
            Err(_) => Ok(()),
        }
    }

    /// Removes the images under `root` older than [`KEEP`].
    pub fn sweep(&mut self, root: PathBuf) {
        let _ = self.send(Work::Sweep(root));
    }

    /// Removes session `id`'s images under `root`.
    pub fn drop_session(&mut self, root: PathBuf, id: SessionId) {
        let _ = self.send(Work::Session { root, id });
    }

    /// Removes the image at `path`.
    pub fn drop_file(&mut self, path: PathBuf) {
        let _ = self.send(Work::File(path));
    }

    /// Sends `work` to the thread, starting one first if none runs. Gives the work back if no
    /// thread can be started.
    fn send(&mut self, work: Work) -> Result<(), Work> {
        let work = match &self.0 {
            Some(jobs) => match jobs.send(work) {
                Ok(()) => return Ok(()),
                // Its thread ended, with a panic: a new one takes over.
                Err(back) => back.0,
            },
            None => work,
        };
        let (jobs, inbox) = mpsc::channel::<Work>();
        let started = thread::Builder::new()
            .name("oso-images".into())
            .spawn(move || inbox.into_iter().for_each(run));
        if let Err(e) = started {
            log::warn!("could not start the thread that keeps images: {e}");
            self.0 = None;
            return Err(work);
        }
        let sent = jobs.send(work).map_err(|back| back.0);
        self.0 = Some(jobs);
        sent
    }
}

/// Does one job of the keeper's.
fn run(work: Work) {
    let removed = match work {
        Work::Save {
            root,
            id,
            image,
            done,
        } => {
            let now = SystemTime::now();
            let offset = oso_platform::file_meta::utc_offset_at(now).unwrap_or(0);
            // A panic in keeping it still says so, so that the next yank is not refused as
            // waiting for this one.
            let kept = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                save(&root, &id, &image, now, offset)
            }))
            .unwrap_or_else(|_| Err("Cannot keep the image: keeping it failed".to_owned()));
            drop(image);
            done(kept);
            // Images past their week go while oso runs, too.
            sweep(&root, now);
            Ok(())
        }
        Work::Sweep(root) => {
            let removed = sweep(&root, SystemTime::now());
            if removed > 0 {
                log::info!("removed {removed} images kept more than a week");
            }
            Ok(())
        }
        Work::Session { root, id } => delete_session(&root, &id),
        Work::File(path) => delete_file(&path),
    };
    if let Err(e) = removed {
        log::warn!("could not remove a session's images: {e}");
    }
}

/// An image kept for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// The file.
    pub path: PathBuf,
    /// What is pasted into the session for it ([`reference`]).
    pub reference: String,
    /// It came as TIFF and was converted to PNG.
    pub converted: bool,
}

/// Keeps `image` in session `id`'s directory under `root`, as a new file named by `now` in the
/// time zone `offset` seconds east of UTC, converted to PNG first if it is a TIFF. Says why not,
/// in words for the echo area, if it cannot be kept, and leaves no file behind then.
pub fn save(
    root: &Path,
    id: &SessionId,
    image: &Image,
    now: SystemTime,
    offset: i32,
) -> Result<Kept, String> {
    let directory = session_directory(root, id)
        .ok_or_else(|| "That session's id cannot name a directory".to_owned())?;
    // Before anything is written: a path the reference cannot hold makes the file useless.
    reference(&directory.join("x.png"))?;
    let directory =
        PrivateDir::open(directory).map_err(|e| format!("Cannot keep the image: {e}"))?;
    let stem = stem(now, offset);
    let mut tried = None;
    for attempt in 1..=NAMES_TRIED {
        let name = if attempt == 1 {
            stem.clone()
        } else {
            format!("{stem}-{attempt}")
        };
        let kept = match image.kind {
            ImageKind::Tiff => keep_converted(directory.path(), &name, &image.bytes),
            kind => {
                let path = directory
                    .path()
                    .join(format!("{name}.{}", kind.extension()));
                write_new(&path, &image.bytes).map(|()| (path, false))
            }
        };
        match kept {
            Ok((path, converted)) => {
                let reference = reference(&path)?;
                return Ok(Kept {
                    path,
                    reference,
                    converted,
                });
            }
            Err(Failed::Taken) => tried = Some(name),
            Err(Failed::Said(why)) => return Err(why),
        }
    }
    Err(format!(
        "Cannot keep the image: every name up to {} is taken",
        tried.unwrap_or(stem)
    ))
}

/// Why one name could not be used.
enum Failed {
    /// A file of that name is there: the next is tried.
    Taken,
    /// Why the image cannot be kept.
    Said(String),
}

impl From<io::Error> for Failed {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Self::Taken
        } else {
            Self::Said(format!("Cannot keep the image: {e}"))
        }
    }
}

/// `bytes` in a new file at `path`, mode `0600` from the start, made there and never through a
/// link. A file that could not be written whole is removed.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Failed> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    // Not synced: the file is a cache's, read by Claude Code seconds later, and one lost to a
    // power cut costs nothing.
    if let Err(e) = file.write_all(bytes) {
        let _ = fs::remove_file(path);
        return Err(Failed::Said(format!("Cannot keep the image: {e}")));
    }
    Ok(())
}

/// The TIFF `bytes` as `name.png` in `directory`, through `name.tiff`, which is removed
/// whatever happens. The PNG's name is taken first, as an empty file of this process's own, so
/// that `sips` writes over nothing else.
fn keep_converted(directory: &Path, name: &str, bytes: &[u8]) -> Result<(PathBuf, bool), Failed> {
    let png = directory.join(format!("{name}.png"));
    write_new(&png, &[])?;
    let tiff = directory.join(format!("{name}.tiff"));
    if let Err(failed) = write_new(&tiff, bytes) {
        let _ = fs::remove_file(&png);
        return Err(failed);
    }
    let converted = convert(&tiff, &png);
    let _ = fs::remove_file(&tiff);
    match converted {
        Ok(()) => Ok((png, true)),
        Err(why) => {
            let _ = fs::remove_file(&png);
            Err(Failed::Said(why))
        }
    }
}

/// Converts the TIFF at `tiff` to the PNG at `png` with `sips`, in a child process that is
/// killed after [`CONVERT_TIMEOUT`]. The PNG is made `0600` once it is there.
fn convert(tiff: &Path, png: &Path) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "The clipboard holds only a TIFF, and nothing here converts it to PNG".to_owned(),
        );
    }
    let sips = Executable::absolute(SIPS)
        .map_err(|e| format!("Cannot convert the TIFF to PNG: {SIPS}: {e}"))?;
    let args: [OsString; 6] = [
        "-s".into(),
        "format".into(),
        "png".into(),
        tiff.into(),
        "--out".into(),
        png.into(),
    ];
    let mut child = spawn(&Spawn {
        executable: &sips,
        args: &args,
        cwd: tiff.parent(),
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: false,
        set: &[],
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .map_err(|e| format!("Cannot convert the TIFF to PNG: {e}"))?;
    let deadline = Instant::now() + CONVERT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(CONVERT_POLL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Converting the TIFF to PNG took too long".to_owned());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Cannot convert the TIFF to PNG: {e}"));
            }
        }
    };
    // sips exits 0 having written nothing when it cannot read its input: the file tells.
    let written =
        fs::symlink_metadata(png).is_ok_and(|meta| meta.file_type().is_file() && meta.len() > 0);
    if !status.success() || !written {
        return Err("Could not convert the TIFF to PNG: sips found no image in it".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(png, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Cannot keep the image: {e}"))?;
    }
    Ok(())
}

/// Removes session `id`'s directory under `root` and what is in it, if it is there and it and
/// the way to it are private: a link anywhere on the way is refused, not followed.
pub fn delete_session(root: &Path, id: &SessionId) -> io::Result<()> {
    let Some(directory) = session_directory(root, id) else {
        return Ok(());
    };
    match PrivateDir::existing(directory)? {
        Some(directory) => fs::remove_dir_all(directory.path()),
        None => Ok(()),
    }
}

/// Removes the image at `path`, which a paste refused, if the directory it is in and the way to
/// it are private.
pub fn delete_file(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if PrivateDir::existing(parent)?.is_none() {
        return Ok(());
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Removes the images under `root` last changed before `now` less [`KEEP`], and each session's
/// directory left empty. Only files with the extensions [`save`] writes, in directories named
/// as sessions are, are looked at, at most [`MAX_SWEPT`] entries in all. A `root` that is not
/// private is left alone, and so is a session's directory that is not. Returns how many files
/// went.
pub fn sweep(root: &Path, now: SystemTime) -> usize {
    match PrivateDir::existing(root) {
        Ok(Some(_)) => {}
        Ok(None) => return 0,
        Err(e) => {
            log::warn!("the image directory is not swept: {e}");
            return 0;
        }
    }
    let Some(before) = now.checked_sub(KEEP) else {
        return 0;
    };
    let mut removed = 0;
    let mut budget = MAX_SWEPT;
    for directory in entries(root, &mut budget) {
        let plain = directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(plain_name);
        if !plain || !matches!(PrivateDir::existing(&directory), Ok(Some(_))) {
            continue;
        }
        for file in entries(&directory, &mut budget) {
            let ours = file
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    ImageKind::ALL
                        .iter()
                        .any(|kind| kind.extension() == extension)
                });
            let old = fs::symlink_metadata(&file).is_ok_and(|meta| {
                meta.file_type().is_file() && meta.modified().is_ok_and(|at| at < before)
            });
            if ours && old && fs::remove_file(&file).is_ok() {
                removed += 1;
            }
        }
        // Fails, as it should, while anything is left in it.
        let _ = fs::remove_dir(&directory);
    }
    removed
}

/// The paths of at most `budget` entries of `directory`, taken from it; none if it cannot be
/// read.
fn entries(directory: &Path, budget: &mut usize) -> Vec<PathBuf> {
    let found: Vec<PathBuf> = fs::read_dir(directory)
        .map(|read| {
            read.take(*budget)
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect()
        })
        .unwrap_or_default();
    *budget -= found.len().min(*budget);
    found
}

#[cfg(all(test, unix))]
mod tests;
