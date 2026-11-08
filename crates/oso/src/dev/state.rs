//! What the control socket reads from and does to the editor state on the UI
//! thread, apart from running commands (ADR 0023, ADR 0044). It never returns
//! the text of a buffer that may hold secrets, whichever way it is asked, and
//! `get-state` gives no position in one, and no size once it is edited (ADR 0051,
//! ADR 0058).
//!
//! Must not depend on: `app`, winit or sockets; these are plain functions of
//! the editor state.
//!
//! A buffer can be 1 GB and a response line 1 MiB (ADR 0032), so the work is
//! bounded before any text is copied. `get-state` copies only what the
//! selected window shows, clipped to its columns and to [`MAX_VISIBLE_TEXT`].
//! `get-text` checks the size of the rows asked for against [`MAX_TEXT`],
//! and copies them on a background thread from an O(1) snapshot of the
//! buffer, since each row costs a few tree lookups.
//!
//! Text is what the rope holds: invalid bytes read as U+FFFD, not as the
//! `\xNN` escapes on screen, and tabs are tabs.

use std::ops::Range;
use std::sync::mpsc;
use std::thread;

use oso_core::buffer::{Buffer, BufferId, Mode, ReadOnly};
use oso_core::complete::{Candidate, Candidates};
use oso_core::display_map::{DisplayPoint, DisplaySnapshot, Preedit};
use oso_core::editor::{Editor, Status};
use oso_core::frame::{CellRect, PixelRect};
use oso_core::key::KeyPress;
use oso_core::keymap::display_keys;
use oso_core::minibuffer::{Kind, MAX_ENTRY_BYTES};
use oso_core::motion;
use oso_core::rope::Rope;
use oso_core::selection::{Goal, Selection};
use oso_core::sum_tree::Bias;
use oso_core::window::Window;
use serde_json::{Value, json};

use super::Reply;

/// The most visible text `get-state` returns, in bytes. A cell usually holds
/// a few bytes but can hold dozens (combining marks), and a response this
/// size still fits a line if every byte needs escaping, so the metadata that
/// comes with it always gets through.
pub const MAX_VISIBLE_TEXT: usize = 128 * 1024;

/// The most text one `get-text` returns, in bytes. Well below the line
/// limit, so that text with few characters to escape fits.
pub const MAX_TEXT: usize = 512 * 1024;

/// The most rows one `get-text` returns. Each costs a few bytes of JSON,
/// even empty.
pub const MAX_TEXT_ROWS: usize = 10_000;

/// Why text or pixels are refused.
const WITHHELD: &str = "the buffer may hold secrets or is still being checked, so its text and pixels stay in the editor (ADR 0044)";

/// The result of `get-state`: every buffer of the user's and every window, the selected
/// window's buffer, cursor, scroll position and visible text, the keys of a pending
/// prefix, and the surface size if there is a window.
///
/// The cursor and each selection carry their mark, and each window says whether its
/// regions show and holds its mark ring (ADR 0071).
///
/// For a buffer that may hold secrets, `text` is null, and `cursor` and `scroll`
/// are left out, with every mark. Its other metadata stays, as [`buffer_state`] has it. While any window
/// shows one, `modeline` and `echo` are left out too (ADR 0051, ADR 0063).
///
/// The window, its buffer and its text are the tiled window's, also beneath a prompt.
/// `prompts` describes the open prompts, the outermost first (ADR 0072).
pub fn get_state(editor: &Editor, pending: &[KeyPress], surface_size: Option<(u32, u32)>) -> Value {
    let (window, buffer) = editor.tiled();
    let display = buffer.display();
    let size = window.size();
    let text = (!buffer.is_secret()).then(|| visible_text(window, buffer.text(), &display));
    let mut state = json!({
        "buffer": buffer_state(window.buffer(), buffer),
        "window": { "rows": size.rows, "columns": size.columns },
        "text": text,
        "buffers": editor
            .buffers()
            .map(|(id, buffer)| buffer_state(id, buffer))
            .collect::<Vec<_>>(),
        "windows": editor
            .windows()
            .take(editor.windows().len() - editor.prompts().len())
            .enumerate()
            .map(|(index, window)| window_state(editor, window, index))
            .collect::<Vec<_>>(),
        "keys": { "pending": display_keys(pending).to_string() },
        "prompts": prompts_state(editor),
        "surface": surface_size.map(|(width, height)| json!({ "width": width, "height": height })),
    });
    if !buffer.is_secret() {
        let primary = window.selections.primary();
        let point = display.offset_to_display_point(primary.cursor);
        state["cursor"] = json!({
            "offset": primary.cursor,
            "row": point.row,
            "column": point.column,
            "count": window.selections.len(),
            "mark": mark_state(primary.mark, &display),
        });
        state["scroll"] = scroll_state(window);
    }
    if !editor.shows_secret() && !editor.echo_withheld() {
        let mut status = Status::default();
        editor.status_into(&mut status);
        state["modeline"] = json!(status.modeline());
        state["echo"] = json!(status.echo);
    }
    state["kill_ring"] = kill_ring_state(editor);
    state
}

