//! Tests of prompts on the editor (ADR 0072): their windows and buffers, what the window
//! beneath does meanwhile, where messages go and what a passphrase keeps.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::test_buffers::secret_buffer;
use super::{Editor, Quit};
use crate::buffer::{Buffer, Replacement};
use crate::complete::Candidate;
use crate::frame::CellRect;
use crate::frame::{FrameSize, PixelGrid};
use crate::minibuffer::{Kind, MAX_DEPTH, Outcome, PromptError};
use crate::secret::Verdict;
use crate::selection::{MarkActive, Selection, SelectionSet};

const NUMBER: Kind = Kind::Number {
    history: Some("line"),
};

fn editor(lines: usize) -> Editor {
    let text: String = (0..lines).map(|i| format!("line {i}\n")).collect();
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", &text));
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 100,
    });
    editor
}

fn type_in_prompt(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor
        .apply_edits(
            id,
            &[Replacement {
                range: end..end,
                text,
            }],
        )
        .unwrap();
}

#[test]
fn a_prompt_is_a_window_of_its_own_and_goes_with_its_buffer() {
    let mut editor = editor(3);
    let buffers = editor.buffers.len();
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    assert_eq!(editor.windows().len(), 2);
    assert_eq!(editor.selected_window(), 1);
    assert_eq!(editor.current().1.name(), " *Minibuf-1*");
    assert_eq!(editor.tiled().1.name(), "notes.txt");
    assert_eq!(editor.buffers().count(), 1, "nobody's work");
    assert_eq!(editor.quit(), Quit::Prompt);
    assert_eq!((editor.windows().len(), editor.selected_window()), (1, 0));
    assert_eq!(editor.buffers.len(), buffers, "its buffer is gone");
    assert_eq!(editor.quit(), Quit::Nothing);
}

#[test]
fn the_prompts_window_is_where_its_answer_goes() {
    let mut editor = editor(3);
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    let layout = editor.prompt_layout().unwrap();
    assert_eq!(editor.window().size().columns, layout.input.columns);
    assert_eq!(editor.window().size().rows, 1);
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 200,
    });
    let layout = editor.prompt_layout().unwrap();
    assert_eq!(editor.window().size().columns, layout.input.columns);
    assert_eq!(editor.tiled().0.size().columns, 200, "the window beneath");
    let grid = PixelGrid::new(10.0, 20.0);
    let area = editor.cursor_area(grid).unwrap();
    assert_eq!(
        area,
        grid.rect(crate::frame::CellRect {
            columns: 1,
            ..layout.input
        })
    );
}

#[test]
fn the_modeline_speaks_of_the_window_beneath() {
    let mut editor = editor(3);
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    let mut status = super::Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    assert!(status.name.contains("notes.txt"));
}

#[test]
fn the_wheel_scrolls_the_window_beneath_and_its_cursors_stay() {
    let mut editor = editor(100);
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    assert!(editor.scroll_view(20));
    let (window, buffer) = editor.tiled();
    assert_eq!(window.top_row(&buffer.display()), 20);
    assert_eq!(
        window.selections.primary().cursor,
        0,
        "what the prompt acts on stays"
    );
    assert_eq!(editor.top_row(), 0, "the prompt's own does not move");
}

#[test]
fn a_message_from_a_command_in_a_prompt_goes_there_and_others_to_the_echo_area() {
    let mut editor = editor(3);
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    editor.begin_command();
    editor.message("Beginning of buffer");
    editor.end_command();
    assert_eq!(
        editor.prompt().unwrap().message(),
        Some("Beginning of buffer")
    );
    assert_eq!(editor.echo_text(), "");
    editor.message("Saved");
    assert_eq!(editor.echo_text(), "Saved", "from outside a command");
    editor.clear_echo();
    assert_eq!(editor.prompt().unwrap().message(), None, "the next key");
}

#[test]
fn prompts_go_eight_deep() {
    let mut editor = editor(3);
    for _ in 0..MAX_DEPTH {
        editor.open_prompt("Go to line: ", NUMBER).unwrap();
    }
    assert_eq!(
        editor.open_prompt("Go to line: ", NUMBER),
        Err(PromptError::TooDeep)
    );
    assert_eq!(editor.prompt().unwrap().label(), "[8] Go to line: ");
}

#[test]
fn a_passphrase_may_hold_secrets_and_keeps_no_undo() {
    let mut editor = editor(3);
    editor
        .open_prompt("Passphrase: ", Kind::Passphrase)
        .unwrap();
    assert_eq!(editor.current().1.verdict(), Verdict::Passphrase);
    assert!(editor.shows_secret());
    type_in_prompt(&mut editor, "hunter2");
    assert!(editor.undo().is_err());
    editor.submit_prompt().unwrap();
    assert!(!editor.shows_secret());
    assert!(editor.histories.lists.is_empty());
}

#[test]
fn an_answer_given_around_a_secret_enters_no_history() {
    let buffer = secret_buffer();
    let mut editor = Editor::with_buffer(buffer);
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    type_in_prompt(&mut editor, "2");
    editor.submit_prompt().unwrap();
    assert_eq!(editor.histories.entries("line").count(), 0);
}

#[test]
fn an_answer_given_once_a_secret_came_beneath_enters_no_history() {
    let mut editor = editor(3);
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    let buffer = secret_buffer();
    let secret = editor.add_buffer(buffer);
    editor.show(secret);
    assert_eq!(editor.tiled().0.buffer(), secret, "shown beneath");
    type_in_prompt(&mut editor, "2");
    editor.submit_prompt().unwrap();
    assert_eq!(editor.histories.entries("line").count(), 0);
}

