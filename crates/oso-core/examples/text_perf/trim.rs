//! Whitespace cleanup on save (ADR 0113): a save after typing at many cursors, after pasting a
//! large text, and after a keyboard macro's replay edited every line, one command an edit, on
//! the UI thread and on the scan's thread. Run with `-- trim`.
//!
//! Must not depend on: other workspace crates or crate internals; it uses only the public
//! API.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, Replacement};
use oso_core::display_map::TabWidth;
use oso_core::editor::Editor;
use oso_core::load::decode_all;
use oso_core::selection::{Selection, SelectionSet};

use super::{MB, file_text, floor_boundary};

/// An editor visiting `text` as a file, with cleanup on save on.
fn visiting(text: &str) -> Editor {
    let mut buffer = Buffer::visiting(PathBuf::from("/nowhere/notes.txt"));
    buffer.apply_load(decode_all(text.as_bytes(), TabWidth::default()));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_trim_whitespace(true);
    editor
}

/// Saves as the command does, and says what the UI thread and the scan's thread took.
fn save_costs(label: &str, editor: &mut Editor) {
    let started = Instant::now();
    editor.begin_undo_group();
    let saved = editor.start_save();
    editor.end_undo_group();
    let on_key = started.elapsed();
    let (mut scan, mut applied) = (None, None);
    if let Some(job) = editor.take_trim_jobs().pop() {
        let started = Instant::now();
        let report = job.run();
        scan = Some(started.elapsed());
        let started = Instant::now();
        let _ = editor.trim_reported(report);
        applied = Some(started.elapsed());
    }
    let jobs = editor.take_save_jobs().len();
    println!(
        "{label:<34} key {on_key:>9.2?}  scan {scan:>12.2?}  applied {applied:>12.2?}  saves {jobs} ({saved:?})"
    );
}

pub(crate) fn trim_costs() {
    println!("== Whitespace cleanup on save (ADR 0113)");
    let text = file_text(100 * MB);
    // Typing a space at 1000 cursors, five times, then a save.
    let mut editor = visiting(&text);
    let stride = text.len() / 1000;
    let parts = (0..1000)
        .map(|index| Selection::at(floor_boundary(&text, index * stride)))
        .collect();
    editor.current_mut().0.selections =
        SelectionSet::from_parts(parts, 0).expect("there is a cursor");
    let id = editor.window().buffer();
    for _ in 0..5 {
        let edits: Vec<_> = editor
            .window()
            .selections
            .iter()
            .map(|selection| Replacement {
                range: selection.cursor..selection.cursor,
                text: " ",
            })
            .collect();
        editor.begin_undo_group();
        editor
            .apply_edits(id, &edits)
            .expect("the buffer takes edits");
        editor.end_undo_group();
    }
    save_costs("100 MB, 1000 cursors typed", &mut editor);

    // A paste of 100 MB whose every line ends in a space, then a save.
    for (label, pasted) in [
        ("paste of 1 MB, a space a line", "line \n".repeat(MB / 6)),
        (
            "paste of 100 MB, a space a line",
            "line \n".repeat(100 * MB / 6),
        ),
    ] {
        let mut editor = visiting("");
        let id = editor.window().buffer();
        editor.begin_undo_group();
        editor
            .apply_edits(
                id,
                &[Replacement {
                    range: 0..0,
                    text: &pasted,
                }],
            )
            .expect("the buffer takes edits");
        editor.end_undo_group();
        save_costs(label, &mut editor);
    }
    replay_costs(100_000);
}

/// A forward replay's edits, one command each, as a macro run over `lines` lines makes them:
/// a space at each line's start, so each is an edited range of its own. The first and last
/// tenth's edits are timed apart, so a cost that grows with the ranges kept shows, then a
/// save.
fn replay_costs(lines: usize) {
    const LINE: usize = 40;
    let mut editor = visiting(&format!("{}\n", "x".repeat(LINE - 1)).repeat(lines));
    let id = editor.window().buffer();
    let tenth = (lines / 10).max(1);
    let (mut first, mut last) = (Duration::ZERO, Duration::ZERO);
    let started = Instant::now();
    let mut chunk = Instant::now();
    for line in 0..lines {
        // Each line before is one longer for the space at its start.
        let at = line * (LINE + 1);
        editor.begin_undo_group();
        editor
            .apply_edits(
                id,
                &[Replacement {
                    range: at..at,
                    text: " ",
                }],
            )
            .expect("the buffer takes edits");
        editor.end_undo_group();
        if (line + 1) % tenth == 0 {
            last = chunk.elapsed() / u32::try_from(tenth).unwrap_or(u32::MAX);
            if line + 1 == tenth {
                first = last;
            }
            chunk = Instant::now();
        }
    }
    let total = started.elapsed();
    println!(
        "replay of {lines} far edits    {total:>10.2?}, an edit: first tenth {first:>8.2?} last tenth {last:>8.2?}"
    );
    save_costs("after the replay", &mut editor);
}
