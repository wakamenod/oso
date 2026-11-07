//! The event loop's handler: sends each event from the window and from other
//! threads to the part of the app that deals with it.
//!
//! Must not depend on: what a key does or how a frame is drawn. A new event adds
//! an arm here, and its handling goes in a module beside this one.

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{App, UserEvent};

/// Whether `event` keeps what the app knows of the window true, rather than being input or a
/// frame. Missing a change of modifiers would leave Control held after a leave is taken back,
/// and every letter would then run a command.
fn keeps_state_true(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::ModifiersChanged(_)
            | WindowEvent::Resized(_)
            | WindowEvent::ScaleFactorChanged { .. }
    )
}

impl ApplicationHandler<UserEvent> for App {
    /// Called when the OS signals that the window is ready to be created (or resumed).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.open_window(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
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
            #[cfg(feature = "dev")]
            UserEvent::Control(request) => self.handle_control(event_loop, request),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.check_leaving(event_loop);
        self.ask_for_a_frame_if_status_changed();
    }

    /// The event loop is ending, on every exit. On macOS `applicationWillTerminate` leads
    /// here for a Dock quit or a logout.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.ending();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // An exit only sets a flag, and winit still delivers what is queued. Input
        // handled now, by any path, would be lost with the process. But a leave can be
        // taken back, so what the window's state depends on still counts.
        if self.view.is_none() || (self.is_leaving(event_loop) && !keeps_state_true(&event)) {
            return;
        }

        match event {
            // ---- Close window ----
            WindowEvent::CloseRequested => self.close_requested(event_loop),

            // ---- Resize ----
            WindowEvent::Resized(size) => self.resize(size.width, size.height),

            // ---- Moved to a display with another scale: reload the font ----
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => self.set_scale(scale_factor),

            // ---- Visible again: frames skipped while occluded need a redraw ----
            WindowEvent::Occluded(false) => self.request_redraw(),

            // ---- Redraw ----
            WindowEvent::RedrawRequested => self.draw_frame(),

            // ---- Mouse wheel and trackpad ----
            WindowEvent::MouseWheel { delta, .. } => self.handle_wheel(delta),

            // ---- Keyboard input ----
            WindowEvent::ModifiersChanged(modifiers) => self.set_modifiers(modifiers.state()),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => self.handle_key_event(event_loop, &event, is_synthetic),

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalSize;
    use winit::event::{Modifiers, WindowEvent};

    use super::keeps_state_true;

    #[test]
    fn only_what_keeps_the_windows_state_true_passes_while_leaving() {
        assert!(keeps_state_true(&WindowEvent::ModifiersChanged(
            Modifiers::default()
        )));
        assert!(keeps_state_true(&WindowEvent::Resized(PhysicalSize::new(
            1, 1
        ))));
        assert!(!keeps_state_true(&WindowEvent::RedrawRequested));
        assert!(!keeps_state_true(&WindowEvent::CloseRequested));
        assert!(!keeps_state_true(&WindowEvent::Focused(true)));
    }
}
