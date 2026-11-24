//! The keys of a native session's composer and queue (ADR 0165, step 2): in the composer,
//! typing writes and `C-c` keys ask the host; in the transcript, `i` goes to the composer and
//! `e`, `d` and `M-<up>` act on the queued message at point.
//!
//! Must not depend on: `app` or winit; these drive the default dispatcher.

use oso_core::buffer::{Buffer, Mode};
use oso_core::editor::{Editor, NativeAction, SessionRequest};

use super::Dispatcher;
use super::motion_tests::send;

fn actions(editor: &mut Editor) -> Vec<NativeAction> {
    (editor.take_session_requests().into_iter())
        .filter_map(|request| match request {
            SessionRequest::Native { action, .. } => Some(action),
            _ => None,
        })
        .collect()
}

#[test]
fn the_composer_takes_typing_and_its_own_keys() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let composer = editor.add_buffer(Buffer::from_text("*compose: s*", ""));
    assert!(
        editor
            .buffer_mut(composer)
            .unwrap()
            .set_mode(Mode::Composer)
    );
    editor.show(composer);
    send(
        &mut d,
        &mut editor,
        "h i C-c C-c C-c C-k M-p M-n C-c C-o C-c C-q C-c C-s",
    );
    assert_eq!(editor.current().1.text().to_string(), "hi");
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Send,
            NativeAction::Cancel,
            NativeAction::History(1),
            NativeAction::History(-1),
            NativeAction::Transcript,
            NativeAction::Queue,
            NativeAction::Suggestion,
        ]
    );
}

#[test]
fn the_transcripts_letters_go_to_the_composer_and_act_on_a_queued_message() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let transcript = editor.add_native_session_buffer("*claude: s*");
    editor.show(transcript);
    send(
        &mut d,
        &mut editor,
        "i e d M-<up> M-<down> C-c C-q C-c C-k C-c C-s",
    );
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Compose,
            NativeAction::EditQueued,
            NativeAction::DropQueued,
            NativeAction::MoveQueued(-1),
            NativeAction::MoveQueued(1),
            NativeAction::Queue,
            // The interrupt, over session mode's `C-c C-k`, which it leaves to a terminal.
            NativeAction::Cancel,
            // So is `C-c C-s`.
            NativeAction::Suggestion,
        ]
    );
}

#[test]
fn a_cards_keys_answer_its_call_and_a_digit_chooses() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let card = editor.add_internal_buffer("*card: s*", Mode::Card);
    editor.show(card);
    // It takes no suggestion.
    send(&mut d, &mut editor, "y n N o 3 RET C-c C-s");
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Allow,
            NativeAction::Deny,
            NativeAction::Note,
            NativeAction::Other,
            NativeAction::Choose(3),
            NativeAction::Open,
        ]
    );
    // Its text is oso's: a key types nothing into it.
    assert_eq!(editor.current().1.text().to_string(), "");
}

#[test]
fn a_requests_line_answers_its_call_pages_the_transcript_and_types_nothing() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let line = editor.add_internal_buffer("*request: s*", Mode::Request);
    editor.show(line);
    send(
        &mut d,
        &mut editor,
        "y n N C-c C-k RET SPC C-v DEL M-v M-< M-> i C-c C-o C-g C-c C-s",
    );
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Allow,
            NativeAction::Deny,
            NativeAction::Note,
            NativeAction::Deny,
            NativeAction::Open,
            NativeAction::ReadOn(1),
            NativeAction::ReadOn(1),
            NativeAction::ReadOn(-1),
            NativeAction::ReadOn(-1),
            NativeAction::ReadOn(isize::MIN),
            NativeAction::ReadOn(isize::MAX),
            NativeAction::Compose,
            NativeAction::Transcript,
            NativeAction::Transcript,
        ]
    );
    // Any argument asks to allow it unread, and pages by its count.
    send(&mut d, &mut editor, "C-u y M-1 y C-u 3 SPC");
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Unread,
            NativeAction::Unread,
            NativeAction::ReadOn(3)
        ]
    );
    // Nothing is typed, an option's number or a letter.
    send(&mut d, &mut editor, "a A o e 3 j");
    assert_eq!(actions(&mut editor), []);
    assert_eq!(text(&editor), "");
}

#[test]
fn c_c_c_j_goes_to_the_request_from_the_transcript_and_the_composer() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let transcript = editor.add_native_session_buffer("*claude: s*");
    editor.show(transcript);
    send(&mut d, &mut editor, "C-c C-j");
    composer(&mut editor, "a draft");
    send(&mut d, &mut editor, "C-c C-j y");
    assert_eq!(
        actions(&mut editor),
        [NativeAction::Request, NativeAction::Request]
    );
    // A letter there is text.
    assert_eq!(text(&editor), "ya draft");
}

