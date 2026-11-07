//! Finding emergency copies, off the UI thread (ADR 0062, ADR 0064): what is in the directory,
//! what each copy says, which buffer it belongs to, and what is left of interrupted writes. A
//! copy that proves to hold what its file holds is removed on the way.
//!
//! Must not depend on: `app`, `dispatch` or winit; a caller hands over what it knows of the
//! buffers and takes a [`Scan`].
//!
//! **A path in a `.meta` is shown and compared, and no file operation ever uses it.** Only
//! the directory itself is read, and a file a person opened with the directory it is in. So a
//! hostile `.meta` can say what it likes, and at worst shows a misleading row. A name that
//! does not have the exact form of a copy or a temporary file is left alone: not read, not
//! listed, not deleted.
//!
//! Nothing here reads a copy's text but to compare it with its file, and no text is in a row,
//! a message or a log.

use std::collections::BTreeMap;
use std::fs::{self, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use oso_core::buffer::{Buffer, BufferId, FileStamp, Verdict};
use oso_core::decode::LineEnding;
use oso_core::editor::Editor;
use oso_core::load::LoadSnapshot;
use oso_platform::privacy::PrivateDir;

use super::compare::same_bytes;
use super::meta::{MAX_META, Meta, parse};
use super::write::{encoded, stem_seconds};
use crate::saver::is_temporary_name;

mod files;
mod list;

pub use files::open_copy;
use files::{left_behind, plain_own_file, seconds_of};
pub(super) use files::{open_checked, remove_copy};
pub(super) use list::age;
use list::list_of;

/// The most names read from a directory, here or beside a file.
const MAX_NAMES: usize = 100_000;
/// The most copies that get their `.meta` read and a row.
const MAX_ROWS: usize = 1000;
/// The longest path a row shows.
const MAX_SHOWN: usize = 1024;
/// How old a file with no `.meta` must be to count as left behind. A writer may be at work on
/// a younger one: the hook waits at most 5 minutes in a sync, and a save goes on for as long
/// as it gets somewhere.
pub(super) const LEFT_BEHIND: Duration = Duration::from_secs(60 * 60);
/// From this size a copy is not read. ADR 0051 wants to ask before opening so much, and
/// running out of memory aborts with no emergency save.
pub(super) const MAX_LOOK: u64 = 2 * 1024 * 1024 * 1024;
/// The rows above the first copy in the list.
pub(super) const HEADER_ROWS: usize = 2;

/// What a row says of a copy and its file, which also decides what can be done with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    /// The file's stamp is the one the metadata records.
    Unchanged,
    /// The stamp differs, or a file appeared where the buffer had none.
    Changed,
    /// The metadata records a file, and none is there.
    Gone,
    /// The buffer had no file.
    NoFile,
    /// The metadata names a file no buffer visits.
    NotOpen,
    /// A buffer's path matches and its location does not.
    LeadsElsewhere,
    /// Its text is in a buffer, and has not been saved.
    Recovered,
    /// Its `.txt` is not the size `bytes` says, is missing, or is too large to read.
    Damaged,
    /// No usable `.meta`.
    Unknown,
    /// A leftover temporary file.
    LeftBySave,
}

impl RowState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "file unchanged",
            Self::Changed => "file changed",
            Self::Gone => "file gone",
            Self::NoFile => "no file",
            Self::NotOpen => "not open",
            Self::LeadsElsewhere => "leads elsewhere",
            Self::Recovered => "recovered",
            Self::Damaged => "damaged",
            Self::Unknown => "unknown",
            Self::LeftBySave => "left by a save",
        }
    }
}

