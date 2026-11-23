//! Native sessions in an issue space's own directory in the app (ADR 0176): the space's gate
//! a native resume, fork, restart resume or switch there runs under, judged here from the
//! space's record and taken back after each event while its start is under way; and the
//! record set right when an orchestrator given no message is forgotten.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk: the
//! space's checks look at the directory on the start thread (`check_before_start`).
//!
//! - Only in a space's recorded directory, where no project's trust judges a session: the
//!   directory holds no `.git`. Elsewhere, a native session's project's trust does, as before.
//! - New starts there stay terminals; so does an adoption.
//! - A gate is taken back once the space closed, its directory changed, `[spaces] root` moved,
//!   or the session is no longer the space's (`space_gate::judge` says no now), and once its
//!   start is no longer under way with no report, as when its buffer was killed.

use std::collections::HashMap;
use std::path::Path;

use crate::app::App;
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::spaces::stored::{now_ms, same};
use crate::sessions::start::space_gate::{self, ByGate, NotAdmitted, SpaceGate, Standing};
use crate::sessions::worker::SessionKey;

/// The gates of the native starts under way in spaces' directories, by key.
#[derive(Debug, Default)]
pub(in crate::app) struct SpaceGates {
    kept: HashMap<SessionKey, Kept>,
}

/// A gate kept, and the conversation it was judged for.
#[derive(Debug)]
struct Kept {
    gate: SpaceGate,
    of: SessionId,
}

impl App {
    /// The space whose recorded directory is `directory`, open or closed.
    fn space_at_directory(&self, directory: &Path) -> Option<usize> {
        let spaces = self.watch.spaces();
        spaces.iter().position(|space| {
            (space.stored.directory.as_deref()).is_some_and(|its| same(its, directory))
        })
    }

    /// Whether `directory` is a space's recorded directory: a native session there runs by the
    /// space's gate alone.
    pub(in crate::app) fn is_space_directory(&self, directory: &Path) -> bool {
        self.space_at_directory(directory).is_some()
    }

    /// Whether conversation `of` is a session the person started in `directory`, not an agent:
    /// a fork of an orchestrator there is one.
    fn persons_there(&self, of: &SessionId, directory: &Path) -> bool {
        (self.hosting.table.iter()).any(|session| {
            session.id == *of
                && same(&session.directory, directory)
                && session.origin == Origin::Person
                && session.parent.is_none()
        })
    }

    /// The gate of conversation `of` in `directory`, as the space whose recorded directory it
    /// is has it now; `None` where no space has it.
    fn judge_space_gate(
        &self,
        of: &SessionId,
        directory: &Path,
    ) -> Option<Result<SpaceGate, NotAdmitted>> {
        let at = self.space_at_directory(directory)?;
        let stored = self.watch.spaces().stored(at)?;
        let root = self.spaces_root_now();
        let standing = Standing {
            id: of,
            persons_there: self.persons_there(of, directory),
        };
        Some(space_gate::judge(
            stored,
            root.as_deref(),
            directory,
            standing,
        ))
    }

    /// The gate native start `key`, `name`, of conversation `of` (a resume's own, or the one a
    /// fork copies) runs under in `directory`, when that is a space's recorded directory;
    /// `None` elsewhere, where its project's trust judges it. While starts wait for the
    /// configuration's first look, as `[spaces] root` is not known yet, it waits too, and the
    /// held order is judged again. What is kept takes it back until its start thread reports.
    ///
    /// # Errors
    ///
    /// Why it does not run natively there.
    pub(in crate::app) fn native_space_gate(
        &mut self,
        key: SessionKey,
        of: &SessionId,
        name: &str,
        directory: &Path,
    ) -> Option<Result<ByGate, String>> {
        self.space_at_directory(directory)?;
        if let Some(old) = self.hosting.spaces.gates.kept.remove(&key) {
            old.gate.revoke();
        }
        if self.starts_wait() {
            return Some(Ok(ByGate::Waits));
        }
        Some(match self.judge_space_gate(of, directory)? {
            Ok(gate) => {
                let kept = Kept {
                    gate: gate.clone(),
                    of: of.clone(),
                };
                self.hosting.spaces.gates.kept.insert(key, kept);
                Ok(ByGate::Gate(gate))
            }
            Err(why) => Err(format!(
                "Session {name} {}: it does not run natively there",
                why.reason()
            )),
        })
    }

    /// Whether conversation `of` may run natively in `directory` now, when that is a space's
    /// recorded directory: `None` elsewhere. While starts wait for the configuration's first
    /// look it may, as its start is judged again once they do not.
    ///
    /// # Errors
    ///
    /// Why it does not.
    pub(in crate::app) fn space_gate_now(
        &self,
        of: &SessionId,
        directory: &Path,
    ) -> Option<Result<(), NotAdmitted>> {
        self.space_at_directory(directory)?;
        if self.starts_wait() {
            return Some(Ok(()));
        }
        Some(self.judge_space_gate(of, directory)?.map(|_| ()))
    }

    /// Start `key`'s start thread reported: its gate, if it had one, is forgotten.
    pub(in crate::app) fn space_gate_reported(&mut self, key: SessionKey) {
        self.hosting.spaces.gates.kept.remove(&key);
    }

    /// After each event: takes back the gate of each native start under way whose space
    /// closed, whose directory changed, or that is no longer the space's, and of each start
    /// no longer under way with no report, as when its buffer was killed: its start thread
    /// refuses it at its look before the spawn.
    pub(in crate::app) fn revoke_space_gates(&mut self) {
        if self.hosting.spaces.gates.kept.is_empty() {
            return;
        }
        let gone: Vec<SessionKey> = (self.hosting.spaces.gates.kept.iter())
            .filter(|(key, kept)| {
                !self.hosting.start_under_way(**key)
                    || !matches!(
                        self.judge_space_gate(&kept.of, kept.gate.directory()),
                        Some(Ok(_))
                    )
            })
            .map(|(key, _)| *key)
            .collect();
        for key in gone {
            if let Some(kept) = self.hosting.spaces.gates.kept.remove(&key) {
                kept.gate.revoke();
            }
        }
    }

    /// Session `key`, forgotten as it ended before it was given a message: if it was a space's
    /// orchestrator, the one before it is again, as nothing is left of it to resume (the
    /// author's Q-O8 (a)).
    pub(in crate::app) fn unspoken_orchestrator_forgotten(&mut self, key: SessionKey) {
        let Some(id) = self
            .hosting
            .table
            .get(key)
            .map(|session| session.id.clone())
        else {
            return;
        };
        let spaces = self.watch.spaces_mut();
        let Some(at) = spaces.orchestrated_by(&id) else {
            return;
        };
        spaces.forget_orchestrator(at, &id, now_ms());
        self.sessions_touched();
    }
}
