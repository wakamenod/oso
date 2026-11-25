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
use std::ops::Range;

use self::state::push_state;
use super::Editor;
use super::messages::CUT;
use crate::buffer::{Buffer, Mode, ServerMark};
use crate::diagnostic::{Counts, Severity};
use crate::display_map::text_clusters;
use crate::frame::CellRect;
use crate::window::WindowId;

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
/// What a window too short for its docked windows says (ADR 0170).
const DOCKS_HIDDEN: &str = "[docked hidden] ";
/// The cells a name keeps in a narrow window, at the expense of what follows it.
const MIN_NAME_CELLS: usize = 20;
/// The blank cells after a part of the modeline.
const GAP: &str = "  ";
/// Says that the agent channel is on (ADR 0085).
const AGENT: &str = "Agent  ";
/// Says that a dictation records (ADR 0175).
const RECORDING: &str = "● REC  ";
/// Says that a dictation stopped recording and waits for its text (ADR 0175).
const WAITING: &str = "…  ";
/// Says that Claude fixes a dictation's text (ADR 0175, step 2).
const FIXING: &str = "fix  ";
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
    /// `● REC` while a dictation records, then `…` while it waits for its text, and `fix`
    /// while Claude fixes one, with a gap after it, after [`Self::agent`], or nothing (ADR 0175).
    pub dictation: &'static str,
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
    /// Where in [`Self::echo`] text is composed in isearch, in bytes: what an input method is
    /// composing, after the search string, or empty while it composes nothing (ADR 0158).
    /// `None` while the line is not isearch's.
    pub composed: Option<Range<usize>>,
    /// Its modeline is drawn active (ADR 0112): the window is the selected one, or the host of
    /// the docked one that is (ADR 0170), and no prompt is open.
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
            "{}{}{}{}{}{}{}{}{}{}{}{}",
            self.agent,
            self.dictation,
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

/// Cuts the start of `out` and puts [`CUT`] there if it takes more than `cells` cells, so that
/// its end shows; a cluster stays whole.
pub(super) fn keep_end(out: &mut String, cells: usize) {
    let all = text_cells(out);
    if all <= cells {
        return;
    }
    // One cell is the mark's.
    let over = all + 1 - cells;
    let kept = text_clusters(out, 0)
        .find(|cluster| cluster.column >= over)
        .map_or(out.len(), |cluster| cluster.offset);
    out.replace_range(..kept, "");
    if cells > 0 {
        out.insert(0, CUT);
    }
}

/// The cells `text` takes: its bytes when they are all printable ASCII, each a cell.
fn text_cells(text: &str) -> usize {
    if text.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return text.len();
    }
    text_clusters(text, 0).map(|cluster| cluster.cells).sum()
}

/// The warnings a modeline starts with, of `buffer`, into `out`; `docks_hidden` if the window is
/// too short for the windows docked under it (ADR 0170).
fn warnings_into(buffer: &crate::buffer::Buffer, docks_hidden: bool, out: &mut String) {
    out.clear();
    if buffer.has_bidi_controls() {
        out.push_str(BIDI_WARNING);
    }
    if buffer.gone_from_disk() {
        out.push_str(GONE_WARNING);
    } else if buffer.changed_on_disk() && (buffer.is_modified() || buffer.reload_failed()) {
        out.push_str(DISK_WARNING);
    }
    if buffer.copy_waits() {
        out.push_str(COPY_WARNING);
    }
    if docks_hidden {
        out.push_str(DOCKS_HIDDEN);
    }
}

