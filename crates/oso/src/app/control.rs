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
use super::shell::Loop;
use crate::dev;
use crate::dev::handoff;
use oso_platform::window::ImeEvent;

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
        self.drop_stale_texts();
        let newest = self.editor.prompts().last().map(|prompt| prompt.id());
        self.dispatcher.set_input_from_socket(true);
        let result = self.answer_control(event_loop, op, received, reply);
        self.dispatcher.set_input_from_socket(false);
        if let Some((reply, result)) = result {
            self.note_remote_questions(newest);
            let _ = reply.send(result);
        }
    }

    /// Does what a request asks, and returns its reply unless one has gone or will go
    /// from elsewhere.
    fn answer_control(
        &mut self,
        event_loop: &Loop<'_>,
        op: dev::UiOp,
        received: Instant,
        reply: std::sync::mpsc::Sender<dev::Reply>,
    ) -> Option<(std::sync::mpsc::Sender<dev::Reply>, dev::Reply)> {
        let result = match op {
            dev::UiOp::SendKeys(keys) => {
                let count = keys.len();
                let started = Instant::now();
                let timings = &mut self.timings;
                let log_gate = &self.log_gate;
                let mut watch = dev::SecretWatch::new(&self.editor);
                let effects =
                    self.dispatcher
                        .handle_keys_with(&mut self.editor, keys, |editor, effects| {
                            let withheld = watch.after_key(editor);
                            // So that what the keys after it log, until the batch ends, is held too.
                            log_gate.note(withheld);
                            let key = dev::HandledKey {
                                redraw: effects.redraw,
                                withheld,
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
                        return None;
                    }
                    Err(message) => Err(message),
                }
            }
            dev::UiOp::RunCommand { name, arg } => {
                let (effects, result) = self.dispatcher.run_command(&mut self.editor, &name, arg);
                self.note_logging();
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
                return None;
            }
            dev::UiOp::ExportState => self.export_state(),
            op @ (dev::UiOp::ExportText { .. } | dev::UiOp::ImportText(_)) => {
                return self.carry_text(op, reply);
            }
            dev::UiOp::DevMessage(text) => self.dev_message(text),
            dev::UiOp::CheckHandover => handoff::check(&self.editor),
            dev::UiOp::Panic | dev::UiOp::EndUnasked => self.end_for_a_test(event_loop, &op),
            dev::UiOp::Ime(report) => self.report_ime(event_loop, report, received),
            dev::UiOp::Clipboard(op) => self.memory_clipboard(op),
            dev::UiOp::ImportState(state) => return self.answer_import(*state, reply),
            // Checked here on the UI thread, and nothing runs after it (see the
            // top), so no edit can come between the check and the exit.
            dev::UiOp::Exit { generation } => {
                let allowed = handoff::exit_allowed(&self.editor, generation);
                if allowed.is_err() {
                    // That restart is over, and its snapshot need hold no text.
                    self.forget_export();
                }
                allowed.map(|()| {
                    self.leave(event_loop, true);
                    json!({ "exiting": true })
                })
            }
        };
        Some((reply, result))
    }

    /// Remembers the questions a request opened, and the prompts that complete, those newer
    /// than `newest` was before it, so that keys from the window cannot answer them: a person
    /// typing does not see a question an agent asked appear under their fingers, nor run a
    /// command an agent chose (ADR 0072, ADR 0076). The window's `C-g` still closes one.
    fn note_remote_questions(&mut self, newest: Option<PromptId>) {
        let opened = opened_questions(&self.editor, newest);
        // What an agent typed into `M-x` is not the person's history.
        for &id in &opened {
            self.editor.keep_out_of_history(id);
        }
        self.remote_questions.extend(opened);
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
        state["clipboard"] = json!({ "waiting": self.dispatcher.waits_for_clipboard() });
        // The steps of the last live resize and the frames drawn for them (ADR 0077): numbers
        // only, null on winit.
        let resize = self
            .view
            .as_ref()
            .and_then(|view| view.window.live_resize());
        state["live_resize"] = resize.map_or(
            serde_json::Value::Null,
            |(steps, frames)| json!({ "steps": steps, "frames": frames }),
        );
        // Whether the view gives the input method a context, which it does only while the IME
        // is on (ADR 0070), and whether AppKit's current context is it; null on winit.
        let context = self
            .view
            .as_ref()
            .and_then(|view| view.window.input_context());
        state["ime"]["input_context"] =
            context.map_or(serde_json::Value::Null, |(given, _)| given.into());
        state["ime"]["current_input_context"] =
            context.map_or(serde_json::Value::Null, |(_, current)| current.into());
        // How much of the cursor's line the window holds for the input method, never its
        // text, and none where none is given or while a buffer that may hold secrets is shown,
        // as `area` is withheld (ADR 0044, ADR 0070).
        let extent = (self.view.as_ref())
            .and_then(|view| view.window.ime_excerpt_extent())
            .filter(|_| !self.editor.shows_secret());
        state["ime"]["excerpt"] = extent.map_or(
            serde_json::Value::Null,
            |(len, cursor)| json!({ "len": len, "cursor": cursor }),
        );
        // Whether secure input is wanted, held by oso, and on for any process in the system
        // (ADR 0073): nothing a process of the same user cannot read already (ADR 0044).
        let secure = self
            .view
            .as_ref()
            .and_then(|view| view.window.secure_input());
        state["secure_input"] = secure.map_or(
            serde_json::Value::Null,
            |(wanted, held, system)| json!({ "wanted": wanted, "held": held, "system_any": system }),
        );
        state
    }

    /// The result of `clipboard`: what the clipboard in memory holds, but never text marked
    /// concealed, or that it now holds the text given. Refused with the system's, so that
    /// no client reads it through oso (ADR 0074).
    fn memory_clipboard(&self, op: dev::ClipboardOp) -> dev::Reply {
        let memory = self
            .clipboard
            .as_ref()
            .and_then(oso_platform::clipboard::Clipboard::memory)
            .ok_or("clipboard needs an editor started with --clipboard memory")?;
        Ok(match op {
            // Concealed text may be a secret kill's, which the socket never shows.
            dev::ClipboardOp::Get => match memory.get() {
                Some((_, true)) => json!({ "text": null, "concealed": true }),
                Some((text, false)) => json!({ "text": text, "concealed": false }),
                None => json!({ "text": null, "concealed": false }),
            },
            dev::ClipboardOp::Set { text, concealed } => {
                memory.set(text, concealed);
                json!({ "set": true })
            }
        })
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
        let cells = &mut super::draw::Cells::default();
        let capture = with_panes(&self.editor, &status, cells, |panes| {
            render_state.capture(panes)
        });
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
    pub(super) fn frame(&self) -> Option<handoff::Frame> {
        let window = &self.view.as_ref()?.window;
        let (width, height) = window.inner_size();
        Some(handoff::Frame {
            position: window.outer_position(),
            width,
            height,
        })
    }
}

/// The open questions, and prompts that complete, newer than `newest`, which every prompt open
/// before a request was at most. A completion's answer runs a command, so one a request opened
/// is answered over the socket too (ADR 0076). Ids only grow, so a request that closes one
/// prompt and opens another is told apart.
fn opened_questions(editor: &Editor, newest: Option<PromptId>) -> Vec<PromptId> {
    editor
        .prompts()
        .iter()
        .filter(|prompt| {
            let held = prompt.kind().is_question() || prompt.candidates().is_some();
            held && newest.is_none_or(|id| prompt.id() > id)
        })
        .map(|prompt| prompt.id())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::opened_questions;
    use oso_core::complete::Candidate;
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

    #[test]
    fn a_prompt_that_completes_counts_as_opened_and_one_that_reads_a_number_does_not() {
        let mut editor = Editor::default();
        let number = Kind::Number { history: None };
        let before = editor.open_prompt("Go to line: ", number).unwrap();
        let items: std::sync::Arc<[Candidate]> = [Candidate::new("yank", "C-y", "")].into();
        let completing = editor.open_completion("M-x", "command", items).unwrap();
        assert_eq!(opened_questions(&editor, None), [completing]);
        assert_eq!(opened_questions(&editor, Some(before)), [completing]);
        assert_eq!(opened_questions(&editor, Some(completing)), []);
    }
}
