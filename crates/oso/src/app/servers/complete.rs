//! Completion at point (ADR 0138, Completion in the buffer, steps 9 and 9b), on the UI thread:
//! the ask `completion-at-point` makes, the asks the person's typing makes by itself, an
//! item's resolve, and their answers, which open the popup or give an item its edits besides
//! its own. Every ask goes after the event's edits went to the server.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - The person's typing asks at once after one of the server's trigger characters, and once
//!   [`REST`] has passed with three identifier characters typed and nothing moved, as
//!   `corfu-auto` with `corfu-auto-prefix` 3 does. With the popup open on a list the server said
//!   is incomplete, it asks again at once. Only the person's typing (the dispatcher's note).
//! - An ask the host makes by itself says nothing of no answer or of a failed one.
//! - An answer for another revision, cursor, window or buffer, or once a prompt opened, is
//!   dropped: the editor judges it, as it opens the popup.
//! - A row shows its label, then its kind's word and its detail's first line, which the editor
//!   cuts and makes the runs of blanks of one.
//! - A stale answer goes to the freeing thread; the editor frees one it drops, and a popup's
//!   items once it closes, on its worker.

use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::editor::{AskAt, CompletionPopupAnswer, CompletionPopupItem};
use oso_lsp::protocol::{Invoked, kind_name};
use oso_lsp::server::{AskContext, AskKind, Completions, Resolved, State};

use super::super::App;
use super::asks::{Asker, Who};

/// The most bytes of a detail an annotation copies: more than a row shows.
const DETAIL_SHOWN: usize = 256;

/// How long the person's typing rests before it asks by itself (ADR 0138, Q20).
pub(super) const REST: Duration = Duration::from_millis(200);

/// The identifier characters typed before the typing asks by itself (Q20's answer).
const PREFIX: usize = 3;

impl App {
    /// Opens the popup for `completions`, which server `index` answered to `asker`'s ask
    /// about `asked`, unless the ask is stale.
    pub(super) fn completed(
        &mut self,
        index: usize,
        (asked, asker): (BufferId, Asker),
        completions: Completions,
    ) {
        if !self.still_there(asked, &asker) {
            self.freer.free(completions);
            // The typing went on past an incomplete list's ask: one more, from where it is now.
            let incomplete = self
                .editor
                .completion_popup()
                .is_some_and(|popup| popup.incomplete());
            if asker.quiet() && incomplete {
                let at = self.editor.ask_at();
                self.ask_completions(index, at, Invoked::Incomplete);
            }
            return;
        }
        let Completions {
            revision,
            items,
            incomplete,
        } = completions;
        let items = items
            .into_iter()
            .map(|item| {
                // The kind's word and the detail's first line, at most what a row shows; the
                // editor makes its runs of blanks one.
                let detail = item.detail.lines().next().unwrap_or_default();
                let detail = &detail[..detail.floor_char_boundary(DETAIL_SHOWN)];
                let mut annotation = String::with_capacity(16 + detail.len());
                annotation.push_str(item.kind.map_or("", kind_name));
                annotation.push(' ');
                annotation.push_str(detail);
                CompletionPopupItem {
                    label: item.label.into(),
                    annotation: annotation.into(),
                    filter: item.filter.map(Into::into),
                    text: item.text.into(),
                    range: item.range,
                    extra: (item.extra.into_iter())
                        .map(|(range, text)| (range, text.into()))
                        .collect(),
                }
            })
            .collect();
        // Where it was asked: the editor judges the answer against it too.
        let answer = CompletionPopupAnswer {
            serial: asker.serial(),
            at: asker.at(asked),
            revision,
            items,
            incomplete,
        };
        if let Err(Some(why)) = self.editor.open_completion_popup(answer)
            && !asker.quiet()
        {
            self.editor.message(why);
        }
        self.ask_resolve(index);
        self.request_redraw();
    }

