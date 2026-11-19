//! What the modeline and the echo area say (`ui.md`, ADR 0063): plain text from the
//! editor's state. The host gives it faces and draws it, and asks for a frame when it
//! would read differently.
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O.
//!
//! Building it reads the buffer only through the cursor's position and the display
//! map's summary, so it costs O(log n) plus a scan of one chunk, as drawing the cursor
//! does, however large the buffer is.
//!
//! The modeline's warnings come first, so that no name can push them off the row. A
//! name gets the columns the rest leaves, down to [`MIN_NAME_CELLS`].
//!
//! For a buffer that may hold secrets it says no position and no number of cursors
//! ([`Status::state`]). It stays out of `get-state` while one is shown, as screenshots
//! do (ADR 0051), and the warnings remain for the person at the screen.

use std::fmt::Write;

use super::Editor;
use super::messages::CUT;
use crate::buffer::{Buffer, Mode, ServerMark, Verdict};
use crate::decode::{FileInfo, LineEnding};
use crate::diagnostic::{Counts, Severity};
use crate::display_map::text_clusters;
use crate::frame::CellRect;
use crate::window::{Window, WindowId};

/// Says that the buffer holds bidirectional controls (`ui.md`, Trojan Source).
const BIDI_WARNING: &str = "[bidi controls] ";
/// Says that the file changed under unsaved edits (ADR 0029).
const DISK_WARNING: &str = "[changed on disk] ";
/// Says that the file is gone from disk, or no longer a file, while the buffer holds its text
/// (ADR 0083).
const GONE_WARNING: &str = "[deleted on disk] ";
/// What the modeline says in place of a position, in a buffer judged to hold secrets, or
/// that may.
const NO_POSITION: &str = "secret";
/// Says that an emergency copy of the file waits for an answer (ADR 0064).
const COPY_WARNING: &str = "[emergency copy] ";
/// The cells a name keeps in a narrow window, at the expense of what follows it.
const MIN_NAME_CELLS: usize = 20;
/// The blank cells after a part of the modeline.
const GAP: &str = "  ";
/// Says that the agent channel is on (ADR 0085).
const AGENT: &str = "Agent  ";
/// Says that a keyboard macro is being recorded (ADR 0113).
const DEFINING: &str = "  Def";
/// What a terminal that hosts an agent session calls its mode (ADR 0132).
const SESSION: &str = "Session";

/// The most cells a project's name takes (ADR 0114).
const PROJECT_CELLS: usize = 24;
/// The most cells the name of the space in front takes (ADR 0159).
const SPACE_CELLS: usize = 20;

/// The cells a mode's note, such as a terminal's title, takes at most (ADR 0120).
const NOTE_CELLS: usize = 40;
/// Says that the buffer's project is not trusted to run its code (ADR 0114).
const RESTRICTED: &str = "restricted  ";

/// How the modeline draws its `lsp` segment (ADR 0138).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ServerFace {
    /// Dim, as `restricted`: the server starts.
    #[default]
    Dim,
    /// As the rest of the modeline: it runs.
    Plain,
    /// In the warning face: it stopped.
    Warning,
}

/// The text of the modeline and the echo area. Comparing two says whether a frame
/// would differ there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `Agent` while the agent channel is on, first on the row (ADR 0085).
    pub agent: &'static str,
    /// An agent is connected, so the host draws [`Self::agent`] bright.
    pub agent_connected: bool,
    /// Agent sessions that need the person, after [`Self::agent`] (ADR 0121): `⚠ 2  ` while
    /// two need input, then `● 3  ` while three are done or ready for review, or as many
    /// blanks while their mark blinks off.
    pub sessions: String,
    /// What the space in front is called, cut to [`SPACE_CELLS`], with a gap after it, in the
    /// selected window's modeline (ADR 0159), or nothing. The host draws it dim.
    pub space: String,
    /// The warnings, each with a blank after it. They come first on the row, and on
    /// the echo row of a frame with no modeline.
    pub warnings: String,
    /// The modified marker and the buffer's name, cut to fit, as they come from disk.
    pub name: String,
    /// The name of the project the buffer's file is in, cut to [`PROJECT_CELLS`], or nothing
    /// (ADR 0114).
    pub project: String,
    /// `restricted` while that project is not trusted, which the host draws dim, or nothing
    /// (ADR 0114). It comes from the record of trusted projects alone, never from a name.
    pub restricted: &'static str,
    /// `lsp`, `lsp 34%` or `lsp!`, after the project, while a language server follows the
    /// buffer, or nothing (ADR 0138), drawn as [`Self::lsp_face`] says.
    pub lsp: String,
    pub lsp_face: ServerFace,
    /// The counts of the buffer's diagnostics that are not zero, ` E2 W5 I1`, then the gap
    /// before what follows, while a server follows the buffer, or nothing (ADR 0138). The host
    /// draws each count in its severity's face, as [`Status::count_parts`] splits them.
    pub lsp_counts: String,
    /// The counts themselves (step 6).
    pub counts: Counts,
    /// Where [`Self::lsp_counts`]' warnings and information start.
    count_starts: [usize; 2],
    /// `waits: server` while what an agent's open or save would have started waits for the
    /// person, which the host draws dim, or nothing (ADR 0118). It comes from the buffer's marks
    /// alone.
    pub waits: String,
    /// Loading progress, the position, the major mode, the encoding and line endings.
    pub state: String,
    /// The echo area's line, as it comes from a message.
    pub echo: String,
    /// The window is the selected one and no prompt is open, so its modeline is drawn active
    /// (ADR 0112).
    pub selected: bool,
    /// A frame has no modeline, so the echo row has its warnings.
    warnings_on_echo_row: bool,
    /// `*Messages*` is in view at this revision, so that a message repeated there asks
    /// for a frame though the echo area stays the same.
    messages_in_view: Option<u64>,
}

