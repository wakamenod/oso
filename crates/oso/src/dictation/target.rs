//! Where a dictation's text goes (ADR 0175): the selected window, its buffer and its cursors as
//! they were when the dictation started, which a buffer, the minibuffer or a native session's
//! composer all are, as each is a window. The text goes in at each of those cursors in one
//! undo step, or into the kill ring when the place is gone.
//!
//! Must not depend on: `app`, threads or the API; this reads and changes the editor alone.
//!
//! - Refused at the start: a passphrase prompt, a question, a terminal and a read-only buffer.
//! - Kept in the kill ring instead: when the window is gone or shows another buffer, the buffer
//!   is gone or read-only, or its text changed meanwhile, so that the cursors no longer say
//!   where the person was. A secret buffer's text makes a secret entry.
//! - The text waits while an input method composes in that window, as typing would.
//! - At most [`MAX_YANK`] bytes go in across all cursors, the bound a yank has; more goes to the
//!   kill ring.

use oso_core::buffer::{BufferId, Replacement};
use oso_core::editor::Editor;
use oso_core::kill::MAX_YANK;
use oso_core::minibuffer::Kind;
use oso_core::window::WindowId;

/// Where a dictation started: the place its text goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    window: WindowId,
    buffer: BufferId,
    /// The buffer's revision then: the cursors hold only while it is the same.
    revision: u64,
    /// The cursors' offsets then, in order, each once.
    cursors: Vec<usize>,
    /// The buffer may hold secrets: what goes to the kill ring is a secret entry.
    secret: bool,
}

/// What became of a dictation's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    /// Typed at each of the target's cursors.
    Typed,
    /// Put in the kill ring, for this reason.
    Kept(&'static str),
}

/// The place a dictation started now would type into: the selected window, its buffer and its
/// cursors.
///
/// # Errors
///
/// Why dictation does not type there, for the echo area.
pub fn capture(editor: &Editor) -> Result<Target, &'static str> {
    let (window, buffer) = editor.current();
    let prompt = editor
        .prompts()
        .iter()
        .find(|prompt| prompt.buffer() == window.buffer());
    match prompt.map(oso_core::minibuffer::Prompt::kind) {
        Some(Kind::Passphrase) => return Err("Dictation never types a passphrase"),
        Some(Kind::Question { .. }) => return Err("Dictation does not answer a question"),
        _ => {}
    }
    if buffer.is_terminal() {
        return Err("Dictation does not type into a terminal");
    }
    if buffer.read_only().is_some() {
        return Err("Dictation cannot type here: the buffer is read-only");
    }
    let mut cursors: Vec<usize> = window.selections.iter().map(|s| s.cursor).collect();
    cursors.sort_unstable();
    cursors.dedup();
    Ok(Target {
        window: window.id(),
        buffer: window.buffer(),
        revision: buffer.revision(),
        cursors,
        secret: buffer.is_secret(),
    })
}

impl Target {
    /// Whether the text must wait: an input method composes in the target's window, which is
    /// selected, as the composition is always the selected window's.
    pub fn waits(&self, editor: &Editor) -> bool {
        editor.preedit().is_some() && editor.window().id() == self.window
    }

