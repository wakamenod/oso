//! The agent channel's `open` on the UI thread (ADR 0085): the file loads unshown, and is
//! shown, at its line, only once the loader has judged it ordinary and read it; one judged
//! otherwise is dropped unshown. The channel's server has checked that the path is
//! absolute, names no secret and leads to a file that is there.
//!
//! Must not depend on: sockets, threads or the loader's internals; it visits as the app
//! does, and watches the buffer.

use std::path::PathBuf;
use std::sync::mpsc;

use oso_core::buffer::{BufferId, Verdict};
use oso_core::command::Reach;
use oso_core::editor::Editor;

use crate::loader::LoadError;
use serde_json::json;

use super::super::App;
use crate::channel::{Connection, Reply};
use crate::dispatch::{Given, Unshowable, unshowable};

/// The most files the channel loads at once.
pub const MAX_OPENING: usize = 4;

/// The most files one connection opens.
pub const MAX_OPENED: usize = 100;

/// A file the channel asked for, until it is shown or dropped.
#[derive(Debug)]
pub(in crate::app) struct Opening {
    buffer: BufferId,
    connection: Connection,
    /// The file name asked for, which the file found must have.
    name: Option<std::ffi::OsString>,
    line: Option<usize>,
    reply: mpsc::Sender<Reply>,
    /// When it was asked for: a load that takes longer than the reply may wait is dropped,
    /// so that nothing shows after the agent was told it failed.
    asked: std::time::Instant,
}

/// How long a file the channel opens may take to show, under the server's wait for a reply.
const OPEN_WITHIN: std::time::Duration = std::time::Duration::from_secs(8);

impl App {
    /// Starts visiting `path` for `connection`, unshown; the reply goes once it is shown or
    /// dropped.
    pub(super) fn agent_open(
        &mut self,
        connection: Connection,
        path: PathBuf,
        line: Option<usize>,
        reply: mpsc::Sender<Reply>,
    ) {
        let refused = |why: &str| {
            let _ = reply.send(Err(why.to_owned()));
        };
        if let Some(why) = self.agent_waits() {
            return refused(why);
        }
        // A file a buffer already visits by this path is shown in it, without a load, if it
        // may be (ADR 0094). It counts toward no limit.
        let path = std::path::absolute(&path).unwrap_or(path);
        let open = self
            .editor
            .buffers()
            .find(|(_, buffer)| !buffer.agent_pending() && buffer.path() == Some(path.as_path()))
            .map(|(id, _)| id);
        if let Some(open) = open {
            let _ = reply.send(self.show_open_for_agent(open, line));
            return;
        }
        if self.agent.opening.len() >= MAX_OPENING {
            return refused("too many files are opening; wait for one");
        }
        let opened = self.agent.opened.entry(connection).or_default();
        if *opened >= MAX_OPENED {
            return refused("this agent has opened as many files as it may");
        }
        *opened += 1;
        let name = path.file_name().map(std::ffi::OsStr::to_owned);
        // It reads no file of 2 GiB or more, which the person must agree to (ADR 0087).
        let Some(buffer) = self.visit_with(path, crate::loader::UNASKED, true) else {
            return refused("the file could not start loading");
        };
        if let Some(buffer) = self.editor.buffer_mut(buffer) {
            buffer.mark_opened_by_agent();
            buffer.set_agent_pending(true);
        }
        self.agent.opening.push(Opening {
            buffer,
            connection,
            name,
            line,
            reply,
            asked: std::time::Instant::now(),
        });
        // Something must wake the loop when the deadline passes. The app's single wake
        // (`set_wake`) does not hold it: an event from this thread does.
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-agent-deadline".into())
            .spawn(move || {
                std::thread::sleep(OPEN_WITHIN);
                waker.send(super::super::UserEvent::AgentDeadline);
            });
        if let Err(e) = spawned {
            log::warn!("agent channel: cannot start the deadline thread: {e}");
        }
    }

