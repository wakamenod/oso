//! Input: key presses and the mouse wheel, from the window to the editor.
//!
//! Must not depend on: the render pipeline, beyond the size of a cell, or loading
//! files. Input changes the editor and asks for a frame; what a key does is
//! decided by the keymaps.

use std::time::Instant;

use oso_core::key::KeyPress;
use winit::event::{KeyEvent, MouseScrollDelta};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;

use super::App;
use crate::keys;

/// Rows one notch of a mouse wheel scrolls.
const WHEEL_ROWS: f64 = 3.0;

/// Columns one notch of a sideways wheel scrolls: about as far as the rows, since a
/// cell is about half as wide as it is high.
const WHEEL_COLUMNS: f64 = 6.0;

impl App {
    /// Remembers the modifier keys held, as winit reports them.
    pub(super) fn set_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }

    /// Turns a key event from the window into a key press, and handles it.
    pub(super) fn handle_key_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: &KeyEvent,
        is_synthetic: bool,
    ) {
        if !keys::is_real_press(event.state, is_synthetic) {
            return;
        }
        if let Some(key) = keys::from_event(event, self.modifiers) {
            self.handle_key(event_loop, key);
        } else if let Some(text) = keys::typed_text(event, self.modifiers) {
            self.handle_text(event_loop, text);
        }
    }

    /// Handles one key press from the window, through the keymaps and the
    /// command registry. Keys from the dev control socket take the same
    /// path, through [`crate::dispatch::Dispatcher::handle_keys`].
    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: KeyPress) {
        let started = Instant::now();
        let effects = self.dispatcher.handle_key(&mut self.editor, key);
        log::trace!("key handled in {:?}", started.elapsed());
        #[cfg(feature = "dev")]
        self.timings.keys_handled(started, 1, effects.redraw);
        self.apply(event_loop, effects);
    }

    /// Handles text from one key press that types several characters.
    fn handle_text(&mut self, event_loop: &ActiveEventLoop, text: &str) {
        let started = Instant::now();
        let effects = self.dispatcher.insert_text(&mut self.editor, text);
        #[cfg(feature = "dev")]
        self.timings.keys_handled(started, 1, effects.redraw);
        log::trace!("text handled in {:?}", started.elapsed());
        self.apply(event_loop, effects);
    }

    /// Scrolls the view for the mouse wheel or the trackpad.
    pub(super) fn handle_wheel(&mut self, delta: MouseScrollDelta) {
        // Positive deltas move the content down and right: the view moves the other
        // way.
        let (rows, columns) = match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                (-f64::from(y) * WHEEL_ROWS, -f64::from(x) * WHEEL_COLUMNS)
            }
            MouseScrollDelta::PixelDelta(position) => {
                let Some(view) = &self.view else {
                    return;
                };
                let metrics = view.render_state.cell_metrics();
                (
                    -position.y / f64::from(metrics.height).max(1.0),
                    -position.x / f64::from(metrics.width).max(1.0),
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
