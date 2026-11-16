//! What `textDocument/completion` answers, read within the bounds (ADR 0138, Completion in the
//! buffer, step 9): a `CompletionList` or a list of `CompletionItem`s, at most
//! [`COMPLETION_ITEMS_MAX`] kept, in the server's order by `sortText`, then as they came.
//!
//! Must not depend on: `server`, threads or processes; this reads values, and the session
//! thread places an item's range against the text the server has.
//!
//! - A label, a `filterText` and a detail are cut to their bounds; an item whose text to insert
//!   is past [`INSERT_TEXT_BYTES_MAX`] is dropped, as a cut one would insert the wrong text.
//! - oso claims no snippets (Q22); a snippet that still comes keeps its placeholders' text and
//!   loses its markers.
//! - A `textEdit` is read by its range, or an `InsertReplaceEdit` by its insert range, as the
//!   person's text after the cursor is kept; the session thread keeps a range only on the
//!   asked position's line and around it ([`Offered::range_around`]).
//! - An item whose text to insert holds a control character other than a tab or a line feed,
//!   or a carriage return, is dropped: what it inserts shows nowhere before it goes in.

use serde_json::{Value, json};

use crate::json::{array, integer_in, string};
use crate::limits::{
    ADDITIONAL_EDITS_BYTES_MAX, ADDITIONAL_EDITS_MAX, COMPLETION_ITEMS_MAX, DETAIL_BYTES_MAX,
    INSERT_TEXT_BYTES_MAX, LABEL_BYTES_MAX, SORT_TEXT_BYTES_MAX,
};
use crate::position::Position;

use super::published::range_of;

/// A completion as the server gave it, within the bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    pub label: String,
    /// Its `CompletionItemKind`, 1 to 25.
    pub kind: Option<u8>,
    pub detail: String,
    /// What the typed word is matched against: its `filterText`, else none, for the label.
    pub filter: Option<String>,
    /// Its `textEdit`'s range, as the server counts it, if it has one.
    pub range: Option<(Position, Position)>,
    /// The text it inserts: its edit's, else its `insertText`, else its label.
    pub text: String,
    /// Its `additionalTextEdits`, as the server counts their ranges (step 9b).
    pub extra: Vec<Extra>,
}

/// An edit a completion makes besides its own, such as rust-analyzer's added `use` line: its
/// range as the server counts it, and its text.
pub type Extra = (Position, Position, String);

/// What a server says of its completions in `initialize`'s result (step 9b): the characters
/// that ask for them at once, each one character, and whether it resolves an item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completing {
    pub triggers: Vec<char>,
    pub resolves: bool,
}

/// The most trigger characters kept.
const TRIGGERS_MAX: usize = 16;

/// What `initialize`'s result says of completion, if the server completes at all.
pub fn completing(result: &Value) -> Option<Completing> {
    let provider = result.get("capabilities")?.get("completionProvider")?;
    if !provider.is_object() {
        return None;
    }
    let triggers = (provider.get("triggerCharacters"))
        .and_then(|triggers| array(triggers, TRIGGERS_MAX))
        .unwrap_or_default()
        .iter()
        .filter_map(|trigger| {
            let mut chars = trigger.as_str()?.chars();
            let first = chars.next()?;
            // One printable character, which typing can give.
            (chars.next().is_none() && !first.is_control() && !first.is_whitespace())
                .then_some(first)
        })
        .collect();
    Some(Completing {
        triggers,
        resolves: provider.get("resolveProvider") == Some(&Value::Bool(true)),
    })
}

