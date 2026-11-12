//! Input: key presses and the mouse wheel, from the window to the editor.
//!
//! Must not depend on: the render pipeline, beyond the size of a cell, or loading
//! files. Input changes the editor and asks for a frame; what a key does is
//! decided by the keymaps.

use std::time::Instant;

use oso_core::buffer::Mode;
use oso_core::key::KeyPress;
use oso_core::window::{WheelRemainder, WindowId};
use oso_platform::window::{Held, KeyInput, Position, Wheel};

use super::App;
use super::shell::Loop;
#[cfg(feature = "dev")]
use crate::dev;
use crate::keys;

/// Rows one notch of a mouse wheel scrolls.
const WHEEL_ROWS: f64 = 3.0;

/// Columns one notch of a sideways wheel scrolls: about as far as the rows, since a
/// cell is about half as wide as it is high.
const WHEEL_COLUMNS: f64 = 6.0;

/// Wheel and trackpad scrolling not yet worth a whole row or column, and the window it is
/// for: a wheel over another window starts anew (ADR 0112).
#[derive(Default)]
pub(super) struct Wheeling {
    window: Option<WindowId>,
    remainder: WheelRemainder,
}

/// Whether `key` from the window must not reach the innermost prompt, `innermost`: a question,
/// or a prompt that completes, that a control socket request opened, among `remote`. `C-g`
/// still closes it.
#[cfg(feature = "dev")]
fn held_from_the_window(
    remote: &[oso_core::minibuffer::PromptId],
    innermost: Option<oso_core::minibuffer::PromptId>,
    key: KeyPress,
) -> bool {
    key != oso_core::keymap::CANCEL && innermost.is_some_and(|id| remote.contains(&id))
}

/// Whether `key` in a buffer of `mode`, or in a question, is an answer and not something to
/// repeat. Where a letter answers, holding it must not go on through the rows, and some answers
/// delete. The app also drops the repeats of a press this was true of, as an answer can move the
/// window to a file, where the same letter would type, and a held `y` must answer once
/// (ADR 0072).
fn answers_once(mode: Mode, question: bool, key: KeyPress) -> bool {
    (mode.shows_copies() || question) && key.text().is_some()
}

impl App {
    /// Remembers the modifier keys held, as the window system reports them.
    pub(super) fn set_modifiers(&mut self, modifiers: Held) {
        self.modifiers = modifiers;
    }

    /// Turns a key event from the window into a key press, and handles it.
    pub(super) fn handle_key_event(&mut self, event_loop: &Loop<'_>, input: &KeyInput<'_>) {
        if let Some(key) = keys::from_event(input, self.modifiers) {
            let mode = self.editor.current().1.mode();
            let question = self
                .editor
                .prompt()
                .is_some_and(|prompt| prompt.kind().is_question());
            if input.repeat {
                if self.answering == Some(key) || answers_once(mode, question, key) {
                    return;
                }
            } else {
                self.answering = answers_once(mode, question, key).then_some(key);
            }
            if self.note_key_for_agent(key) {
                return;
            }
            #[cfg(feature = "dev")]
            if self.asked_remotely(key) {
                return;
            }
            self.handle_key(event_loop, key);
        } else if let Some(text) = keys::typed_text(input, self.modifiers) {
            self.handle_text(event_loop, text);
        }
    }

    /// Whether the innermost prompt is a question a control socket request asked, which `key`
    /// from the window must not answer. It says so, and only `C-g` goes through (ADR 0072).
    #[cfg(feature = "dev")]
    fn asked_remotely(&mut self, key: KeyPress) -> bool {
        let innermost = self.editor.prompt().map(|prompt| prompt.id());
        if !held_from_the_window(&self.remote_questions, innermost, key) {
            return false;
        }
        self.editor
            .prompt_says("Asked over the control socket: answer there, or C-g");
        self.request_redraw();
        true
    }

    /// Handles one key press from the window, through the keymaps and the
    /// command registry. Keys from the dev control socket take the same
    /// path, through [`crate::dispatch::Dispatcher::handle_keys_with`].
    fn handle_key(&mut self, event_loop: &Loop<'_>, key: KeyPress) {
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let mut watch = dev::SecretWatch::new(&self.editor);
        let effects = self.dispatcher.handle_key(&mut self.editor, key);
        // Before anything more is logged, for a key that brought a secret buffer into view.
        self.note_logging();
        log::trace!("key handled in {:?}", started.elapsed());
        #[cfg(feature = "dev")]
        self.timings.key_handled(
            started,
            dev::HandledKey {
                redraw: effects.redraw,
                withheld: watch.after_key(&self.editor),
            },
        );
        self.apply(event_loop, effects);
    }