    /// Why the text cannot go in at the target now, if it cannot.
    fn gone(&self, editor: &Editor) -> Option<&'static str> {
        let Some(window) = editor.window_by_id(self.window) else {
            return Some("its window is gone");
        };
        if window.buffer() != self.buffer {
            return Some("its window shows another buffer");
        }
        let Some(buffer) = editor.buffer(self.buffer) else {
            return Some("its buffer is gone");
        };
        if buffer.read_only().is_some() {
            return Some("its buffer is read-only now");
        }
        (buffer.revision() != self.revision).then_some("its text changed while you spoke")
    }

    /// Types `text` at each of the target's cursors, in one undo step, or puts it in the kill
    /// ring if the place is gone. Empty text does nothing.
    pub fn place(&self, editor: &mut Editor, text: &str) -> Placed {
        if let Some(why) = self.gone(editor) {
            return self.keep(editor, text, why);
        }
        let total = text.len().checked_mul(self.cursors.len());
        if total.is_none_or(|total| total > MAX_YANK) {
            return self.keep(editor, text, "it is too much text for so many cursors");
        }
        let edits: Vec<Replacement<'_>> = self
            .cursors
            .iter()
            .map(|&at| Replacement {
                range: at..at,
                text,
            })
            .collect();
        match editor.apply_edits(self.buffer, &edits) {
            Ok(()) => {
                // What it typed in a prompt that completes is matched, as typing is.
                editor.match_completion();
                Placed::Typed
            }
            Err(_) => self.keep(editor, text, "it could not be typed there"),
        }
    }

    /// Puts `text` in the kill ring, for `why`.
    pub fn keep(&self, editor: &mut Editor, text: &str, why: &'static str) -> Placed {
        // Secret if the buffer was at the start or is now.
        let now = editor
            .buffer(self.buffer)
            .is_some_and(oso_core::buffer::Buffer::is_secret);
        editor.kill_new(text, self.secret || now);
        Placed::Kept(why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::buffer::Buffer;
    use oso_core::selection::{Selection, SelectionSet};
    use oso_core::window::ViewSize;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    fn text(editor: &Editor) -> String {
        editor.current().1.text().to_string()
    }

    fn undo(editor: &mut Editor) {
        editor.begin_command();
        editor.begin_undo_group();
        editor.undo().unwrap();
        editor.end_undo_group();
        editor.end_command();
    }

    #[test]
    fn the_text_goes_in_at_each_cursor_from_the_start_in_one_undo_step() {
        let mut editor = editor("ab\ncd\n");
        let set = SelectionSet::from_parts(vec![Selection::at(1), Selection::at(4)], 0);
        editor.current_mut().0.selections = set.unwrap();
        let target = capture(&editor).unwrap();
        // The person moves away while speaking; the text goes where they were.
        editor.current_mut().0.selections = SelectionSet::new(0);
        assert_eq!(target.place(&mut editor, "XY"), Placed::Typed);
        assert_eq!(text(&editor), "aXYb\ncXYd\n");
        undo(&mut editor);
        assert_eq!(text(&editor), "ab\ncd\n", "one step undoes both");
    }

    #[test]
    fn a_place_that_changed_or_went_keeps_the_text_in_the_kill_ring() {
        let mut editor = editor("ab");
        let target = capture(&editor).unwrap();
        let id = editor.window().buffer();
        let edit = Replacement {
            range: 2..2,
            text: "c",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        assert_eq!(
            target.place(&mut editor, "said"),
            Placed::Kept("its text changed while you spoke")
        );
        assert_eq!(text(&editor), "abc");
        let newest = editor.kill_ring().get(0).map(|entry| {
            let pieces = entry.pieces().iter().map(oso_core::kill::Piece::text);
            pieces.collect::<String>()
        });
        assert_eq!(newest.as_deref(), Some("said"));

        let mut editor = super::tests::editor("ab");
        let target = capture(&editor).unwrap();
        let other = editor.add_buffer(Buffer::from_text("other.md", ""));
        editor.show(other);
        assert_eq!(
            target.place(&mut editor, "said"),
            Placed::Kept("its window shows another buffer")
        );
    }

    #[test]
    fn a_terminal_a_passphrase_a_question_and_a_read_only_buffer_are_refused() {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.show(terminal);
        assert_eq!(
            capture(&editor),
            Err("Dictation does not type into a terminal")
        );

        let mut editor = editor_with_prompt(Kind::Passphrase);
        assert_eq!(capture(&editor), Err("Dictation never types a passphrase"));
        editor.quit();
        assert!(capture(&editor).is_ok());

        let mut editor = super::tests::editor("x");
        editor.ask("Go?", "keyboard-quit").unwrap();
        assert_eq!(
            capture(&editor),
            Err("Dictation does not answer a question")
        );

        let mut editor = Editor::default();
        let internal = editor.add_internal_buffer("*Help*", oso_core::buffer::Mode::Fundamental);
        editor.show(internal);
        assert_eq!(
            capture(&editor),
            Err("Dictation cannot type here: the buffer is read-only")
        );
    }

    fn editor_with_prompt(kind: Kind) -> Editor {
        let mut editor = editor("x");
        editor.open_prompt("Passphrase: ", kind).unwrap();
        editor
    }

    #[test]
    fn the_minibuffer_takes_the_text_while_its_prompt_is_open() {
        let mut editor = editor("x");
        editor
            .open_prompt("Find file: ", Kind::Text { history: None })
            .unwrap();
        let target = capture(&editor).unwrap();
        assert_eq!(target.place(&mut editor, "notes"), Placed::Typed);
        assert_eq!(text(&editor), "notes");
        editor.quit();
        assert_eq!(text(&editor), "x", "the buffer beneath is untouched");
        assert_eq!(
            target.place(&mut editor, "again"),
            Placed::Kept("its window is gone")
        );
    }

    #[test]
    fn the_text_waits_while_a_composition_is_open_in_its_window() {
        let mut editor = editor("x");
        let target = capture(&editor).unwrap();
        assert!(!target.waits(&editor));
        editor.set_preedit(Some(oso_core::display_map::Preedit::new(
            "にほ".into(),
            None,
        )));
        assert!(target.waits(&editor));
        editor.set_preedit(None);
        assert!(!target.waits(&editor));
    }

    #[test]
    fn text_past_the_yanks_bound_across_the_cursors_goes_to_the_kill_ring() {
        let mut editor = editor("x");
        let mut target = capture(&editor).unwrap();
        target.cursors = vec![0; 300];
        let text = "a".repeat(crate::dictation::MAX_TEXT);
        assert_eq!(
            target.place(&mut editor, &text),
            Placed::Kept("it is too much text for so many cursors")
        );
        assert_eq!(super::tests::text(&editor), "x");
        target.cursors = vec![0];
        assert_eq!(target.place(&mut editor, &text), Placed::Typed);
    }

    #[test]
    fn a_secret_buffers_text_makes_a_secret_kill() {
        let mut editor = editor("x");
        let mut target = capture(&editor).unwrap();
        target.secret = true;
        target.keep(&mut editor, "pw", "its window is gone");
        assert!(editor.kill_ring().iter().next().unwrap().is_secret());
    }
}