/// The edits `value`, an `additionalTextEdits` list, makes: at most
/// [`ADDITIONAL_EDITS_MAX`], with at most [`ADDITIONAL_EDITS_BYTES_MAX`] of text in all. A list
/// past either, or one whose text holds a control character other than a tab or a line feed,
/// gives none: a part of it could leave the text broken.
pub fn extra_edits(value: Option<&Value>) -> Vec<Extra> {
    let Some(given) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    if given.len() > ADDITIONAL_EDITS_MAX {
        return Vec::new();
    }
    let mut bytes = 0usize;
    let mut edits = Vec::with_capacity(given.len());
    for edit in given {
        let Some(((start, end), text)) = edit
            .get("range")
            .and_then(range_of)
            .zip(edit.get("newText").and_then(Value::as_str))
        else {
            return Vec::new();
        };
        bytes += text.len();
        if bytes > ADDITIONAL_EDITS_BYTES_MAX || has_controls(text) {
            return Vec::new();
        }
        edits.push((start, end, text.to_owned()));
    }
    edits
}

/// Whether `text` holds a control character other than a tab or a line feed.
fn has_controls(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_control() && c != '\t' && c != '\n')
}

/// The edits `result`, a resolved `CompletionItem`, makes besides its own.
pub fn resolved(result: &Value) -> Vec<Extra> {
    extra_edits(result.get("additionalTextEdits"))
}

impl Offered {
    /// Its range, if it is on the line of `at`, the position asked about, and around it, as the
    /// protocol says an edit's range must be: a range elsewhere is the server's mistake, and
    /// following it would edit text the person never sees.
    pub fn range_around(&self, at: Position) -> Option<(Position, Position)> {
        let (start, end) = self.range?;
        let around = start.line == at.line
            && end.line == at.line
            && start.character <= at.character
            && at.character <= end.character;
        around.then_some((start, end))
    }
}

/// What an answer offers, whether it says the list is incomplete, so that typing asks again,
/// and how many it left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionList {
    pub items: Vec<Offered>,
    /// Each item as the server gave it, for `completionItem/resolve` (step 9b).
    pub given: Vec<Value>,
    pub incomplete: bool,
    pub left: usize,
}

/// The items `result` offers. `null` offers none.
pub fn completed(result: &Value) -> CompletionList {
    let (items, incomplete) = match result {
        Value::Array(_) => (result, false),
        Value::Object(list) => (
            list.get("items").unwrap_or(&Value::Null),
            list.get("isIncomplete") == Some(&Value::Bool(true)),
        ),
        _ => return CompletionList::default(),
    };
    let Some(given) = items.as_array() else {
        return CompletionList {
            incomplete,
            ..CompletionList::default()
        };
    };
    let kept = array(items, COMPLETION_ITEMS_MAX).unwrap_or_default();
    let mut read: Vec<(Option<String>, Offered, &Value)> = (kept.iter())
        .filter_map(|value| one(value).map(|(sort, item)| (sort, item, value)))
        .collect();
    let left = given.len() - read.len();
    // The server's order: by `sortText`, else the label, then as they came.
    read.sort_by(|(a, first, _), (b, second, _)| {
        let a = a.as_deref().unwrap_or(&first.label);
        let b = b.as_deref().unwrap_or(&second.label);
        a.cmp(b)
    });
    let given = read.iter().map(|(_, _, value)| (*value).clone()).collect();
    CompletionList {
        items: read.into_iter().map(|(_, item, _)| item).collect(),
        given,
        incomplete,
        left,
    }
}

