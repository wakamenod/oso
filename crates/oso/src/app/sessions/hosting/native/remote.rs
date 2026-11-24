//! Remote Control of a native session in the app (ADR 0181, part RC-2): turned on at the person's
//! `y` (`question`), off, and what its state says. The rules are `sessions::native::remote`'s, its
//! start's `sessions::start::connect`'s; across a restart it is `restarting`'s.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - **On** is a respawn: the idle process ends, and its conversation resumes with no block, under
//!   the trust judged then. Leave for that, a [`Connect`], is made at the `y` or from the parent's
//!   restart state, held from the stop to that process's end alone, and given to that resume: any
//!   other end drops it, and no other start is given one. A resume that waits is judged again.
//! - **Off** is told to claude.ai's side, ends the process, whatever runs at the latest a minute
//!   on (`off`), and resumes it blocked; where a resume is held it stays stopped, and says so.
//! - **Every other end** is blocked after, and says that it is off: before oso ends such a process
//!   its side is told, as far as that is still read.
//! - **The mark** is the modeline's and `*Spaces*`', from the order of such a start to its
//!   process's end, also while its buffer is killed and its process has not ended.
//! - The address is copied at the person's key alone, as text that is not secret; it is logged
//!   nowhere and given to no agent.

use std::collections::HashSet;
use std::time::Instant;

use oso_core::editor::NativeAction;

use super::Native;
use crate::app::App;
use crate::app::sessions::hosting::Hosting;
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::native::mode::Refusal;
use crate::sessions::native::remote::Remote;
use crate::sessions::native::remote::off::Off;
use crate::sessions::native::remote::refusal::{At, Refused, Whose};
use crate::sessions::native::wire::control::Request;
use crate::sessions::native::wire::remote::Enable;
use crate::sessions::start::connect::{Connect, Place, Sent, Settled};
use crate::sessions::worker::SessionKey;

mod again;
mod question;

pub(crate) use question::Looked;
use question::Question;

/// The most characters of a session's name shown: more than a name has (`identity::MAX_NAME`), so
/// that it is the one sent.
const MAX_NAME: usize = 64;

/// What a session's end says once it was marked.
const OFF: &str =
    "Remote Control is off: claude.ai keeps what it has, and may still list this session.";

/// What the person is told as it goes off while a turn runs.
const GOING_OFF: &str = "Turning Remote Control off: claude.ai is told, and claude ends once its turn is over, in \
     60 s at the latest, to resume its conversation. native-remote-control again ends it now.";

/// What Remote Control keeps beside the sessions.
#[derive(Debug, Default)]
pub(super) struct Remotes {
    /// The question before it is turned on, and the last one's number.
    question: Option<Question>,
    token: u64,
    /// Sessions whose buffer was killed and whose process may still be connected.
    pub(super) ending: HashSet<SessionKey>,
    /// Conversations forked, through any number of forks, from an agent's session, and the forks
    /// of one being started: never connected, while oso runs or restarts itself. Any may be one
    /// where a restart could not name them.
    from_agents: HashSet<SessionId>,
    forks_of_agents: HashSet<SessionKey>,
    any_may_be_agents: bool,
}

/// Remote Control of a session beside its process's state.
#[derive(Debug, Default)]
pub(super) struct RemoteControl {
    /// Leave for the resume its process was stopped for.
    connect: Option<Connect>,
    /// The person turned it off: its process ends, and resumes blocked.
    off: bool,
    /// Its process, which may be connected, was ended.
    ending: bool,
    /// Its side was told that it is off: kept past its process's end, so that a restart that
    /// waits for others to end does not connect it again.
    told_off: bool,
    /// Where its process runs, for a restart.
    pub(super) place: Option<Place>,
}

impl RemoteControl {
    /// Whether its process was stopped to resume, for either.
    pub(super) fn respawns(&self) -> bool {
        self.connect.is_some() || self.off
    }

