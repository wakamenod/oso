//! What opening or saving a file starts, on the UI thread (ADR 0118, ADR 0138 step 1): working
//! out what would start for the person, asking the editor what runs now and what waits, letting
//! go what the person's commands and saves release, and saying in `*Agent*` what was held and let
//! go. Nothing here starts a program yet: the language servers come in ADR 0138's step 3. A dev
//! build run with `OSO_DEV_STARTS=1` has a stand-in, for the end-to-end flows: a Rust file in a
//! trusted project would start a server on open and tell it of a save, and what runs only says
//! so in `*Agent*`.
//!
//! Must not depend on: threads, processes or the channel's wire; the editor holds the marks, and
//! this asks it.

use std::collections::{HashMap, HashSet};

use oso_core::buffer::{Acted, BufferId, Starts};

use super::App;

/// The variable that turns the dev build's stand-in on.
#[cfg(feature = "dev")]
const STAND_IN: &str = "OSO_DEV_STARTS";

/// What the app keeps of opens and saves until what they start is settled.
#[derive(Debug, Default)]
pub(super) struct Starting {
    /// The buffers whose open has been settled: what it starts ran, or waits.
    settled: HashSet<BufferId>,
    /// Who started each running save.
    saves: HashMap<BufferId, Acted>,
    /// A load ended, a project was found or the trust record read since opens were settled.
    asked: bool,
    /// The dev build's stand-in is on.
    #[cfg(feature = "dev")]
    stand_in: bool,
}

impl Starting {
    /// As the environment says: the stand-in only in a dev build, with its variable set to 1.
    pub(super) fn new() -> Self {
        Self {
            #[cfg(feature = "dev")]
            stand_in: std::env::var_os(STAND_IN).is_some_and(|value| value == "1"),
            ..Self::default()
        }
    }

    /// Something an open's settling waits for came: a load's end, a project, the trust record.
    pub(super) fn ask(&mut self) {
        self.asked = true;
    }

    /// A save of `buffer` by `by` started.
    pub(super) fn save_started(&mut self, buffer: BufferId, by: Acted) {
        self.saves.insert(buffer, by);
    }

    /// Who started the save of `buffer` that ended, forgotten now: a save no one here started is
    /// taken as the agent's, so that it runs nothing.
    fn save_ended(&mut self, buffer: BufferId) -> Acted {
        self.saves.remove(&buffer).unwrap_or(Acted::Agent)
    }
}

impl App {
    /// Settles the open of each buffer that is ready: loaded, shown to whoever opened it, in a
    /// project, with the record of trusted projects read. What would start for the person runs,
    /// or waits in the buffer if the agent opened it (ADR 0118). Each buffer once.
    pub(super) fn settle_opens(&mut self) {
        if !self.starting.asked || self.editor.trust_list().is_none() {
            return;
        }
        self.starting.asked = false;
        let editor = &self.editor;
        self.starting
            .settled
            .retain(|id| editor.buffer(*id).is_some());
        self.starting
            .saves
            .retain(|id, _| editor.buffer(*id).is_some());
        let ready: Vec<BufferId> = editor
            .buffers()
            .filter(|(id, buffer)| {
                !self.starting.settled.contains(id)
                    && buffer.path().is_some()
                    && buffer.project().is_some()
                    && buffer.loading().is_none()
                    && !buffer.agent_pending()
                    && !buffer.is_internal()
                    && !buffer.is_terminal()
            })
            .map(|(id, _)| id)
            .collect();
        for id in ready {
            self.starting.settled.insert(id);
            let Some(buffer) = self.editor.buffer(id) else {
                continue;
            };
            // A mark a restart carried still holds what it held.
            if !buffer.open_waits().is_empty() {
                continue;
            }
            // The agent's open stays the agent's until the person lets it go, also over a
            // restart (ADR 0118, Q8): one not settled before it, or that held nothing, as in a
            // project restricted then, is held now rather than run.
            let by = if buffer.opened_by_agent() && !buffer.open_let_go() {
                Acted::Agent
            } else {
                Acted::Person
            };
            let would = self.would_start(id, false);
            let runs = self.editor.starts_on_open(id, would, by);
            self.log_held(id, false);
            self.run_starts(id, runs);
        }
    }

    /// Runs, for the person, what each open mark their commands let go would start now
    /// (ADR 0118, Q3, Q4). After every input.
    pub(super) fn start_released(&mut self) {
        for (id, held) in self.editor.take_released_starts() {
            self.log_let_go(id, held);
            let would = self.would_start(id, false);
            let runs = self.editor.starts_on_open(id, would, Acted::Person);
            self.run_starts(id, runs);
        }
    }

