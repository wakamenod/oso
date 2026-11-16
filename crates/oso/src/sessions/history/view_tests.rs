//! Tests of a conversation's read-only view (ADR 0145, step 2): the transcript read whole,
//! past what the search holds, its header and the line of the asked message, and the thread's
//! read, on transcripts written to a scratch directory.
//!
//! Must not depend on: `app` or `dispatch`; or on the person's own transcripts.

use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use oso_core::editor::ConversationScope;

use super::held::{MAX_VIEW, VIEW_LINE_COST};
use super::tests::{Scratch, id, prompt, request, search, tree};
use super::{History, Hold, Report, view_of};

#[test]
fn the_view_reads_the_transcript_whole_with_a_header_and_the_messages_line() {
    let scratch = Scratch::new("view");
    let (older, _) = tree(&scratch);
    // Past what the search holds of one conversation, which the view still reads.
    let many: Vec<String> = (0..3000)
        .map(|n| prompt("/w/m", &format!("message {n} {}", "x".repeat(400))))
        .collect();
    scratch.write("-w-m", &id(5), &many, 5);
    let mut hold = Hold::default();
    search(
        &scratch.0,
        &mut hold,
        &request("trust", ConversationScope::all()),
    );
    let (about, text, line) = view_of(&hold, &older, 1).expect("a view");
    assert_eq!(about.id, older);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("in "), "{text}");
    assert!(
        lines[1].starts_with(&format!("session {}", older.as_str())),
        "{text}"
    );
    assert!(lines[line].starts_with("‹ The trust record"), "{text}");
    assert!(text.contains("› find the trust bug"));
    assert!(!text.contains("trust=secret"), "never a tool result");
    let big = hold
        .conversations
        .iter()
        .find(|c| c.about.id == id(5))
        .unwrap();
    assert!(
        big.cut && big.messages.len() < 3000,
        "the search holds less"
    );
    let (_, text, line) = view_of(&hold, &id(5), 2999).expect("a view");
    assert!(
        text.lines().nth(1).unwrap().ends_with("3000 messages"),
        "read whole"
    );
    assert!(
        text.lines()
            .nth(line)
            .unwrap()
            .starts_with("› message 2999")
    );
    assert!(
        view_of(&hold, &id(6), 0).is_none(),
        "one the search does not know"
    );
}

#[test]
fn the_thread_reads_a_view() {
    let scratch = Scratch::new("thread-view");
    let (older, _) = tree(&scratch);
    let (sent, reports) = mpsc::channel();
    let sent = Mutex::new(sent);
    let mut history = History::new(Some(scratch.0.clone()), move |report| {
        let _ = sent.lock().unwrap().send(report);
    });
    history.search(request("record", ConversationScope::all()));
    let Report::Found(_) = reports.recv_timeout(Duration::from_secs(10)).unwrap() else {
        panic!("rows first");
    };
    history.read(older.clone(), 0);
    let Report::Read(read) = reports.recv_timeout(Duration::from_secs(10)).unwrap() else {
        panic!("the view");
    };
    assert_eq!(read.session, older);
    assert!(read.view.is_some());
    assert!(!format!("{read:?}").contains("trust"));
}

#[test]
fn a_view_of_line_breaks_alone_stops_at_its_bound() {
    let scratch = Scratch::new("view-bound");
    // Replies of nothing but line breaks, which the view would make a line each.
    let breaks = "\n".repeat(64 * 1024);
    let lines: Vec<String> = (0..200)
        .map(|n| {
            if n == 0 {
                prompt("/w/b", "start")
            } else {
                serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": format!("x{breaks}y")}]}})
                    .to_string()
            }
        })
        .collect();
    scratch.write("-w-b", &id(7), &lines, 5);
    let mut hold = Hold::default();
    search(
        &scratch.0,
        &mut hold,
        &request("start", ConversationScope::all()),
    );
    let (_, text, _) = view_of(&hold, &id(7), 0).expect("a view");
    assert!(text.contains("more was said than the view keeps"), "cut");
    let rows = text.lines().count();
    assert!(rows <= MAX_VIEW / VIEW_LINE_COST, "{rows} rows");
    assert!(text.len() <= 2 * MAX_VIEW, "{} bytes", text.len());
}
