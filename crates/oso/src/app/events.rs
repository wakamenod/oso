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
use crate::text;

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
                let load = self.loads.iter().position(|l| l.handle.buffer() == buffer);
                if let Some(index) = load
                    && let Some(update) = self.loads[index].handle.take()
                    && self.apply_load(index, update)
                {
                    self.request_redraw();
                }
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
        let Some(view) = self.view.as_mut() else {
            return;
        };

        match event {
            // ---- Close window ----
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            // ---- Resize ----
            WindowEvent::Resized(physical_size) => {
                let redraw = view
                    .render_state
                    .resize(physical_size.width, physical_size.height);
                view.redraw_if(redraw);
                self.update_view_size();
            }

            // ---- Moved to a display with another scale: reload the font ----
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let redraw = view.render_state.set_text_system(text::load(scale_factor));
                view.redraw_if(redraw);
                self.update_view_size();
            }

            // ---- Visible again: frames skipped while occluded need a redraw ----
            WindowEvent::Occluded(false) => {
                view.window.request_redraw();
            }

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
