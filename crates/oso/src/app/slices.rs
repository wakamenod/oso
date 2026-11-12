//! Work that goes on a slice a turn of the loop, and the input held while it does:
//! query-replace's `!` and a keyboard macro's replay (ADR 0113), and an undo or a redo of a
//! large step (ADR 0119). While one goes on, the loop turns again at once rather than waiting
//! for a frame, and a frame is drawn only when what it shows has moved on: the share done, a
//! step committed, or the work ended (ADR 0119 step 3, Q8). Input is still taken between turns.
//!
//! Must not depend on: winit. The slices' own logic stays in the editor and the dispatcher;
//! this only runs them and hands on what they leave.

use std::time::{Duration, Instant};

use super::App;
use super::shell::Loop;

/// How long a keyboard macro's replay runs at each turn of the loop (ADR 0113).
const REPLAY_TURN: Duration = Duration::from_millis(4);

/// The longest the screen goes without a frame while work goes on, as when `!` has no count
/// yet to show a share of.
const FRAME_EVERY: Duration = Duration::from_millis(100);

/// When work over turns last drew a frame, and the share it showed.
#[derive(Debug, Default)]
pub(super) struct Pace {
    drawn: Option<(Instant, Option<Shown>)>,
}

/// What a frame shows of work over turns: the steps of a count left, and the whole percent
/// done of the current one.
type Shown = (usize, u32);

impl Pace {
    /// Whether work showing `shown` now needs a frame, at `now`: when the share shown moved
    /// on, or [`FRAME_EVERY`] passed. Notes the frame if so.
    fn due(&mut self, shown: Option<Shown>, now: Instant) -> bool {
        let due = match self.drawn {
            None => true,
            Some((at, before)) => before != shown || now.duration_since(at) >= FRAME_EVERY,
        };
        if due {
            self.drawn = Some((now, shown));
        }
        due
    }

    /// The work ended: the next begins with a frame.
    fn end(&mut self) {
        self.drawn = None;
    }
}

/// A share from 0 to 1 in whole percents.
fn percent(done: f32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a share from 0 to 1 in whole percents"
    )]
    let percent = (done.clamp(0.0, 1.0) * 100.0).floor() as u32;
    percent
}

impl App {
    /// Runs a slice of whatever goes on over turns, before the loop waits. `C-g` since the last
    /// turn dropped the held wheel with the held keys; once nothing holds input, the wheel held
    /// meanwhile scrolls.
    pub(super) fn run_slices(&mut self, event_loop: &Loop<'_>) {
        let cancelled = self.dispatcher.take_cancelled();
        if cancelled.is_some() {
            self.held_wheel.clear();
            // A close request held meanwhile survives `C-g`, and asks now.
            let effects = self.dispatcher.run_held_during_all(&mut self.editor);
            self.apply(event_loop, effects);
        }
        self.replace_slice(event_loop);
        let ended = self.undo_slice(event_loop);
        self.agent_undo_turn(ended, cancelled);
        self.run_held_wheel();
        self.replay_macro(event_loop);
        if self.slices_go_on() {
            if self.pace.due(self.shown(), Instant::now()) {
                self.request_redraw();
            }
        } else {
            self.pace.end();
        }
    }

    /// Whether work goes on over turns: `!`, an undo, or a replay not waiting for something
    /// else, whose end wakes the loop as it comes.
    pub(super) fn slices_go_on(&self) -> bool {
        crate::dispatch::holds_input(&self.editor) || self.dispatcher.replay_goes_on(&self.editor)
    }

    /// When the loop wakes for the next slice: at once while work goes on, with no frame to
    /// wait for, also in a window hidden from view.
    pub(super) fn slice_deadline(&self) -> Option<Instant> {
        self.slices_go_on().then(Instant::now)
    }

    /// The share of the work under way that a frame shows, if it has one.
    fn shown(&self) -> Option<Shown> {
        if let Some(crossing) = self.editor.crossing() {
            return Some((crossing.left(), percent(crossing.done())));
        }
        let replace = self.editor.replace()?;
        let (_, total) = replace.count()?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a share shown in whole percents"
        )]
        let done = replace.replaced() as f32 / total.max(1) as f32;
        Some((0, percent(done)))
    }

    /// Replaces `!`'s next slice, if one is under way. Once it has ended, the input held
    /// meanwhile runs, and a frame shows it.
    fn replace_slice(&mut self, event_loop: &Loop<'_>) {
        if !self.editor.replace_works() {
            return;
        }
        self.editor.replace_continue();
        self.dispatcher.publish_for_emergency(&self.editor);
        self.start_searches();
        if !self.editor.replace_works() {
            self.request_redraw();
            let effects = self.dispatcher.run_held_during_all(&mut self.editor);
            self.apply(event_loop, effects);
        }
    }

    /// Undoes or redoes the next slice of a large step, if one is under way. Once it has ended,
    /// the input held meanwhile runs. Returns how it ended, if it did this turn.
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

    /// Feeds a keyboard macro's replay for a few milliseconds (ADR 0113).
    fn replay_macro(&mut self, event_loop: &Loop<'_>) {
        if !self.editor.macro_replaying() {
            return;
        }
        let until = Instant::now() + REPLAY_TURN;
        let mut effects = self.dispatcher.replay(&mut self.editor, until);
        // What it changed shows in the frames the pace draws, and once it ends.
        effects.redraw &= !self.editor.macro_replaying();
        self.apply(event_loop, effects);
        if !self.editor.macro_replaying() {
            self.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{FRAME_EVERY, Pace, percent};

    #[test]
    fn a_frame_is_due_when_the_share_moves_on_or_long_after_the_last() {
        let mut pace = Pace::default();
        let now = Instant::now();
        assert!(pace.due(Some((0, 0)), now), "the first turn draws");
        assert!(!pace.due(Some((0, 0)), now + Duration::from_millis(5)));
        assert!(
            pace.due(Some((0, 1)), now + Duration::from_millis(6)),
            "a percent more"
        );
        assert!(
            pace.due(Some((1, 1)), now + Duration::from_millis(7)),
            "a step of a count"
        );
        assert!(
            pace.due(None, now + Duration::from_millis(8)),
            "no share any more"
        );
        let quiet = now + Duration::from_millis(9);
        assert!(!pace.due(None, quiet));
        assert!(
            pace.due(None, quiet + FRAME_EVERY),
            "and at least every 100 ms"
        );
        pace.end();
        assert!(
            pace.due(None, quiet + FRAME_EVERY),
            "the next work begins with one"
        );
        assert_eq!((percent(0.0), percent(0.429), percent(1.0)), (0, 42, 100));
    }
}
