//! Native sessions in an issue space's own directory in the app (ADR 0176): the space's gate
//! a native resume, fork, restart resume or switch there runs under, and a new orchestrator's
//! start (part O-3), judged here from the space's record and taken back after each event while
//! its start is under way.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk: the
//! space's checks look at the directory on the start thread (`check_before_start`).
//!
//! - Only in a space's recorded directory, where no project's trust judges a session: the
//!   directory holds no `.git`. Elsewhere, a native session's project's trust does, as before.
//! - A new orchestrator oso starts there, as `Begin::orchestrator_of` says, runs natively too,
//!   unless `[sessions] mode = "tui"` (part O-3); its gate is for that space alone while that
//!   start is the space's. Where the gate refuses it, it starts in a terminal, the echo area
//!   saying why. Any other new start there stays a terminal; so does an adoption.
//! - A session of the person's there is one oso made native there before, under this gate: a
//!   fork, or a session handed from native to the TUI. One the record has as a terminal, as a
//!   terminal fork of an agent's session or an adoption is, never counts.
//! - A gate is taken back once the space closed, its directory changed, `[spaces] root` moved,
//!   or the session is no longer the space's (`space_gate::judge` says no now, or a new
//!   orchestrator's start is no longer its space's), and once its start is no longer under way
//!   with no report or its buffer is gone, in the event that killed it.

use std::collections::HashMap;
use std::path::Path;

use crate::app::App;
use crate::app::sessions::hosting::Begin;
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::stored::same;
use crate::sessions::start::space_gate::{self, ByGate, NotAdmitted, SpaceGate, Standing};
use crate::sessions::worker::SessionKey;

/// The gates of the native starts under way in spaces' directories, by key.
#[derive(Debug, Default)]
pub(in crate::app) struct SpaceGates {
    kept: HashMap<SessionKey, Kept>,
}

/// A gate kept, and whom it was judged for.
#[derive(Debug)]
struct Kept {
    gate: SpaceGate,
    of: Whom,
}

/// Whom a gate is judged for, as `space_gate::Standing` has it.
#[derive(Debug, Clone)]
enum Whom {
    /// A resume's own conversation, or the one a fork copies.
    Of(SessionId),
    /// Space `SpaceId`'s new orchestrator, its start the space's while it is under way.
    NewOrchestrator(SpaceId),
}

