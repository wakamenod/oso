//! The looks at a repository an orchestrator's `space_add_repo` names (ADR 0161), kept with
//! the space commands' own checks: one a caller at a time, each with the repositories oso knew
//! when the call came, and one whose connection went adding nothing once it ends.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.

use super::Pending;
use crate::app::App;
use crate::app::sessions::tools::AddLook;

impl App {
    /// Looks at `path` for an agent's `space_add_repo`, as for `space-add-repo`, `path` also
    /// naming it with no link on the way (ADR 0161).
    /// Its call is answered here when the worker did not start.
    pub(in crate::app) fn check_repository_for_agent(&mut self, look: AddLook) {
        if self.sessions.is_none() {
            return look.refuse("spaces need the sessions worker, which did not start");
        }
        // Only a first look tells whether it goes unasked.
        let known = if look.before_question() {
            self.known_roots()
        } else {
            Vec::new()
        };
        let token = self.hosting.spaces.check();
        let path = look.path().to_owned();
        let pending = Pending::Agent(Box::new(look));
        self.hosting.spaces.pending.insert(token, pending);
        self.check_repository(token, path, Some(known));
    }

    /// Whether an agent's look for orchestrator `caller` is out.
    pub(in crate::app) fn agent_looks_at(
        &self,
        caller: crate::sessions::worker::SessionKey,
    ) -> bool {
        (self.hosting.spaces.pending.values())
            .any(|pending| matches!(pending, Pending::Agent(look) if look.caller() == caller))
    }

    /// The agents' looks that `gone` says are of a connection that went: one the person has
    /// not allowed yet adds nothing once it ends. Each stays until its thread answers, so that
    /// its caller's next call waits for it.
    pub(in crate::app) fn abandon_agent_looks(&mut self, gone: impl Fn(&AddLook) -> bool) {
        for pending in self.hosting.spaces.pending.values_mut() {
            if let Pending::Agent(look) = pending
                && gone(look)
            {
                look.abandon();
            }
        }
    }
}
