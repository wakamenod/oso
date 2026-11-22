//! Going back through the buffers a window showed: walking them with `M-,` and `M-/`, and
//! `q`, which buries the buffer it leaves (ADR 0088, ADR 0171).
//!
//! Must not depend on: the host's window, threads or I/O.

use super::Editor;
use super::switch::kept_as_users;
use crate::buffer::BufferId;
use crate::window::Walk;

impl Editor {
    /// Shows the buffer `count` before the one shown in the tiled window's order, the most
    /// recent first and then those it never showed, skipping internal buffers: `M-,`. A
    /// negative count goes the other way, as `M-/` does. Repeating it walks on through every
    /// buffer, as long as nothing else showed one. From the side window, the tree's window
    /// selected last walks, and is selected (ADR 0121). A step raises nothing in the order of
    /// the buffers, as `previous-buffer` records nothing (ADR 0171).
    ///
    /// # Errors
    ///
    /// With no other buffer to show.
    pub fn walk_buffers(&mut self, count: isize) -> Result<(), &'static str> {
        let index = self.tree_index();
        let walk = match self.windows[index].walk() {
            Some(walk) => walk.clone(),
            None => {
                let ring = self.walk_order(index);
                // From an internal buffer, which the walk leaves out, as if from just before
                // the most recent of the user's: `M-,` goes there.
                let on_internal = ring.first() != Some(&self.windows[index].buffer());
                let at = if on_internal && count > 0 {
                    ring.len().saturating_sub(1)
                } else {
                    0
                };
                if ring.is_empty() {
                    return Err("No other buffer");
                }
                Walk { ring, at }
            }
        };
        let len = walk.ring.len();
        let step = count.rem_euclid(isize::try_from(len).unwrap_or(isize::MAX));
        let step = usize::try_from(step).unwrap_or(0);
        let at = (walk.at + step) % len;
        let next = walk.ring[at];
        if next == self.windows[index].buffer() {
            return Err("No other buffer");
        }
        self.window_moves_on();
        self.leave_side();
        self.window_shows_unrecorded(index, next);
        self.windows[index].places_mut().set_walk(Walk {
            ring: walk.ring,
            at,
        });
        Ok(())
    }

    /// `quit-window`: shows in the selected window the buffer it showed before, or, with none,
    /// the most recent other one a person goes to, and sends the buffer it leaves to the end of
    /// the order, as Emacs's `q` buries it (ADR 0171). Does nothing with no other buffer.
    pub fn quit_window(&mut self) {
        let index = self.tree_index();
        let here = self.windows[index].buffer();
        let listed = self.buffers_by_recency();
        let set: std::collections::HashSet<BufferId> = listed.iter().copied().collect();
        let places = self.windows[index].places().map(|(id, _)| id);
        let Some(back) = places
            .chain(listed.iter().copied())
            .find(|id| *id != here && set.contains(id))
        else {
            return;
        };
        self.window_moves_on();
        self.leave_side();
        self.window_shows_unrecorded(index, back);
        self.bury_buffer(here);
    }

    /// The buffers a walk goes through from the window at `index`: the one it shows, those it
    /// showed before, the most recent first, then the others, the least recently used first, as
    /// `previous-buffer` has them (ADR 0171). Internal buffers are left out.
    fn walk_order(&self, index: usize) -> Vec<BufferId> {
        let window = &self.windows[index];
        let mut ring = Vec::new();
        let shown = std::iter::once(window.buffer());
        let shown_before = shown.chain(window.places().map(|(id, _)| id));
        let rest = self.walk_rest();
        // A walk the agent channel runs never reaches a terminal, where the person's next keys
        // would go to its program (ADR 0120).
        let agent = self.runs_for_agent();
        let by_id = self.buffers_by_id();
        let mut seen = std::collections::HashSet::new();
        for id in shown_before.chain(rest) {
            let kept = by_id.get(&id).is_some_and(|buffer| {
                let users = !buffer.is_internal() || kept_as_users(buffer);
                users && !(agent && buffer.is_terminal())
            });
            if kept && seen.insert(id) {
                ring.push(id);
            }
        }
        ring
    }
}
