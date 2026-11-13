//! What commands ask of the host about agent sessions (ADR 0121), and what the editor keeps
//! for them: the side window (`side`) and the window trees of spaces (`trees`).
//!
//! Must not depend on: the host's threads or I/O. A command asks; the host takes the request
//! after the command, does the work on its sessions worker, and fills an internal buffer with
//! [`Editor::set_internal_text`].

use std::path::PathBuf;
use std::sync::Arc;

use super::Editor;
use crate::buffer::BufferId;
use crate::complete::Candidate;

mod side;
mod trees;

use side::SideWindow;
pub use trees::WindowTree;

/// What the editor keeps for agent sessions.
#[derive(Default)]
pub(super) struct Sessions {
    requests: Vec<SessionRequest>,
    /// The side window, while it is shown.
    pub(super) side: Option<SideWindow>,
    marks: SessionMarks,
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
    /// Show `*Sessions*` in the side window and select it; select it if shown; hide it if
    /// selected (`sessions`).
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
    /// Act on what line `line` of `*Sessions*`, from 0, shows.
    Row { line: usize, action: RowAction },
    /// Group `*Sessions*` by state, or by space.
    Regroup,
    /// Render `*Sessions*` again, with its ages as of now.
    Refresh,
    /// Paste `parts` into a session as the person's bracketed paste, not submitted
    /// (`session-send`, `session-mention`). The host puts them together against the session's
    /// directory, and refuses while the session needs input or does not run.
    Send { to: SendTo, parts: Vec<SendPart> },
    /// Show the session that sent the newest message to the session whose terminal is this
    /// buffer (`session-follow-message`).
    FollowMessage(BufferId),
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

/// What a key of `*Sessions*` does to the row it is on.
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
