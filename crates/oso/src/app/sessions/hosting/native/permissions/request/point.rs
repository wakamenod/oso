//! When point goes to a native session's call's line by itself, and what a key does that is typed as
//! it goes (ADR 0182, "Point" 1 to 4).
//!
//! Must not depend on: how a frame is drawn; the loop says when an event ended and what is due.
//!
//! - At once, as the call is shown, while the person is in its session's composer with nothing in it,
//!   or at the end of its transcript, in the middle of nothing, and oso's window has been in front
//!   and the input source what it is for [`QUIET`]. Else once that holds, and [`QUIET`] passed with no
//!   key since the call was shown.
//! - Never with a Japanese input source, which oso is told of only in front; nor, for a call, once the
//!   person chose the composer while it waited. A source oso is not told of is not Japanese. One told
//!   of only after point went takes it back, if the person did nothing since.
//! - Until the line takes a key after that, the first thing the person does: text goes into the
//!   composer, if point came from there, and point with it; `RET` does nothing; a press of the mouse
//!   on the line in the composer's place is one on the composer.
//! - The line takes no key before a frame with point on it was presented, and [`QUIET`] since.
//! - A key held down as oso moves point, either way, does nothing more until it is let go.

use std::time::Instant;

use oso_core::key::{Key, KeyPress, NamedKey};
use oso_core::selection::MarkActive;
use oso_core::window::{DockSlot, Window, WindowId};

use super::Drawn;
use crate::app::agent::QUIET;
use crate::app::{App, UserEvent};
use crate::sessions::worker::SessionKey;

/// Where point is, for a session's call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Where {
    /// In its composer, with nothing in it.
    Composer,
    /// At its transcript's end, or where oso put it, at its block's head.
    Transcript,
    Elsewhere,
}

/// What decides whether point goes to a call's line by itself.
#[derive(Debug, Clone, Copy)]
struct Person {
    at: Where,
    /// In the middle of something.
    middle: bool,
    /// They chose the composer while the call waited.
    chosen: bool,
    /// Their input source is a Japanese one, and since when it is settled, if it ever was not.
    japanese: (bool, Option<Instant>),
    /// Since when they see oso's window, if they do.
    seen: Option<Instant>,
}

impl Person {
    /// Since when point may go, but for what is waited for after.
    fn may_go(&self) -> Option<Instant> {
        let goes = self.at != Where::Elsewhere && !self.middle && !self.chosen && !self.japanese.0;
        let seen = self.seen.filter(|_| goes)?;
        Some(self.japanese.1.map_or(seen, |settled| seen.max(settled)))
    }

    /// Whether point goes at once, as a call is shown at `now`.
    fn goes_at_once(&self, now: Instant) -> bool {
        self.may_go().is_some_and(|since| now >= since + QUIET)
    }

    /// When point goes to the line of a call shown at `shown`, the last key typed at `typed`, if
    /// nothing changes before.
    fn goes_at(&self, shown: Instant, typed: Option<Instant>) -> Option<Instant> {
        let since = self.may_go()?.max(shown);
        Some(typed.map_or(since, |typed| since.max(typed)) + QUIET)
    }
}

/// What the person does as point has just come to a line by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Typing {
    Key(KeyPress),
    /// Text from a key that types several characters, or of an input method.
    Text,
    /// What is no text: a wheel's turn, a press of the mouse.
    Act,
}

/// What comes of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Came {
    /// It is text: point goes back to the composer, where it goes in.
    Draft,
    /// It does nothing.
    Dropped,
    /// It does what it does on the line.
    Line,
}

/// What comes of `typing` at `now`, the first thing done since point came to a line by itself, out of
/// the composer or not (`composer`), which takes keys as from `since`, if it will; the person in the
/// `middle` of something or not.
fn as_it_came(
    typing: Typing,
    composer: bool,
    since: Option<Instant>,
    now: Instant,
    middle: bool,
) -> Came {
    if middle || since.is_some_and(|since| now >= since + QUIET) {
        return Came::Line;
    }
    match typing {
        Typing::Text if composer => Came::Draft,
        Typing::Key(key) if composer && key.text().is_some() => Came::Draft,
        Typing::Key(key) if key.key == Key::Named(NamedKey::Return) => Came::Dropped,
        Typing::Key(_) | Typing::Text | Typing::Act => Came::Line,
    }
}

