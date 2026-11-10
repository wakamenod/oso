//! The cursor flash's clock (ADR 0097, ADR 0105): when a flash began, how strong it is now
//! and when its next frame is due. Whether an input flashes is the editor's
//! (`Editor::flashes`), asked by the dispatcher after each input; this only keeps time.
//!
//! Must not depend on: the renderer or the window. [`Flash`] is told the time, so it is
//! tested with no clock.

use std::time::{Duration, Instant};

use oso_core::editor::Editor;
use oso_render::FlashFill;

use super::App;

/// When each step of a flash begins, from the jump, and its strength: full at first, then
/// two steps down, then gone. Its frames are these three only, and none comes in the first
/// 150 ms, so keys pressed fast, as when `C-v` is held, end it before any. A frame drawn
/// shortly before a key's makes that key's wait for the display (ADR 0105).
const STEPS: [(Duration, Option<f32>); 3] = [
    (Duration::from_millis(150), Some(2.0 / 3.0)),
    (Duration::from_millis(275), Some(1.0 / 3.0)),
    (Duration::from_millis(400), None),
];

/// How early a wake may come and still count as the step's. The timer's date passes
/// through a float, so it can fire a little before the deadline it was given, and the loop
/// would then sleep with the same deadline set again.
const EARLY: Duration = Duration::from_millis(1);

/// A flash on screen, if one is.
#[derive(Debug, Default)]
pub(super) struct Flash {
    /// When it began, until the frame that clears it is asked for.
    began: Option<Instant>,
    /// How many of [`STEPS`] have had their frame asked for.
    stepped: usize,
}

impl Flash {
    /// Begins a flash at `now`, in place of any other. The input that began it asks for its
    /// first frame.
    pub(super) fn begin(&mut self, now: Instant) {
        self.began = Some(now);
        self.stepped = 0;
    }

    /// Ends a flash. Returns whether one was on screen, so a frame clears it.
    pub(super) fn end(&mut self) -> bool {
        let was_on = self.began.is_some();
        *self = Self::default();
        was_on
    }

    /// Its strength in a frame drawn at `now`, from 1 down, or none once it has gone.
    pub(super) fn strength(&self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.began?);
        STEPS
            .iter()
            .rev()
            .find(|(at, _)| elapsed >= *at)
            .map_or(Some(1.0), |&(_, strength)| strength)
    }

    /// Whether a frame should be asked for at `now`: once per step, the last of which clears
    /// it, drawn or not, so a window that draws nothing still sees it end.
    pub(super) fn frame_due(&mut self, now: Instant) -> bool {
        let Some(due) = self.next_wake() else {
            return false;
        };
        if now + EARLY < due {
            return false;
        }
        self.stepped += 1;
        if self.stepped == STEPS.len() {
            self.end();
        }
        true
    }

    /// When the loop should wake for its next step.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        let (at, _) = STEPS.get(self.stepped)?;
        Some(self.began? + *at)
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

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_flash_holds_then_steps_down_in_three_frames() {
        let start = Instant::now();
        let mut flash = Flash::default();
        assert!(!flash.frame_due(start + ms(500)));
        assert_eq!(flash.next_wake(), None, "at rest it asks for nothing");
        flash.begin(start);
        assert_eq!(flash.strength(start + ms(149)), Some(1.0));
        assert_eq!(flash.next_wake(), Some(start + ms(150)));
        assert!(
            !flash.frame_due(start + ms(100)),
            "no frame in the first 150 ms"
        );
        let mut frames = Vec::new();
        for t in (0..=450).step_by(5) {
            if flash.frame_due(start + ms(t)) {
                frames.push((t, flash.strength(start + ms(t))));
            }
        }
        assert_eq!(
            frames,
            [(150, Some(2.0 / 3.0)), (275, Some(1.0 / 3.0)), (400, None)]
        );
        assert_eq!(flash.next_wake(), None);
        assert!(!flash.end(), "the last step ended it");
    }

    #[test]
    fn a_wake_a_little_early_is_the_steps() {
        let start = Instant::now();
        let mut flash = Flash::default();
        flash.begin(start);
        assert!(flash.frame_due(start + ms(150) - EARLY / 2));
        assert_eq!(flash.next_wake(), Some(start + ms(275)));
    }

    #[test]
    fn ending_a_flash_says_whether_one_was_on() {
        let start = Instant::now();
        let mut flash = Flash::default();
        flash.begin(start);
        assert!(flash.end());
        assert_eq!(flash.next_wake(), None);
        assert_eq!(flash.strength(start), None);
    }
}
