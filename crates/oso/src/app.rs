use std::sync::Arc;

use oso_core::cursor::Cursor;
use oso_render::{Redraw, RenderState};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Top-level application state.
///
/// - `view`: the window and what draws into it.
/// - `cursor`: logical cursor position in the editor (data layer).
#[derive(Default)]
pub struct App {
    view: Option<View>,
    cursor: Cursor,
}

/// The window and its renderer. The tip crate owns the window; `oso-render`
/// only draws into it and asks for redraws through [`Redraw`].
struct View {
    window: Arc<Window>,
    render_state: RenderState,
}

impl View {
    /// Requests a redraw if the renderer asked for one.
    fn redraw_if(&self, redraw: Redraw) {
        if redraw == Redraw::Needed {
            self.window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    /// Called when the OS signals that the window is ready to be created (or resumed).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_none() {
            let window_attributes = Window::default_attributes().with_title("oso Editor");
            let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
            let size = window.inner_size();
            let render_state =
                pollster::block_on(RenderState::new(window.clone(), size.width, size.height));
            // Under `ControlFlow::Wait` nothing is drawn until requested, and
            // not every platform sends an initial resize or redraw.
            window.request_redraw();
            self.view = Some(View {
                window,
                render_state,
            });
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
            }

            // ---- Visible again: frames skipped while occluded need a redraw ----
            WindowEvent::Occluded(false) => {
                view.window.request_redraw();
            }

            // ---- Redraw ----
            WindowEvent::RedrawRequested => {
                let size = view.window.inner_size();
                match view
                    .render_state
                    .render(&self.cursor, size.width, size.height)
                {
                    Ok(redraw) => view.redraw_if(redraw),
                    // Not fatal: the frame is skipped and the next one retries.
                    Err(e) => log::error!("render error: {e}"),
                }
            }

            // ---- Keyboard input ----
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                // Process key-press events only.
                if key_event.state != ElementState::Pressed {
                    return;
                }
                let mut changed = false;

                match key_event.physical_key {
                    // Right arrow: delegate movement logic to Cursor.
                    PhysicalKey::Code(KeyCode::ArrowRight) => {
                        self.cursor.move_right();
                        changed = true;
                    }
                    // Left arrow: delegate movement logic to Cursor.
                    PhysicalKey::Code(KeyCode::ArrowLeft) => {
                        self.cursor.move_left();
                        changed = true;
                    }
                    _ => {}
                }

                if changed {
                    view.window.request_redraw();
                }
            }

            _ => {}
        }
    }
}
