//! What one line of a Claude Code transcript says that `session-search` reads (ADR 0145): a
//! prompt's or a reply's text, the directory it ran in, or a title. Every other value is
//! skipped by its brackets and quotes, never decoded, so a tool result costs one pass over its
//! bytes.
//!
//! Must not depend on: files, threads or the clock; plain functions over a line's bytes.
//!
//! - A prompt is a `user` line's text, a string or its `text` parts, that is not `isMeta`, a
//!   compact summary or of a side chain, and does not start as a command's or a hook's
//!   wrapper. A reply is an `assistant` line's `text` parts, not of a side chain and not an API
//!   error. Tool inputs and results, thinking and images are never decoded.
//! - A title is taken only from a line whose bytes start as a title line does, by
//!   `transcripts`' own [`TITLES`] and bounds: a message's text can never start a line.
//! - A directory is the line's top-level `cwd`, when absolute with no `.` or `..` in it.

use std::ops::Range;
use std::path::PathBuf;

use crate::sessions::registry::is_plain_directory;
pub(super) use crate::sessions::transcripts::TITLES;
use crate::sessions::transcripts::{MAX_CWD, MAX_TITLE};

/// The deepest a value nests before the line is left unread.
const MAX_DEPTH: usize = 128;

/// How a prompt that a command or a hook wrapped starts: never a person's words. A deny list:
/// a wrapper Claude Code adds later is searched and shown until it is listed here, which
/// shows on the screen only, never to an agent.
const WRAPPERS: [&str; 6] = [
    "<command-",
    "<local-command",
    "<bash-",
    "<task-notification",
    "<system-reminder",
    "[Request interrupted",
];

/// What a line says.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Line {
    /// The directory it ran in, if the line names one.
    pub(super) cwd: Option<PathBuf>,
    /// A message's texts, and whether it is a prompt rather than a reply.
    pub(super) message: Option<(bool, Vec<String>)>,
    /// A title, and its rank in [`TITLES`].
    pub(super) title: Option<(usize, String)>,
}

/// What `line`, one JSON object, says.
pub(super) fn read(line: &[u8]) -> Line {
    let mut read = Line::default();
    if let Some(rank) = TITLES
        .iter()
        .position(|(start, _)| line.starts_with(start.as_bytes()))
    {
        let [name] = fields(line, [TITLES[rank].1]);
        read.title = name
            .and_then(|value| string(&line[value]))
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty() && name.len() <= MAX_TITLE)
            .map(|name| (rank, name));
        return read;
    }
    let [kind, cwd, meta, side, compact, error, message] = fields(
        line,
        [
            "type",
            "cwd",
            "isMeta",
            "isSidechain",
            "isCompactSummary",
            "isApiErrorMessage",
            "message",
        ],
    );
    let value = |range: Option<Range<usize>>| range.map(|range| &line[range]);
    let prompt = match value(kind) {
        Some(b"\"user\"") => true,
        Some(b"\"assistant\"") => false,
        _ => return read,
    };
    read.cwd = value(cwd)
        .filter(|cwd| cwd.len() <= MAX_CWD + 2)
        .and_then(string)
        .map(PathBuf::from)
        .filter(|cwd| is_plain_directory(cwd));
    let yes = |range| value(range) == Some(b"true".as_slice());
    if yes(side) || (prompt && (yes(meta) || yes(compact))) || (!prompt && yes(error)) {
        return read;
    }
    let Some(message) = value(message) else {
        return read;
    };
    let [content] = fields(message, ["content"]);
    let Some(content) = content.map(|range| &message[range]) else {
        return read;
    };
    let mut texts = Vec::new();
    match content.first() {
        Some(b'"') if prompt => texts.extend(string(content)),
        Some(b'[') => {
            for element in elements(content) {
                let element = &content[element];
                let [kind, text] = fields(element, ["type", "text"]);
                if kind.map(|range| &element[range]) == Some(b"\"text\"".as_slice())
                    && let Some(text) = text.and_then(|range| string(&element[range]))
                {
                    texts.push(text);
                }
            }
        }
        _ => {}
    }
    if prompt {
        texts.retain(|text| {
            let start = text.trim_start();
            !WRAPPERS.iter().any(|wrapper| start.starts_with(wrapper))
        });
    }
    texts.retain(|text| !text.trim().is_empty());
    if !texts.is_empty() {
        read.message = Some((prompt, texts));
    }
    read
}

