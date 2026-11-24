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
//! - The kind of input source the person types with is `source`'s (ADR 0182).

use oso_core::buffer::{Buffer, BufferId};
use oso_core::display_map::Preedit;
use oso_core::frame::PixelRect;
use oso_core::minibuffer::{Kind, Prompt, PromptId};
use oso_core::window::WindowId;
use oso_platform::window::{ImeEvent, InputSource};

mod source;

use std::time::Instant;

use super::App;
use super::sessions::Typing;
use super::shell::{ANSWERS_INPUT_METHOD, COMMITS_WHEN_TURNED_OFF, Loop};

/// Whether oso wants the IME on in `buffer`: unless the next letter is a key, which
/// `reads_keys` says, an open menu included (ADR 0117), or cannot type (ADR 0069), or a
/// prompt that takes no Japanese now is selected (ADR 0072, ADR 0158) or isearch is on
/// (ADR 0082), which `reads_input` says.
fn wants_ime(buffer: &Buffer, reads_keys: bool, reads_input: bool) -> bool {
    // Letters answer among the copies (ADR 0064).
    let answers = buffer.mode().shows_copies();
    // So that a letter fails at once, and not after a whole composition. A terminal is
    // read-only, but what is typed there goes to its program (ADR 0120).
    let refuses = buffer.read_only().is_some() && !buffer.is_terminal();
    // As macOS password fields do.
    let secret = buffer.is_secret();
    !(answers || refuses || secret || reads_keys || reads_input)
}

/// Whether the selected `prompt` takes Japanese now, when no window shows a buffer that may
/// hold secrets (`secret`, ADR 0106): a review comment's draft always, as a buffer does, and
/// a prompt that follows the input source once the person chose a Japanese input mode while
/// this very prompt was selected (`noted`) (ADR 0158). A number, a passphrase or a question
/// never does, and the next prompt starts without.
fn takes_japanese<I: PartialEq>(prompt: Option<(I, Kind)>, noted: Option<I>, secret: bool) -> bool {
    !secret
        && prompt.is_some_and(|(id, kind)| {
            kind == Kind::Draft || (follows_input_source(kind) && noted == Some(id))
        })
}

/// Whether a prompt of `kind` follows the input source (ADR 0106, ADR 0158): it has the input
/// source type ASCII while it is open, and takes Japanese once the person chooses it. Every
/// prompt that reads text or completes one does: C-x C-f's, query-replace's (ADR 0113), M-x's
/// and the others. A review comment's draft keeps the person's source and takes Japanese at
/// once ([`takes_japanese`]).
fn follows_input_source(kind: Kind) -> bool {
    matches!(kind, Kind::Complete { .. } | Kind::Text { .. })
}

/// The prompt a choice of `source` marks as taking Japanese: the selected one, for a
/// Japanese input mode, and none otherwise. oso's own switch to ASCII reads as `Other`, and its
/// giving the person's source back comes after the prompt has closed, so neither marks one.
fn marked<I>(source: InputSource, selected: Option<I>) -> Option<I> {
    match source {
        InputSource::Japanese => selected,
        InputSource::Other => None,
    }
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
    /// The excerpt of the cursor's line is worked out here and handed to the window, which
    /// leaves the one it held here, emptied at once and kept for its room. Never logged.
    excerpt: String,
    /// What the excerpt last given was worked out from ([`Editor::excerpt_stamp`]), or none
    /// while the IME is off.
    excerpt_for: Option<(BufferId, u64, usize)>,
    /// The prompt following the input source in which the person chose a Japanese input
    /// mode, which then takes Japanese until it closes or the person chooses another
    /// (ADR 0106, ADR 0158).
    japanese_prompt: Option<PromptId>,
    /// Whether the window was last asked for an ASCII input source (ADR 0106, ADR 0158).
    ascii: bool,
    /// The kind of input source the person types with (ADR 0182).
    pub(super) source: source::Source,
    /// The tiled window selected after the last event, or the one beneath the prompts, so
    /// that a composition ends when another is selected, as keys go only there (ADR 0112).
    selected: Option<WindowId>,
}