impl Editor {
    /// Fills in what the modeline of the tiled window `id` and the echo area say now, reusing
    /// `status`'s strings. Segments about the frame, as `Agent`, are only in the modeline of
    /// the selected tiled window, also beneath a prompt (ADR 0112). A window that is not tiled
    /// gets the echo area alone.
    pub fn status_into(&self, id: WindowId, status: &mut Status) {
        // A prompt has no modeline: it speaks of the window beneath (ADR 0072). Nor has a
        // docked window: its host's speaks for it while it is selected (ADR 0170).
        let tiled = if self.dock_selected() {
            self.tree_id()
        } else {
            self.tiled_id()
        };
        let Some((window, buffer, regions)) = self.window_in_tree(id) else {
            let mut echo = std::mem::take(&mut status.echo);
            echo.clear();
            let composed = self.write_echo_line(self.frame.echo.columns, &mut echo);
            *status = Status {
                echo,
                composed,
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
        status.dictation = match self.dictation_mark() {
            _ if !frame_segments => "",
            super::DictationMark::Off => "",
            super::DictationMark::Recording => RECORDING,
            super::DictationMark::Waiting => WAITING,
            super::DictationMark::Fixing => FIXING,
        };
        status.sessions.clear();
        if frame_segments {
            self.session_marks().write(&mut status.sessions);
        }
        warnings_into(buffer, self.docks_hidden(id), &mut status.warnings);
        status.state.clear();
        let shows_footer = self.shows_footer(id);
        let note = push_state(&mut status.state, window, buffer, shows_footer);
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
            + text_cells(status.dictation)
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
            + text_cells(&status.state);
        let columns = regions.modeline.columns;
        let cells = note::name_cells(status, note, others, columns);
        push_name(&mut status.name, buffer, marker, cells);

        status.warnings_on_echo_row = regions.modeline.rows == 0 && window.docked().is_none();
        self.echo_into(status);
        status.messages_in_view =
            (window.buffer() == self.messages_buffer).then(|| buffer.revision());
    }

    /// Fills in the echo area's line, after the warnings that go first on its row.
    fn echo_into(&self, status: &mut Status) {
        let columns = (self.frame.echo.columns).saturating_sub(text_cells(status.echo_warnings()));
        status.echo.clear();
        status.composed = self.write_echo_line(columns, &mut status.echo);
    }

    /// Writes the name of the space in front, cut, with a gap after it, into `out` when the
    /// window's modeline says what is about the frame (ADR 0159); else leaves it empty.
    fn push_front_space(&self, out: &mut String, frame_segments: bool) {
        out.clear();
        if frame_segments && !self.front_space_label().is_empty() {
            push_cut(out, self.front_space_label(), SPACE_CELLS);
            out.push_str(GAP);
        }
    }

    /// Whether window `id` shows a footer, which says some of its note itself: not while a prompt's
    /// float covers it, as its modeline then says it all (ADR 0170).
    fn shows_footer(&self, id: WindowId) -> bool {
        (self.frame.footer(id)).is_some_and(|row| !self.prompt_covers(row))
    }

    /// Whether a prompt's float is over any of `cells`.
    fn prompt_covers(&self, cells: CellRect) -> bool {
        (self.prompt_layout()).is_some_and(|layout| {
            let float = layout.regions.outer;
            cells.row < float.row + float.rows
                && float.row < cells.row + cells.rows
                && cells.column < float.column + float.columns
                && float.column < cells.column + cells.columns
        })
    }

    /// Whether the modeline of the window `id` says what is about the frame, as `Agent`: the
    /// selected tiled window's, `tiled`, and while a prompt's float covers that one, the
    /// bottom-most window's too, the left-most of those on the bottom row, so that it stays in
    /// sight (ADR 0085, ADR 0112).
    fn shows_frame_segments(&self, id: WindowId, tiled: WindowId) -> bool {
        if id == tiled {
            return true;
        }
        let covered =
            (self.regions(tiled)).is_some_and(|regions| self.prompt_covers(regions.modeline));
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
    /// takes its place for a key (ADR 0082, ADR 0116), into `out`, in `columns` cells. While
    /// text is composed in isearch, its line comes first, as it shows the composition. Returns
    /// where in it that is, as [`Status::composed`].
    fn write_echo_line(&self, columns: usize, out: &mut String) -> Option<Range<usize>> {
        if self.preedit.is_some() && self.isearch.is_some() {
            return self.write_isearch_line(columns, out);
        }
        // An undo's progress comes first while it goes on: what its command said shows once it
        // ends (ADR 0119).
        self.write_crossing_line(out);
        if out.is_empty() {
            out.push_str(self.echo_text());
        }
        if out.is_empty() {
            self.write_jump_line(out);
        }
        if !out.is_empty() {
            return None;
        }
        let composed = self.write_isearch_line(columns, out);
        self.write_replace_line(out);
        composed
    }

    /// The echo row's cell where text is composed in isearch: after the search string, or the
    /// row's last one past it. `None` while the echo line is not isearch's.
    pub(super) fn search_cell(&self) -> Option<CellRect> {
        let mut status = Status::default();
        self.status_into(self.tiled_id(), &mut status);
        let before = status.echo.get(..status.composed.as_ref()?.start)?;
        let echo = self.frame.echo;
        let column = text_cells(status.echo_warnings()) + text_cells(before);
        (echo.rows > 0 && echo.columns > 0).then(|| CellRect {
            column: echo.column + column.min(echo.columns - 1),
            rows: 1,
            columns: 1,
            ..echo
        })
    }
}

mod note;
mod state;

#[cfg(test)]
mod tests {
    use super::note::NATIVE_NOTE_CELLS;
    use super::state::percent;
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
    fn the_selected_windows_modeline_says_a_dictation_records_then_waits() {
        let mut editor = editor("a\n");
        assert_eq!(status(&editor).dictation, "");
        editor.set_dictation_mark(crate::editor::DictationMark::Recording);
        let shown = status(&editor);
        assert_eq!(shown.dictation, "● REC  ");
        assert!(shown.modeline().starts_with("● REC  -- notes.md"));
        editor.set_dictation_mark(crate::editor::DictationMark::Waiting);
        assert_eq!(status(&editor).dictation, "…  ");
        editor.set_dictation_mark(crate::editor::DictationMark::Fixing);
        assert_eq!(status(&editor).dictation, "fix  ");
        let other = editor.window().id();
        editor.split_window(crate::frame::Way::Down, true).unwrap();
        let mut theirs = Status::default();
        editor.status_into(other, &mut theirs);
        assert_eq!(theirs.dictation, "", "only the selected window's says it");
        editor.set_dictation_mark(crate::editor::DictationMark::Off);
        assert_eq!(status(&editor).dictation, "");
    }

    /// An editor `columns` wide showing a native session whose note is `note`.
    fn native(note: &str, columns: usize) -> Editor {
        named_native("*claude: oso*", note, columns)
    }

    fn named_native(name: &str, note: &str, columns: usize) -> Editor {
        let mut editor = Editor::default();
        editor.set_frame_size(FrameSize { rows: 10, columns });
        let id = editor.add_native_session_buffer(name);
        editor.show(id);
        editor.set_mode_note(id, note.to_owned());
        editor
    }

    #[test]
    fn a_native_sessions_note_is_whole_where_it_fits() {
        let note = "2 queued · Haiku 5.5 · default · ctx 3% · $0.82 · 7d 28%";
        let status = status(&native(note, 120));
        assert_eq!(status.state, format!("1:0  Session {note}"));
        assert_eq!(status.name, "%% *claude: oso*  ");
    }

    #[test]
    fn a_narrow_window_drops_a_native_notes_parts_from_the_right_before_the_name() {
        let name = "*claude: a session with a long name*";
        let note = "2 queued · Haiku 5.5 · default · ctx 3% · $0.82 · 7d 28%";
        let at = |columns| status(&named_native(name, note, columns));
        let wide = at(200);
        let least = 3 + MIN_NAME_CELLS + 2;
        let fits = text_cells(&wide.modeline()) - text_cells(&wide.name) + least;
        // The name is cut to its least first, and the note stays whole.
        assert_eq!(at(fits).state, format!("1:0  Session {note}"));
        assert_eq!(text_cells(&at(fits).name), least);
        // One cell less: the last part goes, and the name keeps its least.
        let less = at(fits - 1);
        assert_eq!(
            less.state,
            "1:0  Session 2 queued · Haiku 5.5 · default · ctx 3% · $0.82"
        );
        assert!(text_cells(&less.name) >= least, "{less:?}");
        assert!(text_cells(&less.modeline()) < fits);
        // Narrower, more parts go, the first one never.
        let narrow = at(fits - 25);
        assert_eq!(narrow.state, "1:0  Session 2 queued · Haiku 5.5 · default");
        let least = at(30);
        assert_eq!(least.state, "2 queued", "nor where it is before it");
        // The name gives way to it, under its least.
        assert!(text_cells(&least.name) < 3 + MIN_NAME_CELLS + 2);
        assert_eq!(text_cells(&least.modeline()), 30);
    }

    /// A native session in a project and a space, `columns` wide, with a session that is done and the
    /// agent channel on, as oso shows one.
    #[cfg(unix)]
    fn placed_native(note: &str, columns: usize) -> Editor {
        let mut editor = named_native("*claude: some-session-of-a-space*", note, columns);
        let id = editor.window().buffer();
        editor.set_front_space("a-space");
        editor.set_agent_channel(crate::editor::AgentChannel::Connected);
        editor.set_session_marks(crate::editor::SessionMarks {
            done: 1,
            ..crate::editor::SessionMarks::default()
        });
        let found = crate::project::Found {
            root: crate::project::RootIdentity {
                path: PathBuf::from("/work/some-project"),
                identity: Some((1, 2)),
            },
            worktree_of: None,
            checked_main: None,
        };
        editor.set_buffer_project(id, Some(found));
        let list = crate::project::TrustList::parse("/work/some-project\tdev=1 ino=2\n").0;
        editor.set_trust_list(Some(list));
        let project = editor.buffer(id).and_then(Buffer::project).unwrap();
        assert!(editor.project_trusted(project), "not restricted");
        editor
    }

    #[cfg(unix)]
    #[test]
    fn a_native_notes_first_part_comes_before_where_it_is() {
        let at = |columns| status(&placed_native("⇄ auto: claude.ai · Opus 5.5", columns));
        let wide = at(200);
        assert!(!wide.project.is_empty() && !wide.space.is_empty());
        assert!(wide.state.starts_with("1:0  Session ⇄ auto: claude.ai · "));
        let mut dropped = Vec::new();
        for columns in (54..200).rev() {
            let status = at(columns);
            assert!(status.state.contains("⇄ auto: claude.ai"), "{columns}");
            let cells = text_cells(&status.modeline());
            assert!(cells <= columns, "{columns}: {}", status.modeline());
            let place = status.state.starts_with("1:0");
            dropped.push((status.space.is_empty(), status.project.is_empty(), !place));
        }
        dropped.dedup();
        // The space first, then the project, then the position and the mode.
        assert_eq!(
            dropped,
            [
                (false, false, false),
                (true, false, false),
                (true, true, false),
                (true, true, true)
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_native_sessions_mark_is_whole_in_narrow_windows() {
        for (columns, marks) in [
            (
                44,
                &[
                    "⇄ claude.ai",
                    "⇄ connecting",
                    "⇄ dropped",
                    "⇄ going off",
                    "⇄ auto: claude.ai",
                ][..],
            ),
            (
                54,
                &[
                    "⇄ claude.ai",
                    "⇄ connecting",
                    "⇄ dropped",
                    "⇄ going off",
                    "⇄ auto: claude.ai",
                    "⇄ plan: connecting",
                    "⇄ auto: dropped",
                ],
            ),
            (99, &["⇄ acceptEdits: connecting", "⇄ auto: claude.ai"]),
        ] {
            for mark in marks {
                let status = status(&placed_native(&format!("{mark} · Opus 5.5"), columns));
                let line = status.modeline();
                assert!(line.contains(mark), "{columns}: {line}");
                assert!(text_cells(&line) <= columns, "{columns}: {line}");
            }
        }
    }

    /// A native session as [`placed_native`] with a composer docked under it and its footer, whose note
    /// is `beside`, in a frame of `rows` rows.
    #[cfg(unix)]
    fn footed_native(note: &str, beside: &str, (rows, columns): (usize, usize)) -> Editor {
        let mut editor = placed_native(note, columns);
        let id = editor.window().buffer();
        let composer = editor.add_internal_buffer("*compose: oso*".to_owned(), Mode::Composer);
        editor.set_dock(id, crate::window::DockSlot::Composer, Some(composer));
        editor.set_footer(id, Some(beside.to_owned()));
        editor.set_frame_size(FrameSize { rows, columns });
        editor
    }

    #[cfg(unix)]
    #[test]
    fn a_window_that_shows_its_footer_says_the_note_beside_it_and_another_the_whole() {
        let (note, beside) = ("Opus 5.5 · auto · ctx 3% · $0.82", "ctx 3% · $0.82");
        let mut editor = footed_native(note, beside, (30, 120));
        let window = editor.window().id();
        assert!(editor.frame().footer(window).is_some());
        assert_eq!(status(&editor).state, format!("1:0  Session {beside}"));
        // The same session in a second window, which carries no docks.
        let other = editor.split_window(crate::frame::Way::Down, false).unwrap();
        let mut theirs = Status::default();
        for id in [window, other] {
            editor.status_into(id, &mut theirs);
            let footer = editor.frame().footer(id).is_some();
            let says = if footer { beside } else { note };
            assert_eq!(theirs.state, format!("1:0  Session {says}"));
        }
        // A short window has no room for it.
        let mut short = footed_native(note, beside, (8, 120));
        let id = short.window().id();
        let rows = (4..12).find(|rows| {
            short.set_frame_size(FrameSize {
                rows: *rows,
                columns: 120,
            });
            short.frame().footer(id).is_some()
        });
        for (rows, says) in [(rows.unwrap(), beside), (rows.unwrap() - 1, note)] {
            short.set_frame_size(FrameSize { rows, columns: 120 });
            assert_eq!(status(&short).state, format!("1:0  Session {says}"));
        }
        // With no footer, the whole again.
        let host = editor.window().buffer();
        editor.set_footer(host, None);
        assert_eq!(editor.frame().footer(window), None);
        assert_eq!(status(&editor).state, format!("1:0  Session {note}"));
    }

    /// While a prompt's float covers the footer, the modeline in sight says the whole.
    #[cfg(unix)]
    #[test]
    fn a_footer_a_float_covers_leaves_nothing_to_itself() {
        let (note, beside) = ("Opus 5.5 · auto · ctx 3%", "ctx 3%");
        let mut editor = footed_native(note, beside, (8, 60));
        let window = editor.window().id();
        let row = editor.frame().footer(window).unwrap();
        let says = |editor: &Editor| {
            let mut status = Status::default();
            editor.status_into(window, &mut status);
            status.state.contains("Opus 5.5")
        };
        assert!(!says(&editor));
        // A question over the window's top rows alone.
        editor.ask("Exit?", "keyboard-quit").unwrap();
        let outer = |editor: &Editor| editor.prompt_layout().unwrap().regions.outer;
        assert!(
            outer(&editor).row + outer(&editor).rows <= row.row,
            "{row:?}"
        );
        assert!(!says(&editor));
        editor.quit();
        // One as tall as the window.
        let label = "word ".repeat(400);
        editor.ask(&label, "keyboard-quit").unwrap();
        assert!(
            outer(&editor).row + outer(&editor).rows > row.row,
            "{:?}",
            outer(&editor)
        );
        assert!(says(&editor));
        editor.quit();
        assert!(!says(&editor));
    }

    /// With nothing beside the footer, what is before it goes whole in a narrow window.
    #[cfg(unix)]
    #[test]
    fn a_note_with_nothing_to_say_lets_the_position_go_whole() {
        for columns in [44, 54] {
            let line = status(&footed_native("whole", "", (30, columns)));
            assert!(
                text_cells(&line.modeline()) <= columns,
                "{}",
                line.modeline()
            );
            assert!(
                ["", "1:0  Session"].contains(&line.state.as_str()),
                "{}",
                line.state
            );
        }
    }

    /// What narrow windows keep of the note beside a footer: its mark whole, then as many parts as fit.
    #[cfg(unix)]
    #[test]
    fn a_narrow_window_beside_its_footer_keeps_the_notes_first_parts() {
        let beside = "ctx 40% · $0.82 · 7d 28%";
        let said: Vec<String> = [44, 54, 99]
            .map(|columns| status(&footed_native("whole", beside, (30, columns))).state)
            .to_vec();
        assert_eq!(
            said,
            [
                "ctx 40%",
                "ctx 40%",
                "1:0  Session ctx 40% · $0.82 · 7d 28%"
            ]
        );
        for columns in [44, 54, 99] {
            let beside = format!("⇄ auto: claude.ai · {beside}");
            let line = status(&footed_native("whole", &beside, (30, columns))).modeline();
            assert!(line.contains("⇄ auto: claude.ai"), "{columns}: {line}");
            assert!(text_cells(&line) <= columns);
        }
    }

    #[test]
    fn a_long_native_note_is_cut_at_its_own_cap() {
        let note = format!("{} · end", "x".repeat(120));
        let status = status(&native(&note, 400));
        let shown = status.state.strip_prefix("1:0  Session ").unwrap();
        assert_eq!(text_cells(shown), NATIVE_NOTE_CELLS);
        assert!(shown.ends_with(CUT));
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
        /// The cells of printable ASCII are its bytes, as `text_clusters` counts any text.
        #[test]
        fn text_cells_counts_as_clusters_do(text in "[ -~]{0,40}|\\PC{0,20}|[\\t\\r\\n -~]{0,20}") {
            let clusters: usize = text_clusters(&text, 0).map(|cluster| cluster.cells).sum();
            prop_assert_eq!(text_cells(&text), clusters);
        }

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
