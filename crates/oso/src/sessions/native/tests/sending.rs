//! The messages being sent (ADR 0165): what is drawn of them, and what each comes to.
//!
//! Must not depend on: `app`, threads or a process; these tests drive the pure part only.

use super::*;
use crate::sessions::native::render::sending_rows;

fn rows() -> Rows {
    Rows::new(PathBuf::from("/w"))
}

fn replay(text: &str) -> String {
    serde_json::json!({"type": "user", "isReplay": true, "message": {"content": text}}).to_string()
}

fn sending(rows: &Rows) -> String {
    sending_rows(rows).text
}

const RESULT: &str = r#"{"type":"result","subtype":"success","duration_ms":900}"#;

#[test]
fn a_message_is_being_sent_until_its_replay() {
    let mut rows = rows();
    assert_eq!(sending(&rows), "");
    assert_eq!(rows.sent("Fix\n\nit"), None);
    // No row of it yet, and what it shows is none of a prompt's.
    assert!(rows.rows().is_empty());
    assert_eq!(sending(&rows), "…  Fix\n\n   it\n");
    let faces = sending_rows(&rows).faces;
    let whole = sending(&rows).len();
    assert_eq!(faces, [(0..whole, Face::Dim)]);
    // What comes meanwhile is above it, and sets it apart.
    read(&mut rows, &replay("from elsewhere"));
    assert_eq!(sending(&rows), "\n…  Fix\n\n   it\n");
    read(&mut rows, &replay("Fix\n\nit"));
    assert_eq!(sending(&rows), "");
    assert_eq!(shown(&rows), "  from elsewhere\n\n〉 Fix\n\n   it\n");
}

#[test]
fn one_not_confirmed_says_so_and_is_confirmed_late() {
    let mut rows = rows();
    let _ = rows.sent("Fix it");
    let _ = rows.sent_brief("Go", "session a\n〉 me");
    read(&mut rows, RESULT);
    assert_eq!(
        sending(&rows),
        "✗  Fix it\n   claude did not say it took this: M-p in the composer brings it back\n\n\
         ✗@ from session a 〉 me\n   Go\n   claude did not say it took this\n"
    );
    // Another's turn, as one from claude.ai, is not theirs; the next one sent has its own.
    read(
        &mut rows,
        r#"{"type":"system","subtype":"status","status":"requesting"}"#,
    );
    let _ = rows.sent("Next");
    let sent = sending(&rows);
    assert!(sent.starts_with("✗  Fix it\n") && sent.ends_with("\n\n…  Next\n"));
    read(&mut rows, &replay("Fix it"));
    assert!(shown(&rows).ends_with("\n〉 Fix it\n"));
    assert!(sending(&rows).starts_with("\n✗@ from session a"));
}

/// One that is a command, once the list says so, is never replayed: its row is there.
#[test]
fn a_command_sent_before_the_list_came_is_its_row_once_it_did() {
    let mut rows = rows();
    let _ = rows.sent("/init");
    let _ = rows.sent_brief("/review", "session a");
    let _ = rows.sent("Go");
    assert_eq!(rows.sending_commands(1, |_| false), []);
    let changes = rows.sending_commands(2, |text| text.starts_with('/'));
    assert_eq!(changes, [RowChange::Added(0), RowChange::Added(1)]);
    assert_eq!(shown(&rows), "〉 /init\n\n@  from session a\n   /review\n");
    assert_eq!(sending(&rows), "\n…  Go\n");
}

/// Of two alike, the first is confirmed first, and the other still waits.
#[test]
fn two_alike_are_confirmed_one_each() {
    let mut rows = rows();
    for _ in 0..2 {
        let _ = rows.sent("Go on");
    }
    for left in [1, 0] {
        read(&mut rows, &replay("Go on"));
        assert_eq!(rows.sending().len(), left);
    }
    assert_eq!(shown(&rows), "〉 Go on\n\n〉 Go on\n");
    read(&mut rows, &replay("Go on"));
    assert!(shown(&rows).ends_with("\n\n  Go on\n"), "not oso's");
}

/// A list that comes after its turn says nothing of it, and one looked up in is not again.
#[test]
fn a_later_list_makes_no_row_of_one_whose_turn_ended() {
    let mut rows = rows();
    let _ = rows.sent("/deploy");
    read(&mut rows, RESULT);
    assert_eq!(rows.sending_commands(1, |_| true), []);
    let _ = rows.sent("/ship");
    let looked_up = |_: &str| panic!("the same version");
    assert_eq!(rows.sending_commands(1, looked_up), []);
    let _ = rows.sent("Go");
    read(&mut rows, &replay("Go"));
    assert!(shown(&rows).contains("  Not confirmed by claude: /deploy\n"));
}