    /// Shows each file the channel opened that is judged ordinary and read, and drops each
    /// one judged otherwise, or that failed to load. After every event.
    pub(super) fn progress_agent_opens(&mut self) {
        let mut index = 0;
        while index < self.agent.opening.len() {
            let opening = &self.agent.opening[index];
            let late = opening.asked.elapsed() >= OPEN_WITHIN;
            let next = next(&self.editor, opening.buffer, opening.name.as_deref(), late);
            if next == Next::Wait {
                index += 1;
                continue;
            }
            let opening = self.agent.opening.swap_remove(index);
            let reply = match next {
                Next::Wait => continue,
                Next::Gone => {
                    self.agent_log("open: failed");
                    Err("the file could not be opened".into())
                }
                Next::Refuse(why) => {
                    self.drop_agents_file(opening.buffer);
                    self.agent_log("open: refused");
                    Err(why.into())
                }
                Next::Reuse(open) => {
                    self.drop_agents_file(opening.buffer);
                    if let Some(opened) = self.agent.opened.get_mut(&opening.connection) {
                        *opened = opened.saturating_sub(1);
                    }
                    self.show_open_for_agent(open, opening.line)
                }
                Next::Show => self.show_agents_file(&opening),
            };
            let _ = opening.reply.send(reply);
        }
    }

    /// A load the channel started for `buffer` failed with `e`: the agent hears why, at once
    /// and in words of its own (ADR 0085). Returns whether it was the channel's.
    pub(in crate::app) fn agent_load_failed(&mut self, buffer: BufferId, e: &LoadError) -> bool {
        self.refuse_agent_open(buffer, load_failed(e))
    }

    /// Refuses the channel's open of `buffer` with `why`, if the channel is opening it: its
    /// load stopped before the file could be shown, and the agent hears why at once, not when
    /// its time is up. Returns whether it was opening it.
    pub(in crate::app) fn refuse_agent_open(&mut self, buffer: BufferId, why: &str) -> bool {
        let Some(opening) = take_opening(&mut self.agent.opening, buffer) else {
            return false;
        };
        let _ = opening.reply.send(Err(why.to_owned()));
        self.agent_log("open: refused");
        true
    }

    /// Shows `open`, a buffer that already holds the file the agent asked for, at `line`, if
    /// it may be shown: judged ordinary, read, and the person not in the middle of something.
    /// Its mark stays as it is: a buffer the person visited stays theirs (ADR 0094).
    fn show_open_for_agent(&mut self, open: BufferId, line: Option<usize>) -> Reply {
        let refused = match self.editor.buffer(open) {
            None => Some("the file could not be opened"),
            Some(buffer) => reusable(buffer),
        };
        if let Some(why) = refused.or_else(|| self.agent_waits()) {
            self.agent_log("open: refused");
            return Err(why.into());
        }
        self.show_for_agent(open, line)
    }

    /// Shows a file the channel opened, which is judged ordinary and read, and goes to its
    /// line, unless the person is in the middle of something: then it is dropped.
    fn show_agents_file(&mut self, opening: &Opening) -> Reply {
        if let Some(why) = self.agent_waits() {
            self.drop_agents_file(opening.buffer);
            self.agent_log("open: dropped, the person was busy");
            return Err(why.into());
        }
        if let Some(buffer) = self.editor.buffer_mut(opening.buffer) {
            buffer.set_agent_pending(false);
        }
        self.show_for_agent(opening.buffer, opening.line)
    }

    /// Shows `buffer` for the agent, at `line` if given, and replies with it. `goto-line`
    /// leaves one cursor, as it does for the person, so in a buffer the person already had open
    /// it ends their several cursors there.
    fn show_for_agent(&mut self, buffer: BufferId, line: Option<usize>) -> Reply {
        let before = self.editor.view_sample();
        self.editor.show(buffer);
        if let Some(line) = line {
            let line = isize::try_from(line).unwrap_or(isize::MAX);
            let _ = self.dispatcher.run_for_agent(
                &mut self.editor,
                "goto-line",
                Given::Count(line),
                Reach::Look,
            );
        }
        // Showing the buffer is no input of the dispatcher's, so it is compared here.
        let jumped = self.dispatcher.take_view_jumped() == Some(true)
            || self.editor.flashes(&before, &self.editor.view_sample());
        self.flash_after_agent(jumped);
        self.request_redraw();
        let name = self.editor.buffer(buffer).map(|b| b.name().to_owned());
        self.agent_log(&format!("open {}: ok", name.as_deref().unwrap_or("")));
        Ok(json!({ "buffer": buffer.get(), "name": name }))
    }

    /// Drops what `connection` was opening, or everything if `None`, as it or the channel
    /// went.
    pub(super) fn drop_agent_opens(&mut self, connection: Option<Connection>) {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.opening)
            .into_iter()
            .partition(|opening| connection.is_none_or(|c| c == opening.connection));
        self.agent.opening = kept;
        for opening in gone {
            self.drop_agents_file(opening.buffer);
        }
    }
}