impl Status {
    /// The modeline as one string.
    pub fn modeline(&self) -> String {
        format!(
            "{}{}{}{}{}{}{}{}{}{}{}",
            self.agent,
            self.sessions,
            self.warnings,
            self.name,
            self.project,
            self.restricted,
            self.space,
            self.lsp,
            self.lsp_counts,
            self.waits,
            self.state
        )
    }

    /// [`Self::lsp_counts`] in three, each for its severity's face: ` E2`, ` W5`, then ` I1` with
    /// the gap after it, which no face shows. A count that is zero is empty.
    pub fn count_parts(&self) -> [(&str, Severity); 3] {
        let [warnings, information] = self.count_starts;
        // Nothing, rather than a panic, if the text was changed since it was made.
        let part = |range: std::ops::Range<usize>| self.lsp_counts.get(range).unwrap_or_default();
        [
            (part(0..warnings), Severity::Error),
            (part(warnings..information), Severity::Warning),
            (
                part(information..self.lsp_counts.len()),
                Severity::Information,
            ),
        ]
    }

    /// The warnings that go first on the echo row: all of them in a frame with no
    /// modeline, and otherwise none.
    pub fn echo_warnings(&self) -> &str {
        if self.warnings_on_echo_row {
            &self.warnings
        } else {
            ""
        }
    }
}

/// `lsp`, `lsp 34%` or `lsp!` for the server that follows the buffer into `status.lsp`, and
/// the counts that are not zero and the gap after them into `status.lsp_counts` (` E2 W5  `),
/// or nothing in both, with how the state is drawn and the counts (ADR 0138).
fn push_server(status: &mut Status, buffer: &Buffer) {
    let (out, text) = (&mut status.lsp, &mut status.lsp_counts);
    out.clear();
    text.clear();
    let counts = buffer
        .diagnostics()
        .map(crate::diagnostic::Diagnostics::counts)
        .unwrap_or_default();
    status.counts = counts;
    let Some(mark) = buffer.server() else {
        (status.lsp_face, status.count_starts) = (ServerFace::Dim, [0, 0]);
        return;
    };
    match mark {
        ServerMark::Starting => out.push_str("lsp"),
        ServerMark::Ready | ServerMark::Working(None) => out.push_str("lsp"),
        ServerMark::Working(Some(percent)) => {
            let _ = write!(out, "lsp {percent}%");
        }
        ServerMark::Stopped => out.push_str("lsp!"),
    }
    let mut starts = [0; 2];
    for (index, (letter, count)) in [
        ('E', counts.errors),
        ('W', counts.warnings),
        ('I', counts.information),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(start) = index.checked_sub(1).and_then(|at| starts.get_mut(at)) {
            *start = text.len();
        }
        if count > 0 {
            let _ = write!(text, " {letter}{count}");
        }
    }
    text.push_str(GAP);
    status.lsp_face = match mark {
        ServerMark::Starting => ServerFace::Dim,
        ServerMark::Ready | ServerMark::Working(_) => ServerFace::Plain,
        ServerMark::Stopped => ServerFace::Warning,
    };
    status.count_starts = starts;
}

/// `waits: server  ` while something waits in `buffer` for the person (ADR 0118), each name
/// once, or nothing, in place of what `out` held.
fn push_waits(out: &mut String, buffer: &Buffer) {
    out.clear();
    let waits = buffer.waits();
    if waits.is_empty() {
        return;
    }
    out.push_str("waits: ");
    for (n, name) in waits.shown().enumerate() {
        if n > 0 {
            out.push_str(", ");
        }
        out.push_str(name);
    }
    out.push_str(GAP);
}

/// `--`, or `**` once modified, with `%` in the first place for a read-only buffer,
/// as Emacs marks them.
fn marker(buffer: &Buffer) -> &'static str {
    match (buffer.read_only().is_some(), buffer.is_modified()) {
        (false, false) => "--",
        (false, true) => "**",
        (true, false) => "%%",
        (true, true) => "%*",
    }
}

