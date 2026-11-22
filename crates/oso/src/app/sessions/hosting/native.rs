//! Native sessions in the app (ADR 0165, step 1): a session's `claude -p` whose stream is shown
//! as rows in its buffer, `*claude: NAME*`, instead of a terminal. The stream's threads read
//! and parse; here, on the UI thread, each batch of events is folded into the session's rows,
//! only the rows that changed are drawn again, and what the events say of its state goes to the
//! sessions worker.
//!
//! Must not depend on: key presses or how a frame is drawn; the process is started on the
//! start thread and read on threads of its own.
//!
//! - A row is drawn where it is, at the cost of what changed: a reply that grows from its
//!   last line only (ADR 0165, "Step 1, as built"). A frame is asked for only when a shown
//!   session's rows changed, or a session ended.
//! - A native session runs only in a trusted project (Q5): elsewhere a session is a terminal.
//! - Its buffer's keys, `TAB`, `RET`, `n`/`p` and `]`/`[`, are carried out in `keys`.
//! - Its bridge asks the person's permission for its calls on the sessions' socket, in
//!   `permissions` (step 2).
//! - Its first message is its brief; a person's start with none opens `*Brief*` for it first.
//! - Its process is stopped when its buffer is killed, which archives the session as a
//!   terminal's does, when the session list stops it, and killed as oso exits. A stopped
//!   native session comes back at oso's next start as a native buffer with no process.

mod composer;
mod first;
mod keys;
mod permissions;

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::project::ProjectId;
use oso_core::syntax::Highlight;

use super::{Begin, now_ms};
use crate::app::{App, UserEvent};
use crate::git::TrustedRoot;
use crate::sessions::hosted::Run;
use crate::sessions::identity::Origin;
use crate::sessions::native::placed::Placed;
use crate::sessions::native::render::{self, Face, Rendered};
use crate::sessions::native::rows::{Changes, RowChange, Rows};
use crate::sessions::native::turns;
use crate::sessions::native::wire::Event;
use crate::sessions::start::{NativeOrder, Ready, Started};
use crate::sessions::state::{Exit, Target};
use crate::sessions::stream::{Out, Stream};
use crate::sessions::worker::{Job, SessionKey};

/// What a stopped native session's modeline says.
const STOPPED: &str = "stopped";
/// The most of a failed process's standard error shown, its end.
const MAX_ERR_SHOWN: usize = 2048;

/// The native sessions the app runs or shows.
#[derive(Debug, Default)]
pub(in crate::app) struct NativeSessions {
    map: HashMap<SessionKey, Native>,
    /// Starts waiting for their first message in `*Brief*`, by its buffer.
    briefs: HashMap<BufferId, Begin>,
    /// The sessions' socket and the permission questions asked on it (ADR 0165, step 2).
    permissions: permissions::Permissions,
}

/// A native session's buffer, rows and process.
#[derive(Debug)]
struct Native {
    buffer: BufferId,
    rows: Rows,
    placed: Placed,
    stream: Option<Stream>,
    /// It was given a first message, so a turn is under way once it runs.
    first: bool,
    /// Its process ended before the start thread said it started: its status and the end of
    /// its standard error, said once the session is in the table.
    ended: Option<(Option<i32>, String)>,
    /// Its `*result: NAME*`, once `RET` showed a call's whole result, and that call's id.
    result: Option<BufferId>,
    result_of: Option<String>,
    /// Where each call of each open row of several is in the row, as drawn last.
    calls: HashMap<usize, Vec<(usize, Range<usize>)>>,
    /// Its composer, its queue and its history (step 2).
    composing: composer::Composing,
}

impl NativeSessions {
    /// Whether session `key` is a native one.
    pub(in crate::app) fn has(&self, key: SessionKey) -> bool {
        self.map.contains_key(&key)
    }

    /// The process id of session `key`'s `claude`, while it runs and is not reaped.
    pub(in crate::app) fn pid(&self, key: SessionKey) -> Option<u32> {
        self.map.get(&key)?.stream.as_ref()?.pid()
    }

    /// The `*Brief*` buffers of native starts not sent yet.
    pub(in crate::app) fn brief_buffers(&self) -> impl Iterator<Item = &BufferId> {
        self.briefs.keys()
    }
}

