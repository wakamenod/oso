//! Agent sessions in the app (ADR 0121): the sessions worker, what commands ask of it, the
//! hosted sessions (`hosting`), the rules the configuration's thread compiled, and what the
//! worker found, carried out on the UI thread. Nothing here reads a file or judges a screen.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::sync::{Arc, Mutex, PoisonError};

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::{Editor, SessionRequest};

mod hosting;
pub(super) use hosting::Hosting;

use super::{App, UserEvent, Waker};
use crate::config::RulesReport;
use crate::sessions::explain::BUFFER;
use crate::sessions::rules::RuleSet;
use crate::sessions::worker::{Job, Out, Worker};

/// The sessions worker for the app, waking the event loop with [`UserEvent::Sessions`], asked
/// first for the sessions an earlier oso remembered in `state`, oso's state directory, which
/// come back stopped. `None` if it cannot start, which it logs: sessions then have no states.
pub(super) fn worker(waker: &Waker, state: Option<&std::path::Path>) -> Option<Worker> {
    let waker = Mutex::new(waker.clone());
    let worker = Worker::start(move |out| {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Sessions(out));
    })?;
    if let Some(state) = state {
        worker.send(Job::Recall(state.to_path_buf()));
    }
    Some(worker)
}

impl App {
    /// After each event: hands what commands asked about sessions to the worker.
    pub(super) fn sync_sessions(&mut self) {
        for request in self.editor.take_session_requests() {
            let job = match request {
                SessionRequest::ExplainFile(path) => Job::ExplainFile(path),
                SessionRequest::Start { name, directory } => {
                    self.start_session(name, directory);
                    continue;
                }
                SessionRequest::Resume(buffer) => {
                    self.resume_session(buffer);
                    continue;
                }
                SessionRequest::Fork(buffer) => {
                    self.fork_session(buffer);
                    continue;
                }
                // The list, spaces and live explain are carried out once sessions are fed
                // (ADR 0121 step 3).
                SessionRequest::ShowList
                | SessionRequest::Switch(_)
                | SessionRequest::NextWaiting
                | SessionRequest::NextDone
                | SessionRequest::Space(_)
                | SessionRequest::Zoom
                | SessionRequest::Explain(_)
                | SessionRequest::Row { .. }
                | SessionRequest::Regroup
                | SessionRequest::Refresh => continue,
                // Sending is carried out once sessions are fed (ADR 0121 step 5).
                SessionRequest::Send { .. } | SessionRequest::FollowMessage(_) => continue,
            };
            match &self.sessions {
                Some(worker) => worker.send(job),
                None => self
                    .editor
                    .message("Sessions are not available: see the log"),
            }
        }
    }

    /// The configuration's thread read the agent rules file: what it says goes to
    /// `*Messages*`, and its rules to the worker, for every session at once (ADR 0121, Q4).
    pub(super) fn agent_rules_reported(&mut self, report: RulesReport) {
        for note in report.notes {
            self.editor.note(note);
        }
        match (report.rules, &self.sessions) {
            (Some(rules), Some(worker)) => worker.send(Job::Rules(Arc::new(rules))),
            // A compiled rule set can be large: it goes on a thread of its own.
            (Some(rules), None) => drop_elsewhere(rules),
            (None, _) => {}
        }
    }

    /// The worker found something.
    pub(super) fn sessions_reported(&mut self, out: Out) {
        match out {
            Out::Explained(text) => self.show_explanation(&text),
            Out::Unexplained(why) => self.editor.message(why),
            Out::Slow(slowest) => self.editor.note(format!(
                "Agent rules took more than {} ms on a screen, so its state holds; the slowest \
                 rule was {slowest}. See agent-rules/claude.toml",
                crate::sessions::rules::BUDGET.as_millis()
            )),
            Out::Dropped(rule) => self.editor.note(format!(
                "Agent rule {rule} ran past the deadline {} looks in a row, so it is not used \
                 until the rules reload. See agent-rules/claude.toml",
                crate::sessions::worker::DROP_AFTER
            )),
            Out::Recalled(remembered) => self.sessions_recalled(remembered),
            Out::Changed(key, change) => {
                log::debug!("session {key}: {}", change.state);
            }
            // Its text is the screen's: never logged.
            Out::Recap(..) => {}
            // Its text is the screen's too; the row shows it once sessions are fed (step 5).
            Out::Message(..) => {}
        }
    }

    /// Shows `text` in `*Session explain*`, which the agent channel cannot read: it holds a
    /// session's screen.
    fn show_explanation(&mut self, text: &str) {
        if show_explanation(&mut self.editor, &mut self.hosting.explain, text) {
            self.request_redraw();
        }
    }
}

/// Drops `rules` on a thread of its own, as freeing a large compiled set would hold the UI
/// thread. Inline if no thread starts.
fn drop_elsewhere(rules: RuleSet) {
    let spawned = std::thread::Builder::new()
        .name("oso-drop".into())
        .spawn(move || drop(rules));
    if let Err(e) = spawned {
        log::debug!("cannot start a thread to drop agent rules: {e}");
    }
}

