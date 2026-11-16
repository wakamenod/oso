//! Tests of a project search's preview (ADR 0114): a read asked for once per row and dropped
//! once stale, an open buffer shown from memory, and never a secret file.
//!
//! Must not depend on: threads or the file system.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use crate::buffer::{Baseline, Buffer, FileStamp};
use crate::display_map::TabWidth;
use crate::editor::edit_tests::edit;
use crate::editor::{
    Editor, Excerpt, FilePreviewRead, MatchCells, Preview, PreviewAt, ProjectSearchRow, VisitColumn,
};
use crate::frame::FrameSize;
use crate::load::decode_all;
use crate::window::Screen;

/// An editor with a frame wide enough for the float's preview, and a search of `/w/oso` open
/// whose input is `needle`, with the rows of `rows` taken in.
fn searching(rows: Vec<ProjectSearchRow>) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 160,
    });
    editor
        .open_project_search(PathBuf::from("/w/oso"), "oso", "project-search")
        .unwrap();
    let id = editor.prompt().unwrap().buffer();
    editor.apply_edits(id, &[edit(0..0, "needle")]).unwrap();
    editor.match_completion();
    let generation = editor.take_search_request().unwrap().generation;
    editor.rows_found(generation, rows, None);
    editor
}

fn row(path: &str, line: usize, offset: u64) -> ProjectSearchRow {
    ProjectSearchRow {
        path: path.into(),
        line,
        offset,
        column: 4,
        text: "one needle".into(),
        secret: false,
    }
}

/// A buffer visiting `path`, holding `text`, judged ordinary, of the file `identity`.
fn visiting(path: &str, text: &str, identity: Option<(u64, u64)>) -> Buffer {
    let path = PathBuf::from(path);
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(text.as_bytes(), TabWidth::default()));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    buffer.set_baseline(Baseline {
        location: path,
        file: Some(FileStamp {
            modified: std::time::SystemTime::UNIX_EPOCH,
            len: text.len() as u64,
            mode: 0o644,
            identity,
        }),
    });
    buffer
}

#[test]
fn a_read_is_asked_for_once_per_row_and_a_stale_one_is_dropped() {
    let mut editor = searching(vec![row("a.rs", 3, 40), row("b.rs", 9, 120)]);
    let request = editor
        .take_preview_request()
        .expect("a read for the first row");
    assert_eq!(&*request.path, "a.rs");
    assert_eq!(
        request.at,
        PreviewAt::Offset {
            offset: 40,
            line: 3,
            column: 4
        }
    );
    assert_eq!(request.root, Path::new("/w/oso"));
    assert!(request.rows > 0);
    assert!(!format!("{request:?}").contains("a.rs"));
    assert!(editor.take_preview_request().is_none(), "once per row");
    assert_eq!(editor.preview(), None, "nothing until the read comes");
    editor.move_completion(1).unwrap();
    let second = editor
        .take_preview_request()
        .expect("a read for the next row");
    assert_ne!(second.generation, request.generation);
    assert_ne!(
        request.live.load(Ordering::Relaxed),
        request.generation,
        "a newer selection stops the read"
    );
    let excerpt = Excerpt::new(b"one needle\n", 9, (9, VisitColumn::Chars(4)), None);
    editor.preview_read(request.generation, FilePreviewRead::Text(excerpt));
    assert_eq!(editor.preview(), None, "the stale read is dropped");
    let excerpt = Excerpt::new(b"one needle\n", 9, (9, VisitColumn::Chars(4)), None);
    editor.preview_read(second.generation, FilePreviewRead::Text(excerpt));
    let Some(Preview::Excerpt { cursor, .. }) = editor.preview() else {
        panic!("the excerpt shows");
    };
    assert_eq!(cursor, 4);
    assert!(editor.preview_excerpt_display().is_some());
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    assert_eq!(screen.rows().len(), 1);
    let mut cells = MatchCells::default();
    editor.preview_cells(&screen, &mut cells);
    assert_eq!(cells.current.len(), 1, "the row's match is the current one");
    assert!(cells.others.is_empty());
}

#[test]
fn no_read_is_asked_for_while_the_float_has_no_room_for_a_preview() {
    let mut editor = searching(vec![row("a.rs", 3, 40)]);
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 40,
    });
    assert!(editor.take_preview_request().is_none());
    // Once it has, the row selected is read.
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 160,
    });
    assert!(editor.take_preview_request().is_some());
}

#[test]
fn a_new_input_or_closing_the_prompt_stops_the_read() {
    let mut editor = searching(vec![row("a.rs", 3, 40)]);
    let request = editor.take_preview_request().unwrap();
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, "s")]).unwrap();
    editor.match_completion();
    assert_ne!(request.live.load(Ordering::Relaxed), request.generation);
    let mut editor = searching(vec![row("a.rs", 3, 40)]);
    let request = editor.take_preview_request().unwrap();
    editor.quit();
    assert_ne!(request.live.load(Ordering::Relaxed), request.generation);
}

#[test]
fn a_secret_row_is_never_read_or_shown() {
    let secret = ProjectSearchRow {
        secret: true,
        text: "".into(),
        ..row(".env", 2, 0)
    };
    let mut editor = searching(vec![secret, row("a.rs", 1, 0)]);
    assert!(editor.take_preview_request().is_none());
    assert_eq!(editor.preview(), Some(Preview::Withheld));
    // A file the host found may hold secrets as it read it says so too.
    editor.move_completion(1).unwrap();
    let request = editor.take_preview_request().unwrap();
    editor.preview_read(request.generation, FilePreviewRead::Withheld);
    assert_eq!(editor.preview(), Some(Preview::Withheld));
    assert!(editor.preview_excerpt_display().is_none());
}