    /// A save of `id` ended: unless it failed, what it starts runs, or waits if it was the
    /// agent's; the person's lets go what an agent's save left waiting there.
    /// Returns who started it, a save no one here started being the agent's.
    pub(super) fn save_starts(&mut self, id: BufferId, failed: bool) -> Acted {
        let by = self.starting.save_ended(id);
        if failed {
            return by;
        }
        let held = self
            .editor
            .buffer(id)
            .map_or(Starts::NONE, |b| b.save_waits());
        let would = self.would_start(id, true);
        let runs = self.editor.starts_on_save(id, would, by);
        match by {
            Acted::Person if !held.is_empty() => self.log_let_go(id, held),
            Acted::Person => {}
            Acted::Agent => self.log_held(id, true),
        }
        self.run_starts(id, runs);
        by
    }

    /// What waits in `id` for the person after an agent's open, or its save if `save`, as the
    /// agent's reply says it: `server waits for the person in main.rs`.
    pub(super) fn waits_note(&self, id: BufferId, save: bool) -> Option<String> {
        let buffer = self.editor.buffer(id)?;
        let waits = if save {
            buffer.save_waits()
        } else {
            buffer.open_waits()
        };
        let shown: Vec<&str> = waits.shown().collect();
        (!shown.is_empty()).then(|| {
            format!(
                "{} waits for the person in {}",
                shown.join(", "),
                buffer.name()
            )
        })
    }

    /// What would start for the person as `id` is opened, or saved if `save`, with the settings
    /// as they are. Only the dev build's stand-in starts anything before ADR 0138's step 3.
    fn would_start(&self, id: BufferId, save: bool) -> Starts {
        #[cfg(feature = "dev")]
        if self.starting.stand_in
            && self
                .editor
                .buffer(id)
                .is_some_and(|buffer| buffer.mode() == oso_core::buffer::Mode::Rust)
        {
            use oso_core::buffer::Start;
            return Starts::of(if save {
                Start::ServerSave
            } else {
                Start::ServerStart
            });
        }
        let _ = (id, save);
        Starts::NONE
    }

    /// Starts what `runs` names for `id`. Nothing does yet but the dev build's stand-in, which
    /// says so in `*Agent*`.
    fn run_starts(&mut self, id: BufferId, runs: Starts) {
        if runs.is_empty() {
            return;
        }
        #[cfg(feature = "dev")]
        if self.starting.stand_in {
            let kinds: Vec<&str> = runs.iter().map(oso_core::buffer::Start::id).collect();
            let name = self.editor.buffer(id).map_or("", |b| b.name()).to_owned();
            self.agent_log(&format!("stand-in ran {} in {name}", kinds.join(", ")));
        }
        let _ = id;
    }

    /// Says in `*Agent*` what waits in `id` after an agent's open, or its save if `save`.
    fn log_held(&mut self, id: BufferId, save: bool) {
        if let Some(note) = self.waits_note(id, save) {
            self.agent_log(&note);
        }
    }

    /// Says in `*Agent*` that the person let go what waited in `id`.
    fn log_let_go(&mut self, id: BufferId, held: Starts) {
        let name = self.editor.buffer(id).map_or("", |b| b.name()).to_owned();
        self.agent_log(&format!(
            "the person let go: {} in {name}",
            held.shown().collect::<Vec<_>>().join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{Acted, Runs, Start};

    use super::Starting;

    /// Every kind that runs a program, so waits after an agent's open or save (ADR 0118, Q2). A
    /// new kind, or one that changes sides, shows in review here.
    const PROGRAM: &[&str] = &["server-start", "server-open", "server-save"];

    #[test]
    fn the_kinds_that_run_a_program_are_these() {
        let programs: Vec<&str> = Start::ALL
            .into_iter()
            .filter(|start| start.runs() == Runs::Program)
            .map(Start::id)
            .collect();
        assert_eq!(programs, PROGRAM);
    }

    #[test]
    fn a_save_is_the_persons_only_when_the_person_started_it() {
        let mut editor = oso_core::editor::Editor::default();
        let mut id = || editor.add_buffer(oso_core::buffer::Buffer::new("b"));
        let (person, agent, unknown) = (id(), id(), id());
        let mut starting = Starting::default();
        starting.save_started(person, Acted::Person);
        starting.save_started(agent, Acted::Agent);
        assert_eq!(starting.save_ended(person), Acted::Person);
        assert_eq!(starting.save_ended(agent), Acted::Agent);
        assert_eq!(starting.save_ended(unknown), Acted::Agent);
        // An ended save is forgotten: its next end, with no new start, is no one's.
        assert_eq!(starting.save_ended(person), Acted::Agent);
    }
}