/// The most pieces of the newest entry `get-state` shows, and the most bytes of each.
const SHOWN_PIECES: usize = 16;
const SHOWN_PIECE_BYTES: usize = 1024;

/// The `kill_ring` field of `get-state`: how many entries there are, and the newest one's
/// pieces, cut short. Secret entries and text from other apps' copies are left out as if
/// absent, and the whole field is null while a buffer that may hold secrets is shown
/// (ADR 0074).
fn kill_ring_state(editor: &Editor) -> Value {
    if editor.shows_secret() {
        return Value::Null;
    }
    let mut ordinary = editor
        .kill_ring()
        .iter()
        .filter(|entry| !entry.is_secret() && !entry.came_from_clipboard());
    let newest = ordinary.next().map(|entry| {
        let pieces = entry.pieces();
        let mut truncated = pieces.len() > SHOWN_PIECES;
        let shown: Vec<String> = pieces
            .iter()
            .take(SHOWN_PIECES)
            .map(|piece| {
                truncated |= piece.len() > SHOWN_PIECE_BYTES;
                piece_start(piece)
            })
            .collect();
        json!({ "pieces": shown, "truncated": truncated })
    });
    json!({ "entries": usize::from(newest.is_some()) + ordinary.count(), "newest": newest })
}

/// The first [`SHOWN_PIECE_BYTES`] of `piece`, cut at a character's start.
fn piece_start(piece: &oso_core::kill::Piece) -> String {
    let mut text = String::new();
    for chunk in piece.chunks() {
        let room = SHOWN_PIECE_BYTES - text.len();
        if chunk.len() <= room {
            text.push_str(chunk);
            continue;
        }
        let mut end = room;
        while !chunk.is_char_boundary(end) {
            end -= 1;
        }
        text.push_str(&chunk[..end]);
        break;
    }
    text
}

/// The open prompts, the outermost first: each one's depth, kind, label, answer so far, cursor
/// and message, its candidates for one that completes, and the innermost's region, which alone
/// shows. While a buffer that may hold secrets is shown, a passphrase's prompt among them, the
/// label, the answer, the cursor, the message and the candidates are null, as a search string
/// or a line number can say something of the text, and which candidates match says what was
/// typed (ADR 0076).
fn prompts_state(editor: &Editor) -> Value {
    let shown = !editor.shows_secret();
    let layout = editor.prompt_layout();
    let count = editor.prompts().len();
    editor
        .prompts()
        .iter()
        .zip(editor.prompt_windows())
        .enumerate()
        .map(|(index, (prompt, window))| {
            let kind = match prompt.kind() {
                Kind::Number { .. } => "number",
                Kind::Passphrase => "passphrase",
                Kind::Complete { .. } => "complete",
                Kind::Question { .. } => "question",
            };
            let buffer = editor.buffer(prompt.buffer()).filter(|_| shown);
            let cursor = shown.then(|| window.selections.primary().cursor);
            let region = layout
                .filter(|_| index + 1 == count)
                .map(|layout| region_state(layout.regions.outer));
            json!({
                "depth": index + 1,
                "kind": kind,
                "prompt": shown.then(|| prompt.label()),
                "text": buffer.map(|buffer| answer_text(buffer.text())),
                "cursor": cursor,
                "message": prompt.message().filter(|_| shown && !prompt.message_withheld()),
                "region": region,
                "completion": prompt
                    .candidates()
                    .filter(|_| shown)
                    .map(|candidates| completion_state(candidates, editor.candidate_rows())),
            })
        })
        .collect()
}

/// How many candidates match of how many, which is selected, and the names of those in the
/// rows shown, as the float shows them (ADR 0076).
fn completion_state(candidates: &Candidates, shown: usize) -> Value {
    let rows = (candidates.top()..candidates.len())
        .take(shown)
        .filter_map(|place| candidates.get(place))
        .map(Candidate::text)
        .collect::<Vec<_>>();
    json!({
        "count": candidates.len(),
        "total": candidates.total(),
        "selected": candidates.selected(),
        "rows": rows,
    })
}

/// What a prompt holds, cut on a character boundary to the longest answer a history keeps,
/// since a yank can put a whole file there.
fn answer_text(text: &oso_core::rope::Rope) -> String {
    let mut out = String::new();
    for chunk in text.chunks() {
        let room = MAX_ENTRY_BYTES - out.len();
        if chunk.len() > room {
            out.push_str(&chunk[..chunk.floor_char_boundary(room)]);
            break;
        }
        out.push_str(chunk);
    }
    out
}

fn region_state(region: CellRect) -> Value {
    json!({
        "row": region.row,
        "column": region.column,
        "rows": region.rows,
        "columns": region.columns,
    })
}

