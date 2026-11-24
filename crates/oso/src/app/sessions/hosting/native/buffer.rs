//! A native session's buffer (ADR 0165): made for a start, a resume or a fork, with its rows,
//! its composer docked under it and the start thread's order, or brought back at oso's start
//! with no process, saying so.
//!
//! Must not depend on: key presses or how a frame is drawn; the start thread starts the process.

use std::collections::HashMap;
use std::path::PathBuf;

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;

use super::note::NativeState;
use super::widths::Widths;
use super::{Native, composer, draw, has_brief, interrupting, markdown, status, todo};
use crate::app::{App, UserEvent};
use crate::sessions::hosted::Run;
use crate::sessions::native::choice::Choice;
use crate::sessions::native::control::Control;
use crate::sessions::native::interrupt::Interrupt;
use crate::sessions::native::mode::ModeState;
use crate::sessions::native::placed::Placed;
use crate::sessions::native::remote::Remote;
use crate::sessions::native::rows::Rows;
use crate::sessions::native::slash::SlashCommands;
use crate::sessions::native::tail::Tail;
use crate::sessions::start::NativeOrder;
use crate::sessions::worker::SessionKey;

/// A new transcript buffer of the session named `name`, whose long lines wrap until the person
/// truncates them there (ADR 0165).
fn new_transcript(editor: &mut Editor, name: &str) -> BufferId {
    let id = editor.add_native_session_buffer(super::super::buffer_name(name));
    editor.set_wraps(id, true);
    id
}

/// A session's first message, as its start sends it.
#[derive(Clone, Copy)]
pub(in crate::app::sessions::hosting) struct First<'a> {
    text: &'a str,
    /// Whose it is, when an agent's tool started the session: `None` for the person's.
    from: Option<&'a str>,
}

impl App {
    /// A new native session `key`'s buffer, as [`App::native_session_buffer`] makes it, with
    /// `brief` as its first message, `from` whose it is, if an agent's: none when blank, as the
    /// person writes it in the composer, and none when it would clear the conversation (step
    /// 2d), which waits in the composer instead and is taken out of `brief`.
    pub(in crate::app::sessions::hosting) fn native_start_buffer(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: PathBuf,
        shown: bool,
        brief: &mut Option<String>,
        from: Option<&str>,
    ) -> NativeOrder {
        let first = brief.as_deref().filter(|brief| has_brief(Some(brief)));
        let held = first.filter(|first| SlashCommands::default().refused(first).is_some());
        let held = held.map(str::to_owned);
        if held.is_some() {
            *brief = None;
        }
        let first = brief.as_deref().filter(|brief| has_brief(Some(brief)));
        let first = first.map(|text| First { text, from });
        let order = self.native_session_buffer(key, name, directory, shown, first);
        if let Some(held) = held {
            self.hold_in_composer(key, &held);
        }
        order
    }

    /// The buffer a native session `key`, named `name`, in `directory`, is shown in: made now,
    /// and shown unless not `shown`. Its start's order for the start thread, which writes
    /// `first` as its first message: its replay is the person's, or a brief.
    pub(in crate::app::sessions::hosting) fn native_session_buffer(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: PathBuf,
        shown: bool,
        first: Option<First<'_>>,
    ) -> NativeOrder {
        // Its bridge asks the person's permission there (ADR 0165, step 2).
        self.start_sessions_socket();
        let buffer = self.add_native_buffer(key, name, directory, first);
        self.editor.set_running(buffer, true);
        self.set_native_state(key, NativeState::Starting);
        if shown {
            self.editor.show(buffer);
            // With no first message, the person writes it there (step 2).
            if first.is_none() {
                self.composer_go(key, true);
            }
        }
        self.hosting.starting.insert(key, buffer);
        self.native_order(key, buffer)
    }

