//! What commands ask of the host about agent sessions (ADR 0121), and what the editor keeps
//! for them: the side window (`side`), the window trees of spaces (`trees`) and the windows
//! docked under a native session's transcript (`docks`, ADR 0170).
//!
//! Must not depend on: the host's threads or I/O. A command asks; the host takes the request
//! after the command, does the work on its sessions worker or its worktree jobs, and fills an
//! internal buffer with [`Editor::set_internal_text`].

use std::path::PathBuf;
use std::sync::Arc;

use super::Editor;
use crate::buffer::BufferId;
use crate::complete::Candidate;
use crate::diagnostic::Severity;

mod docks;
mod native;
mod side;
mod trees;

pub(super) use native::takes_session_flags;
use side::SideWindow;
pub use trees::WindowTree;

/// What the editor keeps for agent sessions.
#[derive(Default)]
pub(super) struct Sessions {
    requests: Vec<SessionRequest>,
    /// The side window, while it is shown.
    pub(super) side: Option<SideWindow>,
    /// What the docked windows asked and where they went at the last layout (ADR 0170).
    pub(super) docks: docks::Docks,
    marks: SessionMarks,
    /// What the space in front is called, as the person sees it, for the selected window's
    /// modeline (ADR 0159); empty with none.
    space: String,
    /// What the host gave for each list an [`ArgSpec::Listed`](crate::command::ArgSpec)
    /// completes over.
    listed: Vec<(&'static str, Arc<[Candidate]>)>,
}

/// What the selected window's modeline says of the sessions that need the person (ADR 0121).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionMarks {
    /// How many need input: `⚠ N`.
    pub needs_input: u16,
    /// How many are done or ready for review: `● N`.
    pub done: u16,
    /// The marks blink off: blanks as wide stand in for them, so nothing after them moves.
    pub dark: bool,
}

impl SessionMarks {
    /// Writes the modeline's segment into `out`, each mark with two blanks after it.
    pub(super) fn write(self, out: &mut String) {
        use std::fmt::Write as _;
        let start = out.len();
        for (mark, count) in [('⚠', self.needs_input), ('●', self.done)] {
            if count > 0 {
                let _ = write!(out, "{mark} {count}  ");
            }
        }
        if self.dark {
            // Each mark takes one cell, as its digits and blanks do.
            let cells = out[start..].chars().count();
            out.truncate(start);
            out.extend(std::iter::repeat_n(' ', cells));
        }
    }
}

