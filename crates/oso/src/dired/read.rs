//! Reading a listing on a lister's thread (ADR 0134): the directory opened by its path, its
//! entries read with their metadata from the descriptor, following no link, and the listing
//! built from them, a partial one every 50 ms while a large directory is read.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or winit. It runs on the lister's thread,
//! which a directory on a file system that hangs stalls alone.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use oso_platform::file_meta::{self, Directory, EntryKind, EntryMeta};

use super::build::{Entry, Leads, Shape, build};
use super::{Answer, Built, MAX_ENTRIES, Request, order};

/// How long reading goes before the first partial listing, and between two of them.
const FIRST_PARTIAL: Duration = Duration::from_millis(20);
const PARTIAL: Duration = Duration::from_millis(50);

/// The rows a partial listing shows: the first in order of those read so far, a few screens'
/// worth, so that one costs a selection over what was read and not a whole listing's build.
const PARTIAL_ROWS: usize = 500;

/// How many entries go between two looks at the clock and at whether the listing is still
/// wanted.
const CHECK_EVERY: usize = 256;

/// Lists `request`'s directory, handing `emit` each listing it builds: partial ones while it
/// reads, if the request asks for them, then the whole one, or why it could not be read. It
/// stops, sending nothing more, once `stale` says a newer request replaced this one.
pub(crate) fn list(request: &Request, stale: &dyn Fn() -> bool, emit: &mut dyn FnMut(Answer)) {
    let answer = |built| Answer {
        buffer: request.buffer,
        generation: request.generation,
        built,
    };
    let directory = match Directory::open(&request.directory) {
        Ok(directory) => directory,
        // It shows as a listing that says why, judged by the path named.
        Err(error) => {
            let mut reading = Reading::new(request, Some(vec![request.directory.clone()]));
            reading.unopened = Some(error.to_string());
            return emit(answer(Ok(reading.built(true))));
        }
    };
    let resolved = resolved_place(&request.directory, &directory);
    let mut reading = Reading::new(request, resolved);
    reading.identity = directory.identity().ok().flatten();
    let mut next = Instant::now() + FIRST_PARTIAL;
    let mut seen = 0_usize;
    let mut gone = false;
    let listed = directory.list_with_metadata(&mut |entry| {
        // Every entry counts, those left out too, so that no directory is read past the limit.
        if seen == MAX_ENTRIES {
            reading.cut = true;
            return false;
        }
        match entry {
            Ok(entry) => reading.take(&directory, entry),
            Err(_) => reading.unreadable += 1,
        }
        seen += 1;
        if !seen.is_multiple_of(CHECK_EVERY) {
            return true;
        }
        if stale() {
            gone = true;
            return false;
        }
        if request.partials && Instant::now() >= next {
            emit(answer(Ok(reading.partial())));
            next = Instant::now() + PARTIAL;
        }
        true
    });
    if gone || stale() {
        return;
    }
    if let Err(error) = listed {
        reading.failed = Some(error.to_string());
    }
    reading.add_dots(&directory);
    // The listing goes before its links are followed, which may wait on another file system,
    // then again once they are.
    let links = reading
        .entries
        .iter()
        .any(|entry| entry.kind == EntryKind::Link);
    if links {
        emit(answer(Ok(reading.built(true))));
        reading.look_at_links(&directory, stale);
        if stale() {
            return;
        }
    }
    emit(answer(Ok(reading.built(true))));
}

/// The real place of `path`, which `directory` was opened as, for its verdict: `None` if it
/// cannot be found, or if what is there now is not, by device and inode, the directory opened,
/// as a link swapped between the two would make it.
fn resolved_place(path: &Path, directory: &Directory) -> Option<Vec<PathBuf>> {
    let real = std::fs::canonicalize(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = directory.metadata().ok()?;
        let there = std::fs::metadata(&real).ok()?;
        if (opened.dev(), opened.ino()) != (there.dev(), there.ino()) {
            return None;
        }
    }
    #[cfg(not(unix))]
    let _ = directory;
    Some(vec![real])
}

/// Whether following the link `target`, in `directory`, may reach another machine by its words
/// alone: a UNC or device path, or a place the system mounts from the network when it is looked
/// at, such as `/net`, or another volume (`/Volumes` on macOS). Such a link is shown, and not
/// followed to tell what it leads to.
pub(super) fn reaches_out(directory: &Path, target: &Path) -> bool {
    let words = target.as_os_str().as_encoded_bytes();
    if words.starts_with(b"//") || words.starts_with(b"\\\\") {
        return true;
    }
    let mut place = PathBuf::new();
    for component in directory.join(target).components() {
        match component {
            std::path::Component::ParentDir => {
                place.pop();
            }
            std::path::Component::CurDir => {}
            // A share or a device: another machine, or something that is no file.
            #[cfg(windows)]
            std::path::Component::Prefix(prefix)
                if matches!(
                    prefix.kind(),
                    std::path::Prefix::UNC(..)
                        | std::path::Prefix::VerbatimUNC(..)
                        | std::path::Prefix::DeviceNS(..)
                ) =>
            {
                return true;
            }
            other => place.push(other),
        }
    }
    let automounted: &[&str] = if cfg!(target_os = "macos") {
        &["/net", "/Network", "/Volumes", "/home"]
    } else {
        &["/net", "/misc"]
    };
    automounted.iter().any(|root| place.starts_with(root))
}

