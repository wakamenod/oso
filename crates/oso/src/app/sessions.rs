//! Agent sessions in the app (ADR 0121): the sessions worker, what commands ask of it, the
//! hosted sessions (`hosting`), worktree sessions and their jobs (`worktrees`), the agents'
//! session tools (`tools`), the rules the configuration's thread compiled, and what the worker
//! and the jobs found, carried out on the UI thread. Nothing here reads a file or judges a
//! screen.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::sync::{Arc, Mutex, PoisonError};

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::{Editor, SessionRequest};

mod conversations;
mod feed;
mod hosting;
mod spaces;
mod tools;
mod watch;
mod worktrees;
pub(super) use conversations::conversations;
pub(super) use hosting::{Hosting, NativeSessions, Placeholder, RequestWaits, Typing};
pub(super) use spaces::Orchestrator;
pub(super) use watch::Watch;
pub(in crate::app) use watch::sending::{Outgoing, Shown};

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
    worker.send(Job::Recall(state.map(std::path::Path::to_path_buf)));
    Some(worker)
}

impl App {
    /// After each event: hands what commands asked about sessions to the worker.
    pub(super) fn sync_sessions(&mut self) {
        let mut asked = false;
        for request in self.editor.take_session_requests() {
            let job = match request {
                SessionRequest::ExplainFile(path) => {
                    Job::ExplainFile(path, self.request_waits().holds_shown())
                }
                SessionRequest::Spaces(request) => {
                    self.space_request(request);
                    continue;
                }
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
                SessionRequest::YankImage(buffer) => {
                    let from_socket = self.dispatcher.take_image_yank_origin();
                    self.yank_image(buffer, from_socket);
                    continue;
                }
                SessionRequest::QuestionDetails => {
                    self.show_question_details();
                    continue;
                }
                SessionRequest::AgentStarts(allow) => {
                    self.hold_agent_starts(!allow);
                    continue;
                }
                SessionRequest::Adopt(pick) => {
                    self.adopt_session(&pick);
                    continue;
                }
                // The message is for step 2's read-only view (ADR 0145).
                SessionRequest::Worktree { .. }
                | SessionRequest::SendBrief(_)
                | SessionRequest::CancelBrief(_)
                | SessionRequest::WorktreeList
                | SessionRequest::WorktreeRemove(_)
                | SessionRequest::WorktreeRemoveConfirmed
                | SessionRequest::WorktreeRemoveStop
                | SessionRequest::WorktreeRemoveStopConfirmed
                | SessionRequest::StartShared => {
                    self.worktree_request(request);
                    continue;
                }
                // The list, spaces and live explain (ADR 0121 step 3).
                request => {
                    match self.session_request(request) {
                        Some(other) => log::debug!("a session request not carried out: {other:?}"),
                        None => asked = true,
                    }
                    continue;
                }
            };
            match &self.sessions {
                Some(worker) => worker.send(job),
                None => self
                    .editor
                    .message("Sessions are not available: see the log"),
            }
        }
        self.drop_unanswered_fork_offer();
        self.show_waiting_adoptable();
        // After the requests, so that a `session-stop` that a question's yes ran finds its
        // stop still asked.
        self.drop_unasked_stop();
        self.drop_unanswered_rename();
        self.drop_unanswered_plan_question();
        self.forget_unanswered_space();
        // A space entered gives `session-search` its scope (ADR 0145).
        self.keep_space_scope(false);
        self.notice_trust();
        self.forget_unanswered_offer();
        // After the requests, so that a yes begins the start it was asked about first, and
        // after the other questions' sweeps, so that what it asks is judged by none of them.
        self.ask_next_shared();
        self.sync_seen();
        // What the person asked shows at once; the worker's changes wait for the loop, so
        // that many make the views once.
        if asked {
            self.refresh_sessions();
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
            Out::Explained(key, text, there) => {
                match key.and_then(|key| self.unasked_explained(key, &text)) {
                    Some(text) => self.show_explanation(&text, there),
                    None => self.show_explanation(&text, there),
                }
            }
            // A session started without asking still shows who started it and its brief.
            Out::Unexplained(key, why, there) => {
                match key.and_then(|key| self.unasked_explained(key, &why)) {
                    Some(text) => self.show_explanation(&text, there),
                    None => self.editor.message(why),
                }
            }
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
            Out::WorktreesMoved(roots) => self.worktrees_moved(&roots),
            Out::SpaceDirMade {
                id,
                made,
                identities,
            } => self.space_directory_made(&id, made, identities),
            Out::RepositoryChecked {
                check,
                path,
                member,
                known,
            } => self.repository_checked(check, &path, member, known),
            Out::InstructionsReady { id, file } => self.instructions_ready(&id, file),
            Out::SpaceSummarized { id, named } => self.space_named(&id, named),
            Out::HandoverLooked {
                id,
                from,
                handoff,
                transcript,
            } => self.handover_looked(&id, &from, (handoff, transcript)),
            Out::SpaceIds(fresh) => {
                if self.watch.spaces_mut().give_ids(fresh) {
                    self.sessions_touched();
                }
            }
            Out::Recalled(recalled) => {
                self.sessions_recalled(recalled);
                self.sessions_touched();
                // Images past their week go once, as oso starts (ADR 0142).
                self.sweep_images();
                // Once oso knows its own, the others it may adopt.
                self.hosting.adopting.recalled = true;
                self.list_adoptable();
                // Those a restart resumes, if the instance before has gone (ADR 0165, part 4c).
                self.resume_natives_carried();
            }
            Out::Changed(key, change) => {
                log::debug!("session {key}: {}", change.state);
                // A review of what it works in is stale (ADR 0122, step 4).
                if change.turn_ended
                    && let Some(root) = self.hosting.table().get(key).and_then(|it| it.root.clone())
                {
                    self.reviews_touched(&root);
                }
                self.sending_changed(key, change.state);
                self.session_changed(key, change);
            }
            // Its text is the screen's: never logged.
            Out::Recap(key, recap) => self.session_recapped(key, recap),
            Out::Externals(live) => {
                // What Claude Code calls oso's own running sessions (ADR 0155).
                if self.watch.polls() {
                    (self.hosting.naming).read_registry(&live, &self.hosting.table);
                }
                self.externals_found(live);
            }
            // Its text is the screen's too: never logged.
            Out::Message(key, from) => self.message_from(key, from),
            Out::Judged(key, held) => self.session_judged(key, held),
        }
    }