/// What a scan is told of one buffer the person has open.
#[derive(Clone, Debug)]
pub struct Target {
    pub buffer: BufferId,
    name: String,
    encoded_path: Option<String>,
    /// Where a copy of it now would say it belongs, in the file's own spelling.
    encoded_location: Option<String>,
    /// Where the path led as typed, for a copy written with no spelling to go by.
    encoded_typed: Option<String>,
    location: Option<PathBuf>,
    stamp: Option<FileStamp>,
    /// Empty and unmodified, so that a text can go in with nothing swapped out.
    blank: bool,
    verdict: Verdict,
}

impl Target {
    fn of(buffer: BufferId, shown: &Buffer) -> Option<Self> {
        if shown.loading().is_some() {
            return None;
        }
        let bytes = |path: &Path| encoded(path.as_os_str().as_encoded_bytes());
        // A file whose place could not be told cannot be saved, so nothing can be recovered.
        let baseline = match (shown.path(), shown.baseline()) {
            (Some(_), None) => return None,
            (_, baseline) => baseline,
        };
        Some(Self {
            buffer,
            name: shown.name().to_owned(),
            encoded_path: shown.path().map(bytes),
            encoded_location: shown.place_for_copy().map(bytes),
            encoded_typed: baseline.map(|baseline| bytes(&baseline.location)),
            location: baseline.map(|baseline| baseline.location.clone()),
            stamp: baseline.and_then(|baseline| baseline.file),
            blank: shown.text().is_empty() && !shown.is_modified(),
            verdict: shown.verdict(),
        })
    }

    fn has_file(&self) -> bool {
        self.encoded_path.is_some()
    }
}

/// What the buffers open now say, for a scan.
pub fn targets_of(editor: &Editor) -> Vec<Target> {
    editor
        .buffers()
        .filter_map(|(id, buffer)| Target::of(id, buffer))
        .collect()
}

/// One copy or temporary file, as a row of the list says it. Holds no text.
#[derive(Clone, Debug)]
pub struct Row {
    /// The file holding the text. Also what names this row.
    pub(super) text: PathBuf,
    pub(super) meta: Option<PathBuf>,
    pub(super) state: RowState,
    pub(super) target: Option<BufferId>,
    /// The buffer's name, or the file's own.
    pub(super) name: String,
    /// What the row shows of where it belongs, not yet escaped.
    pub(super) shown: String,
    /// The paths its metadata names, decoded, for the secret patterns.
    pub(super) paths: Vec<PathBuf>,
    /// Withheld at least this far, whatever `paths` say.
    pub(super) at_least: Verdict,
    pub(super) size: u64,
    pub(super) written: Option<u64>,
    why: Option<&'static str>,
    /// The file's place, if a buffer visits it.
    pub(super) location: Option<PathBuf>,
    pub(super) bom: bool,
    pub(super) line_ending: Option<LineEnding>,
}

/// What a scan is asked to do.
#[derive(Clone, Debug)]
pub struct ScanRequest {
    /// Later scans win over earlier ones that end after them.
    pub(super) id: u64,
    pub(super) directory: Option<PathBuf>,
    pub(super) targets: Vec<Target>,
    /// The copies whose text is in a buffer now, and not yet saved.
    pub(super) recovered: Vec<PathBuf>,
}

/// What a scan found. A `Debug` of it shows no list.
pub struct Scan {
    pub(super) id: u64,
    pub(super) rows: Vec<Row>,
    /// Why the directory was not read.
    pub(super) problem: Option<String>,
    /// Copies removed for holding what their file holds, by name.
    pub(super) removed: Vec<String>,
    /// The list, built here as it can be large.
    pub(super) list: LoadSnapshot,
}

impl std::fmt::Debug for Scan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scan")
            .field("id", &self.id)
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

/// Runs `request` on a thread of its own, unless none can start: a scan helps, and nothing
/// waits for it.
pub(super) fn spawn_scan(request: ScanRequest, report: impl FnOnce(Scan) + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("oso-copies".into())
        .spawn(move || {
            report(caught_scan(&request));
        });
    if let Err(e) = spawned {
        log::warn!("cannot look for emergency copies: {e}");
    }
}