/// A listing being read.
struct Reading<'a> {
    request: &'a Request,
    resolved: Option<Vec<PathBuf>>,
    entries: Vec<Entry>,
    /// `.` and `..`, once looked at.
    dots: Vec<Entry>,
    /// Entries that could not be looked at.
    unreadable: usize,
    /// Why reading stopped part way.
    failed: Option<String>,
    /// Why the directory could not be opened.
    unopened: Option<String>,
    /// Backups ending in `~`, left out as `ls -B` does.
    backups: usize,
    /// More entries were there than a listing keeps.
    cut: bool,
    /// The names marked in place of the request's, once inverted.
    inverted: Option<Arc<HashSet<OsString>>>,
    /// The directory's device and inode, from its descriptor.
    identity: Option<(u64, u64)>,
    names: Names,
    offsets: Offsets,
    now: SystemTime,
}

impl<'a> Reading<'a> {
    fn new(request: &'a Request, resolved: Option<Vec<PathBuf>>) -> Self {
        Self {
            request,
            resolved,
            entries: Vec::new(),
            dots: Vec::new(),
            unreadable: 0,
            failed: None,
            unopened: None,
            backups: 0,
            cut: false,
            inverted: None,
            identity: None,
            names: Names::default(),
            offsets: Offsets::default(),
            now: SystemTime::now(),
        }
    }

    /// Takes in an entry the listing read.
    fn take(&mut self, directory: &Directory, meta: EntryMeta) {
        if meta.name.as_encoded_bytes().ends_with(b"~") {
            self.backups += 1;
            return;
        }
        let target = (meta.kind == EntryKind::Link)
            .then(|| directory.read_link(&meta.name).ok())
            .flatten();
        let leads = if meta.kind == EntryKind::Link {
            Leads::Unknown
        } else {
            Leads::Here
        };
        let owner = self.names.owner(&meta);
        let mode_text = meta.mode_text();
        self.entries.push(Entry {
            name: meta.name,
            kind: meta.kind,
            mode: meta.mode,
            mode_text,
            len: meta.len,
            modified: meta.modified,
            owner,
            target,
            leads,
        });
    }

    /// Looks where each link leads, following it: a link to a directory is entered, and one to
    /// nothing is shown broken. A link to another file system waits for it, on this thread; one
    /// that may reach another machine is not followed. It stops once `stale` says so.
    fn look_at_links(&mut self, directory: &Directory, stale: &dyn Fn() -> bool) {
        let place = &self.request.directory;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.kind != EntryKind::Link {
                continue;
            }
            if index.is_multiple_of(CHECK_EVERY) && stale() {
                return;
            }
            if entry
                .target
                .as_ref()
                .is_none_or(|target| reaches_out(place, target))
            {
                continue;
            }
            entry.leads = match directory.target_kind(&entry.name) {
                Ok(EntryKind::Directory) => Leads::Directory,
                Ok(_) => Leads::Elsewhere,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Leads::Missing,
                Err(_) => Leads::Unknown,
            };
        }
    }

    /// Looks at `.`, the directory itself, and `..`, the one above it, from the directory.
    fn add_dots(&mut self, directory: &Directory) {
        for (name, meta) in [".", ".."].into_iter().zip(directory.dots()) {
            let mut entry = Entry {
                name: OsString::from(name),
                kind: EntryKind::Directory,
                ..Entry::default()
            };
            if let Ok(meta) = meta {
                entry.mode = meta.mode;
                entry.mode_text = meta.mode_text();
                entry.len = meta.len;
                entry.modified = meta.modified;
                entry.owner = self.names.owner(&meta);
            }
            self.dots.push(entry);
        }
    }

    /// A partial listing: the first [`PARTIAL_ROWS`] of what has been read, in order, under a
    /// line that says how many were read.
    fn partial(&mut self) -> Built {
        let sort = self.request.options.sort;
        if self.entries.len() > PARTIAL_ROWS {
            self.entries
                .select_nth_unstable_by(PARTIAL_ROWS, |a, b| order::compare(a, b, sort));
        }
        let first = self.entries.len().min(PARTIAL_ROWS);
        order::sort(&mut self.entries[..first], sort);
        let notes = vec![format!("reading… {} entries so far", self.entries.len())];
        let shown: Vec<&Entry> = self.entries[..first].iter().collect();
        let parts = (self.request, self.resolved.as_ref(), self.now);
        let mut built = listing(parts, &mut self.offsets, &notes, &shown, (false, None));
        built.identity = self.identity;
        built
    }

    /// The whole listing, ordered, its marks inverted if the request asked.
    fn built(&mut self, done: bool) -> Built {
        if self.request.invert && self.inverted.is_none() {
            let marked = &self.request.marked;
            let inverted: HashSet<OsString> = (self.entries.iter())
                .filter(|entry| !marked.contains(&entry.name))
                .map(|entry| entry.name.clone())
                .collect();
            self.inverted = Some(Arc::new(inverted));
        }
        order::sort(&mut self.entries, self.request.options.sort);
        let mut notes = Vec::new();
        if let Some(why) = &self.unopened {
            notes.push(format!("cannot be read: {why}"));
        }
        if self.cut {
            notes.push(format!("cut at {MAX_ENTRIES} entries"));
        }
        if self.unreadable > 0 {
            notes.push(format!(
                "{} entries could not be looked at",
                self.unreadable
            ));
        }
        if let Some(failed) = &self.failed {
            notes.push(format!("not read to its end: {failed}"));
        }
        let shown: Vec<&Entry> = self.dots.iter().chain(&self.entries).collect();
        let parts = (self.request, self.resolved.as_ref(), self.now);
        let inverted = self.inverted.clone();
        let mut built = listing(parts, &mut self.offsets, &notes, &shown, (done, inverted));
        built.identity = self.identity;
        built
    }
}

