//! Replies that wait for the save their command started (ADR 0108): once it has ended, they
//! say whether it wrote, and past [`SAVE_WITHIN`] that it goes on.
//!
//! Must not depend on: sockets or threads, which stay behind the channel's requests.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use serde_json::json;

use oso_core::buffer::BufferId;

use super::super::App;
use crate::channel::server::ANSWER_WITHIN;
use crate::channel::{Connection, Reply};

/// A request whose reply waits until the save its command started has ended: whether it
/// wrote, or was refused before it could.
#[derive(Debug)]
pub(super) struct Saving {
    connection: Connection,
    buffer: BufferId,
    reply: Sender<Reply>,
    /// What the reply says, with how the save went.
    done: serde_json::Value,
    /// When it started: past [`SAVE_WITHIN`] the reply goes, saying it still saves.
    started: Instant,
}

/// How long a request waits for the save its command started, under the channel's wait for
/// a reply: then it says the save goes on, and the person's echo area says how it ended.
const SAVE_WITHIN: Duration = ANSWER_WITHIN;

impl App {
    /// Holds back the reply `done` of a command that started a save of `buffer`, until it ends.
    pub(super) fn wait_for_save(
        &mut self,
        connection: Connection,
        buffer: BufferId,
        reply: Sender<Reply>,
        done: serde_json::Value,
    ) {
        self.agent.saving.push(Saving {
            connection,
            buffer,
            reply,
            done,
            started: Instant::now(),
        });
        self.wake_after(SAVE_WITHIN);
    }

    /// A save has ended: the request that started it, if one waits, hears how it went.
    pub(in crate::app) fn agent_save_ended(&mut self, report: &crate::saver::SaveReport) {
        let Some(index) = self
            .agent
            .saving
            .iter()
            .position(|saving| saving.buffer == report.buffer())
        else {
            return;
        };
        let saving = self.agent.saving.swap_remove(index);
        let failure = report.agent_failure();
        if let Some(why) = failure {
            self.agent_log(&format!("save: {why}"));
        }
        let _ = saving
            .reply
            .send(save_reply(saving.done, failure, report.wrote()));
    }

    /// Replies `why` to the request whose save of `buffer` waited for a whitespace scan and
    /// then could not start (ADR 0113).
    pub(in crate::app) fn agent_save_not_started(&mut self, buffer: BufferId, why: &str) {
        let Some(index) = self
            .agent
            .saving
            .iter()
            .position(|saving| saving.buffer == buffer)
        else {
            return;
        };
        let saving = self.agent.saving.swap_remove(index);
        self.agent_log(&format!("save: {why}"));
        let _ = saving.reply.send(Err(why.to_owned()));
    }

    /// Replies to each request whose save still runs past [`SAVE_WITHIN`], under the
    /// channel's wait: the save goes on, and the person's echo area says how it ended.
    pub(super) fn reply_to_long_saves(&mut self) {
        let (late, kept) = std::mem::take(&mut self.agent.saving)
            .into_iter()
            .partition(|saving| saving.started.elapsed() >= SAVE_WITHIN);
        self.agent.saving = kept;
        for saving in late {
            let _ = saving.reply.send(still_saving(saving.done));
        }
    }
}

/// Forgets the replies `connection` waits for: they go nowhere now, and each save still ends as
/// it would.
pub(super) fn forget_saves_of(saving: &mut Vec<Saving>, connection: Connection) {
    saving.retain(|saving| saving.connection != connection);
}

/// The reply to a request whose command started a save, once that save has ended: what it
/// would have said, with `failure` if the save failed, and whether it wrote.
fn save_reply(mut done: serde_json::Value, failure: Option<&str>, wrote: bool) -> Reply {
    if let Some(why) = failure {
        return Err(why.to_owned());
    }
    done["saved"] = json!(wrote);
    if !wrote {
        done["unchanged"] = json!("nothing to write: the file already held the buffer's text");
    }
    Ok(done)
}

/// The reply to a request whose save still runs: it goes on, and is not yet said to have
/// written.
fn still_saving(mut done: serde_json::Value) -> Reply {
    done["saving"] = json!(true);
    Ok(done)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_save_reply_says_whether_it_wrote_and_why_it_did_not() {
        use super::{save_reply, still_saving};
        use serde_json::json;
        let done = || json!({ "command": "save-buffer" });
        let wrote = save_reply(done(), None, true).unwrap();
        assert_eq!(wrote["saved"], true);
        let unchanged = save_reply(done(), None, false).unwrap();
        assert_eq!(unchanged["saved"], false);
        assert!(
            unchanged["unchanged"]
                .as_str()
                .unwrap()
                .contains("nothing to write")
        );
        let refused = save_reply(done(), Some("refused: the file is in …"), false);
        assert_eq!(refused, Err("refused: the file is in …".to_owned()));
        let long = still_saving(done()).unwrap();
        assert_eq!(long["saving"], true);
        assert!(long.get("saved").is_none(), "not yet said to have written");
    }

    #[test]
    fn the_replies_a_connection_waits_for_go_with_it() {
        use super::{Saving, forget_saves_of};
        use crate::channel::Connection;
        use oso_core::buffer::Buffer;
        use oso_core::editor::Editor;
        let mut editor = Editor::default();
        let buffer = editor.add_buffer(Buffer::from_text("notes.md", ""));
        let gone = Connection { run: 1, number: 1 };
        let other = Connection { run: 1, number: 2 };
        let saving = |connection| {
            let (reply, replies) = std::sync::mpsc::channel();
            let waiting = Saving {
                connection,
                buffer,
                reply,
                done: serde_json::json!({}),
                started: std::time::Instant::now(),
            };
            (waiting, replies)
        };
        let (first, first_replies) = saving(gone);
        let (second, second_replies) = saving(other);
        let mut waiting = vec![first, second];
        forget_saves_of(&mut waiting, gone);
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].connection, other);
        assert!(first_replies.recv().is_err(), "its reply is dropped");
        assert!(second_replies.try_recv().is_err(), "the other still waits");
        drop(waiting);
    }
}
