//! Images the person attaches to a hosted session from the clipboard (`session-yank-image`,
//! `C-c C-y` in session mode, ADR 0142): where they are kept, what they are called, the PNG a
//! TIFF becomes, the `@path` reference pasted into the session, and how long they stay.
//!
//! Must not depend on: `app`, the clipboard's threads or the UI thread. [`save`], [`sweep`] and
//! [`delete_session`] touch the disk, and [`save`] may run `sips`: they run on threads of their
//! own.
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
//!   files older than [`KEEP`] when oso starts.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_platform::clipboard::{Image, ImageKind};
use oso_platform::privacy::PrivateDir;
use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::identity::SessionId;
use super::worker::SessionKey;

/// How long an image is kept at most: files older than this go when oso starts.
pub const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long `sips` may take to convert a TIFF: it takes a few hundredths of a second for a
/// screenshot.
const CONVERT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a conversion is looked at while it runs.
const CONVERT_POLL: Duration = Duration::from_millis(10);

/// macOS's image tool, at the one path it ships at.
const SIPS: &str = "/usr/bin/sips";

/// How many entries a sweep looks at in a directory, so that a directory filled by something
/// else costs a bounded walk.
const MAX_SWEPT: usize = 4096;

/// Names tried for one image: the time, then `-2` to `-9` after it.
const NAMES_TRIED: u32 = 9;

/// The extensions a sweep removes: those [`save`] writes.
const KEPT_EXTENSIONS: [&str; 5] = ["png", "jpg", "gif", "webp", "tiff"];

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

/// The name of an image kept at `at`, without its extension: `YYYYMMDD-HHMMSS-mmm` in UTC.
pub fn stem(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since.as_secs();
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{:03}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60,
        since.subsec_millis()
    )
}

/// The date `days` after 1970-01-01, as (year, month, day): Howard Hinnant's
/// `civil_from_days`, for the days from the epoch on.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// What a thread that keeps an image says, for session `key`.
#[derive(Debug)]
pub struct Report {
    /// The session the image is for.
    pub key: SessionKey,
    /// The image kept, or why it was not.
    pub kept: Result<Kept, String>,
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

/// Keeps `image` in session `id`'s directory under `root`, as a new file named by `now`,
/// converted to PNG first if it is a TIFF. Says why not, in words for the echo area, if it
/// cannot be kept, and leaves no file behind then.
pub fn save(root: &Path, id: &SessionId, image: &Image, now: SystemTime) -> Result<Kept, String> {
    let directory = session_directory(root, id)
        .ok_or_else(|| "That session's id cannot name a directory".to_owned())?;
    // Before anything is written: a path the reference cannot hold makes the file useless.
    reference(&directory.join("x.png"))?;
    let directory =
        PrivateDir::open(directory).map_err(|e| format!("Cannot keep the image: {e}"))?;
    let stem = stem(now);
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
    if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(Failed::Said(format!("Cannot keep the image: {e}")));
    }
    Ok(())
}

/// The TIFF `bytes` as `name.png` in `directory`, through `name.tiff`, which is removed
/// whatever happens. The PNG's name is checked free first, so that `sips` never writes over a
/// file.
fn keep_converted(directory: &Path, name: &str, bytes: &[u8]) -> Result<(PathBuf, bool), Failed> {
    let png = directory.join(format!("{name}.png"));
    match fs::symlink_metadata(&png) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err(Failed::Taken),
        Err(e) => return Err(e.into()),
    }
    let tiff = directory.join(format!("{name}.tiff"));
    write_new(&tiff, bytes)?;
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
        set: &[],
        terminal: None,
        pass_fd: None,
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
            Err(e) => return Err(format!("Cannot convert the TIFF to PNG: {e}")),
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

/// Removes session `id`'s directory under `root` and what is in it, if it is there. A link in
/// its place is removed, not followed.
pub fn delete_session(root: &Path, id: &SessionId) -> io::Result<()> {
    let Some(directory) = session_directory(root, id) else {
        return Ok(());
    };
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(meta) if meta.file_type().is_dir() => fs::remove_dir_all(&directory),
        Ok(_) => fs::remove_file(&directory),
    }
}

/// Removes the image at `path`, which a paste refused.
pub fn delete_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Removes the images under `root` last changed before `now` less [`KEEP`], and each session's
/// directory left empty. Only files with the extensions [`save`] writes, in directories named
/// as sessions are, are looked at, at most [`MAX_SWEPT`] entries a directory. A `root` that
/// is not private is left alone. Returns how many files went.
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
    for directory in entries(root) {
        let plain = directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(plain_name);
        let is_dir = fs::symlink_metadata(&directory).is_ok_and(|meta| meta.file_type().is_dir());
        if !(plain && is_dir) {
            continue;
        }
        for file in entries(&directory) {
            let ours = file
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| KEPT_EXTENSIONS.contains(&extension));
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

/// The paths of at most [`MAX_SWEPT`] entries of `directory`; none if it cannot be read.
fn entries(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .map(|read| {
            read.take(MAX_SWEPT)
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(all(test, unix))]
mod tests;
