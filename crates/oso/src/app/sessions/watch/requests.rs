//! What the commands of ADR 0121 step 3 ask, carried out: `*Spaces*` and its rows, showing a
//! session (in its space, Q2), spaces and zoom, a live explain, and a stop that asks first
//! while a session works or needs input (Q8); and step 5's sends and message follows, in
//! `sending`. The question is the host's, as only the app
//! knows a session's state.
//!
//! Must not depend on: how a frame is drawn, or key presses.

use std::path::Path;

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, RowAction, SessionRequest};

use crate::app::App;
use crate::sessions::list::{Key, Line};
use crate::sessions::spaces::SpaceId;
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
            // `session-search`'s rows and views (ADR 0145).
            SessionRequest::GoToConversation { session, .. } => self.go_to_conversation(&session),
            SessionRequest::ReadConversation { session, message } => {
                self.read_conversation(&session, message);
            }
            SessionRequest::ForkConversation { session } => self.fork_found(&session),
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
            SessionRequest::OnBuffer(buffer, action) => {
                match self.hosting.table().by_buffer(buffer).map(|s| s.key) {
                    Some(key) => self.on_session(key, action),
                    None => self.editor.message("This buffer holds no session"),
                }
            }
            SessionRequest::Regroup => self.watch.list.regroup(),
            SessionRequest::CycleFolds => {
                if !self.watch.list.cycle_folds(self.watch.view.open_to) {
                    self.editor.message("S-TAB folds the tree; s shows it");
                }
            }
            SessionRequest::PaneWidth(columns) => self.change_pane_width(columns),
            SessionRequest::Refresh => self.relist_projects(),
            SessionRequest::Send { to, parts } => self.send_request(&to, &parts),
            SessionRequest::FollowMessage(buffer) => self.follow_message(buffer),
            other => return Some(other),
        }
        self.watch.stale = true;
        self.request_redraw();
        None
    }

    /// `C-c a s` (Q1): hidden, it shows and is selected; shown, it is selected; selected, the
    /// tree's window selected last is (ADR 0140). `C-u C-c a s` hides it, in the command.
    fn toggle_sessions_list(&mut self) {
        if !self.sessions_list_shown() {
            self.show_sessions_side();
        }
        let Some(side) = self.editor.side_window() else {
            return;
        };
        // Selected, it gives the keys back to the tree's window selected last, as `C-x o` does
        // from it (ADR 0140); `C-u` hides it, in the command.
        if self.editor.window().id() == side {
            self.editor.other_window(1);
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
    pub(in crate::app) fn show_session(&mut self, key: SessionKey) {
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
        let space = self.space_of_session(session);
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
        if let Some(at) = space {
            self.watch.spaces.showed(at, Some(buffer), None);
        }
    }

    /// Switches to space `at`, keeping the file buffer shown last in the space left.
    pub(in crate::app) fn enter_space(&mut self, at: usize) {
        let shown = tree_buffer(&mut self.editor);
        let is_session = self.hosting.table().by_buffer(shown).is_some();
        if let Some(from) = self.watch.spaces.current() {
            let (session, file) = if is_session {
                (Some(shown), None)
            } else {
                (None, Some(shown))
            };
            self.watch.spaces.showed(from, session, file);
        }
        if self.watch.spaces.enter(&mut self.editor, at) == Err(Refused) {
            self.editor.message(PROMPT_OPEN);
        }
    }

    /// `RET` on the row of the worktree at `path` (ADR 0131, step 2b): the session there, in
    /// the space that holds it (ADR 0136, step 1a); else that space's tree with its window
    /// selected; else `find-file`'s prompt in it.
    fn enter_worktree(&mut self, space: &SpaceId, path: &Path) {
        // The row's space, where a worktree two spaces show is entered (ADR 0136, step 2).
        let row_space = self.watch.spaces.find(space);
        let there = self
            .hosting
            .table()
            .iter()
            .filter(|session| !session.archived && session.buffer.is_some())
            .filter(|session| row_space.is_none() || self.space_of_session(session) == row_space)
            .find(|session| oso_core::project::same_place(super::place(session), path))
            .map(|session| session.key);
        if let Some(key) = there {
            return self.show_session(key);
        }
        // A space that holds the worktree, or has it as a repository's main checkout: the row's
        // first. One it does not is no place to enter: a file is found in it instead.
        let holds = |at: usize| {
            self.watch
                .spaces
                .stored(at)
                .is_some_and(|stored| stored.holds_worktree(path) || stored.has_member(path))
        };
        let target = row_space
            .filter(|&at| holds(at))
            .or_else(|| self.watch.spaces.holding(path));
        match target {
            Some(at) => {
                if self.watch.spaces.current() != Some(at) {
                    self.enter_space(at);
                }
                tree_buffer(&mut self.editor);
            }
            None => {
                let effects = self.dispatcher.find_file_in(&mut self.editor, path);
                if effects.redraw {
                    self.request_redraw();
                }
            }
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

    /// A key of `*Spaces*` on line `line`.
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
            Line::Worktree(id, path) if action == RowAction::Show => {
                return self.enter_worktree(&id, &path);
            }
            Line::Worktree(..) if action == RowAction::Stop => {
                return self.editor.message(
                    "worktree-remove removes a worktree whose work is done, from a buffer in it or *Worktrees*",
                );
            }
            // A space's row enters it on `RET` (ADR 0136, step 2, Q2); a closed one's reopens it
            // (Q3).
            // Its orchestrator when it has one (ADR 0136, step 3), else the space entered.
            Line::Space(id) if action == RowAction::Show => {
                let Some(at) = self.watch.spaces.find(&id) else {
                    return;
                };
                match self.orchestrator_of(at) {
                    Some(key) => self.show_session(key),
                    None => self.enter_space(at),
                }
                return;
            }
            Line::ClosedSpace(id) if action == RowAction::Show => {
                let name = self
                    .watch
                    .spaces
                    .find(&id)
                    .and_then(|at| Some(self.watch.spaces.get(at)?.name().to_owned()));
                if let Some(name) = name {
                    self.space_request(oso_core::editor::SpaceRequest::Reopen(name));
                }
                return;
            }
            Line::ClosedSpace(_) | Line::Title => return,
            // A repository's row folds on `RET` too, as its worktrees are what it holds.
            Line::Space(_) | Line::Repository(..) | Line::Worktree(..) => {
                if matches!(action, RowAction::Fold | RowAction::Show) {
                    self.watch.list.fold(&row);
                }
                return;
            }
            Line::Session(Key::External(pid)) => {
                return self.editor.message(format!(
                    "Process {pid} runs that session outside oso: it is only listed"
                ));
            }
            Line::Session(Key::Hosted(key)) => key,
        };
        self.on_session(key, action);
    }

    /// Does `action` to hosted session `key`, from its row or from its own terminal's session
    /// mode (ADR 0132).
    fn on_session(&mut self, key: SessionKey, action: RowAction) {
        match action {
            RowAction::Show => self.show_session(key),
            RowAction::Fold => {}
            RowAction::Pin => self.change_session(key, |session| session.pinned ^= true),
            RowAction::Archive => {
                self.change_session(key, |session| session.archived ^= true);
                // Its images go with it (ADR 0142); brought back, it has none.
                if let Some(session) = self.hosting.table().get(key)
                    && session.archived
                {
                    self.drop_session_images(session.id.clone());
                }
            }
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

/// The buffer the tree's window shows, never the side window's `*Spaces*`: from the side
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