/// The string `value` holds, if it is one.
fn text_of(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

/// An item, with its `sortText`.
fn one(value: &Value) -> Option<(Option<String>, Offered)> {
    let label = string(value.get("label")?, LABEL_BYTES_MAX)?.to_owned();
    let kind = value
        .get("kind")
        .and_then(|kind| integer_in(kind, 1..=25))
        .and_then(|kind| u8::try_from(kind).ok());
    let detail = text_of(value.get("detail"))
        .map(|detail| detail[..detail.floor_char_boundary(DETAIL_BYTES_MAX)].to_owned())
        .unwrap_or_default();
    let filter = value
        .get("filterText")
        .and_then(|filter| string(filter, LABEL_BYTES_MAX))
        .map(str::to_owned);
    let sort = value
        .get("sortText")
        .and_then(|sort| string(sort, SORT_TEXT_BYTES_MAX))
        .map(str::to_owned);
    let snippet = value.get("insertTextFormat").and_then(Value::as_i64) == Some(2);
    let edit = value.get("textEdit");
    let range = edit.and_then(|edit| range_of(edit.get("range").or_else(|| edit.get("insert"))?));
    let text = match edit {
        // An edit with no range of either kind gives nothing to insert by.
        Some(edit) => text_of(edit.get("newText")).filter(|_| range.is_some())?,
        None => text_of(value.get("insertText")).unwrap_or(&label),
    };
    if text.len() > INSERT_TEXT_BYTES_MAX || has_controls(text) {
        return None;
    }
    let text = if snippet {
        plain(text)
    } else {
        text.to_owned()
    };
    Some((
        sort,
        Offered {
            label,
            kind,
            detail,
            filter,
            range,
            text,
            extra: extra_edits(value.get("additionalTextEdits")),
        },
    ))
}

/// `snippet` as plain text: `$1` and `$0` dropped, `${1:name}` as `name`, `${1|a,b|}` as `a`,
/// `\$`, `\}` and `\\` as the characters they escape.
pub fn plain(snippet: &str) -> String {
    let mut out = String::with_capacity(snippet.len());
    let mut chars = snippet.chars().peekable();
    // How many `${` are open, whose `}` ends a placeholder.
    let mut open = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(&next) = chars.peek()
                    && matches!(next, '$' | '}' | '\\' | ',' | '|')
                {
                    out.push(next);
                    chars.next();
                } else {
                    out.push('\\');
                }
            }
            '$' => match chars.peek() {
                Some(digit) if digit.is_ascii_digit() => {
                    while chars.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                    }
                }
                Some('{') => {
                    chars.next();
                    while chars.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                    }
                    match chars.peek() {
                        Some(':') => {
                            chars.next();
                            open += 1;
                        }
                        Some('|') => {
                            chars.next();
                            // The first choice, then the rest skipped to `|}`.
                            let mut first = true;
                            while let Some(choice) = chars.next() {
                                match choice {
                                    '|' if chars.peek() == Some(&'}') => {
                                        chars.next();
                                        break;
                                    }
                                    ',' => first = false,
                                    _ if first => out.push(choice),
                                    _ => {}
                                }
                            }
                        }
                        Some('}') => {
                            chars.next();
                        }
                        // A variable, `${name}` or `${name:default}`: its default only.
                        _ => {
                            while chars
                                .peek()
                                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                            {
                                chars.next();
                            }
                            match chars.next() {
                                Some(':') => open += 1,
                                Some('}') | None => {}
                                Some(other) => out.push(other),
                            }
                        }
                    }
                }
                // A variable, `$name`, has no default.
                Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
                    while chars
                        .peek()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                    {
                        chars.next();
                    }
                }
                _ => out.push('$'),
            },
            '}' if open > 0 => open -= 1,
            other => out.push(other),
        }
    }
    out
}

/// The word an item's kind is shown as, as Corfu's kind margin names it.
pub fn kind_name(kind: u8) -> &'static str {
    match kind {
        1 => "text",
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        11 => "unit",
        12 => "value",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        16 => "color",
        17 => "file",
        18 => "reference",
        19 => "folder",
        20 => "enum member",
        21 => "constant",
        22 => "struct",
        23 => "event",
        24 => "operator",
        25 => "type parameter",
        _ => "",
    }
}

/// Why completions are asked for (step 9b): the person asked or typed a word, typed one of the
/// server's trigger characters, or typed on in a list the server said is incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invoked {
    Asked,
    Character(char),
    Incomplete,
}

/// The parameters of `textDocument/completion` at `position` in `uri`, asked as `invoked` says.
pub fn completion_params(uri: &str, position: Position, invoked: Invoked) -> Value {
    let mut params = super::position_params(uri, position);
    params["context"] = match invoked {
        Invoked::Asked => json!({"triggerKind": 1}),
        Invoked::Character(c) => json!({"triggerKind": 2, "triggerCharacter": c.to_string()}),
        Invoked::Incomplete => json!({"triggerKind": 3}),
    };
    params
}

#[cfg(test)]
mod tests;