/// A composer, selected, holding `text`.
fn composer(editor: &mut Editor, text: &str) {
    let composer = editor.add_buffer(Buffer::from_text("*compose: s*", text));
    assert!(
        editor
            .buffer_mut(composer)
            .unwrap()
            .set_mode(Mode::Composer)
    );
    editor.show(composer);
}

/// Types `key` as one the host said answers the card.
fn answers(d: &mut Dispatcher, editor: &mut Editor, key: &str) {
    d.set_key_answers_card(true);
    send(d, editor, key);
    d.take_key_answered_card();
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

#[test]
fn a_key_the_host_said_answers_the_card_runs_the_cards_command_in_an_empty_composer() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    for key in ["y", "n", "N", "o", "3"] {
        answers(&mut d, &mut editor, key);
    }
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Allow,
            NativeAction::Deny,
            NativeAction::Note,
            NativeAction::Other,
            NativeAction::Choose(3),
        ]
    );
    assert_eq!(text(&editor), "", "none of them is typed");
    // With no word of the host's, each is typed, as ever; and its word holds for one key.
    send(&mut d, &mut editor, "y");
    assert_eq!(text(&editor), "y");
    assert_eq!(actions(&mut editor), []);
}

#[test]
fn what_the_host_said_answers_nothing_where_the_key_would_not_be_typed_first() {
    // With text in the composer.
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "dra");
    answers(&mut d, &mut editor, "y");
    assert_eq!(text(&editor), "ydra");
    // After a prefix argument, in a prompt, and in a buffer that is no composer.
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    send(&mut d, &mut editor, "M-2");
    answers(&mut d, &mut editor, "y");
    assert_eq!(text(&editor), "yy");
    send(&mut d, &mut editor, "DEL DEL M-x");
    answers(&mut d, &mut editor, "y");
    assert!(editor.prompt().is_some());
    send(&mut d, &mut editor, "C-g");
    let other = editor.add_buffer(Buffer::from_text("notes", ""));
    editor.show(other);
    answers(&mut d, &mut editor, "n");
    assert_eq!(text(&editor), "n");
    // A key of a sequence, and one the host's word came too late for.
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    send(&mut d, &mut editor, "C-c");
    answers(&mut d, &mut editor, "y");
    send(&mut d, &mut editor, "a");
    assert_eq!(text(&editor), "a");
    assert_eq!(actions(&mut editor), [], "nothing was answered");
}

#[test]
fn a_key_that_ran_the_cards_command_says_so_and_a_typed_one_does_not() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    d.set_key_answers_card(true);
    send(&mut d, &mut editor, "y");
    assert!(d.take_key_answered_card());
    assert!(!d.take_key_answered_card(), "taken");
    composer(&mut editor, "draft");
    d.set_key_answers_card(true);
    send(&mut d, &mut editor, "y");
    assert!(!d.take_key_answered_card(), "typed: its repeats type too");
}

#[test]
fn a_replayed_macro_types_in_the_composer_what_answered_as_it_was_recorded() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    send(&mut d, &mut editor, "C-x (");
    answers(&mut d, &mut editor, "n");
    send(&mut d, &mut editor, "C-x )");
    assert_eq!(actions(&mut editor), [NativeAction::Deny]);
    answers(&mut d, &mut editor, "C-x e");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    assert_eq!(actions(&mut editor), [], "a macro answers nothing");
    assert_eq!(text(&editor), "n");
}

#[test]
fn a_cards_page_keys_turn_it_by_their_count() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let card = editor.add_internal_buffer("*card: s*", Mode::Card);
    editor.show(card);
    send(&mut d, &mut editor, "SPC DEL M-3 SPC");
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Page(1),
            NativeAction::Page(-1),
            NativeAction::Page(3),
        ]
    );
}

fn list_commands(editor: &mut Editor) {
    use oso_core::complete::Candidate;
    let items = [
        Candidate::new("model", "<model>", "Set the model"),
        Candidate::new("context", "", "Show the context"),
    ];
    editor.set_listed(crate::commands::agent_sessions::SLASH_LIST, items.into());
}

#[test]
fn a_slash_as_an_empty_composers_first_character_completes_over_its_commands() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    list_commands(&mut editor);
    send(&mut d, &mut editor, "/");
    assert!(editor.prompt().is_some(), "the commands' prompt opens");
    send(&mut d, &mut editor, "m o d RET");
    assert_eq!(editor.current().1.text().to_string(), "/model ");
    assert!(editor.prompt().is_none());
    // Its arguments follow.
    send(&mut d, &mut editor, "h");
    assert_eq!(editor.current().1.text().to_string(), "/model h");
}

