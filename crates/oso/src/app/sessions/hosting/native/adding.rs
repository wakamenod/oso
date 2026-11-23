//! A native orchestrator given a repository added to its space while it runs (ADR 0176, part
//! O-2, the author's Q-O4 (a)): `claude -p` runs no `/add-dir`, so at the person's `y` to the
//! space's offer it is stopped once idle and resumed with the repository given with
//! `--add-dir`, as a mode change's respawn is (ADR 0165, part 3b), its row then saying
//! "Resumed with REPO added."
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - It waits while a turn runs, an interrupt or a switch is under way, the process starts, or
//!   a resume could not start now (`Process::respawn_refusal`); it goes at the first event that
//!   finds the session free, before a queued message goes.
//! - Once the session is free to stop, each repository is judged again: one no longer of the
//!   space's that oso trusts, or a session no longer the space's running orchestrator, is
//!   given up, saying so. It is also given up once the process ended another way, or did not
//!   resume; a session handed to the TUI meanwhile is given it by that resume.
//! - Any resume of the session gives it the space's trusted repositories, as the line is made
//!   again from its space then (`orchestrator_start_for`): the row is drawn once the start
//!   thread reports, naming the repositories that start passed, else "Resumed.".

use std::path::{Path, PathBuf};

use super::mode::process;
use crate::app::App;
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::stored::same;
use crate::sessions::worker::SessionKey;

/// A resume with repositories added, at the person's `y`s.
#[derive(Debug)]
pub(super) struct Adding {
    space: SpaceId,
    roots: Vec<PathBuf>,
    stage: Stage,
}

/// Where a resume with repositories added is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Waiting for the session to be free to stop.
    Waits,
    /// Stopped for it: `end` resumes it.
    Stopped,
    /// Resumed, by it or otherwise: its row waits for the start thread's report.
    Resuming,
}

