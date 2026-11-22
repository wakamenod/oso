//! The order of the buffers for the whole editor, the most recently used first, as Emacs's
//! `buffer-list` (ADR 0171): `C-x b` lists by it, and `q`, a kill and `M-,` fall back to it
//! once a window's own places run out.
//!
//! Must not depend on: the host's window, threads or I/O.
//!
//! A buffer joins it at the end when it is made, as `get-buffer-create` has it. It rises when
//! the selected window comes to show it, or a window that shows it is selected, as
//! `select-window` records it, but not for a step of `M-,` or `M-/`, nor for what `q` or a kill
//! puts back in a window. The buffer a window leaves rises before the one it comes to, as Emacs
//! records the selected window's buffer again when a prompt closes, so the buffer an `M-,` walk
//! stopped on rises then. `q` sends the buffer it leaves to the end, as `bury-buffer` does.

use std::collections::HashMap;

use super::Editor;
use crate::buffer::{Buffer, BufferId};

/// Every buffer, internal ones too, the most recently used first.
#[derive(Clone, Debug, Default)]
pub(super) struct BufferOrder(Vec<BufferId>);

impl BufferOrder {
    /// Puts a new buffer at the end.
    pub(super) fn add(&mut self, id: BufferId) {
        self.0.push(id);
    }

    /// Moves `id` to the front, if it is in the order.
    fn raise(&mut self, id: BufferId) {
        let Some(at) = self.0.iter().position(|other| *other == id) else {
            return;
        };
        self.0[..=at].rotate_right(1);
    }

    /// Moves `id` to the end, if it is in the order.
    fn bury(&mut self, id: BufferId) {
        let Some(at) = self.0.iter().position(|other| *other == id) else {
            return;
        };
        self.0[at..].rotate_left(1);
    }

    /// Takes out `id`, which is gone.
    pub(super) fn forget(&mut self, id: BufferId) {
        self.0.retain(|other| *other != id);
    }

    /// The buffers of `first` that are in the order, once each and in that order, then the
    /// others as they were.
    fn restore(&mut self, first: &[BufferId]) {
        let mut order: Vec<BufferId> = Vec::with_capacity(self.0.len());
        for id in first {
            if self.0.contains(id) && !order.contains(id) {
                order.push(*id);
            }
        }
        let rest: Vec<BufferId> = (self.0.iter())
            .filter(|id| !order.contains(id))
            .copied()
            .collect();
        order.extend(rest);
        self.0 = order;
    }

    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = BufferId> + '_ {
        self.0.iter().copied()
    }
}

impl Editor {
    /// The buffers a person goes between, the most recently used first, for the whole editor:
    /// every buffer of the user's, `*Messages*`, and the list of emergency copies and `*Agent*`
    /// once made. A prompt's buffer and a copy being looked at are left out (ADR 0088,
    /// ADR 0171).
    pub fn buffers_by_recency(&self) -> Vec<BufferId> {
        let by_id = self.buffers_by_id();
        (self.buffer_order.iter())
            .filter(|id| by_id.get(id).is_some_and(|buffer| self.listed(*id, buffer)))
            .collect()
    }

    /// Every buffer by its id, so that a pass over the order looks each up at once rather than
    /// through the list, which would make it quadratic in the buffers.
    pub(super) fn buffers_by_id(&self) -> HashMap<BufferId, &Buffer> {
        (self.buffers.iter())
            .map(|(id, buffer)| (*id, buffer))
            .collect()
    }

    /// Orders the buffers as a restart carried them: those of `order` first, in its order,
    /// then the rest as they were (ADR 0171). Ids that name no buffer are passed over.
    pub fn restore_buffer_order(&mut self, order: &[BufferId]) {
        self.buffer_order.restore(order);
    }

    /// Sends `id` to the end of the order, as `bury-buffer` does (ADR 0171).
    pub(super) fn bury_buffer(&mut self, id: BufferId) {
        self.buffer_order.bury(id);
    }

    /// Where a kill sends the window at `index` off `going`, as `switch-to-prev-buffer` does
    /// with `kill`: the first of the user's buffers it showed before, then the most recent of
    /// the user's others it may fall back to, before `*scratch*` (ADR 0171). An internal buffer
    /// is reached by its own key.
    pub(super) fn kill_fallback(&self, index: usize, going: BufferId) -> Option<BufferId> {
        let by_id = self.buffers_by_id();
        let users = |id: &BufferId| {
            *id != going && by_id.get(id).is_some_and(|buffer| !buffer.is_internal())
        };
        (self.windows[index].places().map(|(id, _)| id))
            .find(users)
            .or_else(|| {
                (self.buffer_order.iter())
                    .filter(users)
                    .find(|id| by_id.get(id).is_some_and(|buffer| falls_back_to(buffer)))
            })
    }

