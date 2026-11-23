//! The tests of settled spans: what counts as the person's change, what ends one, and how
//! one follows an undo (ADR 0175, step 3).
//!
//! Must not depend on: the host's threads or processes; these drive the editor alone.

use crate::buffer::{Buffer, Replacement};
use crate::editor::DictationSpan;
use crate::editor::Editor;
use crate::editor::dictation::MAX_SETTLED;
use crate::editor::dictation::tests::{editor, persons, span_of, text, type_at, undo};
use crate::window::ViewSize;

fn settled(editor: &mut Editor, text: &str) -> DictationSpan {
    let target = editor.dictation_target().unwrap();
    span_of(editor.place_dictation(&target, text, false))
}

#[test]
fn a_settled_span_counts_the_persons_commands_that_change_it_only() {
    let mut editor = editor("");
    let span = settled(&mut editor, "abcd");
    persons(&mut editor, |editor| type_at(editor, 0, "<"));
    persons(&mut editor, |editor| type_at(editor, 5, ">"));
    assert_eq!(
        editor.dictation_span_edits(span),
        Some(0),
        "typed at its ends"
    );
    persons(&mut editor, |editor| type_at(editor, 3, "-"));
    let id = editor.window().buffer();
    persons(&mut editor, |editor| {
        let edit = Replacement {
            range: 1..2,
            text: "A",
        };
        editor.apply_edits(id, &[edit]).unwrap();
    });
    assert_eq!(text(&editor), "<Ab-cd>");
    assert_eq!(editor.dictation_span_edits(span), Some(2));
    assert_eq!(editor.dictation_span_text(span, 64).unwrap(), "Ab-cd");
    assert!(
        editor.dictation_span_text(span, 4).is_none(),
        "past its bound"
    );
}

#[test]
fn an_agents_command_or_one_never_seen_to_be_the_persons_ends_it() {
    let mut editor = editor("");
    let span = settled(&mut editor, "abcd");
    editor.begin_command();
    editor.for_agent(|editor| type_at(editor, 2, "!"));
    editor.end_command();
    editor.note_persons_command();
    assert!(editor.dictation_span_edits(span).is_none());
    // A command the host ran, which is no input of the person's.
    let span = settled(&mut editor, "efgh");
    editor.begin_command();
    type_at(&mut editor, 7, "?");
    editor.end_command();
    editor.begin_command();
    editor.end_command();
    assert!(editor.dictation_span_edits(span).is_none());
}

#[test]
fn a_secret_ends_a_settled_span_and_a_save_does_not() {
    let mut editor = editor("");
    let id = editor.window().buffer();
    let span = settled(&mut editor, "efgh");
    let content = editor.buffer(id).unwrap().content_id();
    editor.buffer_mut(id).unwrap().mark_saved(content);
    assert_eq!(editor.dictation_span_edits(span), Some(0), "saved");
    editor.buffer_mut(id).unwrap().judge_copy(
        &[std::path::PathBuf::from("/tmp/.env")],
        crate::secret::Verdict::SecretName,
    );
    assert!(editor.dictation_span_text(span, 64).is_none());
    assert!(editor.dictation_span_edits(span).is_none());
    assert!(editor.dictation_span_at(span).is_none(), "it ended");
}

#[test]
fn settled_spans_are_never_drawn_and_the_oldest_goes_past_their_bound() {
    let mut editor = editor("");
    let spans: Vec<DictationSpan> = (0..=MAX_SETTLED)
        .map(|_| settled(&mut editor, "a"))
        .collect();
    assert!(editor.dictation_span_at(spans[0]).is_none());
    assert!(
        spans[1..]
            .iter()
            .all(|&span| editor.dictation_span_at(span).is_some())
    );
    let drawn = span_of(editor.place_dictation(&editor.dictation_target().unwrap(), "b", true));
    assert!(
        editor.dictation_span_at(spans[1]).is_some(),
        "a drawn span is not counted"
    );
    editor.settle_dictation_span(drawn);
    assert!(editor.dictation_span_at(spans[1]).is_none());
    assert!(!editor.dictation_span_at(drawn).unwrap().drawn);
}

#[test]
fn a_span_follows_an_undo_of_its_fix_with_or_without_an_edit_log() {
    // A Markdown buffer keeps a log of its edits, and a plain one does not.
    for name in ["notes.md", "notes"] {
        let mut editor = Editor::with_buffer(Buffer::from_text(name, "x"));
        editor.set_view_size(ViewSize::new(10, 40));
        let target = editor.dictation_target().unwrap();
        let span = span_of(editor.place_dictation(&target, "ビート", true));
        let edits = [(0.."ビート".len(), "Vite")];
        assert_eq!(
            editor.fix_dictation(span, "ビート", &edits),
            Ok(()),
            "{name}"
        );
        persons(&mut editor, undo);
        assert_eq!(text(&editor), "ビートx", "{name}");
        assert_eq!(
            editor.dictation_span_text(span, 64).as_deref(),
            Some("ビート"),
            "{name}"
        );
        assert_eq!(editor.dictation_span_edits(span), Some(1), "{name}");
    }
}

#[test]
fn a_change_by_a_command_never_seen_to_be_the_persons_ends_it_at_the_next_look() {
    let mut editor = editor("");
    let span = settled(&mut editor, "abcd");
    // A command the host runs as no input of the person's, and the watch looks before the
    // next command begins.
    editor.begin_command();
    type_at(&mut editor, 2, "!");
    editor.end_command();
    assert!(editor.dictation_span_edits(span).is_none());
}
