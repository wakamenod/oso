//! Tests of the thread that searches a buffer's lines (ADR 0154): the first row goes at once,
//! the rest with the end, and a superseded search sends nothing.
//!
//! Must not depend on: `app` or winit; the batches go to a channel here.

use std::sync::mpsc;
use std::time::Duration;

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;

use super::*;

const WAIT: Duration = Duration::from_secs(10);

fn searcher() -> (LineSearcher, mpsc::Receiver<FoundLines>) {
    let (sender, batches) = mpsc::channel();
    let searcher = LineSearcher::new(move |found| sender.send(found).is_ok());
    (searcher, batches)
}

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
    editor.open_line_search("consult-line").unwrap();
    editor
}

#[test]
fn the_first_row_goes_at_once_and_the_rest_with_the_end() {
    let (mut searcher, batches) = searcher();
    let mut editor = editor("needle\nb\nc\n");
    let request = editor.take_line_search_request().unwrap();
    let generation = request.generation;
    searcher.search(request);
    let first = batches.recv_timeout(WAIT).unwrap();
    assert_eq!(
        (first.generation, first.rows.len(), first.end),
        (generation, 1, None)
    );
    let mut rows = 1;
    loop {
        let found = batches.recv_timeout(WAIT).unwrap();
        rows += found.rows.len();
        if let Some(end) = found.end {
            assert!(!end.cut);
            break;
        }
    }
    assert_eq!(rows, 3);
    assert!(!format!("{first:?}").contains("needle"));
}

#[test]
fn a_superseded_search_sends_nothing() {
    let (mut searcher, batches) = searcher();
    let mut editor = editor("a\nb\n");
    let request = editor.take_line_search_request().unwrap();
    // The prompt closing moves the generation before the thread takes it.
    editor.quit();
    searcher.search(request);
    assert!(batches.recv_timeout(Duration::from_millis(200)).is_err());
}