/// Whether a press of the mouse, over the line (`on_line`) which has its composer's place (`has_place`)
/// and takes keys as from `since`, if it will, is meant for the composer, at `now`.
fn means_composer(on_line: bool, has_place: bool, since: Option<Instant>, now: Instant) -> bool {
    on_line && has_place && since.is_none_or(|since| now < since + QUIET)
}

/// Whether point goes to a call's line by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Goes {
    /// It has not been there yet.
    pending: bool,
    /// The person chose the composer while the call waits: it goes by itself no more.
    chosen: bool,
}

impl Default for Goes {
    fn default() -> Self {
        Self {
            pending: true,
            chosen: false,
        }
    }
}

/// A move of oso's to session `key`'s line, after which the person did nothing yet.
#[derive(Debug, Clone, Copy)]
struct ByItself {
    key: SessionKey,
    /// Out of its composer, or else from its transcript.
    composer: bool,
}

/// Where point is, and how it came there.
#[derive(Debug, Default)]
pub(super) struct Point {
    /// The tiled window selected as the last event ended.
    window: Option<WindowId>,
    by_itself: Option<ByItself>,
    /// Point went by itself to this session's line, and no frame showed it there yet.
    unseen: Option<SessionKey>,
    /// When a move that was due was last not made.
    refused: Option<Instant>,
}

/// What the tiled window selected shows, of the sessions whose calls wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Here {
    Line(SessionKey),
    Composer(SessionKey),
    Elsewhere,
}

impl Point {
    /// Point went by itself to a line, as `by` says.
    fn went(&mut self, by: ByItself) {
        self.by_itself = Some(by);
        self.unseen = Some(by.key);
    }

    /// The move to a line the person did nothing after, taken, if `here` is that line still.
    fn take(&mut self, here: Here) -> Option<ByItself> {
        self.by_itself
            .take()
            .filter(|by| here == Here::Line(by.key))
    }

    /// A frame was presented: the session whose line it shows point on for the first time since point
    /// went there by itself.
    fn seen(&mut self) -> Option<SessionKey> {
        self.unseen.take()
    }

    /// An event ended, with `here` in the selected tiled window `id`, while the calls of `waits` are
    /// shown: what the person went to by it.
    fn note<'a>(
        &mut self,
        (id, here): (WindowId, Here),
        waits: impl Iterator<Item = &'a mut Drawn>,
    ) {
        let moved = self.window.replace(id) != Some(id);
        (self.by_itself).take_if(|by| here != Here::Line(by.key));
        (self.unseen).take_if(|key| here != Here::Line(*key));
        for drawn in waits {
            if here == Here::Line(drawn.key) {
                drawn.goes.pending = false;
            } else if moved && here == Here::Composer(drawn.key) {
                drawn.goes.chosen = true;
            }
        }
    }

    /// Whether point came to session `key`'s line by itself, and no frame showed it there yet.
    pub(super) fn unseen(&self, key: SessionKey) -> bool {
        self.unseen == Some(key)
    }
}

