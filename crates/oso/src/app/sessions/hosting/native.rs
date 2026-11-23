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
//! - Its buffer's keys, `TAB`, `RET`, `n`/`p` and `]`/`[`, are carried out in `keys`, which
//!   routes the composer's and the queue's to `composer` (step 2).
//! - Its composer, queue and history are `composer`'s; its run state and modeline note are
//!   `note`'s; its resume and a native fork are `resume`'s; how it ends, stopped, failed,
//!   archived or forgotten, `end`'s; its interrupt, `interrupting`'s (step 2c); its slash
//!   commands, `slash`'s (step 2d); its replies' Markdown highlighting, once each is whole,
//!   `markdown`'s (part 2g); its status line, `status`'s (part 3a); its to-do strip,
//!   `todo`'s (part 3e).
//! - Its prompt suggestion is `suggesting`'s, and the recap of a turn that ended out of sight
//!   `recap`'s (part 3f).
//! - Its bridge asks the person's permission for its calls on the sessions' socket, in
//!   `permissions` (step 2).
//! - Its first message is its brief, if it was given one; else the person writes it in its
//!   composer, which a start, a resume and a fork select at once (step 2).
//! - Its process is stopped when its buffer is killed, which archives the session as a
//!   terminal's does (or forgets it, given no message), when the session list stops it, and killed as oso exits. A stopped
//!   native session comes back at oso's next start as a native buffer with no process.

mod composer;
mod end;
mod interrupting;
mod keys;
mod markdown;
mod note;
mod permissions;
mod recap;
mod resume;
mod slash;
mod status;
mod suggesting;
mod todo;

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::project::ProjectId;

use self::note::NativeState;
use super::Begin;
use crate::app::{App, UserEvent};
use crate::git::TrustedRoot;
use crate::sessions::hosted::Run;
use crate::sessions::identity::Origin;
use crate::sessions::native::control::Control;
use crate::sessions::native::interrupt::Interrupt;
use crate::sessions::native::placed::Placed;
use crate::sessions::native::render;
use crate::sessions::native::rows::{Changes, RowChange, Rows};
use crate::sessions::native::slash::SlashCommands;
use crate::sessions::native::turns;
use crate::sessions::native::wire::Event;
use crate::sessions::start::{NativeOrder, Ready, Started};
use crate::sessions::state::Target;
use crate::sessions::stream::{Out, Stream};
use crate::sessions::worker::{Job, SessionKey};

/// The most of a failed process's standard error shown, its end.
const MAX_ERR_SHOWN: usize = 2048;

/// The native sessions the app runs or shows.
#[derive(Debug, Default)]
pub(in crate::app) struct NativeSessions {
    map: HashMap<SessionKey, Native>,
    /// The sessions' socket and the permission questions asked on it (ADR 0165, step 2).
    permissions: permissions::Permissions,
    /// Whose slash commands, at which version, `native-composer-command` completes over
    /// (step 2d).
    slash_listed: Option<(SessionKey, u64)>,
    /// The replies highlighted as Markdown that wait or are open (part 2g).
    replies: markdown::Replies,
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
    /// A message was written to it, or its conversation was there already: without one,
    /// Claude Code keeps no conversation, and nothing is left to resume (a probe of
    /// 2026-11-22).
    spoke: bool,
    /// Where its process is, as its modeline says it.
    state: NativeState,
    /// Its permission card, `*card: NAME*`, once a call asked (step 2e-2).
    card: Option<BufferId>,
    /// Its process's control layer, while it runs, and its interrupt (step 2c).
    control: Option<Control>,
    interrupt: Interrupt,
    /// Its CLI's slash commands (step 2d), and the rows completion offers of them, as made
    /// for that version of them.
    commands: SlashCommands,
    slash_rows: Option<(u64, std::sync::Arc<[oso_core::complete::Candidate]>)>,
    /// The session id its process's first `system/init` gave, and whether a later one gave
    /// another: then it neither resumes of itself nor by `session-resume` (step 2d).
    cli_id: Option<String>,
    moved: bool,
    /// Which of its replies are highlighted as Markdown, or wait to be (part 2g).
    markdown: markdown::Marks,
    /// Its status line, and its process's `get_usage` (part 3a).
    status: status::StatusState,
    /// Its to-do list and strip (part 3e).
    todo: todo::TodoStrip,
    /// Its last turn ended out of the person's sight, and no recap said so yet (part 3f).
    unseen: bool,
}

impl NativeSessions {
    /// Whether session `key` is a native one.
    pub(in crate::app) fn has(&self, key: SessionKey) -> bool {
        self.map.contains_key(&key)
    }

    /// Whether native session `key`'s conversation moved to a session id oso does not follow.
    pub(in crate::app) fn moved(&self, key: SessionKey) -> bool {
        self.map.get(&key).is_some_and(|native| native.moved)
    }

    /// The process id of session `key`'s `claude`, while it runs and is not reaped.
    pub(in crate::app) fn pid(&self, key: SessionKey) -> Option<u32> {
        self.map.get(&key)?.stream.as_ref()?.pid()
    }
}

impl App {
    /// Whether `begin` starts a native session (ADR 0165): `[sessions] mode = "native"`, in a
    /// trusted project (Q5). A person's start with no brief starts at once, the person writing
    /// in its composer (step 2, the author's choice); an agent's with none starts a terminal.
    ///
    /// Only at the root of a project an open buffer says is trusted now, `directory`, or in a
    /// worktree just made, under its own trust: never under the trust of the agent's session
    /// that starts it, which may be an outer project's or one taken back since.
    pub(super) fn native_start(
        &self,
        begin: &Begin,
        directory: &std::path::Path,
        project: Option<ProjectId>,
    ) -> bool {
        let trusted = match &begin.trusted {
            Some(trusted) if begin.worktree => trusted.path() == directory && !trusted.revoked(),
            Some(_) => self.native_trust_now(directory, Some(directory)).is_ok(),
            None => project.is_some_and(|id| TrustedRoot::of(&self.editor, id).is_some()),
        };
        self.configuration.native_sessions()
            && trusted
            && (has_brief(begin.brief.as_deref()) || begin.origin == Origin::Person)
    }

