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
//!
//! Each document also keeps the texts it was given last, so that a late diagnostics report is
//! placed against the text it names and mapped forward to the latest (`history`, step 6).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use oso_core::syntax::Splice;
use serde_json::{Value, json};

use crate::limits::DOCUMENT_BYTES_MAX;
use crate::position::{self, Encoding};
use crate::uri;

use super::{Closed, Document};

mod history;
mod sent;
#[cfg(test)]
mod tests;

pub(super) use history::Diagnose;
use history::Given;

/// One document, as the session thread keeps it.
#[derive(Debug)]
struct Kept {
    uri: String,
    /// The path its URI reads as, which a report's is compared with: on Windows its drive
    /// letter is upper case, whatever case the server writes it in.
    path: Option<PathBuf>,
    language_id: String,
    /// The text as the server has it, or will once what waits is sent.
    text: Rope,
    version: i32,
    /// Opened with the server in this run.
    sent: bool,
    /// A whole text waits to be sent, as room in the writer's queue comes.
    waits: Waits,
    /// The person saved it while a whole text waited: `didSave` follows that text.
    save_after: bool,
    /// Kept from the server, and why.
    closed: Option<Closed>,
    /// The texts given last, oldest first: the last is `text`.
    given: VecDeque<Given>,
}

/// A whole text that waits for room in the writer's queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waits {
    Nothing,
    /// The document's `didOpen`.
    Open,
    /// The server has an older text: a change of the whole text.
    Whole,
}

/// What the session thread should send now, and what it should report, for one ask. Whole
/// texts are not here: they wait, and [`Documents::next_whole`] gives them one at a time.
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

/// The documents, by the caller's id, and the paths opened since the session last asked.
#[derive(Debug, Default)]
pub(super) struct Documents(BTreeMap<u64, Kept>, Vec<PathBuf>);

impl Documents {
    /// A new document. `live` says whether the server is ready to be told: its open then waits
    /// for room.
    pub(super) fn open(&mut self, document: Document, live: bool) -> Out {
        let closed = why_not(&document.text);
        let given = VecDeque::from([Given {
            revision: document.revision,
            text: document.text.clone(),
            splices: None,
            version: None,
        }]);
        let kept = Kept {
            path: uri::to_path(&document.uri),
            uri: document.uri,
            language_id: document.language_id,
            text: document.text,
            version: 0,
            sent: false,
            waits: if live && closed.is_none() {
                Waits::Open
            } else {
                Waits::Nothing
            },
            save_after: false,
            closed,
            given,
        };
        self.0.insert(document.id, kept);
        // Said of every open, so that a report about an earlier document of the same id never
        // stands for this one.
        Out {
            send: Vec::new(),
            state: Some(closed),
        }
    }