/// What the app has told the OS of the input method, for [`ime_state`].
pub struct ImeFacts {
    /// Whether the app wants it on (ADR 0069).
    pub wanted: bool,
    /// Whether it last allowed it, which waits for a composition to end.
    pub allowed: bool,
    /// The cursor area last given to the OS, in physical pixels.
    pub area: Option<PixelRect>,
}

/// The `ime` field of `get-state`: what the app wants and allowed, whether a composition
/// is open, its preedit and range, and the area given to the OS. While a buffer that may
/// hold secrets is shown, the preedit, its range and the area are null (ADR 0069).
pub fn ime_state(editor: &Editor, facts: &ImeFacts) -> Value {
    let preedit = editor.preedit();
    let shown = !editor.shows_secret();
    let area = facts.area.filter(|_| shown).map(
        |area| json!({ "x": area.x, "y": area.y, "width": area.width, "height": area.height }),
    );
    json!({
        "wanted": facts.wanted,
        "allowed": facts.allowed,
        "composing": preedit.is_some(),
        "preedit": preedit.filter(|_| shown).map(Preedit::text),
        "range": preedit
            .filter(|_| shown)
            .and_then(Preedit::range)
            .map(|range| [range.start, range.end]),
        "area": area,
    })
}

fn scroll_state(window: &Window) -> Value {
    json!({ "top": window.scroll().top, "left": window.scroll().left })
}

/// A buffer's metadata. A buffer that may hold secrets gives no size while it is
/// modified (ADR 0058).
fn buffer_state(id: BufferId, buffer: &Buffer) -> Value {
    let info = buffer.file_info();
    // The size of a withheld copy or compare says how far it is from its file (ADR 0064).
    let withheld_view = buffer.mode() == Mode::CopyView;
    let size_shown = !(buffer.is_secret() && (buffer.is_modified() || withheld_view));
    json!({
        "id": id.get(),
        "name": buffer.name(),
        "path": buffer.path().map(|path| path.to_string_lossy()),
        "secret": buffer.is_secret(),
        "read_only": buffer.read_only().map(ReadOnly::as_str),
        "modified": buffer.is_modified(),
        "saving": buffer.is_saving(),
        "line_ending": info.line_ending.as_str(),
        "bom": info.bom,
        "invalid_bytes": buffer.invalid_bytes().len(),
        "bytes": size_shown.then(|| buffer.text().len()),
        "lines": size_shown.then(|| buffer.display().row_count()),
        "loading": buffer.loading().map(|progress| json!({
            "read": size_shown.then_some(progress.read),
            "total": if size_shown { progress.total } else { None },
        })),
    })
}

fn window_state(editor: &Editor, window: &Window, index: usize) -> Value {
    let size = window.size();
    let shown = editor.shown(window);
    let mut state = json!({
        "buffer": window.buffer().get(),
        "selected": index == editor.selected_window(),
        "rows": size.rows,
        "columns": size.columns,
    });
    if !shown.is_secret() {
        let display = shown.display();
        let selections = &window.selections;
        state["scroll"] = scroll_state(window);
        state["primary"] = json!(selections.primary_index());
        state["selections"] = selections
            .iter()
            .map(|selection| selection_state(selection, &display))
            .collect();
        state["mark_active"] = json!(selections.mark_active().is_active());
        state["mark_ring"] = selections.mark_ring().iter().collect();
    }
    state
}

fn selection_state(selection: &Selection, display: &DisplaySnapshot) -> Value {
    let point = display.offset_to_display_point(selection.cursor);
    json!({
        "cursor": selection.cursor,
        "row": point.row,
        "column": point.column,
        "goal": goal_state(selection.goal),
        "mark": mark_state(selection.mark, display),
    })
}

/// Where a mark is, or null with none (ADR 0071).
fn mark_state(mark: Option<usize>, display: &DisplaySnapshot) -> Value {
    mark.map_or(Value::Null, |mark| {
        let point = display.offset_to_display_point(mark);
        json!({ "offset": mark, "row": point.row, "column": point.column })
    })
}

/// A vertical motion's goal, as `get-state` and the state handoff write it.
pub(super) fn goal_state(goal: Option<Goal>) -> Value {
    match goal {
        None => Value::Null,
        Some(Goal::Column(column)) => json!({ "column": column }),
        Some(Goal::LineEnd) => json!("line-end"),
    }
}

/// The rows the window shows in full, from its top row and left column, cut
/// at its right edge. A wide character across either edge is included. Stops
/// at [`MAX_VISIBLE_TEXT`] and says so.
fn visible_text(window: &Window, text: &Rope, display: &DisplaySnapshot) -> Value {
    let size = window.size();
    let scroll = window.scroll();
    let end_row = scroll
        .top
        .saturating_add(size.rows)
        .min(display.row_count());
    let right = scroll.left.saturating_add(size.columns);
    let mut budget = MAX_VISIBLE_TEXT;
    let mut truncated = false;
    let mut rows = Vec::new();
    for row in scroll.top..end_row {
        let start =
            display.display_point_to_offset(DisplayPoint::new(row, scroll.left), Bias::Left);
        let end = display.display_point_to_offset(DisplayPoint::new(row, right), Bias::Right);
        if end - start > budget {
            truncated = true;
            rows.push(slice(text, start..start + budget));
            break;
        }
        budget -= end - start;
        rows.push(slice(text, start..end));
    }
    json!({
        "start_row": scroll.top,
        "start_column": scroll.left,
        "rows": rows,
        "truncated": truncated,
    })
}

