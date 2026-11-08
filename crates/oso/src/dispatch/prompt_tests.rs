//! Scenario tests of prompts (ADR 0072): a command's argument read before it runs, a
//! question's answer, history, nesting, `C-g` and what a passphrase leaves behind.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::path::PathBuf;

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;
use oso_core::load::decode_all;
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::motion_tests::{cursors, send};
use crate::commands::kept_messages;

const LINES: &str = "one\ntwo\nthree\nfour\n";

fn on(text: &str) -> (Dispatcher, Editor) {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_view_size(ViewSize::new(10, 60));
    (Dispatcher::new(), editor)
}

fn on_a_secret(text: &str) -> (Dispatcher, Editor) {
    let path = PathBuf::from("/tmp/.env");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(text.as_bytes(), Default::default()));
    buffer.judge_secret(Some(&[path]));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(10, 60));
    (Dispatcher::new(), editor)
}

/// What the innermost prompt holds.
fn answer(editor: &Editor) -> String {
    let prompt = editor.prompt().expect("a prompt");
    editor.buffer(prompt.buffer()).unwrap().text().to_string()
}

#[test]
fn m_g_g_reads_a_line_and_goes_there() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-g g");
    assert_eq!(editor.prompt().unwrap().label(), "Go to line: ");
    send(&mut d, &mut editor, "3");
    assert_eq!(answer(&editor), "3");
    assert_eq!(cursors(&editor), [1], "typing went to the prompt");
    send(&mut d, &mut editor, "RET");
    assert!(editor.prompt().is_none());
    assert_eq!(cursors(&editor), ["one\ntwo\n".len()]);
}

#[test]
fn a_line_past_the_ends_goes_to_the_nearest() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-g M-g 9 9 RET");
    assert_eq!(cursors(&editor), [LINES.len()], "the empty last line");
    send(&mut d, &mut editor, "M-g g - 5 RET");
    assert_eq!(cursors(&editor), [0]);
}

#[test]
fn a_number_typed_as_a_prefix_stands_for_the_answer_and_c_u_alone_does_not() {
    for keys in ["C-u 3 M-g g", "M-3 M-g g"] {
        let (mut d, mut editor) = on(LINES);
        send(&mut d, &mut editor, keys);
        assert!(editor.prompt().is_none(), "{keys}");
        assert_eq!(cursors(&editor), ["one\ntwo\n".len()], "{keys}");
    }
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "C-u M-g g");
    assert!(editor.prompt().is_some());
}

#[test]
fn editing_keys_work_in_the_prompt() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-g g 1 2 DEL 4 C-a 2 C-e");
    assert_eq!(answer(&editor), "214");
    send(&mut d, &mut editor, "C-z");
    assert_eq!(answer(&editor), "14", "undo works there too");
}

#[test]
fn c_g_closes_the_prompt_and_runs_nothing() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "C-n M-g g 3 C-g");
    assert!(editor.prompt().is_none());
    assert_eq!(cursors(&editor), ["one\n".len()]);
    assert_eq!(editor.buffers().count(), 1, "its buffer went with it");
    send(&mut d, &mut editor, "C-n");
    assert_eq!(
        cursors(&editor),
        ["one\ntwo\n".len()],
        "keys reach the file again"
    );
}

#[test]
fn a_number_that_is_not_one_keeps_the_prompt_until_it_is() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-g g x RET");
    let prompt = editor.prompt().expect("still open");
    assert_eq!(prompt.message(), Some("Not a number"));
    send(&mut d, &mut editor, "DEL");
    assert_eq!(
        editor.prompt().unwrap().message(),
        None,
        "the next key clears it"
    );
    send(&mut d, &mut editor, "2 RET");
    assert_eq!(cursors(&editor), ["one\n".len()]);
}

#[test]
fn a_message_from_a_key_in_the_prompt_shows_there_and_not_on_the_echo_row() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-g g C-b");
    assert_eq!(
        editor.prompt().unwrap().message(),
        Some("Beginning of buffer")
    );
    let mut status = oso_core::editor::Status::default();
    editor.status_into(&mut status);
    assert_eq!(status.echo, "");
    assert!(kept_messages(&editor).contains(&"Beginning of buffer".to_owned()));
}

#[test]
fn history_brings_back_earlier_answers() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-g g 3 RET M-g g 2 RET M-g g");
    send(&mut d, &mut editor, "M-p");
    assert_eq!(answer(&editor), "2");
    send(&mut d, &mut editor, "M-p");
    assert_eq!(answer(&editor), "3");
    send(&mut d, &mut editor, "M-n M-n");
    assert_eq!(answer(&editor), "", "back to what was typed");
    send(&mut d, &mut editor, "M-p RET");
    assert_eq!(cursors(&editor), ["one\n".len()]);
}

#[test]
fn a_prompt_opened_around_a_secret_keeps_no_history() {
    let (mut d, mut editor) = on_a_secret("KEY=1\nB=2\n");
    send(&mut d, &mut editor, "M-g g 2 RET M-g g M-p");
    assert_eq!(answer(&editor), "");
    assert_eq!(editor.prompt().unwrap().message(), Some("No history here"));
}

