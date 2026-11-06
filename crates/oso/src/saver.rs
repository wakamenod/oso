//! Safe save: writing a buffer's snapshot over its file on a background thread
//! (ADR 0013, ADR 0029, ADR 0059). Losing or damaging a file is what this is
//! designed against.
//!
//! Must not depend on: `app`, winit or the GPU; the caller supplies the report's way
//! back. [`run`] needs no thread, so tests can save deterministically.
//!
//! It shares with `loader` how a path resolves and what is recorded of a file, as they
//! must agree.
//!
//! The order, each step before the next:
//!
//! 1. Follow symbolic links to the file's real place, so that a link stays a link.
//! 2. Check the baseline, so that a path now leading elsewhere writes nothing there, and
//!    a file already changed costs no write.
//! 3. Create a temporary file with `create_new` and mode 0600 in that file's
//!    directory, so that the rename stays on one file system.
//! 4. Write the BOM and the text. Line endings are already in the text.
//! 5. Give it the owner, the group and the mode of the file it replaces, fsync and close
//!    it, and read back what the file is now.
//! 6. Check the baseline again, right before the rename, and that the file's mode and
//!    owner did not change meanwhile.
//! 7. Rename it over the file, then fsync the directory.
//!
//! Up to the rename the file on disk is untouched, and after it the file holds the
//! whole new text, unless the file system cannot sync, which the message then says.
//! Whatever fails or panics on the way, the temporary file is removed. Only an end to the
//! process, or its directory itself being renamed meanwhile, can leave one behind.
//!
//! A save of an unmodified buffer only asks whether its file has gone, and writes if it
//! has, since the text may then exist nowhere else.
//!
//! Errors and messages name the path and never quote the text (ADR 0028).

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::SystemTime;

use oso_core::buffer::{Baseline, BufferId, ContentId, FileStamp};
use oso_core::decode::BOM;
use oso_core::editor::{Editor, SaveJob, SaveRefused};

use crate::loader::{location, parent, stamp_of};

mod leaving;

pub use leaving::{AfterSave, Leaving};

/// How much is written at a time, so that a 1 GB file takes a thousand writes and not
/// a million.
const WRITE_SIZE: usize = 1024 * 1024;

/// Every temporary file's name starts with this. It is hidden, says what it is, and
/// does not hold the file's own name, which could leave it too long for the system.
const TEMPORARY_PREFIX: &str = ".oso-save-";

/// How many names to try before giving up on the directory.
const NAME_ATTEMPTS: u32 = 64;

/// A temporary file's mode while the text goes in: only the user can read it.
const PRIVATE_MODE: u32 = 0o600;

/// The mode a program asks for a new file, which the umask then narrows.
#[cfg(unix)]
const DEFAULT_MODE: u32 = 0o666;

/// Why a save wrote nothing. The file on disk is as it was.
#[derive(Debug)]
pub enum SaveError {
    /// The file on disk is not what the buffer last read or wrote (ADR 0029).
    Conflict,
    /// The path no longer leads to where the buffer's file was: a link or a directory on
    /// it has changed. Nothing is written at the new place.
    Elsewhere,
    /// Where the buffer's file is could not be told when it was read, so no place is safe
    /// to write. Nothing changed on disk.
    UnknownLocation,
    /// The file system refused.
    Io(io::Error),
    /// The saving thread panicked. This holds none of its message, which may quote text.
    Panicked,
}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A save that replaced the file.
#[derive(Debug)]
pub struct Written {
    /// The state of the text now on disk.
    content: ContentId,
    /// The file as it now is.
    stamp: FileStamp,
    /// The file is in place, but the disk did not confirm all of it.
    unsynced: Option<Unsynced>,
}

/// What the disk did not confirm.
#[derive(Debug)]
pub enum Unsynced {
    /// The text: the file system cannot sync a file. A power cut could lose it, with
    /// whatever file it replaced.
    File(io::Error),
    /// The rename: the text is safe, but a power cut could bring back what was there.
    Directory(io::Error),
}

/// How one save ended.
#[derive(Debug)]
pub struct SaveReport {
    buffer: BufferId,
    /// The path the buffer visits, as the messages name it.
    path: PathBuf,
    /// `Ok(None)` if there was nothing to write.
    outcome: Result<Option<Written>, SaveError>,
}

impl SaveReport {
    /// Whether the text this was meant to put on disk is not there.
    pub fn failed(&self) -> bool {
        self.outcome.is_err()
    }