fn failed(id: u64) -> Scan {
    let problem = "looking for emergency copies failed".to_owned();
    Scan {
        id,
        rows: Vec::new(),
        list: list_of(&[], 0, Some(&problem), SystemTime::now()),
        problem: Some(problem),
        removed: Vec::new(),
    }
}

/// What a scan gathers as it goes.
#[derive(Default)]
struct Gathered {
    rows: Vec<Row>,
    /// Temporary files, which take the room the copies leave.
    left: Vec<Row>,
    more: usize,
    problem: Option<String>,
    removed: Vec<String>,
}

/// [`scan`], which a panic costs only this list. Its message may not reach anything.
pub(super) fn caught_scan(request: &ScanRequest) -> Scan {
    let now = SystemTime::now();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(request, now)))
        .unwrap_or_else(|_| failed(request.id))
}

/// Looks through the directory, and beside each file open, as of `now`.
pub(super) fn scan(request: &ScanRequest, now: SystemTime) -> Scan {
    let mut found = Gathered::default();
    if let Some(directory) = &request.directory {
        // Refused when another user could have put anything there.
        match PrivateDir::existing(directory) {
            Ok(None) => {}
            Ok(Some(_)) => scan_directory(directory, request, now, &mut found),
            Err(e) => found.problem = Some(e.to_string()),
        }
    }
    found
        .left
        .extend(leftovers_beside_files(&request.targets, now));
    // Copies come first for the limit, as a directory can hold any number of leftovers. The
    // newest first. A time from a `.meta` is untrusted, and only orders rows.
    let newest_first = |a: &Row, b: &Row| b.written.cmp(&a.written).then(a.text.cmp(&b.text));
    found.left.sort_by(newest_first);
    let room = MAX_ROWS.saturating_sub(found.rows.len());
    found.more += found.left.len().saturating_sub(room);
    found.left.truncate(room);
    found.rows.append(&mut found.left);
    found.rows.sort_by(newest_first);
    let list = list_of(&found.rows, found.more, found.problem.as_deref(), now);
    Scan {
        id: request.id,
        rows: found.rows,
        problem: found.problem,
        removed: found.removed,
        list,
    }
}

/// What the `.meta` at `path` says, if it is a plain file saying something this reads.
fn read_meta(path: &Path) -> Option<Meta> {
    let (file, _) = open_checked(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_META + 1).read_to_end(&mut bytes).ok()?;
    parse(&bytes)
}

/// Reads the names in `directory`, and adds a row for each copy and temporary file. A copy
/// found to hold what its file holds is removed here, as it has nothing to offer.
fn scan_directory(directory: &Path, request: &ScanRequest, now: SystemTime, found: &mut Gathered) {
    // Each stem, and whether a `.meta` of that name is there.
    let mut stems: BTreeMap<String, bool> = BTreeMap::new();
    let mut temporaries = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut names = 0;
    for entry in entries.take(MAX_NAMES).flatten() {
        names += 1;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_temporary_name(name) {
            temporaries.push(entry.path());
        } else if let Some(stem) = name
            .strip_suffix(".txt")
            .filter(|s| stem_seconds(s).is_some())
        {
            stems.entry(stem.to_owned()).or_default();
        } else if let Some(stem) = name
            .strip_suffix(".meta")
            .filter(|s| stem_seconds(s).is_some())
        {
            stems.insert(stem.to_owned(), true);
        }
    }
    let mut newest: Vec<(String, bool)> = stems.into_iter().collect();
    newest.sort_by_cached_key(|(stem, _)| std::cmp::Reverse(stem_seconds(stem)));
    if names == MAX_NAMES {
        found.problem = Some(format!(
            "{} holds more names than are read, so some copies may not be listed",
            directory.display()
        ));
    }
    found.more += newest.len().saturating_sub(MAX_ROWS);
    for (stem, has_meta) in newest.into_iter().take(MAX_ROWS) {
        match copy_row(directory, &stem, has_meta, request, now) {
            Copy::Row(row) => found.rows.push(*row),
            Copy::Equal => found.removed.push(stem),
            Copy::Nothing => {}
        }
    }
    found.left.extend(
        temporaries
            .into_iter()
            .filter_map(|path| left_by_a_save(&path, now)),
    );
}

