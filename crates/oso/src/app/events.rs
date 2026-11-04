//! The event loop's handler: sends each event from the window and from other
//! threads to the part of the app that deals with it.
//!
//! Must not depend on: what a key does or how a frame is drawn. A new event adds
//! an arm here, and its handling goes in a module beside this one.

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::draw::View;
use super::{App, UserEvent};

impl ApplicationHandler<UserEvent> for App {
    /// Called when the OS signals that the window is ready to be created (or resumed).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_none() {
            if let Some(started) = self.started {
                log::debug!("resumed {:?} after start", started.elapsed());
            }
            self.view = Some(View::open(event_loop));
            self.update_view_size();
            // Content that loaded while the window opened shows in the
            // first frame.
            self.poll_loads();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let _ = event_loop;
        match event {
            UserEvent::Load(buffer) => {
                self.load_updated(buffer);
            }
            #[cfg(feature = "dev")]
            UserEvent::Control(request) => self.handle_control(event_loop, request),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.view.is_none() {
            return;
        }

        match event {
            // ---- Close window ----
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

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
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => self.handle_key_event(event_loop, &event),

            _ => {}
        }
    }
}
