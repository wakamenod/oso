//! Safe save: writing a buffer's snapshot over its file on a background thread
//! (ADR 0013, ADR 0029, ADR 0059, ADR 0067). Losing or damaging a file is what this is
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
//! 3. Create a temporary file with mode 0600 in that file's directory, so that the rename
//!    stays on one file system, taking no access list from there, so that only this user can
//!    read it while it fills, from its first moment.
//! 4. Write the BOM and the text. Line endings are already in the text.
//! 5. Give it the owner and the group of the file it replaces, then its quarantine flag and
//!    access list, or what a file made there gets if there was no such file, and then the
//!    mode. Nothing is denied too late, and nobody else can read it before now, then fsync and
//!    close it, and read back what the file is now.
//! 6. Check the baseline again, right before the rename, and that the file's mode, owner and
//!    access list did not change meanwhile.
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
//!
//! `emergency` shares the temporary file and the syncing with this, from the layer above
//! (ADR 0062).

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::SystemTime;

use oso_core::buffer::{Baseline, BufferId, ContentId, FileStamp};
use oso_core::decode::BOM;
use oso_core::editor::SaveJob;
use oso_platform::file_meta::{self, AccessList, Quarantine};

use crate::loader::{location, parent, stamp_of};

mod attributes;
mod elsewhere;
mod leaving;
mod report;

#[cfg(all(test, unix))]
use attributes::kept_mode;
use attributes::{Made, attributes_moved, keep_mode, keep_owner};
pub use leaving::{AfterSave, Agreement, AgreementCell, Leaving};
pub use report::apply;

/// How much is written at a time, so that a 1 GB file takes a thousand writes and not
/// a million.
pub(crate) const WRITE_SIZE: usize = 1024 * 1024;

/// Every temporary file's name starts with this. It is hidden, says what it is, and
/// does not hold the file's own name, which could leave it too long for the system.
const TEMPORARY_PREFIX: &str = ".oso-save-";

/// How many names to try before giving up on the directory.
const NAME_ATTEMPTS: u32 = 64;

/// A temporary file's mode while the text goes in: only the user can read it.
pub(crate) const PRIVATE_MODE: u32 = 0o600;

/// Why a save wrote nothing. The file on disk is as it was.
#[derive(Debug)]
pub enum SaveError {
    /// The file on disk is not what the buffer last read or wrote (ADR 0029). With the regular
    /// file found, which an agreed save may go over (ADR 0072), if its stamp could be read.
    Conflict(Option<FileStamp>),
    /// The path no longer leads to where the buffer's file was: a link or a directory on
    /// it has changed. Nothing is written at the new place.
    Elsewhere,
    /// Where the buffer's file is could not be told when it was read, so no place is safe
    /// to write. Nothing changed on disk.
    UnknownLocation,
    /// An access list could not be read, put on the new file or confirmed there, and what it allows
    /// or denies must not be lost.
    AccessList(io::Error),
    /// The system refused the rename, and the access list the file would have denies someone
    /// deleting it, which a rename needs, though whom it names cannot be told.
    ListForbidsReplacing(io::Error),
    /// The file system refused.
    Io(io::Error),
    /// The saving thread panicked. This holds none of its message, which may quote text.
    Panicked,
    /// A write to a new place found a regular file there, at `location`, which the person has
    /// not agreed to write over (ADR 0087).
    Exists { found: FileStamp, location: PathBuf },
    /// A write to a new place found another buffer's file there.
    OpenElsewhere,
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
    /// The file is in place, but its quarantine flag could not be kept.
    quarantine_lost: Option<io::Error>,
    /// The text went to a new place, which the buffer now visits (`C-x C-w`).
    moved: Option<Box<elsewhere::Moved>>,
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
    /// The text was to go to a new place, not where the buffer's file was (`C-x C-w`).
    elsewhere: bool,
}

impl SaveReport {
    /// Whether the text this was meant to put on disk is not there.
    pub fn failed(&self) -> bool {
        self.outcome.is_err()
    }

    /// The buffer this save was of.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// Whether the text reached the disk and the disk confirmed all of it, so that an
    /// emergency copy of it has done its work (ADR 0064).
    pub fn reached_the_disk(&self) -> bool {
        matches!(&self.outcome, Ok(Some(written)) if written.unsynced.is_none())
    }