#[test]
fn a_question_from_a_prompt_nests_and_c_g_closes_one_at_a_time() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "x M-g g 3 C-x C-c");
    assert_eq!(editor.prompts().len(), 2);
    let label = editor.prompt().unwrap().label().to_owned();
    assert!(label.starts_with("[2] 1 modified buffer"), "{label}");
    send(&mut d, &mut editor, "C-g");
    assert_eq!(editor.prompts().len(), 1);
    assert_eq!(answer(&editor), "3", "the outer prompt is as it was");
    send(&mut d, &mut editor, "RET");
    assert_eq!(
        cursors(&editor),
        ["xone\ntwo\n".len()],
        "its command still runs"
    );
}

#[test]
fn y_from_a_nested_question_exits() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "x M-g g C-x C-c");
    assert!(send(&mut d, &mut editor, "y").exit);
}

#[test]
fn prompts_nest_eight_deep_and_no_further() {
    let (mut d, mut editor) = on(LINES);
    for _ in 0..9 {
        send(&mut d, &mut editor, "M-g g");
    }
    assert_eq!(editor.prompts().len(), 8);
    assert_eq!(
        editor.prompt().unwrap().message(),
        Some("Too many prompts open")
    );
    for _ in 0..8 {
        send(&mut d, &mut editor, "C-g");
    }
    assert!(editor.prompt().is_none());
    send(&mut d, &mut editor, "M-g g 2 RET");
    assert_eq!(
        cursors(&editor),
        ["one\n".len()],
        "nothing was left waiting"
    );
}

#[test]
fn a_prompt_cannot_change_which_buffer_its_window_shows() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-g g C-h e");
    assert_eq!(
        editor.prompt().unwrap().message(),
        Some("Not in the minibuffer")
    );
    send(&mut d, &mut editor, "C-g");
    assert_eq!(editor.current().1.name(), "notes.txt");
}

#[test]
fn run_command_gives_the_answer_and_never_waits_for_a_person() {
    let (mut d, mut editor) = on(LINES);
    let (_, result) = d.run_command(&mut editor, "goto-line", Some(3));
    assert_eq!(result, Ok(()));
    assert_eq!(cursors(&editor), ["one\ntwo\n".len()]);
    let (_, result) = d.run_command(&mut editor, "goto-line", None);
    assert_eq!(result, Err("goto-line needs a number".to_owned()));
    assert!(editor.prompt().is_none());
}

#[test]
fn the_close_button_asks_the_exit_question_once() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "x");
    assert!(!d.close_requested(&mut editor).exit);
    assert!(
        editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
    );
    assert!(!d.close_requested(&mut editor).exit, "pressed again");
    assert_eq!(editor.prompts().len(), 1);
    assert!(send(&mut d, &mut editor, "y").exit);
}

#[test]
fn the_close_button_exits_at_once_with_nothing_at_risk_and_asks_from_a_prompt() {
    let (mut d, mut editor) = on(LINES);
    assert!(d.close_requested(&mut editor).exit);
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "x M-g g");
    assert!(!d.close_requested(&mut editor).exit);
    assert_eq!(editor.prompts().len(), 2, "a question over the prompt");
}

#[test]
fn the_close_button_drops_a_prefix_in_progress() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "x C-u C-x");
    assert!(!d.close_requested(&mut editor).exit);
    assert!(
        d.pending().is_empty(),
        "no prefix key waits under the question"
    );
    assert!(
        send(&mut d, &mut editor, "y").exit,
        "y answers, and is no count's digit"
    );
}

#[test]
fn text_in_a_question_says_how_to_answer() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "x C-x C-c");
    d.insert_text(&mut editor, "yes");
    let prompt = editor.prompt().expect("still asking");
    assert_eq!(prompt.message(), Some("Answer y or n"));
}

/// Asks a question and answers it at once, so that each answer runs it again.
fn ask_and_answer(
    ctx: &mut oso_core::command::Ctx<'_>,
    _: oso_core::command::Args,
) -> Result<(), oso_core::command::CommandError> {
    let error =
        |e: oso_core::minibuffer::PromptError| oso_core::command::CommandError::new(e.to_string());
    ctx.editor.ask("Again?", "ask-and-answer").map_err(error)?;
    ctx.editor.answer_question(true).map_err(error)?;
    Ok(())
}

#[test]
fn what_is_left_past_the_bound_on_follow_ups_does_not_run_later() {
    use oso_core::command::{ArgSpec, Command, Registry};
    let mut registry = Registry::new();
    registry
        .register(Command {
            name: "ask-and-answer",
            doc: "Ask and answer at once.",
            args: ArgSpec::None,
            run: ask_and_answer,
        })
        .unwrap();
    let mut d = Dispatcher::with(registry, oso_core::keymap::Keymap::default());
    let mut editor = Editor::default();
    let (_, result) = d.run_command(&mut editor, "ask-and-answer", None);
    assert_eq!(result, Ok(()));
    assert!(editor.take_prompt_outcome().is_none(), "dropped");
    assert!(editor.prompt().is_none());
}

