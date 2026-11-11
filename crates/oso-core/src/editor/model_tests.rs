//! A reference model of editing through [`Editor`]: random multi-cursor edits,
//! undo and redo checked against a `String` after every step (ADR 0053).
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written
//! out here.
//!
//! The model keeps the text as a `String` and works edits and cursors out by
//! naive scans. Only display clusters come from a display map, built anew.

use proptest::prelude::*;

use super::edit_tests::{cursors, edit, set_at, text};
use super::*;
use crate::display_map::{DisplayMap, DisplayPoint, DisplaySnapshot, TabWidth};
use crate::motion;
use crate::rope::Rope;
use crate::rope::tests::{floor_boundary, has_long_cluster, text_strategy};
use crate::selection::Selection;

/// What a model edit did: `old` at `start` became `new`.
#[derive(Clone, Debug)]
struct ModelEdit {
    start: usize,
    old: String,
    new: String,
}

/// A model of one undo step.
#[derive(Clone, Debug)]
struct ModelGroup {
    /// Batches of edits, each sorted and in the offsets before it, in order.
    batches: Vec<Vec<ModelEdit>>,
    selected_before: Vec<usize>,
    selected_after: Vec<usize>,
    content_before: u64,
    content_after: u64,
}

struct Model {
    text: String,
    /// Every window's sorted cursors. The first is selected.
    windows: Vec<Vec<usize>>,
    undo: Vec<ModelGroup>,
    redo: Vec<ModelGroup>,
    content: u64,
    saved: u64,
    next_content: u64,
}

/// `start` to `end` clamped, as edits are: to the text and to characters, and
/// a reversed range is empty at its start.
fn model_clamp(text: &str, start: usize, end: usize) -> (usize, usize) {
    let start = floor_boundary(text, start);
    (start, floor_boundary(text, end).max(start))
}

/// The batch of `edits` on `text`.
fn model_batch(text: &str, edits: &[(usize, usize, String)]) -> Vec<ModelEdit> {
    let mut edits: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|(a, b, new)| {
            let (start, end) = model_clamp(text, *a, *b);
            (start, end, new.clone())
        })
        .filter(|(start, end, new)| start != end || !new.is_empty())
        .collect();
    edits.sort_by_key(|(start, end, _)| (*start, *end));
    let mut merged: Vec<(usize, usize, String)> = Vec::new();
    for (start, end, new) in edits {
        if let Some(last) = merged.last_mut() {
            let same_point = last.0 == last.1 && start == end && start == last.0;
            if start < last.1 || same_point {
                last.1 = last.1.max(end);
                last.2.push_str(&new);
                continue;
            }
        }
        merged.push((start, end, new));
    }
    merged
        .into_iter()
        .map(|(start, end, new)| ModelEdit {
            start,
            old: text[start..end].to_owned(),
            new,
        })
        .collect()
}

/// `edits` applied to `text`, from the last to the first.
fn model_apply(text: &mut String, edits: &[ModelEdit]) {
    for edit in edits.iter().rev() {
        text.replace_range(edit.start..edit.start + edit.old.len(), &edit.new);
    }
}

/// What undoes `edits`, in the offsets after them.
fn model_inverse(edits: &[ModelEdit]) -> Vec<ModelEdit> {
    let mut shift = 0isize;
    edits
        .iter()
        .map(|edit| {
            let start = edit.start.checked_add_signed(shift).unwrap();
            shift += edit.new.len() as isize - edit.old.len() as isize;
            ModelEdit {
                start,
                old: edit.new.clone(),
                new: edit.old.clone(),
            }
        })
        .collect()
}

