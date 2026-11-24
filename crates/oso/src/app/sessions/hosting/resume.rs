//! `session-resume` in the app (ADR 0121): a stopped session's conversation taken up again in
//! its own buffer, under its name and in its directory, at the person's command or an agent's
//! `session_resume` (ADR 0153). The start thread refuses a conversation that runs elsewhere;
//! `fork` offers the person a fork then.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::Path;

use oso_core::buffer::BufferId;

use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::hosted::Run;
use crate::sessions::identity::SessionId;
use crate::sessions::start::space_gate::ByGate;
use crate::sessions::start::trust::ByRecord;
use crate::sessions::start::{Chosen, Kind, NativeOrder, Order, Runs, TerminalOrder};
use crate::sessions::worker::SessionKey;

impl App {
    /// `session-resume`: the stopped session whose terminal is `buffer`, in the same buffer.
    pub(in crate::app) fn resume_session(&mut self, buffer: BufferId) {
        self.resume_session_shown(buffer, true, true);
    }

    /// Resumes the stopped session whose terminal is `buffer`, as the person's act, shown in
    /// the selected window if `shown`: `session-resume`, and a session moved into a space
    /// (ADR 0151). A move is not judged by its worktree's claim (`by_claim`).
    pub(in crate::app) fn resume_session_shown(
        &mut self,
        buffer: BufferId,
        shown: bool,
        by_claim: bool,
    ) {
        let Some(session) = self.hosting.table.by_buffer(buffer) else {
            self.editor.message("This buffer holds no session");
            return;
        };
        let key = session.key;
        match self.resume_hosted_with(key, shown, None, by_claim) {
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
    /// Why it does not start: the session is gone, runs, has no buffer, is native in a
    /// project not trusted now, or its space's checks fail.
    pub(in crate::app) fn resume_hosted(
        &mut self,
        key: SessionKey,
        shown: bool,
    ) -> Result<(), String> {
        self.resume_hosted_with(key, shown, None, true)
    }

    /// [`App::resume_hosted`], a TUI given `chosen`: what the native session handed to it
    /// held (ADR 0165, part 4d-1), which no other resume passes. A native resume of a session
    /// whose TUI's `claude` may still run waits for it to be gone on its start thread, whoever
    /// resumes it (part 4d-2). With `by_claim`, as every resume but a move's into a space, a native
    /// one in a linked worktree with no buffer of it open is judged by its claim there.
    ///
    /// # Errors
    ///
    /// As [`App::resume_hosted`].
    pub(in crate::app::sessions::hosting) fn resume_hosted_with(
        &mut self,
        key: SessionKey,
        shown: bool,
        chosen: Option<Chosen>,
        by_claim: bool,
    ) -> Result<(), String> {
        let Some(session) = self.hosting.table.get(key) else {
            return Err("That session is gone".into());
        };
        // The restart going ends the sessions and the new oso resumes them: one here meanwhile
        // would be a second writer of the conversation (ADR 0165, part 4c).
        if self.restart_going() {
            return Err("oso-restart is under way".into());
        }
        if session.runs() {
            return Err(format!("Session {} runs", session.name));
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
        // A native session resumes natively, in its own buffer, only where its project is
        // trusted now (ADR 0165, step 2): looked at before anything changes.
        let native = self.hosting.natives.has(key);
        if native && self.hosting.natives.moved(key) {
            return Err(format!(
                "Session {name}'s conversation moved to a session id oso does not follow: \
                 resuming it would reach the conversation before, so start a new session"
            ));
        }
        if native && !self.native_spoke(key) {
            return Err(format!(
                "Session {name} was never given a message, so Claude Code has no conversation \
                 to resume: claude starts a new one"
            ));
        }
        let (trusted, trust, gate) = match (native, &kind) {
            (true, Kind::Resume(id)) => {
                let root = root.as_deref();
                self.native_gate_or_trust(key, id, &name, &directory, root, by_claim)?
            }
            _ => (None, ByRecord::No, ByGate::No),
        };
        // An orchestrator's command line is made again from its space as it is now: the CLI
        // keeps neither its `--add-dir`s nor its prompt (ADR 0136, step 3).
        let space = self.orchestrator_start_for(key)?;
        self.turn_agent_channel_on_for_a_session();
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.run = Run::Starting;
        }
        let runs = match native {
            true => Runs::Native(NativeOrder {
                trust,
                by_claim,
                gate,
                tui_to_end: self.hosting.table.get(key).and_then(|s| s.tui_process),
                ..self.resume_native_buffer(key, buffer, shown)
            }),
            false => Runs::Terminal(TerminalOrder {
                handed: chosen,
                ..self.session_terminal(key, &name, Some(buffer), shown)
            }),
        };
        // Resumed now, by whoever asked: not again as a restart's (part 4c).
        self.forget_restart_resume(key);
        self.order_session(Order {
            token: key,
            kind,
            name,
            directory,
            project: root,
            // A native session's gate; a resume takes no baseline, as it keeps the one it had.
            trusted,
            baseline: false,
            state: crate::app::reviews::state(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            runs,
            brief: None,
            steer_worktrees: self.configuration.steer_worktrees(),
            check_shared: false,
            space,
        });
        Ok(())
    }

    /// What native start `key`, `name`, of conversation `of` (its own, or the one a fork
    /// copies) runs under in `directory`, of the project at `root`: its space's gate in a
    /// space's directory (ADR 0176), else its project's trust (ADR 0165, part 4a), each kept
    /// to be taken back until it starts. A resume of a session oso has may be judged by its
    /// worktree's claim (`by_claim`); a fork, an adoption or a move into a space never is.
    ///
    /// # Errors
    ///
    /// Why it does not run natively there.
    pub(in crate::app::sessions::hosting) fn native_gate_or_trust(
        &mut self,
        key: SessionKey,
        of: &SessionId,
        name: &str,
        directory: &Path,
        root: Option<&Path>,
        by_claim: bool,
    ) -> Result<(Option<TrustedRoot>, ByRecord, ByGate), String> {
        if let Some(gate) = self.native_space_gate(key, of, name, directory) {
            return Ok((None, ByRecord::No, gate?));
        }
        let judged = self.native_resume_trust(name, directory, root, by_claim)?;
        let (trusted, trust) = self.native_judged(key, judged);
        Ok((trusted, trust, ByGate::No))
    }
}
