//! What opening or saving a file starts, on the UI thread (ADR 0118, ADR 0138 steps 1 and 3):
//! working out what would start for the person, asking the editor what runs now and what waits,
//! letting go what the person's commands and saves release, and saying in `*Agent*` what was
//! held, let go and run. An open starts its mode's language server, through `servers`; telling
//! a server of an open or a save comes with ADR 0138's step 4.
//!
//! Must not depend on: processes or the channel's wire; the editor holds the marks, and this
//! asks it. A server is started through `servers`, whose threads spawn it.

use std::collections::{HashMap, HashSet};

use oso_core::buffer::{Acted, BufferId, Start, Starts};

use super::App;

/// What the app keeps of opens and saves until what they start is settled.
#[derive(Debug, Default)]
pub(super) struct Starting {
    /// The buffers whose open has been settled: what it starts ran, or waits.
    settled: HashSet<BufferId>,
    /// Who started each running save.
    saves: HashMap<BufferId, Acted>,
    /// A load ended, a project was found or the trust record read since opens were settled.
    asked: bool,
}

impl Starting {
    pub(super) fn new() -> Self {
        Self::default()
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
    /// project, with the record of trusted projects and the configuration read. What would start
    /// for the person runs, or waits in the buffer if the agent opened it (ADR 0118). Each
    /// buffer once.
    pub(super) fn settle_opens(&mut self) {
        if !self.starting.asked
            || self.editor.trust_list().is_none()
            || !self.configuration.looked()
        {
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
        self.mark_servers();
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
    /// as they are: on an open, its mode's server, when none runs for its project. Telling a
    /// server of an open or a save comes with ADR 0138's step 4.
    fn would_start(&self, id: BufferId, save: bool) -> Starts {
        if save {
            return if self.would_tell_a_save(id) {
                Starts::of(Start::ServerSave)
            } else {
                Starts::NONE
            };
        }
        let mut starts = Starts::NONE;
        if self.would_start_server(id) {
            starts = starts.union(Starts::of(Start::ServerStart));
        }
        if self.would_open_document(id) {
            starts = starts.union(Starts::of(Start::ServerOpen));
        }
        starts
    }

    /// Starts what `runs` names for `id`, and says so in `*Agent*`: the server, the open of its
    /// document, and the save told to the server.
    fn run_starts(&mut self, id: BufferId, runs: Starts) {
        if runs.is_empty() {
            return;
        }
        if runs.iter().any(|start| start == Start::ServerStart) {
            self.start_server_for(id);
        }
        if runs.iter().any(|start| start == Start::ServerOpen) {
            self.open_document(id);
        }
        if runs.iter().any(|start| start == Start::ServerSave) {
            self.tell_a_save(id);
        }
        self.mark_servers();
        let kinds: Vec<&str> = runs.iter().map(Start::id).collect();
        let name = self.editor.buffer(id).map_or("", |b| b.name()).to_owned();
        self.agent_log(&format!("ran {} in {name}", kinds.join(", ")));
    }

    /// Forgets that the opens of `project`'s buffers were settled, as when it is trusted now,
    /// so that the next settling starts what the person's would (ADR 0138, Q5).
    pub(super) fn unsettle_opens_in(&mut self, project: oso_core::project::ProjectId) {
        let editor = &self.editor;
        self.starting.settled.retain(|id| {
            editor
                .buffer(*id)
                .is_none_or(|b| b.project() != Some(project))
        });
        self.starting.ask();
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
