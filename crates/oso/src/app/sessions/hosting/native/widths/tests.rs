//! What a session's rows draw for a width, drawn again when it changes: in view, while a reply
//! streams, and for the newest tables in slices.
//!
//! Must not depend on: the syntax threads or a process.

use oso_core::frame::FrameSize;
use oso_core::minibuffer::cells_of;

use super::super::markdown::tests::native;
use super::*;
use crate::sessions::native::wire::parse;

const RESULT: &[u8] = br#"{"type":"result","subtype":"success","duration_ms":1500}"#;

fn sync(editor: &mut Editor, native: &mut Native) -> bool {
    let mut shown = Vec::new();
    editor.shown_ranges(&mut shown);
    lay(editor, native, &shown, &mut { SLICE_BYTES })
}

/// How wide each result is drawn, in columns.
fn widths(editor: &Editor, native: &Native) -> Vec<usize> {
    let text = editor.buffer(native.buffer).unwrap().text().to_string();
    (text.lines().filter(|line| !line.is_empty()))
        .map(|line| line.chars().count())
        .collect()
}

#[test]
fn results_in_view_stand_at_the_edge_of_the_narrowest_window() {
    let mut editor = Editor::default();
    let size = |rows, columns| FrameSize { rows, columns };
    editor.set_frame_size(size(12, 60));
    let id = editor.add_native_session_buffer("*claude: s*");
    editor.show(id);
    let mut native = native(id);
    assert!(!sync(&mut editor, &mut native), "nothing is drawn yet");
    for _ in 0..40 {
        let changes = native.rows.apply(parse(RESULT));
        draw(&mut editor, &mut native, changes);
    }
    let columns = |editor: &Editor| {
        let window = editor.window().id();
        editor.regions(window).unwrap().text.columns - 1
    };
    assert_eq!(widths(&editor, &native), vec![columns(&editor); 40]);
    assert!(!sync(&mut editor, &mut native), "laid out already");

    // Only what is in view is drawn again.
    editor.set_frame_size(size(12, 40));
    assert!(sync(&mut editor, &mut native));
    let narrow = columns(&editor);
    let drawn = widths(&editor, &native);
    let again = drawn.iter().filter(|width| **width == narrow).count();
    assert!((1..12).contains(&again), "{drawn:?}");
    assert!(!sync(&mut editor, &mut native), "once");
    // A new one takes the new width.
    let changes = native.rows.apply(parse(RESULT));
    draw(&mut editor, &mut native, changes);
    assert_eq!(widths(&editor, &native).last(), Some(&narrow));
}

const TABLE: &str =
    "a `b`\n| Key | Words |\n|---|---|\n| w | some words that wrap in a narrow window |\n\n`end`";

/// A session shown in a frame of `columns`, which wraps.
fn shown(columns: usize) -> (Editor, Native) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows: 12, columns });
    let id = editor.add_native_session_buffer("*claude: s*");
    editor.show(id);
    editor.set_wraps(id, true);
    let mut native = native(id);
    sync(&mut editor, &mut native);
    (editor, native)
}

fn feed(editor: &mut Editor, native: &mut Native, line: &serde_json::Value) {
    let changes = native.rows.apply(parse(line.to_string().as_bytes()));
    draw(editor, native, changes);
}

fn reply(id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({"type": "assistant", "message": {"id": id, "content": [{"type": "text", "text": text}]}})
}

fn stream(id: &str, event: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"type": "stream_event", "api_message_id": id, "event": event})
}

fn delta(text: &str) -> serde_json::Value {
    let delta = serde_json::json!({"type": "text_delta", "text": text});
    stream(
        "m",
        serde_json::json!({"type": "content_block_delta", "index": 0, "delta": delta}),
    )
}

/// The buffer's text and highlights are what its rows are, each drawn whole for its width.
fn is_drawn(editor: &Editor, native: &Native) {
    let buffer = editor.buffer(native.buffer).unwrap();
    let (mut text, mut runs) = (String::new(), Vec::new());
    for (at, row) in native.rows.rows().iter().enumerate() {
        let drawn = render::render(row, native.rows.directory(), native.row_width(at));
        let covered: usize = drawn.runs().iter().map(|run| run.0).sum();
        runs.extend(drawn.runs());
        runs.push((drawn.text.len() - covered, None));
        text.push_str(&drawn.text);
    }
    assert_eq!(buffer.text().to_string(), text);
    let mut faces = oso_core::syntax::Highlights::new(text.len());
    faces.replace(0..text.len(), runs.into_iter().filter(|run| run.0 > 0));
    let on = buffer.highlights().unwrap();
    assert!(on.runs(0..text.len()).eq(faces.runs(0..text.len())));
}

#[test]
fn a_table_is_drawn_for_the_narrowest_window() {
    let (mut editor, mut native) = shown(60);
    feed(&mut editor, &mut native, &reply("a", TABLE));
    is_drawn(&editor, &native);
    let text = |editor: &Editor| editor.buffer(native_id(editor)).unwrap().text().to_string();
    let one = text(&editor);
    assert!(
        one.contains("\n  │ w   │ some words that wrap in a narrow window │\n"),
        "{one}"
    );
    // What is put on its row since is kept but on the table's lines.
    let end = one.find("end").unwrap();
    let mut highlights = editor
        .buffer(native.buffer)
        .unwrap()
        .highlights()
        .unwrap()
        .clone();
    let keyword = Some(oso_core::syntax::Highlight::Keyword);
    highlights.replace(end..end + 3, [(3, keyword)]);
    editor.set_internal_highlights(native.buffer, highlights);

    // A second window, narrower: the one text fits both.
    editor
        .split_window(oso_core::frame::Way::Across, true)
        .unwrap();
    assert!(sync(&mut editor, &mut native));
    assert!(!sync(&mut editor, &mut native), "once");
    let half = text(&editor);
    let width = native.rows.width();
    assert!(width < 32);
    assert!(half.lines().all(|line| cells_of(line) <= width), "{half}");
    assert!(half.contains("\n  ├─────┼"), "{half}");
    let end = half.find("end").unwrap();
    let on = editor.buffer(native.buffer).unwrap().highlights().unwrap();
    assert_eq!(on.at(end), keyword);
    assert_eq!(native.placed.end(), half.len());
    // The narrower one gone, it is as it was.
    editor.delete_other_windows();
    assert!(sync(&mut editor, &mut native));
    assert_eq!(text(&editor), one);
}

