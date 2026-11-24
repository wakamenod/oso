//! Restarts on the UI thread, `oso dev`'s and `oso-restart`'s: exporting the state and the
//! texts it carries, and taking over from the instance that exported one (ADR 0047, ADR 0055,
//! ADR 0090, ADR 0133). The import, taking over, is `import`'s.
//!
//! Must not depend on: sockets or threads. The handoff's carry jobs copy and build the
//! texts off this thread, and this only clones ropes and installs what they built.

mod import;

use std::sync::mpsc;

use oso_core::buffer::BufferId;
use serde_json::{Value, json};

use super::App;
use crate::carry::{self, pieces};
use crate::carry::{Op, Passed, Reply};

/// What a restart keeps on the UI thread.
#[derive(Default)]
pub(super) struct Handover {
    /// Positions handed over from the previous instance, each window's, until their files
    /// have loaded (ADR 0112).
    pub(super) restore: Vec<carry::Restore>,
    /// How many states this instance has exported, which names the latest.
    pub(super) exports: u64,
    /// The texts the latest export carries, for `export-text`.
    pub(super) exported: Option<pieces::Exported>,
    /// The texts `import-text` has brought for the import that follows.
    staging: pieces::Staging,
    /// The reply to an import that waits for its carried buffers to be judged.
    import: Option<ImportReply>,
    /// Terminals going to the next instance (ADR 0120, step 11).
    pub(super) going: super::terminals::carry::Going,
    /// Terminals the import carried, until their masters come.
    pub(super) coming: super::terminals::Coming,
    /// The instances this one took over from, the newest last, at most 16: what a session's
    /// bridge pinned to one of them looks for (ADR 0121).
    pub(super) follows: Vec<u32>,
}

/// An import's reply, held until every carried buffer has a verdict.
struct ImportReply {
    reply: mpsc::Sender<Reply>,
    result: Value,
    judging: Vec<BufferId>,
}

/// How long an export waits for the reviews to be saved.
const REVIEWS_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

impl App {
    /// The OS window's place and size, if there is a window.
    pub(super) fn frame(&self) -> Option<carry::Frame> {
        let window = &self.view.as_ref()?.window;
        let (width, height) = window.inner_size();
        Some(carry::Frame {
            position: window.outer_position(),
            width,
            height,
        })
    }

    /// `export-state`: the state, with an O(1) snapshot of each text it carries kept for
    /// `export-text`, replacing the one before.
    pub(super) fn export_state(&mut self) -> Reply {
        self.resume_terminals();
        // The reviews are on disk before the next instance opens them again (ADR 0122, step 6),
        // waited for only when one was handed over now.
        if self.hand_review_saves() {
            self.wait_review_saves(REVIEWS_WAIT);
        }
        let (mut state, exported) = self.export_texts(None)?;
        // Last, as they never refuse the restart (ADR 0165, part 4c).
        carry::natives::add_agents(&mut state, self.natives_of_agents());
        carry::natives::add(&mut state, &self.native_resumes());
        self.keep_export(exported);
        Ok(state)
    }

    /// The state and the texts it carries, numbered as the next export; `terminals` the
    /// running terminals it carries too, which the caller adds.
    pub(super) fn export_texts(
        &self,
        terminals: Option<(&[BufferId], usize)>,
    ) -> Result<(Value, pieces::Exported), String> {
        // What a native session's queue holds is lost by a restart: it is refused instead.
        self.refuse_unsent_queues()?;
        let reviews = self.reviews_to_hand_over();
        carry::export(
            &self.editor,
            self.dispatcher.pending_to_carry(),
            self.frame(),
            &self.handover.restore,
            self.handover.exports + 1,
            terminals,
            &reviews,
        )
    }

    /// Keeps `exported` for `export-text`, as the latest export, dropping the one before.
    pub(super) fn keep_export(&mut self, exported: pieces::Exported) {
        self.handover.exports = exported.number();
        pieces::drop_later(self.handover.exported.replace(exported));
    }

    /// The methods of a restart but `exit`: their reply, if it is ready now.
    pub(super) fn answer_restart(
        &mut self,
        op: Op,
        reply: mpsc::Sender<Reply>,
        passed: Passed,
    ) -> Option<(mpsc::Sender<Reply>, Reply)> {
        let result = match op {
            Op::ExportState { library: None } => self.export_state(),
            Op::ExportState {
                library: Some(library),
            } => return self.export_with_terminals(&library, passed.parent, reply),
            op @ (Op::ExportText { .. } | Op::ImportText(_)) => {
                return self.carry_text(op, reply, passed.parent);
            }
            Op::HandOverTerminals { generation } => self.hand_over_terminals(generation, &passed),
            Op::ImportTerminals { count } => self.before_gone(count, passed),
            #[cfg(feature = "dev")]
            Op::CheckHandover { terminals } => self.check_handover(terminals, passed.parent),
            Op::ImportState(state) => {
                return self.answer_import(*state, reply, passed.parent);
            }
        };
        Some((reply, result))
    }