impl App {
    /// Whether `begin` starts a native session (ADR 0165): `[sessions] mode = "native"`, in a
    /// trusted project (Q5), with a first message. A person's start with none opens `*Brief*`
    /// for it and starts nothing yet (`None`); an agent's with none starts a terminal.
    pub(super) fn native_start(
        &mut self,
        begin: &Begin,
        project: Option<ProjectId>,
    ) -> Option<bool> {
        let trusted = begin.trusted.is_some()
            || project.is_some_and(|id| TrustedRoot::of(&self.editor, id).is_some());
        if !self.configuration.native_sessions() || !trusted {
            return Some(false);
        }
        if begin
            .brief
            .as_deref()
            .is_some_and(|brief| !brief.trim().is_empty())
        {
            return Some(true);
        }
        if begin.origin != Origin::Person {
            return Some(false);
        }
        self.write_first_message(begin.clone());
        None
    }

    /// The buffer a native session `key`, named `name`, in `directory`, is shown in: made now,
    /// and shown unless not `shown`. Its start's order for the start thread, which writes
    /// `first` as the person's first message: its replay is the person's.
    pub(super) fn native_session_buffer(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: PathBuf,
        shown: bool,
        first: Option<&str>,
    ) -> NativeOrder {
        // Its bridge asks the person's permission there (ADR 0165, step 2).
        self.start_sessions_socket();
        let buffer = self.add_native_buffer(key, name, directory, first);
        self.editor.set_running(buffer, true);
        self.set_native_state(key, "starting");
        if shown {
            self.editor.show(buffer);
        }
        self.hosting.starting.insert(key, buffer);
        let waker = self.waker.clone();
        NativeOrder {
            buffer,
            settings: self.terminal_settings(),
            env: self.configuration.session_env().to_vec(),
            out: Box::new(move |out| {
                waker.send(UserEvent::NativeSession(key, out));
            }),
        }
    }

    /// A native session buffer for session `key`, named `name`, running in `directory`, with
    /// its rows, and `first` as the message oso sent.
    fn add_native_buffer(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: PathBuf,
        first: Option<&str>,
    ) -> BufferId {
        let buffer = self
            .editor
            .add_native_session_buffer(super::buffer_name(name));
        self.mark_session_buffer(key, buffer);
        let mut rows = Rows::new(directory);
        if let Some(first) = first {
            rows.sent(first);
        }
        let native = Native {
            buffer,
            rows,
            placed: Placed::default(),
            stream: None,
            first: first.is_some(),
            ended: None,
            result: None,
            result_of: None,
            calls: HashMap::new(),
            // Its first message's turn is under way once it runs.
            composing: composer::Composing::starting(first.is_some()),
        };
        self.hosting.natives.map.insert(key, native);
        // Its composer, docked under it (ADR 0170).
        self.dock_composer(key, buffer, name);
        buffer
    }

