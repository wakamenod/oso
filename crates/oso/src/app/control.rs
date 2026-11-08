//! Answering control socket requests on the UI thread, in dev builds
//! (ADR 0023, ADR 0044, ADR 0047).
//!
//! Must not depend on: sockets or threads, which stay behind `UiRequest`.
//! Plain functions of the editor state do not belong here either; this holds
//! what needs the window, the loads or the dispatcher.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;

use oso_core::editor::Editor;
use oso_core::minibuffer::PromptId;

use super::App;
use super::draw::with_panes;
use super::ime::ImeEvent;
use super::shell::Loop;
use crate::dev;
use crate::dev::handoff;

impl App {
    /// Answers a control socket request on the UI thread.
    pub(super) fn handle_control(&mut self, event_loop: &Loop<'_>, request: dev::UiRequest) {
        let dev::UiRequest {
            op,
            received,
            reply,
        } = request;
        // `exit` only sets a flag, and the loop still delivers what is queued. An
        // edit run now would be confirmed and then lost with the process.
        if self.is_leaving(event_loop) {
            let _ = reply.send(Err("the editor is shutting down".into()));
            return;
        }
        let newest = self.editor.prompts().last().map(|prompt| prompt.id());
        let result = match op {
            dev::UiOp::SendKeys(keys) => {
                let count = keys.len();
                let started = Instant::now();
                let timings = &mut self.timings;
                let mut watch = dev::SecretWatch::new(&self.editor);
                let effects =
                    self.dispatcher
                        .handle_keys_with(&mut self.editor, keys, |editor, effects| {
                            let key = dev::HandledKey {
                                redraw: effects.redraw,
                                withheld: watch.after_key(editor),
                            };
                            timings.key_handled(received, key);
                        });
                log::trace!("key handled in {:?}", started.elapsed());
                self.apply(event_loop, effects);
                Ok(json!({ "keys": count }))
            }
            dev::UiOp::OpenFile(path) => self.open_file(path),
            dev::UiOp::TakeTimings => Ok(self.take_timings()),
            dev::UiOp::GetState => Ok(self.get_state()),
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
            dev::UiOp::CheckHandover => handoff::check(&self.editor),
            dev::UiOp::Panic | dev::UiOp::EndUnasked => self.end_for_a_test(event_loop, &op),
            dev::UiOp::Ime(report) => self.report_ime(event_loop, report, received),
            dev::UiOp::ImportState(state) => self.import_state(*state),
            // Checked here on the UI thread, and nothing runs after it (see the
            // top), so no edit can come between the check and the exit.
            dev::UiOp::Exit { generation } => {
                handoff::exit_allowed(&self.editor, generation).map(|()| {
                    self.leave(event_loop, true);
                    json!({ "exiting": true })
                })
            }
        };
        self.note_remote_questions(newest);
        let _ = reply.send(result);
    }

    /// Remembers the questions a request opened, those newer than `newest` was before it, so
    /// that keys from the window cannot answer them: a person typing does not see a question
    /// an agent asked appear under their fingers (ADR 0072). The window's `C-g` still closes
    /// one.
    fn note_remote_questions(&mut self, newest: Option<PromptId>) {
        self.remote_questions
            .extend(opened_questions(&self.editor, newest));
        let prompts = self.editor.prompts();
        self.remote_questions
            .retain(|id| prompts.iter().any(|prompt| prompt.id() == *id));
    }

    /// Ends the process as the emergency save's flows need (ADR 0062): by a panic outside the
    /// command boundary, whose message quotes the text, or with nobody having agreed to it.
    fn end_for_a_test(&self, event_loop: &Loop<'_>, op: &dev::UiOp) -> dev::Reply {
        if *op == dev::UiOp::Panic {
            panic!("a deliberate panic: {}", self.editor.current().1.text());
        }
        event_loop.exit();
        Ok(json!({ "ending": true }))
    }

    /// The result of `get-state`, with what the app has told the OS of the input method.
    fn get_state(&self) -> serde_json::Value {
        let surface = self.view.as_ref().map(|v| v.render_state.surface_size());
        let mut state = dev::get_state(&self.editor, self.dispatcher.pending(), surface);
        let facts = dev::ImeFacts {
            wanted: self.ime_wanted(),
            allowed: self.ime_allowed(),
            area: self.ime_area(),
        };
        state["ime"] = dev::ime_state(&self.editor, &facts);
        state
    }