    /// Handles text from one key press that types several characters.
    fn handle_text(&mut self, event_loop: &Loop<'_>, text: &str) {
        self.person_typed("text");
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let mut watch = dev::SecretWatch::new(&self.editor);
        let effects = self.dispatcher.insert_text(&mut self.editor, text);
        self.note_logging();
        #[cfg(feature = "dev")]
        self.timings.key_handled(
            started,
            dev::HandledKey {
                redraw: effects.redraw,
                withheld: watch.after_key(&self.editor),
            },
        );
        log::trace!("text handled in {:?}", started.elapsed());
        self.apply(event_loop, effects);
    }

    /// Scrolls the view for the mouse wheel or the trackpad: of the window under the pointer
    /// `at`, without selecting it, or of the selected one where no window is (ADR 0112).
    /// Returns the window and whether its view moved, or `None` before the window opens.
    pub(super) fn handle_wheel(
        &mut self,
        delta: Wheel,
        at: Option<Position>,
    ) -> Option<(WindowId, bool)> {
        let view = self.view.as_ref()?;
        let grid = view.render_state.pixel_grid();
        let cell = at.and_then(|at| grid.cell_at(at.x, at.y));
        let window = self.editor.wheel_window(cell);
        // Positive deltas move the content down and right: the view moves the other
        // way.
        let (rows, columns) = match delta {
            Wheel::Lines { x, y } => (-f64::from(y) * WHEEL_ROWS, -f64::from(x) * WHEEL_COLUMNS),
            Wheel::Pixels { x, y } => {
                let metrics = view.render_state.cell_metrics();
                (
                    -y / f64::from(metrics.height).max(1.0),
                    -x / f64::from(metrics.width).max(1.0),
                )
            }
        };
        let wheeling = &mut self.wheel;
        if wheeling.window != Some(window) {
            *wheeling = Wheeling {
                window: Some(window),
                remainder: WheelRemainder::default(),
            };
        }
        let (rows, columns) = wheeling.remainder.add(rows, columns);
        let moved_rows = rows != 0 && self.editor.scroll_view(window, rows);
        let moved_columns = columns != 0 && self.editor.scroll_view_sideways(window, columns);
        let moved = moved_rows || moved_columns;
        if moved {
            self.request_redraw();
        }
        Some((window, moved))
    }
}

#[cfg(test)]
mod tests {
    use super::answers_once;
    use oso_core::buffer::Mode;
    use oso_core::key::KeySequence;

    fn key(name: &str) -> oso_core::key::KeyPress {
        let keys: KeySequence = name.parse().unwrap();
        keys.0[0]
    }

    #[cfg(feature = "dev")]
    #[test]
    fn a_question_the_socket_asked_takes_only_c_g_from_the_window() {
        use super::held_from_the_window;
        let mut editor = oso_core::editor::Editor::default();
        let asked = editor.ask("Exit?", "keyboard-quit").unwrap();
        let mine = editor.ask("Again?", "keyboard-quit").unwrap();
        assert!(held_from_the_window(&[asked], Some(asked), key("y")));
        assert!(held_from_the_window(&[asked], Some(asked), key("n")));
        assert!(!held_from_the_window(&[asked], Some(asked), key("C-g")));
        assert!(
            !held_from_the_window(&[asked], Some(mine), key("y")),
            "asked by a key"
        );
        assert!(!held_from_the_window(&[asked], None, key("y")));
    }

    #[test]
    fn only_a_letter_among_the_copies_answers_once() {
        assert!(answers_once(Mode::CopyList, false, key("d")));
        assert!(answers_once(Mode::CopyView, false, key("r")));
        assert!(
            !answers_once(Mode::CopyList, false, key("C-n")),
            "motion repeats"
        );
        assert!(!answers_once(Mode::CopyList, false, key("<down>")));
        assert!(
            !answers_once(Mode::Fundamental, false, key("d")),
            "typing repeats"
        );
        assert!(
            answers_once(Mode::Fundamental, true, key("y")),
            "a question"
        );
    }
}