/// What came of looking at one stem.
enum Copy {
    Row(Box<Row>),
    /// It held what its file holds, and has gone.
    Equal,
    /// There is nothing to say yet.
    Nothing,
}

/// A row with what every row needs, for a file at `text` with only its own name to go on.
fn bare_row(
    text: &Path,
    meta: Option<&Path>,
    state: RowState,
    size: u64,
    written: Option<u64>,
) -> Row {
    let name = text
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    Row {
        text: text.to_owned(),
        meta: meta.map(Path::to_owned),
        state,
        target: None,
        shown: text.display().to_string(),
        name,
        paths: Vec::new(),
        at_least: Verdict::Unresolved,
        size,
        written,
        why: None,
        location: None,
        bom: false,
        line_ending: None,
    }
}

/// A row for what has no usable `.meta`: nothing says where it belongs. A text with none at
/// all may be being written, so it waits until it has been left long enough.
fn unknown_row(text: &Path, meta: Option<&Path>, now: SystemTime) -> Copy {
    let metadata = fs::symlink_metadata(text).ok();
    if meta.is_none()
        && !metadata
            .as_ref()
            .is_some_and(|metadata| left_behind(metadata, now))
    {
        return Copy::Nothing;
    }
    let written = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(seconds_of);
    let size = metadata.map_or(0, |metadata| metadata.len());
    Copy::Row(Box::new(bare_row(
        text,
        meta,
        RowState::Unknown,
        size,
        written,
    )))
}

fn copy_row(
    directory: &Path,
    stem: &str,
    has_meta: bool,
    request: &ScanRequest,
    now: SystemTime,
) -> Copy {
    let text = directory.join(format!("{stem}.txt"));
    let meta_path = directory.join(format!("{stem}.meta"));
    let meta_kept = has_meta.then_some(meta_path.as_path());
    let Some(meta) = has_meta.then(|| read_meta(&meta_path)).flatten() else {
        return unknown_row(&text, meta_kept, now);
    };
    let looked = plain_own_file(&text);
    let size = looked.as_ref().map_or(0, Metadata::len);
    let mut row = bare_row(&text, meta_kept, RowState::NotOpen, size, meta.written);
    row.name.clone_from(&meta.name);
    row.shown = meta.shown();
    row.paths = meta.decoded_paths();
    row.at_least = if meta.withheld {
        Verdict::Unresolved
    } else {
        Verdict::Ordinary
    };
    row.why = meta.why;
    row.bom = meta.bom;
    row.line_ending = meta.line_ending;
    row.state = if meta.path.is_none() && meta.location.is_none() {
        RowState::NoFile
    } else {
        RowState::NotOpen
    };

    let matched = matching(&meta, &request.targets);
    match matched {
        Matched::Target(target) => {
            row.target = Some(target.buffer);
            if target.verdict != Verdict::Ordinary {
                row.at_least = target.verdict;
            }
            row.location.clone_from(&target.location);
            if target.has_file() {
                row.state = relation(&meta, target.stamp);
            }
        }
        Matched::Elsewhere => row.state = RowState::LeadsElsewhere,
        Matched::Nothing => {}
    }
    if looked.is_none() || size != meta.bytes || size >= MAX_LOOK {
        row.state = RowState::Damaged;
    } else if request.recovered.contains(&text) {
        row.state = RowState::Recovered;
    } else if let Matched::Target(target) = matched
        && target.has_file()
        && holds_its_file(&row, target)
    {
        return if remove_copy(&row.text, row.meta.as_deref()).is_ok() {
            Copy::Equal
        } else {
            Copy::Row(Box::new(row))
        };
    }
    Copy::Row(Box::new(row))
}

