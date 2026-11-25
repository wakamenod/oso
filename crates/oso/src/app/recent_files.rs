//! The recent files (ADR 0148), remembered in `oso/recent-files` in the per-user state
//! directory, beside `theme` and `sessions`: one JSON object a line, a path and when it was
//! last shown in milliseconds since the epoch, the most recent first. No text of any file.
//!
//! Must not depend on: key presses or how a frame is drawn. The file is read on a thread of
//! its own, started first thing and taken after an event once done, and written on another,
//! so the UI thread never waits on it, but at an exit or a restart's start, at most
//! [`FLUSH_WAIT`].
//!
//! - It is read as `sessions` is, with `sessions::record`'s helpers: only if the directory is
//!   private and the file a regular one of this user's with one name, at most [`MAX_BYTES`]. A
//!   line that holds no whole entry, or a path the editor would not keep, is left out, and only
//!   how many were is logged.
//! - It holds the person's visits until their loads end, each marked if it was chosen from the
//!   list, so that a load that finds nothing then drops the file rather than opening a new
//!   one (`app/loads.rs`).
//! - A write waits [`QUIET`] after the last change, then replaces the file whole through a
//!   `.part` of mode `0600`, or leaves it as it was. It reads the file first and keeps what
//!   another instance wrote that this one did not drop, each path once, the most recently
//!   shown first, at most [`MAX_RECENT_FILES`] and [`MAX_BYTES`].

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_core::buffer::BufferId;
use oso_core::editor::{MAX_RECENT_FILES, MAX_RECENT_PATH};
use oso_platform::privacy::PrivateDir;
use serde_json::{Value, json};

use super::App;

/// The file's name in its directory.
const FILE: &str = "recent-files";
/// Where a write goes first, so that the file is whole or not there.
const PART: &str = "recent-files.part";
/// The most bytes read or written of the file.
const MAX_BYTES: u64 = 256 * 1024;
/// How long a write waits after the last change, so that a run of visits writes once.
const QUIET: Duration = Duration::from_secs(1);
/// How long an exit, or a restart's start, waits for this write and the sessions' in all, as
/// ADR 0121 waits for those alone: a write takes some milliseconds; a thread that is stuck must
/// not hold the exit.
const FLUSH_WAIT: Duration = Duration::from_secs(1);

/// A recent file, with when it was last shown, in milliseconds since the epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    path: PathBuf,
    shown: u64,
}

/// What goes to the writer.
enum Job {
    /// The list as this instance knows it, and the files it dropped, each with when.
    Write {
        entries: Vec<Entry>,
        forgotten: Vec<Entry>,
    },
    /// Answer once what was handed over before is written.
    Flush(SyncSender<()>),
}

/// The recent files' side of the app: the stamps of what the editor lists, the person's
/// visits whose loads have not ended, and the writer.
#[derive(Default)]
pub(super) struct RecentFiles {
    /// When each listed file was last shown.
    stamps: HashMap<PathBuf, u64>,
    /// The files dropped since the last write, each with when.
    forgotten: Vec<Entry>,
    /// The buffers the person's commands opened, until their loads end, each with whether it
    /// was chosen from the recent files (ADR 0148).
    pub(super) awaiting: HashMap<BufferId, bool>,
    /// Something changed since the writer was last handed the list.
    unsaved: bool,
    /// The newest stamp given, so that two changes in one millisecond keep their order.
    last: u64,
    /// What the read at start found, until it is taken.
    read: Option<Receiver<Vec<Entry>>>,
    jobs: Option<Sender<Job>>,
}

impl RecentFiles {
    /// Starts reading the file in `directory`, taken after an event once it is read, and the
    /// thread that writes it. With no directory, nothing is read or written.
    pub(super) fn start(directory: Option<PathBuf>) -> Self {
        let Some(directory) = directory else {
            return Self::default();
        };
        let reading = directory.clone();
        let (sender, found) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("oso-recent-read".into())
            .spawn(move || {
                let _ = sender.send(read(&reading));
            });
        let read = match spawned {
            Ok(_) => Some(found),
            Err(e) => {
                log::warn!("not reading the recent files: {e}");
                None
            }
        };
        let (jobs, waiting) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("oso-recent".into())
            .spawn(move || run(&directory, &waiting));
        let jobs = match spawned {
            Ok(_) => Some(jobs),
            Err(e) => {
                log::warn!("the recent files will not be remembered: {e}");
                None
            }
        };
        Self {
            read,
            jobs,
            ..Self::default()
        }
    }

