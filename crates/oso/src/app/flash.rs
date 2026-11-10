//! The cursor flash's clock (ADR 0097): when a flash began, how strong it is now and when
//! its next frame is due. Whether an input flashes is the editor's (`Editor::flashes`), asked
//! by the dispatcher after each input; this only keeps time.
//!
//! Must not depend on: the renderer or the window. [`Flash`] is told the time, so it is
//! tested with no clock.

use std::time::{Duration, Instant};

use oso_core::editor::Editor;
use oso_render::FlashFill;

use super::App;

/// How long a flash fades.
const FADE: Duration = Duration::from_millis(400);

/// How long after a frame of the flash is drawn the next may be asked for: every other frame
/// of a 60 Hz display. Every 16 ms ran ahead of the display, filled the swap chain, and a
/// key's frame then waited about two refreshes for room (ADR 0097).
const FRAME: Duration = Duration::from_millis(33);

/// How early a wake may come and still count as the frame's. The timer's date passes
/// through a float, so it can fire a little before the deadline it was given, and the loop
/// would then sleep with the same deadline set again.
const EARLY: Duration = Duration::from_millis(1);

/// A flash fading, if one is. Its frames never queue behind each other: the next is asked
/// for only once the last is drawn, and later by as long as drawing it took, which is long
/// when the display is slow to give a frame room. So a key's frame finds room at any refresh
/// rate.
#[derive(Debug, Default)]
pub(super) struct Flash {
    /// When it began, until the frame that clears it is asked for.
    began: Option<Instant>,
    /// When its next frame may be asked for, once the last one is drawn.
    due: Option<Instant>,
}

impl Flash {
    /// Begins a flash at `now`, in place of any other. The input that began it asks for its
    /// first frame.
    pub(super) fn begin(&mut self, now: Instant) {
        self.began = Some(now);
        self.due = None;
    }

    /// Ends a flash. Returns whether one was on screen, so a frame clears it.
    pub(super) fn end(&mut self) -> bool {
        let was_on = self.began.is_some();
        *self = Self::default();
        was_on
    }

    /// Its strength in a frame drawn at `now`, from 1 down to 0, or none once it has faded.
    pub(super) fn strength(&self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.began?);
        (elapsed < FADE).then(|| 1.0 - elapsed.as_secs_f32() / FADE.as_secs_f32())
    }

    /// A frame was drawn from `started` to `now`: the next may be asked for a [`FRAME`]
    /// later, and later again by as long as this one took.
    pub(super) fn drawn(&mut self, started: Instant, now: Instant) {
        if self.began.is_some() {
            self.due = Some(now + FRAME + now.saturating_duration_since(started));
        }
    }

    /// Whether a frame should be asked for at `now`. None is while the last is not drawn
    /// yet. Once it has faded, the frame that clears it is the last, drawn or not, so a
    /// window that draws nothing still sees it end.
    pub(super) fn frame_due(&mut self, now: Instant) -> bool {
        let Some(began) = self.began else {
            return false;
        };
        if now + EARLY >= began + FADE {
            self.end();
            return true;
        }
        if self.due.is_some_and(|due| now + EARLY >= due) {
            self.due = None;
            return true;
        }
        false
    }

    /// When the loop should wake for it: its next frame, or while one is not drawn yet, the
    /// end of the fade.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        let began = self.began?;
        Some(self.due.unwrap_or(began + FADE).min(began + FADE))
    }
}

impl App {
    /// After an input the dispatcher handled: the cursor flashes if the view jumped, and a
    /// flash fading ends if it did not.
    pub(super) fn flash_after_input(&mut self) {
        let redraw = match self.dispatcher.take_jump() {
            Some(true) => {
                self.flash.begin(Instant::now());
                true
            }
            Some(false) => self.flash.end(),
            None => false,
        };
        if redraw {
            self.request_redraw();
        }
    }

    /// After an agent showed a buffer: the cursor flashes if the view `jumped`, so the person
    /// sees where it went. Not the person's input, so it ends no flash.
    pub(super) fn flash_after_agent(&mut self, jumped: bool) {
        if jumped {
            self.flash.begin(Instant::now());
            self.request_redraw();
        }
    }

    /// Ends the flash, as when the window is hidden.
    pub(super) fn end_flash(&mut self) {
        self.flash.end();
    }

    /// Asks for the flash's next frame once it is due, while one fades.
    pub(super) fn draw_flash_if_due(&mut self) {
        if self.flash.frame_due(Instant::now()) {
            self.request_redraw();
        }
    }
}

/// The flash's cells in the tiled window and its strength, for a frame drawn at `now`.
pub(super) fn fill(editor: &Editor, flash: &Flash, now: Instant) -> Option<FlashFill> {
    let strength = flash.strength(now)?;
    let cells = editor.flash_cells()?;
    Some(FlashFill { cells, strength })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flash_fades_over_its_time_and_one_frame_after_clears_it() {
        let start = Instant::now();
        let mut flash = Flash::default();
        assert!(!flash.frame_due(start + FADE));
        assert_eq!(flash.next_wake(), None, "at rest it asks for nothing");
        flash.begin(start);
        assert_eq!(flash.strength(start), Some(1.0));
        let half = flash.strength(start + FADE / 2).unwrap();
        assert!((half - 0.5).abs() < 1e-3, "{half}");
        assert_eq!(flash.strength(start + FADE), None);
        assert!(flash.frame_due(start + FADE), "the frame that clears it");
        assert_eq!(flash.next_wake(), None);
        assert!(!flash.end(), "it had ended");
    }

    #[test]
    fn the_next_frame_waits_for_the_last_to_be_drawn() {
        let start = Instant::now();
        let mut flash = Flash::default();
        flash.begin(start);
        assert!(
            !flash.frame_due(start + FRAME * 2),
            "the first is not drawn yet"
        );
        assert_eq!(flash.next_wake(), Some(start + FADE));
        let drawn = start + FRAME * 2;
        flash.drawn(drawn - Duration::from_millis(1), drawn);
        let due = drawn + FRAME + Duration::from_millis(1);
        assert_eq!(flash.next_wake(), Some(due));
        assert!(!flash.frame_due(due - EARLY * 2));
        assert!(
            flash.frame_due(due - EARLY / 2),
            "a wake a little early counts"
        );
        assert!(!flash.frame_due(due), "asked for once");
    }

    #[test]
    fn a_frame_slow_to_find_room_spaces_out_the_next() {
        let start = Instant::now();
        let mut flash = Flash::default();
        flash.begin(start);
        let slow = Duration::from_millis(30);
        flash.drawn(start, start + slow);
        assert_eq!(flash.next_wake(), Some(start + slow + FRAME + slow));
    }

    #[test]
    fn ending_a_flash_says_whether_one_was_on() {
        let start = Instant::now();
        let mut flash = Flash::default();
        flash.begin(start);
        assert!(flash.end());
        assert_eq!(flash.next_wake(), None);
        assert_eq!(flash.strength(start), None);
        flash.drawn(start, start);
        assert_eq!(
            flash.next_wake(),
            None,
            "a frame drawn after it ended asks for none"
        );
    }
}