/// Sets `out` to `buffer`'s `marker` and name, cut to `cells`, and a gap: nothing for the
/// spaces pane, whose note names it and says its keys (ADR 0140).
fn push_name(out: &mut String, buffer: &Buffer, marker: &str, cells: usize) {
    out.clear();
    if buffer.mode() == Mode::Sessions && !buffer.mode_note().is_empty() {
        return;
    }
    out.push_str(marker);
    out.push(' ');
    push_cut(out, buffer.name(), cells);
    out.push_str(GAP);
}

/// Adds `name` to `out`, ended by [`CUT`] if it would take more than `cells` cells.
/// A cluster stays whole.
fn push_cut(out: &mut String, name: &str, cells: usize) {
    let mut used = 0;
    let mut keep = 0;
    for cluster in text_clusters(name, 0) {
        used += cluster.cells;
        if used > cells {
            out.push_str(&name[..keep]);
            out.push(CUT);
            return;
        }
        if used < cells {
            keep = cluster.offset + cluster.text.len();
        }
    }
    out.push_str(name);
}

/// The cells `text` takes.
fn text_cells(text: &str) -> usize {
    text_clusters(text, 0).map(|cluster| cluster.cells).sum()
}

/// How far along a load is. It stays under 100 while the load runs, since a file can
/// grow after its size was read.
fn percent(read: u64, total: u64) -> u64 {
    if total == 0 {
        return 99;
    }
    let share = u128::from(read) * 100 / u128::from(total);
    u64::try_from(share).unwrap_or(99).min(99)
}

fn push_encoding(out: &mut String, info: FileInfo, invalid: bool) {
    out.push_str(match (info.binary, invalid, info.bom) {
        (true, _, _) => "Binary",
        (false, true, _) => "UTF-8 (invalid)",
        (false, false, true) => "UTF-8-BOM",
        (false, false, false) => "UTF-8",
    });
    out.push_str(match info.line_ending {
        LineEnding::Lf => " LF",
        LineEnding::Crlf => " CRLF",
    });
}

fn push_state(out: &mut String, window: &Window, buffer: &Buffer) {
    // A terminal has no position, no file and no encoding: its mode and what the host notes,
    // such as its program's title (ADR 0120). A session's says `Session` (ADR 0132).
    if buffer.is_terminal() {
        out.push_str(if buffer.hosts_session() {
            SESSION
        } else {
            buffer.mode().name()
        });
        if !buffer.mode_note().is_empty() {
            out.push_str(GAP);
            push_cut(out, buffer.mode_note(), NOTE_CELLS);
        }
        return;
    }
    // `*Spaces*` has no position worth saying: what the host notes, its name, grouping and
    // keys (ADR 0131, ADR 0140).
    if buffer.mode() == Mode::Sessions && !buffer.mode_note().is_empty() {
        push_cut(out, buffer.mode_note(), NOTE_CELLS);
        return;
    }
    if let Some(progress) = buffer.loading() {
        let _ = match progress.total {
            Some(total) => write!(out, "Loading {}%{GAP}", percent(progress.read, total)),
            None => write!(out, "Loading {} MiB{GAP}", progress.read >> 20),
        };
    }
    match buffer.verdict() {
        // Nothing has loaded, and whether it may hold secrets is not known yet.
        Verdict::Unjudged => {}
        Verdict::Ordinary => {
            let point = window.cursor_point(&buffer.display());
            let _ = write!(out, "{}:{}", point.row + 1, point.column);
            let cursors = window.selections.len();
            if cursors > 1 {
                let _ = write!(out, " ({cursors} cursors)");
            }
            out.push_str(GAP);
        }
        Verdict::Unresolved
        | Verdict::SecretName
        | Verdict::Passphrase
        | Verdict::Yanked
        | Verdict::EchoOff => {
            out.push_str(NO_POSITION);
            out.push_str(GAP);
        }
    }
    out.push_str(buffer.mode().name());
    // A listing has no encoding: its order instead, as dired's `Dired by name` (ADR 0134). A git
    // status has none either (ADR 0127).
    if matches!(
        buffer.mode(),
        crate::buffer::Mode::Directory | crate::buffer::Mode::GitStatus
    ) {
        if !buffer.mode_note().is_empty() {
            out.push(' ');
            push_cut(out, buffer.mode_note(), NOTE_CELLS);
        }
        return;
    }
    out.push_str(GAP);
    push_encoding(out, buffer.file_info(), !buffer.invalid_bytes().is_empty());
}

