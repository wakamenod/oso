//! Work that goes on a slice a turn of the loop, and the input held while it does:
//! query-replace's `!` and a keyboard macro's replay (ADR 0113), and an undo or a redo of a
//! large step (ADR 0119). Each slice asks for a redraw, and the redraw brings the next turn;
//! a window hidden from view draws no frame, so the loop also wakes on its own meanwhile.
//!
//! Must not depend on: winit. The slices' own logic stays in the editor and the dispatcher;
//! this only runs them and hands on what they leave.

use std::time::{Duration, Instant};

use super::App;
use super::shell::Loop;

/// How long a keyboard macro's replay runs at each turn of the loop (ADR 0113).
const REPLAY_TURN: Duration = Duration::from_millis(4);

/// The longest the next slice waits for a frame, about one at 60 Hz: a window hidden from view
/// draws none, and work over turns would stop with it.
const SLICE_EVERY: Duration = Duration::from_millis(16);

impl App {
    /// Runs a slice of whatever goes on over turns, before the loop waits. `C-g` since the last
    /// turn dropped the held wheel with the held keys; once nothing holds input, the wheel held
    /// meanwhile scrolls.
    pub(super) fn run_slices(&mut self, event_loop: &Loop<'_>) {
        let cancelled = self.dispatcher.take_cancelled();
        if cancelled {
            self.held_wheel.clear();
        }
        self.replace_slice(event_loop);
        let ended = self.undo_slice(event_loop);
        self.agent_undo_turn(ended, cancelled);
        self.run_held_wheel();
        self.replay_macro(event_loop);
    }

    /// When the loop wakes for the next slice with no frame, while work goes on over turns.
    pub(super) fn slice_deadline(&self) -> Option<Instant> {
        (crate::dispatch::holds_input(&self.editor) || self.editor.macro_replaying())
            .then(|| Instant::now() + SLICE_EVERY)
    }

    /// Replaces `!`'s next slice, if one is under way, and asks for another turn. Once it has
    /// ended, the input held meanwhile runs.
    fn replace_slice(&mut self, event_loop: &Loop<'_>) {
        if !self.editor.replace_works() {
            return;
        }
        self.editor.replace_continue();
        self.dispatcher.publish_for_emergency(&self.editor);
        self.start_searches();
        self.request_redraw();
        if !self.editor.replace_works() {
            let effects = self.dispatcher.run_held_during_all(&mut self.editor);
            self.apply(event_loop, effects);
        }
    }

    /// Undoes or redoes the next slice of a large step, if one is under way, and asks for
    /// another turn. Once it has ended, the input held meanwhile runs. Returns how it ended, if
    /// it did this turn.
    fn undo_slice(
        &mut self,
        event_loop: &Loop<'_>,
    ) -> Option<Result<(), oso_core::buffer::EditError>> {
        self.editor.crossing()?;
        let (effects, ended) = self.dispatcher.go_on_crossing(&mut self.editor);
        self.dispatcher.publish_for_emergency(&self.editor);
        self.apply(event_loop, effects);
        ended
    }

    /// Scrolls as the wheel did while `!` or an undo went on, once nothing holds input and the
    /// keys held with it have run (ADR 0119). A window gone meanwhile scrolls nothing.
    fn run_held_wheel(&mut self) {
        if self.held_wheel.is_empty() || crate::dispatch::holds_input(&self.editor) {
            return;
        }
        let mut moved = false;
        for (window, rows, columns) in std::mem::take(&mut self.held_wheel) {
            moved |= rows != 0 && self.editor.scroll_view(window, rows);
            moved |= columns != 0 && self.editor.scroll_view_sideways(window, columns);
        }
        if moved {
            self.request_redraw();
        }
    }

    /// Feeds a keyboard macro's replay for a few milliseconds, and asks for another turn while
    /// it runs (ADR 0113).
    fn replay_macro(&mut self, event_loop: &Loop<'_>) {
        if !self.editor.macro_replaying() {
            return;
        }
        let until = Instant::now() + REPLAY_TURN;
        let effects = self.dispatcher.replay(&mut self.editor, until);
        self.apply(event_loop, effects);
        self.request_redraw();
    }
}
