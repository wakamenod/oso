//! Tests of the documents a server knows: changes that take the text before to the text after
//! in both encodings, replayed by a scan of their own; the carriage return check after a batch
//! against a scan of the whole text; and when a document is kept from the server.
//!
//! Must not depend on: the session's loop or a process.

use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use proptest::prelude::*;
use serde_json::Value;

use super::*;
use crate::server::{Closed, Document};

/// Applies `changes` to `text` as a server would, finding each position by a scan of its own:
/// lines end at `\n`, and a column counts bytes or UTF-16 units.
fn replay(text: &str, changes: &[Value], encoding: Encoding) -> String {
    let mut text = text.to_owned();
    for change in changes {
        let Some(range) = change.get("range") else {
            text = change["text"].as_str().unwrap().to_owned();
            continue;
        };
        let at = |position: &Value| offset(&text, position, encoding);
        let (start, end) = (at(&range["start"]), at(&range["end"]));
        text.replace_range(start..end, change["text"].as_str().unwrap());
    }
    text
}

fn offset(text: &str, position: &Value, encoding: Encoding) -> usize {
    let line = position["line"].as_u64().unwrap() as usize;
    let column = position["character"].as_u64().unwrap() as usize;
    let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let mut units = 0;
    for (index, c) in text[start..].char_indices() {
        if units >= column || c == '\n' {
            return start + index;
        }
        units += match encoding {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
        };
    }
    text.len()
}

/// The splices, ascending and apart, that `edits` make, each `(start, old_len, insert)` in the
/// text before, with the text after.
fn batch(before: &str, edits: &[(usize, usize, String)]) -> (Vec<Splice>, String) {
    let mut splices = Vec::new();
    let mut after = String::new();
    let mut at = 0;
    for (start, len, insert) in edits {
        let start = floor(before, (*start).max(at));
        let end = floor(before, (start + len).min(before.len()));
        after.push_str(&before[at..start]);
        after.push_str(insert);
        splices.push(Splice {
            start,
            old_len: end - start,
            new_len: insert.len(),
        });
        at = end;
    }
    after.push_str(&before[at..]);
    (splices, after)
}

fn floor(text: &str, at: usize) -> usize {
    text.floor_char_boundary(at.min(text.len()))
}

fn text_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just("a".to_owned()),
            Just("é".to_owned()),
            Just("日".to_owned()),
            Just("\u{1F600}".to_owned()),
            Just("\n".to_owned()),
            Just("\r\n".to_owned()),
            Just("\r".to_owned()),
            Just(" ".to_owned()),
        ],
        0..60,
    )
    .prop_map(|parts| parts.concat())
}

fn edits_strategy() -> impl Strategy<Value = Vec<(usize, usize, String)>> {
    prop::collection::vec((0usize..200, 0usize..20, text_strategy()), 0..6).prop_map(|mut edits| {
        edits.sort_by_key(|edit| edit.0);
        edits
    })
}

proptest! {
    #[test]
    fn changes_take_the_text_before_to_the_text_after(
        before in text_strategy(),
        edits in edits_strategy(),
        utf16 in any::<bool>(),
    ) {
        let encoding = if utf16 { Encoding::Utf16 } else { Encoding::Utf8 };
        let (splices, after) = batch(&before, &edits);
        let changes = content_changes(
            &Rope::from(before.as_str()),
            &Rope::from(after.as_str()),
            Some(&splices),
            encoding,
        );
        prop_assert_eq!(replay(&before, &changes, encoding), after);
    }

    #[test]
    fn the_check_after_a_batch_finds_what_a_whole_scan_does(
        before in text_strategy(),
        edits in edits_strategy(),
    ) {
        // A text with no lone carriage return to begin from: each `\r` kept is a `\r\n`'s.
        let before = before.replace("\r\n", "\0").replace('\r', "").replace('\0', "\r\n");
        prop_assert!(!lone_cr(&Rope::from(before.as_str())));
        let (splices, after) = batch(&before, &edits);
        let after = Rope::from(after.as_str());
        prop_assert_eq!(lone_cr_near(&after, &splices), lone_cr(&after));
    }
}

#[test]
fn a_batch_that_does_not_place_goes_whole() {
    let before = Rope::from("abc");
    let after = Rope::from("abXc");
    let wrong = [Splice {
        start: 1,
        old_len: 0,
        new_len: 5,
    }];
    let changes = content_changes(&before, &after, Some(&wrong), Encoding::Utf8);
    assert_eq!(changes, [serde_json::json!({"text": "abXc"})]);
    let changes = content_changes(&before, &after, None, Encoding::Utf8);
    assert_eq!(changes, [serde_json::json!({"text": "abXc"})]);
    // Inside a character: whole.
    let before = Rope::from("é");
    let after = Rope::from("xé");
    let inside = [Splice {
        start: 1,
        old_len: 0,
        new_len: 1,
    }];
    assert_eq!(
        content_changes(&before, &after, Some(&inside), Encoding::Utf8),
        [serde_json::json!({"text": "xé"})]
    );
}

