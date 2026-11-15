//! What runs when a buffer is opened or saved, and what waits for the person (ADR 0118): an
//! agent's open or save starts no program, and marks the buffer instead; the person's first
//! command there, or their next save of it, lets the mark go.
//!
//! Must not depend on: threads, processes or the host's servers. The host says what would start
//! for the person, as its settings and the project's trust are, and starts what these return.

use super::Editor;
use crate::buffer::{Acted, BufferId, Starts};

impl Editor {
    /// What runs now as `id` is opened by `by`, when `would` is what would start for the
    /// person, with the settings and trust as they are. For the agent, only oso's own work runs:
    /// the programs are kept in the buffer's open mark. Nothing runs in a project that is not
    /// trusted, nor is anything kept (ADR 0118, Q7).
    pub fn starts_on_open(&mut self, id: BufferId, would: Starts, by: Acted) -> Starts {
        self.starts(id, would, by, false)
    }

    /// What runs now as a save of `id` by `by` ends, as [`Self::starts_on_open`] says for an
    /// open. The person's save also lets go what an agent's save left waiting there: what it
    /// would have started runs again only if it is in `would` (ADR 0118, Q4).
    pub fn starts_on_save(&mut self, id: BufferId, would: Starts, by: Acted) -> Starts {
        self.starts(id, would, by, true)
    }

    fn starts(&mut self, id: BufferId, would: Starts, by: Acted, save: bool) -> Starts {
        let trusted = self
            .buffer(id)
            .and_then(|buffer| buffer.project())
            .is_some_and(|project| self.project_trusted(project));
        let Some(buffer) = self.buffer_mut(id) else {
            return Starts::NONE;
        };
        let mark = if save {
            &mut buffer.waits_mut().save
        } else {
            &mut buffer.waits_mut().open
        };
        if !trusted {
            *mark = Starts::NONE;
            return would.in_oso();
        }
        match by {
            Acted::Person => {
                if save {
                    *mark = Starts::NONE;
                }
                would
            }
            Acted::Agent => {
                *mark = mark.union(would.programs());
                would.in_oso()
            }
        }
    }

    /// The person ran a command from the window (ADR 0118, Q3): what an agent's open left
    /// waiting in the selected window's buffer is let go, for the host to take with
    /// [`Self::take_released_starts`]. Not for a command the agent channel ran, nor for the one
    /// the person's `y` to its question ran, which is the agent's; the host does not call it for
    /// the answer to the agent's question itself (Q11), nor for what is no input of the
    /// person's. The person's open is then theirs: [`crate::buffer::Buffer::open_let_go`].
    pub fn note_persons_command(&mut self) {
        if self.runs_for_agent() {
            return;
        }
        let id = self.window().buffer();
        let Some(buffer) = self.buffer_mut(id) else {
            return;
        };
        let open = std::mem::take(&mut buffer.waits_mut().open);
        if !open.is_empty() {
            buffer.waits_mut().open_let_go = true;
            self.agent.released.push((id, open));
        }
    }

    /// The open marks the person's commands let go since the last call, each with what it held.
    /// What runs is the host's to work out again, from its settings and the trust as they are now.
    pub fn take_released_starts(&mut self) -> Vec<(BufferId, Starts)> {
        std::mem::take(&mut self.agent.released)
    }

    /// Clears the marks of every buffer whose project is known and not trusted now, as untrusting
    /// it does (ADR 0118, Q7). A buffer whose project is not found yet, as after a restart,
    /// keeps them.
    pub(super) fn drop_untrusted_waits(&mut self) {
        let untrusted: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| !buffer.waits().is_empty())
            .filter(|(_, buffer)| {
                buffer
                    .project()
                    .is_some_and(|project| !self.project_trusted(project))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in untrusted {
            if let Some(buffer) = self.buffer_mut(id) {
                buffer.carry_waits(Starts::NONE, Starts::NONE);
            }
        }
    }
}
