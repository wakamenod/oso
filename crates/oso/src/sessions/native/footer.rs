//! What a native session's footer says (ADR 0165, the look, part G2; ADR 0170): the permission
//! mode in force, in the words Claude Code's terminal has for it, the key that switches it, and the
//! model, each as far as the window has room.
//!
//! Must not depend on: threads, the stream, the UI or how a frame is drawn; plain values.
//!
//! - The mode's words are oso's, after a mark the stream's text cannot hold. What the stream says
//!   stands in them only for a mode oso does not know, behind oso's own words, and is cut.
//! - The mode is never left out: the key goes first, then the model, then where the mode was set, so
//!   that no name the stream gives takes that one's place.
//! - The model is at the right edge, apart from the rest and dim, and holds ASCII alone
//!   (`wire::status_line::readable`). A quiet mode is dim too: what tells its words are their mark
//!   and their place.

use oso_core::minibuffer::{cells_of, cut_to};

use crate::channel::protocol::mode::PermissionMode;

/// The most cells the model takes, and a mode's name that is not oso's.
const MOST: usize = 24;
/// The blank cells before the model, at the least.
const GAP: usize = 1;
/// What is said of a mode no key here gave (ADR 0181).
const STRAYED: &str = " · set by no key here";
/// What is said while the mode is being changed, in the key's place.
const CHANGING: &str = " (changing…)";

/// What kind of mode it is, which its colour says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    /// Everything that would ask, asks; or no mode was said.
    #[default]
    Quiet,
    /// Edits run unasked.
    Edits,
    /// Claude plans.
    Plan,
    /// Claude Code judges what runs unasked.
    Auto,
    /// Anything may run unasked, or oso does not know the mode, or cannot show it.
    Alarm,
}

/// What is known of a session's mode, for its footer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Said<'a> {
    /// A process runs, or starts.
    pub runs: bool,
    /// The mode it said it is in, if it can be shown.
    pub mode: Option<&'a str>,
    /// It said one that cannot.
    pub unshown: bool,
    /// No key here gave it (ADR 0181).
    pub strayed: bool,
    /// Another is asked for, and not said yet.
    pub changing: bool,
}

/// A native session's footer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footer {
    tone: Tone,
    mode: Measured,
    strayed: bool,
    changing: bool,
    model: Measured,
}

/// A text, and the cells it takes, measured as it is made and not in each frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Measured {
    text: String,
    cells: usize,
}

impl Measured {
    fn new(text: String) -> Self {
        let cells = cells_of(&text);
        Self { text, cells }
    }
}

/// A footer's parts, as they fit a window: each whole or empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts<'a> {
    pub tone: Tone,
    pub mode: &'a str,
    pub strayed: &'static str,
    /// What follows in the dim face: the key, or that the mode is being changed.
    pub then: &'a str,
    pub model: &'a str,
    /// The cells the model takes.
    pub model_cells: usize,
}

impl Footer {
    /// The footer of a session of whose mode `said` is known, and whose model the status line calls
    /// `model`, if it knows one.
    pub fn of(said: Said<'_>, model: &str) -> Self {
        let (tone, mode) = match said.mode {
            _ if !said.runs => (Tone::Quiet, String::new()),
            Some(mode) => words(mode),
            None if said.unshown => (Tone::Alarm, "⚠ mode not shown".to_owned()),
            None => (Tone::Quiet, "mode not known".to_owned()),
        };
        let (kept, cut) = cut_to(model, MOST);
        Self {
            tone,
            strayed: said.strayed && said.runs,
            changing: said.changing && said.runs,
            mode: Measured::new(mode),
            model: Measured::new(format!("{kept}{cut}")),
        }
    }

    /// Its parts in `columns` cells, with `key` after the mode while the mode can be switched there.
    pub fn parts<'a>(&'a self, columns: usize, key: &'a str) -> Parts<'a> {
        let strayed = if self.strayed { STRAYED } else { "" };
        let then = match () {
            () if self.mode.text.is_empty() => "",
            () if self.changing => CHANGING,
            () => key,
        };
        let mut parts = Parts {
            tone: self.tone,
            mode: &self.mode.text,
            strayed,
            then,
            model: &self.model.text,
            model_cells: self.model.cells,
        };
        let model = match self.model.cells {
            0 => 0,
            cells => cells + GAP,
        };
        let room = columns.saturating_sub(self.mode.cells);
        let strayed = cells_of(strayed);
        if cells_of(then) + strayed + model > room {
            parts.then = "";
            if strayed + model > room {
                (parts.model, parts.model_cells) = ("", 0);
                if strayed > room {
                    parts.strayed = "";
                }
            }
        }
        parts
    }
}

/// The colour and the words of the mode the CLI calls `name`, as its terminal has them (CLI 2.1.331),
/// behind a mark that no text of the stream's here can hold.
fn words(name: &str) -> (Tone, String) {
    let (tone, words) = match (PermissionMode::named(name), name) {
        (Some(PermissionMode::Default), _) => (Tone::Quiet, "⏸ manual mode on"),
        (Some(PermissionMode::AcceptEdits), _) => (Tone::Edits, "▶▶ accept edits on"),
        (Some(PermissionMode::Plan), _) => (Tone::Plan, "⏸ plan mode on"),
        (Some(PermissionMode::Auto), _) => (Tone::Auto, "▶▶ auto mode on"),
        // What oso never sends.
        (None, "bypassPermissions") => (Tone::Alarm, "▶▶ bypass permissions on"),
        (None, "dontAsk") => (Tone::Alarm, "▶▶ don't ask on"),
        (None, other) => {
            let (kept, cut) = cut_to(other, MOST);
            return (Tone::Alarm, format!("⚠ mode {kept}{cut}"));
        }
    };
    (tone, words.to_owned())
}

#[cfg(test)]
mod tests;
