//! Input: key presses and the mouse wheel, from the window to the editor.
//!
//! Must not depend on: the render pipeline, beyond the size of a cell, or loading
//! files. Input changes the editor and asks for a frame; what a key does is
//! decided by the keymaps.

use std::time::Instant;

use oso_core::buffer::Mode;
use oso_core::key::KeyPress;

use super::App;
use super::events::Wheel;
use super::shell::Loop;
#[cfg(feature = "dev")]
use crate::dev;
use crate::keys::{self, Held, KeyInput};

/// Rows one notch of a mouse wheel scrolls.
const WHEEL_ROWS: f64 = 3.0;

/// Columns one notch of a sideways wheel scrolls: about as far as the rows, since a
/// cell is about half as wide as it is high.
const WHEEL_COLUMNS: f64 = 6.0;

/// Whether `key` in a buffer of `mode` is an answer to a question and not something to repeat.
/// Where a letter answers, holding it must not go on through the rows, and some answers delete.
/// The app also drops the repeats of a press this was true of, as an answer can move the window
/// to a file, where the same letter would type.
fn answers_once(mode: Mode, key: KeyPress) -> bool {
    mode.shows_copies() && key.text().is_some()
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
            if input.repeat {
                if self.answering == Some(key) || answers_once(mode, key) {
                    return;
                }
            } else {
                self.answering = answers_once(mode, key).then_some(key);
            }
            self.handle_key(event_loop, key);
        } else if let Some(text) = keys::typed_text(input, self.modifiers) {
            self.handle_text(event_loop, text);
        }
    }

    /// Handles one key press from the window, through the keymaps and the
    /// command registry. Keys from the dev control socket take the same
    /// path, through [`crate::dispatch::Dispatcher::handle_keys_with`].
    fn handle_key(&mut self, event_loop: &Loop<'_>, key: KeyPress) {
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let mut watch = dev::SecretWatch::new(&self.editor);
        let effects = self.dispatcher.handle_key(&mut self.editor, key);
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
        let started = Instant::now();
        #[cfg(feature = "dev")]
        let mut watch = dev::SecretWatch::new(&self.editor);
        let effects = self.dispatcher.insert_text(&mut self.editor, text);
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

    /// Scrolls the view for the mouse wheel or the trackpad.
    pub(super) fn handle_wheel(&mut self, delta: Wheel) {
        // Positive deltas move the content down and right: the view moves the other
        // way.
        let (rows, columns) = match delta {
            Wheel::Lines { x, y } => (-f64::from(y) * WHEEL_ROWS, -f64::from(x) * WHEEL_COLUMNS),
            Wheel::Pixels { x, y } => {
                let Some(view) = &self.view else {
                    return;
                };
                let metrics = view.render_state.cell_metrics();
                (
                    -y / f64::from(metrics.height).max(1.0),
                    -x / f64::from(metrics.width).max(1.0),
                )
            }
        };
        let (rows, columns) = self.wheel.add(rows, columns);
        let moved_rows = rows != 0 && self.editor.scroll_view(rows);
        let moved_columns = columns != 0 && self.editor.scroll_view_sideways(columns);
        if moved_rows || moved_columns {
            self.request_redraw();
        }
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

    #[test]
    fn only_a_letter_among_the_copies_answers_once() {
        assert!(answers_once(Mode::CopyList, key("d")));
        assert!(answers_once(Mode::CopyView, key("r")));
        assert!(!answers_once(Mode::CopyList, key("C-n")), "motion repeats");
        assert!(!answers_once(Mode::CopyList, key("<down>")));
        assert!(!answers_once(Mode::Fundamental, key("d")), "typing repeats");
    }
}