    /// The report of a save whose thread could not start.
    pub fn not_started(buffer: BufferId, path: PathBuf, error: io::Error) -> Self {
        Self {
            buffer,
            path,
            outcome: Err(SaveError::Io(error)),
            elsewhere: false,
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
        elsewhere: job.elsewhere.is_some(),
        path: job.path,
        outcome,
    }
}

fn write(job: &SaveJob) -> Result<Option<Written>, SaveError> {
    if let Some(elsewhere) = &job.elsewhere {
        return elsewhere::write(job, elsewhere);
    }
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
    /// The access list it holds: that file's, or if there is none what it took from its directory.
    list: AccessList,
    quarantine_lost: Option<io::Error>,
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
    let mut kept_list = None;
    let mut quarantine = Ok(None);
    if replaced.is_some() {
        // A rename needs only the directory, so it would replace a file the user has
        // made read-only, or may not write. Opening for writing changes nothing.
        file_meta::open_for_writing(&baseline.location)?;
        kept_list = Some(AccessList::at(&baseline.location).map_err(SaveError::AccessList)?);
        quarantine = Quarantine::at(&baseline.location);
    }
    // The real directory, so that a link changed during the write does not lose track of
    // the temporary file.
    let directory = parent(&baseline.location);
    // What a new file gets is what a file made there gets, and nothing goes in that one.
    let made = match replaced {
        Some(_) => None,
        None => Some(Made::there(directory)?),
    };
    let (temporary, file) = Temporary::create_private(directory)?;
    // A Linux file system still gives it a list, whose mask its mode shuts, so it grants
    // nothing. It comes off, so that what goes on is only what is wanted.
    AccessList::take_from(&file).map_err(SaveError::AccessList)?;
    write_text(&file, job)?;
    let group_kept = keep_owner(&file, replaced.as_ref())?;
    let quarantine_lost = quarantine
        .and_then(|flag| flag.map_or(Ok(()), |flag| flag.give_to(&file)))
        .err();
    let (list, made_mode) = match made {
        Some(made) => (made.list, Some(made.permissions)),
        None => (kept_list.unwrap_or_default(), None),
    };
    list.give_to(&file).map_err(SaveError::AccessList)?;
    keep_mode(
        &file,
        replaced.as_ref(),
        baseline.file,
        made_mode,
        group_kept,
    )?;
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
        list,
        quarantine_lost,
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
            return Err(SaveError::Conflict(None));
        }
        // The rename would undo a list changed during the write, as it would a `chmod`.
        if self.replaced.is_some() && now.is_some() {
            let list = AccessList::at(&baseline.location).map_err(SaveError::AccessList)?;
            if list != self.list {
                return Err(SaveError::Conflict(None));
            }
        }
        let forbidden = self.list.denies_deleting();
        // A list that denies deleting the file would keep its temporary name from going, so it
        // gets the rename, which the system then refuses, as for a file it replaces.
        let put = if self.replaced.is_none() && !forbidden {
            self.temporary.put_where_nothing_is(&baseline.location)
        } else {
            self.temporary.rename_over(&baseline.location)
        };
        put.map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                let found = fs::symlink_metadata(&baseline.location)
                    .ok()
                    .filter(Metadata::is_file)
                    .and_then(|metadata| stamp_of(&metadata).ok());
                return SaveError::Conflict(found);
            }
            if forbidden && e.kind() == io::ErrorKind::PermissionDenied {
                SaveError::ListForbidsReplacing(e)
            } else {
                e.into()
            }
        })?;
        let directory = sync_directory(parent(&baseline.location)).err();
        Ok(Written {
            content: job.content,
            stamp: self.stamp,
            unsynced: self
                .unsynced
                .map(Unsynced::File)
                .or(directory.map(Unsynced::Directory)),
            quarantine_lost: self.quarantine_lost,
            moved: None,
        })
    }
}

