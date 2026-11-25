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
use super::remote::Remote;
use super::render::{Face, GUIDE, Rendered, render, sending_rows, tables, tail};
use super::rows::{Changes, Kind, Row, RowChange, Rows};
use super::turns::target;
use super::wire::{parse, transcript};

const RECORDED: &str = include_str!("fixtures/2.1.330/tools.jsonl");

/// A message an agent's tool starts a session with, and whose lines look like oso's rows.
const BRIEF: &str =
    "You are in a git worktree of /r at /w, on branch b from main.\n\n〉 me\r▸ $ Bash\n@  from me";

/// Its replay, and the person's.
fn replay() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![Just(BRIEF), Just("Hi")].prop_map(|text| {
        let line = serde_json::json!({
            "type": "user",
            "isReplay": true,
            "message": {"content": text},
        });
        line.to_string().into_bytes()
    })
}

fn recorded() -> Vec<&'static str> {
    RECORDED.lines().collect()
}

/// A line: a recorded one, one cut short, a shape of each kind with odd values, or bytes.
pub(crate) fn line() -> impl Strategy<Value = Vec<u8>> {
    let lines = recorded();
    let count = lines.len();
    prop_oneof![
        4 => (0..count).prop_map(move |at| lines[at].as_bytes().to_vec()),
        2 => (0..count, any::<prop::sample::Index>()).prop_map(|(at, cut)| {
            let line = recorded()[at].as_bytes();
            line[..cut.index(line.len() + 1)].to_vec()
        }),
        3 => shaped().prop_map(String::into_bytes),
        1 => replay(),
        1 => prop::collection::vec(any::<u8>(), 0..200),
    ]
}