    /// Whether its side was told that it is off since its process started.
    pub(super) fn told_off(&self) -> bool {
        self.told_off
    }

    /// When the process of one turned off is to end, until it was ended: a wake past it would
    /// come again and again.
    pub(super) fn deadline(&self, remote: &Remote) -> Option<Instant> {
        let waits = self.off && !self.ending;
        remote.off().filter(|_| waits).map(Off::deadline)
    }

    /// Whether the person turned it off, and its process is to end.
    pub(super) fn off(&self) -> bool {
        self.off
    }

    /// Its process ended: the leave held for its resume.
    pub(super) fn ended(&mut self) -> Option<Connect> {
        let before = std::mem::take(self);
        self.told_off = before.told_off;
        before.connect
    }
}

/// Ends `native`'s process, telling claude.ai's side first that Remote Control is off, where it
/// may be on.
pub(super) fn stop(native: &mut Native) {
    tell_off(native);
    let Some(stream) = &native.stream else {
        return;
    };
    // The first end of one that may not have read it yet waits for it; a turn's tools get no
    // second more, and a second end none.
    let unread = native.remote.off().is_some_and(Off::unread);
    let first =
        native.remote.marked() && !std::mem::replace(&mut native.remote_control.ending, true);
    match first && unread && !native.composing.busy {
        true => stream.stop_told(),
        false => stream.stop(),
    }
}

/// Tells claude.ai's side of `native`'s process that Remote Control is off, where it may be on and
/// was not told before. Whether it was written.
pub(super) fn tell_off(native: &mut Native) -> bool {
    let was_marked = native.remote.marked();
    let id = native.remote.end(native.control.as_mut(), Instant::now());
    native.remote_control.told_off |= was_marked;
    let written = (native.stream.as_ref().zip(id))
        .is_some_and(|(stream, id)| stream.request(id, Request::RemoteControl).is_ok());
    if id.is_some() && !written {
        native.remote.off_not_written();
    }
    written
}

impl super::NativeSessions {
    /// Session `key`'s mark, while its process may be connected to claude.ai.
    pub(in crate::app) fn remote_mark(&self, key: SessionKey) -> Option<&'static str> {
        (self.map.get(&key))
            .and_then(|native| native.remote.mark())
            .or_else(|| self.remotes.ending.contains(&key).then_some("⇄ going off"))
    }
}

impl App {
    /// `native-remote-control` and what goes with it, for the session whose transcript or composer
    /// is in front.
    pub(super) fn remote_key(&mut self, key: SessionKey, action: NativeAction, composer: bool) {
        match action {
            NativeAction::RemoteControl if self.remote_marked(key) => self.remote_off(key),
            NativeAction::RemoteControl => self.ask_remote(key, composer),
            NativeAction::RemoteAddress => self.copy_remote_address(key),
            _ => {}
        }
    }

    /// Whether a call of native session `key`'s may come from claude.ai: it is marked, or the canary
    /// said that it may be connected.
    pub(in crate::app) fn remote_watched(&self, key: SessionKey) -> bool {
        let native = self.hosting.natives.map.get(&key);
        self.remote_marked(key) || native.is_some_and(|native| native.remote.watched())
    }

    /// Whether native session `key`'s process may be connected to claude.ai, by how oso started
    /// it.
    pub(in crate::app) fn remote_marked(&self, key: SessionKey) -> bool {
        self.hosting.natives.remote_mark(key).is_some()
    }

    /// Whose session `key` is, for what is never connected.
    pub(super) fn remote_whose(&self, key: SessionKey) -> Whose {
        let Some(session) = self.hosting.table.get(key) else {
            return Whose::default();
        };
        let agents = session.parent.is_some()
            || session.origin != Origin::Person
            || self.hosting.natives.remotes.any_may_be_agents
            || self
                .hosting
                .natives
                .remotes
                .from_agents
                .contains(&session.id);
        Whose {
            agents,
            orchestrator: self.orchestrates(&session.id)
                || self.is_space_directory(&session.directory),
        }
    }