    /// The report of a save whose thread could not start.
    pub fn not_started(buffer: BufferId, path: PathBuf, error: io::Error) -> Self {
        Self {
            buffer,
            path,
            outcome: Err(SaveError::Io(error)),
        }
    }
}

/// Runs `job` on a background thread, and gives its report to `report` there.
///
/// # Errors
///
/// If the thread cannot start. The job is gone, and nothing was written.
pub fn spawn<R>(job: SaveJob, report: R) -> io::Result<()>
where
    R: FnOnce(SaveReport) + Send + 'static,
{
    thread::Builder::new()
        .name("oso-save".into())
        .spawn(move || report(run(job)))
        .map(drop)
}

/// Runs `job` here and now.
pub fn run(job: SaveJob) -> SaveReport {
    run_with(job, write)
}

/// Runs `job` through `write`. A panic must not leave the buffer saving for good.
fn run_with(
    job: SaveJob,
    write: impl FnOnce(&SaveJob) -> Result<Option<Written>, SaveError>,
) -> SaveReport {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&job)))
        .unwrap_or(Err(SaveError::Panicked));
    // The text goes with what is left of `job` when this returns, so the UI thread
    // never frees what only the snapshot still held.
    SaveReport {
        buffer: job.buffer,
        path: job.path,
        outcome,
    }
}

/// Takes a report in: updates the buffer, and says how it went. Never quotes text.
pub fn apply(editor: &mut Editor, report: SaveReport) {
    let SaveReport {
        buffer,
        path,
        outcome,
    } = report;
    let path = path.display();
    let message = match outcome {
        Ok(None) => {
            editor.save_unneeded(buffer);
            SaveRefused::Unmodified.to_string()
        }
        Ok(Some(written)) => {
            editor.save_finished(buffer, written.content, written.stamp);
            match written.unsynced {
                None => format!("Wrote {path}"),
                Some(Unsynced::File(e)) => format!(
                    "Wrote {path}, but the file system cannot confirm the text reached the disk. A power cut could lose it, and any file it replaced: {e}"
                ),
                Some(Unsynced::Directory(e)) => format!(
                    "Wrote {path}, but the disk did not confirm the rename. A power cut could bring back what was there: {e}"
                ),
            }
        }
        Err(error) => {
            editor.save_failed(buffer);
            match error {
                SaveError::Conflict => format!(
                    "{path} changed on disk since it was read. It was kept, and the buffer is not saved"
                ),
                SaveError::Elsewhere => format!(
                    "{path} now leads somewhere other than when it was read. Nothing was written, and the buffer is not saved"
                ),
                SaveError::UnknownLocation => format!(
                    "Cannot save {path}: where it leads could not be told when it was read, so no place is safe. Nothing was written"
                ),
                SaveError::Io(e) => format!("Cannot save {path}: {e}"),
                SaveError::Panicked => format!("Saving {path} failed. The buffer is not saved"),
            }
        }
    };
    editor.message(message);
}

fn write(job: &SaveJob) -> Result<Option<Written>, SaveError> {
    if job.only_if_gone && !file_is_gone(job)? {
        return Ok(None);
    }
    prepare(job)?.commit(job).map(Some)
}

/// Whether the file of an unmodified buffer has gone from where it was. If anything is
/// there, the buffer has nothing to say about it.
fn file_is_gone(job: &SaveJob) -> Result<bool, SaveError> {
    let baseline = baseline_of(job)?;
    let gone = match fs::symlink_metadata(&baseline.location) {
        Ok(_) => false,
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) => return Err(e.into()),
    };
    // The only copy, with a path that no longer leads to where it belongs: say so.
    if gone && location(&job.path)? != baseline.location {
        return Err(SaveError::Elsewhere);
    }
    Ok(gone)
}

fn baseline_of(job: &SaveJob) -> Result<&Baseline, SaveError> {
    job.baseline.as_ref().ok_or(SaveError::UnknownLocation)
}

/// The temporary file holding the whole text, closed, and not yet in place.
struct Prepared {
    temporary: Temporary,
    /// The file it replaces, as it was before the write.
    replaced: Option<Metadata>,
    stamp: FileStamp,
    /// Why the text may not be on the disk yet.
    unsynced: Option<io::Error>,
}

/// Steps 1 to 5 of the order at the top. The file on disk is not touched.
fn prepare(job: &SaveJob) -> Result<Prepared, SaveError> {
    prepare_with(job, File::sync_all)
}

