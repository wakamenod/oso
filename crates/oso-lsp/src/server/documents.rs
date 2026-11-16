//! The documents a server knows (ADR 0138, Document sync, step 4), kept on its session thread
//! across its runs: each one's text as last sent, its version, and whether it is open with the
//! server or closed for a reason. A batch of edits becomes one change for each splice, highest
//! first, with positions in the agreed encoding from the text before the batch and new text from
//! the text after it; a batch it cannot place goes as the whole text. A document is not sent, or
//! is closed, when it holds a carriage return that ends no line, which oso and a server would
//! count differently, or when it is larger than [`DOCUMENT_BYTES_MAX`].
//!
//! Must not depend on: the session's loop, the pipes or the outbox; this keeps text and builds
//! values, and the session thread sends them.

use std::collections::BTreeMap;

use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use oso_core::syntax::Splice;
use serde_json::{Value, json};

use crate::limits::DOCUMENT_BYTES_MAX;
use crate::position::{self, Encoding};

use super::{Closed, Document};

#[cfg(test)]
mod tests;

/// One document, as the session thread keeps it.
#[derive(Debug)]
struct Kept {
    uri: String,
    language_id: String,
    /// The text as the server has it, or would once opened.
    text: Rope,
    version: i32,
    /// Opened with the server in this run.
    sent: bool,
    /// Kept from the server, and why.
    closed: Option<Closed>,
}

/// What the session thread should send, and what it should report, for one ask.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Out {
    /// Notifications to send, by method and parameters, in order.
    pub(super) send: Vec<(&'static str, Value)>,
    /// A document closed or opened again, for the UI.
    pub(super) state: Option<Option<Closed>>,
}

impl Out {
    fn send(method: &'static str, params: Value) -> Self {
        Self {
            send: vec![(method, params)],
            state: None,
        }
    }
}

/// The documents, by the caller's id.
#[derive(Debug, Default)]
pub(super) struct Documents(BTreeMap<u64, Kept>);

impl Documents {
    /// A new document. `live` says whether the server is ready to be told now.
    pub(super) fn open(&mut self, document: Document, live: bool) -> Out {
        let closed = why_not(&document.text);
        let kept = Kept {
            uri: document.uri,
            language_id: document.language_id,
            text: document.text,
            version: 0,
            sent: false,
            closed,
        };
        let id = document.id;
        self.0.insert(id, kept);
        let mut out = if live {
            self.send_open(id)
        } else {
            Out::default()
        };
        if closed.is_some() {
            out.state = Some(closed);
        }
        out
    }

    /// A batch of edits to document `id`: `before` the text as last given, `after` the text now,
    /// `splices` the batch from one to the other if the caller has it.
    pub(super) fn change(
        &mut self,
        id: u64,
        before: &Rope,
        after: Rope,
        splices: Option<&[Splice]>,
        encoding: Encoding,
    ) -> Out {
        let Some(kept) = self.0.get_mut(&id) else {
            return Out::default();
        };
        // The caller's before is the text it last gave; the session's is what the server has.
        let placed = splices.filter(|_| before.len() == kept.text.len());
        let reason = if after.len() > DOCUMENT_BYTES_MAX {
            Some(Closed::TooLarge)
        } else if placed.map_or_else(|| lone_cr(&after), |s| lone_cr_near(&after, s)) {
            Some(Closed::LoneCarriageReturn)
        } else {
            None
        };
        let mut out = Out::default();
        if !kept.sent || kept.closed.is_some() {
            kept.text = after;
            return out;
        }
        if let Some(reason) = reason {
            out.send
                .push(("textDocument/didClose", identifier(&kept.uri)));
            kept.sent = false;
            kept.closed = Some(reason);
            kept.text = after;
            out.state = Some(Some(reason));
            return out;
        }
        let changes = content_changes(&kept.text, &after, placed, encoding);
        kept.version += 1;
        out.send.push((
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": kept.uri, "version": kept.version},
                "contentChanges": changes,
            }),
        ));
        kept.text = after;
        out
    }

    /// Document `id` is closed by the caller: it goes.
    pub(super) fn close(&mut self, id: u64) -> Out {
        match self.0.remove(&id) {
            Some(kept) if kept.sent => Out::send("textDocument/didClose", identifier(&kept.uri)),
            _ => Out::default(),
        }
    }

    /// The person saved document `id`: `didSave` if it is open; if it was closed for a reason
    /// that no longer holds, it is opened again (step 4's Q2). `live` as for [`Self::open`].
    pub(super) fn save(&mut self, id: u64, live: bool) -> Out {
        let Some(kept) = self.0.get_mut(&id) else {
            return Out::default();
        };
        if kept.closed.is_some() {
            if why_not(&kept.text).is_some() {
                return Out::default();
            }
            kept.closed = None;
            let mut out = if live {
                self.send_open(id)
            } else {
                Out::default()
            };
            out.state = Some(None);
            return out;
        }
        if !kept.sent {
            return Out::default();
        }
        Out::send("textDocument/didSave", identifier(&kept.uri))
    }

    /// A run ended: no document is open with the next one until it is ready.
    pub(super) fn forget_sent(&mut self) {
        self.0.values_mut().for_each(|kept| kept.sent = false);
    }

    /// The server is ready: every document not closed for a reason is opened with its text.
    pub(super) fn open_all(&mut self) -> Out {
        let ids: Vec<u64> = self.0.keys().copied().collect();
        let mut out = Out::default();
        for id in ids {
            out.send.extend(self.send_open(id).send);
        }
        out
    }

    fn send_open(&mut self, id: u64) -> Out {
        let Some(kept) = self.0.get_mut(&id) else {
            return Out::default();
        };
        if kept.sent || kept.closed.is_some() {
            return Out::default();
        }
        kept.sent = true;
        kept.version += 1;
        Out::send(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": kept.uri,
                "languageId": kept.language_id,
                "version": kept.version,
                "text": kept.text.to_string(),
            }}),
        )
    }

    #[cfg(test)]
    pub(super) fn text(&self, id: u64) -> Option<String> {
        self.0.get(&id).map(|kept| kept.text.to_string())
    }
}