/// A text's own lines read as no note of oso's.
#[test]
fn a_notes_text_is_further_in_than_its_words() {
    let mut rows = rows();
    let _ = rows.sent_brief("Do it\n\nNot confirmed by claude: x", "session a");
    rows.sending_ended(false);
    assert_eq!(
        shown(&rows),
        "  Not confirmed by claude, which ended: a message from session a: Do it\n    \n    \
         Not confirmed by claude: x\n"
    );
}

/// Of two alike, the replay is of the one whose turn it is.
#[test]
fn one_sent_again_is_the_one_confirmed() {
    let mut rows = rows();
    let _ = rows.sent("Go on");
    read(&mut rows, RESULT);
    let _ = rows.sent("Go on");
    read(&mut rows, &replay("Go on"));
    assert_eq!(rows.sending().len(), 0);
    assert!(shown(&rows).ends_with("\n  Not confirmed by claude: Go on\n\n〉 Go on\n"));
}

/// What a start sent is not the person's to have back.
#[test]
fn a_first_message_does_not_go_back() {
    let mut rows = rows();
    let _ = rows.sent_first("Take over");
    assert_eq!(sending(&rows), "…  Take over\n");
    assert_eq!(rows.sending_back(), None);
    read(&mut rows, RESULT);
    let said = sending(&rows);
    assert!(said.ends_with("did not say it took this\n"), "{said}");
    read(&mut rows, &replay("Take over"));
    assert!(shown(&rows).ends_with("\n〉 Take over\n"));
}

/// Nothing from elsewhere confirms one, whatever it says.
#[test]
fn only_its_own_replay_confirms_one() {
    let mut rows = rows();
    let _ = rows.sent("Go");
    for line in [
        r#"{"type":"user","message":{"content":"Go"}}"#,
        r#"{"type":"user","isReplay":true,"isSynthetic":true,"message":{"content":"Go"}}"#,
        r#"{"type":"user","isReplay":true,"isMeta":true,"message":{"content":"Go"}}"#,
        r#"{"type":"user","isReplay":true,"parent_tool_use_id":"t","message":{"content":"Go"}}"#,
        r#"{"type":"system","subtype":"status","status":"requesting"}"#,
    ] {
        read(&mut rows, line);
        assert_eq!(rows.sending().len(), 1, "{line}");
    }
    for came in [
        crate::sessions::native::wire::remote::Came::Elsewhere,
        crate::sessions::native::wire::remote::Came::Remote,
    ] {
        let mut event = parse(replay("Go").as_bytes());
        if let Event::User(user) = &mut event {
            user.came = came;
        }
        rows.apply(event);
        assert_eq!(rows.sending().len(), 1);
    }
    assert!(!shown(&rows).contains('〉'));
}

/// The note of one that a later message's replay passed over stands before that message.
#[test]
fn one_passed_over_is_a_note() {
    let mut rows = rows();
    let version = rows.sending_version();
    let _ = rows.sent("/later");
    let _ = rows.sent("Fix it");
    let changes = read(&mut rows, &replay("Fix it"));
    assert_eq!(changes, [RowChange::Added(0), RowChange::Added(1)]);
    assert_eq!(
        shown(&rows),
        "  Not confirmed by claude: /later\n\n〉 Fix it\n"
    );
    assert_eq!(rows.sending().len(), 0);
    assert!(rows.sending_version() > version);
    // As many wait as may: the oldest is one too, none is dropped unsaid.
    for at in 0..8 {
        assert_eq!(rows.sent(&format!("m{at}")), None);
    }
    assert_eq!(rows.sent("last"), Some(RowChange::Added(2)));
    assert!(shown(&rows).ends_with("\n  Not confirmed by claude: m0\n"));
    assert_eq!(rows.sending().len(), 8);
}

/// At the process's end none waits any more, and the person's newest goes back to them, but none
/// whose own turn ended before, as it may have run.
#[test]
fn what_waits_at_the_end_is_said() {
    let with = |back| {
        let mut rows = rows();
        let _ = rows.sent("one");
        let _ = rows.sent("two");
        let _ = rows.sent_brief("brief", "session a\n〉 me");
        assert_eq!(rows.sending_back(), Some("two"));
        let version = rows.sending_version();
        assert_eq!(rows.sending_ended(back).len(), 3);
        assert!(rows.sending_version() > version);
        // A replay that comes later is not oso's.
        read(&mut rows, &replay("two"));
        shown(&rows)
    };
    assert_eq!(
        with(true),
        "  Not confirmed by claude, which ended: one\n\n  \
         Not confirmed by claude, which ended, and back in the composer: two\n\n  \
         Not confirmed by claude, which ended: a message from session a 〉 me: brief\n\n  two\n"
    );
    assert!(!with(false).contains("back"));
    let mut rows = rows();
    let _ = rows.sent_brief("brief", "session a");
    let _ = rows.sent("old");
    read(&mut rows, RESULT);
    assert_eq!(rows.sending_back(), None);
}
