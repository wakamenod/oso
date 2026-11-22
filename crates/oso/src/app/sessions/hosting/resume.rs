//! `session-resume` in the app (ADR 0121): a stopped session's conversation taken up again in
//! its own buffer, under its name and in its directory, at the person's command or an agent's
//! `session_resume` (ADR 0153). The start thread refuses a conversation that runs elsewhere;
//! `fork` offers the person a fork then.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::Path;

use oso_core::buffer::BufferId;

use crate::app::App;
use crate::sessions::hosted::Run;
use crate::sessions::start::{Kind, Order, Runs};
use crate::sessions::worker::SessionKey;

impl App {
    /// `session-resume`: the stopped session whose terminal is `buffer`, in the same buffer.
    pub(in crate::app) fn resume_session(&mut self, buffer: BufferId) {
        self.resume_session_shown(buffer, true);
    }

    /// Resumes the stopped session whose terminal is `buffer`, as the person's act, shown in
    /// the selected window if `shown`: `session-resume`, and a session moved into a space
    /// (ADR 0151).
    pub(in crate::app) fn resume_session_shown(&mut self, buffer: BufferId, shown: bool) {
        let Some(session) = self.hosting.table.by_buffer(buffer) else {
            self.editor.message("This buffer holds no session");
            return;
        };
        let key = session.key;
        match self.resume_hosted(key, shown) {
            // The person's resume: no agent resumed it any more (ADR 0153).
            Ok(()) => self.forget_agent_resume(key),
            Err(why) => self.editor.message(why),
        }
    }

    /// Resumes the stopped session `key` in its own buffer, under its name and in its
    /// directory: for the person's `session-resume`, shown, or for an agent's `session_resume`,
    /// behind what the person is doing (ADR 0144, ADR 0153). The start thread refuses a
    /// conversation that runs elsewhere (ADR 0121, the fork hazard).
    ///
    /// # Errors
    ///
    /// Why it does not start: the session is gone, runs, has no buffer, or its space's checks
    /// fail.
    pub(in crate::app) fn resume_hosted(
        &mut self,
        key: SessionKey,
        shown: bool,
    ) -> Result<(), String> {
        let Some(session) = self.hosting.table.get(key) else {
            return Err("That session is gone".into());
        };
        if session.runs() {
            return Err(format!("Session {} runs", session.name));
        }
        // A native session resumes natively, in its own buffer (ADR 0165, step 2).
        if self.hosting.natives.has(key) {
            return self.resume_native(key, shown);
        }
        let Some(buffer) = session
            .buffer
            .filter(|id| self.editor.buffer(*id).is_some())
        else {
            return Err(format!(
                "Session {} has no buffer to resume in",
                session.name
            ));
        };
        let (kind, name, directory, root) = (
            Kind::Resume(session.id.clone()),
            session.name.clone(),
            session.directory.clone(),
            session.root.clone(),
        );
        // An orchestrator's command line is made again from its space as it is now: the CLI
        // keeps neither its `--add-dir`s nor its prompt (ADR 0136, step 3).
        let space = self.orchestrator_start_for(key)?;
        self.turn_agent_channel_on_for_a_session();
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.run = Run::Starting;
        }
        let terminal = self.session_terminal(key, &name, Some(buffer), shown);
        self.order_session(Order {
            token: key,
            kind,
            name,
            directory,
            project: root,
            // A resume takes no baseline: the session keeps the one it had.
            trusted: None,
            state: crate::app::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            runs: Runs::Terminal(terminal),
            brief: None,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: false,
            space,
        });
        Ok(())
    }
}
