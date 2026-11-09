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
use crate::dispatch::Given;

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
        // Something must wake the loop when the deadline passes, as nothing else may.
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
            let Some(buffer) = self.editor.buffer(opening.buffer) else {
                let opening = self.agent.opening.swap_remove(index);
                let _ = opening
                    .reply
                    .send(Err("the file could not be opened".into()));
                self.agent_log("open: failed");
                continue;
            };
            let waiting = buffer.verdict() == Verdict::Unjudged
                || (!buffer.is_secret() && buffer.loading().is_some());
            let late = opening.asked.elapsed() >= OPEN_WITHIN;
            if waiting && !late {
                index += 1;
                continue;
            }
            // Past the deadline it goes, whatever it has come to: the agent was told so.
            let refused = if late {
                Some("the file took too long to load")
            } else if buffer.is_secret() {
                Some("the file may hold secrets, so it stays closed; ask the person")
            } else {
                found_elsewhere(buffer, opening.name.as_deref())
            };
            // Another buffer may hold that very file, by another spelling of its path: that
            // one is shown instead, if it may be, and this load goes (ADR 0094).
            let open = same_file_open(&self.editor, opening.buffer);
            let opening = self.agent.opening.swap_remove(index);
            if let Some(why) = refused {
                self.drop_agents_file(opening.buffer);
                let _ = opening.reply.send(Err(why.into()));
                self.agent_log("open: refused");
                continue;
            }
            let reply = match open {
                Some(open) => {
                    self.drop_agents_file(opening.buffer);
                    if let Some(opened) = self.agent.opened.get_mut(&opening.connection) {
                        *opened = opened.saturating_sub(1);
                    }
                    self.show_open_for_agent(open, opening.line)
                }
                None => self.show_agents_file(&opening),
            };
            let _ = opening.reply.send(reply);
        }
    }

    /// A load the channel started for `buffer` failed with `e`: the agent hears why, in words
    /// of its own (ADR 0085).
    pub(in crate::app) fn agent_load_failed(&mut self, buffer: BufferId, e: &LoadError) {
        let why = match e {
            LoadError::TooLargeForTheAgent => {
                "the file is larger than the agent may open; ask the person"
            }
            LoadError::SeveralNames => "the file has more than one name; ask the person",
            _ => "the file could not be opened",
        };
        self.refuse_agent_open(buffer, why);
    }

    /// Refuses the channel's open of `buffer` with `why`, if the channel is opening it: its
    /// load stopped before the file could be shown, and the agent hears why.
    pub(in crate::app) fn refuse_agent_open(&mut self, buffer: BufferId, why: &str) {
        let Some(index) = self.agent.opening.iter().position(|o| o.buffer == buffer) else {
            return;
        };
        let opening = self.agent.opening.swap_remove(index);
        let _ = opening.reply.send(Err(why.to_owned()));
        self.agent_log("open: refused");
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

    /// Shows `buffer` for the agent, at `line` if given, and replies with it.
    fn show_for_agent(&mut self, buffer: BufferId, line: Option<usize>) -> Reply {
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

/// Why a buffer that already holds the file the agent asked for may not be shown for it, if
/// it may not (ADR 0094).
fn reusable(buffer: &oso_core::buffer::Buffer) -> Option<&'static str> {
    if buffer.is_secret() {
        return Some("the file is open in a buffer that may hold secrets; ask the person");
    }
    if buffer.loading().is_some() {
        return Some("the file is open and still loading; try again");
    }
    None
}

/// The buffer, other than `id` and one the channel is still opening, that holds the very file
/// `id`'s load opened, by its device and inode (ADR 0087, ADR 0094).
fn same_file_open(editor: &Editor, id: BufferId) -> Option<BufferId> {
    let identity = editor.buffer(id)?.baseline()?.file?.identity?;
    editor
        .buffers()
        .find(|(other, buffer)| {
            *other != id
                && !buffer.agent_pending()
                && buffer.baseline().and_then(|b| b.file?.identity) == Some(identity)
        })
        .map(|(other, _)| other)
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
    use super::{found_elsewhere, reusable, same_file_open};
    use oso_core::buffer::{Baseline, Buffer, FileStamp};
    use oso_core::display_map::TabWidth;
    use oso_core::editor::Editor;
    use oso_core::load::decode_all;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    /// A loaded buffer visiting `path`, judged ordinary, holding the file `identity`.
    fn visited(path: &str, identity: Option<(u64, u64)>) -> Buffer {
        let path = PathBuf::from(path);
        let mut buffer = Buffer::visiting(path.clone());
        buffer.judge_secret(Some(std::slice::from_ref(&path)));
        buffer.apply_load(decode_all(b"x\n", TabWidth::default()));
        buffer.set_baseline(Baseline {
            location: path,
            file: Some(FileStamp {
                modified: std::time::SystemTime::UNIX_EPOCH,
                len: 2,
                mode: 0o644,
                identity,
            }),
        });
        buffer
    }

    #[test]
    fn the_buffer_holding_the_same_file_is_found_but_not_one_the_channel_is_opening() {
        let mut editor = Editor::default();
        let persons = editor.add_buffer(visited("/work/notes.md", Some((1, 7))));
        editor.add_buffer(visited("/work/other.md", Some((1, 8))));
        let mut loading = visited("/link/notes.md", Some((1, 7)));
        loading.set_agent_pending(true);
        let agents = editor.add_buffer(loading);
        assert_eq!(
            same_file_open(&editor, agents),
            Some(persons),
            "by device and inode"
        );
        // Another of the channel's own, still opening, is not one to show.
        let mut also = visited("/third/notes.md", Some((1, 7)));
        also.set_agent_pending(true);
        let mut editor2 = Editor::default();
        let first = editor2.add_buffer(also);
        let mut second = visited("/fourth/notes.md", Some((1, 7)));
        second.set_agent_pending(true);
        let second = editor2.add_buffer(second);
        assert_eq!(same_file_open(&editor2, second), None);
        assert_ne!(first, second);
        // With no identity, as where the system gives none, nothing is the same file.
        let none = editor.add_buffer(visited("/x/notes.md", None));
        assert_eq!(same_file_open(&editor, none), None);
    }

    #[test]
    fn a_buffer_open_may_be_shown_for_the_agent_only_judged_ordinary_and_read() {
        assert_eq!(reusable(&visited("/work/notes.md", None)), None);
        let loading = Buffer::visiting(PathBuf::from("/work/big.log"));
        // Not judged yet: it may hold secrets until it is, and says so first.
        assert!(reusable(&loading).is_some_and(|why| why.contains("may hold secrets")));
        let mut judged = Buffer::visiting(PathBuf::from("/work/big.log"));
        judged.judge_secret(Some(&[PathBuf::from("/work/big.log")]));
        assert!(reusable(&judged).is_some_and(|why| why.contains("still loading")));
        let mut env = Buffer::visiting(PathBuf::from("/work/config.txt"));
        env.judge_secret(Some(&[PathBuf::from("/work/.env")]));
        assert!(reusable(&env).is_some_and(|why| why.contains("may hold secrets")));
    }

    #[test]
    fn a_buffer_with_no_file_found_is_not_the_one_asked_for() {
        let buffer = Buffer::from_text("notes.md", "");
        assert!(found_elsewhere(&buffer, Some(OsStr::new("notes.md"))).is_some());
    }
}