/// The request, the directory's real place and the time it is read at, which every listing
/// of a reading shares.
type Parts<'a> = (&'a Request, Option<&'a Vec<PathBuf>>, SystemTime);

/// Builds the text of `shown` under the header and `notes`.
fn listing(
    (request, resolved, now): Parts<'_>,
    offsets: &mut Offsets,
    notes: &[String],
    shown: &[&Entry],
    (done, inverted): (bool, Option<Arc<HashSet<OsString>>>),
) -> Built {
    let header = header_of(&request.directory, request.home.as_deref());
    let shape = Shape {
        header: header.as_encoded_bytes(),
        notes,
        details: request.options.details,
        marked: inverted.as_deref().unwrap_or(&request.marked),
    };
    let shown = build(&shape, shown, now, &mut |time| offsets.at(time));
    Built {
        text: shown.text,
        highlights: shown.highlights,
        rows: shown.rows,
        header_lines: shown.header_lines,
        name_column: shown.name_column,
        resolved: resolved.cloned(),
        done,
        marked: shown.marked,
        marked_names: inverted,
        identity: None,
        stamp: None,
    }
}

/// The directory as a header writes it: with the home directory as `~`, ending in `/`.
fn header_of(directory: &Path, home: Option<&Path>) -> OsString {
    let mut header = match home.and_then(|home| directory.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => OsString::from("~"),
        Some(rest) => {
            let mut under = OsString::from("~/");
            under.push(rest);
            under
        }
        None => directory.as_os_str().to_os_string(),
    };
    if !header.as_encoded_bytes().ends_with(b"/") {
        header.push("/");
    }
    header
}

/// The names of owners and groups, looked up once each and shared by the rows.
#[derive(Default)]
struct Names {
    users: HashMap<u32, Arc<str>>,
    groups: HashMap<u32, Arc<str>>,
}

impl Names {
    /// The owner and group of `meta`, only when someone other than this user owns it.
    fn owner(&mut self, meta: &EntryMeta) -> Option<(Arc<str>, Arc<str>)> {
        let (uid, gid) = meta.owner.filter(|_| meta.others)?;
        Some(self.pair(uid, gid))
    }

    fn pair(&mut self, uid: u32, gid: u32) -> (Arc<str>, Arc<str>) {
        let user = self
            .users
            .entry(uid)
            .or_insert_with(|| shown_name(file_meta::user_name(uid), uid).into())
            .clone();
        let group = self
            .groups
            .entry(gid)
            .or_insert_with(|| shown_name(file_meta::group_name(gid), gid).into())
            .clone();
        (user, group)
    }
}

/// A name from the user or group database as a row shows it: as it is if it is printable ASCII,
/// else the id, so every byte before a row's name is ASCII and no name from there can carry a
/// control or bidirectional character into the row.
fn shown_name(name: Option<String>, id: u32) -> String {
    match name {
        Some(name) if !name.is_empty() && name.bytes().all(|b| b.is_ascii_graphic()) => name,
        _ => id.to_string(),
    }
}

/// The local zone's offset, asked once for each hour of time.
#[derive(Default)]
struct Offsets {
    by_hour: HashMap<i64, i32>,
}

impl Offsets {
    fn at(&mut self, time: SystemTime) -> i32 {
        let hour = match time.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_secs() / 3600).unwrap_or(i64::MAX),
            Err(before) => -i64::try_from(before.duration().as_secs() / 3600).unwrap_or(i64::MAX),
        };
        *self
            .by_hour
            .entry(hour)
            .or_insert_with(|| file_meta::utc_offset_at(time).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests;