#[test]
fn lone_carriage_returns_are_found() {
    for (text, lone) in [
        ("", false),
        ("a\r\nb\r\n", false),
        ("a\rb", true),
        ("a\r", true),
        ("\r\r\n", true),
    ] {
        assert_eq!(lone_cr(&Rope::from(text)), lone, "{text:?}");
    }
}

fn document(id: u64, text: &str) -> Document {
    Document {
        id,
        uri: format!("file:///w/{id}.rs"),
        language_id: "rust".into(),
        text: Rope::from(text),
        revision: 0,
    }
}

fn methods(out: &Out) -> Vec<&'static str> {
    out.send.iter().map(|(method, _)| *method).collect()
}

/// Every whole text that waits, sent as room allows: here, at once.
fn waiting(documents: &mut Documents) -> Vec<(&'static str, Value)> {
    let mut sent = Vec::new();
    while let Some(next) = documents.next_whole() {
        sent.extend(next);
    }
    sent
}

fn names(sent: &[(&'static str, Value)]) -> Vec<&'static str> {
    sent.iter().map(|(method, _)| *method).collect()
}

#[test]
fn a_document_opens_changes_saves_and_closes() {
    let mut documents = Documents::default();
    let out = documents.open(document(1, "fn a() {}\n"), true);
    assert!(out.send.is_empty(), "the open waits for room");
    assert_eq!(out.state, Some(None), "every open says how it went");
    let sent = waiting(&mut documents);
    assert_eq!(names(&sent), ["textDocument/didOpen"]);
    assert_eq!(sent[0].1["textDocument"]["version"], 1);
    let before = Rope::from("fn a() {}\n");
    let after = Rope::from("fn ab() {}\n");
    let splices = [Splice {
        start: 4,
        old_len: 0,
        new_len: 1,
    }];
    let out = documents.change(1, 0, &before, after, Some(&splices), Encoding::Utf8, true);
    assert_eq!(methods(&out), ["textDocument/didChange"]);
    assert_eq!(out.send[0].1["textDocument"]["version"], 2);
    assert_eq!(documents.text(1).unwrap(), "fn ab() {}\n");
    assert_eq!(methods(&documents.save(1, true)), ["textDocument/didSave"]);
    assert_eq!(methods(&documents.close(1)), ["textDocument/didClose"]);
    assert!(documents.text(1).is_none());
}

#[test]
fn a_document_is_kept_until_the_server_is_ready_and_opened_again_each_run() {
    let mut documents = Documents::default();
    assert!(documents.open(document(1, "a"), false).send.is_empty());
    assert!(!documents.waiting());
    let out = documents.change(
        1,
        0,
        &Rope::from("a"),
        Rope::from("ab"),
        None,
        Encoding::Utf8,
        true,
    );
    assert!(out.send.is_empty());
    documents.open_all();
    let sent = waiting(&mut documents);
    assert_eq!(names(&sent), ["textDocument/didOpen"]);
    assert_eq!(sent[0].1["textDocument"]["text"], "ab");
    documents.forget_sent();
    documents.open_all();
    let sent = waiting(&mut documents);
    assert_eq!(names(&sent), ["textDocument/didOpen"], "the next run");
    assert_eq!(
        sent[0].1["textDocument"]["version"], 2,
        "versions only go up"
    );
}

#[test]
fn a_whole_text_waits_and_goes_as_the_latest() {
    let mut documents = Documents::default();
    documents.open(document(1, "a"), true);
    waiting(&mut documents);
    // No splices: a whole text waits, and later batches only change what it will be.
    let out = documents.change(
        1,
        0,
        &Rope::from("a"),
        Rope::from("ab"),
        None,
        Encoding::Utf8,
        true,
    );
    assert!(out.send.is_empty());
    let splices = [Splice {
        start: 2,
        old_len: 0,
        new_len: 1,
    }];
    let out = documents.change(
        1,
        0,
        &Rope::from("ab"),
        Rope::from("abc"),
        Some(&splices),
        Encoding::Utf8,
        true,
    );
    assert!(
        out.send.is_empty(),
        "the server's copy is behind: no incremental change"
    );
    assert!(
        documents.save(1, true).send.is_empty(),
        "the save waits for the text"
    );
    let sent = waiting(&mut documents);
    assert_eq!(
        names(&sent),
        ["textDocument/didChange", "textDocument/didSave"]
    );
    assert_eq!(
        sent[0].1["contentChanges"],
        serde_json::json!([{"text": "abc"}])
    );
    // In step again: the next batch is incremental.
    let splices = [Splice {
        start: 3,
        old_len: 0,
        new_len: 1,
    }];
    let out = documents.change(
        1,
        0,
        &Rope::from("abc"),
        Rope::from("abcd"),
        Some(&splices),
        Encoding::Utf8,
        true,
    );
    assert_eq!(methods(&out), ["textDocument/didChange"]);
    // A server that wants whole texts gets one that waits, each time.
    let out = documents.change(
        1,
        0,
        &Rope::from("abcd"),
        Rope::from("abcde"),
        Some(&splices),
        Encoding::Utf8,
        false,
    );
    assert!(out.send.is_empty());
    assert!(documents.waiting());
}

#[test]
fn a_lone_carriage_return_keeps_a_document_from_the_server_until_a_save_that_fits() {
    let mut documents = Documents::default();
    let out = documents.open(document(1, "a\rb"), true);
    assert_eq!(out.state, Some(Some(Closed::LoneCarriageReturn)));
    assert!(waiting(&mut documents).is_empty());

    documents.open(document(2, "a\nb"), true);
    assert_eq!(names(&waiting(&mut documents)), ["textDocument/didOpen"]);
    let splices = [Splice {
        start: 1,
        old_len: 1,
        new_len: 1,
    }];
    let out = documents.change(
        2,
        0,
        &Rope::from("a\nb"),
        Rope::from("a\rb"),
        Some(&splices),
        Encoding::Utf8,
        true,
    );
    assert_eq!(methods(&out), ["textDocument/didClose"]);
    assert_eq!(out.state, Some(Some(Closed::LoneCarriageReturn)));
    // Edited while closed: nothing goes, and a save with the \r still there opens nothing.
    let out = documents.change(
        2,
        0,
        &Rope::from("a\rb"),
        Rope::from("a\rbc"),
        None,
        Encoding::Utf8,
        true,
    );
    assert!(out.send.is_empty() && out.state.is_none());
    assert_eq!(documents.save(2, true), Out::default());
    // Fixed, then saved: opened again with its text.
    documents.change(
        2,
        0,
        &Rope::from("a\rbc"),
        Rope::from("a\r\nbc"),
        None,
        Encoding::Utf8,
        true,
    );
    let out = documents.save(2, true);
    assert_eq!(out.state, Some(None));
    let sent = waiting(&mut documents);
    assert_eq!(names(&sent), ["textDocument/didOpen"]);
    assert_eq!(sent[0].1["textDocument"]["text"], "a\r\nbc");
}

#[test]
fn a_batch_that_does_not_place_is_checked_whole() {
    let mut documents = Documents::default();
    documents.open(document(1, "x\ny"), true);
    waiting(&mut documents);
    // Splices that do not take one length to the other place nowhere: the text is checked
    // whole, and its lone \r found far from them.
    let wrong = [Splice {
        start: 3,
        old_len: 0,
        new_len: 5,
    }];
    let out = documents.change(
        1,
        0,
        &Rope::from("x\ny"),
        Rope::from("\rx\ny!"),
        Some(&wrong),
        Encoding::Utf8,
        true,
    );
    assert_eq!(out.state, Some(Some(Closed::LoneCarriageReturn)), "{out:?}");
}

#[test]
fn a_document_past_the_bound_is_closed() {
    let mut documents = Documents::default();
    documents.open(document(1, "a"), true);
    waiting(&mut documents);
    let large = Rope::from("x".repeat(DOCUMENT_BYTES_MAX + 1).as_str());
    let out = documents.change(1, 0, &Rope::from("a"), large, None, Encoding::Utf8, true);
    assert_eq!(methods(&out), ["textDocument/didClose"]);
    assert_eq!(out.state, Some(Some(Closed::TooLarge)));
    documents.open(document(2, &"x".repeat(DOCUMENT_BYTES_MAX)), true);
    assert_eq!(
        names(&waiting(&mut documents)),
        ["textDocument/didOpen"],
        "at the bound"
    );
}

#[test]
fn the_check_after_a_batch_sees_each_way_to_leave_a_lone_carriage_return() {
    let check = |after: &str, start: usize, old_len: usize, new_len: usize| {
        lone_cr_near(
            &Rope::from(after),
            &[Splice {
                start,
                old_len,
                new_len,
            }],
        )
    };
    assert!(check("a\rb", 1, 0, 1), "inserted before another character");
    assert!(check("ab\r", 2, 0, 1), "inserted at the end");
    assert!(
        check("a\rxb", 2, 0, 1),
        "inserted after a \\r, taking its \\n's place"
    );
    assert!(check("a\rb", 2, 1, 0), "the \\n after a \\r deleted");
    assert!(!check("a\r\nb", 1, 0, 2), "a \\r\\n inserted");
    assert!(!check("a\r\nb", 2, 0, 1), "a \\n put after a \\r");
}

#[test]
fn a_batch_before_the_server_is_ready_is_judged_too() {
    let mut documents = Documents::default();
    documents.open(document(1, "a"), false);
    let large = Rope::from("x".repeat(DOCUMENT_BYTES_MAX + 1).as_str());
    let out = documents.change(1, 0, &Rope::from("a"), large, None, Encoding::Utf8, true);
    assert_eq!(out.state, Some(Some(Closed::TooLarge)));
    assert!(out.send.is_empty(), "nothing was sent to close");
    documents.open_all();
    assert!(waiting(&mut documents).is_empty(), "and nothing opens");
}