#[test]
fn an_unreadable_file_shows_nothing() {
    let mut editor = searching(vec![row("a.rs", 1, 0)]);
    let request = editor.take_preview_request().unwrap();
    editor.preview_read(request.generation, FilePreviewRead::Unreadable);
    assert_eq!(editor.preview(), None);
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    assert!(screen.rows().is_empty());
}

#[test]
fn an_open_buffer_of_the_file_is_shown_from_memory() {
    let mut editor = searching(vec![row("a.rs", 2, 6)]);
    let text = "first\none needle\nthird\n";
    let id = editor.add_buffer(visiting("/w/oso/a.rs", text, None));
    assert!(editor.take_preview_request().is_none(), "nothing to read");
    assert_eq!(
        editor.preview(),
        Some(Preview::Buffer {
            id,
            start: 0,
            cursor: 10
        })
    );
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    let mut cells = MatchCells::default();
    editor.preview_cells(&screen, &mut cells);
    assert_eq!(cells.current.len(), 1);
}

#[test]
fn a_row_an_open_buffer_showed_is_read_once_that_buffer_is_killed() {
    let mut editor = searching(vec![row("a.rs", 2, 6)]);
    let id = editor.add_buffer(visiting("/w/oso/a.rs", "first\none needle\n", None));
    assert!(editor.take_preview_request().is_none());
    assert!(
        editor.take_preview_request().is_none(),
        "nothing while it shows"
    );
    editor.kill_buffer(id);
    let request = editor
        .take_preview_request()
        .expect("a read once it has gone");
    assert_eq!(&*request.path, "a.rs");
    assert!(editor.take_preview_request().is_none(), "once");
}

#[test]
fn a_buffer_the_agent_channel_is_still_opening_is_not_previewed() {
    let mut editor = searching(vec![row("a.rs", 2, 6)]);
    let mut by_path = visiting("/w/oso/a.rs", "first\none needle\n", None);
    by_path.set_agent_pending(true);
    editor.add_buffer(by_path);
    let mut by_identity = visiting("/w/oso/b.rs", "first\none needle\n", Some((1, 2)));
    by_identity.set_agent_pending(true);
    editor.add_buffer(by_identity);
    // The file is read as if no buffer had it, and the read shows.
    let request = editor.take_preview_request().expect("a read");
    let excerpt = Excerpt::new(
        b"first\none needle\n",
        1,
        (2, VisitColumn::Chars(4)),
        Some((1, 2)),
    );
    editor.preview_read(request.generation, FilePreviewRead::Text(excerpt));
    assert!(matches!(editor.preview(), Some(Preview::Excerpt { .. })));
}

#[test]
fn an_open_buffer_of_the_same_file_by_its_identity_replaces_the_excerpt() {
    let mut editor = searching(vec![row("a.rs", 1, 0)]);
    let request = editor.take_preview_request().unwrap();
    let id = editor.add_buffer(visiting("/elsewhere/a.rs", "one needle\n", Some((7, 8))));
    let excerpt = Excerpt::new(b"one needle\n", 1, (1, VisitColumn::Chars(4)), Some((7, 8)));
    editor.preview_read(request.generation, FilePreviewRead::Text(excerpt));
    assert_eq!(
        editor.preview(),
        Some(Preview::Buffer {
            id,
            start: 0,
            cursor: 4
        })
    );
}

#[test]
fn an_open_buffer_that_may_hold_secrets_is_withheld() {
    let mut editor = searching(vec![row("a.rs", 1, 0)]);
    let mut buffer = visiting("/w/oso/a.rs", "one needle\n", None);
    buffer.treat_as_secret();
    editor.add_buffer(buffer);
    assert!(editor.take_preview_request().is_none());
    assert_eq!(editor.preview(), Some(Preview::Withheld));
}

#[test]
fn a_match_past_the_previews_width_scrolls_it_into_view() {
    let column = 400;
    let line = format!("{}needle", "z".repeat(column));
    let mut editor = searching(vec![ProjectSearchRow {
        column,
        ..row("a.rs", 1, 0)
    }]);
    let request = editor.take_preview_request().unwrap();
    let excerpt = Excerpt::new(line.as_bytes(), 1, (1, VisitColumn::Chars(column)), None);
    editor.preview_read(request.generation, FilePreviewRead::Text(excerpt));
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    let mut cells = MatchCells::default();
    editor.preview_cells(&screen, &mut cells);
    assert_eq!(cells.current.len(), 1, "the match shows");
}

#[test]
fn an_excerpt_of_a_file_open_in_a_buffer_that_may_hold_secrets_is_withheld() {
    let mut editor = searching(vec![row("a.rs", 1, 0)]);
    let request = editor.take_preview_request().unwrap();
    let mut buffer = visiting("/elsewhere/.env", "one needle\n", Some((7, 8)));
    buffer.treat_as_secret();
    editor.add_buffer(buffer);
    let excerpt = Excerpt::new(b"one needle\n", 1, (1, VisitColumn::Chars(4)), Some((7, 8)));
    editor.preview_read(request.generation, FilePreviewRead::Text(excerpt));
    assert_eq!(editor.preview(), Some(Preview::Withheld));
    assert!(
        editor.read_excerpt().is_none(),
        "nothing of the read is kept"
    );
    assert!(editor.preview_excerpt_display().is_none());
    let mut screen = Screen::default();
    editor.preview_screen(&mut screen);
    assert!(screen.rows().is_empty(), "nothing of it is laid out");
}
