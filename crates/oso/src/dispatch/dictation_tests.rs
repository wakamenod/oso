//! Dictation's settled spans as the dispatcher runs commands (ADR 0175, step 3): the person's
//! keys count as their rewrites, and an agent's command ends the span, learning nothing.
//!
//! Must not depend on: `app` or winit; an editor and the dispatcher.

use oso_core::buffer::Buffer;
use oso_core::command::Reach;
use oso_core::editor::{DictationPlaced, DictationSpan, Editor};
use oso_core::key::KeySequence;
use oso_core::selection::SelectionSet;
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::args::Given;

fn editor() -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes", "\n"));
    editor.set_view_size(ViewSize::new(10, 40));
    editor
}

/// A plain dictation of `text` at the start, settled, with point inside it.
fn settled(editor: &mut Editor, text: &str) -> DictationSpan {
    let target = editor.dictation_target().unwrap();
    let DictationPlaced::Typed { span: Some(span) } = editor.place_dictation(&target, text, false)
    else {
        panic!("no span");
    };
    editor.current_mut().0.selections = SelectionSet::new(2);
    span
}

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0);
}

#[test]
fn the_persons_keys_count_and_an_agents_insert_ends_the_span() {
    let mut editor = editor();
    let mut d = Dispatcher::new();
    let span = settled(&mut editor, "abcd");
    send(&mut d, &mut editor, "x C-d");
    assert_eq!(editor.dictation_span_edits(span), Some(2));
    assert_eq!(
        editor.dictation_span_text(span, 64).as_deref(),
        Some("abxd")
    );
    let ran = d.run_for_agent(&mut editor, "insert", Given::Text("!"), Reach::Change);
    assert!(ran.is_ok_and(|(_, result)| result.is_ok()));
    assert!(
        editor.dictation_span_edits(span).is_none(),
        "the agent's edit"
    );
}

#[test]
fn a_command_run_as_no_input_of_the_persons_ends_the_span() {
    let mut editor = editor();
    let mut d = Dispatcher::new();
    let span = settled(&mut editor, "abcd");
    d.not_as_input(|d| send(d, &mut editor, "y"));
    assert!(editor.dictation_span_edits(span).is_none());
}
