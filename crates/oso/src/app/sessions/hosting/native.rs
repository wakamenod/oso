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
//! - A native session runs only in a trusted project (Q5), or in an issue space's own
//!   directory under its gate (ADR 0176): elsewhere a session is a terminal.
//! - Its buffer's keys, `TAB`, `RET`, `n`/`p` and `]`/`[`, are carried out in `keys`, which
//!   routes the composer's and the queue's to `composer` (step 2).
//! - Its buffer, made for a start, a resume or a fork, or brought back with no process, is
//!   `buffer`'s.
//! - Its composer, queue and history are `composer`'s; its run state, modeline note and footer are
//!   `note`'s; its resume and a native fork are `resume`'s; how it ends, stopped, failed,
//!   archived or forgotten, `end`'s; its interrupt, `interrupting`'s (step 2c); its slash
//!   commands, `slash`'s (step 2d); its replies' Markdown highlighting, once each is whole,
//!   `markdown`'s (part 2g); its status line, `status`'s (part 3a); its permission mode,
//!   `mode`'s (part 3b); its `/model` and `/effort` completion, `slash`'s (part 3c); its
//!   to-do strip, `todo`'s (part 3e).
//! - A row is drawn, and what stands after the rows put, by `drawing`; the messages being sent,
//!   which stand there first, are `sending`'s.
//! - Its turns' results stand at its window's right edge, and its replies' tables are drawn for
//!   its width, by `widths`.
//! - Its prompt suggestion is `suggesting`'s, and the recap of a turn that ended out of sight
//!   `recap`'s (part 3f).
//! - Its process ended before `oso-restart`, and resumed by the new instance, is
//!   `restarting`'s (part 4c); what a resume says of how it went is `resumed`'s.
//! - An orchestrator's resume with a repository added to its space is `adding`'s
//!   (ADR 0176, part O-2).
//! - Its side of a switch to the TUI, and its transcript set aside meanwhile, are
//!   `switching`'s (part 4d).
//! - Its earlier turns, read from Claude Code's transcript at a start whose rows hold none of
//!   them, and the turns after its rows when its conversation may have gone on elsewhere, are
//!   `past`'s (step 5).
//! - Its bridge asks the person's permission for its calls on the sessions' socket, in
//!   `permissions` (step 2).
//! - Its first message is its brief, if it was given one; else the person writes it in its
//!   composer, which a start, a resume and a fork select at once (step 2).
//! - Its process is stopped when its buffer is killed, which archives the session as a
//!   terminal's does (or forgets it, given no message), when the session list stops it, and killed as oso exits. A stopped
//!   native session comes back at oso's next start as a native buffer with no process.

mod adding;
mod buffer;
mod carrier;
mod composer;
mod control;
mod drawing;
mod end;
mod holds;
mod interrupting;
mod keys;
mod markdown;
mod mode;
mod note;
mod past;
mod permissions;
mod prompt_history;
mod recap;
mod remote;
mod restarting;
mod resume;
mod resumed;
mod sending;
mod slash;
mod status;
mod suggesting;
mod switching;
mod todo;
mod widths;
mod windows;

pub(crate) use prompt_history::PromptHistoryEvent;
pub(crate) use remote::Looked;
pub(super) use resumed::runs_elsewhere;

use std::collections::HashMap;
use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::project::ProjectId;

