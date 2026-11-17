//! A language server's code actions, as `eglot-code-actions` reads one (ADR 0138, step 13): the
//! list of an answer's actions, which `RET` gives to `lsp-code-action-apply`, and a server's
//! edits applied to the buffers they name.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! opens the list once an answer comes, resolves the picked action and hands its edits over.
//!
//! - The list shows every action in the server's order, the first one the server prefers
//!   selected, each with its kind beside it (Q2). One the server disabled is drawn faded, with
//!   its reason, and `RET` on it says the reason and keeps the list open.
//! - A server's edit applies whole or not at all (Q8): every buffer it names must be there, may
//!   hold no secrets, be writable and be at the revision the server had, and no two of its edits
//!   in one buffer may overlap. Each buffer's edits are then one undo step.
//! - Nothing of an action reaches a log: its title is the server's text, and an edit's text may
//!   quote what the server read.

use std::ops::Range;
use std::sync::Arc;

use crate::buffer::{BufferId, Replacement};
use crate::complete::Candidate;
use crate::editor::{CompletionStyle, Editor};
use crate::minibuffer::{PromptError, PromptId};

/// One action of a list, as it shows: its title, its kind, whether the server prefers it, and
/// why it cannot be applied, if the server disabled it.
#[derive(Clone, PartialEq, Eq)]
pub struct CodeActionItem {
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
    pub disabled: Option<String>,
}

/// One buffer's part of a server's edit: its edits, in bytes of its revision `revision`, in
/// the server's order.
#[derive(Clone, PartialEq, Eq)]
pub struct BufferEdit {
    pub buffer: BufferId,
    pub revision: u64,
    pub edits: Vec<(Range<usize>, String)>,
}

/// What a server's edit changed: how many edits, in how many buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditApplied {
    pub edits: usize,
    pub buffers: usize,
}

/// What the list of code actions keeps beside its rows: its prompt, each action's reason it is
/// disabled, if it is, and the action `RET` took, for the host.
#[derive(Debug, Default)]
pub(in crate::editor) struct ActionList {
    prompt: Option<PromptId>,
    disabled: Vec<Option<Box<str>>>,
    hit: Option<usize>,
}

impl Editor {
    /// Opens the list of `items`, a server's code actions, the first one the server prefers and
    /// has not disabled selected, else the first.
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be, and
    /// [`PromptError::Invalid`] while a list of code actions is open already, or with none.
    /// Nothing changes.
    pub fn open_code_actions(&mut self, items: &[CodeActionItem]) -> Result<PromptId, PromptError> {
        if self.servers.actions.prompt.is_some() || items.is_empty() {
            return Err(PromptError::Invalid);
        }
        let candidates: Arc<[Candidate]> = items
            .iter()
            .map(|item| {
                let kind = item.kind.as_deref().unwrap_or("");
                let annotation = match &item.disabled {
                    Some(reason) if reason.is_empty() => format!("{kind} (disabled)"),
                    Some(reason) => format!("{kind} (disabled: {reason})"),
                    None => kind.to_owned(),
                };
                Candidate::new(item.title.as_str(), "", annotation.trim_start())
            })
            .collect();
        let style = CompletionStyle {
            given_order: true,
            ..CompletionStyle::default()
        };
        let id = self.open_completion_as("Code action:", "lsp-code-actions", candidates, style)?;
        let preferred = (items.iter()).position(|item| item.preferred && item.disabled.is_none());
        if let Some(preferred) = preferred
            && let Some((candidates, _)) =
                (self.prompts.last_mut()).and_then(|prompt| prompt.completion.as_mut())
        {
            candidates.select(preferred);
        }
        self.servers.actions = ActionList {
            prompt: Some(id),
            disabled: (items.iter())
                .map(|item| item.disabled.as_deref().map(Box::from))
                .collect(),
            hit: None,
        };
        self.relayout_prompt();
        Ok(id)
    }

    /// Whether the action shown at `place` of the innermost prompt is disabled, if the prompt
    /// lists code actions: its row is drawn faded.
    pub fn code_action_disabled(&self, place: usize) -> Option<bool> {
        if !self.lists_code_actions() {
            return None;
        }
        let item = self.prompts.last()?.candidates()?.item_at(place)?;
        Some(matches!(
            self.servers.actions.disabled.get(item),
            Some(Some(_))
        ))
    }