/// The result of syncing a file, as what to say about it: nothing, or that a file system
/// cannot do it. Any other error fails the save, as the disk has refused the text.
///
/// `std` makes `sync_all` a bare `F_FULLFSYNC` on macOS, with no fallback, and some network
/// and FUSE mounts refuse that. So nothing could ever be saved there.
pub(crate) fn unless_unsupported(synced: io::Result<()>) -> io::Result<Option<io::Error>> {
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

/// The file at the buffer's location, if there is one, unless it is not what `baseline`
/// says.
///
/// `path` must still lead there. Otherwise a link or a directory changed since would send
/// the text to a place nobody chose. Being missing is no conflict, even if the baseline says
/// a file was there: a file that was deleted holds nothing to keep, as in Emacs.
///
/// The check goes by the location, which a rename replaces, without following a link: one
/// swapped in since is a change, and so is another file renamed in (ADR 0083).
fn unchanged_file(path: &Path, baseline: &Baseline) -> Result<Option<Metadata>, SaveError> {
    if location(path)? != baseline.location {
        return Err(SaveError::Elsewhere);
    }
    match fs::symlink_metadata(&baseline.location) {
        Ok(metadata) if metadata.is_file() => match baseline.file {
            Some(file) if file.same_file(stamp_of(&metadata)?) => Ok(Some(metadata)),
            _ => Err(SaveError::Conflict(stamp_of(&metadata).ok())),
        },
        Ok(_) => Err(SaveError::Conflict(None)),
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

/// A rename is durable once its directory is synced. Windows has no such thing.
pub(crate) fn sync_directory(directory: &Path) -> io::Result<()> {
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

/// Whether `text` is one or more ASCII digits.
pub(crate) fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `name` has the exact form [`Temporary::create`] gives a file.
pub(crate) fn is_temporary_name(name: &str) -> bool {
    let middle = name
        .strip_prefix(TEMPORARY_PREFIX)
        .and_then(|rest| rest.strip_suffix(".tmp"));
    let parts = middle.map(|middle| middle.split('-').collect::<Vec<_>>());
    parts.is_some_and(|parts| parts.len() == 3 && parts.iter().all(|part| all_digits(part)))
}

/// A temporary file's name, removed when this is dropped unless it was renamed, also
/// on the way out of a panic.
pub(crate) struct Temporary {
    path: PathBuf,
    renamed: bool,
}

impl Temporary {
    /// Creates a file with a new name beside nothing it could be mistaken for.
    pub(crate) fn create(directory: &Path, mode: u32) -> io::Result<(Self, File)> {
        Self::create_with(directory, |path| open_new(path, mode))
    }

    /// Creates a file with a new name, as [`Self::create`] does, with mode 0600 and granting
    /// nothing through an access list at any moment.
    fn create_private(directory: &Path) -> io::Result<(Self, File)> {
        Self::create_with(directory, file_meta::create_private)
    }

    /// Creates a file through `open`, which must make a new one, and fail with `AlreadyExists` for
    /// a name already there.
    fn create_with(
        directory: &Path,
        open: impl Fn(&Path) -> io::Result<File>,
    ) -> io::Result<(Self, File)> {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let process = std::process::id();
        for attempt in 0..NAME_ATTEMPTS {
            let name = format!("{TEMPORARY_PREFIX}{process}-{stamp}-{attempt}.tmp");
            let path = directory.join(name);
            match open(&path) {
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

    /// The temporary name.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn rename_over(mut self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)?;
        self.renamed = true;
        Ok(())
    }

    /// Puts the file at `target`, where nothing was when it was looked at, without replacing
    /// whatever has come there since: as a second name, which fails if the name is taken, and
    /// then without its own. A file system with no second names gets a rename, which has the
    /// moment between the look and the rename that a save has (ADR 0059).
    ///
    /// # Errors
    ///
    /// `AlreadyExists` if something has come to `target`, or why the file could not go there.
    pub(crate) fn put_where_nothing_is(mut self, target: &Path) -> io::Result<()> {
        match fs::hard_link(&self.path, target) {
            Ok(()) => {
                // The file is in place. Its own name goes here, and never as a drop does it,
                // which would take the list off the file now under `target` too.
                self.renamed = true;
                if let Err(e) = fs::remove_file(&self.path) {
                    log::warn!("a saved file kept its temporary name too: {e}");
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
            Err(_) => self.rename_over(target),
        }
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        // A list it was given can deny removing it, and only until that list is gone, which comes
        // after its mode is shut, so that it is never left more open.
        if !self.renamed && fs::remove_file(&self.path).is_err() {
            let _ = file_meta::clear_access_list_at(&self.path);
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
