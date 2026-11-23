//! What dictation learns, in the app (ADR 0175, step 3): each dictation typed outside the
//! minibuffer and buffers that may hold secrets is watched once it settles, and when the watch
//! ends what it saw goes to a thread of its own, which keeps the record and the log in oso's
//! state directory. Learning is silent: only `dictation-learning-log` shows it (`log_view`).
//!
//! Must not depend on: how a frame is drawn, or TypeWhisper: its dictionary is `promoting`'s,
//! whose jobs run on the same thread.
//!
//! - **A watch** keeps the span (`oso_core::editor`'s dictation), the text as typed, the text
//!   oso left and the fixes made. It ends [`QUIET`] after the last of the person's commands
//!   that changed the span, but never before [`SHORTEST`], and at [`LONGEST`] at the latest;
//!   past [`MAX_WATCHES`] the oldest ends at once. It is dropped, learning nothing, when the
//!   editor ends the span: its buffer went, was read again or may hold secrets, or something
//!   other than the person's commands changed it. `dictation-forget-learned`, an exit and a
//!   restart drop every watch.
//! - **The thread** reads the record, takes in what the watch saw, and replaces the record and
//!   the log whole through a `.part` of mode `0600`, in a private directory. A file it may not
//!   read is left as it is. A write under way at an exit may be lost.
//! - A fix's thread reads the record itself, for the history (`fix.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_core::editor::DictationSpan;
use oso_platform::privacy::PrivateDir;

use super::App;
use super::promoting::{self, Endpoint, Passes, Taken};
use crate::app::UserEvent;
use crate::dictation::learning::{self, History, Record, Watched};
use crate::dictation::state::Heard;
use crate::sessions::record::{read_file, replace_file};

/// How long after the last of the person's changes to a dictation its watch ends.
const QUIET: Duration = Duration::from_secs(30);
/// How long a watch lasts at least, so that an undo soon after a fix is seen.
const SHORTEST: Duration = Duration::from_secs(60);
/// How long a watch lasts at most.
const LONGEST: Duration = Duration::from_secs(5 * 60);
/// The most watches at once, as many as the editor keeps settled spans.
const MAX_WATCHES: usize = 8;

/// How long a watch waits: [`QUIET`], [`SHORTEST`] and [`LONGEST`].
#[derive(Clone, Copy)]
struct Timing {
    quiet: Duration,
    shortest: Duration,
    longest: Duration,
}

/// The watches' timing; in a dev build's end-to-end flow (`OSO_DEV_DICTATION_QUICK`), 1 s,
/// 2 s and 10 s, so that a flow sees a watch end.
fn timing() -> Timing {
    #[cfg(feature = "dev")]
    if std::env::var_os("OSO_DEV_DICTATION_QUICK").is_some() {
        return Timing {
            quiet: Duration::from_secs(1),
            shortest: Duration::from_secs(2),
            longest: Duration::from_secs(10),
        };
    }
    Timing {
        quiet: QUIET,
        shortest: SHORTEST,
        longest: LONGEST,
    }
}

/// One dictation watched.
pub(super) struct Watch {
    span: DictationSpan,
    typed: String,
    left: String,
    fixed: Vec<(String, String)>,
    /// A fix saw the text.
    sent: bool,
    started: Instant,
    timing: Timing,
    /// The person's commands that changed it, as last seen.
    edits: u64,
    /// When they were last seen to have.
    last_edit: Option<Instant>,
}

impl Watch {
    /// When it ends, unless the person changes it again.
    fn due(&self) -> Instant {
        let Timing {
            quiet,
            shortest,
            longest,
        } = self.timing;
        let longest = self.started + longest;
        match self.last_edit {
            Some(edited) => (edited + quiet).max(self.started + shortest).min(longest),
            None => longest,
        }
    }
}

/// What goes to the learning's thread, when, in seconds since the epoch, and how many forgets
/// came before it was made: one made before the last forget is dropped.
pub(super) struct Job {
    at: u64,
    made: u64,
    work: Work,
}

