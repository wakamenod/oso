//! HTML's embedded code: which `<script>` and `<style>` elements hold text JavaScript's, JSON's
//! or CSS's grammar parses, as their start tag says (ADR 0157); [`super::nested`] parses and
//! paints it, as it does Markdown's fenced code.
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and HTML's tree, and runs only
//! on the worker.
//!
//! - A `<script>` is JavaScript without a `type`, or with an empty one, `module` or a JavaScript
//!   MIME type; JSON with `importmap`, `speculationrules` or a JSON MIME type; and stays plain
//!   with any other (a template's, say). A `<style>` is CSS without a `type`, or with an empty
//!   one or `text/css`. The value is read with ASCII white space trimmed and ignoring ASCII case,
//!   and only the first `type` counts, as in a browser.
//! - The text of an element longer than [`MAX_EMBEDDED_BYTES`] stays plain, as a fence's does,
//!   so that the piece that holds it stays small: a piece widens to the whole of each element
//!   it meets.
//! - Children are walked with a `TreeCursor`, never by recursion, one for an element and one
//!   for its start tag's attributes, and an attribute's name is read only when it is as long as
//!   `type`.

use oso_core::rope::Rope;
use tree_sitter::{Node, TreeCursor};

use super::nested::{Found, short_text};
use crate::language::Language;

/// The longest element whose text is highlighted with its language, in bytes, the tags too:
/// a fence's limit ([`super::fence::MAX_FENCE_BYTES`]).
pub(super) const MAX_EMBEDDED_BYTES: usize = super::fence::MAX_FENCE_BYTES;

/// The longest `type` value read, white space around it too: a longer one names nothing here.
const MAX_TYPE_BYTES: usize = 64;

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

/// An element whose text may be code.
#[derive(Clone, Copy)]
enum Element {
    Script,
    Style,
}

/// What an element's start tag says of its `type`.
enum Type<'value> {
    /// No `type` attribute.
    Absent,
    /// A value too long to name anything.
    Unread,
    /// The value, trimmed and lowercased.
    Value(&'value [u8]),
}

/// What another grammar parses of `node`, which HTML's injection query found: the text of a
/// `<script>` or `<style>` element short enough, in the language its start tag gives it, or
/// nothing.
pub(super) fn found<'tree>(text: &Rope, node: Node<'tree>) -> Found<'tree> {
    match injectable(text, node) {
        Some((content, language)) => Found::Code(content, language),
        None => Found::Skip,
    }
}

/// The text of `node`, a `<script>` or `<style>` element short enough, with the language its
/// start tag gives it, if it holds text and oso parses that language.
fn injectable<'tree>(text: &Rope, node: Node<'tree>) -> Option<(Node<'tree>, Language)> {
    let element = match node.kind() {
        "script_element" => Element::Script,
        "style_element" => Element::Style,
        _ => return None,
    };
    if node.byte_range().len() > MAX_EMBEDDED_BYTES {
        return None;
    }
    let (mut language, mut content) = (None, None);
    let mut buffer = [0; MAX_TYPE_BYTES];
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            match child.kind() {
                "start_tag" => language = of_type(element, type_value(text, child, &mut buffer)),
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

/// What the first `type` attribute of `start_tag` says, its value read into `buffer`. An
/// attribute with no value is an empty one. One cursor walks all the attributes.
fn type_value<'buffer>(
    text: &Rope,
    start_tag: Node,
    buffer: &'buffer mut [u8; MAX_TYPE_BYTES],
) -> Type<'buffer> {
    let mut cursor = start_tag.walk();
    let cursor = &mut cursor;
    if !cursor.goto_first_child() {
        return Type::Absent;
    }
    loop {
        if cursor.node().kind() == "attribute" && cursor.goto_first_child() {
            if is_type(text, cursor.node()) {
                return value(text, cursor, buffer);
            }
            cursor.goto_parent();
        }
        if !cursor.goto_next_sibling() {
            return Type::Absent;
        }
    }
}

/// Whether `name`, an attribute's first child, is the name `type`, in any ASCII case. A name
/// holds no white space, so one of another length is read no further.
fn is_type(text: &Rope, name: Node) -> bool {
    let mut read = [0; 4];
    name.kind() == "attribute_name"
        && name.byte_range().len() == read.len()
        && short_text(text, name.byte_range(), &mut read)
            .is_some_and(|name| name.eq_ignore_ascii_case(b"type"))
}

/// The value of the attribute whose name `cursor` is at, read into `buffer`.
fn value<'buffer>(
    text: &Rope,
    cursor: &mut TreeCursor,
    buffer: &'buffer mut [u8; MAX_TYPE_BYTES],
) -> Type<'buffer> {
    let mut range = None;
    while cursor.goto_next_sibling() {
        let part = cursor.node();
        match part.kind() {
            "attribute_value" => range = Some(part.byte_range()),
            // Its value is its only named child, between the quotes: `type=""` has none.
            "quoted_attribute_value" => {
                range = Some(part.named_child(0).map_or(0..0, |value| value.byte_range()));
            }
            _ => {}
        }
    }
    let Some(range) = range else {
        return Type::Value(&[]);
    };
    let Some(read) = short_text(text, range, buffer) else {
        return Type::Unread;
    };
    let start = read
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(read.len());
    let end = read
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |last| last + 1);
    match read.get_mut(start..end) {
        Some(trimmed) => {
            trimmed.make_ascii_lowercase();
            Type::Value(trimmed)
        }
        None => Type::Unread,
    }
}

/// The language of `element` whose start tag says `value` of its `type`, if oso parses it.
fn of_type(element: Element, value: Type) -> Option<Language> {
    let value = match value {
        Type::Absent => &[][..],
        Type::Unread => return None,
        Type::Value(value) => value,
    };
    match element {
        Element::Style => CSS_TYPES.contains(&value).then_some(Language::Css),
        Element::Script if JAVASCRIPT_TYPES.contains(&value) => Some(Language::JavaScript),
        Element::Script => JSON_TYPES.contains(&value).then_some(Language::Json),
    }
}