impl App {
    /// Where point is, for session `key`'s call.
    fn point_is(&self, key: SessionKey) -> Where {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return Where::Elsewhere;
        };
        let window = self.editor.window();
        let selections = &window.selections;
        if selections.len() != 1 || selections.mark_active() != MarkActive::Inactive {
            return Where::Elsewhere;
        }
        // Where it is docked: a composer shown elsewhere is a buffer as any.
        if window.docked() == Some((native.buffer, DockSlot::Composer)) {
            return match self.composer_free(key) {
                true => Where::Composer,
                false => Where::Elsewhere,
            };
        }
        let end = self.editor.buffer(native.buffer).map(|b| b.text().len());
        let cursor = Some(selections.primary().cursor);
        let at_end = cursor == end || cursor == self.head_of(key, window.id());
        match window.docked().is_none() && window.buffer() == native.buffer && at_end {
            true => Where::Transcript,
            false => Where::Elsewhere,
        }
    }

    /// What decides whether point goes to session `key`'s line by itself.
    fn person_at(&self, key: SessionKey) -> Person {
        let drawn = &self.hosting.natives.permissions.requests.drawn;
        let drawn = drawn.iter().find(|drawn| drawn.key == key);
        Person {
            at: self.point_is(key),
            middle: self.person_in_the_middle() || self.dispatcher.waits_for_branches(),
            chosen: drawn.is_none_or(|drawn| drawn.goes.chosen),
            japanese: self.types_japanese(),
            seen: self.window_seen_since(),
        }
    }

    /// As session `key`'s call is shown: point goes to its line at once, if it does.
    pub(super) fn line_focus_at_once(&mut self, key: SessionKey) {
        if self.person_at(key).goes_at_once(Instant::now()) {
            self.line_focus(key);
        }
    }

    /// The call point goes to the line of next, and when, if nothing changes before.
    fn line_focus_next(&self) -> Option<(SessionKey, Instant)> {
        let permissions = &self.hosting.natives.permissions;
        let typed = self.agent_person_typed();
        let refused = permissions.requests.point.refused;
        (permissions.requests.drawn.iter())
            .filter(|drawn| drawn.goes.pending)
            .filter_map(|drawn| {
                let shown = permissions.asks.shown(drawn.key)?.shown?;
                // A line that does not show takes no point.
                let at = (self.person_at(drawn.key).goes_at(shown, typed))
                    .filter(|_| self.line_shows_all(drawn.key))?;
                // One not made is not due again until something changes.
                let due = refused.is_none_or(|refused| at > refused);
                due.then_some((drawn.key, at))
            })
            .min_by_key(|(_, at)| *at)
    }

    /// When point goes to a call's line by itself, if nothing changes before.
    pub(in crate::app::sessions::hosting::native) fn line_focus_deadline(&self) -> Option<Instant> {
        self.line_focus_next().map(|(_, at)| at)
    }

    /// Before the loop waits, at `now`: point goes to a call's line, if it is due.
    pub(in crate::app::sessions::hosting::native) fn line_focus_due(&mut self, now: Instant) {
        if let Some((key, _)) = self.line_focus_next().filter(|(_, at)| *at <= now) {
            let went = self.line_focus(key);
            let point = &mut self.hosting.natives.permissions.requests.point;
            point.refused = (!went).then_some(now);
        }
    }

    /// Point goes to session `key`'s line by itself, if its window shows all of it. Whether it
    /// went.
    fn line_focus(&mut self, key: SessionKey) -> bool {
        let composer = match self.point_is(key) {
            Where::Elsewhere => return false,
            from => from == Where::Composer,
        };
        if !self.line_shows_all(key) || !self.select_line(key) {
            return false;
        }
        self.point_moved_by_oso();
        let requests = &mut self.hosting.natives.permissions.requests;
        if let Some(drawn) = requests.drawn(key) {
            drawn.goes.pending = false;
        }
        requests.point.went(ByItself { key, composer });
        self.note_current_card();
        self.request_redraw();
        true
    }

    /// oso moved point: the window selected now was not chosen by the person, and a key held down does
    /// nothing more there.
    pub(super) fn point_moved_by_oso(&mut self) {
        let now = self.editor.tiled().0.id();
        self.hosting.natives.permissions.requests.point.window = Some(now);
        self.held_key.point_moved();
        // The input method is as it is wanted there, before the next key.
        self.waker.send(UserEvent::AgentDeadline);
    }

    /// After each event, while a call is shown: whether the person went to its line, where point then
    /// waits no more, or chose its composer, from where it never goes by itself.
    pub(super) fn note_point(&mut self) {
        let (id, here) = (self.editor.tiled().0.id(), self.here());
        let requests = &mut self.hosting.natives.permissions.requests;
        requests.point.note((id, here), requests.drawn.iter_mut());
    }

    /// What the selected tiled window shows, of the sessions whose calls wait.
    fn here(&self) -> Here {
        let window = self.editor.tiled().0;
        let shown = window.buffer();
        if let Some(key) = self.line_session(shown) {
            return Here::Line(key);
        }
        match self.composer_session(shown) {
            Some(key) => Here::Composer(key),
            None => Here::Elsewhere,
        }
    }

    /// A frame was drawn, and presented: one that shows point on the line it went to by itself starts
    /// the wait there.
    pub(super) fn point_seen(&mut self) {
        let point = &mut self.hosting.natives.permissions.requests.point;
        if let Some(key) = point.seen() {
            self.wait_on_card_again(key);
        }
    }

    /// The move to a line the person did nothing after, taken, if that line is selected still.
    fn take_by_itself(&mut self) -> Option<ByItself> {
        self.hosting.natives.permissions.requests.point.by_itself?;
        let here = match self.editor.prompt() {
            Some(_) => Here::Elsewhere,
            None => self.here(),
        };
        let requests = &mut self.hosting.natives.permissions.requests;
        requests.point.take(here)
    }

    /// The person does `typing`, not yet carried out: text typed as point came to a line by itself out
    /// of the composer takes point back there. Whether it does nothing more.
    pub(in crate::app) fn typed_as_point_came(&mut self, typing: Typing) -> bool {
        let Some(by) = self.take_by_itself() else {
            return false;
        };
        let since = self.line_since(by.key);
        let middle = self.person_in_the_middle();
        match as_it_came(typing, by.composer, since, Instant::now(), middle) {
            Came::Line => false,
            Came::Draft if self.point_back(by) => false,
            Came::Draft | Came::Dropped => {
                // The next key is as close to its coming.
                let requests = &mut self.hosting.natives.permissions.requests;
                requests.point.by_itself = Some(by);
                self.editor.message(super::gate::Refusal::Wait.message());
                self.request_redraw();
                true
            }
        }
    }

    /// Point goes back where it went to a line from by itself, and waits for the line again. Whether it
    /// did: where the composer does not fit, it stays.
    fn point_back(&mut self, by: ByItself) -> bool {
        let line = self.editor.tiled().0.id();
        self.composer_go(by.key, by.composer);
        let shown = self.editor.tiled().0.buffer();
        let native = self.hosting.natives.map.get(&by.key);
        let back = native.is_some_and(|native| match by.composer {
            true => native.composing.buffer == Some(shown),
            false => native.buffer == shown,
        });
        if !back {
            self.editor.select_window(line);
            return false;
        }
        if !by.composer {
            self.composer_back(by.key);
        }
        self.point_moved_by_oso();
        let requests = &mut self.hosting.natives.permissions.requests;
        requests.point.unseen = None;
        if let Some(drawn) = requests.drawn(by.key) {
            drawn.goes.pending = true;
        }
        self.note_current_card();
        self.request_redraw();
        true
    }

    /// With a Japanese input source, point that went to a line by itself before oso was told of it goes
    /// back, the person having done nothing since; not beneath a prompt, but once that closed.
    pub(in crate::app) fn point_back_if_japanese(&mut self) {
        if self.editor.prompt().is_some() || !self.types_japanese().0 {
            return;
        }
        let Some(by) = self.take_by_itself() else {
            return;
        };
        // Where the composer does not fit, the transcript: in no case the line.
        if !self.point_back(by) {
            self.composer_go(by.key, false);
            self.point_moved_by_oso();
            self.hosting.natives.permissions.requests.point.unseen = None;
            self.note_current_card();
        }
    }

    /// The mouse is pressed in the text of window `id`: as point just came to a line by itself, in the
    /// place of the composer, a press there is meant for that. Whether it was.
    pub(in crate::app) fn pressed_as_point_came(&mut self, id: Option<WindowId>) -> bool {
        let Some(by) = self.hosting.natives.permissions.requests.point.by_itself else {
            return false;
        };
        let over = (id.and_then(|id| self.editor.window_by_id(id))).map(Window::buffer);
        let on_line = over.and_then(|over| self.line_session(over)) == Some(by.key);
        let has_place = self.line_has_place(by.key);
        let since = self.line_since(by.key);
        if !means_composer(on_line, has_place, since, Instant::now()) {
            return false;
        }
        let Some(by) = self.take_by_itself() else {
            return false;
        };
        let back = self.point_back(ByItself {
            composer: true,
            ..by
        });
        let requests = &mut self.hosting.natives.permissions.requests;
        match requests.drawn(by.key) {
            Some(drawn) if back => drawn.goes.chosen = true,
            // It stays as it came.
            _ => requests.point.by_itself = Some(by),
        }
        true
    }
}

#[cfg(test)]
#[path = "point_tests.rs"]
mod tests;