impl App {
    /// Handles a report of the input method, from the window system or the dev socket.
    /// Returns whether it asked for a frame.
    pub(super) fn ime_event(&mut self, event_loop: &Loop<'_>, event: ImeEvent) -> bool {
        self.person_typed(match &event {
            ImeEvent::Preedit(text, _) if text.is_empty() => "an empty preedit",
            ImeEvent::Preedit(..) => "a preedit",
            ImeEvent::Commit(_) => "a commit",
            ImeEvent::Disabled => "the IME's end",
        });
        // Typed as point came to a request's line, it goes to the composer (ADR 0182).
        let types = match &event {
            ImeEvent::Preedit(text, _) | ImeEvent::Commit(text) => !text.is_empty(),
            ImeEvent::Disabled => false,
        };
        if types && !self.composing() {
            let before = self.editor.tiled().0.id();
            let dropped = self.typed_as_point_came(Typing::Text);
            let now = self.editor.tiled().0.id();
            // It goes on there.
            if now != before {
                self.ime.selected = Some(now);
            }
            if dropped {
                if let Some(view) = &self.view {
                    view.window.discard_composition();
                }
                return true;
            }
        }
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

    /// The input method's part of what follows every event, before the next is read, as the
    /// window system hands a key to the input method before it reaches oso (ADR 0069): a
    /// composition ends if a buffer that may hold secrets is selected, or another window,
    /// whatever selected it (ADR 0112), then the IME is allowed as
    /// wanted, and the window gets the cursor's cell if it changed, so that a composition
    /// started by the next key opens its candidate window there, and where each character of
    /// a composition is drawn.
    pub(super) fn sync_ime_after_event(&mut self, event_loop: &Loop<'_>) {
        // Japanese chosen in a prompt goes with it: none is kept for the next (ADR 0106).
        let selected = self.editor.prompt().map(Prompt::id);
        if self.ime.japanese_prompt.is_some() && self.ime.japanese_prompt != selected {
            self.ime.japanese_prompt = None;
        }
        // What follows logs, and the event may have brought a secret buffer into view.
        self.note_logging();
        // The tiled window, so that a prompt opened over a composition keeps it, as it waits
        // there or commits where typing goes (ADR 0077).
        let window = self.editor.tiled().0.id();
        let moved = self
            .ime
            .selected
            .replace(window)
            .is_some_and(|last| last != window);
        if self.composing() && (moved || self.editor.current().1.is_secret()) {
            self.end_composition();
            // The window's input method too, so that nothing of it is committed later.
            if let Some(view) = &self.view {
                view.window.discard_composition();
            }
        }
        self.sync_ime(event_loop);
        // Before the OS hears of secure input, so that no excerpt is held for a buffer that
        // may hold secrets while it does.
        self.give_excerpt();
        self.sync_secure_input();
        self.sync_ascii_input();
        self.give_cursor_area();
        self.give_preedit_layout();
    }

    /// A terminal's program began or stopped reading a password between events (ADR 0120,
    /// Q17): what follows a buffer that may hold secrets follows it at once, as after an event
    /// that selected one. A composition in it ends, the log is gated, no excerpt is held and
    /// secure input is asked for while it is selected. A prompt that took Japanese, as a
    /// review comment's draft does, takes none once it is shown anywhere (ADR 0106, ADR 0158):
    /// the IME goes off at once, its composition dropped.
    pub(super) fn secret_changed(&mut self) {
        let shown = self.editor.shows_secret() && self.ime.allowed && !self.ime_wanted();
        if self.composing() && (shown || self.editor.current().1.is_secret()) {
            self.end_composition();
            if let Some(view) = &self.view {
                view.window.discard_composition();
            }
        }
        if shown && let Some(view) = &self.view {
            view.window.set_ime_allowed(false);
            self.ime.allowed = false;
            log::debug!("IME allowed: false");
        }
        self.note_logging();
        self.give_excerpt();
        self.sync_secure_input();
    }

    /// Asks the window for secure input while the selected buffer may hold secrets, as a
    /// password field has it (ADR 0073). Only the verdict crosses: no text, name or place of
    /// the buffer. The window holds it only while oso is in front with the keyboard, and does
    /// nothing when the answer is the same.
    pub(super) fn sync_secure_input(&mut self) {
        let secret = self.editor.current().1.is_secret();
        if let Some(view) = &self.view {
            view.window.set_secure_input(secret);
        }
        self.ime.source.secure(secret, Instant::now());
    }

    /// Asks the window for an ASCII input source while a prompt that follows the input source
    /// is selected, and for the person's own once none is, when that changes (ADR 0106,
    /// ADR 0158).
    fn sync_ascii_input(&mut self) {
        let wanted = self
            .editor
            .prompt()
            .is_some_and(|prompt| follows_input_source(prompt.kind()));
        if let Some(view) = self.view.as_ref().filter(|_| wanted != self.ime.ascii) {
            view.window.set_ascii_input(wanted);
            self.ime.ascii = wanted;
            self.ime.source.ascii(wanted, Instant::now());
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
    pub(super) fn composing(&self) -> bool {
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

    /// The person chose another kind of input source while oso was in front (ADR 0106). A
    /// Japanese input mode marks the selected prompt as taking Japanese, if it follows the
    /// input source (ADR 0158); anything else unmarks it. `after_event` then allows the IME or
    /// not.
    pub(super) fn input_source_chosen(&mut self, source: InputSource) {
        self.ime.japanese_prompt = marked(source, self.editor.prompt().map(Prompt::id));
        self.ime.source.told(source, Instant::now());
        log::debug!("input source: {source:?}");
        self.point_back_if_japanese();
    }

    /// Whether the person's input source is a Japanese input mode, as far as oso was told, and
    /// since when that is settled (ADR 0182, "Point" 2).
    pub(super) fn types_japanese(&self) -> (bool, Option<Instant>) {
        self.ime.source.japanese()
    }

    /// Whether oso wants the IME on now ([`wants_ime`]).
    pub(super) fn ime_wanted(&self) -> bool {
        let prompt = self
            .editor
            .prompt()
            .map(|prompt| (prompt.id(), prompt.kind()));
        if takes_japanese(prompt, self.ime.japanese_prompt, self.editor.shows_secret()) {
            return wants_ime(self.editor.current().1, self.dispatcher.reads_keys(), false);
        }
        let reads_input = self.editor.prompt().is_some()
            || self.editor.isearch().is_some()
            || self.editor.jump().is_some()
            || self.editor.replace().is_some();
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
        let grid = view.render_state.pixel_grid();
        // In a terminal, the program's cursor cell, even while it hides the cursor (ADR 0120).
        let area = if self.editor.current().1.is_terminal() {
            self.terminal_cursor_area(grid)
        } else {
            self.editor.cursor_area(grid)
        };
        let Some(area) = area else {
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
        let Some(view) = self.view.as_ref().filter(|_| ANSWERS_INPUT_METHOD) else {
            return;
        };
        let mut positions = std::mem::take(&mut self.ime.scratch);
        let grid = view.render_state.pixel_grid();
        let first = if self.editor.current().1.is_terminal() {
            self.terminal_preedit_layout(grid, &mut positions)
        } else {
            self.editor.preedit_layout(grid, &mut positions)
        };
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

    /// Gives the window the excerpt of the cursor's line, which the input method may read for
    /// reconversion and better predictions (ADR 0070, ADR 0077): only while the IME is on,
    /// only where [`Editor::excerpt_stamp`] says one is given, and again when that changes.
    /// None while the IME is off.
    fn give_excerpt(&mut self) {
        let Some(view) = self.view.as_ref().filter(|_| ANSWERS_INPUT_METHOD) else {
            return;
        };
        let excerpt_for = self
            .ime
            .allowed
            .then(|| self.editor.excerpt_stamp())
            .flatten();
        if excerpt_for == self.ime.excerpt_for {
            return;
        }
        // During a composition the excerpt the input method reads stays as it was, so that
        // the composition's place in it does not move under the input method, as a reload or
        // an edit in another window would move it. It is dropped at once where none is given.
        if excerpt_for.is_some() && self.ime.excerpt_for.is_some() && self.composing() {
            return;
        }
        self.ime.excerpt_for = excerpt_for;
        let cursor = match excerpt_for {
            Some(_) => self.editor.cursor_excerpt(&mut self.ime.excerpt),
            None => None,
        };
        view.window.set_ime_excerpt(&mut self.ime.excerpt, cursor);
        // What comes back is the excerpt the window held before: no text of a buffer stays.
        self.ime.excerpt.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{follows_input_source, marked, next_allowed, takes_japanese, wants_ime};
    use oso_core::buffer::Buffer;
    use oso_core::minibuffer::Kind;
    use oso_platform::window::InputSource;

    const FILE: Kind = Kind::Complete { history: "file" };
    const QUESTION: Kind = Kind::Question {
        yes: "exit",
        verbs: ["exit", "stay"],
        third: None,
    };

    #[test]
    fn only_the_prompt_it_was_chosen_in_takes_japanese() {
        let command = Kind::Complete { history: "command" };
        let note = Kind::Text {
            history: Some("session-note"),
        };
        for kind in [FILE, command, note] {
            assert!(takes_japanese(Some((1, kind)), Some(1), false), "{kind:?}");
            assert!(
                !takes_japanese(Some((1, kind)), None, false),
                "not chosen: {kind:?}"
            );
            assert!(
                !takes_japanese(Some((1, kind)), Some(1), true),
                "a secret buffer is shown: {kind:?}"
            );
        }
        for kind in [Kind::Passphrase, QUESTION, Kind::Number { history: None }] {
            assert!(!takes_japanese(Some((1, kind)), Some(1), false), "{kind:?}");
        }
    }

    #[test]
    fn a_draft_takes_japanese_at_once_unless_a_secret_buffer_is_shown() {
        assert!(takes_japanese(Some((1, Kind::Draft)), None::<u32>, false));
        assert!(takes_japanese(Some((1, Kind::Draft)), Some(2), false));
        assert!(!takes_japanese(Some((1, Kind::Draft)), None::<u32>, true));
    }

    #[test]
    fn prompts_that_read_text_follow_the_input_source_and_no_other_does() {
        for kind in [
            FILE,
            Kind::Complete { history: "command" },
            Kind::Text { history: None },
            Kind::Text {
                history: Some("query-replace"),
            },
        ] {
            assert!(follows_input_source(kind), "{kind:?}");
        }
        for kind in [
            Kind::Draft,
            Kind::Passphrase,
            QUESTION,
            Kind::Number { history: None },
        ] {
            assert!(!follows_input_source(kind), "{kind:?}");
        }
    }

    #[test]
    fn osos_own_switches_mark_no_prompt() {
        // The prompt opens and oso chooses ASCII: the notice reads Other.
        assert_eq!(marked(InputSource::Other, Some(1)), None);
        // The prompt closed and oso gave the person's Japanese back: no prompt is selected.
        assert_eq!(marked(InputSource::Japanese, None::<u32>), None);
        // The person chose Japanese in the prompt.
        assert_eq!(marked(InputSource::Japanese, Some(1)), Some(1));
    }

    #[test]
    fn the_mark_goes_with_its_prompt() {
        // C-x C-f's prompt closed: nothing is selected, then the next prompt is another one,
        // even another prompt for a file's name.
        assert!(!takes_japanese(None::<(u32, Kind)>, Some(1), false));
        assert!(!takes_japanese(Some((2, FILE)), Some(1), false));
    }

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