/// What a command asks of the host about agent sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    /// Explain the saved screen in this file with the rules in force (`session-explain-file`).
    ExplainFile(PathBuf),
    /// Start a session (`claude`): in `directory`, else the selected window's project root,
    /// else the buffer's directory, named `name`, else after its directory.
    Start {
        name: Option<String>,
        directory: Option<PathBuf>,
    },
    /// Resume the stopped session whose terminal is this buffer (`session-resume`).
    Resume(BufferId),
    /// Copy the conversation of the session whose terminal is this buffer under a new id, in a
    /// new buffer (`session-fork`).
    Fork(BufferId),
    /// Resume the conversation this `session-adopt` candidate names, which runs or ran outside
    /// oso, as a hosted session in a new buffer (`session-adopt`).
    Adopt(String),
    /// Go to the conversation of the session `session`, at its message `message`, which
    /// `session-search`'s row gave (ADR 0145): show it if oso hosts it, else adopt it as
    /// `session-adopt` does.
    GoToConversation { session: String, message: usize },
    /// Read the conversation of the session `session` in `*conversation: NAME*`, at its
    /// message `message` (ADR 0145, step 2).
    ReadConversation { session: String, message: usize },
    /// Fork the conversation of the session `session`: resume a copy of it under a new id in a
    /// new hosted session (ADR 0145, step 2).
    ForkConversation { session: String },
    /// Open `*Brief*` for a new worktree session on `branch`, a new one starting at `base`
    /// (`session-worktree`).
    Worktree {
        branch: String,
        base: Option<String>,
    },
    /// Send the brief in this `*Brief*` buffer: make the worktree and start its session
    /// (`brief-send`, `C-c C-c`).
    SendBrief(BufferId),
    /// Drop this `*Brief*` buffer unsent (`brief-cancel`, `C-c C-k`).
    CancelBrief(BufferId),
    /// List the selected project's linked worktrees in `*Worktrees*` (`worktree-list`).
    WorktreeList,
    /// Remove a linked worktree once every check holds: this directory, else the row at point
    /// in `*Worktrees*`, else the selected buffer's worktree (`worktree-remove`).
    WorktreeRemove(Option<PathBuf>),
    /// The person's `y` to the removal's question (`worktree-remove-confirmed`).
    WorktreeRemoveConfirmed,
    /// Stop a running removal: the one at point in `*Worktrees*`, else the only one
    /// (`worktree-remove-stop`).
    WorktreeRemoveStop,
    /// The person's `y` to the stop's warning (`worktree-remove-stop-confirmed`).
    WorktreeRemoveStopConfirmed,
    /// The person's `y` to "another session runs in this directory" (`session-start-shared`).
    StartShared,
    /// The person's `?` in a question the agent caused: show what it would run
    /// (`agent-question-details`).
    QuestionDetails,
    /// Let the sessions the person started start sessions without asking, as the configuration
    /// says, or make every start ask until oso restarts (`agent-starts-allow`,
    /// `agent-starts-ask`).
    AgentStarts(bool),
    /// Show `*Spaces*` in the side window and select it; select it if shown; while it is
    /// selected, select the tree's window selected last (`sessions`, ADR 0140).
    ShowList,
    /// Show the session whose buffer has this name, switching to its space (`session-switch`).
    Switch(String),
    /// Show the session that has needed input longest (`session-next-waiting`).
    NextWaiting,
    /// Show the session done or ready for review longest (`session-next-done`).
    NextDone,
    /// Show the space of this name (`space-switch`).
    Space(String),
    /// The selected window alone, and back (`space-zoom`).
    Zoom,
    /// Explain the last look at the screen of the session whose terminal is this buffer
    /// (`session-explain`).
    Explain(BufferId),
    /// Act on what line `line` of `*Spaces*`, from 0, shows.
    Row { line: usize, action: RowAction },
    /// Act on the session whose terminal is this buffer, as on its row (`C-c K`, `C-c P`,
    /// `C-c A` in session mode, ADR 0132).
    OnBuffer(BufferId, RowAction),
    /// Act on the row of native session buffer `buffer` that holds its line `line`, from 0
    /// (ADR 0165).
    Native {
        buffer: BufferId,
        line: usize,
        action: NativeAction,
    },
    /// Give the session whose rename the host asked about last this name (`session-rename-to`,
    /// the answer to `session-rename`'s prompt, ADR 0155).
    RenameTo(String),
    /// Group `*Spaces*` by state, or by space.
    Regroup,
    /// Fold `*Spaces*` to its spaces, then open it down to its worktrees, then everything, in
    /// turn (`sessions-cycle-folds`, ADR 0140).
    CycleFolds,
    /// Make `*Spaces*`' pane this many columns wider, narrower if negative, for this oso
    /// instance (`sessions-wider`, `sessions-narrower`, ADR 0131).
    PaneWidth(isize),
    /// Render `*Spaces*` again, with its ages as of now.
    Refresh,
    /// Paste `parts` into a session as the person's bracketed paste, not submitted
    /// (`session-send`, `session-mention`). The host puts them together against the session's
    /// directory, and refuses while the session needs input or does not run.
    Send { to: SendTo, parts: Vec<SendPart> },
    /// Show the session that sent the newest message to the session whose terminal is this
    /// buffer (`session-follow-message`).
    FollowMessage(BufferId),
    /// Keep the clipboard's image in a file and paste an `@` reference to it into the session
    /// whose terminal is this buffer, not submitted (`session-yank-image`, ADR 0142).
    YankImage(BufferId),
    /// Act on the issue spaces (ADR 0136).
    Spaces(SpaceRequest),
}

/// What a command asks of the host about issue spaces (ADR 0136). Each acts on the space in
/// front, but for a new one and one reopened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceRequest {
    /// Make a space for what this description says, of the selected buffer's repository, enter
    /// it and start its orchestrator (`space-new`, ADR 0159).
    New(String),
    /// Add this repository, a path or a listed one's text (`space-add-repo`).
    AddRepo(String),
    /// Take out the repository of this name (`space-remove-repo`).
    RemoveRepo(String),
    /// Replace the space in front's orchestrator by a new one, which takes over from it
    /// (`space-orchestrator-replace`).
    ReplaceOrchestrator,
    /// Use the repository of this name of the space in front for the project commands run
    /// from its orchestrator's buffer (`space-use-repo`).
    UseRepo(String),
    /// Archive the space (`space-close`).
    Close,
    /// Open the archived space of this name again, and enter it (`space-reopen`).
    Reopen(String),
    /// Open `space-rename-to`'s prompt holding the display name of the space in front
    /// (`space-rename`, ADR 0159).
    AskRename,
    /// Give the space `space-rename` asked about this display name; its slug stays
    /// (`space-rename-to`, the answer to that prompt, ADR 0159).
    Rename(String),
    /// Edit the space's instructions (`space-instructions`).
    Instructions,
    /// Show the space's own directory (`space-directory`).
    Directory,
    /// Start the space's orchestrator in its directory, made again first where it has gone
    /// when `remake_directory` (`space-orchestrator-start`, ADR 0136 step 3).
    StartOrchestrator { remake_directory: bool },
    /// The person answered `y` to the space question the host asked last (`space-confirm`).
    Confirmed,
}