    /// Shows `text` in `*Session explain*`, which the agent channel cannot read: it holds a
    /// session's screen.
    ///
    /// Not where a native session's request came to wait since it was asked for, if it was not
    /// asked for `there` (ADR 0182, "Point" 8).
    fn show_explanation(&mut self, text: &str, there: bool) {
        if !there && self.request_waits().holds_shown() {
            return self
                .editor
                .message("Not explained: a native session's request waits for you");
        }
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
/// is gone, withholds it from the agent channel, and shows it. Whether it showed the text.
fn show_explanation(editor: &mut Editor, kept: &mut Option<BufferId>, text: &str) -> bool {
    show_withheld(editor, kept, BUFFER, text)
}

/// A new editable buffer named `name` in `mode` holding `text`, for a message the person writes:
/// a brief or a composer. No agent reads it before it is sent, and the agent channel's changes
/// leave it alone (`dispatch::agent`).
pub(in crate::app) fn withheld_draft(
    editor: &mut Editor,
    name: &str,
    mode: Mode,
    text: &str,
) -> BufferId {
    let mut buffer = Buffer::from_text(name, text);
    buffer.set_mode(mode);
    buffer.withhold_from_agents();
    editor.add_buffer(buffer)
}

/// Whether `buffer` is a native session's composer with nothing in it, which takes what oso
/// offers there: not one that a yank turned secret, which sends nothing (ADR 0165).
pub(in crate::app) fn empty_composer(buffer: &Buffer) -> bool {
    buffer.mode() == Mode::Composer && !buffer.is_secret() && buffer.text().is_empty()
}

/// Puts `text` in the internal buffer `kept` names, or in a new one named `name` if it is
/// gone, withholds it from the agent channel, and shows it: `*Session explain*`, a native
/// session's `*result: NAME*` (ADR 0165). Never a buffer that is not internal: one of that
/// name anyone made stays theirs, and oso never shows it as its own. Whether it showed the
/// text.
pub(in crate::app) fn show_withheld(
    editor: &mut Editor,
    kept: &mut Option<BufferId>,
    name: &str,
    text: &str,
) -> bool {
    let id = match kept.filter(|id| editor.buffer(*id).is_some_and(Buffer::is_internal)) {
        Some(id) => id,
        None => editor.add_internal_buffer(name, Mode::Fundamental),
    };
    *kept = Some(id);
    let Some(buffer) = editor.buffer_mut(id) else {
        return false;
    };
    buffer.withhold_from_agents();
    editor.set_internal_text(id, text);
    // A report, or a result of at most `wire`'s 1 MiB and its change's headers, compared once.
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
        let state = crate::channel::state::state(&editor, None, 0, (&|_| None, None));
        assert!(state["text"].is_null(), "{state}");
        let window = &state["windows"][0];
        assert!(window["selections"].is_null(), "{window}");
        let rows = TextOf::Rows {
            buffer: id.get(),
            start_row: 0,
            rows: 1,
        };
        assert!(
            crate::channel::text::start(&editor, &|_, _| None, rows).is_err(),
            "its rows"
        );
        let Err(why) = crate::channel::text::start(&editor, &|_, _| None, TextOf::Region) else {
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
