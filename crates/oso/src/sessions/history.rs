//! Searching Claude Code's past conversations for `session-search` (ADR 0145): a thread of its
//! own, `oso-history`, holds the prompts and replies of every transcript in memory, read on up
//! to `min(cores - 1, 8)` reading threads, and searches them for each input; it also gives the
//! preview's text around a row.
//!
//! Must not depend on: `app`, winit or the UI thread; this reads the file system, and the caller
//! supplies the wake-up.
//!
//! - What is held is never written anywhere, logged or sent but as rows and previews. It goes
//!   [`KEEP_FOR`] after the last search, and the thread then waits with no timer.
//! - The transcripts are listed again for a search once the last listing is [`LIST_EVERY`]
//!   old; a file that changed is read again from where it grew (`held`).
//! - Conversations are searched newest first, as they are read; a search's generation is read
//!   before each, and a superseded one sends nothing more. A batch goes at most every
//!   [`BATCH_EVERY`], of at most [`MAX_BATCH`] rows; at most [`PER_CONVERSATION`] rows a
//!   conversation, and [`MAX_ROWS`] in all.
//! - At most [`MAX_HELD`] bytes of text are held, newest first; a search past them says it was
//!   cut.
//! - Nothing here logs what is searched for or found; `Debug` gives a generation and a count.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::editor::{
    ConversationPreviewRequest, ConversationSearchRequest, Excerpt, ProjectPreviewRead, WalkEnd,
};
use oso_core::project::{LineQuery, MAX_ROWS, row_text};

pub use held::{About, Message};
use held::{Conversation, Listed};

mod held;
mod scan;

/// How long what was read is held with no search coming.
pub const KEEP_FOR: Duration = Duration::from_secs(5 * 60);

/// How old a listing of the transcripts may be before a search lists them again.
pub const LIST_EVERY: Duration = Duration::from_secs(2);

/// How often a search sends the rows it found, as `project-search`'s does.
pub const BATCH_EVERY: Duration = crate::grep::BATCH_EVERY;

/// The most rows one batch takes to the editor.
pub const MAX_BATCH: usize = crate::grep::MAX_BATCH;

/// The most rows of one conversation.
pub const PER_CONVERSATION: usize = 50;

/// The most bytes of text held in all.
pub const MAX_HELD: usize = 64 * 1024 * 1024;

/// How many messages before the row's the preview shows at most.
const PREVIEW_BEFORE: usize = 3;

/// How many messages after the row's the preview shows at most.
const PREVIEW_AFTER: usize = 4;

/// A message a search found, and the conversation it is of.
#[derive(Clone)]
pub struct Hit {
    pub about: Arc<About>,
    /// Its index in the conversation's messages held.
    pub message: usize,
    pub prompt: bool,
    /// The line of its first match in the message, from 0, the match's column in characters,
    /// and the line's text around it.
    pub line: usize,
    pub column: usize,
    pub text: Box<str>,
}

/// A batch of rows a search found, and whether it ended. `Debug` gives its generation and size
/// only.
pub struct Found {
    pub generation: u64,
    pub hits: Vec<Hit>,
    pub end: Option<WalkEnd>,
}

impl fmt::Debug for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Found")
            .field("generation", &self.generation)
            .field("hits", &self.hits.len())
            .field("end", &self.end)
            .finish()
    }
}

/// The text a preview asked for. `Debug` gives its generation only.
pub struct Previewed {
    pub generation: u64,
    pub read: ProjectPreviewRead,
}

impl fmt::Debug for Previewed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Previewed")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// What the thread reports.
#[derive(Debug)]
pub enum Report {
    Found(Found),
    Previewed(Previewed),
}

enum Job {
    Search(ConversationSearchRequest),
    Preview(ConversationPreviewRequest),
}

/// The searcher: the thread starts with the first job, and ends when this is dropped.
pub struct History {
    projects: Option<PathBuf>,
    send: Arc<dyn Fn(Report) + Send + Sync>,
    jobs: Option<Sender<Job>>,
}

impl fmt::Debug for History {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("History")
            .field("started", &self.jobs.is_some())
            .finish_non_exhaustive()
    }
}

