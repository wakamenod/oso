//! The block of a call at the end of a real transcript: whatever the stream draws above it, whatever
//! waits in the queue and whichever call is shown, the rows end where the tail starts, the block is
//! its call's text in its own faces, which no row has, and a place in it stays on its byte.
//!
//! Must not depend on: the app; the tests draw as it does.
//!
//! It runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=100000 cargo test -p oso --release permissions::request::tail_tests`.

use std::time::{Duration, Instant};

use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::syntax::Highlight;
use proptest::prelude::*;
use serde_json::Value;

use super::{put_title, runs};
use crate::app::sessions::hosting::native::markdown::tests::native;
use crate::app::sessions::hosting::native::{Native, draw, put_tail};
use crate::sessions::native::fuzz_tests::line;
use crate::sessions::native::render::{Face, Rendered};
use crate::sessions::native::request::Block;
use crate::sessions::native::request::fuzz_tests::{call, only_oso_stands_first};
use crate::sessions::native::tail::Part;
use crate::sessions::native::wire::parse;

/// One change of a transcript.
#[derive(Debug, Clone)]
enum Change {
    /// Lines of the stream.
    Stream(Vec<Vec<u8>>),
    /// What waits in the queue.
    Queue(String),
    /// A call is shown, or the one shown goes.
    Call(Option<(String, Value)>),
    /// The title is drawn again.
    Title((usize, usize), usize),
    /// The person goes to this line of the block.
    Point(usize),
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        3 => prop::collection::vec(line(), 1..6).prop_map(Change::Stream),
        1 => "(Queued\\([0-9]\\)\n)?(  [a-z⚠$]{0,6}\n){0,3}".prop_map(Change::Queue),
        2 => prop::option::weighted(0.8, call()).prop_map(Change::Call),
        1 => ((1usize..4, 1usize..4), 0usize..140).prop_map(|(at, width)| Change::Title(at, width)),
        1 => (0usize..40).prop_map(Change::Point),
    ]
}

/// The runs of `rendered` as the buffer holds them, those alike put together.
fn joined(rendered: &Rendered) -> Vec<(usize, Option<Highlight>)> {
    let mut joined: Vec<(usize, Option<Highlight>)> = Vec::new();
    for (len, highlight) in runs(rendered) {
        match joined.last_mut() {
            Some((last, same)) if *same == highlight => *last += len,
            _ => joined.push((len, highlight)),
        }
    }
    joined.retain(|run| run.0 > 0);
    joined
}

/// Draws `change` in `editor` as the app does; the block shown.
fn apply(editor: &mut Editor, native: &mut Native, block: &mut Option<Block>, change: Change) {
    match change {
        Change::Stream(lines) => {
            for line in lines {
                for change in native.rows.apply(parse(&line)) {
                    draw(editor, native, [change]);
                }
            }
        }
        Change::Queue(rows) => {
            let its = vec![(rows.len(), Some(Face::Dim.highlight()))];
            put_tail(editor, native, Part::Queue, rows, its);
        }
        Change::Call(Some((tool, input))) => {
            let within = Instant::now() + Duration::from_secs(5);
            let new = Block::call(&tool, Some(&input), within);
            put_title(editor, native, &new, ((1, 1), 60));
            let body = new.body();
            put_tail(editor, native, Part::Body, body.text.clone(), runs(body));
            *block = Some(new);
        }
        Change::Call(None) => {
            for part in [Part::Title, Part::Body] {
                put_tail(editor, native, part, String::new(), Vec::new());
            }
            *block = None;
        }
        Change::Title(place, width) => {
            if let Some(block) = block {
                put_title(editor, native, block, (place, width));
            }
        }
        Change::Point(line) => {
            let Some(block) = block else { return };
            let body = native.part_place(Part::Body).start;
            let lines = block.body().text.split_inclusive('\n');
            let at: usize = lines.take(line).map(str::len).sum();
            if at < block.body().text.len() {
                editor.current_mut().0.selections.move_primary(body + at);
            }
        }
    }
}

proptest! {
    #[test]
    fn a_block_stays_apart_and_whole_at_a_transcripts_end(
        changes in prop::collection::vec(change(), 1..24),
        columns in 20usize..100,
    ) {
        let mut editor = Editor::default();
        editor.set_frame_size(FrameSize { rows: 30, columns });
        let id = editor.add_native_session_buffer("*claude: s*");
        editor.set_wraps(id, true);
        editor.show(id);
        let mut native = native(id);
        let mut block: Option<Block> = None;
        for change in changes {
            let body = native.part_place(Part::Body);
            let point = editor.window().selections.primary().cursor;
            let held = body.contains(&point).then(|| point - body.start);
            let moves = matches!(change, Change::Call(_) | Change::Point(_));
            apply(&mut editor, &mut native, &mut block, change);

            let buffer = editor.buffer(id).unwrap();
            let text = buffer.text().to_string();
            prop_assert_eq!(text.len(), native.placed.end() + native.tail.len());
            let shown = native.part_place(Part::Body);
            let wanted = block.as_ref().map_or("", |block| block.body().text.as_str());
            prop_assert_eq!(&text[shown.clone()], wanted);
            prop_assert_eq!(text.len(), shown.end);
            // The block's faces are its own, as made; no row above has one of them.
            let Some(highlights) = buffer.highlights() else { continue };
            let above = 0..native.part_place(Part::Title).start;
            let osos = [Highlight::CardTitle, Highlight::CardKey, Highlight::RequestBody];
            for (_, highlight) in highlights.runs(above) {
                prop_assert!(highlight.is_none_or(|highlight| !osos.contains(&highlight)));
            }
            if let Some(block) = &block {
                prop_assert_eq!(only_oso_stands_first(block.body()), Ok(()));
                let mut now: Vec<(usize, Option<Highlight>)> = Vec::new();
                for (range, highlight) in highlights.runs(shown.clone()) {
                    match now.last_mut() {
                        Some((last, same)) if *same == highlight => *last += range.len(),
                        _ => now.push((range.len(), highlight)),
                    }
                }
                prop_assert_eq!(now, joined(block.body()));
                let title = native.part_place(Part::Title);
                let mut first = highlights.runs(title.start + 1..title.end).map(|run| run.1);
                prop_assert!(first.all(|h| h == Some(Highlight::CardTitle)));
                prop_assert_eq!(&text[title.start..=title.start], "\n");
            }
            // A place in it stays on its byte, whatever goes in above.
            if let (Some(at), false) = (held, moves) {
                let point = editor.window().selections.primary().cursor;
                prop_assert_eq!(point, native.part_place(Part::Body).start + at);
            }
        }
    }
}
