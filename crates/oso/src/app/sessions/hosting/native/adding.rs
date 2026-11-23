//! A native orchestrator given a repository added to its space while it runs (ADR 0176, part
//! O-2, the author's Q-O4 (a)): `claude -p` runs no `/add-dir`, so at the person's `y` to the
//! space's offer it is stopped once idle and resumed with the repository given with
//! `--add-dir`, as a mode change's respawn is (ADR 0165, part 3b), its row then saying
//! "Resumed with REPO added."
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - It waits while a turn runs, an interrupt or a switch is under way, the process starts, or
//!   a resume could not start now (`native_resume_held`); it goes at the first event that
//!   finds the session idle, before a queued message goes.
//! - It is given up, saying so, once the session is no longer the space's running
//!   orchestrator, or the repository no longer one of the space's that oso trusts; and once
//!   its process ended another way, or did not resume.
//! - The resume makes the orchestrator's line again from its space as it is then
//!   (`orchestrator_start_for`): the row names the repository only when that start gives it.

use std::path::PathBuf;

use super::mode::process;
use crate::app::App;
use crate::sessions::spaces::SpaceId;
use crate::sessions::worker::SessionKey;

/// A resume with a repository added, at the person's `y`: waiting for the session to be idle,
/// or `stopped` for it.
#[derive(Debug)]
pub(super) struct Adding {
    space: SpaceId,
    root: PathBuf,
    stopped: bool,
}

impl App {
    /// `y` to a space's offer: native orchestrator `key` of `space` is resumed with `root`
    /// added, now if it is idle, else once its turn ends.
    pub(in crate::app) fn resume_with_dir(
        &mut self,
        key: SessionKey,
        space: SpaceId,
        root: PathBuf,
    ) {
        if !self.hosting.natives.has(key) {
            return self.editor.message("That session is gone");
        }
        let name = (self.hosting.table.get(key)).map_or_else(String::new, |s| s.name.clone());
        let shown = root.display().to_string();
        let adding = Adding {
            space,
            root,
            stopped: false,
        };
        self.hosting.natives.adding.insert(key, adding);
        let said = match self.respawn_for_dir(key) {
            true => format!("Resuming {name} with {shown} added"),
            false => format!("{name} resumes with {shown} added once its turn ends"),
        };
        self.editor.message(said);
    }

    /// Stops native session `key`, which waits to be resumed with a repository added, if it is
    /// idle and free to resume: `end` resumes it. Whether it stopped it.
    fn respawn_for_dir(&mut self, key: SessionKey) -> bool {
        let resumable = !self.native_resume_held(key);
        let switching = self.hosting.natives.switches.under_way(key);
        let natives = &mut self.hosting.natives;
        let (Some(native), Some(adding)) = (natives.map.get(&key), natives.adding.get_mut(&key))
        else {
            return false;
        };
        let at = process(native, resumable);
        let Some(stream) = native.stream.as_ref() else {
            return false;
        };
        let free = at.idle && at.spoke && at.resumable && !native.mode.switching() && !switching;
        if adding.stopped || !free {
            return false;
        }
        stream.stop();
        adding.stopped = true;
        let row = format!(
            "Resuming with {} added: claude ends, and resumes its conversation with it.",
            adding.root.display()
        );
        self.native_note_row(key, &row);
        true
    }

    /// After each event, before a queued message goes: each resume with a repository added
    /// that waits goes once its session is idle, or is given up once its offer no longer
    /// holds.
    pub(super) fn sweep_adding_dirs(&mut self) {
        if self.hosting.natives.adding.is_empty() {
            return;
        }
        let waiting: Vec<(SessionKey, SpaceId, PathBuf)> = (self.hosting.natives.adding.iter())
            .filter(|(_, adding)| !adding.stopped)
            .map(|(key, adding)| (*key, adding.space.clone(), adding.root.clone()))
            .collect();
        for (key, space, root) in waiting {
            if !self.hosting.natives.has(key) {
                self.hosting.natives.adding.remove(&key);
            } else if !self.add_dir_holds(key, &space, &root) {
                self.hosting.natives.adding.remove(&key);
                self.editor.message(format!(
                    "Not resumed with {} added: the space or its orchestrator changed meanwhile",
                    root.display()
                ));
            } else {
                self.respawn_for_dir(key);
            }
        }
    }

    /// Whether native session `key` was stopped to be resumed with a repository added.
    pub(super) fn stopped_for_dir(&self, key: SessionKey) -> bool {
        (self.hosting.natives.adding.get(&key)).is_some_and(|adding| adding.stopped)
    }

    /// Native session `key` ended, and does not resume: a resume with a repository added that
    /// it was stopped for is over, and says so in its rows; one that waited is given up.
    pub(super) fn adding_over(&mut self, key: SessionKey) {
        let Some(adding) = self.hosting.natives.adding.remove(&key) else {
            return;
        };
        if adding.stopped {
            let row = format!(
                "{} was not added: claude did not resume.",
                adding.root.display()
            );
            self.native_note_row(key, &row);
        }
    }

    /// The row a resume of native session `key` writes: "Resumed with REPO added." when it was
    /// stopped for that and its start gives it the repository, else "Resumed.".
    pub(super) fn resumed_row(&mut self, key: SessionKey) -> String {
        let adding = (self.hosting.natives.adding.remove(&key)).filter(|adding| adding.stopped);
        match adding {
            Some(adding) if self.was_given(&adding.space, &adding.root) => {
                format!("Resumed with {} added.", adding.root.display())
            }
            _ => "Resumed.".to_owned(),
        }
    }
}