impl History {
    /// A searcher of the transcripts under `projects`, Claude Code's `projects` directory, if
    /// known, which reports through `send`.
    pub fn new(projects: Option<PathBuf>, send: impl Fn(Report) + Send + Sync + 'static) -> Self {
        Self {
            projects,
            send: Arc::new(send),
            jobs: None,
        }
    }

    /// Starts `request`; the search before it stops at its next conversation.
    pub fn search(&mut self, request: ConversationSearchRequest) {
        self.give(Job::Search(request));
    }

    /// Makes the preview's text `request` asks for.
    pub fn preview(&mut self, request: ConversationPreviewRequest) {
        self.give(Job::Preview(request));
    }

    fn give(&mut self, job: Job) {
        let job = match &self.jobs {
            Some(jobs) => match jobs.send(job) {
                Ok(()) => return,
                // The thread ended, as it panicked: a new one starts afresh.
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        self.jobs = None;
        self.give_to_new(job);
    }

    fn give_to_new(&mut self, job: Job) {
        let (jobs, queue) = mpsc::channel();
        let projects = self.projects.clone();
        let send = Arc::clone(&self.send);
        let started = thread::Builder::new()
            .name("oso-history".into())
            .spawn(move || run(projects.as_deref(), &queue, &*send));
        match started {
            Ok(_) => {
                let _ = jobs.send(job);
                self.jobs = Some(jobs);
            }
            Err(error) => {
                log::warn!("session-search: could not start its thread: {error}");
                if let Job::Search(request) = job {
                    (self.send)(Report::Found(Found {
                        generation: request.generation,
                        hits: Vec::new(),
                        end: Some(WalkEnd { cut: true }),
                    }));
                }
            }
        }
    }
}

/// What the thread holds.
#[derive(Default)]
struct Hold {
    /// The conversations, newest first.
    conversations: Vec<Conversation>,
    /// When the transcripts were listed last.
    listed: Option<Instant>,
    /// Conversations left out as more text than [`MAX_HELD`] was held before them.
    left_out: usize,
}

fn run(projects: Option<&Path>, queue: &Receiver<Job>, send: &(dyn Fn(Report) + Sync)) {
    let mut hold: Option<Hold> = None;
    loop {
        let job = if hold.is_some() {
            match queue.recv_timeout(KEEP_FOR) {
                Ok(job) => job,
                Err(RecvTimeoutError::Timeout) => {
                    hold = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match queue.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
        // Only the newest search and the newest preview matter.
        let (mut search, mut preview) = (None, None);
        for job in std::iter::once(job).chain(queue.try_iter()) {
            match job {
                Job::Search(request) => search = Some(request),
                Job::Preview(request) => preview = Some(request),
            }
        }
        let hold = hold.get_or_insert_with(Hold::default);
        if let Some(request) = search {
            run_search(projects, hold, &request, send);
        }
        if let Some(request) = preview
            && live(&request.live, request.generation)
        {
            let read = preview_of(hold, &request);
            send(Report::Previewed(Previewed {
                generation: request.generation,
                read,
            }));
        }
    }
}

fn live(live: &std::sync::atomic::AtomicU64, generation: u64) -> bool {
    live.load(Ordering::Relaxed) == generation
}

/// What a search sends, in batches.
struct Sending<'a> {
    request: &'a ConversationSearchRequest,
    send: &'a (dyn Fn(Report) + Sync),
    batch: Vec<Hit>,
    sent: Instant,
    rows: usize,
    cut: bool,
}

impl Sending<'_> {
    fn live(&self) -> bool {
        live(&self.request.live, self.request.generation) && !self.cut
    }

    /// Searches `conversation`, unless the search is superseded or full.
    fn search(&mut self, conversation: &Conversation) {
        if !self.live() {
            return;
        }
        let about = &conversation.about;
        if !self
            .request
            .scope
            .holds(about.id.as_str(), &about.directories)
        {
            return;
        }
        let mut about = None;
        let mut here = 0;
        for (index, message) in conversation.messages.iter().enumerate() {
            if here == PER_CONVERSATION {
                break;
            }
            let Some(hit) = message_hit(&self.request.query, message) else {
                continue;
            };
            if self.rows == MAX_ROWS {
                self.cut = true;
                break;
            }
            let about = about.get_or_insert_with(|| Arc::new(conversation.about.clone()));
            self.batch.push(Hit {
                about: Arc::clone(about),
                message: index,
                prompt: message.prompt,
                line: hit.0,
                column: hit.1,
                text: hit.2,
            });
            here += 1;
            self.rows += 1;
            if self.batch.len() == MAX_BATCH {
                self.flush(None);
            }
        }
        if self.sent.elapsed() >= BATCH_EVERY && !self.batch.is_empty() {
            self.flush(None);
        }
    }

    fn flush(&mut self, end: Option<WalkEnd>) {
        if !live(&self.request.live, self.request.generation) {
            self.batch.clear();
            return;
        }
        (self.send)(Report::Found(Found {
            generation: self.request.generation,
            hits: std::mem::take(&mut self.batch),
            end,
        }));
        self.sent = Instant::now();
    }
}

/// Searches `hold` for `request`, listing and reading the transcripts first if the last
/// listing is old, and searching each conversation as it is read.
fn run_search(
    projects: Option<&Path>,
    hold: &mut Hold,
    request: &ConversationSearchRequest,
    send: &(dyn Fn(Report) + Sync),
) {
    if !live(&request.live, request.generation) {
        return;
    }
    let mut sending = Sending {
        request,
        send,
        batch: Vec::new(),
        sent: Instant::now(),
        rows: 0,
        cut: false,
    };
    let stale = hold.listed.is_none_or(|at| at.elapsed() >= LIST_EVERY);
    match projects {
        Some(projects) if stale => refresh(projects, hold, |conversation| {
            sending.search(conversation);
        }),
        _ => {
            for conversation in &hold.conversations {
                if !sending.live() {
                    break;
                }
                sending.search(conversation);
            }
        }
    }
    let cut = sending.cut || hold.left_out > 0;
    sending.flush(Some(WalkEnd { cut }));
}

/// Where `message` matches `query`: the line of its first match, from 0, the match's column
/// in characters, and the line's text around it, as a row shows it.
fn message_hit(query: &LineQuery, message: &Message) -> Option<(usize, usize, Box<str>)> {
    let text = message.text.as_bytes();
    let first = query.matches_message(text)?;
    let start = text[..first.start]
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |at| at + 1);
    let end = text[first.start..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(text.len(), |at| first.start + at);
    let line = text[..start].iter().filter(|&&byte| byte == b'\n').count();
    let within = first.start - start..first.end.min(end) - start;
    let (shown, column) = row_text(&text[start..end], within);
    Some((line, column, shown))
}

/// A transcript to read: its place in the listing, its file, and what was held of it.
type ToRead = (usize, Listed, Option<Conversation>);

/// Lists the transcripts under `projects` into `hold`, newest first, reading those that
/// changed on the reading threads, and hands each conversation to `each` in that order as it
/// is ready.
fn refresh(projects: &Path, hold: &mut Hold, mut each: impl FnMut(&Conversation)) {
    let listed = held::list(projects);
    let mut before: HashMap<PathBuf, Conversation> = std::mem::take(&mut hold.conversations)
        .into_iter()
        .map(|conversation| (conversation.path.clone(), conversation))
        .collect();
    // What needs reading, by its place in the listing, with what was held of it.
    let mut ready: Vec<Option<Conversation>> = Vec::with_capacity(listed.len());
    let mut reads: Vec<ToRead> = Vec::new();
    for (at, listed) in listed.iter().enumerate() {
        match before.remove(&listed.path) {
            Some(conversation) if conversation.current(listed) => ready.push(Some(conversation)),
            held => {
                ready.push(None);
                reads.push((at, listed.clone(), held));
            }
        }
    }
    let slots: Vec<Mutex<Option<ToRead>>> = reads
        .into_iter()
        .map(|read| Mutex::new(Some(read)))
        .collect();
    let next = AtomicUsize::new(0);
    let readers = thread::available_parallelism()
        .map_or(1, |count| count.get().saturating_sub(1))
        .clamp(1, 8)
        .min(slots.len().max(1));
    let (done, results) = mpsc::channel::<(usize, Option<Conversation>)>();
    let mut kept = Vec::with_capacity(listed.len());
    let mut bytes = 0;
    hold.left_out = 0;
    thread::scope(|scope| {
        for _ in 0..readers {
            let done = done.clone();
            let (slots, next) = (&slots, &next);
            let _ = thread::Builder::new()
                .name("oso-history-read".into())
                .spawn_scoped(scope, move || {
                    while let Some(slot) = slots.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let taken = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
                        let Some((at, listed, held)) = taken else {
                            continue;
                        };
                        if done.send((at, held::read(&listed, held))).is_err() {
                            return;
                        }
                    }
                });
        }
        drop(done);
        let mut waiting: HashMap<usize, Option<Conversation>> = HashMap::new();
        for (at, slot) in ready.iter_mut().enumerate() {
            let conversation = match slot.take() {
                Some(conversation) => Some(conversation),
                None => loop {
                    if let Some(read) = waiting.remove(&at) {
                        break read;
                    }
                    match results.recv() {
                        Ok((got, read)) if got == at => break read,
                        Ok((got, read)) => {
                            waiting.insert(got, read);
                        }
                        // A reader that could not start or ended: what it had is not read.
                        Err(_) => break None,
                    }
                },
            };
            let Some(conversation) = conversation else {
                continue;
            };
            if bytes + conversation.bytes > MAX_HELD {
                hold.left_out += 1;
                continue;
            }
            bytes += conversation.bytes;
            each(&conversation);
            kept.push(conversation);
        }
    });
    hold.conversations = kept;
    hold.listed = Some(Instant::now());
}

/// The preview's text for `request`, from what `hold` holds.
fn preview_of(hold: &Hold, request: &ConversationPreviewRequest) -> ProjectPreviewRead {
    let Some(conversation) = hold
        .conversations
        .iter()
        .find(|conversation| conversation.about.id.as_str() == &*request.session)
    else {
        return ProjectPreviewRead::Unreadable;
    };
    let (text, line, column) = preview_text(&conversation.messages, request);
    ProjectPreviewRead::Text(Excerpt::wrapped(&text, request.columns, (line, column)))
}

/// The messages around `request`'s, each starting with `› ` for a prompt or `‹ ` for a reply,
/// its other lines indented to match and a blank line between, and the line and column the
/// row's match is at in it.
fn preview_text(
    messages: &[Message],
    request: &ConversationPreviewRequest,
) -> (String, usize, usize) {
    let hit = request.message.min(messages.len().saturating_sub(1));
    let rows = request.rows.max(4);
    let mut text = String::new();
    let mut lines = 0;
    let (mut at_line, mut at_column) = (0, 0);
    let first = hit.saturating_sub(PREVIEW_BEFORE);
    let last = (hit + PREVIEW_AFTER).min(messages.len());
    for (index, message) in messages.iter().enumerate().take(last).skip(first) {
        let all: Vec<&str> = message.text.split('\n').collect();
        // Before the row's message, only its last lines, and of the row's, the lines before
        // its match up to half the preview.
        let skip = if index < hit {
            all.len().saturating_sub(rows / 2)
        } else if index == hit {
            request.line.saturating_sub(rows / 2)
        } else {
            0
        };
        if !text.is_empty() {
            text.push_str("\n\n");
            lines += 2;
        }
        let mark = if message.prompt { "› " } else { "‹ " };
        if skip > 0 {
            text.push_str(mark);
            text.push('…');
            text.push('\n');
            lines += 1;
        }
        for (number, part) in all.iter().enumerate().skip(skip) {
            if number > skip || skip > 0 {
                text.push_str("  ");
            } else {
                text.push_str(mark);
            }
            if index == hit && number == request.line {
                (at_line, at_column) = (lines, request.column + 2);
            }
            text.push_str(part);
            text.push('\n');
            lines += 1;
        }
        if text.ends_with('\n') {
            text.pop();
            lines -= 1;
        }
    }
    (text, at_line, at_column)
}

/// The place a row names for a conversation that ran in `directory`: `repository/worktree` for
/// one under `<repository>/.claude/worktrees/`, else the directory's last name.
pub fn place(directory: &Path) -> String {
    let parts: Vec<&std::ffi::OsStr> = directory
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let worktree = parts
        .windows(4)
        .rev()
        .find(|window| window[1] == ".claude" && window[2] == "worktrees");
    match worktree {
        Some(window) => format!(
            "{}/{}",
            window[0].to_string_lossy(),
            window[3].to_string_lossy()
        ),
        None => parts.last().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        ),
    }
}

#[cfg(all(test, unix))]
mod tests;
