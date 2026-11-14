//! The OS clipboard: handing what a kill took to the clipboard's threads, and a yank the
//! answer to its read (ADR 0074).
//!
//! Must not depend on: key presses or how a frame is drawn. The clipboard is read and
//! written on its own threads, never here, and the text a kill took is joined there too.

use oso_core::command::Effects;
use oso_core::kill::{self, Fingerprint, FromClipboard};
use oso_core::rope::Rope;
use oso_platform::clipboard::{self, Clipboard, Contents, Event, Source};

use super::shell::{Loop, Waker};
use super::{App, UserEvent};

/// What the clipboard's threads report, in the editor's terms.
#[derive(Debug)]
pub enum ClipboardEvent {
    /// The entry `id` reached the clipboard as text with this fingerprint.
    Written { id: u64, fingerprint: Fingerprint },
    /// The answer to a read.
    Read(FromClipboard),
    /// A read did not return, and the clipboard was started anew.
    NotAnswering,
}

/// Starts the clipboard's threads over `source`, reporting through `waker`. Text from
/// another app is built into a rope there, off the UI thread. With no threads, kills
/// stay in the ring, and a yank finds nothing on the clipboard.
pub(super) fn start(source: Source, waker: Waker) -> Option<Clipboard> {
    let report = move |event| {
        let event = match event {
            Event::Written {
                id,
                fingerprint: of,
            } => ClipboardEvent::Written {
                id,
                fingerprint: fingerprint(of),
            },
            Event::Read(contents) => ClipboardEvent::Read(answer(contents)),
            Event::NotAnswering => ClipboardEvent::NotAnswering,
        };
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
            text: Rope::from(text.as_str()),
            concealed,
            fingerprint: fingerprint(of),
        },
        Contents::Nothing => FromClipboard::Nothing,
        Contents::TooLarge => FromClipboard::TooLarge,
        Contents::NoAnswer => FromClipboard::NoAnswer,
    }
}

/// Whether a yank may read `clipboard`: always for the person's own keys, and for input
/// from the dev socket only from a clipboard in memory, so that no client reads the
/// person's clipboard through oso (ADR 0074).
fn may_read(from_socket: bool, in_memory: bool) -> bool {
    !from_socket || in_memory
}

#[cfg(feature = "dev")]
fn is_memory(clipboard: &Clipboard) -> bool {
    clipboard.memory().is_some()
}

#[cfg(not(feature = "dev"))]
fn is_memory(_: &Clipboard) -> bool {
    false
}

impl App {
    /// Hands the clipboard what a command killed, and starts a read a yank waits for. Returns
    /// what a yank did that was answered at once: with no clipboard, or for input from the dev
    /// socket, which never reads the system's.
    pub(super) fn sync_clipboard(&mut self) -> Effects {
        self.write_clipboard();
        let mut effects = Effects::default();
        while let Some(from_socket) = self.dispatcher.take_clipboard_request() {
            match &self.clipboard {
                Some(clipboard) if may_read(from_socket, is_memory(clipboard)) => {
                    clipboard.read();
                    break;
                }
                _ => {
                    let more = self
                        .dispatcher
                        .clipboard_answered(&mut self.editor, &FromClipboard::Nothing);
                    effects = effects.union(more);
                    self.write_clipboard();
                }
            }
        }
        effects
    }

    /// Hands the clipboard the newest kill, if a command has killed since.
    pub(super) fn write_clipboard(&mut self) {
        let write = self.editor.take_clipboard_write();
        if let (Some(write), Some(clipboard)) = (write, &self.clipboard) {
            let (id, concealed) = (write.id, write.secret);
            clipboard.write(id, concealed, Box::new(move || write.text()));
        }
    }

    /// Takes in what the clipboard's threads report.
    pub(super) fn clipboard_event(&mut self, event_loop: &Loop<'_>, event: ClipboardEvent) {
        match event {
            ClipboardEvent::Written { id, fingerprint } => {
                self.editor.note_clipboard_written(id, fingerprint);
            }
            ClipboardEvent::Read(answer) => {
                let effects = self
                    .dispatcher
                    .clipboard_answered(&mut self.editor, &answer);
                self.apply(event_loop, effects);
            }
            ClipboardEvent::NotAnswering => {
                self.editor.message("The clipboard is not answering");
                self.request_redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::may_read;

    #[test]
    fn the_socket_never_reads_the_system_clipboard() {
        assert!(may_read(false, false), "the person's own keys");
        assert!(may_read(false, true));
        assert!(may_read(true, true), "a clipboard in memory, for the flows");
        assert!(!may_read(true, false));
    }
}
