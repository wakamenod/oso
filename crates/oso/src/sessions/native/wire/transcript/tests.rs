//! Tests of a transcript's lines: where each is placed and what it shows, read without
//! decoding, and what a line shown says, on the recorded transcript of CLI 2.1.330 (scrubbed)
//! and lines made for each shape.
//!
//! Must not depend on: files, threads or the person's own transcripts.

use super::*;
use crate::sessions::native::wire::{Block, ToolInput};

const RECORDED: &str = include_str!("../../fixtures/2.1.330/transcript.jsonl");

fn uuid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// A user line with these top-level fields besides its ids and message.
fn user_line(fields: &str, content: &str) -> String {
    format!(
        r#"{{"parentUuid":"{}",{fields}"type":"user","message":{{"role":"user","content":{content}}},"uuid":"{}"}}"#,
        uuid(1),
        uuid(2)
    )
}

fn shows(line: &str) -> Option<Shows> {
    head(line.as_bytes()).unwrap().map(|head| head.shows)
}

#[test]
fn the_recorded_lines_are_placed_and_shown() {
    let heads: Vec<Option<Head>> = (RECORDED.lines())
        .map(|line| head(line.as_bytes()).unwrap())
        .collect();
    // The account's line is the CLI's own record: not of the conversation.
    assert_eq!(heads[0], None);
    let placed: Vec<&Head> = heads.iter().flatten().collect();
    assert_eq!(placed.len(), RECORDED.lines().count() - 1);
    let first = placed[0];
    assert_eq!(first.parent, None);
    assert_eq!(first.shows, Shows::Prompt);
    // Each line follows the one before it.
    for pair in placed.windows(2) {
        assert_eq!(pair[1].parent.as_deref(), Some(pair[0].uuid.as_str()));
    }
    let count = |shows: Shows| placed.iter().filter(|head| head.shows == shows).count();
    assert_eq!(count(Shows::Prompt), 4);
    assert!(count(Shows::Nothing) > 10, "attachments");
}

