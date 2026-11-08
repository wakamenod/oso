//! The input method (ADR 0069): a composition as the window system reports it, whether oso
//! wants the IME on, and where the OS puts its candidate window.
//!
//! Must not depend on: what a key does, or the render pipeline beyond the cell
//! grid in pixels. A commit goes through the dispatcher, as typing does, and the
//! preedit is the editor's.
//!
//! - oso keeps its own state and never reads it back from the events: `Disabled`
//!   also comes unasked, when the input source changes.
//! - Turning the IME off during a composition commits it first, where typing goes then, on
//!   oso's own window layer (ADR 0077). winit cannot commit one, and the OS keeps a
//!   composition it was turned off in, so there it waits for the composition to end.
//! - Nothing of a composition reaches a log but that one happened (ADR 0028).
//! - The window system's reports arrive as [`ImeEvent`], from winit's adapter or the native
//!   layer, and the calls go through its window (ADR 0070), so nothing here knows which.

use oso_core::buffer::Buffer;
use oso_core::display_map::Preedit;
use oso_core::frame::{CellRect, PixelRect};
use oso_platform::window::ImeEvent;

use super::App;
use super::shell::{COMMITS_WHEN_TURNED_OFF, Loop};

/// Whether oso wants the IME on in `buffer`: unless the next letter is a key, or
/// cannot type (ADR 0069), or a prompt is selected (ADR 0072). Isearch and menus add their
/// own reasons here.
fn wants_ime(buffer: &Buffer, reads_keys: bool, in_prompt: bool) -> bool {
    // Letters answer among the copies (ADR 0064).
    let answers = buffer.mode().shows_copies();
    // So that a letter fails at once, and not after a whole composition.
    let refuses = buffer.read_only().is_some();
    // As macOS password fields do.
    let secret = buffer.is_secret();
    !(answers || refuses || secret || reads_keys || in_prompt)
}

/// What to allow the IME as now, if that changes. Turning it off during a composition
/// commits it first where the window layer `commits` (ADR 0077), and otherwise waits for the
/// composition to end, as winit keeps it (ADR 0069).
fn next_allowed(allowed: bool, wanted: bool, composing: bool, commits: bool) -> Option<bool> {
    (wanted != allowed && (wanted || !composing || commits)).then_some(wanted)
}

/// What oso has told the OS about the input method.
#[derive(Default)]
pub(super) struct ImeState {
    /// Whether the IME was last allowed.
    allowed: bool,
    /// The cursor area last given, with the scale factor it was given at, since the window
    /// system keeps it in logical units.
    area: Option<(PixelRect, f64)>,
    /// Where the composition is drawn, as last given: the x of each of its UTF-16 units from
    /// the cursor area's left, then of its end. Empty with no composition.
    offsets: Vec<f32>,
    /// The next offsets are worked out here, so that a key of a composition allocates nothing.
    scratch: Vec<f32>,
}