/// [`prepare`], with `sync` to bring the file to the disk.
fn prepare_with(
    job: &SaveJob,
    sync: impl FnOnce(&File) -> io::Result<()>,
) -> Result<Prepared, SaveError> {
    let baseline = baseline_of(job)?;
    let replaced = unchanged_file(&job.path, baseline)?;
    if replaced.is_some() {
        // A rename needs only the directory, so it would replace a file the user has
        // made read-only, or may not write. Opening for writing changes nothing.
        OpenOptions::new().write(true).open(&baseline.location)?;
    }
    // The real directory, so that a link changed during the write does not lose track of
    // the temporary file.
    let directory = parent(&baseline.location);
    let (temporary, file) = Temporary::create(directory, PRIVATE_MODE)?;
    write_text(&file, job)?;
    keep_attributes(&file, replaced.as_ref(), baseline.file, directory)?;
    let unsynced = unless_unsupported(sync(&file))?;
    // Read after closing it, as Windows may settle the modification time only then. The
    // name is ours until the rename, and that comes after, so a change made right after the
    // rename still counts as a change.
    drop(file);
    let stamp = stamp_of(&fs::symlink_metadata(&temporary.path)?)?;
    // A file system with no sync, and `std` does not report a failed close, may only now
    // have run out of room. Past this, the rename would replace a good file with less.
    check_whole(stamp.len, job)?;
    Ok(Prepared {
        temporary,
        replaced,
        stamp,
        unsynced,
    })
}

/// Whether a file of `len` bytes holds all of `job`.
fn check_whole(len: u64, job: &SaveJob) -> io::Result<()> {
    let bom = if job.bom { BOM.len() } else { 0 };
    if len == (bom + job.text.len()) as u64 {
        Ok(())
    } else {
        Err(io::Error::other(
            "the file system kept less than was written",
        ))
    }
}

impl Prepared {
    /// Steps 6 and 7. The path is walked again, as it must still lead to the file's place,
    /// but the rename goes by the place itself.
    fn commit(self, job: &SaveJob) -> Result<Written, SaveError> {
        let baseline = baseline_of(job)?;
        let now = unchanged_file(&job.path, baseline)?;
        if attributes_moved(self.replaced.as_ref(), now.as_ref()) {
            return Err(SaveError::Conflict);
        }
        self.temporary.rename_over(&baseline.location)?;
        let directory = sync_directory(parent(&baseline.location)).err();
        Ok(Written {
            content: job.content,
            stamp: self.stamp,
            unsynced: self
                .unsynced
                .map(Unsynced::File)
                .or(directory.map(Unsynced::Directory)),
        })
    }
}

/// The result of syncing a file, as what to say about it: nothing, or that a file system
/// cannot do it. Any other error fails the save, as the disk has refused the text.
///
/// `std` makes `sync_all` a bare `F_FULLFSYNC` on macOS, with no fallback, and some network
/// and FUSE mounts refuse that. So nothing could ever be saved there.
fn unless_unsupported(synced: io::Result<()>) -> io::Result<Option<io::Error>> {
    match synced {
        Ok(()) => Ok(None),
        Err(e) if sync_is_unsupported(&e) => Ok(Some(e)),
        Err(e) => Err(e),
    }
}

/// Whether `error` says the file system has no such thing as syncing this file.
fn sync_is_unsupported(error: &io::Error) -> bool {
    // On Apple systems `ENOTSUP` differs from the `EOPNOTSUPP` that `std` knows.
    #[cfg(target_vendor = "apple")]
    if error.raw_os_error() == Some(45) {
        return true;
    }
    matches!(
        error.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
    )
}

/// Whether the attributes the new file took are out of date by now, as the rename would
/// then undo a `chmod` made during the write. A file that came back counts, since it
/// was not there to take them from. One deleted meanwhile has nothing left to keep.
fn attributes_moved(before: Option<&Metadata>, now: Option<&Metadata>) -> bool {
    match (before, now) {
        (Some(before), Some(now)) => attributes(before) != attributes(now),
        (None, Some(_)) => true,
        (_, None) => false,
    }
}

#[cfg(unix)]
fn attributes(metadata: &Metadata) -> (u32, u32, u32) {
    use std::os::unix::fs::MetadataExt;
    (metadata.mode(), metadata.uid(), metadata.gid())
}

#[cfg(not(unix))]
fn attributes(metadata: &Metadata) -> bool {
    metadata.permissions().readonly()
}