/// Which session a send goes to (ADR 0121).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendTo {
    /// The session whose buffer has this name, as the send list completes it.
    Named(String),
    /// The session whose terminal is this buffer.
    Buffer(BufferId),
}

/// One part of what is sent to a session (ADR 0121).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendPart {
    /// Text the person wrote, sent as it is.
    Note(String),
    /// Lines of a buffer: `@region` with their text, `@file:L-L` without it.
    Excerpt(SendExcerpt),
    /// A language server's diagnostics of a buffer or its region, `@diagnostics`, one line each
    /// (ADR 0121, ADR 0138, step 11).
    Diagnostics(Vec<SendDiagnostic>),
    /// The diagnostic at point, its lines around and the words that ask to fix it
    /// (`session-fix-error`, ADR 0121, ADR 0138, step 11).
    FixError {
        diagnostic: SendDiagnostic,
        around: SendExcerpt,
    },
}

/// A language server's diagnostic of a buffer, as a send gives it: where it starts, its line
/// and column from 1, the column in characters, its severity and its message (ADR 0138,
/// step 11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendDiagnostic {
    /// The file the buffer visits; `None` for a buffer that visits none.
    pub path: Option<PathBuf>,
    /// The buffer's name.
    pub buffer: String,
    pub line: usize,
    /// From 1, in characters; 0 where the part shows no column, as `@diagnostics` does.
    pub column: usize,
    pub severity: Severity,
    /// The server's whole message, which a send cuts and escapes.
    pub message: String,
}

/// Lines `first..=last`, counted from 1, of a buffer (ADR 0121).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendExcerpt {
    /// The file the buffer visits; `None` for a buffer that visits none.
    pub path: Option<PathBuf>,
    /// The buffer's name.
    pub buffer: String,
    pub first: usize,
    pub last: usize,
    /// The lines' text; `None` sends the place only: with `C-u`, or from a buffer that may
    /// hold secrets.
    pub text: Option<String>,
}

/// What a key of `*Spaces*` does to the row it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAction {
    /// Show the session in the tree's window selected last, and select that window.
    Show,
    /// Fold or open the group, or the sessions nested under the session.
    Fold,
    /// Pin or unpin the session: pinned ones come first in their group.
    Pin,
    /// Archive the session, or bring an archived one back.
    Archive,
    /// Resume the stopped session.
    Resume,
    /// Hang up the session's terminal, asking first while it works or needs input.
    Stop,
    /// Explain the last look at the session's screen.
    Explain,
    /// Read a new name for the session, its current one in the prompt (`session-rename`,
    /// ADR 0155).
    Rename,
}

/// What a key of a native session's buffer does at the row it is on (ADR 0165, step 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAction {
    /// Fold or unfold the row, or in a row of several calls shown open, the call.
    Toggle,
    /// Show the whole result of the call, read-only, in `*result: NAME*`.
    Open,
    /// Move to the start of the person's message this many after, before if negative.
    Prompt(isize),
    /// Move to the start of the tool row this many after, before if negative.
    Tool(isize),
    /// From the composer (ADR 0165, step 2): send its text, or queue it while a turn runs;
    /// while a queued message is edited there, put it back in its place.
    Send,
    /// From the composer: interrupt the turn, then send its text ahead of the queue.
    SendNow,
    /// From the composer: give up the edit of a queued message, or interrupt the turn.
    GiveUp,
    /// From the composer: the session's message sent this many before, after if negative.
    History(isize),
    /// From the composer: back to the transcript.
    Transcript,
    /// From the transcript: to the composer.
    Compose,
    /// To the first queued message's row.
    Queue,
    /// On a queued message's row: edit it in the composer, in its place.
    EditQueued,
    /// On a queued message's row: drop it.
    DropQueued,
    /// On a queued message's row: move it this many places, up if negative.
    MoveQueued(isize),
}

impl Editor {
    /// A command asks the host for `request`.
    pub fn request_session(&mut self, request: SessionRequest) {
        self.sessions.requests.push(request);
    }

    /// Sets what the selected window's modeline says of the sessions that need the person.
    /// The host asks for a frame itself.
    pub fn set_session_marks(&mut self, marks: SessionMarks) {
        self.sessions.marks = marks;
    }

