use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::editor::cursor::Cursor;
use crate::renderer::RenderState;

/// Top-level application state.
///
/// - `render_state`: wgpu surface, device, and rendering pipeline.
/// - `cursor`: logical cursor position in the editor (data layer).
#[derive(Default)]
pub struct App {
    render_state: Option<RenderState>,
    cursor: Cursor,
}

impl ApplicationHandler for App {
    /// Called when the OS signals that the window is ready to be created (or resumed).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.render_state.is_none() {
            let window_attributes = Window::default_attributes().with_title("oso Editor");
            let window = event_loop.create_window(window_attributes).unwrap();
            let state = pollster::block_on(RenderState::new(window));
            // Under `ControlFlow::Wait` nothing is drawn until requested, and
            // not every platform sends an initial resize or redraw.
            state.window.request_redraw();
            self.render_state = Some(state);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let state = match self.render_state.as_mut() {
            Some(s) => s,
            None => return,
        };

        match event {
            // ---- Close window ----
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            // ---- Resize ----
            WindowEvent::Resized(physical_size) => {
                state.resize(physical_size);
            }

            // ---- Visible again: frames skipped while occluded need a redraw ----
            WindowEvent::Occluded(false) => {
                state.window.request_redraw();
            }

            // ---- Redraw ----
            WindowEvent::RedrawRequested => {
                match state.render(&self.cursor) {
                    Ok(_) => {}
                    // Reconfigure if the surface is lost or outdated.
                    Err(e) if e.contains("Lost") || e.contains("Outdated") => {
                        let size = state.window.inner_size();
                        state.resize(size);
                    }
                    Err(e) => eprintln!("render error: {e}"),
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
                    state.window.request_redraw();
                }
            }

            _ => {}
        }
    }
}
