//! Terminals going over a restart, once exported (ADR 0120, step 11; ADR 0133): the
//! hand-over of their masters, the exit that lets them go, and their reading again when the
//! restart does not come to that.
//!
//! Must not depend on: sockets; the server passes the descriptors.

use std::time::Instant;

use oso_core::buffer::BufferId;
use serde_json::json;

use super::{EXIT_TIME, EXPORT_TIME, NOT_PARENT, QUIET_TIME};
use crate::app::App;
use crate::carry::pieces;
use crate::carry::{Passed, Reply};

impl App {
    /// Whether `item` of export `export` holds a terminal's state, which only the parent may
    /// read, for as long as that export can be read.
    pub(in crate::app) fn is_terminal_item(&self, export: u64, item: usize) -> bool {
        self.handover
            .exported
            .as_ref()
            .is_some_and(|exported| exported.holds_terminal(export, item))
    }

    /// Notes a request about the restart, which keeps exported terminals waiting.
    pub(in crate::app) fn touch_carried(&mut self) {
        if let Some(exported) = self.handover.going.exported.as_mut() {
            exported.touched = Instant::now();
        }
    }

    /// `hand-over-terminals`: a second descriptor for each carried master, in the state's
    /// order, put to go with the reply, if nothing changed since the latest export.
    pub(in crate::app) fn hand_over_terminals(
        &mut self,
        generation: u64,
        passed: &Passed,
    ) -> Reply {
        if !passed.parent {
            return Err(NOT_PARENT.into());
        }
        // A refusal ends that restart: the terminals read again at once.
        let allowed = crate::carry::exit_allowed(&self.editor, Some(generation))
            .and_then(|()| self.carried_unchanged())
            .and_then(|()| {
                let carried: Vec<BufferId> = self.editor.running_terminals().collect();
                self.terminals_may_go(&carried)
            });
        if let Err(why) = allowed {
            self.resume_terminals();
            return Err(why);
        }
        let latest = self.handover.exports;
        let exported = self
            .handover
            .going
            .exported
            .as_mut()
            .filter(|exported| exported.export == latest)
            .ok_or("no terminals were exported by the latest export")?;
        let fds = exported
            .terminals
            .iter()
            .map(|t| crate::carry::fds::try_clone(&t.master))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("cannot hand over the terminals: {e}"))?;
        let count = fds.len();
        passed.passing.put(fds);
        exported.handed = Some(Instant::now());
        exported.touched = Instant::now();
        Ok(json!({ "terminals": count }))
    }

    /// Whether every carried terminal is still there and has read nothing since the export.
    fn carried_unchanged(&self) -> Result<(), String> {
        let Some(exported) = self.handover.going.exported.as_ref() else {
            return Ok(());
        };
        let changed = exported.terminals.iter().any(|carried| {
            self.terminals
                .map
                .get(carried.buffer)
                .and_then(|hosted| hosted.terminal.as_ref())
                .is_none_or(|terminal| terminal.resumes() != carried.resumes)
        });
        if changed {
            return Err(format!(
                "{}: a terminal has read again or ended since the export",
                crate::carry::EXIT_REFUSED
            ));
        }
        Ok(())
    }

    /// Before an `exit` with a generation: the carried terminals go if they were handed over.
    /// An exit that would end a terminal not handed over, or one whose program reads a
    /// password, is refused, and the terminals read again.
    pub(in crate::app) fn exit_with_terminals(&mut self) -> Result<(), String> {
        let handed: Vec<BufferId> = match &self.handover.going.exported {
            Some(exported) if exported.handed.is_some() => {
                exported.terminals.iter().map(|t| t.buffer).collect()
            }
            Some(_) => {
                self.resume_terminals();
                return Err(format!(
                    "{}: the terminals were exported but not handed over",
                    crate::carry::EXIT_REFUSED
                ));
            }
            None => Vec::new(),
        };
        // An exit for a generation ends every terminal it does not let go to the next
        // instance, so one that runs and was not handed over refuses it, as one that read
        // again since its export does.
        let left = self
            .editor
            .running_terminals()
            .find(|buffer| !handed.contains(buffer));
        if let Some(buffer) = left {
            let name = self.buffer_name(buffer);
            self.resume_terminals();
            return Err(format!(
                "{}: {name} runs and was not handed over",
                crate::carry::EXIT_REFUSED
            ));
        }
        let password = handed.iter().copied().find(|&buffer| {
            self.editor
                .buffer(buffer)
                .is_some_and(super::super::reads_a_password)
        });
        if let Some(buffer) = password {
            let name = self.buffer_name(buffer);
            self.resume_terminals();
            return Err(format!(
                "{}: {name}'s program reads a password",
                crate::carry::EXIT_REFUSED
            ));
        }
        if let Err(why) = self.carried_unchanged() {
            self.resume_terminals();
            return Err(why);
        }
        let Some(exported) = self.handover.going.exported.take() else {
            return Ok(());
        };
        for carried in exported.terminals {
            let terminal = self
                .terminals
                .map
                .get_mut(carried.buffer)
                .and_then(|hosted| hosted.terminal.take());
            if let Some(terminal) = terminal {
                terminal.release();
            }
        }
        Ok(())
    }

    /// Has every exported terminal read again, and forgets their masters here.
    pub(in crate::app) fn resume_terminals(&mut self) {
        if let Some(exporting) = self.handover.going.exporting.take() {
            let _ = exporting
                .reply
                .send(Err("the export was overtaken by another".into()));
            self.resume_carried(&exporting.carried);
            pieces::drop_later(exporting);
        }
        let Some(exported) = self.handover.going.exported.take() else {
            return;
        };
        let buffers: Vec<BufferId> = exported.terminals.iter().map(|t| t.buffer).collect();
        self.resume_carried(&buffers);
    }

    /// Has each of `carried` read again; one that was not exported reads on as it was.
    pub(super) fn resume_carried(&self, carried: &[BufferId]) {
        for &buffer in carried {
            if let Some(terminal) = self
                .terminals
                .map
                .get(buffer)
                .and_then(|hosted| hosted.terminal.as_ref())
            {
                terminal.resume();
            }
        }
    }

    /// Before the event loop waits: terminals read again once the restart has gone quiet,
    /// and an export that takes too long fails.
    pub(in crate::app) fn carried_terminals_due(&mut self) {
        if self.handover.going.exporting.is_none() && self.handover.going.exported.is_none() {
            return;
        }
        let now = Instant::now();
        if self
            .handover
            .going
            .exporting
            .as_ref()
            .is_some_and(|e| now.duration_since(e.since) >= EXPORT_TIME)
        {
            self.export_failed("cannot hand over the terminals: they took too long".into());
        }
        let over = self
            .handover
            .going
            .exported
            .as_ref()
            .is_some_and(|e| match e.handed {
                Some(at) => now.duration_since(at) >= EXIT_TIME,
                None => now.duration_since(e.touched) >= QUIET_TIME,
            });
        if over {
            log::info!("restart: the restart went quiet, so the terminals read again");
            self.resume_terminals();
        }
    }

    /// When the event loop wakes for the restart's terminals.
    pub(in crate::app) fn carried_terminals_deadline(&self) -> Option<Instant> {
        let going = &self.handover.going;
        let exporting = going.exporting.as_ref().map(|e| e.since + EXPORT_TIME);
        let exported = going.exported.as_ref().map(|e| match e.handed {
            Some(at) => at + EXIT_TIME,
            None => e.touched + QUIET_TIME,
        });
        exporting.into_iter().chain(exported).min()
    }
}
