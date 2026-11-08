//! The event loop's handler: sends each event from the window and from other
//! threads to the part of the app that deals with it. Events come in oso's own
//! types, `oso_platform::window`'s, from winit's adapter or the native layer (ADR 0070).
//! Each callback of the loop goes to one method of the app, here or in the concern it
//! belongs to (`open_window`, `ending`), so a window layer only translates.
//!
//! Must not depend on: what a key does or how a frame is drawn. A new event adds
//! an arm here, and its handling goes in a module beside this one.

use oso_platform::window::Event;

use super::shell::Loop;
use super::{App, UserEvent};

/// Whether `event` keeps what the app knows of the window true, rather than being input or a
/// frame. Missing a change of modifiers would leave Control held after a leave is taken back,
/// and every letter would then run a command.
fn keeps_state_true(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::ModifiersChanged(_) | Event::Resized { .. } | Event::ScaleFactorChanged(_)
    )
}

impl App {
    /// A thread sent the event loop something.
    pub(super) fn handle_user_event(&mut self, event_loop: &Loop<'_>, event: UserEvent) {
        match event {
            UserEvent::Load(buffer) => {
                self.load_updated(buffer);
            }
            UserEvent::Save(report) => self.save_ended(event_loop, report),
            UserEvent::NoEmergencyCopies(problem) => {
                // Only this level reaches a terminal with no `RUST_LOG`.
                log::error!("{}", problem.escape_debug());
                self.editor.message(problem);
            }
            UserEvent::Copies(event) => self.copies_event(event),
            UserEvent::Clipboard(event) => self.clipboard_event(event_loop, event),
            UserEvent::Search(report) => self.search_reported(report),
            #[cfg(feature = "dev")]
            UserEvent::Control(request) => self.handle_control(event_loop, request),
        }
        self.after_event(event_loop);
    }

    /// The event loop is about to wait for the next events.
    pub(super) fn before_waiting(&mut self, event_loop: &Loop<'_>) {
        self.check_leaving(event_loop);
        self.scan_if_a_file_has_loaded();
        // A load may have grown the buffer under isearch.
        self.start_searches();
        self.ask_for_a_frame_if_status_changed();
    }

    /// An event from the window.
    pub(super) fn handle_window_event(&mut self, event_loop: &Loop<'_>, event: Event<'_>) {
        // An exit only sets a flag, and the loop still delivers what is queued. Input
        // handled now, by any path, would be lost with the process. But a leave can be
        // taken back, so what the window's state depends on still counts.
        if self.view.is_none() || (self.is_leaving(event_loop) && !keeps_state_true(&event)) {
            return;
        }

        match event {
            // ---- Close window ----
            Event::CloseRequested => self.close_requested(event_loop),

            // ---- Resize ----
            Event::Resized { width, height } => self.resize(width, height),

            // ---- Moved to a display with another scale: reload the font ----
            Event::ScaleFactorChanged(scale_factor) => self.set_scale(scale_factor),

            // ---- Visible again: frames skipped while occluded need a redraw ----
            Event::Occluded(false) => self.request_redraw(),

            // ---- Redraw ----
            Event::RedrawRequested => self.draw_frame(),

            // ---- Mouse wheel and trackpad ----
            Event::Wheel(wheel) => self.handle_wheel(wheel),

            // ---- Keyboard input ----
            Event::ModifiersChanged(held) => self.set_modifiers(held),
            Event::Key(input) => self.handle_key_event(event_loop, &input),

            // ---- Input method ----
            Event::Ime(ime) => {
                self.ime_event(event_loop, ime);
            }

            Event::Occluded(true) | Event::Other => {}
        }
        self.after_event(event_loop);
    }
}

#[cfg(test)]
mod tests {
    use super::keeps_state_true;
    use oso_platform::window::{Event, Held};

    #[test]
    fn only_what_keeps_the_windows_state_true_passes_while_leaving() {
        assert!(keeps_state_true(&Event::ModifiersChanged(Held::NONE)));
        assert!(keeps_state_true(&Event::Resized {
            width: 1,
            height: 1
        }));
        assert!(keeps_state_true(&Event::ScaleFactorChanged(2.0)));
        assert!(!keeps_state_true(&Event::RedrawRequested));
        assert!(!keeps_state_true(&Event::CloseRequested));
        assert!(!keeps_state_true(&Event::Other), "focus, among others");
    }
}
