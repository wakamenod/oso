//! An open menu's switches, and what a describe function says of a menu as it is drawn
//! (ADR 0117).
//!
//! Must not depend on: `keymap`. A describe function reads the editor and the open menu only, so
//! it does no I/O.

use std::sync::Arc;

use super::{Definition, HeadAction};
use crate::editor::Editor;

/// What a built-in menu's describe function is: called as the menu is drawn, once after each key
/// and when a worker's report asks for a redraw. It must not show a buffer's text.
pub type Describe = fn(&Editor, &Open) -> Notes;

/// One open menu: the definition it opened with, and its switches' values, which start off each
/// time it opens (Q18).
#[derive(Debug, Clone)]
pub struct Open {
    definition: Arc<Definition>,
    /// Each head's switch value, by the head's index; always off for a head that is no switch.
    on: Vec<bool>,
}

impl Open {
    /// `definition`, opened with every switch off.
    pub fn new(definition: Arc<Definition>) -> Self {
        let on = vec![false; definition.heads().len()];
        Self { definition, on }
    }

    /// The definition it opened with: a reload since does not change it.
    pub fn definition(&self) -> &Definition {
        &self.definition
    }

    /// Whether the switch `name` is on. Off for a name no head flips.
    pub fn switch(&self, name: &str) -> bool {
        self.head_of(name).is_some_and(|head| self.on[head])
    }

    /// Flips the switch `name`, and returns its value now. Changes nothing, and returns false,
    /// for a name no head flips.
    pub fn flip(&mut self, name: &str) -> bool {
        let Some(head) = self.head_of(name) else {
            return false;
        };
        self.on[head] = !self.on[head];
        self.on[head]
    }

    /// The names of the switches that are on, in the order of their heads.
    pub fn switches_on(&self) -> impl Iterator<Item = &str> {
        self.definition
            .heads()
            .iter()
            .zip(&self.on)
            .filter_map(|(head, &on)| match &head.action {
                HeadAction::Switch(name) if on => Some(name.as_str()),
                _ => None,
            })
    }

    /// The head that flips the switch `name`: there is at most one.
    fn head_of(&self, name: &str) -> Option<usize> {
        self.definition
            .heads()
            .iter()
            .position(|head| matches!(&head.action, HeadAction::Switch(n) if n == name))
    }
}

/// What a describe function says of its menu as it is drawn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// A line under the title, such as the session and project a review is about.
    pub line: Option<String>,
    /// The head `RET` runs, in place of the definition's.
    pub default: Option<usize>,
    /// Notes on heads, by the head's index.
    pub heads: Vec<(usize, Note)>,
}

impl Notes {
    /// The note on `head`, if there is one: the first given for it.
    pub fn get(&self, head: usize) -> Option<&Note> {
        self.heads
            .iter()
            .find(|(index, _)| *index == head)
            .map(|(_, note)| note)
    }
}

/// What a describe function says of one head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Note {
    /// What goes after its label, such as `12 files` or `(last)`; empty for nothing.
    pub text: String,
    /// Why it cannot run now, if it cannot: it is drawn dim, and its key says this and keeps the
    /// menu open.
    pub unavailable: Option<String>,
    /// Whether it is left out of the float. Its key is then a foreign key.
    pub hidden: bool,
}