/// A cursor through `edits`, one at a time from the first, working in the
/// offsets of the text before them all.
fn model_follow(cursor: usize, edits: &[ModelEdit], selected: bool) -> usize {
    let mut shift = 0isize;
    for edit in edits {
        let end = edit.start + edit.old.len();
        let delta = edit.new.len() as isize - edit.old.len() as isize;
        let at = |offset: usize| offset.checked_add_signed(shift).unwrap();
        if selected {
            if cursor < edit.start {
                return at(cursor);
            }
            if cursor < end {
                return at(edit.start) + edit.new.len();
            }
        } else {
            if cursor <= edit.start {
                return at(cursor);
            }
            if cursor < end {
                return at(edit.start);
            }
        }
        shift += delta;
    }
    cursor.checked_add_signed(shift).unwrap()
}

/// Where `cursor` lands: the start of the display cluster around it, or the end
/// of its row if it is at or inside a line break. Found by walking the row's
/// clusters in a display map built anew.
fn model_cluster_start(text: &str, display: &DisplaySnapshot, cursor: usize) -> usize {
    let cursor = floor_boundary(text, cursor);
    let row = text[..cursor].matches('\n').count();
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let mut row_end = line_start;
    for cluster in display.clusters(DisplayPoint::new(row, 0)) {
        let end = cluster.offset + cluster.text.len();
        if (cluster.offset..end).contains(&cursor) {
            return cluster.offset;
        }
        row_end = end;
    }
    row_end
}

/// Where a cursor inside a cluster goes when new text before it must stay before
/// it: past the cluster around it, or past the line break it is inside.
fn model_cluster_end(text: &str, display: &DisplaySnapshot, cursor: usize) -> usize {
    let cursor = floor_boundary(text, cursor);
    if model_cluster_start(text, display, cursor) == cursor {
        return cursor;
    }
    let row = text[..cursor].matches('\n').count();
    for cluster in display.clusters(DisplayPoint::new(row, 0)) {
        let end = cluster.offset + cluster.text.len();
        if (cluster.offset..end).contains(&cursor) {
            return end;
        }
    }
    cursor + text[cursor..].find('\n').map_or(0, |index| index + 1)
}

/// `cursors` clipped to display clusters, sorted and merged: to the end of a
/// cluster for those at one of `ends`, and to the start for the rest.
fn model_clip_after(text: &str, cursors: Vec<usize>, ends: &[usize]) -> Vec<usize> {
    let display = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
    let mut clipped: Vec<usize> = cursors
        .into_iter()
        .map(|cursor| {
            if ends.contains(&cursor) {
                model_cluster_end(text, &display, cursor)
            } else {
                model_cluster_start(text, &display, cursor)
            }
        })
        .collect();
    clipped.sort_unstable();
    clipped.dedup();
    clipped
}

fn model_clip(text: &str, cursors: Vec<usize>) -> Vec<usize> {
    model_clip_after(text, cursors, &[])
}

/// Where the new text of `edits` ends, in the offsets after them.
fn model_new_text_ends(edits: &[ModelEdit]) -> Vec<usize> {
    let mut shift = 0isize;
    let mut ends = Vec::new();
    for edit in edits {
        let new_end = edit.start.checked_add_signed(shift).unwrap() + edit.new.len();
        if !edit.new.is_empty() {
            ends.push(new_end);
        }
        shift += edit.new.len() as isize - edit.old.len() as isize;
    }
    ends
}

impl Model {
    fn new(text: &str, windows: usize) -> Self {
        Self {
            text: text.to_owned(),
            windows: vec![vec![0]; windows],
            undo: Vec::new(),
            redo: Vec::new(),
            content: 0,
            saved: 0,
            next_content: 1,
        }
    }

    fn set_cursors(&mut self, window: usize, cursors: Vec<usize>) {
        self.windows[window] = cursors;
    }