impl App {
    /// Drops a file the channel opened, before it shows: its buffer, and its load, whose
    /// thread then stops reading, so that a read that hangs holds nothing.
    fn drop_agents_file(&mut self, buffer: BufferId) {
        self.cancel_load(buffer);
        self.editor.kill_buffer(buffer);
    }
}

/// What the agent hears of a file of 2 GiB or more, which only the person may agree to open
/// (ADR 0087).
pub(in crate::app) const TOO_LARGE_FOR_THE_AGENT: &str =
    "the file is 2 GiB or more; ask the person to open it";

/// What the agent hears of a load the channel started that failed with `e`.
fn load_failed(e: &LoadError) -> &'static str {
    match e {
        LoadError::TooLargeForTheAgent => {
            "the file is larger than the agent may open; ask the person"
        }
        LoadError::SeveralNames => "the file has more than one name; ask the person",
        LoadError::TooLarge(_) => TOO_LARGE_FOR_THE_AGENT,
        _ => "the file could not be opened",
    }
}

/// Takes the open of `buffer` out of `openings`, if one is.
fn take_opening(openings: &mut Vec<Opening>, buffer: BufferId) -> Option<Opening> {
    let index = openings
        .iter()
        .position(|opening| opening.buffer == buffer)?;
    Some(openings.swap_remove(index))
}

/// What comes next of an open the channel started, as its buffer stands now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// It is not judged, or not read, yet.
    Wait,
    /// Its buffer has gone, as when its load failed.
    Gone,
    /// It goes, and the agent hears why.
    Refuse(&'static str),
    /// It goes, and another buffer that holds that very file is shown, if it may be.
    Reuse(BufferId),
    /// It is shown.
    Show,
}

/// What comes next of the open of `buffer`, for the file name `asked`, `late` if its time is
/// up. Past the deadline it goes, whatever it has come to: the agent was told so.
fn next(editor: &Editor, buffer: BufferId, asked: Option<&std::ffi::OsStr>, late: bool) -> Next {
    let Some(opened) = editor.buffer(buffer) else {
        return Next::Gone;
    };
    let waiting = opened.verdict() == Verdict::Unjudged
        || (!opened.is_secret() && opened.loading().is_some());
    if late {
        return Next::Refuse("the file took too long to load");
    }
    if waiting {
        return Next::Wait;
    }
    if opened.is_secret() {
        return Next::Refuse("the file may hold secrets, so it stays closed; ask the person");
    }
    if let Some(why) = found_elsewhere(opened, asked) {
        return Next::Refuse(why);
    }
    match held_elsewhere(editor, buffer) {
        Some(Held::Same(open)) => Next::Reuse(open),
        Some(Held::Stale) => Next::Refuse(
            "the file is open in a buffer that read it at another time; ask the person",
        ),
        None => Next::Show,
    }
}

/// Why a buffer that already holds the file the agent asked for may not be shown for it, if
/// it may not (ADR 0094).
fn reusable(buffer: &oso_core::buffer::Buffer) -> Option<&'static str> {
    Some(match unshowable(buffer)? {
        Unshowable::Secret => "the file is open in a buffer that may hold secrets; ask the person",
        Unshowable::Opening => "the file is still opening for the agent; try again",
        Unshowable::Loading => "the file is open and still loading; try again",
        Unshowable::OwnKeys => "the file is open in a buffer whose keys are the person's",
    })
}

/// What another buffer holds of the file `id`'s load opened (ADR 0087, ADR 0094).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// That very file, by its device and inode, read as it is now: this buffer shows it.
    Same(BufferId),
    /// A file of the same device and inode, read at another time or size: a file since
    /// replaced in that inode, or one that changed since, which cannot be told apart.
    Stale,
}

/// The buffer, other than `id` and one the channel is still opening, that holds the file
/// `id`'s load opened, if one does. Only by device and inode: time and size alone tell no file
/// apart.
fn held_elsewhere(editor: &Editor, id: BufferId) -> Option<Held> {
    let opened = editor.buffer(id)?.baseline()?.file?;
    opened.identity?;
    let (other, held) = editor.buffers().find_map(|(other, buffer)| {
        let held = buffer.baseline()?.file?;
        (other != id && !buffer.agent_pending() && held.identity == opened.identity)
            .then_some((other, held))
    })?;
    Some(if held.same_content(opened) {
        Held::Same(other)
    } else {
        Held::Stale
    })
}