    /// A batch of edits to document `id`: `before` the text as last given, `after` the text now,
    /// `splices` the batch from one to the other if the caller has it. A batch is placed once,
    /// against the text the server has, and that answer decides both the carriage return check
    /// and what goes: the changes, or a whole text that waits. Unless `incremental`, it is a
    /// whole text. A document not sent yet is judged as one sent is. `revision` is the caller's
    /// revision of `after`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the parts of a change, apart, so that a test gives them as it builds them"
    )]
    pub(super) fn change(
        &mut self,
        id: u64,
        revision: u64,
        before: &Rope,
        after: Rope,
        splices: Option<&[Splice]>,
        encoding: Encoding,
        incremental: bool,
    ) -> Out {
        let Some(kept) = self.0.get_mut(&id) else {
            return Out::default();
        };
        let mut out = Out::default();
        // The caller's before is the text it last gave; the session's is what the server has.
        let placed = splices
            .filter(|_| before.len() == kept.text.len())
            .and_then(|splices| placed(&kept.text, &after, splices));
        kept.give(Given {
            revision,
            text: after.clone(),
            splices: placed
                .as_ref()
                .map(|placed| placed.iter().map(|(splice, _)| *splice).collect()),
            version: None,
        });
        if kept.closed.is_some() {
            // Kept from the server: its edits are taken in, for a save to open it again.
            kept.text = after;
            return out;
        }
        let reason = if after.len() > DOCUMENT_BYTES_MAX {
            Some(Closed::TooLarge)
        } else if placed.as_ref().map_or_else(
            || lone_cr(&after),
            |placed| lone_cr_near(&after, placed.iter().map(|(splice, _)| splice)),
        ) {
            Some(Closed::LoneCarriageReturn)
        } else {
            None
        };
        if let Some(reason) = reason {
            if kept.sent {
                out.send
                    .push(("textDocument/didClose", identifier(&kept.uri)));
            }
            kept.sent = false;
            kept.waits = Waits::Nothing;
            kept.save_after = false;
            kept.closed = Some(reason);
            kept.text = after;
            out.state = Some(Some(reason));
            return out;
        }
        let in_step = kept.sent && kept.waits == Waits::Nothing;
        match placed.filter(|_| incremental && in_step) {
            Some(placed) => {
                let changes = changes_of(&kept.text, &after, &placed, encoding);
                kept.version += 1;
                kept.went_as(kept.version);
                out.send.push((
                    "textDocument/didChange",
                    json!({
                        "textDocument": {"uri": kept.uri, "version": kept.version},
                        "contentChanges": changes,
                    }),
                ));
            }
            // Not open yet, or its open or a whole text waits: that takes the text now. Else a
            // whole text waits, as the latest text when it goes.
            None if kept.sent && kept.waits == Waits::Nothing => kept.waits = Waits::Whole,
            None => {}
        }
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

    /// The person saved document `id`: `didSave` if it is open, after the whole text if one
    /// waits; if it was kept from the server for a reason that no longer holds, its open waits
    /// again (step 4's Q2). `live` as for [`Self::open`].
    pub(super) fn save(&mut self, id: u64, live: bool) -> Out {
        let Some(kept) = self.0.get_mut(&id) else {
            return Out::default();
        };
        if kept.closed.is_some() {
            if why_not(&kept.text).is_some() {
                return Out::default();
            }
            kept.closed = None;
            if live {
                kept.waits = Waits::Open;
            }
            return Out {
                send: Vec::new(),
                state: Some(None),
            };
        }
        if !kept.sent {
            return Out::default();
        }
        if kept.waits != Waits::Nothing {
            kept.save_after = true;
            return Out::default();
        }
        Out::send("textDocument/didSave", identifier(&kept.uri))
    }

    /// A run ended: no document is open with the next one until it is ready.
    pub(super) fn forget_sent(&mut self) {
        for kept in self.0.values_mut() {
            kept.sent = false;
            kept.waits = Waits::Nothing;
            kept.save_after = false;
        }
    }

    /// The server is ready: every document not kept from it waits to be opened.
    pub(super) fn open_all(&mut self) {
        for kept in self.0.values_mut() {
            if kept.closed.is_none() && !kept.sent {
                kept.waits = Waits::Open;
            }
        }
    }

    /// Whether a whole text waits.
    pub(super) fn waiting(&self) -> bool {
        self.0.values().any(|kept| kept.waits != Waits::Nothing)
    }

    /// The next whole text that waits, as notifications to send in order: an open, or a change of
    /// the whole text and the save that waited for it. Built only as it goes, so that what
    /// waits takes no more than the documents' ropes.
    pub(super) fn next_whole(&mut self) -> Option<Vec<(&'static str, Value)>> {
        let kept = self
            .0
            .values_mut()
            .find(|kept| kept.waits != Waits::Nothing)?;
        let waited = std::mem::replace(&mut kept.waits, Waits::Nothing);
        kept.version += 1;
        kept.went_as(kept.version);
        let mut send = Vec::with_capacity(2);
        match waited {
            Waits::Open => {
                kept.sent = true;
                self.1.extend(kept.path.clone());
                send.push((
                    "textDocument/didOpen",
                    json!({"textDocument": {
                        "uri": kept.uri,
                        "languageId": kept.language_id,
                        "version": kept.version,
                        "text": kept.text.to_string(),
                    }}),
                ));
            }
            Waits::Whole | Waits::Nothing => send.push((
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": kept.uri, "version": kept.version},
                    "contentChanges": [{"text": kept.text.to_string()}],
                }),
            )),
        }
        if std::mem::take(&mut kept.save_after) {
            send.push(("textDocument/didSave", identifier(&kept.uri)));
        }
        Some(send)
    }

    /// The paths opened with the server since the last call, for a report kept from before.
    pub(super) fn take_opened(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.1)
    }

    /// Whether a document open with the server, or waiting to be, is at `path`.
    pub(super) fn has_path(&self, path: &Path) -> bool {
        self.0
            .values()
            .any(|kept| kept.path.as_deref() == Some(path))
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
pub(super) fn lone_cr_near<'a>(
    after: &Rope,
    splices: impl IntoIterator<Item = &'a Splice>,
) -> bool {
    let mut shift = 0isize;
    for splice in splices {
        let start = splice.start.saturating_add_signed(shift);
        shift += splice.new_len as isize - splice.old_len as isize;
        // A byte before, the new text, and a byte after: \r and \n are ASCII, so whole
        // characters around them do.
        let low = after.clip_offset(start.saturating_sub(1), Bias::Left);
        let high = after.clip_offset(start + splice.new_len + 2, Bias::Right);
        // The bytes in between, through the chunks, with a `\r` waiting for the next one.
        let mut after_cr = false;
        for chunk in after.chunks_in_range(low..high) {
            for &byte in chunk.as_bytes() {
                if after_cr && byte != b'\n' {
                    return true;
                }
                after_cr = byte == b'\r';
            }
        }
        if after_cr && high >= after.len() {
            return true;
        }
    }
    false
}

/// The content changes that take `before` to `after`: one for each splice, highest first, or
/// the whole text when `splices` are missing or do not place within the texts.
#[cfg(test)]
pub(super) fn content_changes(
    before: &Rope,
    after: &Rope,
    splices: Option<&[Splice]>,
    encoding: Encoding,
) -> Vec<Value> {
    match splices.and_then(|splices| placed(before, after, splices)) {
        Some(placed) => changes_of(before, after, &placed, encoding),
        None => vec![json!({"text": after.to_string()})],
    }
}

/// One change for each placed splice, highest first.
fn changes_of(
    before: &Rope,
    after: &Rope,
    placed: &[(Splice, usize)],
    encoding: Encoding,
) -> Vec<Value> {
    placed
        .iter()
        .rev()
        .map(|&(splice, at)| {
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
        .collect()
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