/// What a job does.
pub(super) enum Work {
    /// A watch ended on what it saw.
    Watched(Watched),
    /// A fix was given this history: logged as counts.
    History(History),
    /// A dictation that was typed heard this: the entries of TypeWhisper's dictionary it used
    /// are noted (step 4).
    Heard(Heard),
    /// A pass over TypeWhisper's dictionary.
    Promote(Endpoint),
    /// The record, the log and what oso added to TypeWhisper go; then this is called with
    /// whether they did.
    Forget(Option<Endpoint>, Box<dyn FnOnce(Forgot) + Send>),
}

/// What the app tells the learning's thread at once, past the jobs that wait.
#[derive(Default)]
pub(super) struct Shared {
    /// How many forgets this instance asked for.
    pub(super) forgets: AtomicU64,
    /// A dictation is under way.
    pub(super) busy: AtomicBool,
}

/// Where a fix's thread sends the history it was given.
#[derive(Clone)]
pub(super) struct Jobs {
    jobs: Sender<Job>,
    made: u64,
}

impl Jobs {
    /// Sends `work`, as made when this was.
    pub(super) fn send(&self, work: Work) -> bool {
        let job = Job {
            at: now_secs(),
            made: self.made,
            work,
        };
        self.jobs.send(job).is_ok()
    }
}

/// What came of a forget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Forgot {
    /// The record and the log are gone.
    record: bool,
    /// What came of TypeWhisper's entries.
    typewhisper: Taken,
}

impl Forgot {
    /// What to say of it.
    fn said(self) -> String {
        match (self.record, self.typewhisper) {
            (false, _) => {
                "Dictation could not forget what it learned: oso's log says why".to_owned()
            }
            (true, Taken::All { kept: 0 }) => "Dictation forgot everything it learned".to_owned(),
            (true, Taken::All { kept }) => format!(
                "Dictation forgot everything it learned, and left {kept} you changed in \
                 TypeWhisper"
            ),
            (true, Taken::Left(1)) => "Dictation forgot what it learned; 1 entry goes from \
                                       TypeWhisper once it answers a dictation"
                .to_owned(),
            (true, Taken::Left(left)) => format!(
                "Dictation forgot what it learned; {left} entries go from TypeWhisper once it \
                 answers a dictation"
            ),
            (true, Taken::Unknown) => "Dictation forgot what it learned, but not what it added to \
                                       TypeWhisper: oso's log says why"
                .to_owned(),
        }
    }
}

/// The app's part of learning.
#[derive(Default)]
pub(super) struct Learning {
    watches: Vec<Watch>,
    jobs: Option<Sender<Job>>,
    pub(super) shared: Arc<Shared>,
}

/// Seconds since the epoch now.
pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| now.as_secs())
}

/// Where the record and the log are, in oso's state directory `state`.
pub(super) fn directory(state: &Path) -> PathBuf {
    state.join(learning::DIRECTORY)
}

