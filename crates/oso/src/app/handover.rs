//! Restarts under `oso dev`, on the UI thread: exporting the state and the texts it
//! carries, and taking over from the instance that exported one (ADR 0047, ADR 0055,
//! ADR 0090).
//!
//! Must not depend on: sockets or threads. The handoff's carry jobs copy and build the
//! texts off this thread, and this only clones ropes and installs what they built.

use std::sync::mpsc;

use oso_core::buffer::{BufferId, Verdict};
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use serde_json::{Value, json};

use super::App;
use crate::dev;
use crate::dev::handoff::{self, carry};

/// What a restart under `oso dev` keeps on the UI thread.
#[derive(Default)]
pub(super) struct Handover {
    /// Positions handed over from the previous instance, until their file has loaded.
    pub(super) restore: Option<handoff::Restore>,
    /// How many states this instance has exported, which names the latest.
    exports: u64,
    /// The texts the latest export carries, for `export-text`.
    exported: Option<carry::Exported>,
    /// The texts `import-text` has brought for the import that follows.
    staging: carry::Staging,
    /// The reply to an import that waits for its carried buffers to be judged.
    import: Option<ImportReply>,
}

/// An import's reply, held until every carried buffer has a verdict.
struct ImportReply {
    reply: mpsc::Sender<dev::Reply>,
    result: Value,
    judging: Vec<BufferId>,
}

impl App {
    /// `export-state`: the state, with an O(1) snapshot of each text it carries kept for
    /// `export-text`, replacing the one before.
    pub(super) fn export_state(&mut self) -> dev::Reply {
        let number = self.handover.exports + 1;
        let (state, exported) = handoff::export(
            &self.editor,
            self.dispatcher.pending(),
            self.frame(),
            self.handover.restore.as_ref(),
            number,
        )?;
        self.handover.exports = number;
        carry::drop_later(self.handover.exported.replace(exported));
        Ok(state)
    }

