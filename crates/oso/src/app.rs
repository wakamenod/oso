use std::sync::Arc;

use oso_core::cursor::Cursor;
use oso_core::key::{Key, KeyPress, NamedKey};
use oso_render::{Redraw, RenderState};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

#[cfg(feature = "dev")]
use crate::dev;
use crate::keys;

/// Events sent to the event loop from other threads: control socket
/// requests in dev builds, nothing otherwise.
#[cfg(feature = "dev")]
pub type UserEvent = dev::UiRequest;
/// Events sent to the event loop from other threads: control socket
/// requests in dev builds, nothing otherwise.
#[cfg(not(feature = "dev"))]
pub type UserEvent = ();

/// Top-level application state.
///
/// - `view`: the window and what draws into it.
/// - `cursor`: logical cursor position in the editor (data layer).
/// - `modifiers`: the modifier keys held, as winit last reported them.
/// - `dev`: the control socket, in dev builds where it started.
#[derive(Default)]
pub struct App {
    view: Option<View>,
    cursor: Cursor,
    modifiers: ModifiersState,
    #[cfg(feature = "dev")]
    dev: Option<dev::DevState>,
}

impl App {
    /// An app that answers control socket requests through `dev`.
    #[cfg(feature = "dev")]
    pub fn with_dev(dev: Option<dev::DevState>) -> Self {
        Self {
            dev,
            ..Self::default()
        }
    }

    /// Handles one key press, from the window or from the dev control
    /// socket.
    ///
    /// Until the keymaps of M1-3, only the arrow keys do anything, and
    /// modifiers are ignored.
    fn handle_key(&mut self, key: KeyPress) {
        match key.key {
            Key::Named(NamedKey::Right) => self.cursor.move_right(),
            Key::Named(NamedKey::Left) => self.cursor.move_left(),
            _ => return,
        }
        if let Some(view) = &self.view {
            view.window.request_redraw();
        }
    }
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

#[cfg(feature = "dev")]
impl App {
    /// Answers a control socket request on the UI thread.
    fn handle_control(&mut self, request: dev::UiRequest) {
        let dev::UiRequest { op, reply } = request;
        let result = match op {
            dev::UiOp::SendKeys(keys) => {
                let count = keys.len();
                for key in keys {
                    self.handle_key(key);
                }
                Ok(serde_json::json!({ "keys": count }))
            }
            dev::UiOp::GetState => {
                let surface = self.view.as_ref().map(|v| v.render_state.surface_size());
                Ok(dev::state(&self.cursor, surface))
            }
            dev::UiOp::Screenshot => {
                // Only the GPU submission happens here; the rest runs on a
                // background thread, which replies.
                let (Some(view), Some(dev)) = (self.view.as_mut(), self.dev.as_mut()) else {
                    let _ = reply.send(Err("there is no window yet".into()));
                    return;
                };
                let started = std::time::Instant::now();
                let capture = view.render_state.capture(&self.cursor);
                log::debug!(
                    "screenshot submitted on the UI thread in {:?}",
                    started.elapsed()
                );
                match capture {
                    Ok(capture) => dev.finish_screenshot(capture, reply),
                    Err(e) => {
                        let _ = reply.send(Err(e.to_string()));
                    }
                }
                return;
            }
        };
        let _ = reply.send(result);
    }
}

impl ApplicationHandler<UserEvent> for App {
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

    #[cfg(feature = "dev")]
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, request: UserEvent) {
        self.handle_control(request);
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
                let window = &view.window;
                let window_size = || {
                    let size = window.inner_size();
                    (size.width, size.height)
                };
                match view.render_state.render(&self.cursor, &window_size) {
                    Ok(redraw) => view.redraw_if(redraw),
                    // Not fatal: the frame is skipped and the next one retries.
                    Err(e) => log::error!("render error: {e}"),
                }
            }

            // ---- Keyboard input ----
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                // Process key-press events only.
                if key_event.state != ElementState::Pressed {
                    return;
                }
                if let Some(key) = keys::from_event(&key_event, self.modifiers) {
                    self.handle_key(key);
                }
            }

            _ => {}
        }
    }
}
