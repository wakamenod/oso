//! Drawing: the window and its renderer, and what a frame shows.
//!
//! Must not depend on: key presses or commands. A frame shows the editor's state
//! and does not decide it.

use std::sync::Arc;
use std::time::Instant;

use oso_core::editor::Editor;
use oso_core::window::ViewSize;
use oso_render::{Redraw, RenderState, TextView};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use super::App;
use crate::text;

/// What the selected window shows, for the renderer.
pub(super) fn text_view(editor: &Editor) -> TextView {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let cursor = window.cursor_point(&display);
    TextView {
        display,
        buffer: window.buffer(),
        revision: buffer.revision(),
        scroll: window.scroll(),
        cursor,
    }
}

/// The window and its renderer. The tip crate owns the window; `oso-render`
/// only draws into it and asks for redraws through [`Redraw`].
pub(super) struct View {
    pub(super) window: Arc<Window>,
    pub(super) render_state: RenderState,
    /// When the window was requested, until the first frame is presented.
    started: Option<Instant>,
}

impl View {
    /// Opens the window and sets up its renderer.
    pub(super) fn open(event_loop: &ActiveEventLoop) -> Self {
        let started = Instant::now();
        let window_attributes = Window::default_attributes().with_title("oso Editor");
        let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
        let size = window.inner_size();
        let text_system = text::load(window.scale_factor());
        let render_state = pollster::block_on(RenderState::new(
            window.clone(),
            size.width,
            size.height,
            text_system,
        ));
        // Under `ControlFlow::Wait` nothing is drawn until requested, and
        // not every platform sends an initial resize or redraw.
        window.request_redraw();
        Self {
            window,
            render_state,
            started: Some(started),
        }
    }

    /// Requests a redraw if the renderer asked for one.
    pub(super) fn redraw_if(&self, redraw: Redraw) {
        if redraw == Redraw::Needed {
            self.window.request_redraw();
        }
    }
}

impl App {
    pub(super) fn request_redraw(&self) {
        if let Some(view) = &self.view {
            view.window.request_redraw();
        }
    }

    /// Tells the editor how many rows and columns the window shows.
    pub(super) fn update_view_size(&mut self) {
        if let Some(view) = &self.view {
            let (rows, columns) = view.render_state.text_area();
            self.editor.set_view_size(ViewSize::new(rows, columns));
        }
    }

    /// Draws a frame of what the editor shows, with what has loaded by now.
    pub(super) fn draw_frame(&mut self) {
        let frame = Instant::now();
        self.poll_loads();
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let window = &view.window;
        let window_size = || {
            let size = window.inner_size();
            (size.width, size.height)
        };
        let text = text_view(&self.editor);
        #[cfg(feature = "dev")]
        let presented = view.render_state.presented_frames();
        match view.render_state.render(&text, &window_size) {
            Ok(redraw) => {
                view.redraw_if(redraw);
                #[cfg(feature = "dev")]
                if view.render_state.presented_frames() > presented {
                    let complete = redraw == Redraw::NotNeeded;
                    self.timings.frame_presented(complete);
                }
                // A frame skipped while the window is hidden draws
                // nothing yet.
                if view.render_state.presented_frames() == 0 {
                    return;
                }
                if let Some(started) = view.started.take() {
                    log::debug!("first frame {:?} after the window", started.elapsed());
                }
                if let Some(started) = self.started.take() {
                    log::debug!("first frame {:?} after start", started.elapsed());
                }
                log::trace!("frame drawn in {:?}", frame.elapsed());
                self.note_first_screens();
            }
            // Not fatal: the frame is skipped and the next one retries.
            Err(e) => log::error!("render error: {e}"),
        }
    }
}
