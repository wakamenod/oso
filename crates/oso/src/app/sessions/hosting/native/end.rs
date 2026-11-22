//! How a native session ends (ADR 0165): stopped, failed with the end of its standard error
//! shown, archived when its buffer is killed, as a terminal's is (ADR 0121, Q3), and its
//! process group killed as oso exits.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - A session interrupted with a signal, or stopped while idle to change its mode (part 3b),
//!   ends as asked, and resumes at once, as a resume does (step 2c), in the mode it was in or
//!   is changed to; its queue waits as the interrupt left it.
//! - A session never given a message left Claude Code no conversation, and `--resume` of it
//!   fails ("No conversation found", a probe of 2026-11-22; step 2). It is forgotten, its line
//!   dropped from the record, when it ends cleanly with nothing in its composer or queue, when
//!   its buffer is killed, and at oso's exit. Ended any other way, it stays, stopped or failed,
//!   with what it said and what the person wrote; its resume says it has no conversation.

use super::interrupting::SIGNALS;
use super::note::NativeState;
use super::{MAX_ERR_SHOWN, Native, draw};
use crate::app::App;
use crate::app::sessions::hosting::now_ms;
use crate::sessions::native::interrupt::Interrupt;
use crate::sessions::state::Exit;
use crate::sessions::worker::{Job, SessionKey};

impl App {
    /// Native session `key`'s process ended with `status`: it is stopped, or failed with the end
    /// of what it wrote on standard error shown; given no message, it may be forgotten.
    pub(super) fn native_ended(&mut self, key: SessionKey, status: Option<i32>, stderr: &str) {
        // Its bridge asks nothing any more.
        self.drop_permissions_of(key);
        let in_table = self.hosting.table.get(key).is_some();
        let held = self.native_resume_held(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        if !in_table {
            native.ended = Some((status, stderr.to_owned()));
            return;
        }
        native.stream = None;
        let hung_up = self.watch.take_hung_up(key);
        // Interrupted with a signal, or stopped to resume in another mode (part 3b), it ended
        // as asked, and resumes (step 2c): unless the person stopped it, or a resume is held.
        native.control = None;
        let interrupted =
            std::mem::replace(&mut native.interrupt, Interrupt::new(SIGNALS)).resumes();
        let resumes = (interrupted || native.mode.respawns()) && native.spoke && !hung_up && !held;
        // A switch it was stopped for that does not resume is over, and says so.
        if native.mode.ended(resumes) {
            let change = (native.rows).note("The mode did not change: claude did not resume.");
            draw(&mut self.editor, native, [change]);
        }
        let buffer = native.buffer;
        let status = if hung_up || resumes { Some(0) } else { status };
        let unspoken = !native.spoke && native.composing.queued() == 0;
        // A clean end with nothing said or written: nothing is left to keep.
        if unspoken && status == Some(0) && self.composer_empty(key) {
            self.replaced_orchestrator_ended(key, true);
            return self.forget_unspoken_native(key);
        }
        // No turn runs any more, and what is queued waits for the person; or, as it resumes,
        // waits as the interrupt left it.
        match resumes {
            true => self.composer_idle(key),
            false => self.composer_ended(key),
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
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
            Exit::Stopped => NativeState::Stopped,
            Exit::Failed => NativeState::Failed,
        };
        self.set_native_state(key, state);
        if let Some(worker) = &self.sessions {
            worker.send(Job::Exited(key, exit));
        }
        self.remember_sessions();
        if resumes {
            return self.resume_at_once(key);
        }
        self.replaced_orchestrator_ended(key, status.is_some());
    }

    /// Whether a resume of native session `key` that would start of itself is held now: the
    /// person is asked whether to stop it, oso waits to quit or restart, the replacement of an
    /// orchestrator waits for its end, or its conversation moved to an id oso does not follow.
    pub(super) fn native_resume_held(&self, key: SessionKey) -> bool {
        self.watch.stop_asked(key)
            || self.editor.quit_when_idle()
            || self.restart_holds()
            || self.hosting.spaces.replacing.contains_key(&key)
            || self.hosting.natives.moved(key)
    }

    /// Native session `key`, interrupted with a signal or stopped to change its mode, ended: it
    /// resumes, in its buffer, as a resume does, under the trust judged now; or stays stopped,
    /// saying why.
    fn resume_at_once(&mut self, key: SessionKey) {
        let Err(why) = self.resume_hosted(key, false) else {
            return;
        };
        self.composer_ended(key);
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            let change = native.rows.note(&format!("It did not resume: {why}"));
            draw(&mut self.editor, native, [change]);
        }
    }

    /// Native session `key` ended cleanly before it was given a message: the session and its
    /// buffers go.
    fn forget_unspoken_native(&mut self, key: SessionKey) {
        let name = (self.hosting.table.get(key)).map_or_else(String::new, |s| s.name.clone());
        if let Some(native) = self.hosting.natives.map.remove(&key) {
            self.kill_native_buffers(&native);
        }
        self.forget_started_session(key);
        self.editor.message(format!(
            "Session {name} ended before it was given a message: nothing is left to resume"
        ));
    }

    /// Kills `native`'s transcript and composer, as its session goes.
    pub(super) fn kill_native_buffers(&mut self, native: &Native) {
        if let Some(composer) = native.composing.buffer {
            self.editor.kill_buffer(composer);
        }
        // An internal buffer no kill takes away.
        if let Some(card) = native.card {
            self.editor.remove_internal_buffer(card);
        }
        self.editor.kill_buffer(native.buffer);
    }

    /// After each event: a native session whose buffer was killed is stopped and archived, or
    /// forgotten given no message, a queue that may go does, the replies whole since are
    /// highlighted (part 2g), and a turn that ended out of sight is recapped (part 3f).
    pub(in crate::app) fn sync_native_sessions(&mut self) {
        self.sync_sessions_socket();
        self.sweep_killed_natives();
        // After the sweep: no message goes to a session whose buffer was just killed.
        self.send_waiting_queues();
        self.sync_slash_commands();
        // After the sweep: nothing is opened for a session that is gone.
        self.sync_reply_markdown();
        self.sync_recaps();
    }

    /// Stops and archives each native session whose buffer was killed; one given no message is
    /// forgotten, as it has nothing to keep.
    fn sweep_killed_natives(&mut self) {
        let editor = &self.editor;
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
            let Some(native) = self.hosting.natives.map.remove(&key) else {
                continue;
            };
            if let Some(stream) = &native.stream {
                stream.stop();
            }
            self.kill_native_buffers(&native);
            if native.spoke {
                buffers.push(native.buffer);
            } else {
                self.watch.take_hung_up(key);
                self.forget_started_session(key);
            }
        }
        self.archive_killed_sessions(&buffers);
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
    /// end it, and the record keeps none given no message.
    pub(in crate::app) fn kill_native_sessions(&mut self) {
        let mut unspoken = Vec::new();
        for (key, native) in &mut self.hosting.natives.map {
            // Nothing resumes as oso exits.
            native.control = None;
            native.interrupt = Interrupt::new(SIGNALS);
            if let Some(stream) = native.stream.take() {
                stream.kill();
            }
            if !native.spoke {
                unspoken.push(*key);
            }
        }
        for key in unspoken {
            self.forget_started_session(key);
        }
    }

    /// Whether native session `key` was given a message, or had its conversation already.
    pub(in crate::app::sessions::hosting) fn native_spoke(&self, key: SessionKey) -> bool {
        (self.hosting.natives.map.get(&key)).is_some_and(|native| native.spoke)
    }
}
