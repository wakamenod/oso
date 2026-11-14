//! What the commands of ADR 0121 step 3 ask, carried out: `*Sessions*` and its rows, showing a
//! session (in its space, Q2), spaces and zoom, a live explain, and a stop that asks first
//! while a session works or needs input (Q8); and step 5's sends and message follows, in
//! `sending`. The question is the host's, as only the app
//! knows a session's state.
//!
//! Must not depend on: how a frame is drawn, or key presses.

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, RowAction, SessionRequest};

use crate::app::App;
use crate::sessions::list::{Key, Line};
use crate::sessions::spaces::{Refused, Zoom};
use crate::sessions::state::State;
use crate::sessions::worker::{Job, SessionKey};

/// What a refused space switch or zoom says.
const PROMPT_OPEN: &str = "Close the prompt first";

impl App {
    /// Carries out `request`, one of step 3's. Returns it back if it is not one.
    pub(in crate::app) fn session_request(
        &mut self,
        request: SessionRequest,
    ) -> Option<SessionRequest> {
        match request {
            SessionRequest::ShowList => self.toggle_sessions_list(),
            SessionRequest::Switch(name) => match self.session_named(&name) {
                Some(key) => self.show_session(key),
                None => self.editor.message(format!("No session {name}")),
            },
            SessionRequest::NextWaiting => {
                self.show_oldest(|state| matches!(state, State::NeedsInput(_)), "needs input");
            }
            SessionRequest::NextDone => self.show_oldest(
                |state| matches!(state, State::Done | State::ReadyForReview),
                "is done",
            ),
            SessionRequest::Space(name) => match self.watch.spaces.named(&name) {
                Some(at) => self.enter_space(at),
                None => self.editor.message(format!("No space {name}")),
            },
            SessionRequest::Zoom => match self.watch.spaces.zoom(&mut self.editor) {
                Ok(Zoom::In | Zoom::Out) => {}
                Err(Refused) => self.editor.message(PROMPT_OPEN),
            },
            SessionRequest::Explain(buffer) => match self.hosting.table().by_buffer(buffer) {
                Some(session) => self.explain_session(session.key),
                None => self.editor.message("This buffer holds no session"),
            },
            SessionRequest::Row { line, action } => self.on_row(line, action),
            SessionRequest::Regroup => self.watch.list.regroup(),
            SessionRequest::Refresh => {}
            SessionRequest::Send { to, parts } => self.send_request(&to, &parts),
            SessionRequest::FollowMessage(buffer) => self.follow_message(buffer),
            other => return Some(other),
        }
        self.watch.stale = true;
        self.request_redraw();
        None
    }

    /// `C-x a s` (Q1): hidden, it shows and is selected; shown, it is selected; selected, it
    /// is hidden, and stays hidden until asked for again.
    fn toggle_sessions_list(&mut self) {
        if !self.sessions_list_shown() {
            self.show_sessions_side();
        }
        let Some(side) = self.editor.side_window() else {
            return;
        };
        if self.editor.window().id() == side {
            self.hide_sessions_side();
        } else {
            self.editor.select_window(side);
        }
    }

    /// The hosted session whose terminal's buffer is called `name`.
    pub(super) fn session_named(&self, name: &str) -> Option<SessionKey> {
        self.hosting.table().iter().find_map(|session| {
            let buffer = session.buffer?;
            (self.editor.buffer(buffer)?.name() == name).then_some(session.key)
        })
    }

    /// Shows the session in a state `wanted` that has been in it longest.
    fn show_oldest(&mut self, wanted: fn(State) -> bool, says: &str) {
        let oldest = self
            .watch
            .known
            .iter()
            .filter(|(key, known)| {
                wanted(known.state)
                    && self
                        .hosting
                        .table()
                        .get(**key)
                        .is_some_and(|s| !s.archived && s.buffer.is_some())
            })
            .min_by_key(|(_, known)| known.since)
            .map(|(key, _)| *key);
        match oldest {
            Some(key) => self.show_session(key),
            None => self.editor.message(format!("No session {says}")),
        }
    }

    /// Shows session `key` (Q2): in its space, switched to first, in the window that shows it
    /// there, else in the tree's selected window, which is then selected.
    pub(super) fn show_session(&mut self, key: SessionKey) {
        let Some(session) = self.hosting.table().get(key) else {
            return;
        };
        let Some(buffer) = session
            .buffer
            .filter(|id| self.editor.buffer(*id).is_some())
        else {
            let name = session.name.clone();
            return self.editor.message(format!(
                "Session {name} has no terminal yet: session-resume brings it back"
            ));
        };
        let space = self.watch.spaces.of_path(super::place(session));
        if let Some(at) = space
            && self.watch.spaces.current() != Some(at)
        {
            self.enter_space(at);
        }
        let shown = self
            .editor
            .windows()
            .find(|window| {
                window.buffer() == buffer && Some(window.id()) != self.editor.side_window()
            })
            .map(oso_core::window::Window::id);
        match shown {
            Some(window) => self.editor.select_window(window),
            None => {
                self.editor.show(buffer);
                if let Some(side) = self.editor.side_window()
                    && self.editor.window().id() == side
                {
                    self.editor.other_window(1);
                }
            }
        }
        if let Some(space) = space.and_then(|at| self.watch.spaces.get_mut(at)) {
            space.last_session = Some(buffer);
        }
    }