fn identifier(uri: &str) -> Value {
    json!({"textDocument": {"uri": uri}})
}

/// Why `text` may not be sent, if it may not.
fn why_not(text: &Rope) -> Option<Closed> {
    if text.len() > DOCUMENT_BYTES_MAX {
        Some(Closed::TooLarge)
    } else if lone_cr(text) {
        Some(Closed::LoneCarriageReturn)
    } else {
        None
    }
}

/// Whether `text` holds a carriage return that is not followed by a line feed. O(n).
pub(super) fn lone_cr(text: &Rope) -> bool {
    let mut after_cr = false;
    for chunk in text.chunks() {
        for &byte in chunk.as_bytes() {
            if after_cr && byte != b'\n' {
                return true;
            }
            after_cr = byte == b'\r';
        }
    }
    after_cr
}

/// Whether the batch `splices`, which made `after`, left a lone carriage return there: one it
/// inserted, one before a splice whose line feed it took, or one it ended with. A text that had
/// none before has none elsewhere. O(splices and the bytes they inserted).
pub(super) fn lone_cr_near(after: &Rope, splices: &[Splice]) -> bool {
    let mut shift = 0isize;
    for splice in splices {
        let start = splice.start.saturating_add_signed(shift);
        shift += splice.new_len as isize - splice.old_len as isize;
        // A byte before, the new text, and a byte after: \r and \n are ASCII, so whole
        // characters around them do.
        let low = after.clip_offset(start.saturating_sub(1), Bias::Left);
        let high = after.clip_offset(start + splice.new_len + 2, Bias::Right);
        let near = after.slice(low..high);
        let near = near.to_string();
        let bytes = near.as_bytes();
        let at_end = high >= after.len();
        for (index, &byte) in bytes.iter().enumerate() {
            if byte != b'\r' {
                continue;
            }
            match bytes.get(index + 1) {
                Some(b'\n') => {}
                Some(_) => return true,
                None if at_end => return true,
                None => {}
            }
        }
    }
    false
}

/// The content changes that take `before` to `after`: one for each splice, highest first, or
/// the whole text when `splices` are missing or do not place within the texts.
pub(super) fn content_changes(
    before: &Rope,
    after: &Rope,
    splices: Option<&[Splice]>,
    encoding: Encoding,
) -> Vec<Value> {
    match splices.and_then(|splices| placed(before, after, splices)) {
        Some(placed) => placed
            .into_iter()
            .rev()
            .map(|(splice, at)| {
                let start = position::from_offset(before, splice.start, encoding);
                let end = position::from_offset(before, splice.start + splice.old_len, encoding);
                let text = after.slice(at..at + splice.new_len).to_string();
                json!({
                    "range": {
                        "start": {"line": start.line, "character": start.character},
                        "end": {"line": end.line, "character": end.character},
                    },
                    "text": text,
                })
            })
            .collect(),
        None => vec![json!({"text": after.to_string()})],
    }
}

/// Each splice with where its new text starts in `after`, if they are ascending and apart, lie
/// within `before` on character starts, and take its length to `after`'s.
fn placed(before: &Rope, after: &Rope, splices: &[Splice]) -> Option<Vec<(Splice, usize)>> {
    let mut placed = Vec::with_capacity(splices.len());
    let (mut end, mut shift) = (0usize, 0isize);
    let on_char =
        |text: &Rope, at: usize| at <= text.len() && text.clip_offset(at, Bias::Left) == at;
    for splice in splices {
        let old_end = splice.start.checked_add(splice.old_len)?;
        if splice.start < end || !on_char(before, splice.start) || !on_char(before, old_end) {
            return None;
        }
        let at = splice.start.checked_add_signed(shift)?;
        if !on_char(after, at) || !on_char(after, at.checked_add(splice.new_len)?) {
            return None;
        }
        placed.push((*splice, at));
        shift += splice.new_len as isize - splice.old_len as isize;
        end = old_end;
    }
    (before.len().checked_add_signed(shift)? == after.len()).then_some(placed)
}
