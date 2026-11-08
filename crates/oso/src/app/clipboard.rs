//! The OS clipboard: handing what a kill took to the clipboard's threads, and a yank the
//! answer to its read (ADR 0074).
//!
//! Must not depend on: key presses or how a frame is drawn. The clipboard is read and
//! written on its own threads, never here, and the text a kill took is joined there too.

use oso_core::command::Effects;
use oso_core::kill::{self, Fingerprint, FromClipboard};
use oso_platform::clipboard::{self, Clipboard, Contents, Event, Source};

use super::shell::{Loop, Waker};
use super::{App, UserEvent};

/// Starts the clipboard's threads over `source`, reporting through `waker`. With no
/// threads, kills stay in the ring, and a yank finds nothing on the clipboard.
pub(super) fn start(source: Source, waker: Waker) -> Option<Clipboard> {
    let report = move |event| {
        waker.send(UserEvent::Clipboard(event));
    };
    match Clipboard::start(source, kill::MAX_CLIPBOARD, report) {
        Ok(clipboard) => Some(clipboard),
        Err(e) => {
            log::error!("cannot start the clipboard: {e}");
            None
        }
    }
}

/// The editor's form of a fingerprint.
fn fingerprint(of: clipboard::Fingerprint) -> Fingerprint {
    Fingerprint {
        len: of.len,
        hash: of.hash,
    }
}

/// The editor's form of what the clipboard held.
fn answer(contents: Contents) -> FromClipboard {
    match contents {
        Contents::Own => FromClipboard::Own,
        Contents::Text {
            text,
            concealed,
            fingerprint: of,
        } => FromClipboard::Text {
            text,
            concealed,
            fingerprint: fingerprint(of),
        },
        Contents::Nothing => FromClipboard::Nothing,
        Contents::TooLarge => FromClipboard::TooLarge,
        Contents::NoAnswer => FromClipboard::NoAnswer,
    }
}

impl App {
    /// Hands the clipboard what a command killed, and starts a read a yank waits for.
    /// Returns what a yank asked for when there is no clipboard to wait for.
    pub(super) fn sync_clipboard(&mut self) -> Effects {
        let write = self.editor.take_clipboard_write();
        let Some(clipboard) = &self.clipboard else {
            let mut effects = Effects::default();
            while self.dispatcher.take_clipboard_request() {
                let more = self
                    .dispatcher
                    .clipboard_answered(&mut self.editor, &FromClipboard::Nothing);
                effects = effects.union(more);
                // What it did may have killed, which has nowhere to go either.
                drop(self.editor.take_clipboard_write());
            }
            return effects;
        };
        if let Some(write) = write {
            let (id, concealed) = (write.id, write.secret);
            clipboard.write(id, concealed, Box::new(move || write.text()));
        }
        if self.dispatcher.take_clipboard_request() {
            clipboard.read();
        }
        Effects::default()
    }

    /// Takes in what the clipboard's threads report.
    pub(super) fn clipboard_event(&mut self, event_loop: &Loop<'_>, event: Event) {
        match event {
            Event::Written {
                id,
                fingerprint: of,
            } => {
                self.editor.note_clipboard_written(id, fingerprint(of));
            }
            Event::Read(contents) => {
                let answer = answer(contents);
                let effects = self
                    .dispatcher
                    .clipboard_answered(&mut self.editor, &answer);
                self.apply(event_loop, effects);
            }
        }
    }
}
