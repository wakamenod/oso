//! What the control socket reports about the editor: `get-state` and
//! `get-text` (ADR 0023, ADR 0043). It never returns the text of a buffer
//! that may hold secrets, whichever way it is asked.
//!
//! Must not depend on: `app`, `server`, winit or sockets; these are plain
//! functions of the editor state, called on the UI thread.
//!
//! A buffer can be 1 GB and a response line 1 MiB (ADR 0032), so the work
//! here is bounded before any text is copied. `get-state` copies only what
//! the selected window shows, clipped to its columns, so it works on a
//! 1 GB line too. `get-text` compares the size of the rows asked for with
//! [`MAX_TEXT`] first, which takes no time on any buffer.
//!
//! Text is what the rope holds: invalid bytes read as U+FFFD, not as the
//! `\xNN` escapes on screen, and tabs are tabs.

use std::ops::Range;

use oso_core::buffer::{Buffer, BufferId, ReadOnly};
use oso_core::display_map::{DisplayPoint, DisplaySnapshot};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::display_keys;
use oso_core::motion;
use oso_core::selection::{Goal, Selection};
use oso_core::sum_tree::Bias;
use oso_core::window::Window;
use serde_json::{Value, json};

use super::Reply;

/// The most text one `get-text` returns, in bytes. Well below the line
/// limit, so that text with few characters to escape fits.
pub const MAX_TEXT: usize = 512 * 1024;

/// The most rows one `get-text` returns. Each costs a few bytes of JSON,
/// even empty.
pub const MAX_TEXT_ROWS: usize = 10_000;

/// Why text or pixels are refused.
const WITHHELD: &str = "the buffer may hold secrets or is still being checked, so its text and pixels stay in the editor (ADR 0043)";

/// The result of `get-state`: every buffer and window, the selected window's
/// buffer, cursor, scroll position and visible text, the keys of a pending
/// prefix, and the surface size if there is a window.
///
/// `text` is null for a buffer that may hold secrets. Its metadata stays.
pub fn state(editor: &Editor, pending: &[KeyPress], surface_size: Option<(u32, u32)>) -> Value {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let cursor = window.selections.primary().cursor;
    let point = display.offset_to_display_point(cursor);
    let size = window.size();
    let text = (!buffer.is_secret()).then(|| visible_text(window, buffer, &display));
    json!({
        "buffer": buffer_state(window.buffer(), buffer),
        "cursor": {
            "offset": cursor,
            "row": point.row,
            "column": point.column,
            "count": window.selections.len(),
        },
        "scroll": { "top": window.scroll.top, "left": window.scroll.left },
        "window": { "rows": size.rows, "columns": size.columns },
        "text": text,
        "buffers": editor
            .buffers()
            .map(|(id, buffer)| buffer_state(id, buffer))
            .collect::<Vec<_>>(),
        "windows": editor
            .windows()
            .enumerate()
            .map(|(index, window)| window_state(editor, window, index))
            .collect::<Vec<_>>(),
        "keys": { "pending": display_keys(pending).to_string() },
        "surface": surface_size.map(|(width, height)| json!({ "width": width, "height": height })),
    })
}

fn buffer_state(id: BufferId, buffer: &Buffer) -> Value {
    let info = buffer.file_info();
    json!({
        "id": id.get(),
        "name": buffer.name(),
        "path": buffer.path().map(|path| path.to_string_lossy()),
        "secret": buffer.is_secret(),
        "read_only": buffer.read_only().map(ReadOnly::as_str),
        "line_ending": info.line_ending.as_str(),
        "bom": info.bom,
        "invalid_bytes": buffer.invalid_bytes().len(),
        "bytes": buffer.text().len(),
        "lines": buffer.display().row_count(),
        "loading": buffer.loading().map(|progress| json!({
            "read": progress.read,
            "total": progress.total,
        })),
    })
}

