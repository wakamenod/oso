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
//!   oso's own window layer, if it would go in as typed there (ADR 0077). Otherwise, and on
//!   winit, which cannot commit one, it waits for the composition to end, as the OS keeps a
//!   composition it was turned off in. A commit never takes a prefix argument.
//! - Nothing of a composition reaches a log but that one happened (ADR 0028).
//! - The window system's reports arrive as [`ImeEvent`], from winit's adapter or the native
//!   layer, and the calls go through its window (ADR 0070), so nothing here knows which.

use oso_core::buffer::Buffer;
use oso_core::display_map::Preedit;
use oso_core::frame::PixelRect;
use oso_platform::window::ImeEvent;

use super::App;
use super::shell::{COMMITS_WHEN_TURNED_OFF, Loop};

/// Whether oso wants the IME on in `buffer`: unless the next letter is a key, or
/// cannot type (ADR 0069), or a prompt is selected (ADR 0072) or isearch is on (ADR 0082),
/// which `reads_input` says. Menus add their own reasons here.
fn wants_ime(buffer: &Buffer, reads_keys: bool, reads_input: bool) -> bool {
    // Letters answer among the copies (ADR 0064).
    let answers = buffer.mode().shows_copies();
    // So that a letter fails at once, and not after a whole composition.
    let refuses = buffer.read_only().is_some();
    // As macOS password fields do.
    let secret = buffer.is_secret();
    !(answers || refuses || secret || reads_keys || reads_input)
}

/// What to allow the IME as now, if that changes. Turning it off during a composition
/// commits it first when it `commits` there (ADR 0077), and otherwise waits for the
/// composition to end, as the OS keeps it (ADR 0069).
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
    /// The composition as last laid out for the window (ADR 0077): its text, empty with none,
    /// its first cell, and where each of its characters is from that cell's left.
    laid_out: String,
    first: Option<PixelRect>,
    positions: Vec<f32>,
    /// The next layout is worked out here, so that a key of a composition allocates nothing.
    scratch: Vec<f32>,
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
                let redraw = self.commit(event_loop, &text);
                ended || redraw
            }
        }
    }

    /// What follows every event, before the next is read, as the window system hands a key to the
    /// input method before it reaches oso (ADR 0069): a composition ends if a buffer that
    /// may hold secrets is selected, whatever selected it, then the IME is allowed as
    /// wanted, and the window gets the cursor's cell if it changed, so that a composition
    /// started by the next key opens its candidate window there, and where each character of
    /// a composition is drawn.
    pub(super) fn after_event(&mut self, event_loop: &Loop<'_>) {
        // What follows logs, and the event may have brought a secret buffer into view.
        self.note_logging();
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
        self.give_preedit_layout();
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
        let reads_input = self.editor.prompt().is_some() || self.editor.isearch().is_some();
        wants_ime(
            self.editor.current().1,
            self.dispatcher.reads_keys(),
            reads_input,
        )
    }

    /// Allows or forbids the IME as wanted, if that changed. Turning it off during a
    /// composition commits it first on oso's own window layer, where it would go in as typed
    /// and oso is not leaving, and otherwise waits for it to end, as on winit's (ADR 0077).
    pub(super) fn sync_ime(&mut self, event_loop: &Loop<'_>) {
        let composing = self.composing();
        let commits = COMMITS_WHEN_TURNED_OFF
            && composing
            && self.dispatcher.types_text(&self.editor)
            && !self.is_leaving(event_loop);
        let next = next_allowed(self.ime.allowed, self.ime_wanted(), composing, commits);
        let Some(wanted) = next else {
            return;
        };
        if !wanted && composing {
            self.commit_composition(event_loop);
            // The commit may have changed what is wanted. The IME stays on then, and the
            // window's input method drops what it holds, so that it is not committed twice.
            if self.ime_wanted() {
                if let Some(view) = &self.view {
                    view.window.discard_composition();
                }
                return;
            }
        }
        let Some(view) = &self.view else {
            return;
        };
        // Turning it off also drops what the window's input method holds.
        view.window.set_ime_allowed(wanted);
        self.ime.allowed = wanted;
        log::debug!("IME allowed: {wanted}");
    }

    /// Commits the composition as it is shown, where typing goes now (ADR 0077).
    fn commit_composition(&mut self, event_loop: &Loop<'_>) {
        let Some(text) = self
            .editor
            .preedit()
            .map(|preedit| preedit.text().to_owned())
        else {
            return;
        };
        self.end_composition();
        self.commit(event_loop, &text);
    }

    /// Puts committed `text` in where typing goes, through typing's checks, as the input
    /// method's commits go. Returns whether it asked for a frame.
    fn commit(&mut self, event_loop: &Loop<'_>, text: &str) -> bool {
        log::trace!("commit");
        let effects = self.dispatcher.commit_text(&mut self.editor, text);
        self.note_logging();
        self.apply(event_loop, effects);
        effects.redraw
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

    /// Gives the window where the composition is drawn, so that the input method's question
    /// about a range of it gets that place, and its candidate window follows the segment being
    /// converted (ADR 0077). It is worked out after each event during a composition, as the
    /// view may scroll or the grid change, and given only when it changed. None once the
    /// composition ends.
    fn give_preedit_layout(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let mut positions = std::mem::take(&mut self.ime.scratch);
        let first = self
            .editor
            .preedit_layout(view.render_state.pixel_grid(), &mut positions);
        let text = first
            .and(self.editor.preedit())
            .map_or("", |preedit| preedit.text());
        let ime = &mut self.ime;
        if first != ime.first || positions != ime.positions || text != ime.laid_out {
            let laid_out = first.map(|first| (text, first, positions.as_slice()));
            view.window.set_ime_preedit(laid_out);
            ime.first = first;
            ime.laid_out.clear();
            ime.laid_out.push_str(text);
            std::mem::swap(&mut ime.positions, &mut positions);
        }
        ime.scratch = positions;
    }
}

#[cfg(test)]
mod tests {
    use super::{next_allowed, wants_ime};
    use oso_core::buffer::Buffer;

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
            "a prompt or isearch reads keys and digits"
        );
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