    /// `import-terminals`: the instance before has gone, with `count` the masters of the
    /// terminals the import carried, or stays (`None`). The native sessions the import named
    /// resume here once it has gone (ADR 0165, part 4c), with no terminal too (a count of 0),
    /// and stay stopped otherwise, or if the masters do not match.
    fn before_gone(&mut self, count: Option<usize>, passed: Passed) -> Reply {
        let parent = passed.parent;
        let taken = self.import_terminals(count, passed);
        if parent {
            self.natives_before_gone(count.is_some() && taken.is_ok());
        }
        taken
    }

    /// The exit of a restart, checked here on the UI thread, and nothing runs after it, so no
    /// edit can come between the check and the exit: if the generation is still `generation`,
    /// terminals handed over go now, as the next instance has their masters (ADR 0120, step 11;
    /// ADR 0133). Without a generation, `oso dev`'s plain `exit`.
    pub(super) fn exit_for_restart(
        &mut self,
        event_loop: &super::shell::Loop<'_>,
        generation: Option<u64>,
    ) -> Reply {
        // A queue is not in the buffers' generation: it is looked at again.
        let allowed = carry::exit_allowed(&self.editor, generation).and_then(|()| {
            if generation.is_some() {
                self.refuse_unsent_queues()?;
                // A session started during `oso-restart` would be killed part way, and its
                // conversation is not the state's (ADR 0165, part 4c).
                if self.restart_going() && self.native_runs_or_starts() {
                    return Err("a session started during the restart".into());
                }
                self.exit_with_terminals()
            } else {
                Ok(())
            }
        });
        if allowed.is_err() {
            // That restart is over, and its snapshot need hold no text.
            self.forget_export();
        }
        allowed.map(|()| {
            self.leave(event_loop, true);
            json!({ "exiting": true })
        })
    }

    /// `check-handover`: with `terminals`, whether the running terminals can go too, which
    /// only the parent may ask; without, a running terminal holds the restart.
    pub(super) fn check_handover(&self, terminals: bool, parent: bool) -> Reply {
        if terminals && !parent {
            return Err(super::terminals::carry::NOT_PARENT.into());
        }
        if self.terminals_coming() {
            return Err(
                "cannot hand over: the terminals carried here wait for their masters".into(),
            );
        }
        self.refuse_unsent_queues()?;
        let answer = carry::check(&self.editor, terminals)?;
        if terminals {
            let running: Vec<BufferId> = self.editor.running_terminals().collect();
            self.terminals_may_go(&running)?;
        }
        Ok(answer)
    }

    /// What a thread of the restart of terminals found.
    pub(super) fn carry_event(&mut self, event: super::terminals::carry::CarryEvent) {
        match event {
            super::terminals::carry::CarryEvent::Encoded => self.carry_terminals(),
            super::terminals::carry::CarryEvent::Adopted(buffer, adopted) => {
                self.terminal_adopted(buffer, adopted);
            }
        }
    }

    /// Forgets the snapshot of the latest export, as the restart it was for is over.
    pub(super) fn forget_export(&mut self) {
        pieces::drop_later(self.handover.exported.take());
    }

    /// Forgets a snapshot `export-text` may no longer read, and pieces staged for an import
    /// that has stopped coming, off this thread. Run on each control request.
    pub(super) fn drop_stale_texts(&mut self) {
        if self
            .handover
            .exported
            .as_ref()
            .is_some_and(|e| e.is_stale())
        {
            self.forget_export();
        }
        self.handover.staging.drop_stale();
    }

    /// `export-text` and `import-text`. Only the lookup happens here: a background thread
    /// copies or builds the piece, and replies. Returns the reply if there is one now.
    pub(super) fn carry_text(
        &mut self,
        op: Op,
        reply: mpsc::Sender<Reply>,
        parent: bool,
    ) -> Option<(mpsc::Sender<Reply>, Reply)> {
        self.touch_carried();
        match op {
            Op::ExportText {
                export,
                item,
                offset,
            } => {
                // A terminal's state goes only to the parent (the author's choice).
                if !parent && self.is_terminal_item(export, item) {
                    let refused = super::terminals::carry::NOT_PARENT.to_owned();
                    return Some((reply, Err(refused)));
                }
                let job = self
                    .handover
                    .exported
                    .as_ref()
                    .ok_or_else(|| "nothing has been exported".to_owned())
                    .and_then(|exported| exported.piece(export, item, offset));
                match job {
                    Ok(job) => job.spawn(reply),
                    Err(message) => return Some((reply, Err(message))),
                }
            }
            Op::ImportText(piece) => self.handover.staging.spawn(piece, carry::MAX_ITEMS, reply),
            _ => return Some((reply, Err("not a method that carries text".into()))),
        }
        None
    }

    /// `dev-message`: what `oso dev` has to say, in the echo area and `*Messages*`.
    #[cfg(feature = "dev")]
    pub(super) fn dev_message(&mut self, text: String) -> Reply {
        self.editor.message(text);
        self.request_redraw();
        Ok(Value::Null)
    }
}