fn window_state(editor: &Editor, window: &Window, index: usize) -> Value {
    let size = window.size();
    let display = editor.buffer(window.buffer()).map(Buffer::display);
    let selections = &window.selections;
    let primary = selections.primary();
    json!({
        "buffer": window.buffer().get(),
        "selected": index == editor.selected_window(),
        "rows": size.rows,
        "columns": size.columns,
        "scroll": { "top": window.scroll.top, "left": window.scroll.left },
        "primary": selections.iter().position(|selection| selection == primary),
        "selections": selections
            .iter()
            .map(|selection| selection_state(selection, display.as_ref()))
            .collect::<Vec<_>>(),
    })
}

fn selection_state(selection: &Selection, display: Option<&DisplaySnapshot>) -> Value {
    let point = display.map(|display| display.offset_to_display_point(selection.cursor));
    json!({
        "cursor": selection.cursor,
        "row": point.map(|point| point.row),
        "column": point.map(|point| point.column),
        "goal": match selection.goal {
            None => Value::Null,
            Some(Goal::Column(column)) => json!({ "column": column }),
            Some(Goal::LineEnd) => json!("line-end"),
        },
    })
}

/// The rows the window shows in full, from its top row and left column,
/// cut at its right edge. A wide character across either edge is included.
fn visible_text(window: &Window, buffer: &Buffer, display: &DisplaySnapshot) -> Value {
    let size = window.size();
    let scroll = window.scroll;
    let end_row = scroll
        .top
        .saturating_add(size.rows)
        .min(display.row_count());
    let right = scroll.left.saturating_add(size.columns);
    let rows: Vec<String> = (scroll.top..end_row)
        .map(|row| {
            let start =
                display.display_point_to_offset(DisplayPoint::new(row, scroll.left), Bias::Left);
            let end = display.display_point_to_offset(DisplayPoint::new(row, right), Bias::Right);
            slice(buffer, start..end)
        })
        .collect();
    json!({ "start_row": scroll.top, "start_column": scroll.left, "rows": rows })
}

fn slice(buffer: &Buffer, range: Range<usize>) -> String {
    buffer.text().chunks_in_range(range).collect()
}