    /// A stamp for now, after every one given before.
    fn now(&mut self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |now| u64::try_from(now.as_millis()).unwrap_or(u64::MAX));
        self.last = now.max(self.last.saturating_add(1));
        self.last
    }
}

impl App {
    /// Takes what the read at start found, once it is done: the editor lists those files after
    /// the ones this instance has shown since it started. It never waits.
    fn take_recent_read(&mut self) {
        let Some(receiver) = &self.recent.read else {
            return;
        };
        let read = match receiver.try_recv() {
            Ok(read) => read,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        self.recent.read = None;
        let recent = &mut self.recent;
        for entry in &read {
            recent
                .stamps
                .entry(entry.path.clone())
                .or_insert(entry.shown);
        }
        self.editor
            .remember_recent_files(read.into_iter().map(|entry| entry.path));
    }

    /// The person's visit of `buffer` started, `from_recent` if chosen from the recent files:
    /// its file goes in them once it is loaded, or out if it is gone (ADR 0148).
    pub(super) fn await_recent(&mut self, buffer: BufferId, from_recent: bool) {
        self.recent.awaiting.insert(buffer, from_recent);
    }

    /// The load of `buffer` ended with its text: if the person's command opened it, its file
    /// goes first in the recent files.
    pub(super) fn recent_loaded(&mut self, buffer: BufferId) {
        if self.recent.awaiting.remove(&buffer).is_some() {
            self.editor.note_recent_visit(buffer);
        }
    }

    /// After an event: stamps what went first in the recent files or was dropped, and hands
    /// the list to the writer if it changed.
    pub(super) fn sync_recent_files(&mut self) {
        self.take_recent_read();
        // A visit whose buffer went, or whose load ended without its text, is let go, in one
        // pass over the buffers.
        if !self.recent.awaiting.is_empty() {
            let loading: HashSet<BufferId> = (self.editor.buffers())
                .filter(|(_, buffer)| buffer.loading().is_some())
                .map(|(id, _)| id)
                .collect();
            self.recent.awaiting.retain(|id, _| loading.contains(id));
        }
        let changes = self.editor.take_recent_changes();
        if changes.is_empty() {
            return;
        }
        for path in changes.shown {
            let now = self.recent.now();
            self.recent.stamps.insert(path, now);
        }
        for path in changes.forgotten {
            let shown = self.recent.now();
            self.recent.stamps.remove(&path);
            self.recent.forgotten.push(Entry { path, shown });
        }
        self.recent.unsaved = true;
        self.hand_recent_files();
    }

    /// Hands the list to the writer, if it changed since it was last handed.
    fn hand_recent_files(&mut self) {
        let recent = &mut self.recent;
        if !std::mem::take(&mut recent.unsaved) {
            return;
        }
        let listed = self.editor.recent_files();
        let kept: HashSet<&Path> = listed.iter().map(PathBuf::as_path).collect();
        recent
            .stamps
            .retain(|path, _| kept.contains(path.as_path()));
        let entries = listed
            .iter()
            .map(|path| Entry {
                path: path.clone(),
                shown: recent.stamps.get(path).copied().unwrap_or(0),
            })
            .collect();
        let forgotten = std::mem::take(&mut recent.forgotten);
        if let Some(jobs) = &recent.jobs {
            let _ = jobs.send(Job::Write { entries, forgotten });
        }
    }

    /// Hands the sessions, the spaces, the recent files, the prompt history and the reviews to their threads, if
    /// they changed, and waits at most [`FLUSH_WAIT`] in all until they are written: before the
    /// process goes, or a restart's new instance reads them (ADR 0121, ADR 0148, ADR 0122). Past the wait it goes on,
    /// and says so in the log.
    pub(super) fn flush_records(&mut self) {
        let started = Instant::now();
        // The reviews too (ADR 0122, step 6), handed over first so that all write at once.
        self.hand_review_saves();
        self.sync_recent_files();
        let prompts = self.flush_prompt_history();
        let (done, answer) = mpsc::sync_channel(1);
        let asked =
            (self.recent.jobs.as_ref()).is_some_and(|jobs| jobs.send(Job::Flush(done)).is_ok());
        // Both write at once; the sessions' wait is within the same second.
        self.flush_sessions();
        let left = FLUSH_WAIT.saturating_sub(started.elapsed());
        if asked && answer.recv_timeout(left).is_err() {
            log::warn!("the recent files were not written within {FLUSH_WAIT:?}");
        }
        let left = FLUSH_WAIT.saturating_sub(started.elapsed());
        if prompts.is_some_and(|written| written.recv_timeout(left).is_err()) {
            log::warn!("the prompt history was not written within {FLUSH_WAIT:?}");
        }
        self.wait_review_saves(FLUSH_WAIT.saturating_sub(started.elapsed()));
    }
}

/// The writer's thread: writes the newest list once [`QUIET`] has passed with no other, or
/// at once when asked to flush, until the app is gone.
fn run(directory: &Path, jobs: &Receiver<Job>) {
    let mut waiting: Option<(Vec<Entry>, Vec<Entry>)> = None;
    loop {
        let job = if waiting.is_some() {
            match jobs.recv_timeout(QUIET) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    write_waiting(directory, &mut waiting);
                    return;
                }
            }
        } else {
            match jobs.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            }
        };
        match job {
            Some(Job::Write { entries, forgotten }) => {
                let dropped = match waiting.take() {
                    Some((_, mut before)) => {
                        before.extend(forgotten);
                        before
                    }
                    None => forgotten,
                };
                waiting = Some((entries, dropped));
            }
            Some(Job::Flush(done)) => {
                write_waiting(directory, &mut waiting);
                let _ = done.send(());
            }
            None => write_waiting(directory, &mut waiting),
        }
    }
}

