//! Keyboard macros (ADR 0113): the one being recorded, the last one recorded, and a replay
//! under way, which the host feeds through its dispatcher a little at a time.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads. The dispatcher
//! records what it takes and feeds what this gives back; this keeps the events, their
//! bounds, and the undo step a replay makes.
//!
//! - Nothing here logs an event, and no `Debug` prints one: a macro recorded around a buffer
//!   that may hold secrets holds what was typed there.
//! - A macro lives in memory only, and `oso dev` does not carry it.

use std::fmt;

use super::Editor;
use crate::buffer::{BufferId, ContentId};
use crate::key::KeyPress;

/// The most events a recording holds.
pub const MAX_MACRO_EVENTS: usize = 100_000;

/// The most bytes of committed text a recording holds.
pub const MAX_MACRO_TEXT: usize = 16 * 1024 * 1024;

/// One input a macro holds: a key, or text an input method committed or a key typed.
#[derive(Clone, PartialEq, Eq)]
pub enum MacroEvent {
    Key(KeyPress),
    /// Text, and whether an input method committed it, which undoes as a step of its own.
    Text {
        text: String,
        commit: bool,
    },
}

impl fmt::Debug for MacroEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Neither the key nor the text: either can be part of a secret (ADR 0044).
        f.write_str(match self {
            Self::Key(_) => "Key",
            Self::Text { .. } => "Text",
        })
    }
}

/// Why a macro command did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroError {
    /// `C-x (` while a recording is on.
    AlreadyRecording,
    /// `C-x )` with no recording on.
    NotRecording,
    /// `C-x e` with no macro recorded.
    NoMacro,
    /// A macro command while a macro is replayed.
    Replaying,
}

impl fmt::Display for MacroError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AlreadyRecording => "Already defining a keyboard macro",
            Self::NotRecording => "Not defining a keyboard macro",
            Self::NoMacro => "No keyboard macro defined",
            Self::Replaying => "A keyboard macro is running",
        })
    }
}

/// A recorded macro.
#[derive(Clone, Default)]
struct Macro {
    events: Vec<MacroEvent>,
    text_bytes: usize,
    /// It was recorded while a buffer that may hold secrets was shown.
    secret: bool,
}

/// A replay under way.
struct Replay {
    events: Vec<MacroEvent>,
    next: usize,
    /// The iterations left after this one; `None` for as many as run until one stops it.
    left: Option<usize>,
    /// Each buffer's state when it began, so that what it changed undoes as one step.
    began: Vec<(BufferId, ContentId)>,
}

/// The recording, the last macro and the replay.
#[derive(Default)]
pub(super) struct Macros {
    recording: Option<Macro>,
    last: Option<Macro>,
    replay: Option<Replay>,
}

impl fmt::Debug for Macros {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Macros")
            .field("recording", &self.recording.is_some())
            .field("replaying", &self.replay.is_some())
            .finish_non_exhaustive()
    }
}

impl Editor {
    /// Starts recording a keyboard macro.
    ///
    /// # Errors
    ///
    /// [`MacroError::AlreadyRecording`] or [`MacroError::Replaying`]. Nothing changes.
    pub fn macro_start(&mut self) -> Result<(), MacroError> {
        if self.macros.replay.is_some() {
            return Err(MacroError::Replaying);
        }
        if self.macros.recording.is_some() {
            return Err(MacroError::AlreadyRecording);
        }
        let secret = self.shows_secret();
        self.macros.recording = Some(Macro {
            secret,
            ..Macro::default()
        });
        self.echo("Defining keyboard macro...");
        Ok(())
    }

    /// Ends the recording, which becomes the last macro.
    ///
    /// # Errors
    ///
    /// [`MacroError::NotRecording`]. Nothing changes.
    pub fn macro_end(&mut self) -> Result<(), MacroError> {
        let recorded = self
            .macros
            .recording
            .take()
            .ok_or(MacroError::NotRecording)?;
        self.macros.last = Some(recorded);
        self.echo("Keyboard macro defined");
        Ok(())
    }

    /// Drops the recording, keeping the last macro, and says `why`.
    pub fn macro_cancel(&mut self, why: &str) {
        if self.macros.recording.take().is_some() {
            self.echo(why.to_owned());
        }
    }

    /// Whether a keyboard macro is being recorded.
    pub fn macro_recording(&self) -> bool {
        self.macros.recording.is_some()
    }

    /// Adds `events` to the recording, as the dispatcher took them. Past
    /// [`MAX_MACRO_EVENTS`] or [`MAX_MACRO_TEXT`], the recording is dropped.
    pub fn macro_record(&mut self, events: impl IntoIterator<Item = MacroEvent>) {
        let secret = self.shows_secret();
        let Some(recording) = &mut self.macros.recording else {
            return;
        };
        recording.secret |= secret;
        for event in events {
            if let MacroEvent::Text { text, .. } = &event {
                recording.text_bytes += text.len();
            }
            recording.events.push(event);
        }
        if recording.events.len() > MAX_MACRO_EVENTS || recording.text_bytes > MAX_MACRO_TEXT {
            self.macro_cancel("Keyboard macro too long; not recorded");
        }
    }

    /// Whether the last macro was recorded around a buffer that may hold secrets.
    pub fn macro_secret(&self) -> bool {
        self.macros.last.as_ref().is_some_and(|last| last.secret)
    }

    /// Starts replaying the last macro `count` times, or with `None`, until something stops
    /// it. The host feeds its events through [`Editor::macro_next`].
    ///
    /// # Errors
    ///
    /// [`MacroError::NoMacro`], [`MacroError::Replaying`] or
    /// [`MacroError::AlreadyRecording`]. Nothing changes.
    pub fn macro_call(&mut self, count: Option<usize>) -> Result<(), MacroError> {
        if self.macros.replay.is_some() {
            return Err(MacroError::Replaying);
        }
        if self.macros.recording.is_some() {
            return Err(MacroError::AlreadyRecording);
        }
        let last = self.macros.last.as_ref().ok_or(MacroError::NoMacro)?;
        if last.events.is_empty() || count == Some(0) {
            return Ok(());
        }
        let began = self
            .buffers()
            .map(|(id, buffer)| (id, buffer.content_id()))
            .collect();
        self.macros.replay = Some(Replay {
            events: last.events.clone(),
            next: 0,
            left: count.map(|count| count - 1),
            began,
        });
        Ok(())
    }

    /// Whether a macro is being replayed.
    pub fn macro_replaying(&self) -> bool {
        self.macros.replay.is_some()
    }

    /// The replay's next event, going round again for each iteration left. `None` once the
    /// last has been given, and the replay has ended.
    pub fn macro_next(&mut self) -> Option<MacroEvent> {
        let replay = self.macros.replay.as_mut()?;
        if replay.next == replay.events.len() {
            match &mut replay.left {
                Some(0) => {
                    self.macro_stop(None);
                    return None;
                }
                Some(left) => *left -= 1,
                None => {}
            }
            replay.next = 0;
        }
        let event = replay.events[replay.next].clone();
        replay.next += 1;
        Some(event)
    }

    /// Ends the replay, saying `why` if something stopped it. What it changed in each buffer
    /// undoes as one step there.
    pub fn macro_stop(&mut self, why: Option<&str>) {
        let Some(replay) = self.macros.replay.take() else {
            return;
        };
        for (id, began) in replay.began {
            if let Some(buffer) = self.buffer_mut(id)
                && buffer.content_id() != began
            {
                buffer.join_undo_since(began);
            }
        }
        if let Some(why) = why {
            self.echo(why.to_owned());
        }
    }
}

#[cfg(test)]
mod tests;
