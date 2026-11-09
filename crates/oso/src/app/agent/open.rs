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
            let opening = self.agent.opening.swap_remove(index);
            if let Some(why) = refused {
                self.drop_agents_file(opening.buffer);
                let _ = opening.reply.send(Err(why.into()));
                self.agent_log("open: refused");
                continue;
            }
            let reply = self.show_agents_file(&opening);
            let _ = opening.reply.send(reply);
        }
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
        self.editor.show(opening.buffer);
        if let Some(line) = opening.line {
            let line = isize::try_from(line).unwrap_or(isize::MAX);
            let _ = self.dispatcher.run_for_agent(
                &mut self.editor,
                "goto-line",
                Given::Count(line),
                Reach::Look,
            );
        }
        self.request_redraw();
        let name = self
            .editor
            .buffer(opening.buffer)
            .map(|b| b.name().to_owned());
        self.agent_log(&format!("open {}: ok", name.as_deref().unwrap_or("")));
        Ok(json!({ "buffer": opening.buffer.get(), "name": name }))
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
    use super::found_elsewhere;
    use oso_core::buffer::Buffer;
    use std::ffi::OsStr;

    #[test]
    fn a_buffer_with_no_file_found_is_not_the_one_asked_for() {
        let buffer = Buffer::from_text("notes.md", "");
        assert!(found_elsewhere(&buffer, Some(OsStr::new("notes.md"))).is_some());
    }
}
