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
const WHEEL_ROWS: f32 = 3.0;

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

    /// Scrolls the view for the mouse wheel or the trackpad.
    pub(super) fn handle_wheel(&mut self, delta: MouseScrollDelta) {
        // Positive deltas move the content down: the view moves up.
        let rows = match delta {
            MouseScrollDelta::LineDelta(_, y) => (-y * WHEEL_ROWS).round() as isize,
            MouseScrollDelta::PixelDelta(position) => {
                let Some(view) = &self.view else {
                    return;
                };
                let height = f64::from(view.render_state.cell_metrics().height).max(1.0);
                self.wheel_pixels -= position.y;
                let rows = (self.wheel_pixels / height).trunc();
                self.wheel_pixels -= rows * height;
                rows as isize
            }
        };
        if rows != 0 && self.editor.scroll_view(rows) {
            self.request_redraw();
        }
    }
}