    /// Whether session `key` is never connected, whatever the person asks.
    pub(in crate::app::sessions::hosting) fn remote_never(&self, key: SessionKey) -> bool {
        self.remote_whose(key).refused().is_some()
    }

    /// Whether `id` is an issue space's orchestrator, or was one.
    fn orchestrates(&self, id: &SessionId) -> bool {
        (self.watch.spaces().iter()).any(|space| {
            space.stored.orchestrator.as_ref() == Some(id) || space.stored.earlier.contains(id)
        })
    }

    /// Why Remote Control is not turned on for native session `key` now.
    pub(super) fn remote_refused(&self, key: SessionKey) -> Option<Refused> {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return Some(Refused::NotRunning);
        };
        let natives = &self.hosting.natives;
        let process = super::mode::process(native, !self.native_resume_held(key));
        let untrusted = (self.hosting.table.get(key)).is_none_or(|session| {
            let root = session.root.as_deref();
            self.native_trust_listed(&session.directory, root, true)
                .is_err()
        });
        let secret = |id| (self.editor.buffer(id)).is_some_and(oso_core::buffer::Buffer::is_secret);
        At {
            whose: self.remote_whose(key),
            runs: native.stream.is_some(),
            starting: native.starting(),
            respawn: process
                .respawn_refusal()
                .or(self.watch.is_hung_up(key).then_some(Refusal::Held)),
            asks: self.native_asks_person(key),
            changing: native.interrupt.interrupting()
                || native.mode.switching()
                || native.remote_control.respawns()
                || natives.adding.contains_key(&key)
                || natives.switches.under_way(key),
            secret: secret(native.buffer) || native.composing.buffer.is_some_and(secret),
            untrusted,
        }
        .refused()
    }

    /// The person's `y`: native session `key`'s process ends, idle, to resume with no block.
    fn remote_on(&mut self, key: SessionKey, sent: Sent) {
        let name = self.remote_name(key);
        if let Some(refused) = self.remote_refused(key) {
            return self
                .editor
                .message(format!("Not connected. {}", refused.message(&name)));
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        native.remote_control.connect = Some(Connect::asked(sent));
        stop(native);
        self.native_note_row(
            key,
            "Turning Remote Control on: claude ends, and resumes its conversation to connect.",
        );
        self.editor
            .message(format!("Connecting {name} to claude.ai…"));
    }

    /// `native-remote-control` of a marked session: turned off, or, going off, ended now.
    fn remote_off(&mut self, key: SessionKey) {
        let name = self.remote_name(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return self
                .editor
                .message(format!("Session {name}'s claude is ending"));
        };
        if native.stream.is_none() {
            // It is never asked to connect, and its process ends once it runs.
            native.remote_control.off = true;
            tell_off(native);
            self.native_note_changed(key);
            return self.editor.message(format!(
                "Session {name} is starting: Remote Control goes off as it runs"
            ));
        }
        let again = native.remote_control.off;
        native.remote_control.off = true;
        tell_off(native);
        if again {
            stop(native);
            return self
                .editor
                .message(format!("Ending {name}'s claude to turn Remote Control off"));
        }
        self.native_note_row(key, GOING_OFF);
        self.native_note_changed(key);
        self.sessions_touched();
        self.editor
            .message(format!("Turning Remote Control of {name} off…"));
        self.sweep_remote_off();
    }

    /// Native session `key`, if it is marked, is turned off, as the person's fork of it asks.
    pub(in crate::app::sessions::hosting) fn remote_off_for_fork(&mut self, id: &SessionId) {
        let key = (self.hosting.table.iter())
            .find(|session| session.id == *id)
            .map(|session| session.key);
        if let Some(key) = key.filter(|key| {
            (self.hosting.natives.map.get(key))
                .is_some_and(|native| native.remote.marked() && !native.remote_control.off)
        }) {
            self.remote_off(key);
        }
    }

    /// After each event, before a queued message goes, and as its time comes: each session
    /// turned off whose process is to end now ends.
    pub(super) fn sweep_remote_off(&mut self) {
        let now = Instant::now();
        for native in self.hosting.natives.map.values_mut() {
            if !native.remote_control.off || native.stream.is_none() {
                continue;
            }
            let idle = !native.composing.busy && !native.interrupt.interrupting();
            // Its input closed already: once only.
            if native.remote.off().is_none_or(|off| off.ends(idle, now))
                && !native.remote_control.ending
            {
                stop(native);
            }
        }
    }

    /// Native session `key` ended and was marked: it is off, said in its rows.
    pub(super) fn remote_ended(&mut self, key: SessionKey, resumes: bool, off: bool) {
        self.native_note_row(key, OFF);
        if off && !resumes {
            self.native_note_row(
                key,
                "It stays stopped, as it cannot be resumed now: session-resume resumes it.",
            );
        }
        self.sessions_touched();
    }

    /// Native session `key`, whose start thread said how Remote Control was settled, runs: it is
    /// asked to connect, if it may.
    pub(super) fn remote_settled(&mut self, key: SessionKey, settled: Settled) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        match &settled {
            Settled::Connects(place) => {
                native.remote_control.place = *place;
                self.remote_ready(key);
                return self.sweep_remote_off();
            }
            Settled::Blocked if !native.remote.marked() => return,
            // Nothing is left to turn off.
            Settled::Blocked | Settled::Refused(_) => native.remote_control.off = false,
        }
        let why = match settled {
            Settled::Refused(why) => Some(why),
            Settled::Blocked | Settled::Connects(_) => None,
        };
        // One that ended and resumes already is another start's.
        let canary = native.remote.marked() && native.remote.start_blocked();
        if canary {
            self.follow(key, super::control::Follow::Canary);
        }
        self.remote_stays_off(key, why.as_deref().unwrap_or("it was started blocked"));
    }

    /// Remote Control of native session `key` stays off, as `why` says: in its rows and the echo
    /// area.
    pub(super) fn remote_stays_off(&mut self, key: SessionKey, why: &str) {
        let name = self.remote_name(key);
        self.native_note_row(key, &format!("Remote Control stays off: {why}."));
        self.editor
            .message(format!("Remote Control of {name} stays off: {why}"));
        self.native_note_changed(key);
        self.sessions_touched();
    }

    /// Native session `key` did not start, or may not connect after all: no mark stays.
    pub(super) fn remote_not_started(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.remote = Remote::default();
            native.remote_control = RemoteControl::default();
        }
        self.sessions_touched();
    }

    /// Asks to connect native session `key`, once it is known that it can be asked: after its
    /// events, and once its process's stream is there.
    pub(super) fn remote_ready(&mut self, key: SessionKey) {
        let Hosting { natives, table, .. } = &mut self.hosting;
        let Some(native) = natives.map.get_mut(&key) else {
            return;
        };
        if !native.remote.marked() {
            return;
        }
        let (Some(stream), Some(control), Some(session)) =
            (&native.stream, native.control.as_mut(), table.get(key))
        else {
            return;
        };
        let was = native.remote.mark();
        if let Some(asked) = native.remote.ready(control, random, Instant::now()) {
            let enable = Enable {
                name: Some(&session.name),
                reattach: native.remote.back(),
                // A restart ends the process, and asks again (ADR 0181).
                keep: true,
            };
            if stream.enable(&asked, &enable).is_err() {
                native.remote.not_written(control);
            }
        }
        if was != native.remote.mark() {
            self.remote_changed(key);
        }
    }

    /// The session there is not the one native session `key` comes back to, or is another
    /// session's here: it is turned off at once.
    pub(super) fn remote_elsewhere(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.remote_control.off = true;
            stop(native);
        }
        self.remote_stays_off(key, "claude.ai answered another session than this one's");
    }

    /// Whether the session there of native session `key` is another session's here.
    pub(super) fn remote_is_anothers(&self, key: SessionKey) -> bool {
        let map = &self.hosting.natives.map;
        let id = |native: &Native| native.remote.enabled().id.clone();
        let Some(own) = map.get(&key).and_then(id) else {
            return false;
        };
        (map.iter()).any(|(other, native)| *other != key && id(native).as_ref() == Some(&own))
    }

    /// Where Remote Control of native session `key` is changed: its modeline, its row in the list
    /// and one of its own.
    pub(super) fn remote_changed(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        // The list says only whether it is marked, which its start and its end change.
        if let Some(row) = native.remote.take_row() {
            self.native_note_row(key, &row);
        }
        self.native_note_changed(key);
    }

    /// `native-remote-control-address`.
    fn copy_remote_address(&mut self, key: SessionKey) {
        let url = (self.hosting.natives.map.get(&key))
            .filter(|native| native.remote.marked())
            .and_then(|native| native.remote.enabled().url.clone());
        let Some(url) = url else {
            return self
                .editor
                .message("claude.ai gave this session no address, or Remote Control is off");
        };
        self.editor.kill_new(url.as_str(), false);
        self.editor
            .message("Copied the session's address on claude.ai");
    }

    /// Session `key`'s name, as the echo area says it.
    pub(super) fn remote_name(&self, key: SessionKey) -> String {
        (self.hosting.table.get(key)).map_or_else(String::new, |session| {
            crate::git::printable(&session.name, MAX_NAME)
        })
    }

    /// The fork `key`, with conversation `id`, of `from`: an agent's, if that is one's, so that it
    /// is never connected either.
    pub(in crate::app::sessions::hosting) fn note_fork(
        &mut self,
        from: &SessionId,
        key: SessionKey,
    ) {
        let source = (self.hosting.table.iter())
            .find(|session| session.id == *from)
            .map(|session| session.key);
        let agents = self.hosting.natives.remotes.from_agents.contains(from)
            || source.is_some_and(|source| self.remote_whose(source).agents);
        if agents {
            self.hosting.natives.remotes.forks_of_agents.insert(key);
        }
    }

    /// Fork `key` started with conversation `id`.
    pub(in crate::app::sessions::hosting) fn fork_started(
        &mut self,
        key: SessionKey,
        id: &SessionId,
    ) {
        if self.hosting.natives.remotes.forks_of_agents.remove(&key) {
            self.hosting.natives.remotes.from_agents.insert(id.clone());
        }
    }
}

impl App {
    /// Fork `key` did not start.
    pub(in crate::app::sessions::hosting) fn fork_not_started(&mut self, key: SessionKey) {
        self.hosting.natives.remotes.forks_of_agents.remove(&key);
    }
}

/// A random number, from the system.
fn random() -> std::io::Result<u64> {
    let mut bytes = [0; 8];
    oso_platform::random::fill(&mut bytes)?;
    Ok(u64::from_ne_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{Remote, RemoteControl};
    use crate::sessions::native::control::Control;

    /// What is due of one going off is due once: its process, ended, may take long to go.
    #[test]
    fn a_process_ended_for_off_wakes_the_loop_no_more() {
        let now = Instant::now();
        let mut remote = Remote::unblocked();
        remote.end(Some(&mut Control::started(now)), now);
        let mut control = RemoteControl::default();
        assert_eq!(
            control.deadline(&remote),
            None,
            "stopped for another reason"
        );
        control.off = true;
        assert!(control.deadline(&remote).is_some_and(|by| by > now));
        control.ending = true;
        assert_eq!(control.deadline(&remote), None);
        assert_eq!(remote.deadline(), None);
    }
}
