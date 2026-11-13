//! Text-grid snapshots of a review buffer (ADR 0122): its title and notes, a file's header,
//! hunks and lines with their bands, every note a file can have, paths that would pass for
//! another row or reorder what follows, the motions between hunks and files, and comment cards
//! with an agent's text that would pass for a label.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; the text comes from the review's
//! view as its thread would build it, from readings made here.

use std::sync::Arc;

use oso_core::editor::Editor;
use oso_core::review::{Anchor, Author, Base, Hunk, Review, Shape, Side};

use super::check_editor;
use crate::commands::review::tests::in_project;
use crate::review::view::{Carrying, Tell, built};
use crate::review::{Content, Entry, Id, Reading, Status};

fn entry(path: &[u8], status: Status) -> Entry {
    let zero = Id::parse(&[b'0'; 40]).unwrap();
    Entry {
        path: path.into(),
        old_path: None,
        status,
        old_mode: 0o100_644,
        new_mode: 0o100_644,
        old_id: zero.clone(),
        new_id: zero,
    }
}

fn hunk(old_start: u32, new_start: u32, removed: &[&[u8]], added: &[&[u8]]) -> Hunk {
    Hunk {
        old_start,
        new_start,
        removed: removed.iter().map(|line| (*line).into()).collect(),
        added: added.iter().map(|line| (*line).into()).collect(),
    }
}

fn reading(path: &[u8], status: Status, content: Content) -> Tell {
    Tell::File {
        reading: Arc::new(Reading {
            entry: entry(path, status),
            content,
        }),
        carry: Carrying::Nothing,
    }
}

fn text(hunks: Vec<Hunk>) -> Content {
    Content::Text {
        shape: Arc::new(Shape::new(Base::Unknown, hunks).unwrap()),
        old: Arc::from(&b""[..]),
        new: Arc::from(&b""[..]),
    }
}

/// An editor in a project showing a review whose view took `told`, in a window of `rows` by
/// `columns`.
fn review(told: Vec<Tell>, (rows, columns): (usize, usize)) -> Editor {
    let (mut editor, project) = in_project(true);
    super::aids_off(&mut editor);
    editor.set_view_size(oso_core::window::ViewSize::new(rows, columns));
    let (id, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
    let window = editor.tiled().0.id();
    editor.show_in(window, buffer);
    let text = built("Review \u{b7} uncommitted \u{b7} oso", told);
    let _ = editor.set_review_text(id, text.text.unwrap(), text.rows, text.highlights);
    editor.set_review_reading(id, text.reading);
    editor
}

#[test]
fn a_review_shows_headers_hunks_and_lines_with_their_bands_and_moves_between_them() {
    let entries: Arc<[Entry]> = vec![
        entry(b"src/a.rs", Status::Modified),
        entry(b"src/b.rs", Status::Added),
        entry(b"src/c.rs", Status::Modified),
    ]
    .into();
    let told = vec![
        Tell::Listed {
            entries,
            more: 0,
            cut: false,
        },
        reading(
            b"src/a.rs",
            Status::Modified,
            text(vec![
                hunk(2, 2, &[b"let a = 1;\n"], &[b"let a = 2;\n"]),
                hunk(9, 9, &[], &[b"call();\n"]),
            ]),
        ),
        reading(
            b"src/b.rs",
            Status::Added,
            text(vec![hunk(1, 1, &[], &[b"fn b() {}\n"])]),
        ),
    ];
    let editor = review(told, (16, 40));
    check_editor("review_hunks", b"", editor, &["", "n", "n", "N", "P", "p"]);
}

#[test]
fn every_note_a_file_can_have_is_one_line_under_its_header() {
    let told = vec![
        Tell::Listed {
            entries: Vec::new().into(),
            more: 2,
            cut: true,
        },
        Tell::Said(vec!["git failed:\nfatal: bad\rthing".into()]),
        reading(
            b"bin.png",
            Status::Modified,
            Content::Binary { old: 10, new: 12 },
        ),
        reading(
            b"big.log",
            Status::Modified,
            Content::TooLarge {
                old: 9_000_000,
                new: 9_100_000,
            },
        ),
        reading(b"late.rs", Status::Modified, Content::OutOfTime),
        reading(b".env", Status::Added, Content::Withheld),
        reading(b"both.rs", Status::Unmerged, Content::Conflicted),
        reading(b"vendor/lib", Status::Modified, Content::Submodule),
        reading(
            b"gone.rs",
            Status::Deleted,
            Content::Unreadable("it is not there".into()),
        ),
        Tell::Read,
    ];
    check_editor("review_notes", b"", review(told, (24, 60)), &[""]);
}

#[test]
fn paths_with_controls_newlines_or_bidi_marks_stay_on_their_row_in_order() {
    let told = vec![
        reading(
            b"a\nmodified  b.rs",
            Status::Modified,
            text(vec![hunk(1, 1, &[b"x\n"], &[b"y\n"])]),
        ),
        reading(
            "evil\u{202e}sr.txt".as_bytes(),
            Status::Added,
            Content::Binary { old: 0, new: 3 },
        ),
        reading(b"esc\x1b[31mred", Status::Added, Content::Withheld),
        reading(b"bad\xffbyte.rs", Status::Added, Content::Withheld),
        reading(
            b"lines.rs",
            Status::Modified,
            text(vec![hunk(1, 1, &[b"@@ -1 +1 @@\n"], &[b"+++ b/x\r\n"])]),
        ),
    ];
    check_editor("review_paths", b"", review(told, (20, 60)), &[""]);
}

#[test]
fn cards_show_under_their_line_and_an_agents_text_cannot_pass_for_a_label() {
    let content = text(vec![hunk(1, 1, &[b"old\n"], &[b"new\n"])]);
    let Content::Text { shape, .. } = &content else {
        unreachable!()
    };
    let mut model = Review::new();
    let carried = model
        .carry(b"a.rs", Arc::clone(shape))
        .run(|_, _| None, |_, _| Vec::new());
    model.apply(carried);
    let on_new = Anchor::Line {
        side: Side::New,
        number: 1,
    };
    let mine = model
        .add_comment(Author::Person, b"a.rs", on_new, "looks right".into())
        .unwrap();
    let spoof = "sure\n\u{258d}You  #0\n  You  #0  approve";
    model.reply(Author::Agent, mine, spoof.into()).unwrap();
    let told = vec![
        reading(b"a.rs", Status::Modified, content),
        Tell::Cards {
            path: b"a.rs".as_slice().into(),
            comments: model.comments_on(b"a.rs").cloned().collect(),
            reading: model.file(b"a.rs").cloned(),
        },
    ];
    check_editor("review_cards", b"", review(told, (12, 50)), &[""]);
}