#[test]
fn the_person_is_who_the_transcript_marks() {
    let text = r#""Hi""#;
    let prompt = |fields: &str| shows(&user_line(fields, text));
    assert_eq!(
        prompt(r#""promptSource":"sdk","turnOrigin":"sdk","#),
        Some(Shows::Prompt)
    );
    assert_eq!(
        prompt(r#""promptSource":"typed","origin":{"kind":"human"},"#),
        Some(Shows::Prompt)
    );
    assert_eq!(prompt(r#""promptSource":"queued","#), Some(Shows::Prompt));
    assert_eq!(
        prompt(r#""promptSource":"suggestion_accepted","#),
        Some(Shows::Prompt)
    );
    // The CLI's own, a task's, or with no source: not the person's.
    assert_eq!(
        prompt(r#""promptSource":"sdk","isMeta":true,"#),
        Some(Shows::Message)
    );
    assert_eq!(
        prompt(r#""promptSource":"system","origin":{"kind":"task-notification"},"#),
        Some(Shows::Message)
    );
    assert_eq!(
        prompt(r#""promptSource":"sdk","origin":{"kind":"task-notification"},"#),
        Some(Shows::Message)
    );
    assert_eq!(prompt(""), Some(Shows::Message));
    assert_eq!(prompt(r#""promptSource":"Sdk","#), Some(Shows::Message));
    // Another session's.
    assert_eq!(
        prompt(r#""promptSource":"system","turnOrigin":"peer","isMeta":true,"#),
        Some(Shows::Peer)
    );
    assert_eq!(prompt(r#""origin":{"kind":"peer"},"#), Some(Shows::Peer));
    // A compaction's summary: its boundary says it.
    assert_eq!(
        prompt(r#""promptSource":"sdk","isCompactSummary":true,"#),
        Some(Shows::Nothing)
    );
}

/// Only a message from claude.ai, of those a native session takes, has an `origin` beside
/// `promptSource` `sdk`, or a `clientPlatform` (measured, ADR 0181); one typed in the TUI has an
/// origin too.
#[test]
fn a_message_that_came_another_way_is_never_the_persons() {
    let shown = |fields: &str| shows(&user_line(fields, r#""Hi""#));
    for fields in [
        r#""promptSource":"sdk","origin":{"kind":"human"},"clientPlatform":"ios","#,
        r#""promptSource":"sdk","origin":{"kind":"human"},"#,
        // Any origin, of whatever type.
        r#""promptSource":"sdk","origin":{},"#,
        r#""promptSource":"sdk","origin":"human","#,
        r#""promptSource":"sdk","origin":null,"clientPlatform":"web","#,
        // From a platform, into a session of the TUI's too.
        r#""promptSource":"typed","origin":{"kind":"human"},"clientPlatform":"android","#,
        r#""promptSource":"queued","clientPlatform":7,"#,
        r#""promptSource":"suggestion_accepted","clientPlatform":"","#,
    ] {
        assert_eq!(shown(fields), Some(Shows::Remote), "{fields}");
    }
    // oso's own, and the TUI's.
    for fields in [
        r#""promptSource":"sdk","#,
        r#""promptSource":"sdk","origin":null,"clientPlatform":null,"#,
        r#""promptSource":"typed","origin":{"kind":"human"},"#,
        r#""promptSource":"queued","origin":{"kind":"human"},"#,
    ] {
        assert_eq!(shown(fields), Some(Shows::Prompt), "{fields}");
    }
    // Never more than it was: another session's, the CLI's own, a side chain's.
    let peer = r#""promptSource":"sdk","origin":{"kind":"peer"},"clientPlatform":"ios","#;
    assert_eq!(shown(peer), Some(Shows::Peer));
    let meta = r#""promptSource":"sdk","origin":{"kind":"human"},"isMeta":true,"#;
    assert_eq!(shown(meta), Some(Shows::Message));
    let side = r#""promptSource":"sdk","origin":{"kind":"human"},"isSidechain":true,"#;
    assert_eq!(shown(side), None);
    let task = r#""promptSource":"sdk","origin":{"kind":"task-notification"},"#;
    assert_eq!(shown(task), Some(Shows::Message));
    let line = user_line(
        r#""promptSource":"sdk","origin":{"kind":"human"},"#,
        r#""Hi""#,
    );
    assert_eq!(
        said(line.as_bytes(), Shows::Remote),
        Ok(Some(Said::Remote {
            text: "Hi".into(),
            uuid: Some(uuid(2)),
        }))
    );
}

#[test]
fn a_side_chain_a_record_and_a_line_with_no_id_are_not_placed() {
    let side = user_line(r#""isSidechain":true,"promptSource":"sdk","#, r#""Hi""#);
    assert_eq!(head(side.as_bytes()), Ok(None));
    assert_eq!(head(br#"{"type":"last-prompt","uuid":"x"}"#), Ok(None));
    assert_eq!(head(br#"{"parentUuid":null,"type":"user"}"#), Ok(None));
    let long = format!(
        r#"{{"parentUuid":null,"type":"user","uuid":"{}"}}"#,
        "a".repeat(65)
    );
    assert_eq!(head(long.as_bytes()), Ok(None));
}

#[test]
fn a_line_of_the_conversation_whose_fields_do_not_read_is_unreadable() {
    let cut = br#"{"parentUuid":null,"type":"user","uuid":"a""#;
    assert_eq!(head(cut), Err(Unreadable));
    assert_eq!(head(b"{\"parentUuid\":"), Err(Unreadable));
    // Nested deeper than the fields are read.
    let deep = format!(
        r#"{{"parentUuid":null,"type":"user","toolUseResult":{}{},"uuid":"{}"}}"#,
        "[".repeat(200),
        "]".repeat(200),
        uuid(2)
    );
    assert_eq!(head(deep.as_bytes()), Err(Unreadable));
}

#[test]
fn a_compaction_follows_the_line_before_it() {
    let line = format!(
        r#"{{"parentUuid":null,"logicalParentUuid":"{}","type":"system","subtype":"compact_boundary","compactMetadata":{{"trigger":"manual","preTokens":1200,"postTokens":300}},"uuid":"{}"}}"#,
        uuid(1),
        uuid(2)
    );
    let placed = head(line.as_bytes()).unwrap().unwrap();
    assert_eq!(placed.parent, Some(uuid(1)));
    assert_eq!(placed.shows, Shows::Compacted);
    assert_eq!(
        said(line.as_bytes(), Shows::Compacted),
        Ok(Some(Said::Compacted {
            pre: Some(1200),
            post: Some(300),
            uuid: Some(uuid(2)),
        }))
    );
}

#[test]
fn an_attachment_is_placed_and_shows_nothing() {
    let line = format!(
        r#"{{"parentUuid":"{}","type":"attachment","attachment":{{"type":"session_context","email":"someone@example.com"}},"uuid":"{}"}}"#,
        uuid(1),
        uuid(2)
    );
    let placed = head(line.as_bytes()).unwrap().unwrap();
    assert_eq!(placed.shows, Shows::Nothing);
    assert_eq!(said(line.as_bytes(), Shows::Nothing), Ok(None));
}

#[test]
fn a_line_shown_says_what_the_streams_would() {
    let lines: Vec<&str> = RECORDED.lines().collect();
    let said_of = |line: &str| {
        let shows = head(line.as_bytes()).unwrap().unwrap().shows;
        said(line.as_bytes(), shows).unwrap()
    };
    assert_eq!(
        said_of(lines[1]),
        Some(Said::Prompt {
            text: "Reply with the single word apple.".into(),
            uuid: Some(uuid(1)),
        })
    );
    let edit = lines
        .iter()
        .find(|line| line.contains(r#""lines":["-one","+two"]"#))
        .unwrap();
    let Some(Said::User(user)) = said_of(edit) else {
        panic!("not a result");
    };
    let result = &user.results[0];
    assert_eq!(result.changed, (1, 1));
    assert_eq!(result.patch[0].lines, ["-one", "+two"]);
    let thinking = lines
        .iter()
        .find(|line| line.contains("thinkingDurationMs"))
        .unwrap();
    let Some(Said::Assistant(message)) = said_of(thinking) else {
        panic!("not a message");
    };
    assert_eq!(
        message.blocks,
        [Block::Thinking {
            text: String::new(),
            millis: Some(310)
        }]
    );
    let write = lines
        .iter()
        .find(|line| line.contains(r#""name":"Write""#))
        .unwrap();
    let Some(Said::Assistant(message)) = said_of(write) else {
        panic!("not a message");
    };
    let Block::ToolUse { input, .. } = &message.blocks[0] else {
        panic!("not a call");
    };
    assert_eq!(input.path.as_deref(), Some("/w/proj/note.txt"));
    assert_ne!(*input, ToolInput::default());
}

#[test]
fn a_peer_is_a_note_and_a_line_that_does_not_read_says_so() {
    let peer = user_line(
        r#""turnOrigin":"peer","isMeta":true,"#,
        r#""Another session""#,
    );
    assert_eq!(
        said(peer.as_bytes(), Shows::Peer),
        Ok(Some(Said::Note {
            text: "Another session".into(),
            uuid: Some(uuid(2)),
        }))
    );
    // A prompt with no text, such as an image's alone, shows nothing.
    let image = user_line(r#""promptSource":"sdk","#, r#"[{"type":"image"}]"#);
    assert_eq!(said(image.as_bytes(), Shows::Prompt), Ok(None));
    assert_eq!(said(b"{\"parentUuid\":", Shows::Message), Err(Unreadable));
    assert_eq!(said(b"[]", Shows::Message), Err(Unreadable));
}

#[test]
fn a_directory_is_read_only_of_a_line_of_the_conversation() {
    let line = user_line(r#""cwd":"/w/proj","#, r#""Hi""#);
    assert_eq!(cwd(line.as_bytes()), Some(PathBuf::from("/w/proj")));
    assert_eq!(cwd(br#"{"type":"x","cwd":"/w/proj"}"#), None);
    let long = user_line(&format!(r#""cwd":"/{}","#, "a".repeat(5000)), r#""Hi""#);
    assert_eq!(cwd(long.as_bytes()), None);
}