    /// Whether the innermost prompt lists code actions.
    fn lists_code_actions(&self) -> bool {
        self.servers.actions.prompt.is_some()
            && self.prompts.last().map(|prompt| prompt.id) == self.servers.actions.prompt
    }

    /// The title of the innermost prompt's selected action, if it lists code actions: its place
    /// is kept for the host ([`Editor::take_code_action_hit`]). A disabled one is refused, and
    /// the prompt says why. Nothing goes in a history.
    pub(in crate::editor) fn code_action_answer(&mut self) -> Option<Result<String, PromptError>> {
        if !self.lists_code_actions() {
            return None;
        }
        let candidates = self.prompts.last()?.candidates()?;
        let Some(item) = candidates.selected_item() else {
            return Some(Err(PromptError::Invalid));
        };
        let title = candidates.items().get(item)?.text().to_owned();
        if let Some(Some(reason)) = self.servers.actions.disabled.get(item) {
            let why = if reason.is_empty() {
                "This code action is disabled".to_owned()
            } else {
                format!("This code action is disabled: {reason}")
            };
            self.prompt_says(why);
            return Some(Err(PromptError::Invalid));
        }
        self.servers.actions.hit = Some(item);
        Some(Ok(title))
    }

    /// The place in its answer of the action the last list of code actions' `RET` took, once.
    pub fn take_code_action_hit(&mut self) -> Option<usize> {
        self.servers.actions.hit.take()
    }

    /// Forgets the list of code actions, once its prompt has closed; the action it took stays
    /// for the host.
    pub(in crate::editor) fn forget_code_actions(&mut self, closed: PromptId) {
        if self.servers.actions.prompt == Some(closed) {
            let hit = self.servers.actions.hit;
            self.servers.actions = ActionList {
                hit,
                ..ActionList::default()
            };
        }
    }

    /// Applies `edit`, a server's edit of buffers, whole or not at all: each buffer's edits as one
    /// undo step, the cursors moved as the edits move them.
    ///
    /// # Errors
    ///
    /// Why nothing changed, for the echo area: a buffer that is gone, may hold secrets, cannot be
    /// edited or changed since the server's revision, or edits that overlap.
    pub fn apply_server_edit(&mut self, edit: &[BufferEdit]) -> Result<EditApplied, &'static str> {
        let mut checked = Vec::with_capacity(edit.len());
        for part in edit {
            let buffer = self
                .buffer(part.buffer)
                .ok_or("A buffer it edits is gone")?;
            if buffer.is_secret() {
                return Err("No server edits a buffer that may hold secrets");
            }
            if buffer.read_only().is_some() {
                return Err("A buffer it edits is read-only");
            }
            if buffer.revision() != part.revision {
                return Err("A buffer it edits changed since the server answered");
            }
            checked.push((part.buffer, joined(&part.edits)?));
        }
        let mut applied = EditApplied {
            edits: 0,
            buffers: 0,
        };
        // Nothing failed above, and each buffer was checked writable: each applies.
        for (id, edits) in checked {
            if edits.is_empty() {
                continue;
            }
            let replacements: Vec<Replacement<'_>> = (edits.iter())
                .map(|(range, text)| Replacement {
                    range: range.clone(),
                    text,
                })
                .collect();
            self.apply_edits(id, &replacements)
                .map_err(|_| "A buffer it edits could not be edited")?;
            applied.edits += edits.len();
            applied.buffers += 1;
        }
        Ok(applied)
    }
}

/// `edits` in offset order, the insertions at one place joined in the server's order, the one
/// that starts a replacement with it, or why they overlap.
fn joined(edits: &[(Range<usize>, String)]) -> Result<Vec<(Range<usize>, String)>, &'static str> {
    let mut sorted: Vec<&(Range<usize>, String)> = edits.iter().collect();
    // Stable: insertions at one place keep the server's order.
    sorted.sort_by_key(|(range, _)| range.start);
    let mut out: Vec<(Range<usize>, String)> = Vec::with_capacity(sorted.len());
    for (range, text) in sorted {
        if range.end < range.start {
            return Err("Its edits do not read");
        }
        match out.last_mut() {
            Some((last, joined)) if last.start == last.end && last.start == range.start => {
                joined.push_str(text);
                last.end = range.end;
            }
            Some((last, _)) if range.start < last.end => {
                return Err("Its edits overlap");
            }
            _ => out.push((range.clone(), text.clone())),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
