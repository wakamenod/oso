//! Native sessions' fuzz target (ADR 0165, "Code"): lines made at random, from the recorded
//! stream's lines cut, spliced and reordered, from shapes that reach every branch of `wire`,
//! and from arbitrary bytes. Whatever they are, reading never panics, the rows' changes name
//! rows that exist, a buffer kept by the changes alone holds what rendering every row gives,
//! and each row renders to text whose faces lie on its characters. A transcript's lines, read
//! for a session's earlier turns (step 5), are folded the same way.
//!
//! Must not depend on: `app`, threads or a process; these tests drive the pure part only.
//!
//! It runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=200000 cargo test -p oso --release sessions::native::fuzz_tests`.

use proptest::prelude::*;

use std::path::{Path, PathBuf};

use super::past::Index;
use super::placed::Placed;
use super::render::{render, tail};
use super::rows::{Changes, RowChange, Rows};
use super::turns::target;
use super::wire::{parse, transcript};

const RECORDED: &str = include_str!("fixtures/2.1.330/tools.jsonl");

fn recorded() -> Vec<&'static str> {
    RECORDED.lines().collect()
}

/// A line: a recorded one, one cut short, a shape of each kind with odd values, or bytes.
fn line() -> impl Strategy<Value = Vec<u8>> {
    let lines = recorded();
    let count = lines.len();
    prop_oneof![
        4 => (0..count).prop_map(move |at| lines[at].as_bytes().to_vec()),
        2 => (0..count, any::<prop::sample::Index>()).prop_map(|(at, cut)| {
            let line = recorded()[at].as_bytes();
            line[..cut.index(line.len() + 1)].to_vec()
        }),
        3 => shaped().prop_map(String::into_bytes),
        1 => prop::collection::vec(any::<u8>(), 0..200),
    ]
}

