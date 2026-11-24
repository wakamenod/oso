//! The composer's place under a native session's transcript, given to its call's line while point is
//! there with nothing written (ADR 0182, "The line"), and taken back.
//!
//! Must not depend on: key presses or how a frame is drawn; the host says when point came or a text
//! is put in.
//!
//! - The composer's buffer is kept. While its place is given, it is docked under the line, which hosts
//!   nothing: it shows nowhere, and no list of buffers has it.
//! - It gives its place with nothing in it and nothing under way there: no text, no edit of a queued
//!   message, no send or image that waits, no dictation, and no secret that was in it.
//! - It takes it back as its session's call goes, as the person goes to it, before a text is put in
//!   it, and after any event that left one in it.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::window::DockSlot;

use crate::app::App;
use crate::app::sessions::empty_composer;
use crate::app::sessions::hosting::native::Native;
use crate::app::sessions::hosting::native::carrier::carrier;
use crate::sessions::worker::SessionKey;

/// Takes `composer` from under `transcript`, and docks it under `line`, where it shows nowhere.
pub(super) fn give(editor: &mut Editor, transcript: BufferId, line: BufferId, composer: BufferId) {
    editor.set_dock(transcript, DockSlot::Composer, None);
    editor.set_dock(line, DockSlot::Composer, Some(composer));
}

/// Docks `composer` under `transcript` again, from under `line`.
pub(super) fn take(editor: &mut Editor, transcript: BufferId, line: BufferId, composer: BufferId) {
    editor.set_dock(line, DockSlot::Composer, None);
    editor.set_dock(transcript, DockSlot::Composer, Some(composer));
}

/// Docks `composer` under `transcript` again as [`take`] does, unless the window that carries the docks
/// shows none of them then, where the line stays, selected if it was. Whether it has its place.
fn take_if_it_fits(
    editor: &mut Editor,
    transcript: BufferId,
    line: BufferId,
    composer: BufferId,
) -> bool {
    let shown = carrier(editor, transcript, DockSlot::Card).is_some();
    let selected = editor.tiled().0.id();
    take(editor, transcript, line, composer);
    if !shown || carrier(editor, transcript, DockSlot::Composer).is_some() {
        return true;
    }
    give(editor, transcript, line, composer);
    editor.select_window(selected);
    false
}

/// Whether `native`'s composer, in `editor`, holds nothing, and nothing is under way there, a dictation
/// apart.
fn free(editor: &Editor, native: &Native) -> bool {
    let composer = native.composing.buffer.and_then(|id| editor.buffer(id));
    !native.composing.under_way() && composer.is_some_and(empty_composer)
}

/// Whether `native`'s composer gave its place to its line.
fn given(editor: &Editor, native: &Native) -> bool {
    let window = (native.line).and_then(|line| editor.dock_window(line, DockSlot::Composer));
    window.is_some_and(|window| {
        editor.window_by_id(window).map(|w| w.buffer()) == native.composing.buffer
    })
}

impl App {
    /// Whether session `key`'s composer holds nothing, and nothing is under way there, so that point
    /// may go from it by itself and the line may take its place.
    pub(super) fn composer_free(&self, key: SessionKey) -> bool {
        !self.dictation_under_way()
            && !self.reads_image_for(key)
            && (self.hosting.natives.map.get(&key)).is_some_and(|native| free(&self.editor, native))
    }

    /// Session `key`'s line takes the place of its composer, if point is on the line and the composer
    /// is free.
    pub(super) fn line_takes_place(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let (Some(line), Some(composer)) = (native.line, native.composing.buffer) else {
            return;
        };
        let transcript = native.buffer;
        let here = self.editor.tiled().0.buffer() == line
            && (self.editor.dock_window(transcript, DockSlot::Composer)).is_some();
        if here && self.composer_free(key) {
            give(&mut self.editor, transcript, line, composer);
            self.draw_line(key);
            self.request_redraw();
        }
    }

    /// Session `key`'s composer has its place under the transcript again, if it gave it.
    pub(in crate::app::sessions::hosting::native) fn composer_back(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let (Some(line), Some(composer)) = (native.line, native.composing.buffer) else {
            return;
        };
        if given(&self.editor, native) {
            take(&mut self.editor, native.buffer, line, composer);
            self.request_redraw();
        }
    }

    /// Whether session `key`'s line has its composer's place.
    pub(super) fn line_has_place(&self, key: SessionKey) -> bool {
        (self.hosting.natives.map.get(&key)).is_some_and(|native| given(&self.editor, native))
    }

    /// Session `key`'s composer has its place again, if something is in it or under way there.
    pub(super) fn composer_back_with_text(&mut self, key: SessionKey) {
        let held = (self.hosting.natives.map.get(&key)).is_some_and(|n| !free(&self.editor, n));
        if held {
            self.composer_back(key);
        }
    }

    /// As point is to go to session `key`'s composer: it has its place again, unless its window shows
    /// no dock with it, where the line stays. Whether it has it.
    pub(in crate::app::sessions::hosting::native) fn composer_back_for_point(
        &mut self,
        key: SessionKey,
    ) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let (Some(line), Some(composer)) = (native.line, native.composing.buffer) else {
            return true;
        };
        if !given(&self.editor, native) {
            return true;
        }
        let free = self.composer_free(key);
        let Some(transcript) = self.hosting.natives.map.get(&key).map(|n| n.buffer) else {
            return false;
        };
        self.request_redraw();
        // A text in it is never out of sight.
        if !free {
            take(&mut self.editor, transcript, line, composer);
            return true;
        }
        take_if_it_fits(&mut self.editor, transcript, line, composer)
    }

    /// Point goes to session `key`'s line, which takes a free composer's place: also where its window
    /// is too short for both. Whether it went.
    pub(super) fn select_line(&mut self, key: SessionKey) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let (transcript, line, composer) = (native.buffer, native.line, native.composing.buffer);
        let Some(window) = self.editor.dock_window(transcript, DockSlot::Card) else {
            return false;
        };
        let shows = |editor: &Editor| carrier(editor, transcript, DockSlot::Card).is_some();
        // The composer's window goes with it, so point is there no more.
        let (in_composer, free) = (
            Some(self.editor.tiled().0.buffer()) == composer,
            self.composer_free(key),
        );
        if !shows(&self.editor)
            && let (true, false, Some(line), Some(composer)) = (free, in_composer, line, composer)
        {
            give(&mut self.editor, transcript, line, composer);
            if !shows(&self.editor) {
                take(&mut self.editor, transcript, line, composer);
            }
        }
        if !shows(&self.editor) {
            return false;
        }
        self.editor.select_window(window);
        self.line_takes_place(key);
        true
    }
}

#[cfg(test)]
#[path = "place_tests.rs"]
mod tests;