impl App {
    /// The space whose recorded directory is `directory`, open or closed.
    pub(in crate::app::sessions::spaces) fn space_at_directory(
        &self,
        directory: &Path,
    ) -> Option<usize> {
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

    /// Whether conversation `of` is a session of the person's in `directory` that oso made
    /// native there: started by the person, not an agent, and native now, or handed from
    /// native to the TUI. A fork of an orchestrator there is one.
    fn persons_there(&self, of: &SessionId, directory: &Path) -> bool {
        (self.hosting.table.iter()).any(|session| {
            session.id == *of
                && same(&session.directory, directory)
                && session.origin == Origin::Person
                && session.parent.is_none()
                && (session.native
                    || self.hosting.natives.has(session.key)
                    || self.hosting.natives.has_aside(session.key))
        })
    }

    /// The gate of `of` in `directory`, as the space whose recorded directory it is has it
    /// now, with `[spaces] root` at `root`; `None` where no space has it.
    fn judge_space_gate(
        &self,
        of: &Whom,
        directory: &Path,
        root: Option<&Path>,
    ) -> Option<Result<SpaceGate, NotAdmitted>> {
        let at = self.space_at_directory(directory)?;
        let stored = self.watch.spaces().stored(at)?;
        let standing = match of {
            Whom::Of(id) => Standing::Of {
                id,
                persons_there: self.persons_there(id, directory),
            },
            Whom::NewOrchestrator(space) => Standing::NewOrchestrator { space },
        };
        Some(space_gate::judge(stored, root, directory, standing))
    }

    /// The gate new start `key`, `name`, `begin`, runs natively under in `directory`
    /// (part O-3): only for the new orchestrator of the space `begin.orchestrator_of` names,
    /// in that space's recorded directory, where its checks run, with `[sessions] mode` not
    /// `"tui"`. `None` where it starts as before: a terminal there, the echo area saying why
    /// the gate refused, or by its project's trust elsewhere. What is kept takes it back until
    /// its start thread reports.
    pub(in crate::app) fn new_orchestrator_gate(
        &mut self,
        key: SessionKey,
        name: &str,
        begin: &Begin,
        directory: &Path,
    ) -> Option<ByGate> {
        let space = begin.orchestrator_of.clone()?;
        let checked = (begin.space.as_ref()).is_some_and(|space| space.in_space_directory);
        if !checked || !self.configuration.native_sessions() {
            return None;
        }
        self.space_at_directory(directory)?;
        match self.keep_space_gate(key, Whom::NewOrchestrator(space), directory) {
            Ok(gate) => Some(gate),
            Err(why) => {
                let why = refused_there(name, why);
                self.editor
                    .message(format!("{why}: it starts in a terminal"));
                None
            }
        }
    }

    /// The gate start `key` of `of` runs under in `directory`, a space's recorded directory,
    /// kept to be taken back until its start thread reports: waiting while starts wait for
    /// the configuration's first look.
    fn keep_space_gate(
        &mut self,
        key: SessionKey,
        of: Whom,
        directory: &Path,
    ) -> Result<ByGate, NotAdmitted> {
        if let Some(old) = self.hosting.spaces.gates.kept.remove(&key) {
            old.gate.revoke();
        }
        if self.starts_wait() {
            return Ok(ByGate::Waits);
        }
        let root = self.spaces_root_now();
        let judged = self.judge_space_gate(&of, directory, root.as_deref());
        let gate = judged.unwrap_or(Err(NotAdmitted::NotMade))?;
        let kept = Kept {
            gate: gate.clone(),
            of,
        };
        self.hosting.spaces.gates.kept.insert(key, kept);
        Ok(ByGate::Gate(gate))
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
        let kept = self.keep_space_gate(key, Whom::Of(of.clone()), directory);
        Some(kept.map_err(|why| refused_there(name, why)))
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
        let root = self.spaces_root_now();
        let of = Whom::Of(of.clone());
        Some((self.judge_space_gate(&of, directory, root.as_deref())?).map(|_| ()))
    }

    /// Whether start `key` is under way, its buffer still there.
    fn start_buffer_kept(&self, key: SessionKey) -> bool {
        (self.hosting.start_buffer(key)).is_some_and(|buffer| self.editor.buffer(buffer).is_some())
    }

    /// Start `key`'s start thread reported: its gate, if it had one, is forgotten.
    pub(in crate::app) fn space_gate_reported(&mut self, key: SessionKey) {
        self.hosting.spaces.gates.kept.remove(&key);
    }

    /// After each event: takes back the gate of each native start under way whose space
    /// closed, whose directory changed, or that is no longer the space's (a new
    /// orchestrator's start no longer its space's), and of each start no longer under way with
    /// no report, or whose buffer is gone, as in the event that killed it: its start thread
    /// refuses it at its look before the spawn.
    pub(in crate::app) fn revoke_space_gates(&mut self) {
        if self.hosting.spaces.gates.kept.is_empty() {
            return;
        }
        let root = self.spaces_root_now();
        let gone: Vec<SessionKey> = (self.hosting.spaces.gates.kept.iter())
            .filter(|(key, kept)| {
                let judged =
                    self.judge_space_gate(&kept.of, kept.gate.directory(), root.as_deref());
                // A new orchestrator's start is its space's until it reports.
                let its_space = match &kept.of {
                    Whom::NewOrchestrator(space) => {
                        self.hosting.spaces.starting.get(key) == Some(space)
                    }
                    Whom::Of(_) => true,
                };
                !self.start_buffer_kept(**key) || !its_space || !matches!(judged, Some(Ok(_)))
            })
            .map(|(key, _)| *key)
            .collect();
        for key in gone {
            if let Some(kept) = self.hosting.spaces.gates.kept.remove(&key) {
                kept.gate.revoke();
            }
        }
    }
}

/// What the echo area says of session `name`, which does not run natively in a space's
/// directory as `why` says.
fn refused_there(name: &str, why: NotAdmitted) -> String {
    format!(
        "Session {name} {}: it does not run natively there",
        why.reason()
    )
}