    /// Brings native session `key` back as a buffer with no process, which says so. Nothing
    /// starts.
    pub(super) fn bring_back_native(&mut self, key: SessionKey) {
        let Some(session) = self.hosting.table.get(key) else {
            return;
        };
        let (name, directory) = (session.name.clone(), session.directory.clone());
        let buffer = self.add_native_buffer(key, &name, directory, None);
        self.editor.set_running(buffer, false);
        self.set_native_state(key, STOPPED);
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
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

    /// The start thread started native session `key`'s process. Returns whether its buffer
    /// and the table take it; else it is stopped.
    pub(super) fn session_native_started(&mut self, key: SessionKey, ready: Ready) -> bool {
        self.hosting.starting.remove(&key);
        let Ready {
            buffer,
            started: Started::Native(stream),
            directory,
            ..
        } = ready
        else {
            return false;
        };
        let kept = self.editor.buffer(buffer).is_some()
            && self.hosting.natives.map.contains_key(&key)
            && self.hosting.table.get(key).is_some();
        if !kept {
            stream.stop();
            return false;
        }
        let child = stream.pid();
        let Some(session) = self.hosting.table.get_mut(key) else {
            return false;
        };
        session.buffer = Some(buffer);
        session.run = Run::Running { child };
        session.archived = false;
        session.native = true;
        let name = session.name.clone();
        self.set_native_state(key, "");
        self.editor.note_unlogged(format!(
            "Started native session {name} in {}",
            directory.display()
        ));
        if let Some(worker) = &self.sessions {
            worker.send(Job::StartStream(key));
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return false;
        };
        native.stream = Some(stream);
        // Its first message was written: a turn is under way.
        if native.first
            && let Some(worker) = &self.sessions
        {
            worker.send(Job::Stream(key, Target::Working));
        }
        // It ended before its start was said: it is said now.
        if let Some((status, stderr)) = native.ended.take() {
            self.native_ended(key, status, &stderr);
        }
        true
    }

    /// Native session `key` did not start: its buffer goes.
    pub(super) fn session_native_not_started(&mut self, key: SessionKey) {
        self.hosting.starting.remove(&key);
        if let Some(native) = self.hosting.natives.map.remove(&key) {
            self.editor.kill_buffer(native.buffer);
            if let Some(composer) = native.composing.buffer {
                self.editor.kill_buffer(composer);
            }
        }
    }

    /// What native session `key`'s stream read.
    pub(in crate::app) fn native_reported(&mut self, key: SessionKey, out: Out) {
        match out {
            Out::Events(events) => self.native_events(key, events),
            Out::Ended { status, stderr } => {
                self.native_ended(key, status, &stderr);
                self.request_redraw();
            }
        }
    }

    /// Folds `events` into session `key`'s rows, draws the rows that changed once each, and
    /// gives the worker what they say of its state.
    fn native_events(&mut self, key: SessionKey, events: Vec<Event>) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let mut changes = Changes::default();
        let mut targets = Vec::new();
        for event in events {
            if let Some(target) = turns::target(&event) {
                // A turn under way, oso's or one it did not start, holds what is sent back.
                native.composing.busy = !matches!(target, Target::Idle { .. });
                targets.push(target);
            }
            for change in native.rows.apply(event) {
                changes.add(change);
            }
        }
        let buffer = native.buffer;
        let changed = !changes.is_empty();
        draw(&mut self.editor, native, changes);
        let idle = targets
            .iter()
            .any(|target| matches!(target, Target::Idle { .. }));
        for target in targets {
            self.tell_tracker(key, target);
        }
        // A turn ended: the queue's first may go.
        if idle {
            self.send_queued(key);
        }
        if changed
            && self
                .editor
                .windows()
                .any(|window| window.buffer() == buffer)
        {
            self.request_redraw();
        }
        // A call the session's bridge asks about may have come.
        self.sync_permissions();
    }

