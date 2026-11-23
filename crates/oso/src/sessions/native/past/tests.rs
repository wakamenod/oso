//! Tests of which lines of a transcript a session's earlier turns show: the chain from the
//! newest line, branches and compactions, the budget and its cut at a prompt, the notes, and
//! the rows the recorded transcript gives.
//!
//! Must not depend on: files, threads or the person's own transcripts.

use std::path::{Path, PathBuf};

use super::*;
use crate::sessions::native::render::render;
use crate::sessions::native::rows::{Kind, RowChange, Rows, ToolState};
use crate::sessions::native::wire::transcript::said;

const RECORDED: &str = include_str!("../fixtures/2.1.330/transcript.jsonl");

/// The index of `lines`, each at its offset.
fn index(lines: &[String]) -> Index {
    let mut index = Index::default();
    let mut at = 0;
    for line in lines {
        assert!(index.take(at, line.as_bytes()));
        at += line.len() as u64 + 1;
    }
    index
}

fn id(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn parent(of: Option<usize>) -> String {
    of.map_or_else(|| "null".to_owned(), |n| format!("\"{}\"", id(n)))
}

fn prompt(n: usize, of: Option<usize>, text: &str) -> String {
    format!(
        r#"{{"parentUuid":{},"promptSource":"sdk","type":"user","message":{{"role":"user","content":"{text}"}},"uuid":"{}"}}"#,
        parent(of),
        id(n)
    )
}

fn reply(n: usize, of: usize, text: &str) -> String {
    format!(
        r#"{{"parentUuid":"{}","type":"assistant","message":{{"id":"m{n}","role":"assistant","content":[{{"type":"text","text":"{text}"}}]}},"uuid":"{}"}}"#,
        id(of),
        id(n)
    )
}

/// The texts of the chosen lines, read as they would be.
fn texts(lines: &[String], chosen: &Chosen) -> Vec<String> {
    let whole = lines.join("\n");
    (chosen.lines.iter())
        .map(|shown| {
            let at = usize::try_from(shown.at).unwrap();
            let line = &whole.as_bytes()[at..at + shown.len];
            match said(line, shown.shows).unwrap() {
                Some(Said::Prompt { text, .. }) => text,
                Some(Said::Assistant(message)) => format!("{:?}", message.blocks),
                other => format!("{other:?}"),
            }
        })
        .collect()
}

#[test]
fn the_chain_ends_at_the_newest_line_and_leaves_a_rewound_branch() {
    let lines = vec![
        prompt(1, None, "first"),
        reply(2, 1, "one"),
        // Rewound: this branch is left.
        prompt(3, Some(2), "left"),
        reply(4, 3, "gone"),
        // The newest, from the first reply.
        prompt(5, Some(2), "second"),
        reply(6, 5, "two"),
    ];
    let chosen = index(&lines).chosen(&[]);
    assert!(!chosen.cut);
    let shown = texts(&lines, &chosen);
    assert_eq!(shown.len(), 4, "{shown:?}");
    assert_eq!(shown[0], "first");
    assert_eq!(shown[2], "second");
    assert!(
        !shown
            .iter()
            .any(|text| text.contains("left") || text.contains("gone"))
    );
}

#[test]
fn the_chain_goes_on_across_a_compaction() {
    let boundary = format!(
        r#"{{"parentUuid":null,"logicalParentUuid":"{}","type":"system","subtype":"compact_boundary","compactMetadata":{{"preTokens":10}},"uuid":"{}"}}"#,
        id(2),
        id(3)
    );
    let summary = format!(
        r#"{{"parentUuid":"{}","isCompactSummary":true,"type":"user","message":{{"content":"Summary"}},"uuid":"{}"}}"#,
        id(3),
        id(4)
    );
    let lines = vec![
        prompt(1, None, "before"),
        reply(2, 1, "one"),
        boundary,
        summary,
        prompt(5, Some(4), "after"),
    ];
    let chosen = index(&lines).chosen(&[]);
    let shown: Vec<Shows> = chosen.lines.iter().map(|shown| shown.shows).collect();
    assert_eq!(
        shown,
        [
            Shows::Prompt,
            Shows::Message,
            Shows::Compacted,
            Shows::Prompt
        ]
    );
}

#[test]
fn a_cut_keeps_the_newest_whole_turns() {
    let big = "x".repeat(1 << 20);
    let mut lines = Vec::new();
    for turn in 0..12 {
        let at = turn * 2 + 1;
        lines.push(prompt(
            at,
            (turn > 0).then(|| at - 1),
            &format!("turn {turn}"),
        ));
        lines.push(reply(at + 1, at, &big));
    }
    let chosen = index(&lines).chosen(&[]);
    assert!(chosen.cut);
    let bytes: u64 = chosen.lines.iter().map(|shown| shown.len as u64).sum();
    assert!(bytes <= MAX_BYTES);
    // It starts at a prompt, and says how many come before it.
    assert_eq!(chosen.lines[0].shows, Shows::Prompt);
    let kept = chosen.lines.len() / 2;
    assert_eq!(chosen.lines.len() % 2, 0);
    assert_eq!(chosen.prompts_left_out, 12 - kept);
    let shown = texts(&lines, &chosen);
    assert_eq!(shown[0], format!("turn {}", 12 - kept));
}

#[test]
fn at_most_so_many_lines_are_shown() {
    let mut lines = vec![prompt(1, None, "one")];
    for n in 2..MAX_LINES + 10 {
        lines.push(reply(n, n - 1, "a"));
    }
    let chosen = index(&lines).chosen(&[]);
    // No prompt within the lines kept: the newest are shown as they are.
    assert_eq!(chosen.lines.len(), MAX_LINES);
    assert!(chosen.cut);
    assert_eq!(chosen.prompts_left_out, 1);
}

#[test]
fn the_newest_line_is_shown_whatever_its_length() {
    let lines = vec![
        prompt(1, None, "one"),
        reply(2, 1, &"y".repeat(usize::try_from(MAX_BYTES).unwrap() + 1)),
    ];
    let chosen = index(&lines).chosen(&[]);
    assert_eq!(chosen.lines.len(), 1);
    assert_eq!(chosen.prompts_left_out, 1);
}

#[test]
fn a_parent_not_before_its_line_ends_the_chain() {
    // A line naming a later one, or itself, as its parent.
    let lines = vec![
        prompt(1, Some(2), "one"),
        prompt(2, Some(2), "two"),
        reply(3, 2, "three"),
    ];
    let chosen = index(&lines).chosen(&[]);
    assert_eq!(chosen.lines.len(), 2);
    // What it named is not known: the rows say turns may be left out.
    assert!(chosen.cut);
}

#[test]
fn nothing_to_show_chooses_nothing() {
    let lines = vec![
        r#"{"type":"session-owner"}"#.to_owned(),
        "garbage".to_owned(),
    ];
    assert_eq!(index(&lines).chosen(&[]), Chosen::default());
    assert_eq!(Index::default().chosen(&[]), Chosen::default());
}

#[test]
fn the_notes_say_what_is_left_out_and_why() {
    let said = vec![Said::Prompt {
        text: "hi".into(),
        uuid: None,
    }];
    let past = Past {
        said: said.clone(),
        cut: true,
        prompts_left_out: 3,
        unreadable: 2,
        ..Past::default()
    };
    assert_eq!(
        past.before().as_deref(),
        Some(
            "Its earlier turns, as Claude Code's transcript records them; 3 turns before them \
             are not shown here: session-search shows the whole conversation."
        )
    );
    let whole = Past {
        said,
        ..Past::default()
    };
    assert_eq!(
        whole.before().as_deref(),
        Some("Its earlier turns, as Claude Code's transcript records them.")
    );
    assert_eq!(
        past.after(),
        ["2 lines of its transcript did not read, and are not shown."]
    );
    assert_eq!(Past::default().before(), None);
    assert!(Past::default().after().is_empty());
    let missing = Past::missing(Missing::NotFound);
    assert_eq!(
        missing.after(),
        ["Its earlier turns are not shown: its transcript was not found."]
    );
    // What a conversation says is never in its `Debug`.
    let said = Past {
        said: vec![Said::Prompt {
            text: "a secret".into(),
            uuid: None,
        }],
        ..Past::default()
    };
    assert!(!format!("{said:?}").contains("secret"));
}

/// The rows the recorded transcript gives, as `sessions::past` reads it.
fn recorded_rows() -> (Rows, Vec<RowChange>) {
    let lines: Vec<String> = RECORDED.lines().map(str::to_owned).collect();
    let chosen = index(&lines).chosen(&[]);
    let whole = lines.join("\n");
    let said: Vec<Said> = (chosen.lines.iter())
        .filter_map(|shown| {
            let at = usize::try_from(shown.at).unwrap();
            said(&whole.as_bytes()[at..at + shown.len], shown.shows).unwrap()
        })
        .collect();
    let mut rows = Rows::new(PathBuf::from("/w/proj"));
    let changes = rows.past(said);
    (rows, changes)
}

#[test]
fn the_recorded_turns_are_rows_as_live_ones_are() {
    let (rows, changes) = recorded_rows();
    assert!(rows.holds_conversation());
    let kinds: Vec<String> = (rows.rows().iter())
        .map(|row| match &row.kind {
            Kind::Prompt(text) => format!("prompt {text}"),
            Kind::Reply { text, .. } => format!("reply {text}"),
            Kind::Thinking { .. } => "thinking".to_owned(),
            Kind::Tools(tools) => {
                let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
                format!("tools {}", names.join(" "))
            }
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds[0], "prompt Reply with the single word apple.");
    assert_eq!(kinds[1], "reply apple");
    assert!(kinds.contains(&"tools Write".to_owned()), "{kinds:?}");
    assert!(kinds.contains(&"tools Edit".to_owned()), "{kinds:?}");
    assert!(kinds.contains(&"tools Agent".to_owned()), "{kinds:?}");
    assert_eq!(kinds.last().map(String::as_str), Some("reply plum"));
    // Each call has its result, and the edit its diff.
    for row in rows.rows() {
        if let Kind::Tools(tools) = &row.kind {
            assert!(tools.iter().all(|tool| tool.state == ToolState::Done));
            assert!(row.folded);
        }
    }
    let edit = (rows.rows().iter())
        .find_map(|row| match &row.kind {
            Kind::Tools(tools) if tools[0].name == "Edit" => Some(&tools[0]),
            _ => None,
        })
        .unwrap();
    assert_eq!(edit.result.as_ref().unwrap().changed, (1, 1));
    // Every row is said once, as added.
    assert_eq!(changes.len(), rows.rows().len());
    assert!((changes.iter().enumerate()).all(|(at, change)| *change == RowChange::Added(at)));
    for row in rows.rows() {
        assert!(render(row, Path::new("/w/proj")).text.ends_with('\n'));
    }
}

#[test]
fn a_call_with_no_result_is_unfinished() {
    let call = format!(
        r#"{{"parentUuid":"{}","type":"assistant","message":{{"id":"m","content":[{{"type":"tool_use","id":"t","name":"Bash","input":{{"command":"ls"}}}}]}},"uuid":"{}"}}"#,
        id(1),
        id(2)
    );
    let lines = vec![prompt(1, None, "run"), call];
    let chosen = index(&lines).chosen(&[]);
    let whole = lines.join("\n");
    let said: Vec<Said> = (chosen.lines.iter())
        .filter_map(|shown| {
            let at = usize::try_from(shown.at).unwrap();
            said(&whole.as_bytes()[at..at + shown.len], shown.shows).unwrap()
        })
        .collect();
    let mut rows = Rows::new(PathBuf::from("/w"));
    rows.past(said);
    let Kind::Tools(tools) = &rows.rows()[1].kind else {
        panic!("not a call");
    };
    assert_eq!(tools[0].state, ToolState::Unfinished);
}

#[test]
fn a_wrapped_prompt_and_a_commands_output_show_as_live_ones() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    rows.past(vec![
        Said::Prompt {
            text: "<command-name>/model</command-name>".into(),
            uuid: None,
        },
        Said::Prompt {
            text: "hello".into(),
            uuid: None,
        },
        Said::Note {
            text: "From another session".into(),
            uuid: None,
        },
    ]);
    let kinds: Vec<&Kind> = rows.rows().iter().map(|row| &row.kind).collect();
    assert_eq!(
        kinds,
        [
            &Kind::Prompt("hello".into()),
            &Kind::Note("From another session".into())
        ]
    );
    assert!(!Rows::new(PathBuf::from("/w")).holds_conversation());
}

#[test]
fn a_chain_from_a_line_not_placed_is_cut_there() {
    // Its first line follows one before the part read, or one that did not read.
    let lines = vec![
        "{\"parentUuid\":null,\"type\":\"user\"".to_owned(),
        prompt(2, Some(1), "two"),
        reply(3, 2, "three"),
    ];
    let index = index(&lines);
    let chosen = index.chosen(&[]);
    assert_eq!(chosen.unreadable, 1);
    assert_eq!(chosen.lines.len(), 2);
    assert!(chosen.cut);
    assert_eq!(chosen.prompts_left_out, 0);
}

#[test]
fn after_the_rows_newest_message_only_what_follows_is_shown() {
    let lines = vec![
        prompt(1, None, "first"),
        reply(2, 1, "one"),
        prompt(3, Some(2), "second"),
        reply(4, 3, "two"),
    ];
    let placed = index(&lines);
    // The rows end at the first reply: the second turn follows it.
    let chosen = placed.chosen(&[id(2), id(1)]);
    assert_eq!(chosen.since, Since::After { last: true });
    assert!(!chosen.cut);
    assert_eq!(texts(&lines, &chosen)[0], "second");
    assert_eq!(chosen.lines.len(), 2);
    // The rows end at the newest line: nothing follows.
    let chosen = placed.chosen(&[id(4), id(3)]);
    assert!(chosen.lines.is_empty());
    assert_eq!(chosen.since, Since::After { last: true });
    // The rows' newest is on a branch left since, an earlier one on the chain: it went on
    // from there.
    let branched = vec![
        prompt(1, None, "first"),
        reply(2, 1, "one"),
        prompt(3, Some(2), "left"),
        reply(4, 3, "gone"),
        prompt(5, Some(2), "instead"),
    ];
    let chosen = index(&branched).chosen(&[id(4), id(2)]);
    assert_eq!(chosen.since, Since::After { last: false });
    assert_eq!(texts(&branched, &chosen), ["instead"]);
}

#[test]
fn rows_not_on_the_chain_get_its_newest_turns() {
    let lines = vec![
        prompt(1, None, "first"),
        reply(2, 1, "one"),
        // Rewound elsewhere to the start: a new branch from nothing.
        prompt(3, None, "again"),
        reply(4, 3, "anew"),
    ];
    let chosen = index(&lines).chosen(&[id(2), id(1)]);
    assert_eq!(chosen.since, Since::Rewound);
    let shown = texts(&lines, &chosen);
    assert_eq!(shown[0], "again");
    assert_eq!(shown.len(), 2);
}

#[test]
fn the_note_says_where_the_turns_start() {
    let said = vec![Said::Prompt {
        text: "hi".into(),
        uuid: None,
    }];
    let note = |since| {
        Past {
            said: said.clone(),
            since,
            ..Past::default()
        }
        .before()
        .unwrap()
    };
    assert_eq!(
        note(Since::After { last: true }),
        "Its turns since, as Claude Code's transcript records them."
    );
    assert!(note(Since::After { last: false }).starts_with("Its conversation went on from"));
    assert!(note(Since::Rewound).starts_with("The turns above are no longer its conversation"));
    // Rows that hold the conversation say nothing of a transcript not found.
    let quiet = Past {
        after_rows: true,
        ..Past::missing(Missing::NotFound)
    };
    assert!(quiet.after().is_empty());
    assert_eq!(quiet.before(), None);
}

#[test]
fn the_rows_keep_their_newest_messages_ids() {
    let (rows, _) = recorded_rows();
    let kept = rows.kept_ids();
    assert!(!kept.is_empty());
    assert!(kept.len() <= 64);
    // The newest first: the last reply's line.
    let last = RECORDED.lines().last().unwrap();
    assert!(
        last.contains(&format!("\"uuid\":\"{}\"", kept[0])),
        "{}",
        kept[0]
    );
}

#[test]
fn a_copy_of_a_kept_id_written_later_does_not_move_where_the_rows_end() {
    let lines = vec![
        prompt(1, None, "first"),
        reply(2, 1, "one"),
        prompt(3, Some(2), "second"),
        reply(4, 3, "two"),
        // The newest line, written into the file with the id of the rows' first reply.
        reply(2, 4, "forged"),
    ];
    let chosen = index(&lines).chosen(&[id(2)]);
    assert_eq!(chosen.since, Since::After { last: true });
    let shown = texts(&lines, &chosen);
    assert!(shown.iter().any(|text| text == "second"), "{shown:?}");
}

#[test]
fn the_newest_kept_id_the_transcript_has_is_the_last() {
    let lines = vec![
        prompt(1, None, "first"),
        reply(2, 1, "one"),
        prompt(3, Some(2), "more"),
    ];
    // `only-the-stream` is an id the transcript never wrote.
    let chosen = index(&lines).chosen(&["only-the-stream".into(), id(2)]);
    assert_eq!(chosen.since, Since::After { last: true });
    assert_eq!(texts(&lines, &chosen), ["more"]);
}

#[test]
fn lines_that_did_not_read_are_counted_after_where_the_rows_end() {
    let lines = vec![
        "{\"parentUuid\":null,\"type\":\"user\"".to_owned(),
        prompt(1, None, "first"),
        reply(2, 1, "one"),
        "{\"parentUuid\":null,\"type\":\"user\"".to_owned(),
        prompt(3, Some(2), "second"),
    ];
    let index = index(&lines);
    assert_eq!(index.chosen(&[]).unreadable, 2);
    assert_eq!(index.chosen(&[id(2)]).unreadable, 1);
}

#[test]
fn rows_holding_the_conversation_hear_of_a_transcript_that_did_not_read() {
    let past = |why| Past {
        after_rows: true,
        ..Past::missing(why)
    };
    assert!(past(Missing::NotFound).after().is_empty());
    for why in [
        Missing::NotRead,
        Missing::Elsewhere,
        Missing::TooSlow,
        Missing::TooMany,
    ] {
        let notes = past(why).after();
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].starts_with("Its turns since are not shown: "),
            "{notes:?}"
        );
    }
}