    /// Switches to space `at`, keeping the file buffer shown last in the space left.
    fn enter_space(&mut self, at: usize) {
        let shown = tree_buffer(&mut self.editor);
        let is_session = self.hosting.table().by_buffer(shown).is_some();
        if let Some(from) = self.watch.spaces.current()
            && let Some(space) = self.watch.spaces.get_mut(from)
        {
            if is_session {
                space.last_session = Some(shown);
            } else {
                space.last_file = Some(shown);
            }
        }
        if self.watch.spaces.enter(&mut self.editor, at) == Err(Refused) {
            self.editor.message(PROMPT_OPEN);
        }
    }

    /// Asks the worker to explain the last look at session `key`'s screen.
    fn explain_session(&mut self, key: SessionKey) {
        match &self.sessions {
            Some(worker) => worker.send(Job::Explain(key)),
            None => self
                .editor
                .message("Sessions are not available: see the log"),
        }
    }

    /// A key of `*Sessions*` on line `line`.
    fn on_row(&mut self, line: usize, action: RowAction) {
        let Some(row) = self.sessions_line(line).cloned() else {
            return;
        };
        let key = match row {
            Line::Header(_) if action == RowAction::Fold => {
                return self.watch.list.fold(&row);
            }
            Line::Session(key) if action == RowAction::Fold => {
                return self.watch.list.fold(&Line::Session(key));
            }
            Line::Header(_) => return,
            Line::Session(Key::External(pid)) => {
                return self.editor.message(format!(
                    "Process {pid} runs that session outside oso: it is only listed"
                ));
            }
            Line::Session(Key::Hosted(key)) => key,
        };
        match action {
            RowAction::Show => self.show_session(key),
            RowAction::Fold => {}
            RowAction::Pin => self.change_session(key, |session| session.pinned ^= true),
            RowAction::Archive => self.change_session(key, |session| session.archived ^= true),
            RowAction::Resume => match self.hosting.table().get(key).and_then(|s| s.buffer) {
                Some(buffer) => self.resume_session(buffer),
                None => self
                    .editor
                    .message("That session has no terminal to resume in"),
            },
            RowAction::Stop => self.stop_session(key),
            RowAction::Explain => self.explain_session(key),
        }
    }

    /// Changes session `key` as `change` does, and remembers it.
    fn change_session(
        &mut self,
        key: SessionKey,
        change: fn(&mut crate::sessions::hosted::Hosted),
    ) {
        if let Some(session) = self.hosting.table.get_mut(key) {
            change(session);
            self.remember_sessions();
        }
    }

    /// `k` (Q8): hangs up session `key`'s terminal, asking first while it works or needs
    /// input. The yes runs `session-stop` again, which finds the question answered.
    fn stop_session(&mut self, key: SessionKey) {
        if let Some((prompt, asked)) = self.watch.stop.take()
            && !self.editor.prompts().iter().any(|p| p.id() == prompt)
        {
            // The yes to the question: the session asked about, whatever row point is on now.
            return self.hang_up_session(asked);
        }
        let Some(session) = self.hosting.table().get(key) else {
            return;
        };
        let state = self.watch.known.get(&key).map(|known| known.state);
        let busy = matches!(state, Some(State::Working | State::NeedsInput(_)));
        if let Some(state) = state.filter(|_| busy) {
            let question = format!("Session {} is {state}; hang it up?", session.name);
            match self.editor.ask(question, "session-stop") {
                Ok(prompt) => self.watch.stop = Some((prompt, key)),
                Err(e) => self.editor.message(e.to_string()),
            }
            return;
        }
        self.hang_up_session(key);
    }

    /// Sends the hang-up to session `key`'s terminal: oso stopped it, so its end is no failure.
    /// A worktree's removal stops the worktree's idle sessions so too (ADR 0121 step 4).
    pub(in crate::app) fn hang_up_session(&mut self, key: SessionKey) {
        let Some(buffer) = self.hosting.table().get(key).and_then(|s| s.buffer) else {
            return self.editor.message("That session does not run");
        };
        let terminal = self
            .terminals
            .map
            .get(buffer)
            .and_then(|hosted| hosted.terminal.as_ref());
        let Some(terminal) = terminal else {
            return self.editor.message("That session does not run");
        };
        terminal.hang_up();
        // oso stopped it: its end by the signal is no failure (Q8).
        self.watch.hung_up.insert(key);
    }

    /// Drops a stop whose question closed without a yes, so that a later `k` asks again.
    pub(in crate::app) fn drop_unasked_stop(&mut self) {
        if let Some((prompt, _)) = self.watch.stop
            && !self.editor.prompts().iter().any(|p| p.id() == prompt)
        {
            self.watch.stop = None;
        }
    }
}

/// The buffer the tree's window shows, never the side window's `*Sessions*`: from the side
/// window, the tree's window selected last is selected first, as the tree's commands do
/// (ADR 0121). With a prompt open, the window beneath it.
pub(super) fn tree_buffer(editor: &mut Editor) -> BufferId {
    let side = editor.side_window();
    if editor.prompt().is_none() && side.is_some() && side == Some(editor.tiled().0.id()) {
        // From the side window, `C-x o` selects the tree's window selected last.
        editor.other_window(1);
    }
    editor.tiled().0.buffer()
}