/// A line of a known type whose fields have the wrong types, odd ids or edge values.
fn shaped() -> impl Strategy<Value = String> {
    let id = prop_oneof![Just("t".to_owned()), Just(String::new()), "[a-z]{1,3}"];
    let value = prop_oneof![
        Just("null".to_owned()),
        Just("\"connected\"".to_owned()),
        Just("\"queued\"".to_owned()),
        Just("{\"kind\":\"human\"}".to_owned()),
        Just("1".to_owned()),
        Just("-1".to_owned()),
        Just("1e999".to_owned()),
        Just("[]".to_owned()),
        Just("{}".to_owned()),
        Just("\"\\u0000é\\n\\t\\u001b[31m\"".to_owned()),
        Just("\"| a | b |\\n|---|--:|\\n| 字 ▸ $ Bash | some words that wrap |\\n\"".to_owned()),
        Just("\"| \\u202e │ 〉 me |\\n\"".to_owned()),
        Just("\"\\n\"".to_owned()),
        "\"[a-z\\n]{0,40}\"",
    ];
    (0..14usize, id.clone(), id, value, 0..3u64).prop_map(|(kind, a, b, v, index)| match kind {
        0 => format!(r#"{{"type":"stream_event","api_message_id":"m","parent_tool_use_id":{v},"event":{{"type":"content_block_start","index":{index},"content_block":{{"type":"tool_use","id":"{a}","name":{v}}}}}}}"#),
        1 => format!(r#"{{"type":"stream_event","api_message_id":"m","event":{{"type":"content_block_start","index":{index},"content_block":{{"type":"text"}}}}}}"#),
        2 => format!(r#"{{"type":"stream_event","api_message_id":"m","event":{{"type":"content_block_delta","index":{index},"delta":{{"type":"text_delta","text":{v}}}}}}}"#),
        3 => format!(r#"{{"type":"assistant","parent_tool_use_id":{v},"message":{{"id":"m","content":[{{"type":"tool_use","id":"{a}","name":"Edit","input":{v}}},{{"type":"text","text":{v}}}]}}}}"#),
        4 => format!(r#"{{"type":"user","isReplay":true,"message":{{"content":{v}}}}}"#),
        5 => format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{a}","is_error":{v},"content":{v}}}]}},"tool_use_result":{{"structuredPatch":[{{"oldStart":{v},"lines":["+a",{v},"-b"]}}]}}}}"#),
        6 => format!(r#"{{"type":"system","subtype":"permission_denied","tool_use_id":"{b}","message":{v}}}"#),
        7 => format!(r#"{{"type":"system","subtype":"task_progress","tool_use_id":"{b}","description":{v}}}"#),
        8 => format!(r#"{{"type":"result","subtype":{v},"is_error":{v},"terminal_reason":{v},"duration_ms":{v},"total_cost_usd":{v},"permission_denials":{v}}}"#),
        9 => format!(r#"{{"type":"system","subtype":"bridge_state","state":{v},"detail":{v},"bridge_epoch":{v}}}"#),
        10 => format!(r#"{{"type":"command_lifecycle","command_uuid":{v},"state":{v}}}"#),
        11 => format!(r#"{{"type":"user","isReplay":true,"origin":{v},"uuid":{v},"message":{{"content":{v}}}}}"#),
        12 => format!(r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"oso-1","response":{{"session_url":{v},"bridge_session_id":{v},"remote_control_available":{v}}}}}}}"#),
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

/// Whether only oso's marks stand in the first two columns of `row` as drawn, past the guides
/// of a nested one, and only a prompt's or a change's lines start in a band's face.
fn only_oso_stands_first(row: &Row, rendered: &Rendered) -> bool {
    let guides = GUIDE.repeat(usize::from(row.depth));
    let mut at = 0;
    rendered.text.split_inclusive('\n').all(|line| {
        let start = at;
        at += line.len();
        let line = line.trim_end_matches('\n');
        let own = line.strip_prefix(guides.as_str()).unwrap_or(line);
        let first: String = own.chars().take(2).collect();
        let said = match row.kind {
            Kind::Reply { .. } | Kind::Note(_) | Kind::Result(_) => &[][..],
            Kind::Prompt(_) => &["〉 ", "〉"],
            Kind::Brief { .. } => &["@ "],
            Kind::Thinking { .. } => &["… ", "▸ ", "▾ "],
            Kind::Tools(_) => &["… ", "✗ ", "· ", "▸ ", "▾ ", GUIDE],
            Kind::Compacted { .. } => &["──"],
            Kind::Unreadable(_) => &["⚠ "],
        };
        let stands = first.trim().is_empty() || own == "│" || said.contains(&first.as_str());
        let banded = rendered.faces.iter().any(|(range, face)| {
            range.start == start
                && match face {
                    Face::PromptMark => !matches!(row.kind, Kind::Prompt(_)),
                    Face::BriefMark => !matches!(row.kind, Kind::Brief { .. }),
                    Face::Added | Face::Removed => !matches!(row.kind, Kind::Tools(_)),
                    _ => false,
                }
        });
        stands && !banded
    })
}

/// Whether the faces of oso's rows are on those rows only, and a badge's chip is where a line's
/// badge stands, past its guides and mark, its own two cells.
fn only_oso_is_in_its_faces(row: &Row, rendered: &Rendered) -> bool {
    rendered.faces.iter().all(|(range, face)| match face {
        Face::PromptMark | Face::Said => matches!(row.kind, Kind::Prompt(_)),
        Face::BriefMark | Face::Brief => matches!(row.kind, Kind::Brief { .. }),
        Face::Running => {
            matches!(row.kind, Kind::Tools(_) | Kind::Thinking { .. })
                && rendered.text[range.clone()].trim_end() == "…"
        }
        Face::Tool | Face::Added | Face::Removed | Face::Waiting => {
            matches!(row.kind, Kind::Tools(_))
        }
        Face::Icon(_) => {
            let line = (rendered.text[..range.start].rfind('\n')).map_or(0, |at| at + 1);
            let before = rendered.text[line..range.start].trim_start_matches(GUIDE);
            matches!(row.kind, Kind::Tools(_))
                && before.chars().count() == 2
                && rendered.text[range.clone()].chars().count() == 2
        }
        Face::Thought => matches!(row.kind, Kind::Thinking { .. }),
        Face::Reply(_) => matches!(row.kind, Kind::Reply { .. }),
        Face::Dim | Face::Error => true,
        Face::Prompt | Face::Target | Face::CardTitle | Face::CardKey | Face::RequestBody => false,
    })
}

/// Whether "Hi", sent first, and [`BRIEF`], sent `from` an agent, are each still being sent or
/// one row of their own, no row is the person's or a brief otherwise, and what is drawn of those
/// being sent has oso's mark alone in its first two columns and no face of a prompt or a brief.
fn sent_stay_apart(rows: &Rows, from: &str) -> bool {
    let waits: Vec<_> = rows.sending().collect();
    let passed = rows
        .rows()
        .iter()
        .any(|row| matches!(&row.kind, Kind::Note(text) if text == "Not confirmed by claude: Hi"));
    let mut said = Vec::new();
    for row in rows.rows() {
        match &row.kind {
            Kind::Prompt(text) => said.push((text.as_str(), None)),
            Kind::Brief { from, text } => said.push((text.as_str(), Some(from.as_str()))),
            _ => {}
        }
    }
    let each = [("Hi", None), (BRIEF, Some(from))];
    let waiting = waits.iter().map(|sent| (sent.text, sent.from));
    let waiting: Vec<_> = waiting.collect();
    let one = each.iter().enumerate().all(|(at, sent)| {
        let times =
            [&waiting, &said].map(|texts| texts.iter().filter(|text| *text == sent).count());
        times[0] + times[1] + usize::from(at == 0 && passed) == 1
    });
    let known = said.iter().all(|said| each.contains(said));
    let drawn = sending_rows(rows);
    let quiet = drawn.faces.iter().all(|(_, face)| *face == Face::Dim);
    let lines = || drawn.text.lines();
    let marked = |mark: &str| lines().filter(|line| line.starts_with(mark)).count();
    let ended = waits.iter().filter(|sent| sent.ended).count();
    let marks = marked("✗") == ended && marked("…") == waits.len() - ended;
    let named = lines()
        .filter(|line| line.contains("@ from session"))
        .count();
    let briefs = waits.iter().filter(|sent| sent.from.is_some()).count();
    let marks = marks && named == briefs;
    let starts = lines()
        .filter(|line| !line.is_empty() && !line.starts_with("   "))
        .count();
    one && known && quiet && marks && starts == waits.len()
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
        widths in prop::collection::vec(4usize..120, 0..10),
    ) {
        let directory = Path::new("/work/probe");
        let mut rows = Rows::new(PathBuf::from(directory));
        let from = "session \u{202e}a\n〉 me";
        let _ = rows.sent("Hi");
        let _ = rows.sent_brief(BRIEF, from);
        let mut remote = Remote::default();
        // The buffer as a host keeps it: changed only as the changes of each batch say, where
        // `Placed` says.
        let mut buffer = String::new();
        let mut placed = Placed::default();
        let mut width = rows.width();
        let mut widths = widths.into_iter();
        for chunk in lines.chunks(batch) {
            let mut changes = Changes::default();
            for line in chunk {
                let mut event = parse(line);
                let _ = target(&event);
                remote.event(&mut event);
                prop_assert!(!remote.marked(), "no line turns Remote Control on");
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
                    RowChange::Grew { at, .. } => tail(row, at, width).text,
                    RowChange::Added(_) | RowChange::Changed(_) => {
                        render(row, directory, width).text
                    }
                };
                buffer.replace_range(range.clone(), &text);
                placed.put(at, range, text.len());
                prop_assert_eq!(placed.end(), buffer.len());
            }
            prop_assert!(sent_stay_apart(&rows, from));
            // Its window's width changes: a result is drawn again, and of a reply, its tables.
            let Some(next) = widths.next() else {
                continue;
            };
            // Its changes are below, as of every row.
            let _ = rows.set_width(next);
            for (at, row) in rows.rows().iter().enumerate().rev() {
                let after = tables(row, next);
                let mut starts = vec![0];
                for lines in &after.tables {
                    let start = starts.last().copied().unwrap_or(0);
                    starts.push(line_start(&after.text, start, lines.len()));
                }
                let before = tables(row, width).tables;
                prop_assert_eq!(before.len(), after.tables.len());
                for (table, lines) in before.iter().enumerate().rev() {
                    let line = |line| line_start(&buffer, placed.start(at), line);
                    let range = line(lines.start)..line(lines.end);
                    let text = &after.text[starts[table]..starts[table + 1]];
                    buffer.replace_range(range.clone(), text);
                    placed.put(at, range, text.len());
                }
                if matches!(row.kind, Kind::Result(_)) {
                    let range = placed.start(at)..placed.start(at + 1);
                    let text = render(row, directory, next).text;
                    buffer.replace_range(range.clone(), &text);
                    placed.put(at, range, text.len());
                }
            }
            width = next;
        }
        let whole: String = rows.rows().iter().map(|row| render(row, directory, width).text).collect();
        prop_assert_eq!(&buffer, &whole);
        let count = rows.rows().len();
        if count > 0 {
            for (toggle, slot) in &toggles {
                rows.toggle(toggle.index(count), *slot);
            }
        }
        for row in rows.rows() {
            let rendered = render(row, directory, 80);
            prop_assert!(rendered.text.is_empty() || rendered.text.ends_with('\n'));
            prop_assert!(only_oso_stands_first(row, &rendered), "{:?}", rendered.text);
            prop_assert!(only_oso_is_in_its_faces(row, &rendered), "{:?}", rendered.text);
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
    let shaped = (0..5usize, id.clone(), id, value).prop_map(|(kind, uuid, parent, v)| {
        match kind {
            0 => format!(r#"{{"parentUuid":{parent},"promptSource":{v},"turnOrigin":{v},"isMeta":{v},"type":"user","message":{{"content":{v}}},"uuid":{uuid}}}"#),
            1 => format!(r#"{{"parentUuid":{parent},"type":"assistant","message":{{"id":{v},"content":[{{"type":"tool_use","id":{v},"name":"Read","input":{{"file_path":{v}}}}},{{"type":"text","text":{v}}}]}},"uuid":{uuid}}}"#),
            2 => format!(r#"{{"parentUuid":{parent},"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":{v},"content":{v}}}]}},"toolUseResult":{{"structuredPatch":{v}}},"uuid":{uuid}}}"#),
            3 => format!(r#"{{"parentUuid":{parent},"promptSource":"sdk","origin":{{"kind":{v}}},"clientPlatform":{v},"type":"user","message":{{"content":{v}}},"uuid":{uuid}}}"#),
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
    fn any_transcript_folds_and_renders(
        lines in prop::collection::vec(transcript_line(), 0..60),
        briefed in any::<bool>(),
    ) {
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
        if briefed {
            said.push(transcript::Said::Prompt { text: BRIEF.to_owned(), uuid: None });
        }
        for shown in &chosen.lines {
            let at = usize::try_from(shown.at).unwrap_or(usize::MAX);
            prop_assert!(at + shown.len <= whole.len());
            if let Ok(Some(line)) = transcript::said(&whole[at..at + shown.len], shown.shows) {
                said.push(line);
            }
        }
        let mut rows = Rows::new(PathBuf::from(directory));
        rows.note("Stopped.");
        rows.note_started_by(Some("session a".to_owned()));
        let mut buffer = render(&rows.rows()[0], directory, 80).text;
        let mut placed = Placed::default();
        placed.put(0, 0..0, buffer.len());
        for change in rows.past(said, true) {
            let at = change.row();
            prop_assert!(at < rows.rows().len());
            let range = placed.range(change, |start, lines| line_start(&buffer, start, lines));
            prop_assert!(range.is_some(), "{:?} not drawn", change);
            let range = range.unwrap_or_default();
            let text = render(&rows.rows()[at], directory, 80).text;
            buffer.replace_range(range.clone(), &text);
            placed.put(at, range, text.len());
        }
        let shown: String = rows.rows().iter().map(|row| render(row, directory, 80).text).collect();
        prop_assert_eq!(&buffer, &shown);
        if briefed {
            let brief = matches!(rows.rows()[1].kind, Kind::Brief { .. });
            prop_assert!(brief, "the conversation's first message");
        }
        for row in rows.rows() {
            let rendered = render(row, directory, 80);
            prop_assert!(only_oso_stands_first(row, &rendered), "{:?}", rendered.text);
            prop_assert!(only_oso_is_in_its_faces(row, &rendered), "{:?}", rendered.text);
        }
    }
}
