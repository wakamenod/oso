//! HTML's embedded code: the text of `<script>` and `<style>` elements, parsed with JavaScript's,
//! JSON's or CSS's grammar as their start tag says, and highlighted over HTML's captures (ADR
//! 0157). It is found and painted as Markdown's fenced code is ([`super::fence`]), and the same
//! bookkeeping marks an element that no longer holds code to highlight again.
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and HTML's tree, and runs only
//! on the worker.
//!
//! - A `<script>` is JavaScript without a `type`, or with an empty one, `module` or a JavaScript
//!   MIME type; JSON with `importmap`, `speculationrules` or a JSON MIME type; and stays plain
//!   with any other (a template's, say). A `<style>` is CSS without a `type`, or with `text/css`.
//!   The value is read with ASCII white space trimmed and ignoring ASCII case, and only the first
//!   `type` counts, as in a browser.
//! - The text of an element longer than [`MAX_EMBEDDED_BYTES`] stays plain, as a fence's does,
//!   so that the piece that holds it stays small: a piece widens to the whole of each element
//!   it meets.
//! - Children are walked with a `TreeCursor`, never by recursion.

use std::ops::Range;
use std::time::Instant;

use oso_core::rope::Rope;
use tree_sitter::{Node, Tree};

use super::highlight::{Piece, run};
use super::inline::Nested;
use crate::language::Language;
use crate::queries::{Host, Kind, Queries};

/// The longest element whose text is highlighted with its language, in bytes, the tags too:
/// a fence's limit ([`super::fence::MAX_FENCE_BYTES`]).
pub(crate) const MAX_EMBEDDED_BYTES: usize = super::fence::MAX_FENCE_BYTES;

/// The longest attribute name or `type` value read: longer ones name nothing here.
const MAX_WORD_BYTES: usize = 32;

/// `piece` widened to the whole of each `<script>` and `<style>` element it meets whose text a
/// grammar parses, with those texts and their languages; or how the piece went if the query to
/// find them did not run to its end. Elements never overlap, so the ones the widened piece
/// meets are those.
pub(super) fn widen<'tree>(
    text: &Rope,
    queries: &Queries,
    tree: &'tree Tree,
    piece: Range<usize>,
    deadline: Instant,
) -> Result<(Range<usize>, Nested<'tree>), Piece> {
    let Some(injections) = queries.get(Kind::Injections(Host::Html)) else {
        return Err(Piece::Limit);
    };
    let mut widened = piece.clone();
    let mut nested = Nested::default();
    let outcome = run(
        &injections.query,
        tree,
        text,
        piece,
        deadline,
        |node, _, _| {
            let Some(code) = injectable(text, node) else {
                return;
            };
            if nested.fences.last() == Some(&code) {
                return;
            }
            nested.fences.push(code);
            nested.blocks.push(node.byte_range());
            widened.start = widened.start.min(node.start_byte());
            widened.end = widened.end.max(node.end_byte());
        },
    );
    match outcome {
        Some(outcome) => Err(outcome),
        None => Ok((widened, nested)),
    }
}

/// The text of `node`, a `<script>` or `<style>` element short enough, with the language its
/// start tag gives it, if it holds text and oso parses that language.
fn injectable<'tree>(text: &Rope, node: Node<'tree>) -> Option<(Node<'tree>, Language)> {
    let script = match node.kind() {
        "script_element" => true,
        "style_element" => false,
        _ => return None,
    };
    if node.byte_range().len() > MAX_EMBEDDED_BYTES {
        return None;
    }
    let (mut language, mut content) = (None, None);
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            match child.kind() {
                "start_tag" => language = of_type(script, type_value(text, child)),
                "raw_text" => content = Some(child),
                _ => {}
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    Some((content?, language?))
}

/// What an element's first `type` attribute says, read from `start_tag`: `None` without one,
/// `Some(None)` for a value too long to name anything, and the bytes otherwise, lowercased and
/// trimmed. An attribute with no value is an empty one.
fn type_value(text: &Rope, start_tag: Node) -> Option<Option<Word>> {
    let mut cursor = start_tag.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        let attribute = cursor.node();
        if attribute.kind() == "attribute" {
            let (mut name, mut value) = (None, None);
            let mut inner = attribute.walk();
            if inner.goto_first_child() {
                loop {
                    let part = inner.node();
                    match part.kind() {
                        "attribute_name" => name = Some(part),
                        "attribute_value" => value = Some(Some(part)),
                        // Its value is its only named child, between the quotes.
                        "quoted_attribute_value" => value = Some(part.named_child(0)),
                        _ => {}
                    }
                    if !inner.goto_next_sibling() {
                        break;
                    }
                }
            }
            let is_type = name
                .and_then(|name| word(text, name.byte_range()))
                .is_some_and(|name| name.as_bytes() == b"type");
            if is_type {
                // `type=""` has a quoted value with no value inside it.
                return Some(match value {
                    None | Some(None) => Some(Word::default()),
                    Some(Some(value)) => word(text, value.byte_range()),
                });
            }
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

/// The `type` values of a `<script>` of JavaScript, JSX among it.
const JAVASCRIPT_TYPES: [&[u8]; 8] = [
    b"",
    b"module",
    b"text/javascript",
    b"application/javascript",
    b"text/ecmascript",
    b"application/ecmascript",
    b"text/jsx",
    b"text/babel",
];

/// The `type` values of a `<script>` of JSON.
const JSON_TYPES: [&[u8]; 5] = [
    b"importmap",
    b"speculationrules",
    b"application/json",
    b"application/ld+json",
    b"text/json",
];

/// The `type` values of a `<style>` of CSS.
const CSS_TYPES: [&[u8]; 2] = [b"", b"text/css"];

/// The language of a `<script>` (`script`) or `<style>` with the `type` value `value` (as
/// [`type_value`] gives it), if oso parses it.
fn of_type(script: bool, value: Option<Option<Word>>) -> Option<Language> {
    let value = match value {
        // No `type`: an empty one.
        None => Word::default(),
        Some(None) => return None,
        Some(Some(value)) => value,
    };
    let value = value.as_bytes();
    if !script {
        CSS_TYPES.contains(&value).then_some(Language::Css)
    } else if JAVASCRIPT_TYPES.contains(&value) {
        Some(Language::JavaScript)
    } else {
        JSON_TYPES.contains(&value).then_some(Language::Json)
    }
}

/// A short word read from the text, lowercased.
#[derive(Default)]
struct Word {
    bytes: [u8; MAX_WORD_BYTES],
    len: usize,
}

impl Word {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// The text of `range`, ASCII white space trimmed and ASCII-lowercased, if it is at most
/// [`MAX_WORD_BYTES`] long once trimmed.
fn word(text: &Rope, range: Range<usize>) -> Option<Word> {
    // Read at most a little more than a word, then trim: a value padded with much white space
    // names nothing, as no real one is.
    let end = range
        .end
        .min(range.start.saturating_add(2 * MAX_WORD_BYTES));
    if end < range.end {
        return None;
    }
    let mut read = [0; 2 * MAX_WORD_BYTES];
    let mut len = 0;
    for chunk in text.chunks_in_range(range.start..end) {
        let slot = read.get_mut(len..len + chunk.len())?;
        slot.copy_from_slice(chunk.as_bytes());
        len += chunk.len();
    }
    let trimmed = read.get(..len)?.trim_ascii();
    let mut word = Word::default();
    let slot = word.bytes.get_mut(..trimmed.len())?;
    slot.copy_from_slice(trimmed);
    slot.make_ascii_lowercase();
    word.len = trimmed.len();
    Some(word)
}