/// Writes what waits, if anything does.
fn write_waiting(directory: &Path, waiting: &mut Option<(Vec<Entry>, Vec<Entry>)>) {
    if let Some((entries, forgotten)) = waiting.take()
        && let Err(e) = write(directory, &entries, &forgotten)
    {
        log::warn!("could not remember the recent files: {e}");
    }
}

/// What the file in `directory` remembers: nothing if there is none or it may not be read.
fn read(directory: &Path) -> Vec<Entry> {
    match read_checked(directory) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("not reading the recent files: {e}");
            Vec::new()
        }
    }
}

/// What the file in `directory` remembers: nothing if there is none, and an error if there is
/// one that may not be read, which a write must then not replace. It is read as `sessions` is.
fn read_checked(directory: &Path) -> io::Result<Vec<Entry>> {
    let bytes = crate::sessions::record::read_file(directory, FILE, MAX_BYTES)?;
    Ok(bytes.map_or_else(Vec::new, |bytes| parse(&bytes)))
}

/// The entries in the file's bytes, each path once, the most recently shown first, at most
/// [`MAX_RECENT_FILES`].
fn parse(bytes: &[u8]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut bad = 0;
    for line in bytes.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match parse_line(line) {
            Some(entry) => entries.push(entry),
            None => bad += 1,
        }
    }
    if bad > 0 {
        log::warn!("left out {bad} lines of the recent files that hold no whole entry");
    }
    newest_first(entries)
}

/// The entry a line holds, if the editor would keep its path: absolute, in UTF-8, of at most
/// [`MAX_RECENT_PATH`] bytes, with no control character and no name that may hold secrets. So
/// a write never copies back what another instance, or an older build, kept that this one
/// would not (ADR 0148).
fn parse_line(line: &[u8]) -> Option<Entry> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let text = value.get("path")?.as_str()?;
    let shown = value.get("shown")?.as_u64()?;
    let path = PathBuf::from(text);
    let kept = path.is_absolute()
        && text.len() <= MAX_RECENT_PATH
        && !text.chars().any(char::is_control)
        && !oso_core::buffer::Buffer::name_may_hold_secrets(&path);
    kept.then_some(Entry { path, shown })
}

