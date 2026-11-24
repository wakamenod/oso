//! Input: key presses and the mouse wheel, from the window to the editor. The mouse's buttons
//! are `terminals::mouse`'s.
//!
//! Must not depend on: the render pipeline, beyond the size of a cell, or loading
//! files. Input changes the editor and asks for a frame; what a key does is
//! decided by the keymaps.

use std::time::Instant;

use oso_core::buffer::Mode;
use oso_core::key::{Key, KeyPress, NamedKey};
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
/// delete; on a native session's card `RET` answers too, in its plan's edit a chord does, while
/// `sequence` says one is under way, and on its request's line every key but those that move: no
/// repeat of one answers (ADR 0182). The app also
/// drops the repeats of a press this was true of, as an answer can move the window to a file,
/// where the same letter would type, and a held `y` must answer once (ADR 0072).
fn answers_once(mode: Mode, question: bool, sequence: bool, key: KeyPress) -> bool {
    let letter = key.text().is_some();
    (question && letter)
        || match mode {
            Mode::Card => letter || key.key == Key::Named(NamedKey::Return),
            Mode::Request => {
                (letter && key.text() != Some(' '))
                    || key.key == Key::Named(NamedKey::Return)
                    || sequence
            }
            Mode::Plan => sequence,
            mode => mode.shows_copies() && letter,
        }
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
            let sequence = self.dispatcher.keys_in_progress();
            let once = answers_once(mode, question, sequence, key);
            if input.repeat {
                if self.answering == Some(key) || once {
                    return;
                }
            } else {
                self.answering = once.then_some(key);
            }
            self.dispatcher.set_key_repeats(input.repeat);
            if self.note_key_for_agent(key) {
                return;
            }
            #[cfg(feature = "dev")]
            if self.asked_remotely(key) {
                return;
            }
            let answers = !input.repeat && self.composer_key_answers(key);
            self.dispatcher.set_key_answers_card(answers);
            self.handle_key(event_loop, key);
            // A key that answered a card from the composer answers once too (ADR 0180).
            if self.dispatcher.take_key_answered_card() {
                self.answering = Some(key);
            }
        } else if let Some(text) = keys::typed_text(input, self.modifiers) {
            self.handle_text(event_loop, text);
        } else if !input.repeat {
            // A key with no Emacs name still shows in `view-lossage` (ADR 0150).
            self.dispatcher.note_unnamed_key(&self.editor);
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
        self.note_person_acted();
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
        self.note_person_acted();
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
        // Over a request's line, the transcript above it (ADR 0182).
        let window = self.wheel_over_request(self.editor.wheel_window(cell));
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
        // Over a server's description at point, the wheel scrolls it (ADR 0138).
        let over_hover =
            (self.editor.hover_layout())
                .zip(cell)
                .is_some_and(|(regions, (row, column))| {
                    let cells = regions.outer;
                    (cells.row..cells.row + cells.rows).contains(&row)
                        && (cells.column..cells.column + cells.columns).contains(&column)
                });
        if over_hover {
            let moved = rows != 0 && self.editor.scroll_hover(rows);
            if moved {
                self.request_redraw();
            }
            return Some((window, moved));
        }
        // Elsewhere it scrolls the view, which the description goes with.
        if self.editor.hover().is_some() {
            self.editor.close_hover();
            self.request_redraw();
        }
        // Over a terminal, the wheel scrolls its scrollback, on its thread (ADR 0120).
        if self.wheel_terminal(window, (rows, columns), at) {
            return Some((window, false));
        }
        if crate::dispatch::holds_input(&self.editor) {
            match self
                .held_wheel
                .iter_mut()
                .find(|(held, ..)| *held == window)
            {
                Some((_, held_rows, held_columns)) => {
                    *held_rows = held_rows.saturating_add(rows);
                    *held_columns = held_columns.saturating_add(columns);
                }
                None => self.held_wheel.push((window, rows, columns)),
            }
            return Some((window, false));
        }
        let moved_rows = rows != 0 && self.editor.scroll_view(window, rows);
        let moved_columns = columns != 0 && self.editor.scroll_view_sideways(window, columns);
        let moved = moved_rows || moved_columns;
        if moved {
            self.note_person_acted();
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
    fn a_held_answer_answers_once_and_what_moves_or_edits_repeats() {
        let once = [
            (Mode::CopyList, false, false, "d"),
            (Mode::CopyView, false, false, "r"),
            // A held `y` on a native session's card answers once, and types nothing after it.
            (Mode::Card, false, false, "y"),
            (Mode::Card, false, false, "3"),
            // `RET` there gives a question's marked options.
            (Mode::Card, false, false, "RET"),
            (Mode::Card, false, false, "S-RET"),
            // A request's line's, its chord and an argument's key among them.
            (Mode::Request, false, false, "y"),
            (Mode::Request, false, false, "n"),
            (Mode::Request, false, false, "N"),
            (Mode::Request, false, false, "RET"),
            (Mode::Request, false, true, "C-k"),
            (Mode::Request, false, true, "C-u"),
            // In a plan's edit, what comes in a chord.
            (Mode::Plan, false, true, "C-c"),
            (Mode::Plan, false, true, "C-A"),
            // A question.
            (Mode::Fundamental, true, false, "y"),
            (Mode::Plan, true, false, "y"),
        ];
        for (mode, question, sequence, name) in once {
            assert!(
                answers_once(mode, question, sequence, key(name)),
                "{mode:?} {name}"
            );
        }
        let repeats = [
            (Mode::CopyList, false, false, "C-n"),
            (Mode::CopyList, false, false, "<down>"),
            (Mode::Card, false, false, "C-n"),
            (Mode::Card, false, false, "<down>"),
            (Mode::Card, false, false, "DEL"),
            (Mode::Request, false, false, "SPC"),
            (Mode::Request, false, false, "DEL"),
            (Mode::Request, false, false, "C-v"),
            (Mode::Plan, false, false, "x"),
            (Mode::Plan, false, false, "RET"),
            (Mode::Plan, false, false, "C-k"),
            (Mode::Plan, false, false, "DEL"),
            (Mode::Fundamental, false, false, "d"),
            (Mode::Fundamental, false, true, "C-c"),
        ];
        for (mode, question, sequence, name) in repeats {
            assert!(
                !answers_once(mode, question, sequence, key(name)),
                "{mode:?} {name}"
            );
        }
    }

    #[test]
    fn a_chord_in_a_plans_edit_is_answered_by_keys_pressed() {
        use oso_core::buffer::Buffer;
        use oso_core::editor::Editor;
        let mut plan = Buffer::from_text("*plan: s*", "Do it\n");
        assert!(plan.set_mode(Mode::Plan));
        let mut editor = Editor::with_buffer(plan);
        let mut dispatcher = crate::dispatch::Dispatcher::new();
        dispatcher.handle_key(&mut editor, key("C-c"));
        let sequence = dispatcher.keys_in_progress();
        assert!(answers_once(Mode::Plan, false, sequence, key("C-c")));
        // Pressed, it ends the chord.
        dispatcher.handle_key(&mut editor, key("C-c"));
        let asked = editor.take_session_requests();
        assert_eq!(asked.len(), 1, "{asked:?}");
    }
}