    /// A new native session `key`'s buffer, as [`App::native_session_buffer`] makes it, with
    /// `brief` as its first message: none when blank, as the person writes it in the composer,
    /// and none when it would clear the conversation (step 2d), which waits in the composer
    /// instead and is taken out of `brief`.
    pub(super) fn native_start_buffer(
        &mut self,
        key: SessionKey,
        name: &str,
        directory: PathBuf,
        shown: bool,
        brief: &mut Option<String>,
    ) -> NativeOrder {
        let first = brief.as_deref().filter(|brief| has_brief(Some(brief)));
        let held = first.filter(|first| SlashCommands::default().refused(first).is_some());
        let held = held.map(str::to_owned);
        if held.is_some() {
            *brief = None;
        }
        let first = brief.as_deref().filter(|brief| has_brief(Some(brief)));
        let order = self.native_session_buffer(key, name, directory, shown, first);
        if let Some(held) = held {
            self.hold_in_composer(key, &held);
        }
        order
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
    fn native_order(&mut self, key: SessionKey, buffer: BufferId) -> NativeOrder {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.control = Some(Control::started(std::time::Instant::now()));
            native.interrupt = Interrupt::new(interrupting::SIGNALS);
            native.commands.new_process();
            native.cli_id = None;
            native.status.new_process();
        }
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

    /// Native session `key`'s control layer as if its process started now: its start waited
    /// for the configuration's first look, and its `initialize` is not yet written (ADR 0173).
    pub(super) fn native_control_from_now(&mut self, key: SessionKey) {
        let native = self.hosting.natives.map.get_mut(&key);
        if let Some(control) = native.and_then(|native| native.control.as_mut()) {
            *control = Control::started(std::time::Instant::now());
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
            state: NativeState::Starting,
            card: None,
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
            todo: todo::TodoStrip::default(),
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
        self.set_native_state(key, NativeState::Running);
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
        // Its `initialize` may have been answered before its stream was here (part 3a).
        self.ask_usage(key, None);
        true
    }

    /// Native session `key` did not start: its buffer goes.
    pub(super) fn session_native_not_started(&mut self, key: SessionKey) {
        self.hosting.starting.remove(&key);
        // No process answers its control layer.
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.control = None;
        }
        // A resume keeps its buffer and rows, stopped again.
        let buffer = self
            .hosting
            .natives
            .map
            .get(&key)
            .map(|native| native.buffer);
        if buffer.is_some_and(|buffer| self.native_resume_refused(key, buffer)) {
            return;
        }
        if let Some(native) = self.hosting.natives.map.remove(&key) {
            self.kill_native_buffers(&native);
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
        let seen = self.native_in_front(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let mut changes = Changes::default();
        let mut targets = Vec::new();
        let mut then = Vec::new();
        let mut todo = false;
        let interrupting = |native: &Native| native.interrupt.interrupting();
        let was_interrupting = interrupting(native);
        let (mut noted, mut usage_id) = (false, None);
        let mut suggested = false;
        for mut event in events {
            suggested |= suggesting::suggestion_event(native, &event);
            recap::recap_event(native, &event, seen);
            interrupting::control_event(native, &mut event, &mut then);
            let (changed, ask) = status::status_event(native, &event);
            noted |= changed;
            usage_id = usage_id.or(ask);
            if let Some(change) = slash::commands_event(native, &mut event) {
                changes.add(change);
            }
            if let Some(target) = turns::target(&event) {
                // A turn under way, oso's or one it did not start, holds what is sent back.
                native.composing.busy = !matches!(target, Target::Idle { .. });
                targets.push(target);
            }
            todo |= native.todo.list.apply(&event);
            for change in native.rows.apply(event) {
                changes.add(change);
            }
        }
        let buffer = native.buffer;
        let changed = !changes.is_empty();
        let noted = noted || was_interrupting != interrupting(native);
        draw(&mut self.editor, native, changes);
        if suggested {
            self.draw_queue(key);
        }
        // A turn that ended lets the queue's first go, after the event (`send_waiting_queues`).
        for target in targets {
            self.tell_tracker(key, target);
        }
        for then in then {
            self.interrupt_step(key, then);
        }
        if todo {
            self.draw_todo(key);
        }
        if usage_id.is_some() {
            self.ask_usage(key, usage_id);
        }
        // The interrupt ended with its turn, or the status line changed (part 3a).
        if noted {
            self.native_note_changed(key);
        }
        if (changed || suggested)
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

    /// Gives the sessions worker `target` for native session `key`, as its stream says it.
    fn tell_tracker(&self, key: SessionKey, target: Target) {
        if let Some(worker) = &self.sessions {
            worker.send(Job::Stream(key, target));
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
        let runs = rendered.runs();
        // Drawn plain: a whole reply is highlighted again (part 2g).
        native.markdown.drawn(at);
        editor.replace_native_rows(native.buffer, range.clone(), &rendered.text, &runs);
        native.placed.put(at, range, rendered.text.len());
        debug_assert_eq!(
            editor.buffer(native.buffer).map(|b| b.text().len()),
            Some(native.placed.end() + native.composing.rows.len())
        );
    }
}

/// Whether `brief` says anything: a blank one is none, and starts nothing natively on its own.
pub(super) fn has_brief(brief: Option<&str>) -> bool {
    brief.is_some_and(|brief| !brief.trim().is_empty())
}