pub(in crate::app) use self::composer::Placeholder;
use self::drawing::{draw, put_tail};
use self::note::NativeState;
pub(in crate::app) use self::permissions::{RequestWaits, Typing};
pub(in crate::app) use self::windows::with_transcripts_shown;
use super::Begin;
use super::trust::starts_natively_in;
use crate::app::App;
use crate::sessions::identity::Origin;
use crate::sessions::native::choice::Choice;
use crate::sessions::native::control::Control;
use crate::sessions::native::interrupt::Interrupt;
use crate::sessions::native::mode::ModeState;
use crate::sessions::native::placed::Placed;
use crate::sessions::native::remote::Remote;
use crate::sessions::native::rows::{Changes, RowChange, Rows, UNSHOWN_WIDTH};
use crate::sessions::native::slash::SlashCommands;
use crate::sessions::native::tail::{Part, Tail};
use crate::sessions::native::turns;
use crate::sessions::native::wire::Event;
use crate::sessions::start::connect::Settled;
use crate::sessions::start::{Ready, Started};
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
    /// (step 2d), and its models and effort levels (part 3c).
    slash_listed: Option<slash::Listed>,
    /// The replies highlighted as Markdown that wait or are open (part 2g).
    replies: markdown::Replies,
    /// In a new instance: the sessions the restart's state resumes (part 4c).
    resuming: restarting::Resuming,
    /// The transcripts set aside while their sessions run in the TUI (part 4d).
    aside: HashMap<SessionKey, Native>,
    /// The sessions switching between the modes, and where each switch is (part 4d).
    pub(super) switches: crate::sessions::native::switch::Switches,
    /// The orchestrators resumed with a repository added, at the person's `y` (ADR 0176,
    /// part O-2).
    adding: HashMap<SessionKey, adding::Adding>,
    /// What the person sent from the composers, kept in its file (step 2).
    prompts: prompt_history::Prompts,
    /// What Remote Control keeps beside the sessions (ADR 0181).
    remotes: remote::Remotes,
    /// What an empty composer's placeholder says (ADR 0170), until the user's keys change.
    placeholder: Option<Placeholder>,
}

/// A native session's buffer, rows and process.
#[derive(Debug)]
struct Native {
    buffer: BufferId,
    rows: Rows,
    placed: Placed,
    /// What stands after its rows: the messages being sent, the queue's, and the block of the
    /// call that waits (ADR 0182).
    tail: Tail,
    /// Of which messages being sent it is.
    sending: sending::Drawn,
    /// Its rows that a window's width changes, and the width each was drawn for last.
    widths: widths::Widths,
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
    /// Its card, `*card: NAME*`, once questions or a plan asked (step 2e-2), and its request's
    /// line, `*request: NAME*`, once a call did (ADR 0182).
    card: Option<BufferId>,
    line: Option<BufferId>,
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
    /// Its permission mode, as the CLI says it, and the person's switch of it (part 3b), and
    /// the model and effort level the person chose (part 3c).
    mode: ModeState,
    choice: Choice,
    /// Where Remote Control is in its process (ADR 0181), and what it is respawned for.
    remote: Remote,
    remote_control: remote::RemoteControl,
    /// Its start reads its earlier turns (step 5): the notes said meanwhile, drawn after them.
    notes_after_past: Option<Vec<String>>,
    /// What its resume under way says once its start thread reports.
    resuming_for: resumed::ResumeFor,
    /// Why it did not resume, for its row of `*Spaces*`, until it runs.
    not_resumed: Option<String>,
    /// Its conversation may have gone on elsewhere since its rows: its process ended other
    /// than to resume at once, or the TUI ran it (part 5b).
    may_have_gone_on: bool,
}

impl Native {
    /// Where `part` of what stands after its rows is in its buffer.
    fn part_place(&self, part: Part) -> Range<usize> {
        let (start, place) = (self.placed.end(), self.tail.place(part));
        start + place.start..start + place.end
    }

    /// Whether its process is being started: from its order, held or not (ADR 0173), until
    /// the start thread reports.
    fn starting(&self) -> bool {
        self.state == NativeState::Starting
    }
}

impl NativeSessions {
    /// Whether `buffer` is a transcript with a call's block at its end: what it holds is drawn so that
    /// white space that looks like another is told apart, whatever the person's setting (ADR 0182).
    pub(in crate::app) fn shows_block(&self, buffer: BufferId) -> bool {
        (self.map.values())
            .any(|native| native.buffer == buffer && !native.tail.text(Part::Body).0.is_empty())
    }

    /// Whether session `key` is a native one.
    pub(in crate::app) fn has(&self, key: SessionKey) -> bool {
        self.map.contains_key(&key)
    }

    /// Whether session `key`'s native transcript is set aside while it runs in the TUI
    /// (part 4d).
    pub(in crate::app) fn has_aside(&self, key: SessionKey) -> bool {
        self.aside.contains_key(&key)
    }