    /// The result of `ime`: the report goes where the OS's would, through the same
    /// handler, and is timed as a key (ADR 0069). While oso has the IME off it is
    /// refused, as the OS would send nothing.
    fn report_ime(
        &mut self,
        event_loop: &Loop<'_>,
        report: dev::ImeReport,
        received: Instant,
    ) -> dev::Reply {
        if !self.ime_allowed() {
            return Err("the input method is off".into());
        }
        let mut watch = dev::SecretWatch::new(&self.editor);
        let cleared = || ImeEvent::Preedit(String::new(), None);
        let events = match report {
            dev::ImeReport::Preedit { text, range } => vec![ImeEvent::Preedit(text, range)],
            // As the OS does, an empty preedit first.
            dev::ImeReport::Commit(text) => vec![cleared(), ImeEvent::Commit(text)],
            dev::ImeReport::Clear => vec![cleared()],
            dev::ImeReport::Disabled => vec![ImeEvent::Disabled],
        };
        let mut redraw = false;
        for event in events {
            redraw |= self.ime_event(event_loop, event);
        }
        let key = dev::HandledKey {
            redraw,
            withheld: watch.after_key(&self.editor),
        };
        self.timings.key_handled(received, key);
        Ok(json!({ "ime": true }))
    }

    /// The result of `open-file`: visits a file in the selected window.
    fn open_file(&mut self, path: PathBuf) -> dev::Reply {
        let visited = self.visit(path);
        let result = visited
            .map(|buffer| {
                self.editor.show(buffer);
                json!({ "opened": true })
            })
            .ok_or_else(|| "cannot start loading; see the log".to_owned());
        self.request_redraw();
        result
    }

    /// The result of `take-timings`: the samples, and the GPU they ran on.
    fn take_timings(&mut self) -> serde_json::Value {
        let mut timings = self.timings.take();
        let gpu = self.view.as_ref().map(|v| v.render_state.gpu_info());
        timings["gpu"] = gpu.map_or(
            serde_json::Value::Null,
            |gpu| json!({ "name": gpu.name, "kind": gpu.kind }),
        );
        timings
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
        let mut status = oso_core::editor::Status::default();
        self.editor.status_into(&mut status);
        let render_state = &mut view.render_state;
        let capture = with_panes(&self.editor, &status, |panes| render_state.capture(panes));
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
        let (width, height) = window.inner_size();
        Some(handoff::Frame {
            position: window.outer_position(),
            width,
            height,
        })
    }

    /// Takes over from the instance that exported `state`: opens its files
    /// again, each judged anew, and moves the window. A reply means every file
    /// has begun to load, which is what lets the old instance exit. Refused
    /// without changing anything if this instance has opened something already
    /// or has been edited.
    fn import_state(&mut self, state: handoff::State) -> dev::Reply {
        handoff::import_allowed(&self.editor)?;
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
        // The size first, so that positions are restored against it. A system that applies
        // it at once says what it came to, with no `Resized`. Most apply it later, so the
        // view takes the size asked for now, and a `Resized` event corrects this if the
        // window ends up another size.
        if let (Some(view), Some(frame)) = (&mut self.view, state.frame) {
            if let Some((x, y)) = frame.position {
                view.window.set_outer_position(x, y);
            }
            let (width, height) = view
                .window
                .request_inner_size(frame.width, frame.height)
                .unwrap_or((frame.width, frame.height));
            self.resize(width, height);
        }
        self.restore = Some(handoff::Restore {
            buffer: shown,
            selections: state.selections,
            scroll: state.scroll,
        });
        self.try_restore_positions();
        self.dispatcher
            .restore_pending(&mut self.editor, &state.pending_keys);
        self.request_redraw();
        Ok(json!({ "buffers": self.editor.buffers().count() }))
    }

    /// Applies positions from a handoff if their file has loaded far enough.
    /// Returns whether this is over: restored, or the window has moved on.
    pub(super) fn try_restore_positions(&mut self) -> bool {
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

/// The open questions newer than `newest`, which every prompt open before a request was at
/// most. Ids only grow, so a request that closes one prompt and opens another is told apart.
fn opened_questions(editor: &Editor, newest: Option<PromptId>) -> Vec<PromptId> {
    editor
        .prompts()
        .iter()
        .filter(|prompt| prompt.kind().is_question() && newest.is_none_or(|id| prompt.id() > id))
        .map(|prompt| prompt.id())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::opened_questions;
    use oso_core::editor::Editor;
    use oso_core::minibuffer::Kind;

    #[test]
    fn a_question_opened_where_a_prompt_closed_counts_as_opened() {
        let mut editor = Editor::default();
        let before = editor.ask("Asked by a key?", "keyboard-quit").unwrap();
        let number = Kind::Number { history: None };
        let prompt = editor.open_prompt("Go to line: ", number).unwrap();
        // A request closes the prompt and opens a question: the count stays the same.
        editor.quit();
        let asked = editor.ask("Exit?", "keyboard-quit").unwrap();
        assert_eq!(opened_questions(&editor, Some(prompt)), [asked]);
        assert_eq!(opened_questions(&editor, None), [before, asked]);
    }
}