/// Why a file the channel opened and read is not the one it asked for, if it is not: the
/// load found no file there any more, so it would be a new one, which the channel never
/// starts, or the path led, through a link, to a file of another name.
fn found_elsewhere(
    buffer: &oso_core::buffer::Buffer,
    asked: Option<&std::ffi::OsStr>,
) -> Option<&'static str> {
    let Some(baseline) = buffer.baseline() else {
        return Some("where the path leads could not be told");
    };
    if baseline.file.is_none() {
        return Some("no file is at that path now");
    }
    (baseline.location.file_name() != asked).then_some("the path leads to a file of another name")
}

#[cfg(test)]
mod tests {
    use super::{
        Held, Next, Opening, TOO_LARGE_FOR_THE_AGENT, found_elsewhere, held_elsewhere, load_failed,
        next, reusable, take_opening,
    };
    use crate::channel::Connection;
    use crate::loader::LoadError;
    use oso_core::buffer::{Baseline, Buffer, FileStamp};
    use oso_core::display_map::TabWidth;
    use oso_core::editor::Editor;
    use oso_core::load::decode_all;
    use std::ffi::OsStr;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    /// The stamp of a file of `len` bytes, modified at `at` seconds, with `identity`.
    fn stamp(identity: Option<(u64, u64)>, len: u64, at: u64) -> FileStamp {
        FileStamp {
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(at),
            len,
            mode: 0o644,
            identity,
        }
    }

    /// A loaded buffer visiting `path`, judged ordinary, whose file has `file` as its stamp.
    fn visited(path: &str, file: FileStamp) -> Buffer {
        let path = PathBuf::from(path);
        let mut buffer = Buffer::visiting(path.clone());
        buffer.judge_secret(Some(std::slice::from_ref(&path)));
        buffer.apply_load(decode_all(b"x\n", TabWidth::default()));
        buffer.set_baseline(Baseline {
            location: path,
            file: Some(file),
        });
        buffer
    }

    /// `buffer`, as one the channel is still opening.
    fn pending(mut buffer: Buffer) -> Buffer {
        buffer.set_agent_pending(true);
        buffer
    }

    #[test]
    fn the_buffer_holding_the_same_file_is_found_by_inode_and_stamp() {
        let file = stamp(Some((1, 7)), 2, 10);
        let mut editor = Editor::default();
        let persons = editor.add_buffer(visited("/work/notes.md", file));
        editor.add_buffer(visited("/work/other.md", stamp(Some((1, 8)), 2, 10)));
        let agents = editor.add_buffer(pending(visited("/link/notes.md", file)));
        assert_eq!(held_elsewhere(&editor, agents), Some(Held::Same(persons)));
        // The same inode, read at another time: a file since put in that inode, or one that
        // changed, which cannot be told apart.
        let later = editor.add_buffer(pending(visited("/b.md", stamp(Some((1, 7)), 9, 99))));
        assert_eq!(held_elsewhere(&editor, later), Some(Held::Stale));
        // With no identity, as where the system gives none, nothing is the same file.
        let none = editor.add_buffer(pending(visited("/x/notes.md", stamp(None, 2, 10))));
        assert_eq!(held_elsewhere(&editor, none), None);
    }

    #[test]
    fn a_buffer_the_channel_is_still_opening_is_never_the_one_found() {
        let file = stamp(Some((1, 7)), 2, 10);
        let mut editor = Editor::default();
        editor.add_buffer(pending(visited("/a/notes.md", file)));
        let second = editor.add_buffer(pending(visited("/b/notes.md", file)));
        assert_eq!(held_elsewhere(&editor, second), None);
    }

    #[test]
    fn a_buffer_open_may_be_shown_for_the_agent_only_as_switching_may_show_it() {
        let file = stamp(Some((1, 7)), 2, 10);
        assert_eq!(reusable(&visited("/work/notes.md", file)), None);
        let unjudged = Buffer::visiting(PathBuf::from("/work/big.log"));
        assert!(reusable(&unjudged).is_some_and(|why| why.contains("may hold secrets")));
        let mut loading = Buffer::visiting(PathBuf::from("/work/big.log"));
        loading.judge_secret(Some(&[PathBuf::from("/work/big.log")]));
        assert!(reusable(&loading).is_some_and(|why| why.contains("still loading")));
        let opening = pending(visited("/work/notes.md", file));
        assert!(reusable(&opening).is_some_and(|why| why.contains("still opening")));
        let mut env = Buffer::visiting(PathBuf::from("/work/secret.key"));
        env.judge_secret(Some(&[PathBuf::from("/work/secret.key")]));
        assert_eq!(
            reusable(&env),
            Some("the file is open in a buffer that may hold secrets; ask the person")
        );
    }

