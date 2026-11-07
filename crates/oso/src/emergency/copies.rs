//! Finding emergency copies, off the UI thread (ADR 0062, ADR 0064): what is in the directory,
//! what each copy says, which buffer it belongs to, and what is left of interrupted writes.
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
use super::write::encoded;

mod files;
mod list;

pub use files::open_copy;
use files::{is_temporary, left_behind, plain_own_file, seconds_of, stem_seconds};
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
pub enum Word {
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

impl Word {
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
    encoded_location: Option<String>,
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
            encoded_location: baseline.map(|baseline| bytes(&baseline.location)),
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
    pub(super) word: Word,
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
    pub(super) file_bytes: Option<u64>,
    pub(super) file_modified: Option<SystemTime>,
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
            let now = SystemTime::now();
            // A panic must cost only this list. Its message may not reach anything.
            let id = request.id;
            let scan =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(&request, now)));
            report(scan.unwrap_or_else(|_| failed(id)));
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

/// Looks through the directory, and beside each file open, as of `now`.
pub(super) fn scan(request: &ScanRequest, now: SystemTime) -> Scan {
    let mut rows = Vec::new();
    let mut more = 0;
    let mut problem = None;
    let mut removed = Vec::new();
    if let Some(directory) = &request.directory {
        // Refused when another user could have put anything there.
        match PrivateDir::existing(directory) {
            Ok(None) => {}
            Ok(Some(_)) => {
                more = scan_directory(directory, request, now, &mut rows, &mut removed);
            }
            Err(e) => problem = Some(e.to_string()),
        }
    }
    rows.extend(leftovers_beside_files(&request.targets, now));
    // The newest first. A time from a `.meta` is untrusted, and only orders rows.
    rows.sort_by(|a: &Row, b: &Row| b.written.cmp(&a.written).then(a.text.cmp(&b.text)));
    let list = list_of(&rows, more, problem.as_deref(), now);
    Scan {
        id: request.id,
        rows,
        problem,
        removed,
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

/// Reads the names in `directory`, adds a row for each copy and temporary file, and returns
/// how many the limit left out.
fn scan_directory(
    directory: &Path,
    request: &ScanRequest,
    now: SystemTime,
    rows: &mut Vec<Row>,
    removed: &mut Vec<String>,
) -> usize {
    // Each stem, and whether a `.meta` of that name is there.
    let mut stems: BTreeMap<String, bool> = BTreeMap::new();
    let mut temporaries = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    for entry in entries.take(MAX_NAMES).flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_temporary(name) {
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
    newest.sort_by_key(|(stem, _)| std::cmp::Reverse(stem_seconds(stem)));
    let more = newest.len().saturating_sub(MAX_ROWS);
    for (stem, has_meta) in newest.into_iter().take(MAX_ROWS) {
        match copy_row(directory, &stem, has_meta, request, now) {
            Found::Row(row) => rows.push(*row),
            Found::Equal => removed.push(stem),
            Found::Nothing => {}
        }
    }
    rows.extend(
        temporaries
            .into_iter()
            .filter_map(|path| left_by_a_save(&path, now)),
    );
    more
}

/// What came of looking at one stem.
enum Found {
    Row(Box<Row>),
    /// It held what its file holds, and has gone.
    Equal,
    /// There is nothing to say yet.
    Nothing,
}

/// A row with what every row needs, for a file at `text` with only its own name to go on.
fn bare_row(text: &Path, meta: Option<&Path>, word: Word, size: u64, written: Option<u64>) -> Row {
    let name = text
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    Row {
        text: text.to_owned(),
        meta: meta.map(Path::to_owned),
        word,
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
        file_bytes: None,
        file_modified: None,
    }
}

/// A row for what has no usable `.meta`: nothing says where it belongs. A text with none at
/// all may be being written, so it waits until it has been left long enough.
fn unknown_row(text: &Path, meta: Option<&Path>, now: SystemTime) -> Found {
    let metadata = fs::symlink_metadata(text).ok();
    if meta.is_none()
        && !metadata
            .as_ref()
            .is_some_and(|metadata| left_behind(metadata, now))
    {
        return Found::Nothing;
    }
    let written = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(seconds_of);
    let size = metadata.map_or(0, |metadata| metadata.len());
    Found::Row(Box::new(bare_row(text, meta, Word::Unknown, size, written)))
}

fn copy_row(
    directory: &Path,
    stem: &str,
    has_meta: bool,
    request: &ScanRequest,
    now: SystemTime,
) -> Found {
    let text = directory.join(format!("{stem}.txt"));
    let meta_path = directory.join(format!("{stem}.meta"));
    let meta_kept = has_meta.then_some(meta_path.as_path());
    let Some(meta) = has_meta.then(|| read_meta(&meta_path)).flatten() else {
        return unknown_row(&text, meta_kept, now);
    };
    let looked = plain_own_file(&text);
    let size = looked.as_ref().map_or(0, Metadata::len);
    let mut row = bare_row(&text, meta_kept, Word::NotOpen, size, meta.written);
    row.name.clone_from(&meta.name);
    row.shown = meta.shown();
    row.paths = meta.decoded_paths();
    row.at_least = Verdict::Ordinary;
    row.why = meta.why;
    row.bom = meta.bom;
    row.line_ending = meta.line_ending;
    row.file_bytes = meta.file_bytes;
    row.file_modified = meta.file_modified;
    row.word = if meta.path.is_none() && meta.location.is_none() {
        Word::NoFile
    } else {
        Word::NotOpen
    };

    let matched = matching(&meta, &request.targets);
    match matched {
        Matched::Target(target) => {
            row.target = Some(target.buffer);
            row.at_least = target.verdict;
            row.location.clone_from(&target.location);
            if target.has_file() {
                row.word = relation(&meta, target.stamp);
            }
        }
        Matched::Elsewhere => row.word = Word::LeadsElsewhere,
        Matched::Nothing => {}
    }
    if looked.is_none() || size != meta.bytes || size >= MAX_LOOK {
        row.word = Word::Damaged;
    } else if request.recovered.contains(&text) {
        row.word = Word::Recovered;
    } else if let Matched::Target(target) = matched
        && target.has_file()
        && holds_its_file(&row, target)
    {
        return if remove_copy(&row.text, row.meta.as_deref()).is_ok() {
            Found::Equal
        } else {
            Found::Row(Box::new(row))
        };
    }
    Found::Row(Box::new(row))
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
        && let Some(target) = files().find(|t| t.encoded_location.as_ref() == Some(location))
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
fn relation(meta: &Meta, now: Option<FileStamp>) -> Word {
    match (meta.file_bytes, now) {
        (None, None) => Word::Unchanged,
        (Some(_), None) => Word::Gone,
        (None, Some(_)) => Word::Changed,
        (Some(len), Some(now)) => {
            let same = meta.file_modified.is_some_and(|modified| {
                now.same_content(FileStamp {
                    modified,
                    len,
                    mode: 0,
                })
            });
            if same { Word::Unchanged } else { Word::Changed }
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
    let mut row = bare_row(path, None, Word::LeftBySave, metadata.len(), written);
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
            if name.to_str().is_some_and(is_temporary)
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