    /// Moves cursors through `steps`, except the selected ones if `restored`.
    fn move_cursors(&mut self, steps: &[Vec<ModelEdit>], restored: Option<&[usize]>) {
        let text = self.text.clone();
        for (index, window) in self.windows.iter_mut().enumerate() {
            let selected = index == 0;
            let moved = match restored {
                Some(restored) if selected => restored.to_vec(),
                _ => {
                    let mut moved = window.clone();
                    for edits in steps {
                        for cursor in &mut moved {
                            *cursor = model_follow(*cursor, edits, selected && restored.is_none());
                        }
                    }
                    moved
                }
            };
            // Only an edit moves the selected cursors past new text.
            let ends = match restored {
                None if selected => model_new_text_ends(&steps[0]),
                _ => Vec::new(),
            };
            *window = model_clip_after(&text, moved, &ends);
        }
    }

    /// One command made of `batches` of edits.
    fn command(&mut self, batches: &[Vec<(usize, usize, String)>]) {
        let selected_before = self.windows[0].clone();
        let content_before = self.content;
        let mut applied = Vec::new();
        for edits in batches {
            let batch = model_batch(&self.text, edits);
            if batch.is_empty() {
                continue;
            }
            model_apply(&mut self.text, &batch);
            self.move_cursors(std::slice::from_ref(&batch), None);
            applied.push(batch);
            self.content = self.next_content;
            self.next_content += 1;
        }
        if !applied.is_empty() {
            self.redo.clear();
            self.undo.push(ModelGroup {
                batches: applied,
                selected_before,
                selected_after: self.windows[0].clone(),
                content_before,
                content_after: self.content,
            });
        }
    }

    fn cross(&mut self, undo: bool) -> bool {
        let group = if undo {
            self.undo.pop()
        } else {
            self.redo.pop()
        };
        let Some(group) = group else {
            return false;
        };
        let steps: Vec<Vec<ModelEdit>> = if undo {
            group
                .batches
                .iter()
                .rev()
                .map(|edits| model_inverse(edits))
                .collect()
        } else {
            group.batches.clone()
        };
        for edits in &steps {
            model_apply(&mut self.text, edits);
        }
        let (selected, content) = if undo {
            (&group.selected_before, group.content_before)
        } else {
            (&group.selected_after, group.content_after)
        };
        self.content = content;
        self.move_cursors(&steps, Some(selected));
        if undo {
            self.redo.push(group);
        } else {
            self.undo.push(group);
        }
        true
    }
}

/// One step of the random tests.
#[derive(Clone, Debug)]
enum Step {
    /// A command: batches of edits, some at the cursors of the first window
    /// and some anywhere.
    Command(Vec<Vec<(usize, usize, String)>>),
    /// A command that types at every cursor.
    AtCursors {
        before: usize,
        after: usize,
        text: String,
    },
    Undo,
    Redo,
    Save,
    /// Places a window's cursors.
    Cursors(usize, Vec<usize>),
}

fn step_strategy() -> impl Strategy<Value = Step> {
    let edit = (0usize..120, 0usize..120, text_strategy(4));
    prop_oneof![
        3 => prop::collection::vec(prop::collection::vec(edit, 0..5), 1..3).prop_map(Step::Command),
        3 => (0usize..8, 0usize..8, text_strategy(4))
            .prop_map(|(before, after, text)| Step::AtCursors { before, after, text }),
        2 => Just(Step::Undo),
        2 => Just(Step::Redo),
        1 => Just(Step::Save),
        2 => (0usize..3, prop::collection::vec(0usize..150, 1..6))
            .prop_map(|(window, cursors)| Step::Cursors(window, cursors)),
    ]
}

