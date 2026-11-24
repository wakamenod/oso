//! What a request's block costs the UI thread at the end of a long transcript, printed to be read: run
//! by hand, in release (`cargo test -p oso --release permissions::request::measure_tests -- --ignored
//! --nocapture`).
//!
//! Must not depend on: the app; the tests draw as it does.

use std::time::Instant;

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::window::{DockSlot, Screen};

use super::read::view_of;
use super::{put_title, runs};
use crate::app::sessions::hosting::native::markdown::tests::native;
use crate::app::sessions::hosting::native::{NativeSessions, put_tail};
use crate::sessions::native::request::measure_tests::{calls, times};
use crate::sessions::native::request::reading::View;
use crate::sessions::native::request::{Block, COMPARE_WITHIN, Request};
use crate::sessions::native::tail::Part;

const KEY: u64 = 7;

fn transcript(columns: usize) -> (Editor, NativeSessions) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows: 64, columns });
    let id = editor.add_native_session_buffer("*claude: s*");
    editor.set_wraps(id, true);
    editor.show(id);
    let row = "  The refresh token is compared before it is trimmed, so a break fails it.\n\n";
    let rows = row.repeat(10_000);
    editor.replace_native_rows(id, 0..0, &rows, &[(rows.len(), None)]);
    let line = editor.add_internal_buffer("*request: s*", Mode::Request);
    editor.set_dock(id, DockSlot::Card, Some(line));
    let mut its = native(id);
    its.placed.put(0, 0..0, rows.len());
    its.line = Some(line);
    let mut natives = NativeSessions::default();
    natives.map.insert(KEY, its);
    (editor, natives)
}

#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(clippy::disallowed_macros, reason = "the timing is printed to be read")]
fn what_a_block_costs_the_ui_thread() {
    for columns in [20, 62, 100] {
        let (mut editor, mut natives) = transcript(columns);
        for (what, tool, input) in &calls() {
            let deadline = Instant::now() + COMPARE_WITHIN * 100;
            let block = Block::call(tool, Some(input), deadline);
            let body = block.body();
            let mut screen = Screen::default();
            let window = editor.window().id();
            // Of the same block, which changes nothing: the frame's rows alone.
            let same = times(|| {
                let native = natives.map.get_mut(&KEY).unwrap();
                editor.refill_wraps();
                put_title(&mut editor, native, &block, ((1, 1), columns - 1));
                let its = runs(body);
                put_tail(&mut editor, native, Part::Body, body.text.clone(), its);
                editor.screen_of(window, &mut screen);
            });
            // Put as it comes, in place of nothing and of another, and taken away as it goes, with the
            // rows of the first frame.
            let mut put_took = Vec::new();
            let mut take_took = Vec::new();
            let mut over_took = Vec::new();
            let word = serde_json::json!({"command": "\u{301}a\n".repeat(8000)});
            let other = Block::call("Bash", Some(&word), deadline);
            for _ in 0..100 {
                let native = natives.map.get_mut(&KEY).unwrap();
                let started = Instant::now();
                for part in [Part::Title, Part::Body] {
                    put_tail(&mut editor, native, part, String::new(), Vec::new());
                }
                take_took.push(started.elapsed());
                let started = Instant::now();
                editor.refill_wraps();
                put_title(&mut editor, native, &block, ((1, 1), columns - 1));
                let its = runs(body);
                put_tail(&mut editor, native, Part::Body, body.text.clone(), its);
                editor.screen_of(window, &mut screen);
                put_took.push(started.elapsed());
                // As the answer of one shows the next.
                let body_of_other = other.body();
                let its = runs(body_of_other);
                put_tail(
                    &mut editor,
                    native,
                    Part::Body,
                    body_of_other.text.clone(),
                    its,
                );
                let started = Instant::now();
                editor.refill_wraps();
                put_title(&mut editor, native, &block, ((1, 2), columns - 1));
                let its = runs(body);
                put_tail(&mut editor, native, Part::Body, body.text.clone(), its);
                editor.screen_of(window, &mut screen);
                over_took.push(started.elapsed());
            }
            over_took.sort_unstable();
            put_took.sort_unstable();
            take_took.sort_unstable();
            eprintln!(
                "{columns} columns, {what}: put p50 {:?} p98 {:?}, taken p98 {:?}; in place of another p98 {:?}; the same {same}",
                put_took[50], put_took[98], take_took[98], over_took[98]
            );
            let mut reading = Request::new(block.clone()).reading;
            let mut view = View::default();
            let now = Instant::now();
            let look = times(|| {
                view_of(&editor, &natives, (KEY, true), (&mut screen, &mut view));
                reading.look(&view, true, now);
            });
            eprintln!("  a look {look}");
            let rows = view.rows.len();
            let scroll = times(|| {
                editor.scroll_view(window, -20);
                editor.scroll_view(window, 20);
            });
            eprintln!("  scrolled up and down {scroll}, {rows} rows in view");
            let (text, its) = (body.text.len(), runs(body).len());
            eprintln!("  {text} bytes and {its} runs of it kept");
        }
    }
}
