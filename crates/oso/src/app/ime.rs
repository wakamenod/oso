//! The input method (ADR 0069): a composition as the window system reports it, whether oso
//! wants the IME on, and where the OS puts its candidate window.
//!
//! Must not depend on: what a key does, or the render pipeline beyond the cell
//! grid in pixels. A commit goes through the dispatcher, as typing does, and the
//! preedit is the editor's.
//!
//! - oso keeps its own state and never reads it back from the events: `Disabled`
//!   also comes unasked, when the input source changes.
//! - Turning the IME off waits while a composition is open, as the OS keeps a
//!   composition it was turned off in, and the next key would bring it back.
//! - Nothing of a composition reaches a log but that one happened (ADR 0028).
//! - The window system's reports arrive as [`ImeEvent`], from `shell`, and the calls go
//!   through its window (ADR 0070), so nothing here knows winit.

use std::ops::Range;

use oso_core::buffer::Buffer;
use oso_core::display_map::Preedit;
use oso_core::frame::PixelRect;

use super::App;
use super::shell::Loop;

/// What an input method reports, from the window system or from the dev socket's stand-in.
pub(super) enum ImeEvent {
    /// The text being composed, and the caret or the segment being converted, in
    /// bytes. No text ends the composition with no commit.
    Preedit(String, Option<Range<usize>>),
    /// The text the composition gave, which comes after an empty preedit.
    Commit(String),
    /// The composition is over, whether oso asked for it or not.
    Disabled,
}

/// Whether oso wants the IME on in `buffer`: unless the next letter is a key, or
/// cannot type (ADR 0069). The minibuffer, isearch and menus add their own reasons
/// here.
fn wants_ime(buffer: &Buffer, reads_keys: bool) -> bool {
    // Letters answer among the copies (ADR 0064).
    let answers = buffer.mode().shows_copies();
    // So that a letter fails at once, and not after a whole composition.
    let refuses = buffer.read_only().is_some();
    // As macOS password fields do.
    let secret = buffer.is_secret();
    !(answers || refuses || secret || reads_keys)
}

/// What to allow the IME as now, if that changes: turning it off waits while a
/// composition is open.
fn next_allowed(allowed: bool, wanted: bool, composing: bool) -> Option<bool> {
    (wanted != allowed && (wanted || !composing)).then_some(wanted)
}

/// What oso has told the OS about the input method.
#[derive(Default)]
pub(super) struct ImeState {
    /// Whether the IME was last allowed.
    allowed: bool,
    /// The cursor area last given, with the scale factor it was given at, since the window
    /// system keeps it in logical units.
    area: Option<(PixelRect, f64)>,
}

impl App {
    /// Handles a report of the input method, from the window system or the dev socket.
    /// Returns whether it asked for a frame.
    pub(super) fn ime_event(&mut self, event_loop: &Loop<'_>, event: ImeEvent) -> bool {
        match event {
            ImeEvent::Preedit(text, range) if !text.is_empty() => {
                if self.editor.preedit().is_none() {
                    self.dispatcher.begin_composition(&mut self.editor);
                }
                self.editor.set_preedit(Some(Preedit::new(text, range)));
                self.request_redraw();
                true
            }
            ImeEvent::Preedit(..) | ImeEvent::Disabled => self.end_composition(),
            ImeEvent::Commit(text) => {
                let ended = self.end_composition();
                log::trace!("commit");
                let effects = self.dispatcher.commit_text(&mut self.editor, &text);
                self.apply(event_loop, effects);
                ended || effects.redraw
            }
        }
    }

    /// What follows every event, before the next is read, as the window system hands a key to the
    /// input method before it reaches oso (ADR 0069): a composition ends if a buffer that
    /// may hold secrets is selected, whatever selected it, then the IME is allowed as
    /// wanted, and the OS gets the cursor's cell if it changed, so that a composition
    /// started by the next key opens its candidate window there.
    pub(super) fn after_event(&mut self) {
        if self.composing() && self.editor.current().1.is_secret() {
            self.end_composition();
        }
        self.sync_ime();
        self.give_cursor_area();
    }

    /// Drops the preedit, if one is shown. Returns whether one was.
    fn end_composition(&mut self) -> bool {
        let shown = self.editor.preedit().is_some();
        if shown {
            self.editor.set_preedit(None);
            self.request_redraw();
        }
        shown
    }

    /// Whether a composition is open.
    fn composing(&self) -> bool {
        self.editor.preedit().is_some()
    }

    /// Whether the IME was last allowed.
    #[cfg(feature = "dev")]
    pub(super) fn ime_allowed(&self) -> bool {
        self.ime.allowed
    }

    /// The cursor area last given to the OS, in physical pixels.
    #[cfg(feature = "dev")]
    pub(super) fn ime_area(&self) -> Option<PixelRect> {
        self.ime.area.map(|(area, _)| area)
    }

    /// Whether oso wants the IME on now ([`wants_ime`]).
    pub(super) fn ime_wanted(&self) -> bool {
        wants_ime(self.editor.current().1, self.dispatcher.reads_keys())
    }

    /// Allows or forbids the IME as wanted, if that changed. Turning it off waits for
    /// a composition to end.
    pub(super) fn sync_ime(&mut self) {
        let next = next_allowed(self.ime.allowed, self.ime_wanted(), self.composing());
        let (Some(wanted), Some(view)) = (next, &self.view) else {
            return;
        };
        view.window.set_ime_allowed(wanted);
        self.ime.allowed = wanted;
        log::debug!("IME allowed: {wanted}");
    }

    /// Gives the OS the primary cursor's cell, for its candidate window, if it moved
    /// since the last time. Not before the first frame is presented, as the first call
    /// costs a few milliseconds then, not during a composition, where it moves nothing,
    /// and not while the IME is off, as in a buffer that may hold secrets.
    fn give_cursor_area(&mut self) {
        if self.composing() || !self.ime.allowed {
            return;
        }
        let Some(view) = self
            .view
            .as_ref()
            .filter(|view| view.render_state.presented_frames() > 0)
        else {
            return;
        };
        let Some(area) = self.editor.cursor_area(view.render_state.pixel_grid()) else {
            return;
        };
        let scale = view.window.scale_factor();
        if self.ime.area == Some((area, scale)) {
            return;
        }
        view.window.set_ime_cursor_area(area);
        self.ime.area = Some((area, scale));
    }
}

#[cfg(test)]
mod tests {
    use super::{next_allowed, wants_ime};
    use oso_core::buffer::Buffer;

    #[test]
    fn the_ime_is_wanted_only_where_a_letter_types() {
        let file = Buffer::from_text("notes.txt", "");
        assert!(wants_ime(&file, false));
        assert!(
            !wants_ime(&file, true),
            "a prefix key or argument reads keys"
        );
        assert!(
            !wants_ime(&Buffer::from_bytes("bin", b"\xff"), false),
            "read-only"
        );
        let unjudged = Buffer::visiting(".env".into());
        assert!(unjudged.is_secret());
        assert!(!wants_ime(&unjudged, false), "may hold secrets");
    }

    #[test]
    fn turning_off_waits_for_the_composition_and_turning_on_does_not() {
        assert_eq!(next_allowed(true, false, false), Some(false));
        assert_eq!(next_allowed(true, false, true), None, "waits");
        assert_eq!(next_allowed(false, true, true), Some(true));
        assert_eq!(next_allowed(false, true, false), Some(true));
        assert_eq!(next_allowed(true, true, true), None);
        assert_eq!(next_allowed(false, false, false), None);
    }
}