#[cfg(feature = "dev")]
mod passphrase {
    use super::*;

    #[test]
    fn a_kill_in_a_passphrase_prompt_reaches_neither_the_ring_nor_the_clipboard() {
        let (mut d, mut editor) = on(LINES);
        send(&mut d, &mut editor, "<f34> h u n t e r C-a C-k");
        assert_eq!(answer(&editor), "", "it deletes");
        assert!(editor.kill_ring().is_empty());
        assert!(editor.take_clipboard_write().is_none());
    }

    #[test]
    fn a_secret_yanked_into_a_passphrase_prompt_and_killed_goes_nowhere() {
        let (mut d, mut editor) = on(LINES);
        let fingerprint = oso_core::kill::Fingerprint { len: 3, hash: 1 };
        editor.take_from_clipboard(oso_core::rope::Rope::from("KEY"), true, fingerprint);
        send(&mut d, &mut editor, "<f34> h u n t e r C-y");
        assert!(d.take_clipboard_request().is_some());
        d.clipboard_answered(&mut editor, &oso_core::kill::FromClipboard::Own);
        assert_eq!(answer(&editor), "hunterKEY");
        assert_eq!(
            editor.current().1.verdict(),
            oso_core::buffer::Verdict::Passphrase
        );
        send(&mut d, &mut editor, "C-a C-k");
        assert_eq!(answer(&editor), "");
        assert_eq!(editor.kill_ring().len(), 1, "only the entry it yanked");
        assert!(editor.take_clipboard_write().is_none());
    }

    #[test]
    fn a_passphrase_counts_as_secret_keeps_no_undo_and_leaves_no_trace() {
        let (mut d, mut editor) = on(LINES);
        send(&mut d, &mut editor, "<f34> h u n t e r");
        assert!(
            editor.shows_secret(),
            "every secret rule applies while it shows"
        );
        assert_eq!(answer(&editor), "hunter");
        send(&mut d, &mut editor, "C-z");
        assert_eq!(answer(&editor), "hunter", "no undo record holds it");
        let state = crate::dev::get_state(&editor, &[], None);
        let prompt = &state["prompts"][0];
        assert_eq!(prompt["kind"], "passphrase");
        assert!(prompt["text"].is_null() && prompt["prompt"].is_null());
        assert!(state.get("echo").is_none() && state.get("modeline").is_none());
        send(&mut d, &mut editor, "RET");
        assert!(editor.prompt().is_none());
        assert!(!editor.shows_secret());
        let messages = kept_messages(&editor);
        assert!(messages.iter().all(|message| !message.contains("hunter")));
        assert!(
            messages
                .iter()
                .all(|message| message != "Read a passphrase"),
            "what its command says stays out of *Messages*"
        );
        let mut status = oso_core::editor::Status::default();
        editor.status_into(&mut status);
        assert_eq!(status.echo, "Read a passphrase", "but shows");
        let state = crate::dev::get_state(&editor, &[], None);
        assert!(state.get("echo").is_none(), "and not over the socket");
        send(&mut d, &mut editor, "<f34> M-p");
        assert_eq!(answer(&editor), "", "it entered no history");
    }

    #[test]
    fn a_prompt_opened_over_a_passphrase_keeps_run_command_refused() {
        let (mut d, mut editor) = on(LINES);
        send(&mut d, &mut editor, "<f34> a b M-g g");
        assert_eq!(editor.prompts().len(), 2);
        let (_, result) = d.run_command(&mut editor, "backward-char", None);
        assert!(result.unwrap_err().contains("passphrase prompt is open"));
    }

    #[test]
    fn what_a_passphrase_command_says_in_a_prompt_is_withheld_from_the_socket() {
        let (mut d, mut editor) = on(LINES);
        send(&mut d, &mut editor, "M-g g");
        editor.begin_command();
        editor.command_reads_passphrase();
        editor.message("Wrong passphrase");
        editor.end_command();
        let prompt = editor.prompt().unwrap();
        assert_eq!(prompt.message(), Some("Wrong passphrase"));
        assert!(prompt.message_withheld());
        let state = crate::dev::get_state(&editor, &[], None);
        assert!(state["prompts"][0]["message"].is_null());
        send(&mut d, &mut editor, "C-g");
    }

    #[test]
    fn run_command_refuses_a_passphrase_and_runs_only_c_g_while_one_is_open() {
        let (mut d, mut editor) = on(LINES);
        let (_, result) = d.run_command(&mut editor, "dev-read-passphrase", None);
        assert!(result.unwrap_err().contains("through send-keys"));
        send(&mut d, &mut editor, "<f34> a b");
        let (_, result) = d.run_command(&mut editor, "backward-char", None);
        assert!(result.unwrap_err().contains("passphrase prompt is open"));
        let (_, result) = d.run_command(&mut editor, "keyboard-quit", None);
        assert_eq!(result, Ok(()));
        assert!(editor.prompt().is_none());
    }
}
