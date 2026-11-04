//! The application: the window, the event loop handler and the glue
//! between them and the editor (ADR 0004). It turns winit events into key
//! presses, wheel scrolling and frames, starts file loads and takes in
//! their snapshots.
//!
//! Must not depend on: wgpu or OS bindings. Editor logic belongs in oso-core
//! and commands, not here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::window::{self, ViewSize};
use oso_render::{Redraw, RenderState, TextView};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

#[cfg(feature = "dev")]
use crate::dev;
use crate::dispatch::Dispatcher;
use crate::loader::{self, LoadError, LoadHandle, LoadUpdate};
use crate::{keys, text};

/// Events sent to the event loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// A loading thread has an update for this buffer.
    Load(BufferId),
    /// A control socket request, in dev builds.
    #[cfg(feature = "dev")]
    Control(dev::UiRequest),
}

/// Rows one notch of a mouse wheel scrolls.
const WHEEL_ROWS: f32 = 3.0;

/// Top-level application state.
///
/// - `view`: the window and what draws into it.
/// - `editor`: the editor state that commands operate on.
/// - `dispatcher`: keymaps and commands, which turn keys into changes.
/// - `modifiers`: the modifier keys held, as winit last reported them.
/// - `loads`: files loading in the background.
/// - `dev`: the control socket, in dev builds where it started.
pub struct App {
    view: Option<View>,
    editor: Editor,
    dispatcher: Dispatcher,
    modifiers: ModifiersState,
    proxy: EventLoopProxy<UserEvent>,
    loads: Vec<Load>,
    /// When the process started, until the first frame is presented.
    started: Option<Instant>,
    /// Trackpad scrolling not yet worth a whole row, in pixels.
    wheel_pixels: f64,
    #[cfg(feature = "dev")]
    dev: Option<dev::DevState>,
}

/// A file loading into a buffer.
struct Load {
    /// Holds the latest update; cancels the load when dropped.
    handle: LoadHandle,
    started: Instant,
    /// When its first content arrived, until that frame is drawn.
    first_screen: Option<Instant>,
}

impl App {
    /// An app with no window yet, showing `*scratch*`. Loading threads
    /// report through `proxy`.
    pub fn new(proxy: EventLoopProxy<UserEvent>, started: Instant) -> Self {
        Self {
            view: None,
            editor: Editor::default(),
            dispatcher: Dispatcher::new(),
            modifiers: ModifiersState::default(),
            proxy,
            loads: Vec::new(),
            started: Some(started),
            wheel_pixels: 0.0,
            #[cfg(feature = "dev")]
            dev: None,
        }
    }

    /// Answers control socket requests through `dev`.
    #[cfg(feature = "dev")]
    pub fn set_dev(&mut self, dev: Option<dev::DevState>) {
        self.dev = dev;
    }

    /// Visits `path` in the selected window: a buffer named after the file
    /// shows at once, read-only, and fills in as a background thread loads
    /// it. Nothing in the file is run (security.md, T1).
    pub fn open(&mut self, path: PathBuf) {
        // Absolute without touching the file system, so the UI never waits.
        let path = std::path::absolute(&path).unwrap_or(path);
        let buffer = self.editor.add_buffer(Buffer::visiting(path.clone()));
        self.editor.show(buffer);
        let proxy = self.proxy.clone();
        let spawned = loader::spawn(buffer, path, move |buffer| {
            proxy.send_event(UserEvent::Load(buffer)).is_ok()
        });
        match spawned {
            Ok(handle) => self.loads.push(Load {
                handle,
                started: Instant::now(),
                first_screen: None,
            }),
            Err(e) => {
                log::warn!("cannot start loading: {e}");
                self.editor.kill_buffer(buffer);
            }
        }
        self.request_redraw();
    }