fn slice(text: &Rope, range: Range<usize>) -> String {
    text.chunks_in_range(range).collect()
}

/// Starts `get-text`: whole rows of the tiled window's buffer, also beneath a prompt,
/// without their line breaks. An error if the buffer may hold secrets. Everything
/// else happens in [`TextJob::run`], off the UI thread.
pub fn get_text(editor: &Editor, start_row: usize, rows: usize) -> Result<TextJob, String> {
    let (window, buffer) = editor.tiled();
    if buffer.is_secret() {
        return Err(WITHHELD.into());
    }
    Ok(TextJob {
        buffer: window.buffer().get(),
        text: buffer.text().clone(),
        display: buffer.display(),
        start_row,
        rows,
    })
}

/// A `get-text` request, with a snapshot of a buffer that was not secret
/// when it was asked for.
pub struct TextJob {
    buffer: u64,
    text: Rope,
    display: DisplaySnapshot,
    start_row: usize,
    rows: usize,
}

impl TextJob {
    /// Runs on a background thread, which replies. If the thread cannot
    /// start, the reply sender is dropped, which the client sees as an error.
    pub fn spawn(self, reply: mpsc::Sender<Reply>) {
        let spawned = thread::Builder::new()
            .name("oso-get-text".into())
            .spawn(move || {
                let _ = reply.send(self.run());
            });
        if let Err(e) = spawned {
            log::warn!("cannot start the get-text thread: {e}");
        }
    }

    /// The result: rows past the end are left out. An error if the rows are
    /// more than a response takes.
    fn run(&self) -> Reply {
        let display = &self.display;
        if self.rows > MAX_TEXT_ROWS {
            return Err(format!(
                "get-text returns at most {MAX_TEXT_ROWS} rows; {} were asked for",
                self.rows
            ));
        }
        let row_count = display.row_count();
        let end_row = self.start_row.saturating_add(self.rows).min(row_count);
        let range = self.start_row.min(end_row)..end_row;
        if !range.is_empty() {
            let bytes =
                motion::line_end(display, range.end - 1) - motion::line_start(display, range.start);
            if bytes > MAX_TEXT {
                return Err(format!(
                    "rows {} to {} hold {bytes} bytes, and get-text returns at most {MAX_TEXT}; ask for fewer rows",
                    range.start,
                    range.end - 1,
                ));
            }
        }
        let start_row = range.start;
        let rows: Vec<String> = range
            .map(|row| {
                slice(
                    &self.text,
                    motion::line_start(display, row)..motion::line_end(display, row),
                )
            })
            .collect();
        Ok(json!({
            "buffer": self.buffer,
            "start_row": start_row,
            "row_count": row_count,
            "rows": rows,
        }))
    }
}

/// Refuses a screenshot while any window shows a buffer that may hold
/// secrets. Checked on the UI thread right before the capture, so nothing
/// can switch the buffer in between.
pub fn screenshot_allowed(editor: &Editor) -> Result<(), String> {
    if editor.shows_secret() {
        return Err(WITHHELD.into());
    }
    Ok(())
}

/// Shows the buffer numbered `buffer` in the selected window, from its
/// start. There is no command for this until M2 (`switch-to-buffer`).
pub fn show_buffer(editor: &mut Editor, buffer: u64) -> Reply {
    let id = editor
        .buffers()
        .find_map(|(id, _)| (id.get() == buffer).then_some(id))
        .ok_or_else(|| format!("no buffer with id {buffer}"))?;
    editor.show(id);
    Ok(json!({ "buffer": buffer }))
}

#[cfg(test)]
mod tests {
    use super::{
        ImeFacts, MAX_TEXT, MAX_TEXT_ROWS, MAX_VISIBLE_TEXT, Reply, get_state, get_text, ime_state,
        screenshot_allowed, show_buffer,
    };
    use oso_core::buffer::{Buffer, BufferId, Replacement};
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::selection::{MarkActive, SelectionSet};
    use oso_core::window::{Scroll, ViewSize};
    use serde_json::json;
    use std::path::PathBuf;

    /// `get-text` from start to finish, on this thread.
    fn text(editor: &Editor, start_row: usize, rows: usize) -> Reply {
        get_text(editor, start_row, rows)?.run()
    }

    fn state(editor: &Editor, pending: &[oso_core::key::KeyPress]) -> serde_json::Value {
        get_state(editor, pending, None)
    }