    /// Forgets the snapshot of the latest export, as the restart it was for is over.
    pub(super) fn forget_export(&mut self) {
        carry::drop_later(self.handover.exported.take());
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
        &self,
        op: dev::UiOp,
        reply: mpsc::Sender<dev::Reply>,
    ) -> Option<(mpsc::Sender<dev::Reply>, dev::Reply)> {
        match op {
            dev::UiOp::ExportText {
                export,
                item,
                offset,
            } => {
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
            dev::UiOp::ImportText(piece) => {
                self.handover
                    .staging
                    .spawn(piece, handoff::MAX_ITEMS, reply)
            }
            _ => return Some((reply, Err("not a method that carries text".into()))),
        }
        None
    }

    /// `import-state`, replying now if it fails, and otherwise once every carried file has
    /// been judged, so that the emergency save covers it before the old instance goes
    /// (ADR 0062). The reply's generation is the one right after the import, which lets the
    /// supervisor close this instance while nothing has changed since.
    pub(super) fn answer_import(
        &mut self,
        state: handoff::State,
        reply: mpsc::Sender<dev::Reply>,
    ) -> Option<(mpsc::Sender<dev::Reply>, dev::Reply)> {
        let judging = match self.import_state(state) {
            Ok(judging) => judging,
            Err(message) => return Some((reply, Err(message))),
        };
        let result = json!({
            "buffers": self.editor.buffers().count(),
            "generation": handoff::generation(&self.editor),
        });
        self.handover.import = Some(ImportReply {
            reply,
            result,
            judging,
        });
        self.reply_to_import();
        None
    }

    /// Sends the import's reply once every carried file has a verdict, or has gone.
    pub(super) fn reply_to_import(&mut self) {
        let judged = self.handover.import.as_ref().is_some_and(|import| {
            import.judging.iter().all(|&id| {
                self.editor
                    .buffer(id)
                    .is_none_or(|buffer| buffer.verdict() != Verdict::Unjudged)
            })
        });
        if let Some(import) = self.handover.import.take_if(|_| judged) {
            // The old instance goes once this replies, and the text is then only here.
            self.dispatcher.publish_for_emergency(&self.editor);
            let _ = import.reply.send(Ok(import.result));
        }
    }

    /// Takes over from the instance that exported `state`: opens its files again, each
    /// judged anew, puts in the texts it carried, and moves the window. Returns the carried
    /// buffers whose paths are being judged. Then every carried text is in its buffer and
    /// every other file has begun to load. Refused without changing anything if this
    /// instance has opened something already or has been edited, or if a text has not all
    /// arrived, and left clean if a file cannot begin to load or be judged.
    fn import_state(&mut self, mut state: handoff::State) -> Result<Vec<BufferId>, String> {
        let texts = self.handover.staging.take(state.export, &state.items);
        handoff::import_allowed(&self.editor)?;
        let mut texts: Vec<_> = texts?.into_iter().map(Some).collect();
        let mut take = |item: usize| texts[item].take().ok_or("an item is claimed twice");
        // What could fail is worked out before anything changes.
        let kills = state
            .kills
            .iter()
            .map(|kill| split(take(kill.item)?.text(), &kill.pieces))
            .collect::<Result<Vec<_>, _>>()?;
        let histories = state
            .histories
            .iter()
            .map(|history| {
                let parts = split(take(history.item)?.text(), &history.lengths)?;
                Ok((history.name.as_str(), parts))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut carried = Vec::new();
        for entry in &state.buffers {
            carried.push(match &entry.carried {
                Some(text) => Some((take(text.item)?, text.facts.clone())),
                None => None,
            });
        }
        let scratch = self.editor.window().buffer();
        let mut shown = scratch;
        let mut added = Vec::new();
        let mut judging = Vec::new();
        let mut scratch_text = None;
        for (index, (entry, carried)) in std::mem::take(&mut state.buffers)
            .into_iter()
            .zip(carried)
            .enumerate()
        {
            let opened_by_agent = entry.opened_by_agent;
            let buffer = match (entry.path, carried) {
                // The old instance held it, so it opens without asking, whatever its size
                // (ADR 0087).
                (Some(path), None) => self.visit(path, u64::MAX),
                (Some(path), Some((text, facts))) => {
                    let buffer = self.visit_carried(path, text, facts);
                    judging.extend(buffer);
                    buffer
                }
                (None, carried) => {
                    scratch_text = carried.or(scratch_text);
                    Some(scratch)
                }
            };
            let Some(buffer) = buffer else {
                // Back to clean, so that this instance holds no text and can go.
                added.into_iter().for_each(|id| self.editor.kill_buffer(id));
                return Err(
                    "a file could not begin to load, so the old instance should stay".into(),
                );
            };
            if buffer != scratch {
                added.push(buffer);
                // Its saves still ask the person, as before the restart (ADR 0085).
                if opened_by_agent && let Some(buffer) = self.editor.buffer_mut(buffer) {
                    buffer.mark_opened_by_agent();
                }
            }
            if index == state.shown {
                shown = buffer;
            }
        }
        if let (Some((text, facts)), Some(buffer)) = (scratch_text, self.editor.buffer_mut(scratch))
        {
            buffer.apply_carried(text, facts);
        }
        self.editor.carry_kills(
            kills
                .into_iter()
                .map(|pieces| pieces.into_iter().map(Into::into).collect())
                .collect(),
        );
        for (name, answers) in histories {
            let answers = answers.iter().map(Rope::to_string);
            self.editor.carry_history(name, answers);
        }
        self.take_view(shown, state);
        Ok(judging)
    }

    /// Shows `shown` in a window of the state's size and place, with its positions, and
    /// feeds back its pending keys.
    fn take_view(&mut self, shown: BufferId, state: handoff::State) {
        self.editor.show(shown);
        // The size first, so that positions are restored against it. A system that applies
        // it at once says what it came to, with no `Resized`. Most apply it later, so the
        // view takes the size asked for now, and a `Resized` event corrects this if the
        // window ends up another size.
        if let (Some(view), Some(frame)) = (&mut self.view, state.frame) {
            if let Some((x, y)) = frame.position {
                view.window.set_outer_position(x, y);
            }
            let (width, height) = view
                .window
                .request_inner_size(frame.width, frame.height)
                .unwrap_or((frame.width, frame.height));
            self.resize(width, height);
        }
        self.handover.restore = Some(handoff::Restore {
            buffer: shown,
            selections: state.selections,
            scroll: state.scroll,
        });
        self.try_restore_positions();
        self.dispatcher
            .restore_pending(&mut self.editor, &state.pending_keys);
        self.request_redraw();
    }

    /// Applies positions from a handoff if their file has loaded far enough.
    /// Returns whether this is over: restored, or the window has moved on.
    pub(super) fn try_restore_positions(&mut self) -> bool {
        let done = self
            .handover
            .restore
            .as_ref()
            .is_some_and(|restore| restore.try_apply(&mut self.editor));
        if done {
            self.handover.restore = None;
        }
        done
    }

    /// `dev-message`: what `oso dev` has to say, in the echo area and `*Messages*`.
    pub(super) fn dev_message(&mut self, text: String) -> dev::Reply {
        self.editor.message(text);
        self.request_redraw();
        Ok(Value::Null)
    }
}

/// `text` cut into parts of `lengths`, each on a character boundary. O(the parts × log n).
fn split(text: &Rope, lengths: &[usize]) -> Result<Vec<Rope>, String> {
    let mut start = 0;
    lengths
        .iter()
        .map(|len| {
            let end = start + len;
            if end > text.len() || text.clip_offset(end, Bias::Left) != end {
                return Err("a part of an item ends inside a character".to_owned());
            }
            let part = text.slice(start..end);
            start = end;
            Ok(part)
        })
        .collect()
}