    #[test]
    fn an_open_waits_until_judged_and_read_and_then_reuses_shows_or_refuses() {
        let file = stamp(Some((1, 7)), 2, 10);
        let name = Some(OsStr::new("notes.md"));
        let mut editor = Editor::default();
        let gone = editor.add_buffer(Buffer::from_text("gone", ""));
        editor.kill_buffer(gone);
        assert_eq!(next(&editor, gone, name, false), Next::Gone);
        let unjudged = editor.add_buffer(pending(Buffer::visiting(PathBuf::from("/a.md"))));
        assert_eq!(next(&editor, unjudged, name, false), Next::Wait);
        assert!(matches!(
            next(&editor, unjudged, name, true),
            Next::Refuse(why) if why.contains("too long")
        ));
        let own = editor.add_buffer(pending(visited(
            "/own/notes.md",
            stamp(Some((2, 1)), 2, 10),
        )));
        assert_eq!(next(&editor, own, name, false), Next::Show);
        assert!(matches!(
            next(&editor, own, Some(OsStr::new("other.md")), false),
            Next::Refuse(why) if why.contains("another name")
        ));
        let persons = editor.add_buffer(visited("/work/notes.md", file));
        let again = editor.add_buffer(pending(visited("/link/notes.md", file)));
        assert_eq!(next(&editor, again, name, false), Next::Reuse(persons));
        let stale = editor.add_buffer(pending(visited("/b/notes.md", stamp(Some((1, 7)), 5, 50))));
        assert!(matches!(
            next(&editor, stale, name, false),
            Next::Refuse(why) if why.contains("at another time")
        ));
        let mut secret = pending(Buffer::visiting(PathBuf::from("/s/notes.md")));
        secret.judge_secret(Some(&[PathBuf::from("/s/.env")]));
        let secret = editor.add_buffer(secret);
        assert!(matches!(
            next(&editor, secret, name, false),
            Next::Refuse(why) if why.contains("may hold secrets")
        ));
    }

    #[test]
    fn a_load_that_stops_tells_the_agent_why_at_once() {
        assert_eq!(
            load_failed(&LoadError::TooLarge(3 << 30)),
            TOO_LARGE_FOR_THE_AGENT
        );
        assert_eq!(
            TOO_LARGE_FOR_THE_AGENT,
            "the file is 2 GiB or more; ask the person to open it"
        );
        assert!(load_failed(&LoadError::TooLargeForTheAgent).contains("larger than the agent"));
        // Only Unix counts a file's names.
        #[cfg(unix)]
        assert!(load_failed(&LoadError::SeveralNames).contains("more than one name"));
        assert_eq!(
            load_failed(&LoadError::NotFound),
            "the file could not be opened"
        );
        // The open is taken out at once, so no later "took too long" reaches the agent.
        let (reply, replies) = std::sync::mpsc::channel();
        let mut editor = Editor::default();
        let buffer = editor.add_buffer(Buffer::from_text("big.iso", ""));
        let mut openings = vec![Opening {
            buffer,
            connection: Connection { run: 1, number: 1 },
            name: None,
            line: None,
            reply,
            asked: std::time::Instant::now(),
        }];
        let opening = take_opening(&mut openings, buffer).unwrap();
        assert!(openings.is_empty());
        assert!(take_opening(&mut openings, buffer).is_none());
        let _ = opening.reply.send(Err(TOO_LARGE_FOR_THE_AGENT.into()));
        assert_eq!(
            replies.try_recv().unwrap(),
            Err(TOO_LARGE_FOR_THE_AGENT.into())
        );
    }

    #[test]
    fn a_buffer_with_no_file_found_is_not_the_one_asked_for() {
        let buffer = Buffer::from_text("notes.md", "");
        assert!(found_elsewhere(&buffer, Some(OsStr::new("notes.md"))).is_some());
    }
}
