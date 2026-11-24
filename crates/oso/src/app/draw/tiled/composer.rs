//! A native session's composer as its window is drawn (ADR 0170): the rules above and below it, each
//! along the middle of a row of its own, half a row from its text, or, where its window has no such
//! rows, in its own rows, over its gutter and its text.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! - A rule is the composer's alone, so that it shows where a text to send is written: a card, the
//!   strip, a request's line and any other buffer docked in the composer's place have none.

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::window::{DockSlot, WindowId};
use oso_render::{Bands, Face, Layer, Pane, PaneContent, Rules};

use super::{Part, pane_id};
use crate::app::draw::card::dock_face;

/// The lines along a composer's edges, in the prompt's dim colour, as its placeholder is.
const RULES: Rules = Rules {
    above: Some(Face::Prompt),
    below: Some(Face::Prompt),
};

/// Whether window `id` is a composer's. A question's answer, once one is written in the composer's
/// place (ADR 0182, R2), is to have its rules and padding, which the editor's docks give by the same
/// test, and not its keys, which the footer asks for: the two are then apart.
pub(super) fn is_composer(editor: &Editor, id: WindowId) -> bool {
    (editor.window_by_id(id))
        .filter(|window| matches!(window.docked(), Some((_, DockSlot::Composer))))
        .is_some_and(|window| editor.shown(window).mode() == Mode::Composer)
}

/// The rules in window `id`'s own rows: a composer's that has no rows around its text, else none.
pub(super) fn rules_of(editor: &Editor, id: WindowId) -> Rules {
    match editor.frame().padding(id) {
        None if is_composer(editor, id) => RULES,
        _ => Rules::NONE,
    }
}

/// Pushes the rules of docked window `id` that are in rows of their own, around its text: the text's
/// background beyond them, and its own within.
pub(super) fn rule_panes(editor: &Editor, id: WindowId, list: &mut Vec<Pane<'_>>) {
    let Some([above, below]) = editor.frame().padding(id) else {
        return;
    };
    let rule = |region, part, [above, below]: [Face; 2]| Pane {
        id: pane_id(id, part),
        region,
        layer: Layer::TILED,
        background: None,
        panel: None,
        bands: Bands::NONE,
        rules: Rules::NONE,
        content: PaneContent::Rule {
            ink: Face::Prompt,
            above,
            below,
        },
    };
    let within = dock_face(editor, id).unwrap_or(Face::Float);
    list.push(rule(above, Part::RuleAbove, [Face::Text, within]));
    list.push(rule(below, Part::RuleBelow, [within, Face::Text]));
}

#[cfg(test)]
mod tests;