#[test]
fn a_region_in_the_answer_takes_its_cells_and_not_the_window_beneath() {
    let mut editor = editor(3);
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    type_in_prompt(&mut editor, "12345");
    let mut cells = Vec::new();
    editor.prompt_region_cells(&prompt_screen(&editor), &mut cells);
    assert!(cells.is_empty(), "no region yet");
    let (window, _) = editor.current_mut();
    let selection = Selection {
        mark: Some(1),
        ..Selection::at(4)
    };
    window.selections = SelectionSet::from_parts(vec![selection], 0).unwrap();
    window.selections.set_mark_active(MarkActive::Active);
    editor.prompt_region_cells(&prompt_screen(&editor), &mut cells);
    let input = editor.prompt_layout().unwrap().input;
    assert_eq!(
        cells,
        [CellRect {
            row: input.row,
            column: input.column + 1,
            rows: 1,
            columns: 3,
        }]
    );
    editor.region_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
    assert!(cells.is_empty(), "the window beneath has none");
}

#[test]
fn a_completion_a_socket_request_opened_records_no_history() {
    let items: std::sync::Arc<[Candidate]> = [Candidate::new("undo", "", "")].into();
    let mut editor = editor(3);
    let id = editor.open_completion("M-x", "command", items).unwrap();
    editor.keep_out_of_history(id);
    editor.submit_prompt().unwrap();
    assert_eq!(editor.histories.entries("command").count(), 0);
}

#[test]
fn beside_a_secret_an_empty_answer_takes_a_candidate_only_once_moved_to() {
    let items: std::sync::Arc<[Candidate]> = ["undo", "yank"]
        .map(|text| Candidate::new(text, "", ""))
        .into();
    let mut editor = editor(3);
    let secret = editor.add_buffer(secret_buffer());
    editor.show(secret);
    editor.open_completion("M-x", "command", items).unwrap();
    assert_eq!(editor.submit_prompt(), Err(PromptError::Invalid));
    assert_eq!(
        editor.prompt().unwrap().message(),
        Some("Type a name, or move to one")
    );
    editor.move_completion(1).unwrap();
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("no answer");
    };
    assert_eq!(text, "yank");
}

#[test]
fn completion_beside_a_secret_neither_reads_nor_records_its_history() {
    let items = || -> std::sync::Arc<[Candidate]> {
        ["undo", "forward-char", "backward-char"]
            .map(|text| Candidate::new(text, "", ""))
            .into()
    };
    let first = |editor: &Editor| {
        let candidates = editor.prompt().unwrap().candidates().unwrap();
        candidates.selected_candidate().unwrap().text().to_owned()
    };
    let mut editor = editor(3);
    editor.open_completion("M-x", "command", items()).unwrap();
    type_in_prompt(&mut editor, "forward");
    editor.match_completion();
    editor.submit_prompt().unwrap();
    editor.open_completion("M-x", "command", items()).unwrap();
    assert_eq!(first(&editor), "forward-char", "the history comes first");
    editor.quit();
    let path = std::path::PathBuf::from("/tmp/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(&[path]));
    let secret = editor.add_buffer(buffer);
    editor.show(secret);
    editor.open_completion("M-x", "command", items()).unwrap();
    assert_eq!(first(&editor), "undo", "no history beside a secret");
    type_in_prompt(&mut editor, "backward");
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor.histories.entries("command").collect::<Vec<_>>(),
        ["forward-char"]
    );
}

#[test]
fn an_answer_given_after_a_secret_came_beneath_and_went_enters_no_history() {
    let mut editor = editor(3);
    let plain = editor.window().buffer();
    editor.open_prompt("Go to line: ", NUMBER).unwrap();
    assert!(!editor.prompt().unwrap().around_secret());
    let buffer = secret_buffer();
    let secret = editor.add_buffer(buffer);
    editor.show(secret);
    type_in_prompt(&mut editor, "2");
    editor.show(plain);
    assert!(!editor.shows_secret());
    assert!(editor.prompt().unwrap().around_secret(), "it stays so");
    editor.submit_prompt().unwrap();
    assert_eq!(editor.histories.entries("line").count(), 0);
}

#[test]
fn a_command_that_gets_a_passphrase_says_nothing_to_the_log() {
    let mut editor = editor(3);
    editor.begin_command();
    editor.command_reads_passphrase();
    editor.message("Wrong passphrase");
    editor.end_command();
    assert_eq!(editor.echo_text(), "Wrong passphrase");
    assert!(editor.echo_withheld());
    let id = editor.messages_buffer();
    let kept = editor.buffer(id).unwrap().text().to_string();
    assert!(!kept.contains("Wrong passphrase"));
}

#[test]
fn a_carried_history_goes_behind_what_is_there_and_keeps_its_bounds() {
    let mut editor = editor(3);
    editor.histories.record("line", "7");
    let long = "9".repeat(crate::minibuffer::MAX_ENTRY_BYTES + 1);
    let carried = ["3", "7", "", &long, "2"].map(String::from);
    editor.carry_history("line", carried);
    let (name, entries) = editor.histories().next().unwrap();
    assert_eq!(name, "line");
    assert_eq!(entries.collect::<Vec<_>>(), ["7", "3", "2"]);
    let many = (0..crate::minibuffer::HISTORY_LENGTH + 5).map(|n| n.to_string());
    editor.carry_history("other", many);
    assert_eq!(
        editor.histories.entries("other").count(),
        crate::minibuffer::HISTORY_LENGTH
    );
}

/// The innermost prompt's answer as a frame shows it.
fn prompt_screen(editor: &Editor) -> crate::window::Screen {
    let mut screen = crate::window::Screen::default();
    editor.prompt_screen(&mut screen);
    screen
}