    /// Handles one key press from the window, through the keymaps and the
    /// command registry. Keys from the dev control socket take the same
    /// path, through [`Dispatcher::handle_keys`].
    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: KeyPress) {
        let started = Instant::now();
        let effects = self.dispatcher.handle_key(&mut self.editor, key);
        log::trace!("key handled in {:?}", started.elapsed());
        self.apply(event_loop, effects);
    }

    /// Does what commands asked for.
    fn apply(&self, event_loop: &ActiveEventLoop, effects: Effects) {
        if effects.redraw {
            self.request_redraw();
        }
        if effects.exit {
            event_loop.exit();
        }
    }

    fn request_redraw(&self) {
        if let Some(view) = &self.view {
            view.window.request_redraw();
        }
    }

    /// Takes in the latest update of every load, before a frame, so that
    /// the frame shows what has loaded by then.
    fn poll_loads(&mut self) {
        for index in (0..self.loads.len()).rev() {
            if let Some(update) = self.loads[index].handle.take() {
                self.apply_load(index, update);
            }
        }
    }

    /// Applies one update of `self.loads[index]`. Returns whether a frame
    /// is needed: only if the window shows the buffer and reaches the part
    /// that changed, or the buffer was replaced.
    fn apply_load(&mut self, index: usize, update: LoadUpdate) -> bool {
        let id = self.loads[index].handle.buffer();
        let shown = self.editor.window().buffer() == id;
        let Some(buffer) = self.editor.buffer_mut(id) else {
            self.loads.swap_remove(index);
            return false;
        };
        if let Some(resolved) = self.loads[index].handle.take_resolved() {
            buffer.judge_secret(resolved.paths());
        }
        match update {
            LoadUpdate::Snapshot(snapshot) => {
                let old_rows = buffer.display().row_count();
                let first = buffer.text().is_empty();
                let done = snapshot.done();
                buffer.apply_load(*snapshot);
                let load = &mut self.loads[index];
                if first {
                    load.first_screen = Some(load.started);
                }
                if done {
                    log::debug!("{} loaded in {:?}", buffer.name(), load.started.elapsed());
                }
                let window = self.editor.window();
                shown && window::reaches(window.scroll.top, window.size(), old_rows - 1)
            }
            LoadUpdate::Failed(LoadError::NotFound) => {
                // As Emacs does: a new file, empty until saved.
                log::info!("(New file) {}", buffer.name());
                buffer.finish_empty();
                self.loads.swap_remove(index);
                shown
            }
            LoadUpdate::Failed(e) => {
                // The echo area shows this from M2.
                log::warn!("cannot open {}: {e}", buffer.name());
                self.editor.kill_buffer(id);
                self.loads.swap_remove(index);
                shown
            }
        }
    }

    /// Logs how long the first screen of each new file took, once drawn,
    /// and forgets loads that finished.
    fn note_first_screens(&mut self) {
        let editor = &self.editor;
        self.loads.retain_mut(|load| {
            if let Some(started) = load.first_screen.take() {
                log::debug!(
                    "first screen drawn {:?} after loading started",
                    started.elapsed()
                );
            }
            editor
                .buffer(load.handle.buffer())
                .is_some_and(|buffer| buffer.loading().is_some())
        });
    }

    /// Scrolls the view for the mouse wheel or the trackpad.
    fn handle_wheel(&mut self, delta: MouseScrollDelta) {
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

    /// Tells the editor how many rows and columns the window shows.
    fn update_view_size(&mut self) {
        if let Some(view) = &self.view {
            let (rows, columns) = view.render_state.text_area();
            self.editor.set_view_size(ViewSize::new(rows, columns));
        }
    }
}

/// What the selected window shows, for the renderer.
fn text_view(editor: &Editor) -> TextView {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let cursor = display.offset_to_display_point(window.selections.primary().cursor);
    TextView {
        display,
        buffer: window.buffer(),
        revision: buffer.revision(),
        scroll: window.scroll,
        cursor,
    }
}

/// The window and its renderer. The tip crate owns the window; `oso-render`
/// only draws into it and asks for redraws through [`Redraw`].
struct View {
    window: Arc<Window>,
    render_state: RenderState,
    /// When the window was requested, until the first frame is presented.
    started: Option<Instant>,
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
    fn handle_control(&mut self, event_loop: &ActiveEventLoop, request: dev::UiRequest) {
        let dev::UiRequest { op, reply } = request;
        let result = match op {
            dev::UiOp::SendKeys(keys) => {
                let count = keys.len();
                let started = Instant::now();
                let effects = self.dispatcher.handle_keys(&mut self.editor, keys);
                log::trace!("key handled in {:?}", started.elapsed());
                self.apply(event_loop, effects);
                Ok(serde_json::json!({ "keys": count }))
            }
            dev::UiOp::GetState => {
                let surface = self.view.as_ref().map(|v| v.render_state.surface_size());
                Ok(dev::get_state(
                    &self.editor,
                    self.dispatcher.pending(),
                    surface,
                ))
            }
            dev::UiOp::GetText { start_row, rows } => {
                // Only the check happens here; a background thread copies
                // the text and replies.
                match dev::get_text(&self.editor, start_row, rows) {
                    Ok(job) => {
                        job.spawn(reply);
                        return;
                    }
                    Err(message) => Err(message),
                }
            }
            dev::UiOp::RunCommand { name, arg } => {
                let (effects, result) = self.dispatcher.run_command(&mut self.editor, &name, arg);
                self.apply(event_loop, effects);
                result.map(|()| serde_json::json!({ "command": name }))
            }
            dev::UiOp::ShowBuffer(buffer) => {
                let result = dev::show_buffer(&mut self.editor, buffer);
                if result.is_ok() {
                    self.request_redraw();
                }
                result
            }
            dev::UiOp::Screenshot => {
                if let Err(message) = dev::screenshot_allowed(&self.editor) {
                    let _ = reply.send(Err(message));
                    return;
                }
                // Only the GPU submission happens here; the rest runs on a
                // background thread, which replies.
                let (Some(view), Some(dev)) = (self.view.as_mut(), self.dev.as_mut()) else {
                    let _ = reply.send(Err("there is no window yet".into()));
                    return;
                };
                let started = Instant::now();
                let capture = view.render_state.capture(&text_view(&self.editor));
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
            if let Some(started) = self.started {
                log::debug!("resumed {:?} after start", started.elapsed());
            }
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
            self.view = Some(View {
                window,
                render_state,
                started: Some(started),
            });
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
            WindowEvent::RedrawRequested => {
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
                match view.render_state.render(&text, &window_size) {
                    Ok(redraw) => {
                        view.redraw_if(redraw);
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

            // ---- Mouse wheel and trackpad ----
            WindowEvent::MouseWheel { delta, .. } => self.handle_wheel(delta),

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
                    self.handle_key(event_loop, key);
                }
            }

            _ => {}
        }
    }
}
