//! What of a native session's call the person was shown (ADR 0182, "Reading"): what the window that
//! carries its line shows of its block, looked at after each event, as each frame was presented,
//! and before a key answers.
//!
//! Must not depend on: key presses. What is read of it is the reading's own to say.
//!
//! - Only of the window the session's line is laid out in, and only while it, or what is docked in
//!   it, is selected, oso's window is in front and in sight, and nothing is open over the rows: a
//!   prompt, a menu, a float, a jump's labels, or an input method's candidates.
//! - A frame's view is taken as the frame was built, before it was drawn: what came after is another.
//! - Rows are laid out only while the request may be read: else a look costs a few seeks.

use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::display_map::wrap_map::WrapSnapshot;
use oso_core::editor::Editor;
use oso_core::frame::WindowRegions;
use oso_core::window::{DockSlot, Screen, WindowId};

use crate::app::App;
use crate::app::sessions::hosting::native::NativeSessions;
use crate::sessions::native::request::reading::View;
use crate::sessions::native::request::reading::rows::rows;
use crate::sessions::native::tail::Part;
use crate::sessions::worker::SessionKey;

/// Room for what a window shows, kept between looks.
#[derive(Default)]
pub(super) struct Room {
    screen: Screen,
    view: View,
    /// What each shown call's window shows as the frame under way was built.
    frame: Vec<(SessionKey, View)>,
}

impl std::fmt::Debug for Room {
    /// Leaves out the rows, as a screen's does.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Room").finish_non_exhaustive()
    }
}

/// The window that carries what is docked under `transcript`, as the dock in `slot` is laid out: the
/// one of the frame that shows it and whose own rows hold the dock's. None while no row of the dock
/// shows.
pub(in super::super) fn carrier(
    editor: &Editor,
    transcript: BufferId,
    slot: DockSlot,
) -> Option<(WindowId, &WindowRegions)> {
    let dock = editor.dock_window(transcript, slot)?;
    let inside = editor.regions(dock)?.text;
    if inside.rows == 0 || inside.columns == 0 {
        return None;
    }
    (editor.frame().windows().iter()).find_map(|(id, regions)| {
        let window = editor.window_by_id(*id)?;
        let holds = (regions.text.row..regions.modeline.row).contains(&inside.row)
            && (regions.gutter.column..regions.text.column + regions.text.columns)
                .contains(&inside.column);
        let its = window.docked().is_none() && window.buffer() == transcript && holds;
        (its && regions.text.columns > 0).then_some((*id, regions))
    })
}

/// Puts what the window that carries session `key`'s line shows of its block in `view`: attended
/// only if `watched`, which says that nothing is open over it and the person is at it.
fn view_of(
    editor: &Editor,
    natives: &NativeSessions,
    (key, watched): (SessionKey, bool),
    (screen, view): (&mut Screen, &mut View),
) {
    view.rows.clear();
    (view.window, view.columns) = (None, 0);
    (view.wraps, view.attended) = (false, false);
    let Some(native) = natives.map.get(&key) else {
        return;
    };
    let Some((id, regions)) = carrier(editor, native.buffer, DockSlot::Card) else {
        return;
    };
    view.window = Some(id);
    // The window, or one docked in it, which is laid out in the one window that carries them.
    let (selected, _) = editor.tiled();
    let here = selected.id() == id
        || selected
            .docked()
            .is_some_and(|(host, _)| host == native.buffer);
    view.attended = watched && here;
    if !view.attended {
        return;
    }
    let (Some(window), Some(buffer)) = (editor.window_by_id(id), editor.buffer(native.buffer))
    else {
        return;
    };
    let display = buffer.display();
    editor.screen_of(id, screen);
    view.columns = regions.text.columns;
    // What is laid out, and not what the buffer asks for: a narrow window does not wrap.
    let wraps = (window.wrap_of(&display)).is_some_and(WrapSnapshot::wraps);
    view.wraps = wraps && window.scroll().left == 0;
    let body = native.part_place(Part::Body);
    rows(screen, &display, regions.text.rows, &body, &mut view.rows);
}

impl App {
    /// Whether the person is at oso's window with nothing open over its rows, as a row read then
    /// was seen.
    fn rows_are_seen(&self) -> bool {
        let editor = &self.editor;
        self.window_seen_since().is_some()
            && editor.prompt().is_none()
            && !self.dispatcher.menu_open()
            && editor.hover().is_none()
            && editor.completion_popup().is_none()
            && editor.jump().is_none()
            && !self.composing()
    }

    /// Looks at what session `key`'s call's window shows, at `now`.
    pub(super) fn look_at_request(&mut self, key: SessionKey, now: Instant) {
        let watched = self.rows_are_seen();
        let mut room = std::mem::take(&mut self.hosting.natives.permissions.requests.room);
        view_of(
            &self.editor,
            &self.hosting.natives,
            (key, watched),
            (&mut room.screen, &mut room.view),
        );
        self.looked(key, &room.view, false, now);
        self.hosting.natives.permissions.requests.room = room;
    }

    /// What session `key`'s call's window showed at `now` was `view`, a frame of it `presented` or
    /// not.
    fn looked(&mut self, key: SessionKey, view: &View, presented: bool, now: Instant) {
        let permissions = &mut self.hosting.natives.permissions;
        let Some(request) = (permissions.asks.shown_mut(key)).and_then(|ask| ask.request_mut())
        else {
            return;
        };
        if !request.reading.look(view, presented, now) {
            return;
        }
        // What the line says of it may be another thing now.
        if let Some(drawn) = permissions.requests.drawn(key) {
            drawn.stale = true;
        }
        // It is read once a frame of it was presented.
        if view.attended && !presented {
            self.request_redraw();
        }
    }

    /// A frame is about to be drawn: what it shows of each shown call is kept.
    pub(in crate::app) fn note_frame_of_requests(&mut self) {
        let mut room = std::mem::take(&mut self.hosting.natives.permissions.requests.room);
        room.frame.clear();
        let watched = self.rows_are_seen();
        for drawn in &self.hosting.natives.permissions.requests.drawn {
            let mut view = View::default();
            view_of(
                &self.editor,
                &self.hosting.natives,
                (drawn.key, watched),
                (&mut room.screen, &mut view),
            );
            room.frame.push((drawn.key, view));
        }
        self.hosting.natives.permissions.requests.room = room;
    }

    /// The frame was drawn, and `presented` or not: what it showed was seen from now on.
    pub(in crate::app) fn frame_of_requests_drawn(&mut self, presented: bool) {
        let mut frame = std::mem::take(&mut self.hosting.natives.permissions.requests.room.frame);
        for (key, view) in frame.drain(..) {
            self.looked(key, &view, presented, Instant::now());
            self.draw_line(key);
        }
        self.hosting.natives.permissions.requests.room.frame = frame;
    }

    /// When what is in view of a call is read, if nothing changes first.
    pub(in crate::app::sessions::hosting::native) fn request_read_deadline(
        &self,
    ) -> Option<Instant> {
        let permissions = &self.hosting.natives.permissions;
        (permissions.requests.drawn.iter())
            .filter_map(|drawn| permissions.asks.shown(drawn.key)?.request()?.reading.due())
            .min()
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
