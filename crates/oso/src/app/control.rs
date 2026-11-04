//! Answering control socket requests on the UI thread, in dev builds
//! (ADR 0023, ADR 0044, ADR 0047).
//!
//! Must not depend on: sockets or threads, which stay behind `UiRequest`.
//! Plain functions of the editor state do not belong here either; this holds
//! what needs the window, the loads or the dispatcher.

use std::time::Instant;

use oso_core::editor::SCRATCH;
use serde_json::json;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;

use super::{App, text_view};
use crate::dev;
use crate::dev::handoff;

impl App {
    /// Answers a control socket request on the UI thread.
    pub(super) fn handle_control(&mut self, event_loop: &ActiveEventLoop, request: dev::UiRequest) {
        let dev::UiRequest { op, reply } = request;
        let result = match op {
            dev::UiOp::SendKeys(keys) => {
                let count = keys.len();
                let started = Instant::now();
                let effects = self.dispatcher.handle_keys(&mut self.editor, keys);
                log::trace!("key handled in {:?}", started.elapsed());
                self.apply(event_loop, effects);
                Ok(json!({ "keys": count }))
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
                result.map(|()| json!({ "command": name }))
            }
            dev::UiOp::ShowBuffer(buffer) => {
                let result = dev::show_buffer(&mut self.editor, buffer);
                if result.is_ok() {
                    self.request_redraw();
                }
                result
            }
            dev::UiOp::Screenshot => {
                self.take_screenshot(reply);
                return;
            }
            dev::UiOp::ExportState => handoff::export(
                &self.editor,
                self.dispatcher.pending(),
                self.frame(),
                self.restore.as_ref(),
            ),
            dev::UiOp::ImportState(state) => self.import_state(*state),
            dev::UiOp::Exit => {
                event_loop.exit();
                Ok(json!({ "exiting": true }))
            }
        };
        let _ = reply.send(result);
    }

    /// Submits a capture of the frame. A background thread finishes it and
    /// replies, unless it is refused here.
    fn take_screenshot(&mut self, reply: std::sync::mpsc::Sender<dev::Reply>) {
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
    }

    /// The OS window's place and size, if there is a window.
    fn frame(&self) -> Option<handoff::Frame> {
        let window = &self.view.as_ref()?.window;
        let size = window.inner_size();
        Some(handoff::Frame {
            position: window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            width: size.width,
            height: size.height,
        })
    }

    /// Takes over from the instance that exported `state`: opens its files
    /// again, each judged anew, and moves the window. A reply means every file
    /// has begun to load, which is what lets the old instance exit. Refused
    /// without changing anything if this instance has opened something already.
    fn import_state(&mut self, state: handoff::State) -> dev::Reply {
        if self.editor.buffers().count() > 1 || self.editor.current().1.name() != SCRATCH {
            return Err("this instance has already opened something".into());
        }
        let scratch = self.editor.window().buffer();
        let mut shown = scratch;
        for (index, path) in state.buffers.into_iter().enumerate() {
            let buffer = match path {
                Some(path) => self
                    .visit(path)
                    .ok_or("a file could not begin to load, so the old instance should stay")?,
                None => scratch,
            };
            if index == state.shown {
                shown = buffer;
            }
        }
        self.editor.show(shown);
        // The size first, so that positions are restored against it where the
        // window system applies it at once. Elsewhere the view can still move
        // to keep the cursor in sight when the resize arrives.
        if let (Some(view), Some(frame)) = (&mut self.view, state.frame) {
            if let Some((x, y)) = frame.position {
                view.window.set_outer_position(PhysicalPosition::new(x, y));
            }
            let size = PhysicalSize::new(frame.width, frame.height);
            if let Some(size) = view.window.request_inner_size(size) {
                let redraw = view.render_state.resize(size.width, size.height);
                view.redraw_if(redraw);
                self.update_view_size();
            }
        }
        self.restore = Some(handoff::Restore {
            buffer: shown,
            selections: state.selections,
            scroll: state.scroll,
        });
        self.restore_positions();
        self.dispatcher.restore_pending(&state.pending_keys);
        self.request_redraw();
        Ok(json!({ "buffers": self.editor.buffers().count() }))
    }

    /// Applies positions from a handoff if their file has loaded far enough.
    /// Returns whether it did.
    pub(super) fn restore_positions(&mut self) -> bool {
        let done = self
            .restore
            .as_ref()
            .is_some_and(|restore| restore.try_apply(&mut self.editor));
        if done {
            self.restore = None;
        }
        done
    }
}