/// Compares everything the editor shows with the model.
fn check_against_model(editor: &Editor, id: BufferId, model: &Model) {
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.text().to_string(), model.text);
    buffer.text().check_invariants();
    assert_eq!(buffer.is_modified(), model.content != model.saved);
    if has_long_cluster(&model.text) {
        // Long clusters are split at chunks, which differ between ropes.
        return;
    }
    let display = buffer.display();
    crate::display_map::tests::check_against(&display, &model.text, TabWidth::default());
    for (window, expected) in editor.windows.iter().zip(&model.windows) {
        let set = &window.selections;
        assert!(!set.is_empty());
        assert_eq!(set.iter().count(), set.len());
        assert!(set.primary_index() < set.len());
        let got = cursors(window);
        assert!(got.windows(2).all(|pair| pair[0] < pair[1]), "{got:?}");
        for cursor in &got {
            assert!(*cursor <= model.text.len());
            assert_eq!(motion::cluster_start(&display, *cursor), *cursor);
        }
        assert_eq!(&got, expected, "{:?}", model.text);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn edits_undo_and_redo_match_a_string(
        start in text_strategy(30),
        steps in prop::collection::vec(step_strategy(), 1..16),
    ) {
        const WINDOWS: usize = 3;
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &start));
        let id = editor.window().buffer();
        for _ in 1..WINDOWS {
            let window = editor.new_window(id);
            editor.windows.push(window);
        }
        let mut model = Model::new(&start, WINDOWS);
        check_against_model(&editor, id, &model);
        for step in steps {
            let revision = editor.buffer(id).unwrap().revision();
            let content = editor.buffer(id).unwrap().content_id();
            let undo_before = (model.undo.len(), model.redo.len());
            match step {
                Step::Command(batches) => {
                    editor.begin_undo_group();
                    for edits in &batches {
                        let replacements: Vec<_> = edits
                            .iter()
                            .map(|(a, b, new)| edit(*a..*b, new))
                            .collect();
                        editor.apply_edits(id, &replacements).unwrap();
                    }
                    editor.end_undo_group();
                    model.command(&batches);
                }
                Step::AtCursors { before, after, text } => {
                    let edits: Vec<(usize, usize, String)> = model.windows[0]
                        .iter()
                        .map(|c| (c.saturating_sub(before), c + after, text.clone()))
                        .collect();
                    editor.begin_undo_group();
                    let replacements: Vec<_> = edits
                        .iter()
                        .map(|(a, b, new)| edit(*a..*b, new))
                        .collect();
                    editor.apply_edits(id, &replacements).unwrap();
                    editor.end_undo_group();
                    model.command(&[edits]);
                }
                Step::Undo => {
                    let done = editor.undo().is_ok();
                    prop_assert_eq!(done, model.cross(true));
                }
                Step::Redo => {
                    let done = editor.redo().is_ok();
                    prop_assert_eq!(done, model.cross(false));
                }
                Step::Save => {
                    let now = editor.buffer(id).unwrap().content_id();
                    editor.buffer_mut(id).unwrap().mark_saved(now);
                    model.saved = model.content;
                }
                Step::Cursors(window, offsets) => {
                    let text = editor.buffer(id).unwrap().text().to_string();
                    let clipped = model_clip(&text, offsets);
                    editor.windows[window].selections = set_at(&clipped);
                    model.set_cursors(window, clipped);
                }
            }
            check_against_model(&editor, id, &model);
            // The revision and the content id move with the content, and only
            // then. A command in a row of them changed the history, or nothing.
            let changed = undo_before != (model.undo.len(), model.redo.len());
            let buffer = editor.buffer(id).unwrap();
            prop_assert_eq!(buffer.revision() != revision, changed);
            prop_assert_eq!(buffer.content_id() != content, changed);
        }
    }

    /// Whatever it joins, what was typed stays in order before the cursor.
    #[test]
    fn typed_text_stays_before_the_cursor(
        start in text_strategy(12),
        at in 0usize..60,
        pieces in prop::collection::vec(text_strategy(2), 1..8),
    ) {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &start));
        let id = editor.window().buffer();
        let display = editor.buffer(id).unwrap().display();
        editor.current_mut().0.selections = SelectionSet::new(motion::cluster_start(&display, at));
        for piece in pieces {
            let cursor = editor.window().selections.primary().cursor;
            editor.apply_edits(id, &[edit(cursor..cursor, &piece)]).unwrap();
            let after = text(&editor, id);
            prop_assert_eq!(&after[cursor..cursor + piece.len()], piece.as_str());
            prop_assert!(editor.window().selections.primary().cursor >= cursor + piece.len());
        }
    }

    /// A random group opened and left: closing it on the panic path keeps
    /// buffer and history in step, and everything undoes to the start.
    #[test]
    fn undoing_everything_returns_to_the_start(
        start in text_strategy(20),
        commands in prop::collection::vec(
            prop::collection::vec((0usize..80, 0usize..80, text_strategy(4)), 0..4),
            0..10,
        ),
        panics in prop::collection::vec(any::<bool>(), 10),
    ) {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &start));
        let id = editor.window().buffer();
        for (edits, panics) in commands.iter().zip(panics) {
            let replacements: Vec<_> = edits.iter().map(|(a, b, new)| edit(*a..*b, new)).collect();
            editor.begin_undo_group();
            editor.apply_edits(id, &replacements).unwrap();
            if panics {
                editor.end_undo_group_after_panic();
            } else {
                editor.end_undo_group();
            }
        }
        while editor.undo().is_ok() {}
        prop_assert_eq!(text(&editor, id), start.clone());
        let buffer = editor.buffer(id).unwrap();
        buffer.text().check_invariants();
        prop_assert!(!buffer.is_modified());
        while editor.redo().is_ok() {}
        prop_assert_eq!(editor.buffer(id).unwrap().undo_lens().1, 0);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Cursors, marks and the marks in the ring stay where a cursor can be through
    /// edits, undo and redo, though only those near an edit are clipped (ADR 0091).
    #[test]
    fn marks_stay_at_clusters_when_only_those_near_an_edit_are_clipped(
        start in text_strategy(30),
        places in prop::collection::vec((0usize..150, 0usize..150), 1..6),
        steps in prop::collection::vec(step_strategy(), 1..12),
    ) {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &start));
        let id = editor.window().buffer();
        let display = editor.buffer(id).unwrap().display();
        let at = |offset: usize| motion::cluster_start(&display, floor_boundary(&start, offset));
        let parts = places
            .iter()
            .map(|&(cursor, mark)| Selection {
                cursor: at(cursor),
                mark: Some(at(mark)),
                goal: None,
            })
            .collect();
        let mut set = SelectionSet::from_parts(parts, 0).unwrap();
        set.push_marks();
        editor.windows[0].selections = set;
        for step in steps {
            match step {
                Step::Command(batches) => {
                    editor.begin_undo_group();
                    for edits in &batches {
                        let replacements: Vec<_> =
                            edits.iter().map(|(a, b, new)| edit(*a..*b, new)).collect();
                        let _ = editor.apply_edits(id, &replacements);
                    }
                    editor.end_undo_group();
                }
                Step::AtCursors { before, after, text } => {
                    let replacements: Vec<_> = editor.windows[0]
                        .selections
                        .iter()
                        .map(|s| edit(s.cursor.saturating_sub(before)..s.cursor + after, &text))
                        .collect();
                    editor.begin_undo_group();
                    let _ = editor.apply_edits(id, &replacements);
                    editor.end_undo_group();
                }
                Step::Undo => {
                    let _ = editor.undo();
                }
                Step::Redo => {
                    let _ = editor.redo();
                }
                Step::Save | Step::Cursors(..) => {}
            }
            let display = editor.buffer(id).unwrap().display();
            let set = &editor.windows[0].selections;
            for selection in set.iter() {
                prop_assert_eq!(motion::cluster_start(&display, selection.cursor), selection.cursor);
                if let Some(mark) = selection.mark {
                    prop_assert_eq!(motion::cluster_start(&display, mark), mark);
                }
            }
            for mark in set.mark_ring().iter() {
                prop_assert_eq!(motion::cluster_start(&display, mark), mark);
            }
        }
    }
}