    /// Native session `key`'s process ended with `status`: it is stopped, or failed with the end
    /// of what it wrote on standard error shown.
    fn native_ended(&mut self, key: SessionKey, status: Option<i32>, stderr: &str) {
        // Its bridge asks nothing any more.
        self.drop_permissions_of(key);
        let in_table = self.hosting.table.get(key).is_some();
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if !in_table {
            native.ended = Some((status, stderr.to_owned()));
            return;
        }
        native.stream = None;
        let buffer = native.buffer;
        // No turn runs any more, and what is queued waits for the person.
        self.composer_ended(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let status = if self.watch.take_hung_up(key) {
            Some(0)
        } else {
            status
        };
        if status != Some(0) {
            let end = stderr.trim();
            let end = &end[end.floor_char_boundary(end.len().saturating_sub(MAX_ERR_SHOWN))..];
            let said = match status {
                Some(code) => format!("claude exited with status {code}"),
                None => "claude ended by a signal".to_owned(),
            };
            let mut changes = vec![native.rows.note(&said)];
            if !end.is_empty() {
                changes.push(
                    native
                        .rows
                        .note(&format!("claude's standard error:\n{end}")),
                );
            }
            draw(&mut self.editor, native, changes);
        }
        self.editor.set_running(buffer, false);
        let Some(exit) = self.hosting.table.ended(key, status, now_ms()) else {
            return;
        };
        let state = match exit {
            Exit::Stopped => STOPPED,
            Exit::Failed => "failed",
        };
        self.set_native_state(key, state);
        if let Some(worker) = &self.sessions {
            worker.send(Job::Exited(key, exit));
        }
        self.remember_sessions();
        self.replaced_orchestrator_ended(key, status.is_some());
    }

    /// After each event: a native session whose buffer was killed is stopped and archived, as
    /// a terminal's is (ADR 0121, Q3), a `*Brief*` killed unsent is forgotten, and a queue that
    /// may go does.
    pub(in crate::app) fn sync_native_sessions(&mut self) {
        self.sync_sessions_socket();
        self.send_waiting_queues();
        let editor = &self.editor;
        self.hosting
            .natives
            .briefs
            .retain(|buffer, _| editor.buffer(*buffer).is_some());
        let gone: Vec<SessionKey> = (self.hosting.natives.map.iter())
            .filter(|(_, native)| editor.buffer(native.buffer).is_none())
            .map(|(key, _)| *key)
            .collect();
        if gone.is_empty() {
            return;
        }
        let mut buffers = Vec::new();
        for key in gone {
            self.drop_permissions_of(key);
            self.hosting.starting.remove(&key);
            if let Some(native) = self.hosting.natives.map.remove(&key) {
                if let Some(stream) = &native.stream {
                    stream.stop();
                }
                buffers.push(native.buffer);
                if let Some(composer) = native.composing.buffer {
                    self.editor.kill_buffer(composer);
                }
            }
        }
        self.archive_killed_sessions(&buffers);
    }

    /// Gives the sessions worker `target` for native session `key`, as its stream says it.
    fn tell_tracker(&self, key: SessionKey, target: Target) {
        if let Some(worker) = &self.sessions {
            worker.send(Job::Stream(key, target));
        }
    }

    /// Stops native session `key`'s process, as the session list's stop does. Whether it ran.
    pub(in crate::app) fn hang_up_native(&mut self, key: SessionKey) -> bool {
        let Some(stream) = (self.hosting.natives.map.get(&key)).and_then(|n| n.stream.as_ref())
        else {
            return false;
        };
        stream.stop();
        true
    }

    /// As oso exits: every native session's process group is killed, as nothing else would
    /// end it.
    pub(in crate::app) fn kill_native_sessions(&mut self) {
        for native in self.hosting.natives.map.values_mut() {
            if let Some(stream) = native.stream.take() {
                stream.kill();
            }
        }
    }
}

/// Draws each of `changes` in the session's buffer, in their rows' order, where `Placed` says
/// each row is.
fn draw(
    editor: &mut oso_core::editor::Editor,
    native: &mut Native,
    changes: impl IntoIterator<Item = RowChange>,
) {
    for change in changes {
        let at = change.row();
        let Some(row) = native.rows.rows().get(at) else {
            continue;
        };
        let Some(text) = editor.buffer(native.buffer).map(Buffer::text) else {
            return;
        };
        let range = native.placed.range(change, |start, lines| {
            let first = text.offset_to_point(start).row;
            text.line_start(first + lines)
        });
        let Some(range) = range else {
            continue;
        };
        let rendered = match change {
            RowChange::Grew { lines, from, .. } => render::tail(row, lines, from),
            RowChange::Added(_) | RowChange::Changed(_) => {
                render::render(row, native.rows.directory())
            }
        };
        if !matches!(change, RowChange::Grew { .. }) {
            if rendered.calls.is_empty() {
                native.calls.remove(&at);
            } else {
                native.calls.insert(at, rendered.calls.clone());
            }
        }
        let runs = runs(&rendered);
        editor.replace_native_rows(native.buffer, range.clone(), &rendered.text, &runs);
        native.placed.put(at, range, rendered.text.len());
        debug_assert_eq!(
            editor.buffer(native.buffer).map(|b| b.text().len()),
            Some(native.placed.end() + native.composing.rows.len())
        );
    }
}

/// A rendered row's faces as runs over its text.
fn runs(rendered: &Rendered) -> Vec<(usize, Option<Highlight>)> {
    let mut runs = Vec::new();
    let mut at = 0;
    for (range, face) in &rendered.faces {
        if range.start > at {
            runs.push((range.start - at, None));
        }
        runs.push((range.end - range.start, Some(highlight(*face))));
        at = range.end;
    }
    runs
}

/// The theme's face for a row's.
fn highlight(face: Face) -> Highlight {
    match face {
        Face::Prompt => Highlight::CommentPerson,
        Face::Dim => Highlight::SessionQuiet,
        Face::Tool => Highlight::Function,
        Face::Target => Highlight::String,
        Face::Added => Highlight::DiffAdded,
        Face::Removed => Highlight::DiffRemoved,
        Face::Hunk => Highlight::DiffHunkHeader,
        Face::Error => Highlight::SessionAlert,
    }
}
