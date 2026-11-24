//! A native session's composer as its window is drawn (ADR 0170): the rules above and below it,
//! in its own rows, over its gutter and its text, and the one background both are on.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! - A rule is the composer's alone, so that it shows where a text to send is written: a card, the
//!   strip, a request's line and any other buffer docked in the composer's place have none.

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::window::{DockSlot, WindowId};
use oso_render::{Face, Rules};

/// The lines along a composer's edges, in the prompt's dim colour, as its placeholder is.
const RULES: Rules = Rules {
    above: Some(Face::Prompt),
    below: Some(Face::Prompt),
};

/// The rules of window `id`: a composer's, else none. A question's answer, once one is written in
/// the composer's place (ADR 0182, R2), adds its mode here.
pub(super) fn rules_of(editor: &Editor, id: WindowId) -> Rules {
    let composer = (editor.window_by_id(id))
        .filter(|window| matches!(window.docked(), Some((_, DockSlot::Composer))))
        .is_some_and(|window| editor.shown(window).mode() == Mode::Composer);
    if composer { RULES } else { Rules::NONE }
}

/// The rules of what is drawn over the first row of composer `id` alone, its placeholder, so that
/// it is cut as the composer's text is.
pub(super) fn first_row_rules(editor: &Editor, id: WindowId) -> Rules {
    let one = (editor.regions(id)).is_some_and(|regions| regions.text.rows == 1);
    let rules = rules_of(editor, id);
    Rules {
        below: rules.below.filter(|_| one),
        ..rules
    }
}

/// The background of a window's gutter with `rules`: the float's, as a composer's text is on, else
/// none of its own.
pub(super) fn gutter_background(rules: Rules) -> Option<Face> {
    (rules != Rules::NONE).then_some(Face::Float)
}

#[cfg(test)]
mod tests;