fn native_id(editor: &Editor) -> BufferId {
    editor.window().buffer()
}

#[test]
fn a_reply_that_streams_grows_on_for_a_new_width() {
    let (mut editor, mut native) = shown(60);
    let size = |columns| FrameSize { rows: 12, columns };
    let block = serde_json::json!({"type": "text", "text": ""});
    let start =
        serde_json::json!({"type": "content_block_start", "index": 0, "content_block": block});
    feed(&mut editor, &mut native, &stream("m", start));
    let text = format!("{TABLE}\n\n| c | d |\n|---|---|\n| some | more words that wrap too |\n\nx");
    let mut columns = [40, 60, 30, 100, 24, 50].into_iter().cycle();
    let mut at = 0;
    while at < text.len() {
        let next = text.ceil_char_boundary(at + 7);
        feed(&mut editor, &mut native, &delta(&text[at..next]));
        at = next;
        is_drawn(&editor, &native);
        editor.set_frame_size(size(columns.next().unwrap()));
        sync(&mut editor, &mut native);
        is_drawn(&editor, &native);
    }
    feed(&mut editor, &mut native, &reply("m", &text));
    is_drawn(&editor, &native);
}

#[test]
fn the_newest_tables_are_drawn_again_in_slices_and_the_others_in_view() {
    let (mut editor, mut native) = shown(80);
    let long = format!("{TABLE}\n{}", "words\n".repeat(200));
    for n in 0..NEWEST + 40 {
        feed(&mut editor, &mut native, &reply(&format!("m{n}"), &long));
    }
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 40,
    });
    let stale = |native: &Native| {
        let width = native.rows.width();
        let rows = native.widths.rows.values();
        rows.filter(|row| row.width != width).count()
    };
    let mut events = 0;
    while sync(&mut editor, &mut native) {
        events += 1;
        assert!(events < 40);
        is_drawn(&editor, &native);
    }
    assert!(events > 1, "in slices");
    assert_eq!(stale(&native), 40, "the oldest wait");
    // As they come into view.
    while editor.scroll_lines(-1000).is_ok_and(|moved| moved) {}
    assert!(sync(&mut editor, &mut native));
    assert!(stale(&native) < 40);
    is_drawn(&editor, &native);
}

#[test]
fn a_box_is_at_most_a_hundred_columns_and_in_the_window_at_any_depth() {
    let words = "words ".repeat(60);
    let text = format!("| a |\n|---|\n| {words}|");
    for (columns, most) in [(140, 100), (60, 60)] {
        let (mut editor, mut native) = shown(columns);
        feed(&mut editor, &mut native, &reply("a", &text));
        let mut nested = reply("b", &text);
        nested["parent_tool_use_id"] = "t".into();
        feed(&mut editor, &mut native, &nested);
        let most = most.min(native.rows.width());
        let buffer = editor.buffer(native.buffer).unwrap().text().to_string();
        let tops: Vec<&str> = buffer.lines().filter(|line| line.contains('┌')).collect();
        assert!(
            tops[0].starts_with("  ┌") && tops[1].starts_with("│   ┌"),
            "{tops:?}"
        );
        // As wide as they may be.
        assert_eq!(
            tops.iter().map(|top| cells_of(top)).collect::<Vec<_>>(),
            [most, most]
        );
    }
}

#[test]
fn earlier_turns_past_the_bound_come_in_as_written_and_are_drawn_after() {
    let (mut editor, mut native) = shown(80);
    let rows = "| a | b | c | d |\n".repeat(300);
    let text = format!("| a | b | c | d |\n|---|---|---|---|\n{rows}");
    for n in 0..80 {
        native
            .rows
            .apply(parse(reply(&format!("m{n}"), &text).to_string().as_bytes()));
    }
    super::super::past::draw_added(&mut editor, &mut native, 0);
    is_drawn(&editor, &native);
    let boxes = |editor: &Editor| {
        let text = editor.buffer(native_id(editor)).unwrap().text().to_string();
        text.matches('┌').count()
    };
    let first = boxes(&editor);
    assert!((1..80).contains(&first), "{first}");
    assert_eq!(native.row_width(79), 0);
    let mut events = 0;
    while sync(&mut editor, &mut native) {
        events += 1;
        assert!(events < 80);
    }
    is_drawn(&editor, &native);
    assert_eq!(native.row_width(79), native.rows.width(), "the newest");
    // No more than the bound's are drawn while out of view.
    assert!(boxes(&editor) < 80);
}

#[test]
fn a_session_no_window_shows_is_left_alone() {
    let (mut editor, mut native) = shown(60);
    feed(&mut editor, &mut native, &reply("a", TABLE));
    let id = native.buffer;
    let other = editor.add_native_session_buffer("*claude: other*");
    editor.show(other);
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 30,
    });
    assert!(!sync(&mut editor, &mut native));
    assert_eq!(native.rows.width(), 59);
    editor.show(id);
    assert!(sync(&mut editor, &mut native));
    assert!(native.rows.width() < 30);
    is_drawn(&editor, &native);
}