    /// Raises what the window at `index` leaves and then `buffer`, which it is to show, when
    /// it is the selected one: a show other than a walk's step or a fallback (ADR 0171).
    pub(super) fn raise_shown(&mut self, index: usize, buffer: BufferId) {
        if index != self.tiled_index() {
            return;
        }
        self.buffer_order.raise(self.windows[index].buffer());
        self.buffer_order.raise(buffer);
    }

    /// Raises the buffer of the window at `index`, as it is selected (ADR 0171).
    pub(super) fn raise_selected(&mut self, index: usize) {
        if let Some(window) = self.windows.get(index) {
            let buffer = window.buffer();
            self.buffer_order.raise(buffer);
        }
    }

    /// The buffers past those a window showed, which a walk goes through last: the least
    /// recently used first, as `previous-buffer` goes through the reversed `buffer-list`, so
    /// that `M-/` reaches the most recent first (ADR 0171).
    pub(super) fn walk_rest(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.buffer_order.iter().rev()
    }
}

/// Whether a kill may put `buffer` in a window that never showed it, in place of `*scratch*`
/// (ADR 0171): not a terminal, whose program the person's next keys would reach (ADR 0120),
/// nor a file the agent channel is still opening (ADR 0087), nor one that may hold secrets or
/// is not judged yet, which would come on screen unasked (ADR 0044).
fn falls_back_to(buffer: &Buffer) -> bool {
    !buffer.is_terminal() && !buffer.agent_pending() && !buffer.verdict().is_withheld()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::frame::Way;

    /// An editor on `*scratch*` with three ordinary buffers, `a`, `b` and `c`, none shown yet.
    fn three() -> (Editor, [BufferId; 3]) {
        let mut editor = Editor::default();
        let ids = ["a", "b", "c"].map(|name| editor.add_buffer(Buffer::from_text(name, "x\n")));
        (editor, ids)
    }

    fn names(editor: &Editor) -> Vec<&str> {
        (editor.buffers_by_recency().into_iter())
            .filter_map(|id| Some(editor.buffer(id)?.name()))
            .collect()
    }

    fn shown(editor: &Editor) -> &str {
        editor.current().1.name()
    }

    #[test]
    fn a_buffer_rises_as_the_selected_window_shows_it_or_its_window_is_selected() {
        let (mut editor, [a, b, c]) = three();
        assert_eq!(names(&editor), ["*scratch*", "*Messages*", "a", "b", "c"]);
        editor.show(a);
        editor.show(b);
        assert_eq!(
            names(&editor),
            ["b", "a", "*scratch*", "*Messages*", "c"],
            "a buffer never shown stays where it was made, at the end"
        );
        let other = editor.split_window(Way::Down, false).unwrap();
        editor.show_in(other, c);
        assert_eq!(
            names(&editor),
            ["b", "a", "*scratch*", "*Messages*", "c"],
            "a window that is not selected raises nothing"
        );
        editor.select_window(other);
        assert_eq!(names(&editor), ["c", "b", "a", "*scratch*", "*Messages*"]);
    }

    #[test]
    fn a_walk_raises_nothing_until_the_window_shows_something_else() {
        let (mut editor, [a, b, c]) = three();
        let scratch = editor.buffer_named("*scratch*").unwrap();
        editor.show(a);
        editor.show(b);
        editor.show(c);
        editor.walk_buffers(1).unwrap();
        editor.walk_buffers(1).unwrap();
        assert_eq!(shown(&editor), "a");
        assert_eq!(names(&editor), ["c", "b", "a", "*scratch*", "*Messages*"]);
        // As Emacs records the window's buffer again when the next prompt closes.
        editor.show(scratch);
        assert_eq!(names(&editor), ["*scratch*", "a", "c", "b", "*Messages*"]);
    }

    #[test]
    fn a_walk_goes_past_the_window_s_buffers_least_recent_first() {
        let (mut editor, [a, b, c]) = three();
        editor.show(a);
        editor.restore_buffer_order(&[c, b]);
        // The window showed a and `*scratch*`; then b, the least recent of the rest, and c.
        let mut seen = Vec::new();
        for _ in 0..3 {
            editor.walk_buffers(1).unwrap();
            seen.push(shown(&editor).to_owned());
        }
        assert_eq!(seen, ["*scratch*", "b", "c"]);
        let (mut editor, [a, b, c]) = three();
        editor.show(a);
        editor.restore_buffer_order(&[c, b]);
        editor.walk_buffers(-1).unwrap();
        assert_eq!(shown(&editor), "c", "M-/ reaches the most recent first");
    }

    #[test]
    fn quitting_shows_the_buffer_before_and_buries_the_one_left() {
        let (mut editor, [a, b, _]) = three();
        editor.show(a);
        editor.show(b);
        editor.quit_window();
        assert_eq!(shown(&editor), "a");
        assert_eq!(names(&editor), ["a", "*scratch*", "*Messages*", "c", "b"]);
    }

    #[test]
    fn a_kill_falls_back_to_the_most_recent_buffer_before_a_new_scratch() {
        let (mut editor, [a, b, c]) = three();
        let scratch = editor.buffer_named("*scratch*").unwrap();
        editor.show(a);
        editor.kill_buffer(scratch);
        // The window showed nothing else, so the order says where it goes.
        editor.restore_buffer_order(&[c, b]);
        editor.kill_buffer(a);
        assert_eq!(shown(&editor), "c");
        assert_eq!(editor.buffer_named("*scratch*"), None, "none is made");
        assert_eq!(names(&editor), ["c", "b", "*Messages*"]);
    }

    fn order(ids: &[u64]) -> BufferOrder {
        BufferOrder(ids.iter().map(|id| BufferId(*id)).collect())
    }

    fn ids(order: &BufferOrder) -> Vec<u64> {
        order.iter().map(|id| id.0).collect()
    }

    #[test]
    fn raising_and_burying_move_one_buffer_and_keep_the_rest() {
        let mut o = order(&[1, 2, 3, 4]);
        o.raise(BufferId(3));
        assert_eq!(ids(&o), [3, 1, 2, 4]);
        o.bury(BufferId(1));
        assert_eq!(ids(&o), [3, 2, 4, 1]);
        o.raise(BufferId(9));
        o.bury(BufferId(9));
        assert_eq!(ids(&o), [3, 2, 4, 1], "a buffer not in it is not added");
        o.forget(BufferId(2));
        assert_eq!(ids(&o), [3, 4, 1]);
    }

    #[test]
    fn a_restored_order_puts_its_buffers_first_once_and_keeps_the_others() {
        let mut o = order(&[1, 2, 3, 4]);
        o.restore(&[BufferId(4), BufferId(9), BufferId(2), BufferId(4)]);
        assert_eq!(ids(&o), [4, 2, 1, 3]);
    }

    #[test]
    fn a_kill_falls_back_past_a_terminal_a_pending_open_and_a_secret() {
        let (mut editor, [a, b, c]) = three();
        let scratch = editor.buffer_named("*scratch*").unwrap();
        editor.show(a);
        editor.kill_buffer(scratch);
        editor.kill_buffer(c);
        let terminal = editor.add_terminal_buffer("*terminal*");
        let secret = editor.add_buffer(crate::editor::test_buffers::secret_buffer());
        let mut pending = Buffer::from_text("pending", "x\n");
        pending.set_agent_pending(true);
        let pending = editor.add_buffer(pending);
        editor.restore_buffer_order(&[terminal, pending, secret, b]);
        editor.kill_buffer(a);
        assert_eq!(
            shown(&editor),
            "b",
            "the first that *scratch* could stand for"
        );
        // With none, `*scratch*` comes back.
        editor.kill_buffer(b);
        assert_eq!(shown(&editor), "*scratch*");
    }

    #[test]
    fn what_a_fallback_shows_does_not_rise() {
        let (mut editor, [a, b, _]) = three();
        editor.show(a);
        editor.show(b);
        let messages = editor.buffer_named("*Messages*").unwrap();
        editor.restore_buffer_order(&[b, messages]);
        // b, `*Messages*`, a, `*scratch*`, c: `q` shows a, behind `*Messages*`, and leaves it there.
        editor.quit_window();
        assert_eq!(shown(&editor), "a");
        assert_eq!(names(&editor), ["*Messages*", "a", "*scratch*", "c", "b"]);
        editor.show(b);
        editor.restore_buffer_order(&[b, messages]);
        editor.kill_buffer(b);
        assert_eq!(shown(&editor), "a");
        assert_eq!(names(&editor), ["*Messages*", "a", "*scratch*", "c"]);
    }
}