/// Where `preedit` is drawn: into `out`, the x of each of its UTF-16 units from its start,
/// then of its end, as `cells_wide` gives the width of so many cells. A unit takes its
/// cluster's place, as the input method's ranges count units (ADR 0077).
fn preedit_offsets(preedit: &Preedit, cells_wide: impl Fn(usize) -> f32, out: &mut Vec<f32>) {
    out.clear();
    for (byte, char) in preedit.text().char_indices() {
        let x = cells_wide(preedit.column_of(byte));
        out.extend(std::iter::repeat_n(x, char.len_utf16()));
    }
    out.push(cells_wide(preedit.cells()));
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
    pub(super) fn after_event(&mut self, event_loop: &Loop<'_>) {
        if self.composing() && self.editor.current().1.is_secret() {
            self.end_composition();
            // The window's input method too, so that nothing of it is committed later.
            if let Some(view) = &self.view {
                view.window.discard_composition();
            }
        }
        self.sync_ime(event_loop);
        self.sync_secure_input();
        self.give_cursor_area();
        self.give_preedit_offsets();
    }

    /// Asks the window for secure input while the selected buffer may hold secrets, as a
    /// password field has it (ADR 0073). Only the verdict crosses: no text, name or place of
    /// the buffer. The window holds it only while oso is in front with the keyboard, and does
    /// nothing when the answer is the same.
    pub(super) fn sync_secure_input(&self) {
        if let Some(view) = &self.view {
            view.window
                .set_secure_input(self.editor.current().1.is_secret());
        }
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
        let in_prompt = self.editor.prompt().is_some();
        wants_ime(
            self.editor.current().1,
            self.dispatcher.reads_keys(),
            in_prompt,
        )
    }

    /// Allows or forbids the IME as wanted, if that changed. Turning it off during a
    /// composition commits it first on oso's own window layer, and waits for it to end on
    /// winit's (ADR 0077).
    pub(super) fn sync_ime(&mut self, event_loop: &Loop<'_>) {
        let composing = self.composing();
        let next = next_allowed(
            self.ime.allowed,
            self.ime_wanted(),
            composing,
            COMMITS_WHEN_TURNED_OFF,
        );
        let Some(wanted) = next else {
            return;
        };
        if !wanted && composing {
            self.commit_before_turning_off(event_loop);
        }
        let Some(view) = &self.view else {
            return;
        };
        view.window.set_ime_allowed(wanted);
        self.ime.allowed = wanted;
        log::debug!("IME allowed: {wanted}");
    }

    /// Commits the composition as it is shown, where typing goes now (ADR 0077), and has the
    /// window's input method drop its own, so that it is not committed twice.
    fn commit_before_turning_off(&mut self, event_loop: &Loop<'_>) {
        let Some(text) = self
            .editor
            .preedit()
            .map(|preedit| preedit.text().to_owned())
        else {
            return;
        };
        self.end_composition();
        if let Some(view) = &self.view {
            view.window.discard_composition();
        }
        log::trace!("commit");
        let effects = self.dispatcher.commit_text(&mut self.editor, &text);
        self.apply(event_loop, effects);
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

impl App {
    /// Gives the window where the composition is drawn, so that the input method's question
    /// about a range of it gets that place, and its candidate window follows the segment being
    /// converted (ADR 0077). Only when that changed, and none once the composition ends.
    fn give_preedit_offsets(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let mut offsets = std::mem::take(&mut self.ime.scratch);
        offsets.clear();
        if let Some(preedit) = self.editor.preedit() {
            let grid = view.render_state.pixel_grid();
            let cells_wide = |columns| {
                let cells = CellRect {
                    row: 0,
                    column: 0,
                    rows: 0,
                    columns,
                };
                grid.rect(cells).width
            };
            preedit_offsets(preedit, cells_wide, &mut offsets);
        }
        if offsets != self.ime.offsets {
            view.window.set_ime_preedit_offsets(&offsets);
            std::mem::swap(&mut self.ime.offsets, &mut offsets);
        }
        self.ime.scratch = offsets;
    }
}

#[cfg(test)]
mod tests {
    use super::{next_allowed, preedit_offsets, wants_ime};
    use oso_core::buffer::Buffer;
    use oso_core::display_map::Preedit;

    #[test]
    fn the_ime_is_wanted_only_where_a_letter_types() {
        let file = Buffer::from_text("notes.txt", "");
        assert!(wants_ime(&file, false, false));
        assert!(
            !wants_ime(&file, true, false),
            "a prefix key or argument reads keys"
        );
        assert!(
            !wants_ime(&Buffer::from_bytes("bin", b"\xff"), false, false),
            "read-only"
        );
        let unjudged = Buffer::visiting(".env".into());
        assert!(unjudged.is_secret());
        assert!(!wants_ime(&unjudged, false, false), "may hold secrets");
        assert!(
            !wants_ime(&file, false, true),
            "a prompt reads keys and digits"
        );
    }

    #[test]
    fn each_unit_of_the_composition_is_where_its_cluster_is_drawn() {
        let mut out = vec![99.0];
        // A wide letter, a narrow one, and an emoji of two UTF-16 units and two cells.
        let preedit = Preedit::new("かa😀".into(), None);
        preedit_offsets(&preedit, |columns| columns as f32 * 8.0, &mut out);
        assert_eq!(out, [0.0, 16.0, 24.0, 24.0, 40.0]);
        preedit_offsets(&Preedit::new(String::new(), None), |_| 1.0, &mut out);
        assert_eq!(out, [1.0], "only the end");
    }

    #[test]
    fn turning_off_waits_for_the_composition_and_turning_on_does_not() {
        for commits in [false, true] {
            assert_eq!(next_allowed(true, false, false, commits), Some(false));
            assert_eq!(next_allowed(false, true, true, commits), Some(true));
            assert_eq!(next_allowed(false, true, false, commits), Some(true));
            assert_eq!(next_allowed(true, true, true, commits), None);
            assert_eq!(next_allowed(false, false, false, commits), None);
        }
        assert_eq!(next_allowed(true, false, true, false), None, "winit waits");
        assert_eq!(
            next_allowed(true, false, true, true),
            Some(false),
            "the native layer commits, then turns it off"
        );
    }
}