/// The string a JSON string's bytes, quotes included, hold.
fn string(value: &[u8]) -> Option<String> {
    if value.first() != Some(&b'"') {
        return None;
    }
    serde_json::from_slice(value).ok()
}

/// Where the values of the keys `wanted` are in the JSON object `object`, at its top level
/// only, each as a range of its bytes; a key named twice gives its last. Other values are
/// skipped, never decoded. An object that is not whole, closed with nothing but blanks after
/// it, or nested past [`MAX_DEPTH`], gives none.
fn fields<const N: usize>(object: &[u8], wanted: [&str; N]) -> [Option<Range<usize>>; N] {
    let none = [const { None }; N];
    if object.first() != Some(&b'{') {
        return none;
    }
    let mut found = [const { None }; N];
    let mut at = skip_blanks(object, 1);
    if object.get(at) == Some(&b'}') {
        return none;
    }
    loop {
        if object.get(at) != Some(&b'"') {
            return none;
        }
        let Some(key_end) = string_end(object, at) else {
            return none;
        };
        let key = &object[at + 1..key_end];
        at = skip_blanks(object, key_end + 1);
        if object.get(at) != Some(&b':') {
            return none;
        }
        let start = skip_blanks(object, at + 1);
        let Some(end) = value_end(object, start) else {
            return none;
        };
        if let Some(index) = wanted.iter().position(|name| name.as_bytes() == key) {
            found[index] = Some(start..end);
        }
        at = skip_blanks(object, end);
        match object.get(at) {
            Some(b',') => at = skip_blanks(object, at + 1),
            Some(b'}') if skip_blanks(object, at + 1) == object.len() => return found,
            _ => return none,
        }
    }
}

/// The ranges of the elements of the JSON array `array`, in order. Past one cut short, none.
fn elements(array: &[u8]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if array.first() != Some(&b'[') {
        return found;
    }
    let mut at = skip_blanks(array, 1);
    if array.get(at) == Some(&b']') {
        return found;
    }
    loop {
        let start = skip_blanks(array, at);
        let Some(end) = value_end(array, start) else {
            return found;
        };
        found.push(start..end);
        at = skip_blanks(array, end);
        match array.get(at) {
            Some(b',') => at += 1,
            _ => return found,
        }
    }
}

/// The index just past the JSON value that starts at `start`, skipped by its brackets and
/// quotes.
fn value_end(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => string_end(bytes, start).map(|end| end + 1),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut at = start;
            while at < bytes.len() {
                match bytes[at] {
                    b'"' => at = string_end(bytes, at)?,
                    b'{' | b'[' => {
                        depth += 1;
                        if depth > MAX_DEPTH {
                            return None;
                        }
                    }
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at + 1);
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
            None
        }
        _ => {
            let mut at = start;
            while at < bytes.len() && !matches!(bytes[at], b',' | b'}' | b']') {
                at += 1;
            }
            // Trailing blanks are not part of a number, `true` or `null`.
            while at > start && bytes[at - 1].is_ascii_whitespace() {
                at -= 1;
            }
            (at > start).then_some(at)
        }
    }
}

/// The index of the quote that ends the string starting with the quote at `start`.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return Some(at),
            _ => at += 1,
        }
    }
    None
}