    fn editor(text: &str, rows: usize, columns: usize) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
        editor.set_view_size(ViewSize::new(rows, columns));
        editor
    }

    /// An editor showing `path`, loaded with `content`, with its path
    /// resolved to `resolved`.
    fn visiting(path: &str, resolved: Option<&str>, content: &str) -> (Editor, BufferId) {
        let mut editor = Editor::default();
        let mut buffer = Buffer::visiting(PathBuf::from(path));
        buffer.apply_load(oso_core::load::decode_all(
            content.as_bytes(),
            oso_core::display_map::TabWidth::default(),
        ));
        let resolved = resolved.map(|path| [PathBuf::from(path)]);
        buffer.judge_secret(resolved.as_ref().map(|paths| &paths[..]));
        let id = editor.add_buffer(buffer);
        editor.show(id);
        (editor, id)
    }

    #[test]
    fn state_reports_the_buffer_cursor_scroll_and_surface() {
        let mut editor = Editor::with_buffer(Buffer::from_bytes("bad.txt", b"a\r\n\xffSECRET"));
        editor.set_view_size(ViewSize::new(5, 2));
        editor.current_mut().0.selections = SelectionSet::new(6);
        editor.follow_cursor();
        let state = get_state(&editor, &[], Some((800, 600)));
        let buffer = json!({
            "id": 0,
            "name": "bad.txt",
            "path": null,
            "secret": false,
            "read_only": "invalid-utf8",
            "modified": false,
            "saving": false,
            "line_ending": "crlf",
            "bom": false,
            "invalid_bytes": 1,
            "bytes": 12,
            "lines": 2,
            "loading": null,
        });
        assert_eq!(
            state,
            json!({
                "modeline": "%% bad.txt  2:4  Fundamental  UTF-8 (invalid) CRLF",
                "echo": "",
                "buffer": buffer,
                "cursor": { "offset": 6, "row": 1, "column": 4, "count": 1, "mark": null },
                "scroll": { "top": 0, "left": 3 },
                "window": { "rows": 5, "columns": 2 },
                "text": {
                    "start_row": 0,
                    "start_column": 3,
                    "rows": ["", "\u{fffd}S"],
                    "truncated": false,
                },
                "buffers": [buffer],
                "windows": [{
                    "buffer": 0,
                    "selected": true,
                    "rows": 5,
                    "columns": 2,
                    "scroll": { "top": 0, "left": 3 },
                    "primary": 0,
                    "selections": [
                        { "cursor": 6, "row": 1, "column": 4, "goal": null, "mark": null },
                    ],
                    "mark_active": false,
                    "mark_ring": [],
                }],
                "keys": { "pending": "" },
                "kill_ring": { "entries": 0, "newest": null },
                "prompts": [],
                "surface": { "width": 800, "height": 600 },
            })
        );
        let pending: KeySequence = "C-x 4".parse().unwrap();
        let state = get_state(&Editor::default(), &pending.0, None);
        assert_eq!(state["keys"]["pending"], "C-x 4");
        assert_eq!(state["buffer"]["name"], "*scratch*");
        assert_eq!(state["surface"], json!(null));
    }

    #[test]
    fn visible_text_is_what_the_window_shows() {
        let content: String = (0..100).map(|i| format!("line {i:02}\n")).collect();
        let mut editor = editor(&content, 3, 6);
        assert_eq!(
            state(&editor, &[])["text"],
            json!({
                "start_row": 0,
                "start_column": 0,
                "rows": ["line 0", "line 0", "line 0"],
                "truncated": false,
            })
        );
        // The cursor at the view's first cell, so that following it moves nothing.
        let cursor = SelectionSet::new(98 * "line 00\n".len() + 5);
        editor.restore_positions(cursor, Scroll { top: 98, left: 5 });
        assert_eq!(
            state(&editor, &[])["text"],
            json!({
                "start_row": 98,
                "start_column": 5,
                "rows": ["98", "99", ""],
                "truncated": false,
            }),
            "only rows that exist, and the columns from the left edge",
        );
    }

    #[test]
    fn visible_text_includes_wide_characters_across_the_edges() {
        // In 2 columns, a view can start inside a wide character only while the
        // cursor is on another row: the cursor shows a whole cluster.
        let mut editor = editor("日本語\nabc\n", 2, 2);
        let cursor = SelectionSet::new("日本語\nab".len());
        editor.restore_positions(cursor, Scroll { top: 0, left: 1 });
        assert_eq!(state(&editor, &[])["text"]["rows"], json!(["日本", "bc"]));
    }

    #[test]
    fn a_huge_line_is_clipped_to_the_window() {
        let long = "x".repeat(2 * 1024 * 1024);
        let editor = editor(&long, 2, 80);
        let state = state(&editor, &[]);
        assert_eq!(state["text"]["rows"], json!(["x".repeat(80)]));
    }

    #[test]
    fn rows_of_combining_marks_are_cut_and_the_metadata_survives() {
        // Each cell holds 33 bytes, more than it looks.
        let cell = format!("a{}", "\u{1d165}".repeat(8));
        let row = cell.repeat(200) + "\n";
        let editor = editor(&row.repeat(100), 50, 200);
        let state = state(&editor, &[]);
        assert_eq!(state["text"]["truncated"], true);
        let rows = state["text"]["rows"].as_array().unwrap();
        let bytes: usize = rows.iter().map(|r| r.as_str().unwrap().len()).sum();
        assert!(bytes <= MAX_VISIBLE_TEXT, "{bytes}");
        assert!(bytes > MAX_VISIBLE_TEXT - 4, "{bytes}");
        assert_eq!(state["buffers"][0]["id"], 0);
    }

    #[test]
    fn get_text_returns_whole_rows_without_line_breaks() {
        let editor = editor("one\r\ntwo\n\nfour", 1, 1);
        let reply = text(&editor, 0, 10).unwrap();
        assert_eq!(
            reply,
            json!({
                "buffer": 0,
                "start_row": 0,
                "row_count": 4,
                "rows": ["one", "two", "", "four"],
            })
        );
        assert_eq!(text(&editor, 1, 2).unwrap()["rows"], json!(["two", ""]));
        assert_eq!(text(&editor, 3, 1).unwrap()["rows"], json!(["four"]));
        assert_eq!(
            text(&editor, 4, 1).unwrap()["rows"],
            json!([]),
            "past the end"
        );
        assert_eq!(text(&editor, usize::MAX, 1).unwrap()["rows"], json!([]));
        assert_eq!(text(&editor, 0, 0).unwrap()["rows"], json!([]));
    }

    #[test]
    fn get_text_errors_clearly_past_its_limits() {
        let at_limit = "x".repeat(MAX_TEXT);
        let editor_at = editor(&at_limit, 1, 1);
        assert_eq!(
            text(&editor_at, 0, 1).unwrap()["rows"][0]
                .as_str()
                .unwrap()
                .len(),
            MAX_TEXT
        );
        let over = "x".repeat(MAX_TEXT + 1);
        let message = text(&editor(&over, 1, 1), 0, 1).unwrap_err();
        assert!(message.contains("ask for fewer rows"), "{message}");
        assert!(message.contains(&(MAX_TEXT + 1).to_string()), "{message}");

        let lines = "\n".repeat(MAX_TEXT_ROWS + 5);
        let editor = editor(&lines, 1, 1);
        assert_eq!(
            text(&editor, 0, MAX_TEXT_ROWS).unwrap()["rows"]
                .as_array()
                .unwrap()
                .len(),
            MAX_TEXT_ROWS
        );
        assert!(
            text(&editor, 0, MAX_TEXT_ROWS + 1)
                .unwrap_err()
                .contains("at most")
        );
    }

    #[test]
    fn a_secret_buffer_gives_no_text_or_pixels_but_keeps_its_metadata() {
        const CONTENT: &str = "SECRET-VALUE\n";
        for (path, resolved) in [
            ("/p/.env", Some("/p/.env")),
            ("/h/.ssh/id_rsa", Some("/h/.ssh/id_rsa")),
            ("/p/notes.txt", Some("/h/.ssh/id_rsa")),
            ("/p/notes.txt", None),
        ] {
            let (editor, _) = visiting(path, resolved, CONTENT);
            let state = state(&editor, &[]);
            assert_eq!(state["text"], json!(null), "{path}");
            assert_eq!(state["buffer"]["secret"], true, "{path}");
            assert_eq!(state["buffer"]["path"], path);
            assert_eq!(state["buffer"]["bytes"], CONTENT.len());
            assert!(!state.to_string().contains("SECRET-VALUE"), "{path}");
            assert!(text(&editor, 0, 1).unwrap_err().contains("ADR 0044"));
            assert!(
                screenshot_allowed(&editor)
                    .unwrap_err()
                    .contains("ADR 0044")
            );
        }
    }

    #[test]
    fn the_modeline_and_the_echo_area_read_back_unless_a_secret_is_shown() {
        let mut editor = Editor::with_buffer(Buffer::from_text("plain.txt", "ab"));
        editor.message("Saved");
        let state = get_state(&editor, &[], None);
        assert_eq!(state["echo"], "Saved");
        let modeline = state["modeline"].as_str().unwrap();
        assert!(modeline.starts_with("-- plain.txt  1:0  "), "{modeline}");

        let secret = editor.add_buffer(Buffer::visiting(PathBuf::from("/tmp/.env")));
        editor.show(secret);
        let state = get_state(&editor, &[], None);
        assert!(state.get("modeline").is_none());
        assert!(state.get("echo").is_none());
    }

    #[test]
    fn a_withheld_copy_gives_no_size_also_while_it_loads() {
        let mut editor = Editor::default();
        let view = editor.add_internal_buffer("v", oso_core::buffer::Mode::CopyView);
        let shown = editor.buffer_mut(view).unwrap();
        shown.judge_copy(&[], oso_core::buffer::Verdict::Unresolved);
        let mut builder = oso_core::load::Builder::new(Some(100), Default::default());
        builder.push(b"some of it");
        shown.apply_load(builder.snapshot());
        editor.show(view);
        let state = get_state(&editor, &[], None);
        assert!(state["buffer"]["bytes"].is_null());
        assert!(state["buffer"]["lines"].is_null());
        assert!(state["buffer"]["loading"]["read"].is_null());
        assert!(state["buffer"]["loading"]["total"].is_null());
        assert!(
            !state["buffer"]["loading"].is_null(),
            "it says it is loading"
        );
    }

    #[test]
    fn the_messages_buffer_is_not_among_the_users_buffers() {
        let mut editor = Editor::default();
        editor.toggle_messages();
        let state = get_state(&editor, &[], None);
        assert_eq!(state["buffer"]["name"], "*Messages*");
        assert_eq!(state["buffer"]["read_only"], "internal");
        assert_eq!(state["buffers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_secret_buffer_shows_no_positions() {
        let (mut editor, _) = visiting("/p/.env", Some("/p/.env"), "ONE\nTWO THREE\n");
        editor.restore_positions(SelectionSet::new(6), Scroll { top: 1, left: 0 });
        // A mark, one in the ring and a region shown, none of which may show either.
        let selections = &mut editor.current_mut().0.selections;
        selections.push_marks();
        selections.update(|selection| {
            selection.move_to(9);
            Ok::<(), ()>(())
        });
        selections.push_marks();
        selections.update(|selection| {
            selection.move_to(11);
            Ok::<(), ()>(())
        });
        selections.set_mark_active(MarkActive::Active);
        let state = state(&editor, &[]);
        assert!(!state.to_string().contains("mark"), "no mark anywhere");
        let result = state.as_object().unwrap();
        assert!(!result.contains_key("cursor"));
        assert!(!result.contains_key("scroll"));
        let window = state["windows"][0].as_object().unwrap();
        for hidden in [
            "scroll",
            "primary",
            "selections",
            "mark_active",
            "mark_ring",
        ] {
            assert!(!window.contains_key(hidden), "{hidden}");
        }
        // What the window is stays, and so does the rest.
        assert_eq!(state["window"]["rows"], state["windows"][0]["rows"]);
        assert_eq!(state["windows"][0]["buffer"], state["buffer"]["id"]);
        assert_eq!(state["buffer"]["lines"], 3);
        assert_eq!(state["keys"]["pending"], "");
    }

    #[test]
    fn the_preedit_its_range_and_the_area_are_withheld_while_a_secret_is_shown() {
        let (mut editor, secret) = visiting("/p/.env", Some("/p/.env"), "ONE\n");
        let facts = ImeFacts {
            wanted: true,
            allowed: true,
            area: Some(oso_core::frame::PixelRect {
                x: 8.0,
                y: 8.0,
                width: 10.0,
                height: 20.0,
            }),
        };
        let preedit = oso_core::display_map::Preedit::new("にほ".into(), Some(6..6));
        editor.set_preedit(Some(preedit));
        let withheld = ime_state(&editor, &facts);
        assert_eq!(withheld["composing"], true);
        for hidden in ["preedit", "range", "area"] {
            assert_eq!(withheld[hidden], json!(null), "{hidden}");
        }
        let scratch = editor.buffers().next().unwrap().0;
        show_buffer(&mut editor, scratch.get()).unwrap();
        let shown = ime_state(&editor, &facts);
        assert_eq!(shown["preedit"], "にほ");
        assert_eq!(shown["range"], json!([6, 6]));
        assert_eq!(shown["area"]["width"], 10.0);
        show_buffer(&mut editor, secret.get()).unwrap();
        assert_eq!(ime_state(&editor, &facts)["preedit"], json!(null));
    }

    #[test]
    fn the_kill_ring_shows_its_newest_own_entry_cut_short_and_nothing_secret_or_copied() {
        use super::{SHOWN_PIECE_BYTES, kill_ring_state};
        use oso_core::kill::{Direction, Fingerprint};
        let (mut editor, secret) = visiting("/p/.env", Some("/p/.env"), "ONE\n");
        let scratch = editor.buffers().next().unwrap().0;
        show_buffer(&mut editor, scratch.get()).unwrap();
        let long = "é".repeat(SHOWN_PIECE_BYTES);
        let typed = Replacement {
            range: 0..0,
            text: &long,
        };
        editor.apply_edits(scratch, &[typed]).unwrap();
        let all = 0..long.len();
        editor
            .kill(std::iter::once(all), Direction::Forward, true)
            .unwrap();
        let copied = oso_core::rope::Rope::from("another app's copy");
        editor.take_from_clipboard(copied, false, Fingerprint { len: 18, hash: 1 });
        show_buffer(&mut editor, secret.get()).unwrap();
        editor
            .kill(std::iter::once(0..3), Direction::Forward, true)
            .unwrap();
        assert!(editor.kill_ring().get(0).unwrap().is_secret());
        assert_eq!(kill_ring_state(&editor), json!(null), "while it is shown");

        show_buffer(&mut editor, scratch.get()).unwrap();
        let state = kill_ring_state(&editor);
        assert_eq!(state["entries"], 1, "the secret one and the copy left out");
        let piece = state["newest"]["pieces"][0].as_str().unwrap();
        assert_eq!(piece.len(), SHOWN_PIECE_BYTES, "cut at a character's start");
        assert_eq!(state["newest"]["truncated"], true);
    }

    #[test]
    fn an_edit_to_a_secret_buffer_hides_its_size_until_undone() {
        let content = "ONE TWO\n";
        let (mut editor, id) = visiting("/p/.env", Some("/p/.env"), content);
        let edit = Replacement {
            range: 0..3,
            text: "",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        let shown = state(&editor, &[]);
        assert_eq!(shown["buffer"]["bytes"], json!(null));
        assert_eq!(shown["buffer"]["lines"], json!(null));
        assert_eq!(shown["buffers"][1]["bytes"], json!(null), "in the list too");
        assert!(!shown.to_string().contains("\"bytes\":5"));
        editor.undo().unwrap();
        assert_eq!(state(&editor, &[])["buffer"]["bytes"], content.len());
    }

    #[test]
    fn positions_come_back_with_an_ordinary_buffer_and_a_secret_one_in_another_window_stays_out() {
        let (mut editor, secret) = visiting("/p/.env", Some("/p/.env"), "ONE\n");
        let scratch = editor.buffers().next().unwrap().0;
        show_buffer(&mut editor, scratch.get()).unwrap();
        let shown = state(&editor, &[]);
        assert_eq!(shown["cursor"]["offset"], 0);
        assert_eq!(shown["scroll"]["top"], 0);
        assert_eq!(shown["windows"][0]["selections"][0]["cursor"], 0);
        show_buffer(&mut editor, secret.get()).unwrap();
        assert_eq!(state(&editor, &[])["cursor"], json!(null));
    }

    #[test]
    fn a_file_is_withheld_until_its_path_is_resolved() {
        let mut editor = Editor::default();
        let id = editor.add_buffer(Buffer::visiting(PathBuf::from("/p/notes.txt")));
        editor.show(id);
        assert_eq!(state(&editor, &[])["text"], json!(null));
        assert!(screenshot_allowed(&editor).is_err());
        assert!(text(&editor, 0, 1).is_err());
        assert_eq!(state(&editor, &[])["cursor"], json!(null));
        editor
            .buffer_mut(id)
            .unwrap()
            .judge_secret(Some(&[PathBuf::from("/p/notes.txt")]));
        assert!(screenshot_allowed(&editor).is_ok());
        assert!(text(&editor, 0, 1).is_ok());
        assert_eq!(state(&editor, &[])["cursor"]["offset"], 0);
    }

    #[test]
    fn switching_away_from_a_secret_buffer_allows_text_and_pixels_again() {
        let (mut editor, secret) = visiting("/p/.env", Some("/p/.env"), "SECRET-VALUE\n");
        assert!(screenshot_allowed(&editor).is_err());
        let scratch = editor.buffers().next().unwrap().0;
        assert_eq!(
            show_buffer(&mut editor, scratch.get()).unwrap(),
            json!({ "buffer": scratch.get() })
        );
        assert_eq!(screenshot_allowed(&editor), Ok(()));
        let state = state(&editor, &[]);
        assert_eq!(state["text"]["rows"], json!([""]));
        assert_eq!(state["buffers"][1]["secret"], true, "still listed");
        assert!(!state.to_string().contains("SECRET-VALUE"));
        assert!(text(&editor, 0, 1).is_ok());
        show_buffer(&mut editor, secret.get()).unwrap();
        assert!(screenshot_allowed(&editor).is_err());
        assert!(show_buffer(&mut editor, 99).unwrap_err().contains("99"));
    }

    #[test]
    fn every_selection_is_reported() {
        let editor = editor("abc\ndef", 5, 5);
        let state = state(&editor, &[]);
        assert_eq!(state["windows"][0]["primary"], 0);
        assert_eq!(
            state["windows"][0]["selections"],
            json!([{ "cursor": 0, "row": 0, "column": 0, "goal": null, "mark": null }])
        );
    }

    #[test]
    fn marks_the_region_and_the_ring_are_reported() {
        let mut editor = editor("abc\ndef", 5, 5);
        let selections = &mut editor.current_mut().0.selections;
        selections.push_marks();
        selections.update(|selection| {
            selection.move_to(5);
            Ok::<(), ()>(())
        });
        selections.push_marks();
        selections.update(|selection| {
            selection.move_to(6);
            Ok::<(), ()>(())
        });
        selections.set_mark_active(MarkActive::Active);
        let state = state(&editor, &[]);
        let mark = json!({ "offset": 5, "row": 1, "column": 1 });
        assert_eq!(state["cursor"]["mark"], mark);
        let window = &state["windows"][0];
        assert_eq!(window["selections"][0]["mark"], mark);
        assert_eq!(window["mark_active"], true);
        assert_eq!(window["mark_ring"], json!([0]));
    }
}
