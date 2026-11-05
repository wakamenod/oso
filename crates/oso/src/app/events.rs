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
            #[cfg(feature = "dev")]
            UserEvent::Control(request) => self.handle_control(event_loop, request),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.check_leaving(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // An exit only sets a flag, and winit still delivers what is queued. Input
        // handled now, by any path, would be lost with the process, and nothing
        // else here matters to a window about to go.
        if self.view.is_none() || self.is_leaving(event_loop) {
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