fn skip_blanks(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(texts: &[&str]) -> Option<(bool, Vec<String>)> {
        Some((true, texts.iter().map(|text| (*text).to_owned()).collect()))
    }

    #[test]
    fn a_prompt_as_a_string_is_read_with_its_directory() {
        let line = br#"{"parentUuid":null,"isSidechain":false,"cwd":"/w/oso","type":"user","message":{"role":"user","content":"find the \"trust\" bug\nplease"}}"#;
        let read = read(line);
        assert_eq!(read.message, prompt(&["find the \"trust\" bug\nplease"]));
        assert_eq!(read.cwd, Some(PathBuf::from("/w/oso")));
    }

    #[test]
    fn a_reply_keeps_its_text_parts_and_skips_tool_uses_and_thinking() {
        let line = br#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"secret plan"},{"type":"text","text":"Here it is"},{"type":"tool_use","name":"Bash","input":{"command":"cat .env"}},{"type":"text","text":"Done"}]}}"#;
        assert_eq!(
            read(line).message,
            Some((false, vec!["Here it is".to_owned(), "Done".to_owned()]))
        );
    }

    #[test]
    fn tool_results_meta_side_chains_and_wrappers_are_not_prompts() {
        let results = br#"{"type":"user","message":{"content":[{"type":"tool_result","content":"API_KEY=abc"}]}}"#;
        assert_eq!(read(results).message, None);
        let meta =
            br#"{"type":"user","isMeta":true,"message":{"content":"Caveat: the messages below"}}"#;
        assert_eq!(read(meta).message, None);
        let side = br#"{"isSidechain":true,"type":"assistant","message":{"content":[{"type":"text","text":"sub"}]}}"#;
        assert_eq!(read(side).message, None);
        let compact = br#"{"type":"user","isCompactSummary":true,"message":{"content":"This session is being continued"}}"#;
        assert_eq!(read(compact).message, None);
        let wrapped =
            br#"{"type":"user","message":{"content":"  <command-name>/clear</command-name>"}}"#;
        assert_eq!(read(wrapped).message, None);
        let error = br#"{"type":"assistant","isApiErrorMessage":true,"message":{"content":[{"type":"text","text":"Overloaded"}]}}"#;
        assert_eq!(read(error).message, None);
    }

    #[test]
    fn other_records_say_nothing_but_a_title_line_names_it() {
        assert_eq!(
            read(br#"{"type":"attachment","attachment":{"content":"memory"}}"#),
            Line::default()
        );
        let title =
            read(br#"{"type":"custom-title","customTitle":"  Fix login  ","sessionId":"x"}"#);
        assert_eq!(title.title, Some((0, "Fix login".to_owned())));
        let ai = read(br#"{"type":"ai-title","aiTitle":"Search history"}"#);
        assert_eq!(ai.title, Some((2, "Search history".to_owned())));
    }

    #[test]
    fn a_directory_with_dots_or_relative_is_not_taken() {
        let dots = br#"{"type":"user","cwd":"/w/../etc","message":{"content":"hi"}}"#;
        assert_eq!(read(dots).cwd, None);
        let relative = br#"{"type":"user","cwd":"w/oso","message":{"content":"hi"}}"#;
        assert_eq!(read(relative).cwd, None);
    }

    #[test]
    fn a_line_cut_short_or_nested_deep_reads_what_it_can() {
        assert_eq!(
            read(br#"{"type":"user","message":{"content":"half"#).message,
            None
        );
        let deep = format!(
            r#"{{"type":"user","x":{}{},"message":{{"content":"hi"}}}}"#,
            "[".repeat(MAX_DEPTH + 1),
            "]".repeat(MAX_DEPTH + 1)
        );
        assert_eq!(read(deep.as_bytes()).message, None);
        let fine = br#"{"type":"user","x":[[1, 2], {"a": "b]"}],"message":{"content":"hi"}}"#;
        assert_eq!(read(fine).message, prompt(&["hi"]));
    }
}

/// The scanner's fuzz target (ADR 0145): lines made from the parts of a transcript's records,
/// cut and joined at random, and arbitrary bytes. Whatever they are, reading never panics, and
/// what it reads is what serde reads: a prompt or reply only from a `user` or `assistant`
/// record, and its texts as JSON decodes them.
///
/// It runs in `just ci` with proptest's default cases. For a long run:
/// `PROPTEST_CASES=200000 cargo test -p oso --release sessions::history::scan`.
#[cfg(test)]
mod fuzz {
    use proptest::prelude::*;

    use super::*;

    /// A part of a record's JSON.
    fn part() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(r#"{"type":"user","#.to_owned()),
            Just(r#"{"type":"assistant","#.to_owned()),
            Just(r#""message":{"content":"#.to_owned()),
            Just(r#""cwd":"/w/oso","#.to_owned()),
            Just(r#""isMeta":true,"#.to_owned()),
            Just(r#"[{"type":"text","text":"#.to_owned()),
            Just(r#"{"type":"tool_result","content":"#.to_owned()),
            Just(r#""a \" b \\ é 😀""#.to_owned()),
            Just("}".to_owned()),
            Just("]".to_owned()),
            Just(",".to_owned()),
            Just(" ".to_owned()),
            "[a-z\"\\\\{}\\[\\],:]{0,8}",
        ]
    }

    /// A text a message may hold: words, escapes, wide characters and wrappers' starts.
    fn text() -> impl Strategy<Value = String> {
        prop_oneof![
            r#"[a-z \n\t"\\]{0,16}"#,
            Just("  <command-name>/clear</command-name>".to_owned()),
            Just("<system-reminder>x</system-reminder>".to_owned()),
            Just("日本語 😀 \u{202e}".to_owned()),
            Just("   ".to_owned()),
        ]
    }

    /// A part of a reply's or a prompt's content: text, or what is never searched.
    fn block() -> impl Strategy<Value = serde_json::Value> {
        let kinds = prop_oneof![
            Just("text"),
            Just("tool_use"),
            Just("tool_result"),
            Just("thinking")
        ];
        (kinds, text()).prop_map(|(kind, text)| block_of(kind, &text))
    }

    fn block_of(kind: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"type": kind, "text": text, "input": {"a": [text]}})
    }

    /// A whole record, as the CLI writes one, of any kind, flags and content.
    fn record() -> impl Strategy<Value = Vec<u8>> {
        let kinds = prop_oneof![Just("user"), Just("assistant"), Just("attachment")];
        let content = prop_oneof![
            text().prop_map(serde_json::Value::String),
            prop::collection::vec(block(), 0..4).prop_map(serde_json::Value::Array),
        ];
        (kinds, any::<[bool; 4]>(), content)
            .prop_map(|(kind, flags, content)| record_of(kind, flags, content))
    }

    fn record_of(
        kind: &str,
        [meta, side, compact, error]: [bool; 4],
        content: serde_json::Value,
    ) -> Vec<u8> {
        let record = serde_json::json!({
            "parentUuid": null,
            "isSidechain": side,
            "isMeta": meta,
            "isCompactSummary": compact,
            "isApiErrorMessage": error,
            "cwd": "/w/oso",
            "type": kind,
            "message": {"role": kind, "content": content},
        });
        record.to_string().into_bytes()
    }

    fn line() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            record(),
            prop::collection::vec(part(), 0..24).prop_map(|parts| parts.concat().into_bytes()),
            prop::collection::vec(any::<u8>(), 0..256),
        ]
    }

    /// What `value`, a line serde read, says of a message by the rules of [`read`].
    fn by_serde(value: &serde_json::Value) -> Option<(bool, Vec<String>)> {
        let yes = |key: &str| value[key].as_bool() == Some(true);
        let prompt = match value.get("type")?.as_str()? {
            "user" => true,
            "assistant" => false,
            _ => return None,
        };
        if yes("isSidechain")
            || (prompt && (yes("isMeta") || yes("isCompactSummary")))
            || (!prompt && yes("isApiErrorMessage"))
        {
            return None;
        }
        let content = value.get("message")?.as_object()?.get("content")?;
        let mut texts: Vec<String> = match content {
            serde_json::Value::String(text) if prompt => vec![text.clone()],
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter(|part| part.get("type").and_then(|kind| kind.as_str()) == Some("text"))
                .filter_map(|part| part.get("text")?.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        if prompt {
            texts.retain(|text| {
                let start = text.trim_start();
                !WRAPPERS.iter().any(|wrapper| start.starts_with(wrapper))
            });
        }
        texts.retain(|text| !text.trim().is_empty());
        (!texts.is_empty()).then_some((prompt, texts))
    }

    proptest! {
        #[test]
        fn a_line_is_read_without_panicking_and_as_serde_reads_it(line in line()) {
            let read = read(&line);
            match serde_json::from_slice::<serde_json::Value>(&line) {
                // A title line is read by its start, as `transcripts` reads one.
                Ok(value) if value.is_object() && read.title.is_none() => {
                    prop_assert_eq!(read.message, by_serde(&value));
                }
                _ => {}
            }
            if let Some(cwd) = &read.cwd {
                prop_assert!(cwd.is_absolute());
            }
        }
    }
}