impl App {
    /// `y` to a space's offer: native orchestrator `key` of `space` is resumed with `root`
    /// added, now if it is free to stop, else once it is, with any other waiting.
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
        let adding = (self.hosting.natives.adding.entry(key)).or_insert_with(|| Adding {
            space,
            roots: Vec::new(),
            stage: Stage::Waits,
        });
        if adding.stage == Stage::Waits && !adding.roots.iter().any(|was| same(was, &root)) {
            adding.roots.push(root);
        }
        let said = match self.respawn_for_dir(key) {
            true => format!("Resuming {name} with {shown} added"),
            false => format!("{name} resumes with {shown} added once its turn ends"),
        };
        self.editor.message(said);
    }

    /// Whether native session `key`'s process could be stopped to resume now: it runs, free
    /// to resume, and no switch is under way.
    fn free_to_respawn(&self, key: SessionKey) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let at = process(native, !self.native_resume_held(key));
        native.stream.is_some()
            && at.respawn_refusal().is_none()
            && !native.mode.switching()
            && !self.hosting.natives.switches.under_way(key)
    }

    /// Stops native session `key`, whose resume with repositories added waits, if it is free
    /// to: `end` resumes it. Whether it stopped it.
    fn respawn_for_dir(&mut self, key: SessionKey) -> bool {
        if !self.free_to_respawn(key) {
            return false;
        }
        let natives = &mut self.hosting.natives;
        let (Some(native), Some(adding)) = (natives.map.get(&key), natives.adding.get_mut(&key))
        else {
            return false;
        };
        let Some(stream) = native
            .stream
            .as_ref()
            .filter(|_| adding.stage == Stage::Waits)
        else {
            return false;
        };
        stream.stop();
        adding.stage = Stage::Stopped;
        let row = format!(
            "Resuming with {} added: claude ends, and resumes its conversation with them.",
            listed(&adding.roots)
        );
        self.native_note_row(key, &row);
        true
    }

    /// After each event, before a queued message goes: each resume with repositories added
    /// that waits goes once its session is free to, each repository judged again then, and
    /// given up if that no longer holds. While the session is busy, nothing more is looked at.
    pub(super) fn sweep_adding_dirs(&mut self) {
        if self.hosting.natives.adding.is_empty() {
            return;
        }
        // A session gone, killed, forgotten or handed to the TUI takes its resume with it.
        let super::NativeSessions { adding, map, .. } = &mut self.hosting.natives;
        adding.retain(|key, _| map.contains_key(key));
        let waiting: Vec<SessionKey> = (adding.iter())
            .filter(|(_, adding)| adding.stage == Stage::Waits)
            .map(|(key, _)| *key)
            .collect();
        for key in waiting {
            if !self.free_to_respawn(key) {
                continue;
            }
            let Some(adding) = self.hosting.natives.adding.get(&key) else {
                continue;
            };
            let (space, roots) = (adding.space.clone(), adding.roots.clone());
            let (held, dropped): (Vec<PathBuf>, Vec<PathBuf>) =
                (roots.into_iter()).partition(|root| self.add_dir_holds(key, &space, root));
            if !dropped.is_empty() {
                self.editor.message(format!(
                    "Not resumed with {} added: the space or its orchestrator changed meanwhile",
                    listed(&dropped)
                ));
            }
            match held.is_empty() {
                true => drop(self.hosting.natives.adding.remove(&key)),
                false => {
                    if let Some(adding) = self.hosting.natives.adding.get_mut(&key) {
                        adding.roots = held;
                    }
                    self.respawn_for_dir(key);
                }
            }
        }
    }

    /// Whether native session `key` was stopped to be resumed with repositories added.
    pub(super) fn stopped_for_dir(&self, key: SessionKey) -> bool {
        (self.hosting.natives.adding.get(&key)).is_some_and(|adding| adding.stage == Stage::Stopped)
    }

    /// Native session `key` ended and does not resume, or its resume did not start: a resume
    /// with repositories added is over, and says so in its rows if it was stopped for it.
    pub(super) fn adding_over(&mut self, key: SessionKey) {
        let Some(adding) = self.hosting.natives.adding.remove(&key) else {
            return;
        };
        if adding.stage != Stage::Waits {
            let row = format!(
                "{} not added: claude did not resume.",
                listed(&adding.roots)
            );
            self.native_note_row(key, &row);
        }
    }

    /// Native session `key` resumes: whether a resume with repositories added waits for its
    /// start thread's report, so that its row, not "Resumed.", is drawn then. A waiting one
    /// is carried by this resume too, which is made from the space as it is now.
    pub(super) fn resume_takes_dirs(&mut self, key: SessionKey) -> bool {
        let Some(adding) = self.hosting.natives.adding.get_mut(&key) else {
            return false;
        };
        adding.stage = Stage::Resuming;
        true
    }

    /// Native session `key`'s start thread reported, having passed `added` with `--add-dir`:
    /// the row a resume with repositories added waited for, naming those it was given.
    pub(super) fn dirs_added_row(&mut self, key: SessionKey, added: &[PathBuf]) {
        let adding = self.hosting.natives.adding.remove(&key);
        let Some(adding) = adding.filter(|adding| adding.stage == Stage::Resuming) else {
            return;
        };
        let given: Vec<PathBuf> = (adding.roots.into_iter())
            .filter(|root| added.iter().any(|passed| same(passed, root)))
            .collect();
        let row = match given.is_empty() {
            true => "Resumed.".to_owned(),
            false => format!("Resumed with {} added.", listed(&given)),
        };
        self.native_note_row(key, &row);
    }
}

/// `roots`, as a row names them.
fn listed(roots: &[PathBuf]) -> String {
    let shown: Vec<String> = roots.iter().map(|root| shown(root)).collect();
    shown.join(", ")
}

/// `root` as a row shows it: a path `Command::add_dir` took, so one plain line.
fn shown(root: &Path) -> String {
    root.display().to_string()
}