/// The file at the buffer's location, if there is one, unless it is not what `baseline`
/// says.
///
/// `path` must still lead there. Otherwise a link or a directory changed since would send
/// the text to a place nobody chose. Being missing is no conflict, even if the baseline says
/// a file was there: a file that was deleted holds nothing to keep, as in Emacs.
///
/// The check goes by the location, which is what a rename replaces, and does not follow a
/// link: the location has none left unless one was swapped in since, and that is a change.
fn unchanged_file(path: &Path, baseline: &Baseline) -> Result<Option<Metadata>, SaveError> {
    if location(path)? != baseline.location {
        return Err(SaveError::Elsewhere);
    }
    match fs::symlink_metadata(&baseline.location) {
        Ok(metadata) if metadata.is_file() => match baseline.file {
            Some(file) if file.same_content(stamp_of(&metadata)?) => Ok(Some(metadata)),
            _ => Err(SaveError::Conflict),
        },
        Ok(_) => Err(SaveError::Conflict),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_text(file: impl Write, job: &SaveJob) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(WRITE_SIZE, file);
    if job.bom {
        out.write_all(BOM)?;
    }
    for chunk in job.text.chunks() {
        out.write_all(chunk.as_bytes())?;
    }
    out.flush()
}

/// Gives `file` the owner, group and mode of the file it replaces. If that file was
/// deleted since it was `read`, it gets its mode back, as if its group were lost, and a
/// file that never existed gets the default mode. Extended attributes, ACLs and other
/// hard links do not carry over (ADR 0059).
#[cfg(unix)]
fn keep_attributes(
    file: &File,
    replaced: Option<&Metadata>,
    read: Option<FileStamp>,
    directory: &Path,
) -> io::Result<()> {
    use std::fs::Permissions;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, fchown};

    let Some(old) = replaced else {
        if let Some(read) = read {
            return file.set_permissions(Permissions::from_mode(kept_mode(read.mode, false)));
        }
        let (_probe, probe_file) = Temporary::create(directory, DEFAULT_MODE)?;
        return file.set_permissions(probe_file.metadata()?.permissions());
    };
    let new = file.metadata()?;
    if old.uid() != new.uid() {
        fchown(file, Some(old.uid()), None).map_err(|_| {
            io::Error::other("the file belongs to another user, and a save would take it over")
        })?;
    }
    let group_kept = old.gid() == new.gid() || fchown(file, None, Some(old.gid())).is_ok();
    file.set_permissions(Permissions::from_mode(kept_mode(old.mode(), group_kept)))
}

/// The mode the new file gets from the `mode` of the old one. Set-user-ID and set-group-ID
/// go, as they do when a file is written. If the file ends up in another group, that
/// group gets no more than everyone else, since the bits were meant for the old one.
#[cfg(unix)]
fn kept_mode(mode: u32, group_kept: bool) -> u32 {
    let mode = mode & 0o777;
    if group_kept {
        mode
    } else {
        mode & !(0o070 & !((mode & 0o007) << 3))
    }
}

#[cfg(not(unix))]
fn keep_attributes(
    file: &File,
    replaced: Option<&Metadata>,
    _read: Option<FileStamp>,
    _directory: &Path,
) -> io::Result<()> {
    match replaced {
        Some(old) => file.set_permissions(old.permissions()),
        None => Ok(()),
    }
}

/// A rename is durable once its directory is synced. Windows has no such thing.
fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// Creates the file at `path`, which must not exist in any form. A link left there,
/// even to nothing, makes this fail and is not followed. `mode` means nothing on Windows.
fn open_new(path: &Path, mode: u32) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    #[cfg(not(unix))]
    let _ = mode;
    options.open(path)
}

/// A temporary file's name, removed when this is dropped unless it was renamed, also
/// on the way out of a panic.
struct Temporary {
    path: PathBuf,
    renamed: bool,
}

impl Temporary {
    /// Creates a file with a new name beside nothing it could be mistaken for.
    fn create(directory: &Path, mode: u32) -> io::Result<(Self, File)> {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let process = std::process::id();
        for attempt in 0..NAME_ATTEMPTS {
            let name = format!("{TEMPORARY_PREFIX}{process}-{stamp}-{attempt}.tmp");
            let path = directory.join(name);
            match open_new(&path, mode) {
                Ok(file) => {
                    let renamed = false;
                    return Ok((Self { path, renamed }, file));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("could not find a free temporary name"))
    }

    fn rename_over(mut self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)?;
        self.renamed = true;
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.renamed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