    /// What the selected window's modeline says of the sessions that need the person.
    pub(crate) fn session_marks(&self) -> SessionMarks {
        self.sessions.marks
    }

    /// Sets what the selected window's modeline calls the space in front (ADR 0159): its
    /// display name, cleaned by the host; empty for none. The host asks for a frame itself.
    pub fn set_front_space(&mut self, label: &str) {
        if self.sessions.space != label {
            label.clone_into(&mut self.sessions.space);
        }
    }

    /// What the selected window's modeline calls the space in front.
    pub(crate) fn front_space_label(&self) -> &str {
        &self.sessions.space
    }

    /// Sets the candidates of `list`, which an
    /// [`ArgSpec::Listed`](crate::command::ArgSpec) prompt over it completes over from its next
    /// opening.
    pub fn set_listed(&mut self, list: &'static str, items: Arc<[Candidate]>) {
        match self
            .sessions
            .listed
            .iter_mut()
            .find(|(name, _)| *name == list)
        {
            Some((_, kept)) => *kept = items,
            None => self.sessions.listed.push((list, items)),
        }
    }

    /// The candidates the host last gave for `list`, or none.
    pub fn listed_candidates(&self, list: &str) -> Arc<[Candidate]> {
        self.sessions
            .listed
            .iter()
            .find_map(|(name, items)| (*name == list).then(|| Arc::clone(items)))
            .unwrap_or_else(|| Arc::from([]))
    }

    /// What commands asked about sessions since the host last looked, oldest first.
    pub fn take_session_requests(&mut self) -> Vec<SessionRequest> {
        std::mem::take(&mut self.sessions.requests)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{SessionMarks, SessionRequest};
    use crate::editor::{Editor, Status};
    use crate::frame::Way;

    fn modeline(editor: &Editor, id: crate::window::WindowId) -> String {
        let mut status = Status::default();
        editor.status_into(id, &mut status);
        status.sessions
    }

    #[test]
    fn the_marks_are_on_the_selected_windows_modeline_only() {
        let mut editor = Editor::default();
        editor.set_view_size(crate::window::ViewSize::new(20, 80));
        let first = editor.window().id();
        let other = editor.split_window(Way::Down, false).unwrap();
        assert_eq!(modeline(&editor, first), "");
        let marks = SessionMarks {
            needs_input: 2,
            done: 13,
            dark: false,
        };
        editor.set_session_marks(marks);
        assert_eq!(editor.session_marks(), marks);
        assert_eq!(modeline(&editor, first), "⚠ 2  ● 13  ");
        assert_eq!(modeline(&editor, other), "");
        editor.set_session_marks(SessionMarks {
            needs_input: 0,
            ..marks
        });
        assert_eq!(modeline(&editor, first), "● 13  ");
        let mut status = Status::default();
        editor.status_into(first, &mut status);
        assert!(status.modeline().starts_with("● 13  "));
    }

    #[test]
    fn a_list_keeps_what_the_host_gave_last() {
        use crate::complete::Candidate;
        let mut editor = Editor::default();
        assert!(editor.listed_candidates("sessions").is_empty());
        let one = |text: &str| -> std::sync::Arc<[Candidate]> {
            vec![Candidate::new(text, "", "needs input")].into()
        };
        editor.set_listed("sessions", one("*claude: a*"));
        editor.set_listed("spaces", one("oso"));
        editor.set_listed("sessions", one("*claude: b*"));
        assert_eq!(
            editor.listed_candidates("sessions")[0].text(),
            "*claude: b*"
        );
        assert_eq!(editor.listed_candidates("spaces")[0].text(), "oso");
    }

    #[test]
    fn dark_marks_leave_blanks_as_wide() {
        let mut editor = Editor::default();
        editor.set_view_size(crate::window::ViewSize::new(20, 80));
        let id = editor.window().id();
        editor.set_session_marks(SessionMarks {
            needs_input: 1,
            done: 0,
            dark: true,
        });
        assert_eq!(modeline(&editor, id), " ".repeat("⚠ 1  ".chars().count()));
    }

    #[test]
    fn requests_are_taken_once_in_order() {
        let mut editor = Editor::default();
        assert!(editor.take_session_requests().is_empty());
        editor.request_session(SessionRequest::ExplainFile(PathBuf::from("a")));
        editor.request_session(SessionRequest::ExplainFile(PathBuf::from("b")));
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::ExplainFile(PathBuf::from("a")),
                SessionRequest::ExplainFile(PathBuf::from("b")),
            ]
        );
        assert!(editor.take_session_requests().is_empty());
    }
}