impl Editor {
    /// Fills in what the modeline of the tiled window `id` and the echo area say now, reusing
    /// `status`'s strings. Segments about the frame, as `Agent`, are only in the modeline of
    /// the selected tiled window, also beneath a prompt (ADR 0112). A window that is not tiled
    /// gets the echo area alone.
    pub fn status_into(&self, id: WindowId, status: &mut Status) {
        // A prompt has no modeline: it speaks of the window beneath (ADR 0072).
        let tiled = self.tiled_id();
        let Some((window, buffer, regions)) = self.window_in_tree(id) else {
            let mut echo = std::mem::take(&mut status.echo);
            echo.clear();
            self.write_echo_line(&mut echo);
            *status = Status {
                echo,
                ..Status::default()
            };
            return;
        };
        let agent = self.agent_channel();
        status.selected = id == tiled && self.prompts.is_empty();
        let frame_segments = self.shows_frame_segments(id, tiled);
        status.agent = if agent == super::AgentChannel::Off || !frame_segments {
            ""
        } else {
            AGENT
        };
        status.agent_connected = frame_segments && agent == super::AgentChannel::Connected;
        status.sessions.clear();
        if frame_segments {
            self.session_marks().write(&mut status.sessions);
        }
        status.warnings.clear();
        if buffer.has_bidi_controls() {
            status.warnings.push_str(BIDI_WARNING);
        }
        if buffer.gone_from_disk() {
            status.warnings.push_str(GONE_WARNING);
        } else if buffer.changed_on_disk() && (buffer.is_modified() || buffer.reload_failed()) {
            status.warnings.push_str(DISK_WARNING);
        }
        if buffer.copy_waits() {
            status.warnings.push_str(COPY_WARNING);
        }
        status.state.clear();
        push_state(&mut status.state, window, buffer);
        self.review_status_into(window.buffer(), &mut status.state);
        // `Def` while a keyboard macro is recorded, as Emacs's (ADR 0113).
        if id == tiled && self.macro_recording() {
            status.state.push_str(DEFINING);
        }
        status.project.clear();
        status.restricted = "";
        if let Some(id) = buffer.project()
            && let Some(project) = self.project(id)
        {
            push_cut(&mut status.project, project.name(), PROJECT_CELLS);
            status.project.push_str(GAP);
            if !self.project_trusted(id) {
                status.restricted = RESTRICTED;
            }
        }

        self.push_front_space(&mut status.space, frame_segments);

        push_server(status, buffer);
        push_waits(&mut status.waits, buffer);
        let marker = marker(buffer);
        // These parts are ASCII, so their bytes are their cells.
        let others = status.agent.len()
            + text_cells(&status.sessions)
            + status.warnings.len()
            + marker.len()
            + 1
            + GAP.len()
            + text_cells(&status.project)
            + status.restricted.len()
            + text_cells(&status.space)
            + status.lsp.len()
            + status.lsp_counts.len()
            + status.waits.len()
            + status.state.len();
        let columns = regions.modeline.columns;
        let cells = columns.saturating_sub(others).max(MIN_NAME_CELLS);
        push_name(&mut status.name, buffer, marker, cells);

        status.echo.clear();
        self.write_echo_line(&mut status.echo);
        status.warnings_on_echo_row = regions.modeline.rows == 0;
        status.messages_in_view =
            (window.buffer() == self.messages_buffer).then(|| buffer.revision());
    }

    /// Writes the name of the space in front, cut, with a gap after it, into `out` when the
    /// window's modeline says what is about the frame (ADR 0159); else leaves it empty.
    fn push_front_space(&self, out: &mut String, frame_segments: bool) {
        out.clear();
        if frame_segments && !self.front_space().is_empty() {
            push_cut(out, self.front_space(), SPACE_CELLS);
            out.push_str(GAP);
        }
    }

    /// Whether the modeline of the window `id` says what is about the frame, as `Agent`: the
    /// selected tiled window's, `tiled`, and while a prompt's float covers that one, the
    /// bottom-most window's too, the left-most of those on the bottom row, so that it stays in
    /// sight (ADR 0085, ADR 0112).
    fn shows_frame_segments(&self, id: WindowId, tiled: WindowId) -> bool {
        if id == tiled {
            return true;
        }
        let Some(float) = self.prompt_layout().map(|layout| layout.regions.outer) else {
            return false;
        };
        let meets = |a: CellRect, b: CellRect| {
            a.row < b.row + b.rows
                && b.row < a.row + a.rows
                && a.column < b.column + b.columns
                && b.column < a.column + a.columns
        };
        let covered = self
            .regions(tiled)
            .is_some_and(|regions| meets(regions.modeline, float));
        let bottom = self
            .frame
            .windows()
            .iter()
            .filter(|(_, regions)| regions.modeline.rows > 0 && regions.modeline.columns > 0)
            .min_by_key(|(_, regions)| {
                let modeline = regions.modeline;
                (std::cmp::Reverse(modeline.row), modeline.column)
            });
        covered && bottom.is_some_and(|(bottom, _)| *bottom == id)
    }

