//! Serving input on the terminal's thread (ADR 0120): each input is encoded against the
//! engine's modes as they are now, so the bytes match what the program asked for when they
//! reach it. The thread and the fuzz target both serve input here.
//!
//! Must not depend on: the PTY; the thread writes what this returns.

use super::{keep, pointer};
use crate::engine::Engine;
use crate::event::Event;
use crate::input::{self, Refused};
use crate::limits;
use crate::snapshot::{Button, Scroll, Snapshot};
use crate::terminal::Input;

/// What the thread keeps to serve input: the focus it last heard of, the mouse button held,
/// and a key's or a report's bytes.
pub(in crate::terminal) struct Inputs {
    focused: Option<bool>,
    button_held: Option<Button>,
    encoded: [u8; limits::KEY_BYTES_MAX],
}

impl Inputs {
    pub(in crate::terminal) fn new() -> Inputs {
        Inputs {
            focused: None,
            button_held: None,
            encoded: [0; limits::KEY_BYTES_MAX],
        }
    }

    /// Encodes one input against the engine's modes as they are now, and returns its bytes
    /// for the program, maybe none, and whether it raised an event into `next`. Input that
    /// sends anything but a focus report scrolls the view to the bottom first. The terminal's
    /// thread and the fuzz target both serve input through this.
    pub(in crate::terminal) fn serve(
        &mut self,
        input: Input,
        engine: &mut impl Engine,
        next: &mut Snapshot,
    ) -> (Vec<u8>, bool) {
        let bytes = match input {
            Input::Bytes(bytes) => bytes,
            Input::Key(key) => {
                let n = engine.encode_key(key, &mut self.encoded);
                self.encoded[..n].to_vec()
            }
            Input::Pointer(at) => {
                let n = pointer::serve(&mut self.button_held, at, engine, &mut self.encoded);
                self.encoded[..n].to_vec()
            }
            Input::Paste {
                text,
                lines_allowed,
            } => {
                let bracketed = engine.modes().bracketed_paste;
                match input::paste(&text, bracketed, lines_allowed) {
                    Ok(bytes) => bytes,
                    Err(Refused { lines }) => {
                        keep(Event::PasteRefused { lines }, next);
                        return (Vec::new(), true);
                    }
                }
            }
            Input::Focus(focused) => {
                let changed = self.focused.replace(focused) != Some(focused);
                let report = changed && engine.modes().focus_reports;
                let bytes = if report { input::focus(focused) } else { &[] };
                return (bytes.to_vec(), false);
            }
        };
        if !bytes.is_empty() {
            engine.scroll(Scroll::Bottom);
        }
        (bytes, false)
    }
}
