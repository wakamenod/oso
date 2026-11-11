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
use crate::buffer::{Buffer, Verdict};
use crate::decode::{FileInfo, LineEnding};
use crate::display_map::text_clusters;
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

/// The text of the modeline and the echo area. Comparing two says whether a frame
/// would differ there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `Agent` while the agent channel is on, first on the row (ADR 0085).
    pub agent: &'static str,
    /// An agent is connected, so the host draws [`Self::agent`] bright.
    pub agent_connected: bool,
    /// The warnings, each with a blank after it. They come first on the row, and on
    /// the echo row of a frame with no modeline.
    pub warnings: String,
    /// The modified marker and the buffer's name, cut to fit, as they come from disk.
    pub name: String,
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
        format!("{}{}{}{}", self.agent, self.warnings, self.name, self.state)
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
        Verdict::Unresolved | Verdict::SecretName | Verdict::Passphrase | Verdict::Yanked => {
            out.push_str(NO_POSITION);
            out.push_str(GAP);
        }
    }
    out.push_str(buffer.mode().name());
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
        let Some((window, buffer, regions)) = self.tiled_window(id) else {
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
        status.agent = if agent == super::AgentChannel::Off || id != tiled {
            ""
        } else {
            AGENT
        };
        status.agent_connected = id == tiled && agent == super::AgentChannel::Connected;
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

        let marker = marker(buffer);
        // These parts are ASCII, so their bytes are their cells.
        let others = status.agent.len()
            + status.warnings.len()
            + marker.len()
            + 1
            + GAP.len()
            + status.state.len();
        let columns = regions.modeline.columns;
        let cells = columns.saturating_sub(others).max(MIN_NAME_CELLS);
        status.name.clear();
        status.name.push_str(marker);
        status.name.push(' ');
        push_cut(&mut status.name, buffer.name(), cells);
        status.name.push_str(GAP);

        status.echo.clear();
        self.write_echo_line(&mut status.echo);
        status.warnings_on_echo_row = regions.modeline.rows == 0;
        status.messages_in_view =
            (window.buffer() == self.messages_buffer).then(|| buffer.revision());
    }

    /// What the echo area says: a message, or isearch's line until a message takes its place
    /// for a key (ADR 0082), into `out`.
    fn write_echo_line(&self, out: &mut String) {
        out.push_str(self.echo_text());
        if out.is_empty() {
            self.write_isearch_line(out);
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