    /// What the echo area says: a message, or a jump's line or isearch's until a message
    /// takes its place for a key (ADR 0082, ADR 0116), into `out`.
    fn write_echo_line(&self, out: &mut String) {
        // An undo's progress comes first while it goes on: what its command said shows once it
        // ends (ADR 0119).
        self.write_crossing_line(out);
        if out.is_empty() {
            out.push_str(self.echo_text());
        }
        if out.is_empty() {
            self.write_jump_line(out);
        }
        if out.is_empty() {
            self.write_isearch_line(out);
            self.write_replace_line(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Replacement;
    use crate::editor::AgentChannel;
    use crate::frame::FrameSize;
    use crate::load::Builder;
    use crate::selection::{Selection, SelectionSet};
    use crate::window::ViewSize;
    use proptest::prelude::*;
    use std::path::PathBuf;

    fn status(editor: &Editor) -> Status {
        let mut status = Status::default();
        editor.status_into(editor.tiled().0.id(), &mut status);
        status
    }

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", text));
        editor.set_view_size(ViewSize::new(10, 80));
        editor
    }

    /// An editor showing a file judged ordinary.
    fn file(name: &str, columns: usize) -> Editor {
        let path = PathBuf::from("/tmp").join(name);
        let mut buffer = Buffer::visiting(path.clone());
        buffer.judge_secret(Some(&[path]));
        let mut editor = Editor::with_buffer(buffer);
        editor.set_frame_size(FrameSize { rows: 10, columns });
        editor
    }

    /// `text` at the end of the selected window's buffer.
    fn type_text(editor: &mut Editor, text: &str) {
        let id = editor.window().buffer();
        let end = editor.current().1.text().len();
        let edit = Replacement {
            range: end..end,
            text,
        };
        editor.apply_edits(id, &[edit]).unwrap();
    }

    #[test]
    fn it_says_the_name_the_position_the_mode_and_the_encoding() {
        let mut editor = editor("ab\ncd");
        editor.current_mut().0.selections = SelectionSet::new(4);
        let status = status(&editor);
        assert_eq!(status.warnings, "");
        assert_eq!(status.name, "-- notes.md  ");
        assert_eq!(status.state, "2:1  Fundamental  UTF-8 LF");
        assert_eq!(status.modeline(), "-- notes.md  2:1  Fundamental  UTF-8 LF");
    }

    #[test]
    fn a_terminal_says_its_mode_and_note_and_no_position_or_encoding() {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.show(terminal);
        assert_eq!(status(&editor).state, "Terminal");
        editor.set_mode_note(terminal, "vim".to_owned());
        let status = status(&editor);
        assert_eq!(status.name, "%% *terminal*  ");
        assert_eq!(status.state, "Terminal  vim");
    }

    #[test]
    fn the_selected_windows_modeline_names_the_space_in_front_cut_by_its_cells() {
        let mut editor = editor("a\n");
        assert_eq!(status(&editor).space, "", "no space in front");
        editor.set_front_space("ログイン 👨\u{200D}👩\u{200D}👧");
        let shown = status(&editor);
        assert_eq!(shown.space, "ログイン 👨\u{200D}👩\u{200D}👧  ");
        assert!(shown.modeline().contains("ログイン"));
        editor.set_front_space(&"界".repeat(30));
        assert_eq!(status(&editor).space, format!("{}…  ", "界".repeat(9)));
        let other = editor.window().id();
        editor.split_window(crate::frame::Way::Down, true).unwrap();
        let mut theirs = Status::default();
        editor.status_into(other, &mut theirs);
        assert_eq!(theirs.space, "", "only the selected window's says it");
    }

    #[test]
    fn the_sessions_list_says_what_the_host_notes_and_no_position() {
        let mut editor = Editor::default();
        let list = editor.add_internal_buffer("*Spaces*", crate::buffer::Mode::Sessions);
        editor.set_internal_text(list, "▾ oso\n");
        editor.show(list);
        assert!(status(&editor).state.starts_with("1:0"), "no note yet");
        editor.set_mode_note(list, "tree · ? keys".to_owned());
        assert_eq!(status(&editor).state, "tree · ? keys");
    }

    #[test]
    fn a_sessions_terminal_says_session_and_is_in_its_directorys_project() {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*claude: oso*");
        editor.show(terminal);
        // Not yet a session's: no project is asked for.
        editor.request_directory_project(terminal, std::path::Path::new("/work/oso"));
        assert!(editor.take_project_requests().is_empty());
        editor.set_hosts_session(terminal, true);
        editor.set_mode_note(terminal, "working".to_owned());
        assert_eq!(status(&editor).state, "Session  working");
        editor.request_directory_project(terminal, std::path::Path::new("/work/oso"));
        let request = editor.take_project_requests().pop().unwrap();
        assert_eq!(
            request.location.parent(),
            Some(std::path::Path::new("/work/oso"))
        );
        let found = crate::project::Found {
            root: crate::project::RootIdentity {
                path: PathBuf::from("/work/oso"),
                identity: Some((1, 2)),
            },
            worktree_of: None,
            checked_main: None,
        };
        editor.project_found(&request, Some(found));
        assert!(editor.current().1.project().is_some());
        // A plain terminal again: no project, and `Terminal`.
        editor.set_hosts_session(terminal, false);
        assert!(editor.current().1.project().is_none());
        assert_eq!(status(&editor).state, "Terminal  working");
    }

    /// `editor`'s shown buffer, put in the project rooted at `root`.
    fn in_project(editor: &mut Editor, root: &str) {
        let id = editor.window().buffer();
        let found = crate::project::Found {
            root: crate::project::RootIdentity {
                path: PathBuf::from(root),
                identity: Some((1, 2)),
            },
            worktree_of: None,
            checked_main: None,
        };
        editor.set_buffer_project(id, Some(found));
    }

    #[test]
    fn the_project_follows_the_name() {
        let mut editor = file("notes.md", 80);
        in_project(&mut editor, "/w/oso");
        assert_eq!(
            status(&editor).modeline(),
            "%% notes.md  oso  restricted  Loading 0 MiB  1:0  Markdown  UTF-8 LF"
        );
        in_project(&mut editor, "/w/a-very-long-project-name-that-goes-on");
        let long = status(&editor).project;
        assert_eq!(long, format!("a-very-long-project-nam{CUT}  "));
    }

    #[test]
    fn a_buffer_in_no_project_shows_none() {
        let editor = file("notes.md", 80);
        assert_eq!(status(&editor).project, "");
        assert_eq!(status(&editor).restricted, "");
    }

    #[cfg(unix)]
    #[test]
    fn restricted_goes_once_the_record_trusts_the_root() {
        let mut editor = file("notes.md", 80);
        in_project(&mut editor, "/w/oso");
        assert_eq!(status(&editor).restricted, RESTRICTED, "not read yet");
        let (list, _) = crate::project::TrustList::parse("/w/oso\tdev=1 ino=3\n");
        editor.set_trust_list(Some(list));
        assert_eq!(status(&editor).restricted, RESTRICTED, "another directory");
        let (list, _) = crate::project::TrustList::parse("/w/oso\tdev=1 ino=2\n");
        editor.set_trust_list(Some(list));
        assert_eq!(status(&editor).restricted, "");
        assert_eq!(status(&editor).project, "oso  ");
        editor.set_trust_list(None);
        assert_eq!(status(&editor).restricted, RESTRICTED, "refused");
    }

    #[test]
    fn the_server_s_segment_has_the_counts_that_are_not_zero_and_no_hints() {
        use crate::diagnostic::{Diagnostics, New, Severity, Tags};
        let mut editor = editor("let x = 1;\n");
        let id = editor.window().buffer();
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.set_server(Some(ServerMark::Ready));
        buffer.set_followed(true);
        assert_eq!(status(&editor).lsp, "lsp");
        assert_eq!(status(&editor).lsp_counts, "  ");
        let new = |severity| New {
            range: 0..1,
            severity,
            tags: Tags::default(),
            source: "",
            code: "",
            message: "m",
        };
        let set = Diagnostics::new(
            "fake",
            [
                new(Severity::Error),
                new(Severity::Error),
                new(Severity::Information),
                new(Severity::Hint),
            ],
            11,
        );
        let buffer = editor.buffer_mut(id).unwrap();
        let revision = buffer.revision();
        assert!(buffer.take_diagnostics(revision, set));
        let shown = status(&editor);
        assert_eq!(shown.lsp, "lsp");
        assert_eq!(shown.lsp_counts, " E2 I1  ");
        assert_eq!(
            shown.count_parts(),
            [
                (" E2", Severity::Error),
                ("", Severity::Warning),
                (" I1  ", Severity::Information)
            ]
        );
        assert!(shown.modeline().contains("lsp E2 I1  "));
        assert_eq!(
            shown.counts,
            Counts {
                errors: 2,
                warnings: 0,
                information: 1
            }
        );
        editor.buffer_mut(id).unwrap().set_followed(false);
        assert_eq!(status(&editor).lsp_counts, "  ", "gone with the server");
    }

    #[cfg(unix)]
    #[test]
    fn what_waits_for_the_person_follows_restricted_and_goes_when_let_go() {
        use crate::buffer::{Acted, Start, Starts};
        let mut editor = file("main.rs", 80);
        in_project(&mut editor, "/w/oso");
        let (list, _) = crate::project::TrustList::parse("/w/oso\tdev=1 ino=2\n");
        editor.set_trust_list(Some(list));
        let id = editor.window().buffer();
        assert_eq!(status(&editor).waits, "");
        editor.starts_on_open(id, Starts::of(Start::ServerStart), Acted::Agent);
        editor.starts_on_save(id, Starts::of(Start::ServerSave), Acted::Agent);
        assert_eq!(status(&editor).waits, "waits: server  ", "each name once");
        assert!(
            status(&editor)
                .modeline()
                .contains("oso  waits: server  Loading")
        );
        editor.note_persons_command();
        assert_eq!(
            status(&editor).waits,
            "waits: server  ",
            "the save's still waits"
        );
        editor.starts_on_save(id, Starts::NONE, Acted::Person);
        assert_eq!(status(&editor).waits, "");
    }

    #[test]
    fn the_agent_channel_shows_first_while_it_is_on() {
        let mut editor = editor("ab");
        assert_eq!(status(&editor).agent, "");
        editor.set_agent_channel(AgentChannel::On);
        let on = status(&editor);
        assert_eq!(on.agent, "Agent  ");
        assert!(!on.agent_connected);
        assert!(on.modeline().starts_with("Agent  -- notes.md  "));
        editor.set_agent_channel(AgentChannel::Connected);
        assert!(status(&editor).agent_connected);
        assert_ne!(status(&editor), on);
    }

    #[test]
    fn the_marker_follows_the_buffer() {
        let mut editor = editor("ab");
        type_text(&mut editor, "c");
        assert!(status(&editor).name.starts_with("** "));
        editor.undo().unwrap();
        assert!(status(&editor).name.starts_with("-- "));
        let mut editor = Editor::with_buffer(Buffer::from_bytes("bad", b"\xff"));
        assert!(status(&editor).name.starts_with("%% "));
        editor.set_view_size(ViewSize::new(2, 40));
        assert_eq!(
            status(&editor).state,
            "1:0  Fundamental  UTF-8 (invalid) LF"
        );
    }

    #[test]
    fn several_cursors_are_counted() {
        let mut editor = editor("abc");
        let selection = |cursor| Selection::at(cursor);
        let cursors = vec![selection(0), selection(1), selection(2)];
        editor.current_mut().0.selections = SelectionSet::from_parts(cursors, 1).unwrap();
        assert_eq!(
            status(&editor).state,
            "1:1 (3 cursors)  Fundamental  UTF-8 LF"
        );
    }

    #[test]
    fn encoding_and_line_endings_are_the_files() {
        let editor = Editor::with_buffer(Buffer::from_bytes("a", b"\xef\xbb\xbfa\r\n"));
        assert_eq!(status(&editor).state, "1:0  Fundamental  UTF-8-BOM CRLF");
        let editor = Editor::with_buffer(Buffer::from_bytes("b", b"a\0"));
        assert_eq!(status(&editor).state, "1:0  Fundamental  Binary LF");
    }

    #[test]
    fn loading_says_how_far_and_stays_under_all_of_it() {
        let mut editor = file("big.txt", 80);
        let id = editor.window().buffer();
        let mut builder = Builder::new(Some(8), crate::display_map::TabWidth::default());
        builder.push(b"abcd");
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.apply_load(builder.snapshot());
        let state = status(&editor).state;
        assert!(state.starts_with("Loading 50%  "), "{state}");
        builder.push(b"efghijkl");
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.apply_load(builder.snapshot());
        assert!(status(&editor).state.starts_with("Loading 99%  "));
        builder.finish();
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.apply_load(builder.snapshot());
        assert!(status(&editor).state.starts_with("1:0  "));
        assert_eq!(percent(0, 0), 99);
        assert_eq!(percent(u64::MAX, u64::MAX), 99);
        assert_eq!(percent(u64::MAX / 2, u64::MAX), 49);
    }

    #[test]
    fn a_load_of_no_known_size_says_how_much() {
        let mut editor = file("pipe", 80);
        let id = editor.window().buffer();
        let mut builder = Builder::new(None, crate::display_map::TabWidth::default());
        builder.push(&[b'a'; 3 << 20]);
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.apply_load(builder.snapshot());
        assert!(status(&editor).state.starts_with("Loading 3 MiB  "));
    }

    #[test]
    fn bidi_controls_warn_while_they_are_in_the_text() {
        let mut editor = editor("ab");
        assert_eq!(status(&editor).warnings, "");
        type_text(&mut editor, "\u{202e}");
        assert_eq!(status(&editor).warnings, BIDI_WARNING);
        editor.undo().unwrap();
        assert_eq!(status(&editor).warnings, "");
    }

    #[test]
    fn a_file_changed_on_disk_warns_only_under_unsaved_edits() {
        let mut editor = editor("ab");
        let id = editor.window().buffer();
        editor.file_changed_on_disk(id);
        assert_eq!(status(&editor).warnings, "", "nothing to lose");
        type_text(&mut editor, "c");
        assert_eq!(status(&editor).warnings, DISK_WARNING);
        type_text(&mut editor, "\u{2066}");
        assert_eq!(
            status(&editor).warnings,
            format!("{BIDI_WARNING}{DISK_WARNING}")
        );
    }

    #[test]
    fn a_long_name_is_cut_and_the_rest_stays_in_view() {
        let name = "n".repeat(200);
        let status = status(&file(&name, 80));
        assert_eq!(status.state, "Loading 0 MiB  1:0  Fundamental  UTF-8 LF");
        let cells = status.modeline().chars().count();
        assert_eq!(cells, 80);
        assert!(status.name.ends_with("n…  "));
    }

    #[test]
    fn a_narrow_window_keeps_some_of_the_name_and_cuts_what_follows() {
        let name = "n".repeat(200);
        let status = status(&file(&name, 24));
        assert_eq!(status.name.chars().count(), 3 + MIN_NAME_CELLS + 2);
    }

    #[test]
    fn a_name_is_cut_between_clusters_by_their_cells() {
        let cut = |name: &str, cells: usize| {
            let mut out = String::new();
            push_cut(&mut out, name, cells);
            out
        };
        assert_eq!(cut("abc", 3), "abc", "just fits");
        assert_eq!(cut("abcd", 3), "ab…");
        assert_eq!(cut("日本語", 4), "日…", "a wide character stays whole");
        assert_eq!(cut("e\u{301}e\u{301}e\u{301}", 2), "e\u{301}…");
        assert_eq!(cut("a\nb", 2), "a…", "the escape takes two cells");
        assert_eq!(cut("a\nb", 4), "a\nb");
        assert_eq!(cut("abc", 0), "…");
    }

    #[test]
    fn a_buffer_that_may_hold_secrets_says_no_position_and_no_cursors() {
        let mut editor = Editor::with_buffer(super::super::test_buffers::secret_buffer());
        editor.set_view_size(ViewSize::new(10, 80));
        let id = editor.window().buffer();
        let mut builder = Builder::new(None, crate::display_map::TabWidth::default());
        builder.push(b"KEY=value\n");
        builder.finish();
        editor
            .buffer_mut(id)
            .unwrap()
            .apply_load(builder.snapshot());
        let selection = |cursor| Selection::at(cursor);
        let cursors = vec![selection(1), selection(4)];
        editor.current_mut().0.selections = SelectionSet::from_parts(cursors, 0).unwrap();
        let status = status(&editor);
        assert_eq!(status.state, "secret  Fundamental  UTF-8 LF");
        assert_eq!(status.name, "-- .env  ");
    }

    #[test]
    fn a_file_that_could_not_be_judged_counts_as_secret() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/notes.txt"));
        buffer.judge_unresolved(&[]);
        let editor = Editor::with_buffer(buffer);
        assert_eq!(
            status(&editor).state,
            "Loading 0 MiB  secret  Fundamental  UTF-8 LF"
        );
    }

    #[test]
    fn an_unjudged_file_says_no_position_and_does_not_say_secret() {
        let path = PathBuf::from("/tmp/notes.txt");
        let mut editor = Editor::with_buffer(Buffer::visiting(path.clone()));
        editor.set_view_size(ViewSize::new(10, 80));
        assert_eq!(
            status(&editor).state,
            "Loading 0 MiB  Fundamental  UTF-8 LF"
        );
        let id = editor.window().buffer();
        editor.buffer_mut(id).unwrap().judge_secret(Some(&[path]));
        assert_eq!(
            status(&editor).state,
            "Loading 0 MiB  1:0  Fundamental  UTF-8 LF"
        );
    }

    #[test]
    fn the_echo_area_shows_the_newest_until_the_next_key() {
        let mut editor = editor("");
        assert_eq!(status(&editor).echo, "");
        editor.message("one");
        editor.message("two");
        assert_eq!(status(&editor).echo, "two");
        editor.clear_echo();
        assert_eq!(status(&editor).echo, "");
    }

    #[test]
    fn the_keys_in_progress_come_ahead_of_a_message() {
        let mut editor = editor("");
        editor.message("Saved");
        editor.set_keys_in_progress("C-x-");
        assert_eq!(status(&editor).echo, "C-x-");
        editor.set_keys_in_progress("");
        assert_eq!(status(&editor).echo, "Saved");
    }

    #[test]
    fn a_message_is_there_before_the_first_frame() {
        let mut editor = Editor::default();
        editor.message("No emergency copy can be written if oso crashes");
        assert_eq!(
            status(&editor).echo,
            "No emergency copy can be written if oso crashes"
        );
    }

    proptest! {
        /// Whatever the name and the width, the warnings come first and whole, and the
        /// name stays within what it was given.
        #[test]
        fn warnings_come_first_and_a_name_never_passes_its_cells(
            name in "[a-z日\u{301}\u{202e}\n]{1,60}",
            columns in 0usize..200,
        ) {
            let mut editor = Editor::with_buffer(Buffer::from_text(name.clone(), "\u{202e}"));
            editor.set_frame_size(FrameSize { rows: 10, columns });
            let status = status(&editor);
            prop_assert_eq!(status.warnings.as_str(), BIDI_WARNING);
            let cells: usize = text_clusters(&status.name, 0).map(|c| c.cells).sum();
            let budget = columns.saturating_sub(BIDI_WARNING.len() + status.state.len() + 5);
            prop_assert!(cells <= 3 + budget.max(MIN_NAME_CELLS) + 2);
        }
    }
}