/// Puts `text` in the internal `*Session explain*` buffer `kept` names, or in a new one if it
/// is gone, withholds it from the agent channel, and shows it. Never a buffer that is not
/// internal: one of that name anyone made stays theirs, and oso never shows it as its own.
/// Whether it showed the text.
fn show_explanation(editor: &mut Editor, kept: &mut Option<BufferId>, text: &str) -> bool {
    let id = match kept.filter(|id| editor.buffer(*id).is_some_and(Buffer::is_internal)) {
        Some(id) => id,
        None => editor.add_internal_buffer(BUFFER, Mode::Fundamental),
    };
    *kept = Some(id);
    let Some(buffer) = editor.buffer_mut(id) else {
        return false;
    };
    buffer.withhold_from_agents();
    editor.set_internal_text(id, text);
    // A report of some 50 KiB at most, compared once.
    let set = editor.buffer(id).is_some_and(|buffer| {
        buffer.text().len() == text.len() && buffer.text().to_string() == text
    });
    if set {
        editor.show(id);
    }
    set
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::command::Reach;
    use oso_core::editor::Editor;
    use oso_core::window::ViewSize;

    use super::{BUFFER, show_explanation};
    use crate::channel::protocol::TextOf;
    use crate::dispatch::{Dispatcher, Given};

    fn editor() -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", "abc"));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    /// The listed buffers of its name: the internal one is not listed.
    fn listed(editor: &Editor) -> Vec<String> {
        editor
            .buffers()
            .filter(|(_, buffer)| buffer.name() == BUFFER)
            .map(|(_, buffer)| buffer.text().to_string())
            .collect()
    }

    #[test]
    fn a_plain_buffer_of_its_name_is_never_taken_over() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor());
        // Made as `C-x b *Session explain*` and typing would make it.
        editor.add_buffer(Buffer::from_text(BUFFER, "state: needs input"));
        let (_, result) = dispatcher
            .run_for_agent(
                &mut editor,
                "switch-to-buffer",
                Given::Text(BUFFER),
                Reach::Look,
            )
            .unwrap();
        assert_eq!(result, Ok(()));
        let mut kept = None;
        let plain = editor.window().buffer();
        assert!(show_explanation(&mut editor, &mut kept, "state: idle\n"));
        assert_ne!(kept, Some(plain));
        assert_eq!(listed(&editor), ["state: needs input"]);
        let shown = editor.current().1;
        assert!(shown.is_internal() && shown.withheld_from_agents());
        assert_eq!(shown.text().to_string(), "state: idle\n");
        // Even when the id kept names it, as an id of a buffer that went could not.
        let mut kept = Some(plain);
        assert!(show_explanation(&mut editor, &mut kept, "again\n"));
        assert_ne!(kept, Some(plain));
        assert_eq!(listed(&editor), ["state: needs input"]);
    }

    #[test]
    fn explaining_twice_leaves_one_internal_buffer() {
        let mut editor = editor();
        let mut kept = None;
        assert!(show_explanation(&mut editor, &mut kept, "first\n"));
        let first = kept.unwrap();
        assert!(show_explanation(&mut editor, &mut kept, "second\n"));
        assert_eq!(kept, Some(first), "the same buffer again");
        assert_eq!(editor.current().1.text().to_string(), "second\n");
        assert!(listed(&editor).is_empty());
    }

    #[test]
    fn the_channel_reads_nothing_of_it_and_can_still_act() {
        let (mut dispatcher, mut editor) = (Dispatcher::new(), editor());
        let mut kept = None;
        assert!(show_explanation(&mut editor, &mut kept, "screen row\n"));
        let id = kept.unwrap();
        assert_eq!(editor.window().buffer(), id);
        let state = crate::channel::state::state(&editor, None, 0);
        assert!(state["text"].is_null(), "{state}");
        let window = &state["windows"][0];
        assert!(window["selections"].is_null(), "{window}");
        let rows = TextOf::Rows {
            buffer: id.get(),
            start_row: 0,
            rows: 1,
        };
        assert!(
            crate::channel::text::start(&editor, rows).is_err(),
            "its rows"
        );
        let Err(why) = crate::channel::text::start(&editor, TextOf::Region) else {
            panic!("its region was read");
        };
        assert!(why.contains("only the person"), "{why}");
        // It is no secret: the channel still runs commands, and the person's kill is theirs.
        assert!(!editor.shows_secret());
        let (_, result) = dispatcher
            .run_for_agent(&mut editor, "forward-char", Given::Count(3), Reach::Look)
            .unwrap();
        assert_eq!(result, Ok(()));
        let (_, result) = dispatcher
            .run_for_agent(
                &mut editor,
                "switch-to-buffer",
                Given::Text("notes.md"),
                Reach::Look,
            )
            .unwrap();
        assert_eq!(result, Ok(()));
    }
}