impl App {
    /// The sender of jobs to the learning's thread, which starts on the first: `None` with no
    /// state directory, or if the thread cannot start.
    pub(super) fn learning_jobs(&mut self) -> Option<Jobs> {
        let shared = Arc::clone(&self.dictations.learning.shared);
        let made = shared.forgets.load(Ordering::Relaxed);
        if let Some(jobs) = &self.dictations.learning.jobs {
            let jobs = jobs.clone();
            return Some(Jobs { jobs, made });
        }
        let directory = directory(&self.hosting.state_directory()?);
        let (jobs, waiting) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("oso-dictation-learn".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || run(&directory, &waiting, &shared)
            });
        if let Err(e) = spawned {
            log::warn!("dictation will learn nothing: {e}");
            return None;
        }
        self.dictations.learning.jobs = Some(jobs.clone());
        Some(Jobs { jobs, made })
    }

    /// Sends `work` to the learning's thread. Says whether it went.
    pub(super) fn learn(&mut self, work: Work) -> bool {
        self.learning_jobs().is_some_and(|jobs| jobs.send(work))
    }

    /// Watches `span`, whose text was typed as `typed` and left as `left` with the fixes
    /// `fixed`, `sent` saying a fix saw it: or ends it, if the text is longer than a rewrite is
    /// looked for in.
    pub(super) fn watch_dictation(
        &mut self,
        span: DictationSpan,
        typed: String,
        left: String,
        fixed: Vec<(String, String)>,
        sent: bool,
    ) {
        let side = learning::rewrites::MAX_SIDE;
        if typed.len() > side || left.len() > side || self.hosting.state_directory().is_none() {
            return self.editor.end_dictation_span(span);
        }
        if self.dictations.learning.watches.len() >= MAX_WATCHES {
            let oldest = self.dictations.learning.watches.remove(0);
            self.end_watch(oldest);
        }
        self.dictations.learning.watches.push(Watch {
            span,
            typed,
            left,
            fixed,
            sent,
            started: Instant::now(),
            timing: timing(),
            edits: 0,
            last_edit: None,
        });
    }

    /// Looks at each watch: notes the person's changes, drops those whose span ended, and
    /// ends those that are due. The host calls this before it waits.
    pub(in crate::app) fn dictation_watches_due(&mut self) {
        if self.dictations.learning.watches.is_empty() {
            return;
        }
        let now = Instant::now();
        // In place, as this runs each turn of the loop: at most [`MAX_WATCHES`].
        let mut at = 0;
        while let Some(watch) = self.dictations.learning.watches.get_mut(at) {
            let Some(edits) = self.editor.dictation_span_edits(watch.span) else {
                self.dictations.learning.watches.remove(at);
                continue;
            };
            if edits != watch.edits {
                watch.edits = edits;
                watch.last_edit = Some(now);
            }
            if now >= watch.due() {
                let watch = self.dictations.learning.watches.remove(at);
                self.end_watch(watch);
            } else {
                at += 1;
            }
        }
    }

    /// How many dictations are watched, for the dev socket's state.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn dictation_watches(&self) -> usize {
        self.dictations.learning.watches.len()
    }

    /// When the next watch ends, for the event loop to wake.
    pub(in crate::app) fn dictation_watch_deadline(&self) -> Option<Instant> {
        self.dictations
            .learning
            .watches
            .iter()
            .map(Watch::due)
            .min()
    }

    /// Ends `watch`: what it saw goes to the learning's thread, and its span ends.
    fn end_watch(&mut self, watch: Watch) {
        let current = self
            .editor
            .dictation_span_text(watch.span, learning::rewrites::MAX_SIDE);
        self.editor.end_dictation_span(watch.span);
        let Some(current) = current else {
            return;
        };
        let watched = Watched {
            typed: watch.typed,
            left: watch.left,
            fixed: watch.fixed,
            current,
            sent: watch.sent,
        };
        self.learn(Work::Watched(watched));
        self.promote_learned();
    }

    /// Drops every watch, learning nothing from them: as oso exits, or forgets.
    pub(super) fn drop_dictation_watches(&mut self) {
        for watch in std::mem::take(&mut self.dictations.learning.watches) {
            self.editor.end_dictation_span(watch.span);
        }
    }

    /// `dictation-forget-learned`: the fixes under way stop, as `dictation-cancel` stops them,
    /// as each would watch its text; the watches go, then the record and the log, and any job
    /// made before, and what oso added to TypeWhisper's dictionary. The log's buffer empties.
    pub(super) fn forget_dictation_learning(&mut self) {
        if self.stop_dictation_fixes() {
            // A composer's send that waited for a fix does not go with the text unfixed.
            self.drop_held_sends();
        }
        self.drop_dictation_watches();
        self.empty_dictation_log();
        if self.hosting.state_directory().is_none() {
            return self
                .editor
                .message("Dictation has learned nothing: oso has no state directory");
        }
        // At once, so that a pass under way and the jobs that wait see it.
        (self.dictations.learning.shared.forgets).fetch_add(1, Ordering::Relaxed);
        let endpoint = self.typewhisper();
        let waker = self.waker.clone();
        let done = Box::new(move |forgot| {
            waker.send(UserEvent::DictationForgot(forgot));
        });
        if !self.learn(Work::Forget(endpoint, done)) {
            self.editor
                .message("Dictation could not forget what it learned");
        }
    }

    /// The learning's thread said what came of a forget.
    pub(in crate::app) fn dictation_forgot(&mut self, forgot: Forgot) {
        // It may have been shown again before the log was gone.
        self.empty_dictation_log();
        self.editor.message(forgot.said());
        self.request_redraw();
    }
}