/// The result of `get-text`: whole rows of the selected window's buffer,
/// without their line breaks. Rows past the end are left out. An error if
/// the buffer may hold secrets, or the rows are more than a response takes.
pub fn text(editor: &Editor, start_row: usize, rows: usize) -> Reply {
    let (window, buffer) = editor.current();
    if buffer.is_secret() {
        return Err(WITHHELD.into());
    }
    if rows > MAX_TEXT_ROWS {
        return Err(format!(
            "get-text returns at most {MAX_TEXT_ROWS} rows; {rows} were asked for"
        ));
    }
    let display = buffer.display();
    let row_count = display.row_count();
    let end_row = start_row.saturating_add(rows).min(row_count);
    let range = start_row.min(end_row)..end_row;
    if !range.is_empty() {
        let bytes =
            motion::line_end(&display, range.end - 1) - motion::line_start(&display, range.start);
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
                buffer,
                motion::line_start(&display, row)..motion::line_end(&display, row),
            )
        })
        .collect();
    Ok(json!({
        "buffer": window.buffer().get(),
        "start_row": start_row,
        "row_count": row_count,
        "rows": rows,
    }))
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
    use super::{MAX_TEXT, MAX_TEXT_ROWS, screenshot_allowed, show_buffer, state, text};
    use oso_core::buffer::{Buffer, BufferId};
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::selection::SelectionSet;
    use oso_core::window::ViewSize;
    use serde_json::json;
    use std::path::{Path, PathBuf};

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
        buffer.resolve_secret(resolved.map(Path::new));
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
        let state = state(&editor, &[], Some((800, 600)));
        let buffer = json!({
            "id": 0,
            "name": "bad.txt",
            "path": null,
            "secret": false,
            "read_only": "invalid-utf8",
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
                "buffer": buffer,
                "cursor": { "offset": 6, "row": 1, "column": 4, "count": 1 },
                "scroll": { "top": 0, "left": 3 },
                "window": { "rows": 5, "columns": 2 },
                "text": { "start_row": 0, "start_column": 3, "rows": ["", "\u{fffd}S"] },
                "buffers": [buffer],
                "windows": [{
                    "buffer": 0,
                    "selected": true,
                    "rows": 5,
                    "columns": 2,
                    "scroll": { "top": 0, "left": 3 },
                    "primary": 0,
                    "selections": [{ "cursor": 6, "row": 1, "column": 4, "goal": null }],
                }],
                "keys": { "pending": "" },
                "surface": { "width": 800, "height": 600 },
            })
        );
        let pending: KeySequence = "C-x 4".parse().unwrap();
        let state = super::state(&Editor::default(), &pending.0, None);
        assert_eq!(state["keys"]["pending"], "C-x 4");
        assert_eq!(state["buffer"]["name"], "*scratch*");
        assert_eq!(state["surface"], json!(null));
    }

    #[test]
    fn visible_text_is_what_the_window_shows() {
        let content: String = (0..100).map(|i| format!("line {i:02}\n")).collect();
        let mut editor = editor(&content, 3, 6);
        assert_eq!(
            state(&editor, &[], None)["text"],
            json!({
                "start_row": 0,
                "start_column": 0,
                "rows": ["line 0", "line 0", "line 0"],
            })
        );
        editor.current_mut().0.scroll.top = 98;
        editor.current_mut().0.scroll.left = 5;
        assert_eq!(
            state(&editor, &[], None)["text"],
            json!({ "start_row": 98, "start_column": 5, "rows": ["98", "99", ""] }),
            "only rows that exist, and the columns from the left edge",
        );
    }

    #[test]
    fn visible_text_includes_wide_characters_across_the_edges() {
        let mut editor = editor("日本語\n", 1, 2);
        editor.current_mut().0.scroll.left = 1;
        assert_eq!(state(&editor, &[], None)["text"]["rows"], json!(["日本"]));
    }

    #[test]
    fn a_huge_line_is_clipped_to_the_window() {
        let long = "x".repeat(2 * 1024 * 1024);
        let editor = editor(&long, 2, 80);
        let state = state(&editor, &[], None);
        assert_eq!(state["text"]["rows"], json!(["x".repeat(80)]));
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
            let state = state(&editor, &[], None);
            assert_eq!(state["text"], json!(null), "{path}");
            assert_eq!(state["buffer"]["secret"], true, "{path}");
            assert_eq!(state["buffer"]["path"], path);
            assert_eq!(state["buffer"]["bytes"], CONTENT.len());
            assert_eq!(state["cursor"]["offset"], 0);
            assert!(!state.to_string().contains("SECRET-VALUE"), "{path}");
            assert!(text(&editor, 0, 1).unwrap_err().contains("ADR 0043"));
            assert!(
                screenshot_allowed(&editor)
                    .unwrap_err()
                    .contains("ADR 0043")
            );
        }
    }

    #[test]
    fn a_file_is_withheld_until_its_path_is_resolved() {
        let mut editor = Editor::default();
        let id = editor.add_buffer(Buffer::visiting(PathBuf::from("/p/notes.txt")));
        editor.show(id);
        assert_eq!(state(&editor, &[], None)["text"], json!(null));
        assert!(screenshot_allowed(&editor).is_err());
        assert!(text(&editor, 0, 1).is_err());
        editor
            .buffer_mut(id)
            .unwrap()
            .resolve_secret(Some(Path::new("/p/notes.txt")));
        assert!(screenshot_allowed(&editor).is_ok());
        assert!(text(&editor, 0, 1).is_ok());
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
        let state = state(&editor, &[], None);
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
        let state = state(&editor, &[], None);
        assert_eq!(state["windows"][0]["primary"], 0);
        assert_eq!(
            state["windows"][0]["selections"],
            json!([{ "cursor": 0, "row": 0, "column": 0, "goal": null }])
        );
    }
}
