//! Restarts under `oso dev`, on the UI thread: exporting the state and the texts it
//! carries, and taking over from the instance that exported one (ADR 0047, ADR 0055,
//! ADR 0090).
//!
//! Must not depend on: sockets or threads. The handoff's carry jobs copy and build the
//! texts off this thread, and this only clones ropes and installs what they built.

use std::sync::mpsc;

use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use serde_json::{Value, json};

use super::App;
use crate::dev;
use crate::dev::handoff;

impl App {
    /// `export-state`: the state, with an O(1) snapshot of each text it carries kept for
    /// `export-text`, replacing the one before.
    pub(super) fn export_state(&mut self) -> dev::Reply {
        let number = self.exports + 1;
        let (state, exported) = handoff::export(
            &self.editor,
            self.dispatcher.pending(),
            self.frame(),
            self.restore.as_ref(),
            number,
        )?;
        self.exports = number;
        self.exported = Some(exported);
        Ok(state)
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
                    .exported
                    .as_ref()
                    .ok_or_else(|| "nothing has been exported".to_owned())
                    .and_then(|exported| exported.piece(export, item, offset));
                match job {
                    Ok(job) => job.spawn(reply),
                    Err(message) => return Some((reply, Err(message))),
                }
            }
            dev::UiOp::ImportText(piece) => self.staging.spawn(piece, handoff::MAX_ITEMS, reply),
            _ => return Some((reply, Err("not a method that carries text".into()))),
        }
        None
    }

    /// Takes over from the instance that exported `state`: opens its files again, each
    /// judged anew, puts in the texts it carried, and moves the window. A reply means every
    /// carried text is in its buffer and every other file has begun to load, which is what
    /// lets the old instance exit. Refused without changing anything if this instance has
    /// opened something already or has been edited, or if a text has not all arrived.
    pub(super) fn import_state(&mut self, state: handoff::State) -> dev::Reply {
        let texts = self.staging.take(state.export, &state.items);
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
        for (index, (entry, carried)) in state.buffers.into_iter().zip(carried).enumerate() {
            let buffer = match (entry.path, carried) {
                (Some(path), None) => self
                    .visit(path)
                    .ok_or("a file could not begin to load, so the old instance should stay")?,
                (Some(path), Some((text, facts))) => self.visit_carried(path, text, facts),
                (None, Some((text, facts))) => {
                    if let Some(buffer) = self.editor.buffer_mut(scratch) {
                        buffer.apply_carried(text, facts);
                    }
                    scratch
                }
                (None, None) => scratch,
            };
            if index == state.shown {
                shown = buffer;
            }
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
        self.restore = Some(handoff::Restore {
            buffer: shown,
            selections: state.selections,
            scroll: state.scroll,
        });
        self.try_restore_positions();
        self.dispatcher
            .restore_pending(&mut self.editor, &state.pending_keys);
        self.request_redraw();
        Ok(json!({ "buffers": self.editor.buffers().count() }))
    }

    /// Applies positions from a handoff if their file has loaded far enough.
    /// Returns whether this is over: restored, or the window has moved on.
    pub(super) fn try_restore_positions(&mut self) -> bool {
        let done = self
            .restore
            .as_ref()
            .is_some_and(|restore| restore.try_apply(&mut self.editor));
        if done {
            self.restore = None;
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
            if text.clip_offset(end, Bias::Left) != end {
                return Err("a part of an item ends inside a character".to_owned());
            }
            let part = text.slice(start..end);
            start = end;
            Ok(part)
        })
        .collect()
}