/// A line of a known type whose fields have the wrong types, odd ids or edge values.
fn shaped() -> impl Strategy<Value = String> {
    let id = prop_oneof![Just("t".to_owned()), Just(String::new()), "[a-z]{1,3}"];
    let value = prop_oneof![
        Just("null".to_owned()),
        Just("1".to_owned()),
        Just("-1".to_owned()),
        Just("1e999".to_owned()),
        Just("[]".to_owned()),
        Just("{}".to_owned()),
        Just("\"\\u0000é\\n\\t\\u001b[31m\"".to_owned()),
        "\"[a-z\\n]{0,40}\"",
    ];
    (0..10usize, id.clone(), id, value, 0..3u64).prop_map(|(kind, a, b, v, index)| match kind {
        0 => format!(r#"{{"type":"stream_event","api_message_id":"m","parent_tool_use_id":{v},"event":{{"type":"content_block_start","index":{index},"content_block":{{"type":"tool_use","id":"{a}","name":{v}}}}}}}"#),
        1 => format!(r#"{{"type":"stream_event","api_message_id":"m","event":{{"type":"content_block_start","index":{index},"content_block":{{"type":"text"}}}}}}"#),
        2 => format!(r#"{{"type":"stream_event","api_message_id":"m","event":{{"type":"content_block_delta","index":{index},"delta":{{"type":"text_delta","text":{v}}}}}}}"#),
        3 => format!(r#"{{"type":"assistant","parent_tool_use_id":{v},"message":{{"id":"m","content":[{{"type":"tool_use","id":"{a}","name":"Edit","input":{v}}},{{"type":"text","text":{v}}}]}}}}"#),
        4 => format!(r#"{{"type":"user","isReplay":true,"message":{{"content":{v}}}}}"#),
        5 => format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{a}","is_error":{v},"content":{v}}}]}},"tool_use_result":{{"structuredPatch":[{{"oldStart":{v},"lines":["+a",{v},"-b"]}}]}}}}"#),
        6 => format!(r#"{{"type":"system","subtype":"permission_denied","tool_use_id":"{b}","message":{v}}}"#),
        7 => format!(r#"{{"type":"system","subtype":"task_progress","tool_use_id":"{b}","description":{v}}}"#),
        8 => format!(r#"{{"type":"result","subtype":{v},"is_error":{v},"terminal_reason":{v},"duration_ms":{v},"total_cost_usd":{v},"permission_denials":{v}}}"#),
        _ => format!(r#"{{"type":"system","subtype":"compact_boundary","compact_metadata":{{"pre_tokens":{v},"post_tokens":{v},"trigger":{v}}}}}"#),
    })
}

/// Where the line `lines` lines after `start` starts in `buffer`.
fn line_start(buffer: &str, start: usize, lines: usize) -> usize {
    let mut at = start;
    for _ in 0..lines {
        match buffer[at..].find('\n') {
            Some(found) => at += found + 1,
            None => return buffer.len(),
        }
    }
    at
}

proptest! {
    #[test]
    fn any_bytes_read_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let _ = target(&parse(&bytes));
    }

    #[test]
    fn any_stream_folds_and_renders(
        lines in prop::collection::vec(line(), 0..80),
        batch in 1..6usize,
        toggles in prop::collection::vec(
            (any::<prop::sample::Index>(), prop::option::of(0..4usize)),
            0..8,
        ),
    ) {
        let directory = Path::new("/work/probe");
        let mut rows = Rows::new(PathBuf::from(directory));
        rows.sent("Hi");
        // The buffer as a host keeps it: changed only as the changes of each batch say, where
        // `Placed` says.
        let mut buffer = String::new();
        let mut placed = Placed::default();
        for chunk in lines.chunks(batch) {
            let mut changes = Changes::default();
            for line in chunk {
                let event = parse(line);
                let _ = target(&event);
                for change in rows.apply(event) {
                    prop_assert!(change.row() < rows.rows().len());
                    changes.add(change);
                }
            }
            for change in changes {
                let at = change.row();
                let row = &rows.rows()[at];
                let range = placed.range(change, |start, lines| line_start(&buffer, start, lines));
                prop_assert!(range.is_some(), "{:?} not drawn", change);
                let range = range.unwrap_or_default();
                let text = match change {
                    RowChange::Grew { at, .. } => tail(row, at).text,
                    RowChange::Added(_) | RowChange::Changed(_) => render(row, directory).text,
                };
                buffer.replace_range(range.clone(), &text);
                placed.put(at, range, text.len());
                prop_assert_eq!(placed.end(), buffer.len());
            }
        }
        let whole: String = rows.rows().iter().map(|row| render(row, directory).text).collect();
        prop_assert_eq!(&buffer, &whole);
        let count = rows.rows().len();
        if count > 0 {
            for (toggle, slot) in &toggles {
                rows.toggle(toggle.index(count), *slot);
            }
        }
        for row in rows.rows() {
            let rendered = render(row, directory);
            prop_assert!(rendered.text.ends_with('\n'));
            for (range, _) in &rendered.faces {
                prop_assert!(range.start < range.end && range.end <= rendered.text.len());
                prop_assert!(rendered.text.is_char_boundary(range.start));
                prop_assert!(rendered.text.is_char_boundary(range.end));
            }
            for (_, range) in &rendered.calls {
                prop_assert!(range.end <= rendered.text.len());
            }
        }
    }
}

const TRANSCRIPT: &str = include_str!("fixtures/2.1.330/transcript.jsonl");

/// A transcript's line: a recorded one, one cut short, one of a shape with odd values, or
/// bytes.
fn transcript_line() -> impl Strategy<Value = Vec<u8>> {
    let lines: Vec<&'static str> = TRANSCRIPT.lines().collect();
    let count = lines.len();
    let cut_lines = lines.clone();
    let id = prop_oneof![
        Just("null".to_owned()),
        (0..6u8).prop_map(|n| format!("\"u{n}\""))
    ];
    let value = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        Just("[]".to_owned()),
        Just("\"sdk\"".to_owned()),
        Just("\"peer\"".to_owned()),
        "\"[a-z<>\\n]{0,20}\"",
    ];
    let shaped = (0..4usize, id.clone(), id, value).prop_map(|(kind, uuid, parent, v)| {
        match kind {
            0 => format!(r#"{{"parentUuid":{parent},"promptSource":{v},"turnOrigin":{v},"isMeta":{v},"type":"user","message":{{"content":{v}}},"uuid":{uuid}}}"#),
            1 => format!(r#"{{"parentUuid":{parent},"type":"assistant","message":{{"id":{v},"content":[{{"type":"tool_use","id":{v},"name":"Read","input":{{"file_path":{v}}}}},{{"type":"text","text":{v}}}]}},"uuid":{uuid}}}"#),
            2 => format!(r#"{{"parentUuid":{parent},"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":{v},"content":{v}}}]}},"toolUseResult":{{"structuredPatch":{v}}},"uuid":{uuid}}}"#),
            _ => format!(r#"{{"parentUuid":null,"logicalParentUuid":{parent},"type":"system","subtype":"compact_boundary","compactMetadata":{{"preTokens":{v}}},"uuid":{uuid}}}"#),
        }
    });
    prop_oneof![
        4 => (0..count).prop_map(move |at| lines[at].as_bytes().to_vec()),
        2 => (0..count, any::<prop::sample::Index>()).prop_map(move |(at, cut)| {
            let line = cut_lines[at].as_bytes();
            line[..cut.index(line.len() + 1)].to_vec()
        }),
        3 => shaped.prop_map(String::into_bytes),
        1 => prop::collection::vec(any::<u8>(), 0..200),
    ]
}

proptest! {
    #[test]
    fn any_transcript_folds_and_renders(lines in prop::collection::vec(transcript_line(), 0..60)) {
        let directory = Path::new("/work/probe");
        let mut index = Index::default();
        let mut whole = Vec::new();
        for line in &lines {
            let line: Vec<u8> = line.iter().copied().filter(|byte| *byte != b'\n').collect();
            index.take(whole.len() as u64, &line);
            whole.extend_from_slice(&line);
            whole.push(b'\n');
        }
        let chosen = index.chosen(&[]);
        prop_assert!(chosen.lines.len() <= lines.len());
        let mut said = Vec::new();
        for shown in &chosen.lines {
            let at = usize::try_from(shown.at).unwrap_or(usize::MAX);
            prop_assert!(at + shown.len <= whole.len());
            if let Ok(Some(line)) = transcript::said(&whole[at..at + shown.len], shown.shows) {
                said.push(line);
            }
        }
        let mut rows = Rows::new(PathBuf::from(directory));
        rows.note("Stopped.");
        let mut buffer = render(&rows.rows()[0], directory).text;
        let mut placed = Placed::default();
        placed.put(0, 0..0, buffer.len());
        for change in rows.past(said) {
            let at = change.row();
            prop_assert!(at < rows.rows().len());
            let range = placed.range(change, |start, lines| line_start(&buffer, start, lines));
            prop_assert!(range.is_some(), "{:?} not drawn", change);
            let range = range.unwrap_or_default();
            let text = render(&rows.rows()[at], directory).text;
            buffer.replace_range(range.clone(), &text);
            placed.put(at, range, text.len());
        }
        let shown: String = rows.rows().iter().map(|row| render(row, directory).text).collect();
        prop_assert_eq!(&buffer, &shown);
    }
}