    /// Session `key`'s transcript set aside while it runs in the TUI, if one is (part 4d).
    pub(in crate::app) fn aside_buffer(&self, key: SessionKey) -> Option<BufferId> {
        self.aside.get(&key).map(|native| native.buffer)
    }

    /// The buffer of native session `key`'s transcript, unless it is set aside.
    pub(super) fn transcript(&self, key: SessionKey) -> Option<BufferId> {
        self.map.get(&key).map(|native| native.buffer)
    }

    /// Whether native session `key`'s conversation moved to a session id oso does not follow.
    pub(in crate::app) fn moved(&self, key: SessionKey) -> bool {
        self.map.get(&key).is_some_and(|native| native.moved)
    }

    /// What a fork of native session `key` starts with (parts 3b and 3c).
    pub(super) fn forked(&self, key: SessionKey) -> Option<resume::Forked> {
        self.map.get(&key).map(resume::Forked::of)
    }

    /// The process id of session `key`'s `claude`, while it runs and is not reaped.
    pub(in crate::app) fn pid(&self, key: SessionKey) -> Option<u32> {
        self.map.get(&key)?.stream.as_ref()?.pid()
    }
}

impl App {
    /// Whether `begin` starts a native session (ADR 0165): unless `[sessions] mode = "tui"`
    /// (part 4e), in a trusted project (Q5). A person's start with no brief starts at once,
    /// the person writing in its composer (step 2, the author's choice); an agent's with none
    /// starts a terminal.
    ///
    /// Only at the root of a project an open buffer says is trusted now, `directory`, or in a
    /// worktree just made, under its own trust: never under the trust of the agent's session
    /// that starts it, which may be an outer project's or one taken back since. In an issue
    /// space's recorded directory, only a start `gated` by its space's gate, a new orchestrator
    /// (`new_orchestrator_gate`, ADR 0176): never by a project's trust there.
    pub(super) fn native_start(
        &self,
        begin: &Begin,
        directory: &std::path::Path,
        project: Option<ProjectId>,
        gated: bool,
    ) -> bool {
        let trusted = match &begin.trusted {
            _ if gated => true,
            _ if self.is_space_directory(directory) => false,
            Some(trusted) if begin.worktree => trusted.path() == directory && !trusted.revoked(),
            Some(_) => self.native_trust_now(directory, Some(directory)).is_ok(),
            None => project.is_some_and(|id| starts_natively_in(&self.editor, id)),
        };
        self.configuration.native_sessions()
            && trusted
            && (has_brief(begin.brief.as_deref()) || begin.origin == Origin::Person)
    }

    /// The start thread started native session `key`'s process. Returns whether its buffer
    /// and the table take it; else it is stopped.
    pub(super) fn session_native_started(&mut self, key: SessionKey, ready: Ready) -> bool {
        self.hosting.starting.remove(&key);
        let Ready {
            buffer,
            started: Started::Native(stream),
            directory,
            added,
            remote,
            ..
        } = ready
        else {
            return false;
        };
        let kept = self.editor.buffer(buffer).is_some()
            && self.hosting.natives.map.contains_key(&key)
            && self.hosting.table.get(key).is_some();
        if !kept {
            // Marked until it ended, if it may be connected (ADR 0181).
            if matches!(remote, Settled::Connects(_)) {
                self.hosting.natives.remotes.ending.insert(key);
            }
            stream.stop();
            return false;
        }
        let child = stream.pid();
        let Some(session) = self.hosting.table.get_mut(key) else {
            return false;
        };
        session.buffer = Some(buffer);
        session.started_here(child);
        session.archived = false;
        session.native = true;
        // Its start thread saw its TUI's `claude` gone, if it had one (part 4d-2).
        session.tui_process = None;
        let name = session.name.clone();
        self.set_native_state(key, NativeState::Running);
        self.dirs_added_row(key, &added);
        self.native_resumed(key);
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
        // Its first message was written: it is being sent.
        self.editor.refill_wraps();
        let sent = sending::sync(&mut self.editor, native);
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
        if sent && (self.editor.windows()).any(|window| window.buffer() == buffer) {
            self.request_redraw();
        }
        // Its `initialize` may have been answered before its stream was here (part 3a).
        self.ask_usage(key, None);
        self.remote_settled(key, remote);
        true
    }