/// `entries` with each path once, at its latest, the most recently shown first, at most
/// [`MAX_RECENT_FILES`]. Of two shown at once, the one first in `entries` stays first.
fn newest_first(entries: Vec<Entry>) -> Vec<Entry> {
    let mut kept: Vec<Entry> = Vec::with_capacity(entries.len());
    let mut at: HashMap<PathBuf, usize> = HashMap::new();
    for entry in entries {
        match at.get(&entry.path) {
            Some(&index) => kept[index].shown = kept[index].shown.max(entry.shown),
            None => {
                at.insert(entry.path.clone(), kept.len());
                kept.push(entry);
            }
        }
    }
    kept.sort_by_key(|entry| std::cmp::Reverse(entry.shown));
    kept.truncate(MAX_RECENT_FILES);
    kept
}

/// Writes `entries` in `directory`, which is made private if it is missing, with what is there
/// already that this instance did not drop: a path dropped is kept only if it was shown
/// after. A file there that may not be read is not replaced.
fn write(directory: &Path, entries: &[Entry], forgotten: &[Entry]) -> io::Result<()> {
    let theirs = read_checked(directory)?;
    let directory = PrivateDir::open(directory)?;
    let dropped = |entry: &Entry| {
        forgotten
            .iter()
            .any(|gone| gone.path == entry.path && gone.shown >= entry.shown)
    };
    let all = entries
        .iter()
        .cloned()
        .chain(theirs)
        .filter(|e| !dropped(e));
    let mut text = String::new();
    let most = usize::try_from(MAX_BYTES).unwrap_or(usize::MAX);
    for entry in newest_first(all.collect()) {
        let Some(path) = entry.path.to_str() else {
            continue;
        };
        let line = json!({ "path": path, "shown": entry.shown }).to_string();
        if text.len() + line.len() + 1 > most {
            break;
        }
        text.push_str(&line);
        text.push('\n');
    }
    crate::sessions::record::replace_file(&directory, FILE, PART, &text)
}