/// The learning's thread: each job in turn, until the app goes. A job that panics loses only
/// itself, so that learning goes on.
fn run(directory: &Path, jobs: &Receiver<Job>, shared: &Shared) {
    let mut passes = Passes::default();
    while let Ok(Job { at, made, work }) = jobs.recv() {
        let forgotten = made < shared.forgets.load(Ordering::Relaxed);
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match work {
            Work::Forget(endpoint, done) => {
                let record = forget(directory);
                let typewhisper = promoting::forget(
                    directory,
                    endpoint.as_ref(),
                    at,
                    &mut passes,
                    promoting::FORGET_WAIT,
                );
                done(Forgot {
                    record,
                    typewhisper,
                });
            }
            _ if forgotten => {}
            Work::Watched(watched) => learn(directory, &watched, at),
            Work::History(history) => {
                append_to_log(directory, &[learning::log_history(&history, at)], at);
            }
            Work::Heard(heard) => promoting::note_used(directory, &heard, at),
            Work::Promote(endpoint) => {
                promoting::promote(directory, &endpoint, at, &mut passes, shared, made);
            }
        }));
        if ran.is_err() {
            log::error!("a job of dictation's learning stopped short");
        }
    }
}

/// Takes in what a watch saw: the record, then the log.
fn learn(directory: &Path, watched: &Watched, at: u64) {
    let seen = learning::observations(watched);
    if seen.is_empty() {
        return;
    }
    let today = learning::day_of(at);
    let mut record = match read_file(directory, learning::FILE, learning::MAX_BYTES) {
        Ok(bytes) => bytes.map_or_else(Record::default, |bytes| Record::parse(&bytes, today)),
        Err(e) => return log::warn!("dictation's learning record is not read: {e}"),
    };
    record.observe(&seen, today);
    let written = PrivateDir::open(directory).and_then(|private| {
        replace_file(&private, learning::FILE, learning::PART, &record.to_text())
    });
    if let Err(e) = written {
        return log::warn!("dictation's learning record is not written: {e}");
    }
    let lines: Vec<String> = seen
        .iter()
        .map(|observation| learning::log_observed(observation, at))
        .collect();
    append_to_log(directory, &lines, at);
}

/// Adds `lines` to the log at `at`, keeping its last lines of the last month.
pub(super) fn append_to_log(directory: &Path, lines: &[String], at: u64) {
    let old = match read_file(directory, learning::LOG, learning::MAX_BYTES) {
        Ok(bytes) => bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) => return log::warn!("dictation's learning log is not read: {e}"),
    };
    let text = learning::append_log(old.as_deref().unwrap_or(""), lines, at);
    let written = PrivateDir::open(directory)
        .and_then(|private| replace_unsynced(&private, learning::LOG, learning::LOG_PART, &text));
    if let Err(e) = written {
        log::warn!("dictation's learning log is not written: {e}");
    }
}

/// Replaces `name` in `directory` with `text` whole through `part`, as `replace_file` does
/// but with no flush to the disk: the log is for looking at, and a flush of the whole drive
/// for each of its lines would cost a dictation several.
fn replace_unsynced(
    directory: &PrivateDir,
    name: &str,
    part: &str,
    text: &str,
) -> std::io::Result<()> {
    use std::io::Write as _;
    let part = directory.path().join(part);
    match std::fs::remove_file(&part) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&part)?.write_all(text.as_bytes())?;
    std::fs::rename(&part, directory.path().join(name))
}

