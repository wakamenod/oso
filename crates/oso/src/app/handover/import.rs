//! The import of a restart, on the UI thread: taking over from the instance that exported its
//! state (ADR 0047, ADR 0055, ADR 0090, ADR 0133). Its files are opened again, each judged anew,
//! its texts put in, its terminals placed and its windows taken, and `import-state` is answered
//! once every carried file has a verdict.
//!
//! Must not depend on: sockets or threads. The texts were built off this thread.

use std::sync::mpsc;

use oso_core::buffer::{BufferId, Verdict};
use oso_core::frame::{FrameSize, Tiling};
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use serde_json::json;

use super::super::App;
use super::ImportReply;
use crate::carry::{self, Reply};

impl App {
    /// `import-state`, replying now if it fails, and otherwise once every carried file has
    /// been judged, so that the emergency save covers it before the old instance goes
    /// (ADR 0062). The reply's generation is the one right after the import, which lets the
    /// supervisor close this instance while nothing has changed since.
    pub(in crate::app) fn answer_import(
        &mut self,
        state: carry::State,
        reply: mpsc::Sender<Reply>,
        parent: bool,
    ) -> Option<(mpsc::Sender<Reply>, Reply)> {
        if !parent && !state.terminals.is_empty() {
            let refused = super::super::terminals::carry::NOT_PARENT.to_owned();
            return Some((reply, Err(refused)));
        }
        // Which instances a bridge follows here, and which native sessions resume, are the
        // parent's to say (ADR 0165, part 4c).
        let mut state = state;
        if !parent {
            state.follows.clear();
            state.native_resumes.clear();
        }
        let judging = match self.import_state(state) {
            Ok(judging) => judging,
            Err(message) => return Some((reply, Err(message))),
        };
        let result = json!({
            "buffers": self.editor.buffers().count(),
            "generation": carry::generation(&self.editor),
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
    pub(in crate::app) fn reply_to_import(&mut self) {
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
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the state's buffers, a few lines for each kind of entry: files, terminals and listings"
    )]
    fn import_state(&mut self, mut state: carry::State) -> Result<Vec<BufferId>, String> {
        let texts = self.handover.staging.take(state.export, &state.items);
        carry::import_allowed(&self.editor)?;
        let mut texts: Vec<_> = texts?.into_iter().map(Some).collect();
        let mut take = |item: usize| texts[item].take().ok_or("an item is claimed twice");
        // What could fail is worked out before anything changes.
        let kills = state
            .kills
            .iter()
            .map(|kill| split(take(kill.item)?.text(), &kill.pieces))
            .collect::<Result<Vec<_>, _>>()?;
        let carried_terminals = std::mem::take(&mut state.terminals);
        let follows = std::mem::take(&mut state.follows);
        let native_resumes = std::mem::take(&mut state.native_resumes);
        let native_agents = std::mem::take(&mut state.native_agents);
        let histories = state
            .histories
            .iter()
            .map(|history| {
                let parts = split(take(history.item)?.text(), &history.lengths)?;
                Ok((history.name.as_str(), parts))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let terminal_texts = carried_terminals
            .iter()
            .map(|terminal| take(terminal.item).map(|text| text.text().clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut carried = Vec::new();
        for entry in &state.buffers {
            carried.push(match &entry.carried {
                Some(text) => Some((take(text.item)?, text.facts.clone())),
                None => None,
            });
        }
        let scratch = self.editor.window().buffer();
        // Each of the state's buffers as this instance names it, for the windows.
        let mut named = Vec::with_capacity(state.buffers.len());
        let mut added = Vec::new();
        let mut judging = Vec::new();
        let mut scratch_text = None;
        let mut stand_for = Vec::new();
        for (entry, carried) in std::mem::take(&mut state.buffers).into_iter().zip(carried) {
            let (opened_by_agent, wraps, waits) = (entry.opened_by_agent, entry.wraps, entry.waits);
            if let Some(listing) = entry.listing {
                named.push(self.listing_handed_over(listing, &mut added));
                continue;
            }
            // A carried terminal's window, once its buffer is made below.
            if let Some(terminal) = entry.terminal {
                stand_for.push((named.len(), terminal));
                named.push(scratch);
                continue;
            }
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
                // What waited for the person still does: a restart lets nothing go (ADR 0118).
                if let Some(buffer) = self.editor.buffer_mut(buffer) {
                    buffer.carry_waits(waits.open, waits.save);
                    if waits.open_let_go {
                        buffer.carry_open_let_go();
                    }
                }
            }
            carry::restore_wraps(&mut self.editor, buffer, wraps);
            named.push(buffer);
        }
        // The terminals only once nothing can fail: their threads start here.
        self.place_terminals(carried_terminals, terminal_texts, &stand_for, &mut named);
        self.handover.follows = follows;
        // They resume once the instance before has gone (ADR 0165, part 4c).
        self.natives_of_agents_carried(native_agents);
        self.natives_carried(native_resumes);
        if let (Some((text, facts)), Some(buffer)) = (scratch_text, self.editor.buffer_mut(scratch))
        {
            buffer.apply_carried(text, facts);
        }
        self.carry_ring_and_histories(kills, histories);
        let reviews = std::mem::take(&mut state.reviews);
        let order: Vec<BufferId> = (state.order.iter())
            .filter_map(|&at| named.get(at).copied())
            .collect();
        self.take_view(&named, state);
        // After the windows, whose shows and selection would raise their buffers (ADR 0171).
        self.editor.restore_buffer_order(&order);
        // Each once its project is known and trusted (ADR 0122, step 6).
        self.reviews_handed_over(reviews);
        Ok(judging)
    }

    /// Adopts the `terminals` a state carries, `texts` their states, and has each window that
    /// stood for one, by its place in `named`, show its buffer (ADR 0120, step 11).
    fn place_terminals(
        &mut self,
        terminals: Vec<carry::terminals::TerminalState>,
        texts: Vec<Rope>,
        stand_for: &[(usize, usize)],
        named: &mut [BufferId],
    ) {
        let adopted = self.adopt_terminals(terminals, texts);
        for &(at, terminal) in stand_for {
            if let (Some(&buffer), Some(slot)) = (adopted.get(terminal), named.get_mut(at)) {
                *slot = buffer;
            }
        }
    }

    /// Puts in the kill ring's entries and the histories a state carried.
    fn carry_ring_and_histories(
        &mut self,
        kills: Vec<Vec<Rope>>,
        histories: Vec<(&str, Vec<Rope>)>,
    ) {
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
    }

    /// Lays the frame out as the state's windows were, each showing its buffer of `named` with
    /// its positions, in a window of the state's size and place, and feeds back its pending
    /// keys (ADR 0112).
    fn take_view(&mut self, named: &[BufferId], state: carry::State) {
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
        // They win over the configuration file's, which a look not yet applied takes as the
        // baseline without applying them (ADR 0108).
        self.configuration.hand_over();
        // Whether buffers are highlighted (ADR 0111).
        if let Some(on) = state.font_lock {
            self.editor.set_font_lock(on);
        }
        // Whether `RET` indents (ADR 0115).
        if let Some(on) = state.electric_indent {
            self.editor.set_electric_indent(on);
        }
        // Before positions, so that what the aids lay out is there first (ADR 0097).
        if let Some(aids) = state.visual_aids {
            self.editor.set_visual_aids(aids);
        }
        self.take_windows(named, state.windows);
        self.try_restore_positions();
        self.dispatcher
            .restore_pending(&mut self.editor, &state.pending_keys);
        // A theme this build does not have leaves the one it read at its start.
        if let Some(theme) = state
            .theme
            .filter(|theme| self.editor.theme_name() != Some(theme))
        {
            let _ = self.editor.load_theme(&theme);
        }
        self.request_redraw();
    }

    /// Splits the frame as the state's layout says, shows each window's buffer, keeps each
    /// window's positions to restore, and selects the window that was selected. Each window is
    /// shown and restored where it is, none selected but the last, so that taking many costs
    /// one layout each. A layout this editor cannot take, or one of more windows than its frame
    /// can show, leaves one window, the state's first (ADR 0112).
    fn take_windows(&mut self, named: &[BufferId], windows: carry::Windows) {
        let first = self.editor.tiled().0.id();
        let room = room_for_windows(self.editor.frame().size());
        let ids = match windows.layout {
            ref layout @ Tiling::Run { .. } if layout.windows() <= room => self
                .editor
                .set_tiling(layout)
                .unwrap_or_else(|_| vec![first]),
            _ => vec![first],
        };
        self.handover.restore.clear();
        for (id, entry) in ids.iter().zip(windows.windows) {
            let Some(&buffer) = named.get(entry.shown) else {
                continue;
            };
            self.editor.show_in(*id, buffer);
            let untouched = self
                .editor
                .window_by_id(*id)
                .map(|window| (window.selections.clone(), window.scroll()));
            self.handover.restore.push(carry::Restore {
                window: *id,
                buffer,
                selections: entry.selections,
                scroll: entry.scroll,
                top_is_row: entry.top_is_row,
                untouched,
            });
        }
        let selected = ids.get(windows.selected).copied().unwrap_or(first);
        self.editor.select_window(selected);
    }

    /// Applies positions from a handoff if their files have loaded far enough, and forgets
    /// those that no longer stand. Returns whether any window moved, so a frame is drawn.
    pub(in crate::app) fn try_restore_positions(&mut self) -> bool {
        let editor = &mut self.editor;
        let mut moved = false;
        self.handover.restore.retain(|restore| {
            let stands = restore.stands(editor);
            let over = restore.try_apply(editor);
            moved |= over && stands;
            !over
        });
        moved
    }
}

/// The most windows a frame of `size` can show, each with a row of text, a modeline and a
/// column of text beside a divider (ADR 0112): a state's layout of more is not taken.
fn room_for_windows(size: FrameSize) -> usize {
    let rows = size.rows.saturating_sub(1) / 2;
    let columns = size.columns.div_ceil(2);
    rows.max(1).saturating_mul(columns.max(1))
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

#[cfg(test)]
mod tests {
    use super::room_for_windows;
    use oso_core::frame::FrameSize;

    #[test]
    fn a_frame_takes_no_more_windows_than_it_can_show() {
        let room = |rows, columns| room_for_windows(FrameSize { rows, columns });
        // 23 rows above the echo area: 11 windows a column; 80 columns: 40 a row.
        assert_eq!(room(24, 80), 440);
        assert_eq!(room(0, 0), 1, "one, always");
        assert_eq!(room(3, 1), 1);
    }
}