    /// The start thread's order for native session `key`, shown in `buffer`: its variables,
    /// and where its stream's batches go. Its control layer starts now: the process's
    /// `initialize` may be answered before the start thread says it started (step 2c).
    pub(super) fn native_order(&mut self, key: SessionKey, buffer: BufferId) -> NativeOrder {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.control = Some(Control::started(std::time::Instant::now()));
            native.interrupt = Interrupt::new(interrupting::SIGNALS);
            native.commands.new_process();
            native.cli_id = None;
            native.status.new_process();
            native.choice.new_process();
            // Each native start's settings block it (`sessions::start::launch_of`).
            native.remote = Remote::default();
        }
        // The mode it was in, as `--resume` keeps none (part 3b), and the model and effort
        // chosen (part 3c).
        let native = self.hosting.natives.map.get_mut(&key);
        let (permission_mode, (model, effort)) = native.map_or((None, (None, None)), |native| {
            (native.mode.for_start(), native.choice.for_start())
        });
        let waker = self.waker.clone();
        NativeOrder {
            buffer,
            trust: crate::sessions::start::trust::ByRecord::No,
            by_claim: false,
            gate: crate::sessions::start::space_gate::ByGate::No,
            settings: self.terminal_settings(),
            env: self.configuration.session_env().to_vec(),
            out: Box::new(move |out| {
                waker.send(UserEvent::NativeSession(key, out));
            }),
            permission_mode,
            model,
            effort,
            past: None,
            tui_to_end: None,
        }
    }

    /// Native session `key`'s control layer as if its process started now: its start waited
    /// for the configuration's first look, and its `initialize` is not yet written (ADR 0173).
    pub(in crate::app::sessions::hosting) fn native_control_from_now(&mut self, key: SessionKey) {
        let native = self.hosting.natives.map.get_mut(&key);
        if let Some(control) = native.and_then(|native| native.control.as_mut()) {
            *control = Control::started(std::time::Instant::now());
        }
    }

    /// A native session buffer for session `key`, named `name`, running in `directory`, with
    /// its rows, and `first` as the message oso sent.
    pub(super) fn add_native_buffer(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: PathBuf,
        first: Option<First<'_>>,
    ) -> BufferId {
        let buffer = new_transcript(&mut self.editor, name);
        self.mark_session_buffer(key, buffer);
        let mut rows = Rows::new(directory);
        match first {
            Some(First { text, from: None }) => rows.sent(text),
            Some(First {
                text,
                from: Some(from),
            }) => rows.sent_brief(text, from),
            None => {}
        }
        let native = Native {
            buffer,
            rows,
            placed: Placed::default(),
            tail: Tail::default(),
            widths: Widths::default(),
            stream: None,
            state: NativeState::Starting,
            card: None,
            line: None,
            first: first.is_some(),
            ended: None,
            result: None,
            result_of: None,
            calls: HashMap::new(),
            // Its first message's turn is under way once it runs.
            composing: composer::Composing::starting(first.is_some()),
            markdown: markdown::Marks::default(),
            unseen: false,
            spoke: first.is_some(),
            control: None,
            interrupt: Interrupt::new(interrupting::SIGNALS),
            commands: SlashCommands::default(),
            slash_rows: None,
            cli_id: None,
            moved: false,
            status: status::StatusState::default(),
            mode: ModeState::default(),
            choice: Choice::default(),
            remote: Remote::default(),
            todo: todo::TodoStrip::default(),
            notes_after_past: None,
            resuming_for: super::resumed::ResumeFor::default(),
            not_resumed: None,
            may_have_gone_on: false,
        };
        self.hosting.natives.map.insert(key, native);
        // Its composer, docked under it (ADR 0170).
        self.dock_composer(key, buffer, name);
        buffer
    }

    /// Brings native session `key` back as a buffer with no process, which says so. Nothing
    /// starts.
    pub(in crate::app::sessions::hosting) fn bring_back_native(&mut self, key: SessionKey) {
        let Some(session) = self.hosting.table.get(key) else {
            return;
        };
        let (name, directory) = (session.name.clone(), session.directory.clone());
        let buffer = self.add_native_buffer(key, &name, directory, None);
        self.editor.set_running(buffer, false);
        self.set_native_state(key, NativeState::Stopped);
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            // Kept in the record, so it had a conversation.
            native.spoke = true;
            let change = native
                .rows
                .note("Stopped. Its conversation is kept by Claude Code.");
            draw(&mut self.editor, native, [change]);
        }
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.buffer = Some(buffer);
            session.run = Run::Stopped;
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::editor::Editor;

    use super::new_transcript;

    #[test]
    fn a_transcript_wraps() {
        let mut editor = Editor::default();
        let id = new_transcript(&mut editor, "s");
        let buffer = editor.buffer(id).unwrap();
        assert!(buffer.is_native_session() && buffer.wraps());
    }
}