/// Removes the record and the log, and what a write left of them: whether none is there.
fn forget(directory: &Path) -> bool {
    let private = match PrivateDir::existing(directory) {
        Ok(Some(private)) => private,
        Ok(None) => return true,
        Err(e) => {
            log::warn!("dictation's learning is not forgotten: {e}");
            return false;
        }
    };
    forget_record(&private)
}

/// Removes the record and the log from `private`, and what a write left of them: whether none is
/// there.
pub(super) fn forget_record(private: &PrivateDir) -> bool {
    let names = [
        learning::FILE,
        learning::PART,
        learning::LOG,
        learning::LOG_PART,
    ];
    // Each is tried, whatever came of the one before.
    names.iter().fold(true, |all, name| {
        let gone = remove(private, name)
            .inspect_err(|e| log::warn!("dictation's learning is not forgotten: {e}"));
        all && gone.is_ok()
    })
}

/// Removes the file called `name` from `private`, if it is there.
pub(super) fn remove(private: &PrivateDir, name: &str) -> std::io::Result<()> {
    match std::fs::remove_file(private.path().join(name)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The history the record in `directory` gives a fix of `target` on the day of `at`: none if
/// there is no record, or it may not be read.
pub(super) fn history(directory: &Path, target: &str, at: u64) -> History {
    match read_file(directory, learning::FILE, learning::MAX_BYTES) {
        Ok(Some(bytes)) => Record::parse(&bytes, learning::day_of(at)).history(target),
        Ok(None) => History::default(),
        Err(e) => {
            log::warn!("dictation's learning record is not read: {e}");
            History::default()
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn private(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "oso-learning-{name}-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let _ = std::fs::remove_dir_all(&root);
        PrivateDir::open(&root).unwrap();
        root.join(learning::DIRECTORY)
    }

    fn watched(current: &str) -> Watched {
        Watched {
            typed: "リアクトとビート".into(),
            left: "Reactとビート".into(),
            fixed: vec![("リアクト".into(), "React".into())],
            current: current.into(),
            sent: true,
        }
    }

    #[test]
    fn a_watch_writes_the_record_and_the_log_privately_and_forget_removes_them() {
        use std::os::unix::fs::PermissionsExt;
        let directory = private("write");
        let at = 20_000 * 86_400;
        learn(&directory, &watched("ReactとVite"), at);
        let bytes = std::fs::read(directory.join(learning::FILE)).unwrap();
        let record = Record::parse(&bytes, learning::day_of(at));
        let pairs: Vec<(&str, &str, u32, u32)> = record
            .pairs
            .iter()
            .map(|pair| {
                (
                    pair.wrong.as_str(),
                    pair.right.as_str(),
                    pair.fixes,
                    pair.edits,
                )
            })
            .collect();
        assert_eq!(
            pairs,
            [("リアクト", "React", 1, 0), ("ビート", "Vite", 0, 1)]
        );
        for name in [learning::FILE, learning::LOG] {
            let mode = std::fs::metadata(directory.join(name))
                .unwrap()
                .permissions();
            assert_eq!(mode.mode() & 0o777, 0o600, "{name}");
        }
        let log = std::fs::read_to_string(directory.join(learning::LOG)).unwrap();
        assert_eq!(log.lines().count(), 2);
        let history = history(&directory, "ビートで", at);
        assert_eq!(history.accepted[0].0, "ビート", "in the text, first");
        assert!(forget(&directory));
        assert!(!directory.join(learning::FILE).exists());
        assert!(!directory.join(learning::LOG).exists());
        assert!(forget(&directory), "nothing to forget is forgotten");
        let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    }

    #[test]
    fn a_record_that_may_not_be_read_is_left_as_it_is() {
        let directory = private("refused");
        PrivateDir::open(&directory).unwrap();
        let target = directory.join("elsewhere");
        std::fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, directory.join(learning::FILE)).unwrap();
        learn(&directory, &watched("Reactとビート"), 86_400);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}");
        assert!(
            std::fs::symlink_metadata(directory.join(learning::FILE))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(history(&directory, "x", 86_400), History::default());
        let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    }
}