#[test]
fn c_g_leaves_the_slash_and_a_slash_elsewhere_is_only_a_slash() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    list_commands(&mut editor);
    send(&mut d, &mut editor, "/ C-g");
    assert!(editor.prompt().is_none());
    assert_eq!(editor.current().1.text().to_string(), "/");
    send(&mut d, &mut editor, "t m p /");
    assert!(
        editor.prompt().is_none(),
        "not the composer's first character"
    );
    assert_eq!(editor.current().1.text().to_string(), "/tmp/");

    // With no command listed, `/` is only a `/`.
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    send(&mut d, &mut editor, "/");
    assert!(editor.prompt().is_none());
}

#[test]
fn c_c_slash_replaces_the_command_the_composer_begins_with() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "/mo haiku");
    list_commands(&mut editor);
    send(&mut d, &mut editor, "C-c / c o n t RET");
    assert_eq!(editor.current().1.text().to_string(), "/context haiku");

    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "why");
    list_commands(&mut editor);
    send(&mut d, &mut editor, "C-c / m o d e l RET");
    assert_eq!(editor.current().1.text().to_string(), "/model why");

    // Outside a composer it says so.
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    list_commands(&mut editor);
    send(&mut d, &mut editor, "/");
    assert!(editor.prompt().is_none(), "a buffer's `/` is a `/`");
}

#[test]
fn c_c_slash_puts_a_command_before_a_path_and_point_stays() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "/tmp/x look");
    list_commands(&mut editor);
    send(&mut d, &mut editor, "M-> C-c / m o d e l RET");
    assert_eq!(editor.current().1.text().to_string(), "/model /tmp/x look");
    send(&mut d, &mut editor, "!");
    assert_eq!(
        editor.current().1.text().to_string(),
        "/model /tmp/x look!",
        "point stays at the end of what was written"
    );
}

#[test]
fn a_macros_slash_opens_the_prompt_as_the_persons_does() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    list_commands(&mut editor);
    send(&mut d, &mut editor, "C-x ( / m o d RET C-x )");
    assert_eq!(editor.current().1.text().to_string(), "/model ");
    send(&mut d, &mut editor, "C-a C-k");
    assert_eq!(editor.current().1.text().to_string(), "");
    send(&mut d, &mut editor, "C-x e");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    assert_eq!(editor.current().1.text().to_string(), "/model ");
    assert!(editor.prompt().is_none());
}

fn list_arguments(editor: &mut Editor) {
    use crate::commands::agent_sessions::{EFFORT_LIST, MODEL_LIST};
    use oso_core::complete::Candidate;
    let models = [
        Candidate::new("sonnet", "", "Sonnet 5.5  Most efficient"),
        Candidate::new("haiku", "", "Haiku 5.5  Fastest"),
    ];
    editor.set_listed(MODEL_LIST, models.into());
    let efforts = [
        Candidate::new("low", "", ""),
        Candidate::new("high", "", ""),
    ];
    editor.set_listed(EFFORT_LIST, efforts.into());
}

#[test]
fn model_chosen_in_the_commands_prompt_completes_its_argument() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "");
    list_commands(&mut editor);
    list_arguments(&mut editor);
    send(&mut d, &mut editor, "/ m o d e l RET");
    assert!(editor.prompt().is_some(), "the models' prompt opens");
    send(&mut d, &mut editor, "h a i RET");
    assert_eq!(editor.current().1.text().to_string(), "/model haiku");
    assert!(editor.prompt().is_none());
    // Nothing is sent: the person sends it.
    assert!(actions(&mut editor).is_empty());
    // `C-c /` on it completes its argument again, in place of the one typed.
    send(&mut d, &mut editor, "C-c / s o n RET");
    assert_eq!(editor.current().1.text().to_string(), "/model sonnet");
    // `C-g` leaves the command for its argument by hand.
    send(&mut d, &mut editor, "C-a C-k / m o d e l RET C-g");
    assert_eq!(editor.current().1.text().to_string(), "/model ");
}

#[test]
fn c_c_slash_on_effort_completes_its_level_and_elsewhere_the_commands() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "/effort lo and so");
    list_commands(&mut editor);
    list_arguments(&mut editor);
    send(&mut d, &mut editor, "C-c / h i RET");
    assert_eq!(editor.current().1.text().to_string(), "/effort high and so");
    // A word that only begins so is no `/model`: the commands' prompt opens.
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "/models x");
    list_commands(&mut editor);
    list_arguments(&mut editor);
    send(&mut d, &mut editor, "C-c / c o n t RET");
    assert_eq!(editor.current().1.text().to_string(), "/context /models x");
}

#[test]
fn an_argument_chosen_after_the_command_takes_no_word_the_composer_held() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    composer(&mut editor, "fix this bug");
    list_commands(&mut editor);
    list_arguments(&mut editor);
    send(&mut d, &mut editor, "C-c / m o d e l RET");
    assert!(editor.prompt().is_some(), "the models' prompt opens");
    send(&mut d, &mut editor, "h a i RET");
    assert_eq!(
        editor.current().1.text().to_string(),
        "/model haiku fix this bug"
    );
}