/// Whether the copy holds what its file holds now, so that it has nothing to offer.
fn holds_its_file(row: &Row, target: &Target) -> bool {
    let (Some(stamp), Some(location)) = (target.stamp, &target.location) else {
        return false;
    };
    let room = row.size == stamp.len || row.size + oso_core::decode::BOM.len() as u64 == stamp.len;
    if !room {
        return false;
    }
    open_checked(&row.text).is_ok_and(|(copy, _)| same_bytes(location, copy, row.size))
}

/// Which buffer a copy belongs to.
enum Matched<'a> {
    Target(&'a Target),
    /// A buffer has its path, and that path now leads elsewhere.
    Elsewhere,
    Nothing,
}

/// Compares what a copy says with what each buffer would write. All of this is comparing
/// bytes: nothing is decoded to be used.
fn matching<'a>(meta: &Meta, targets: &'a [Target]) -> Matched<'a> {
    let files = || targets.iter().filter(|target| target.has_file());
    if meta.path.is_none() && meta.location.is_none() {
        let same = targets
            .iter()
            .find(|target| !target.has_file() && target.name == meta.name && target.blank);
        return same.map_or(Matched::Nothing, Matched::Target);
    }
    if let Some(location) = &meta.location
        && let Some(target) = files().find(|t| {
            t.encoded_location.as_ref() == Some(location)
                || t.encoded_typed.as_ref() == Some(location)
        })
    {
        return Matched::Target(target);
    }
    let by_path = meta
        .path
        .as_ref()
        .and_then(|path| files().find(|t| t.encoded_path.as_ref() == Some(path)));
    match by_path {
        Some(target) if meta.location.is_none() => Matched::Target(target),
        Some(_) => Matched::Elsewhere,
        None => Matched::Nothing,
    }
}

/// What the file now is, against what the metadata records of it. The metadata is untrusted,
/// so this only chooses a word on a row.
fn relation(meta: &Meta, now: Option<FileStamp>) -> RowState {
    match (meta.file_bytes, now) {
        // A new file nobody had saved, and none there now.
        (None, None) => RowState::NoFile,
        (Some(_), None) => RowState::Gone,
        (None, Some(_)) => RowState::Changed,
        (Some(len), Some(now)) => {
            let same = meta.file_modified.is_some_and(|modified| {
                now.same_content(FileStamp {
                    modified,
                    len,
                    mode: 0,
                })
            });
            if same {
                RowState::Unchanged
            } else {
                RowState::Changed
            }
        }
    }
}

/// A row for the temporary file at `path`, once no writer can still be at work on it.
fn left_by_a_save(path: &Path, now: SystemTime) -> Option<Row> {
    let metadata = plain_own_file(path)?;
    if !left_behind(&metadata, now) {
        return None;
    }
    let written = metadata.modified().ok().and_then(seconds_of);
    let mut row = bare_row(path, None, RowState::LeftBySave, metadata.len(), written);
    // Its name says nothing, but a directory on the way may say it is a secret's.
    row.paths = vec![path.to_owned()];
    Some(row)
}

/// The temporary files left beside each file open. Only directories a person opened a file in
/// are read, and each once.
fn leftovers_beside_files(targets: &[Target], now: SystemTime) -> Vec<Row> {
    let mut directories: Vec<&Path> = targets
        .iter()
        .filter_map(|target| target.location.as_deref()?.parent())
        .collect();
    directories.sort_unstable();
    directories.dedup();
    let mut rows = Vec::new();
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.take(MAX_NAMES).flatten() {
            let name = entry.file_name();
            if name.to_str().is_some_and(is_temporary_name)
                && let Some(row) = left_by_a_save(&entry.path(), now)
            {
                rows.push(row);
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests;