// The paths and the private directory these tests use are Unix ones.
#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    use super::*;

    /// A fresh private directory for one test, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let test = thread::current();
            let test = test.name().unwrap_or("test").replace("::", "-");
            let name = format!("oso-recent-{}-{test}", std::process::id());
            let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entry(path: &str, shown: u64) -> Entry {
        Entry {
            path: PathBuf::from(path),
            shown,
        }
    }

    fn paths(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.path.to_str().unwrap())
            .collect()
    }

    #[test]
    fn what_is_written_reads_back_newest_first_in_a_private_file() {
        let scratch = Scratch::new();
        write(&scratch.0, &[entry("/w/b", 2), entry("/w/a", 3)], &[]).unwrap();
        assert_eq!(paths(&read(&scratch.0)), ["/w/a", "/w/b"]);
        let meta = fs::metadata(scratch.0.join(FILE)).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert!(!scratch.0.join(PART).exists());
    }

    #[test]
    fn a_write_keeps_another_instances_files_but_not_one_dropped_since() {
        let scratch = Scratch::new();
        write(
            &scratch.0,
            &[entry("/w/theirs", 5), entry("/w/gone", 4)],
            &[],
        )
        .unwrap();
        write(
            &scratch.0,
            &[entry("/w/ours", 9), entry("/w/theirs", 1)],
            &[entry("/w/gone", 6)],
        )
        .unwrap();
        let read = read(&scratch.0);
        assert_eq!(paths(&read), ["/w/ours", "/w/theirs"]);
        assert_eq!(read[1].shown, 5, "the latest of a path is kept");
        // A file dropped here and shown since by another instance stays.
        write(&scratch.0, &[], &[entry("/w/ours", 8)]).unwrap();
        assert_eq!(paths(&super::read(&scratch.0)), ["/w/ours", "/w/theirs"]);
    }

    #[test]
    fn a_write_never_copies_back_a_path_the_editor_would_not_keep() {
        let scratch = Scratch::new();
        let theirs = concat!(
            "{\"path\":\"/w/.env\",\"shown\":9}\n",
            "{\"path\":\"/w/.ssh/config\",\"shown\":8}\n",
            "{\"path\":\"/w/a\\u0007b\",\"shown\":7}\n",
            "{\"path\":\"/w/kept\",\"shown\":6}\n",
        );
        fs::write(scratch.0.join(FILE), theirs).unwrap();
        fs::set_permissions(scratch.0.join(FILE), fs::Permissions::from_mode(0o600)).unwrap();
        write(&scratch.0, &[entry("/w/a", 1)], &[]).unwrap();
        let text = fs::read_to_string(scratch.0.join(FILE)).unwrap();
        assert_eq!(
            paths(&parse(text.as_bytes())),
            ["/w/kept", "/w/a"],
            "{text}"
        );
    }

    #[test]
    fn a_write_keeps_a_hundred() {
        let scratch = Scratch::new();
        let many: Vec<Entry> = (0..150).map(|n| entry(&format!("/w/{n}"), n)).collect();
        write(&scratch.0, &many, &[]).unwrap();
        let read = read(&scratch.0);
        assert_eq!(read.len(), MAX_RECENT_FILES);
        assert_eq!(read[0].path, PathBuf::from("/w/149"));
    }

    #[test]
    fn lines_with_no_whole_entry_are_left_out() {
        let text = concat!(
            "{\"path\":\"/w/a\",\"shown\":1}\n",
            "not json\n",
            "{\"path\":\"relative\",\"shown\":2}\n",
            "{\"path\":\"/w/b\"}\n",
            "{\"path\":\"/w/a\",\"shown\":7}\n",
            "\n",
            "{\"path\":\"/w/c\",\"shown\":3}",
        );
        let parsed = parse(text.as_bytes());
        assert_eq!(paths(&parsed), ["/w/a", "/w/c"]);
        assert_eq!(parsed[0].shown, 7);
        let long = format!(
            "{{\"path\":\"/{}\",\"shown\":1}}",
            "a".repeat(MAX_RECENT_PATH)
        );
        assert!(parse(long.as_bytes()).is_empty());
    }

    #[test]
    fn a_file_that_is_not_plain_is_neither_read_nor_replaced() {
        let scratch = Scratch::new();
        // A hard link: the file has two names.
        let elsewhere = scratch.0.join("elsewhere");
        fs::write(&elsewhere, "{\"path\":\"/w/a\",\"shown\":1}\n").unwrap();
        fs::hard_link(&elsewhere, scratch.0.join(FILE)).unwrap();
        assert!(read(&scratch.0).is_empty());
        assert!(write(&scratch.0, &[entry("/w/b", 2)], &[]).is_err());
        assert_eq!(
            fs::read_to_string(&elsewhere).unwrap(),
            "{\"path\":\"/w/a\",\"shown\":1}\n"
        );
        fs::remove_file(scratch.0.join(FILE)).unwrap();
        // A symbolic link.
        std::os::unix::fs::symlink(&elsewhere, scratch.0.join(FILE)).unwrap();
        assert!(read(&scratch.0).is_empty());
        assert!(write(&scratch.0, &[entry("/w/b", 2)], &[]).is_err());
        fs::remove_file(scratch.0.join(FILE)).unwrap();
        // Too large.
        fs::write(scratch.0.join(FILE), vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(read(&scratch.0).is_empty());
    }

    #[test]
    fn a_directory_others_can_reach_is_not_read() {
        let scratch = Scratch::new();
        write(&scratch.0, &[entry("/w/a", 1)], &[]).unwrap();
        fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(read(&scratch.0).is_empty());
        assert!(write(&scratch.0, &[entry("/w/b", 2)], &[]).is_err());
    }

    #[test]
    fn the_writer_waits_for_quiet_and_a_flush_writes_at_once() {
        let scratch = Scratch::new();
        let (jobs, waiting) = mpsc::channel();
        let directory = scratch.0.clone();
        let writer = thread::spawn(move || run(&directory, &waiting));
        for n in 0..20 {
            let entries = vec![entry(&format!("/w/{n}"), n)];
            jobs.send(Job::Write {
                entries,
                forgotten: Vec::new(),
            })
            .unwrap();
        }
        let (done, answer) = mpsc::sync_channel(1);
        jobs.send(Job::Flush(done)).unwrap();
        answer.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(paths(&read(&scratch.0)), ["/w/19"], "the newest list only");
        drop(jobs);
        writer.join().unwrap();
    }
}