    /// Native session `key` did not start, as `why` says: its buffer goes, unless it is a
    /// resume's.
    pub(super) fn session_native_not_started(&mut self, key: SessionKey, why: &str) {
        self.hosting.starting.remove(&key);
        // No earlier turns come: what waited for them is said now (step 5).
        self.native_past_given_up(key);
        self.remote_not_started(key);
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
        if buffer.is_some_and(|buffer| self.native_resume_refused(key, buffer, why)) {
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
            Out::Past(past) => self.native_past(key, *past),
            Out::Ended { status, stderr } => {
                if self.hosting.natives.remotes.ending.remove(&key) {
                    self.sessions_touched();
                }
                self.native_ended(key, status, &stderr);
                // The restart goes on once each session it ends has (part 4c).
                self.native_ended_for_restart(key);
                self.request_redraw();
            }
        }
    }

    /// Adds a row of oso's saying `text` to native session `key`'s transcript, and asks for a
    /// frame when the row is in sight.
    fn native_note_row(&mut self, key: SessionKey, text: &str) {
        // After its earlier turns, while they are read (step 5).
        if self.native_note_waits(key, text) {
            return;
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let change = native.rows.note(text);
        let buffer = native.buffer;
        draw(&mut self.editor, native, [change]);
        if (self.editor.windows()).any(|window| window.buffer() == buffer) {
            self.request_redraw();
        }
    }

    /// Folds `events` into session `key`'s rows, draws the rows that changed once each, and
    /// gives the worker what they say of its state.
    fn native_events(&mut self, key: SessionKey, events: Vec<Event>) {
        let seen = self.native_in_front(key);
        let resumable = !self.native_resume_held(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let mut changes = Changes::default();
        let mut targets = Vec::new();
        let mut todo = false;
        let under_way = |native: &Native| (native.interrupt.interrupting(), native.mode.changing());
        let was_under_way = under_way(native);
        let (mut noted, mut usage_id) = (false, None);
        let mut suggested = false;
        let (marked, mut stray) = (native.remote.mark(), None);
        let mut follows = Vec::new();
        for mut event in events {
            suggested |= suggesting::suggestion_event(native, &event);
            recap::recap_event(native, &event, seen);
            noted |= control::control_event(native, &mut event, &mut follows, resumable);
            noted |= mode::mode_event(native, &event);
            stray = native.mode.take_stray_row().or(stray);
            let (changed, ask) = status::status_event(native, &event);
            noted |= changed;
            usage_id = usage_id.or(ask);
            for change in slash::commands_event(native, &mut event) {
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
        // The rows in view wrap within a budget a batch, as within a command's (ADR 0103).
        if changed || native.rows.sending().len() > 0 {
            self.editor.refill_wraps();
        }
        let noted = noted || was_under_way != under_way(native);
        let changed = draw(&mut self.editor, native, changes) || changed;
        // A turn that ended lets the queue's first go, after the event (`send_waiting_queues`).
        for target in targets {
            self.tell_tracker(key, target);
        }
        for follow in follows {
            self.follow(key, follow);
        }
        if let Some(row) = stray {
            self.native_note_row(key, &row);
        }
        if marked
            != self
                .hosting
                .natives
                .map
                .get(&key)
                .and_then(|n| n.remote.mark())
        {
            self.remote_changed(key);
        }
        self.remote_ready(key);
        if todo {
            self.draw_todo(key);
        }
        if usage_id.is_some() {
            self.ask_usage(key, usage_id);
        }
        // The interrupt ended with its turn, or the status line or the mode changed (part 3a).
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

/// Whether `brief` says anything: a blank one is none, and starts nothing natively on its own.
pub(super) fn has_brief(brief: Option<&str>) -> bool {
    brief.is_some_and(|brief| !brief.trim().is_empty())
}