    /// After the event's edits went to the servers: what the person typed asks for
    /// completions at once, or once it rests, and the selected item's resolve is asked for.
    pub(super) fn complete_as_typed(&mut self) {
        if let Some(at) = self.editor.take_completion_typed() {
            self.typed(at);
        }
        let selected = self
            .editor
            .completion_popup()
            .map(|_| self.editor.ask_at().buffer);
        if let Some(index) = selected.and_then(|buffer| self.completing_server(buffer)) {
            self.ask_resolve(index);
        }
    }

    /// The person typed `typed`: a trigger character asks at once, as does typing on in an
    /// incomplete list; three identifier characters ask once the typing rests.
    fn typed(&mut self, at: AskAt) {
        self.servers.typing_rest = None;
        let Some(index) = self.completing_server(at.buffer) else {
            return;
        };
        let Some(typed) = self.editor.completion_typed(at) else {
            return;
        };
        let triggers = (self.servers.running[index].completing.as_ref())
            .is_some_and(|completing| completing.triggers.contains(&typed.last));
        // One ask of an incomplete list at a time: the popup narrows meanwhile, and a stale
        // answer asks again from where the typing is.
        let asking =
            (self.servers.running[index].asked).contains_key(&(at.buffer, AskKind::Completion));
        let invoked = match self.editor.completion_popup() {
            Some(popup) if popup.incomplete() && !asking => Some(Invoked::Incomplete),
            // An open popup narrows by itself.
            Some(_) => return,
            None if triggers => Some(Invoked::Character(typed.last)),
            None => None,
        };
        match invoked {
            Some(invoked) => self.ask_completions(index, at, invoked),
            None if typed.word_clusters >= PREFIX => {
                self.servers.typing_rest = Some((at, Instant::now() + REST));
            }
            None => {}
        }
    }

    /// Before the loop waits: typing that has rested asks, if nothing moved since.
    pub(in crate::app) fn completion_if_due(&mut self) {
        let Some((at, due)) = self.servers.typing_rest else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.servers.typing_rest = None;
        let moved = self.editor.ask_at() != at
            || self.editor.prompt().is_some()
            || self.editor.completion_popup().is_some();
        if let (false, Some(index)) = (moved, self.completing_server(at.buffer)) {
            self.ask_completions(index, at, Invoked::Invoked);
        }
    }

    /// When typing that waits asks, for the loop's wake.
    pub(in crate::app) fn completion_deadline(&self) -> Option<Instant> {
        self.servers.typing_rest.map(|(_, due)| due)
    }

    /// Asks server `index` for completions at `at`, by itself, saying nothing of no answer.
    fn ask_completions(&mut self, index: usize, at: AskAt, invoked: Invoked) {
        let context = AskContext::Completion(invoked);
        self.send_ask(index, (at, AskKind::Completion, context), Who::Host);
    }

    /// Asks server `index` to resolve the popup's selected item, if it resolves and the item
    /// has not been asked about.
    fn ask_resolve(&mut self, index: usize) {
        let resolves = (self.servers.running.get(index))
            .and_then(|running| running.completing.as_ref())
            .is_some_and(|completing| completing.resolves);
        if !resolves {
            return;
        }
        let Some((answer, item)) = self.editor.take_completion_resolve() else {
            return;
        };
        let at = self.editor.ask_at();
        let context = AskContext::Item {
            answer,
            index: item,
        };
        self.send_ask(index, (at, AskKind::Resolve, context), Who::Host);
    }

    /// An item's resolve came: its edits besides its own go with it, or in its insert's undo
    /// step.
    pub(super) fn resolved(&mut self, resolved: Resolved) {
        let Resolved {
            answer,
            index,
            revision,
            extra,
        } = resolved;
        if self
            .editor
            .completion_resolved((answer, index), revision, extra)
        {
            self.request_redraw();
        }
    }

    /// The server to ask by itself about `buffer`: one that follows it, runs and completes;
    /// nothing is said of none.
    fn completing_server(&self, buffer: BufferId) -> Option<usize> {
        let index = self.running_for(buffer)?;
        let running = &self.servers.running[index];
        let open = running
            .documents
            .get(&buffer)
            .is_some_and(|open| open.closed.is_none());
        let runs = matches!(running.state, State::Ready | State::Working(_));
        (open && runs && running.completing.is_some()).then_some(index)
    }
}
