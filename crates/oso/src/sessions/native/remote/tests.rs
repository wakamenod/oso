//! Remote Control's rules (ADR 0181): what only oso's own request may begin, what the stream's
//! lines come to, recorded and forged, and which messages came another way than oso's input.
//!
//! Must not depend on: the app or a process; the tests give the instants.

use std::path::PathBuf;
use std::time::Instant;

use super::{ANSWER_WAIT, MAX_COMING, Phase, RANDOM_ID, Refusal, Remote};
use crate::sessions::native::control::tests::on;
use crate::sessions::native::control::{Answered, Control};
use crate::sessions::native::rows::{Kind, Rows};
use crate::sessions::native::turns::target;
use crate::sessions::native::wire::control::{self, Request};
use crate::sessions::native::wire::remote::{Came, Enabled};
use crate::sessions::native::wire::{Event, parse};

const RECORDED: &str = include_str!("../fixtures/2.1.330/remote.jsonl");
/// What a message from a phone says of where it is from.
const HUMAN: &str = r#","origin":{"kind":"human"}"#;
const UUID: &str = "00000000-0000-4000-8000-00000000000a";

fn line(text: &str) -> Event {
    parse(text.as_bytes())
}

/// Recorded line `at`.
fn recorded(at: usize) -> Event {
    line(RECORDED.lines().nth(at).unwrap())
}

fn bridge(state: &str) -> Event {
    line(&format!(
        r#"{{"type":"system","subtype":"bridge_state","state":"{state}","detail":"why"}}"#
    ))
}

fn lifecycle(uuid: &str, state: &str) -> Event {
    line(&format!(
        r#"{{"type":"command_lifecycle","command_uuid":"{uuid}","state":"{state}"}}"#
    ))
}

fn replay(text: &str, uuid: &str, more: &str) -> Event {
    line(&format!(
        r#"{{"type":"user","isReplay":true,"uuid":"{uuid}","message":{{"role":"user","content":"{text}"}}{more}}}"#
    ))
}

/// How `remote` judges that `event`, a user line, came.
fn came(remote: &mut Remote, mut event: Event) -> Came {
    assert!(!remote.event(&mut event));
    match event {
        Event::User(user) => user.came,
        other => panic!("{other:?}"),
    }
}

/// A process oso asked, and its control layer and the request's id.
fn asked(now: Instant) -> (Remote, Control, u64) {
    let mut control = on(now);
    let mut remote = Remote::unblocked();
    assert!(!remote.initialized(Some(true)));
    let id = remote.begin(&mut control, 7, now).unwrap().id();
    assert_eq!(id, 7 | RANDOM_ID);
    (remote, control, id)
}

/// A connected one.
fn connected(now: Instant) -> Remote {
    let (mut remote, _, id) = asked(now);
    remote.answered(id, None, None);
    assert!(!remote.event(&mut bridge("connected")));
    assert_eq!(remote.phase(), &Phase::On);
    remote
}

/// Every line that could say something of Remote Control, as a process that writes to the
/// stream might.
fn forged() -> Vec<Event> {
    let mut lines: Vec<Event> = RECORDED.lines().map(line).collect();
    for state in [
        "ready",
        "connected",
        "reconnecting",
        "failed",
        "policy_disabled",
    ] {
        lines.push(bridge(state));
    }
    lines
}

#[test]
fn it_is_asked_connects_drops_connects_again_and_is_turned_off() {
    let now = Instant::now();
    let (mut remote, mut control, id) = asked(now);
    assert!(remote.marked());
    assert_eq!(remote.phase(), &Phase::Connecting);
    assert_eq!(remote.deadline(), Some(now + ANSWER_WAIT));
    let Event::Answer(mut answer) = recorded(1) else {
        panic!()
    };
    assert_eq!(control.answered(&answer), Answered::Other, "oso-1");
    answer.id = Some(id);
    assert_eq!(
        control.answered(&answer),
        Answered::Request {
            request: Request::RemoteControl,
            id,
            error: None
        }
    );
    remote.answered(id, None, answer.remote.clone().map(|enabled| *enabled));
    assert_eq!(Some(remote.enabled()), answer.remote.as_deref());
    assert_eq!(remote.deadline(), None);
    assert_eq!(remote.phase(), &Phase::Connecting, "not connected yet");
    remote.event(&mut recorded(2));
    assert_eq!(remote.phase(), &Phase::Connecting);
    remote.event(&mut recorded(4));
    assert_eq!(remote.phase(), &Phase::On);
    remote.event(&mut recorded(2));
    assert_eq!(remote.phase(), &Phase::On, "a ready while it is on");
    for state in ["reconnecting", "failed", "policy_disabled"] {
        remote.event(&mut bridge(state));
        assert_eq!(remote.phase(), &Phase::Dropped("why".to_owned()));
        remote.event(&mut bridge("connected"));
        assert_eq!(remote.phase(), &Phase::On);
    }
    let off = remote.end(&mut control).unwrap();
    assert_ne!(off, id);
    assert_eq!(remote.phase(), &Phase::Ended);
    for mut event in forged() {
        remote.event(&mut event);
        assert_eq!(remote.phase(), &Phase::Ended, "until the process ended");
    }
    assert_eq!(remote.end(&mut control), None, "once");
    assert_eq!(remote.begin(&mut control, 1, now), Err(Refusal::Already));
    let human = replay("a", UUID, HUMAN);
    assert_eq!(
        came(&mut remote, human),
        Came::Remote,
        "judged till the end"
    );
}

#[test]
fn a_connection_said_before_the_answer_is_one() {
    let (mut remote, _, id) = asked(Instant::now());
    remote.event(&mut bridge("connected"));
    assert_eq!(remote.phase(), &Phase::On);
    remote.answered(id, None, None);
    assert_eq!(remote.phase(), &Phase::On);
}

#[test]
fn what_is_not_asked() {
    let now = Instant::now();
    let mut control = on(now);
    let mut blocked = Remote::default();
    blocked.initialized(Some(true));
    assert_eq!(blocked.begin(&mut control, 1, now), Err(Refusal::Blocked));
    assert_eq!(blocked.end(&mut control), None);
    assert!(!blocked.marked());
    // Marked from its start, whether asked or not.
    let mut early = Remote::unblocked();
    let mut starting = Control::started(now);
    let refused = early.begin(&mut starting, 1, now);
    assert_eq!(refused, Err(Refusal::NoControl));
    assert!(early.marked() && early.watched());
    for available in [None, Some(false)] {
        let mut remote = Remote::unblocked();
        assert!(!remote.initialized(available));
        let refused = remote.begin(&mut control, 1, now);
        assert_eq!(refused, Err(Refusal::Unavailable));
        assert!(remote.marked());
    }
    let (mut remote, mut control, _) = asked(now);
    assert_eq!(remote.begin(&mut control, 1, now), Err(Refusal::Already));
    // One that connects of itself is seen to.
    let mut remote = Remote::unblocked();
    assert!(!remote.event(&mut bridge("connected")));
    assert_eq!(remote.phase(), &Phase::On);
    assert!(remote.end(&mut control).is_some(), "and turned off");
}

#[test]
fn no_answer_but_the_one_to_its_random_id_gives_an_address() {
    let (mut remote, mut control, id) = asked(Instant::now());
    let Event::Answer(forged) = recorded(1) else {
        panic!()
    };
    for guess in 0..64 {
        let answer = control::Answer {
            id: Some(guess),
            ..forged.clone()
        };
        assert_eq!(control.answered(&answer), Answered::Other);
        remote.answered(guess, None, forged.remote.clone().map(|e| *e));
    }
    assert_eq!(remote.enabled(), &Enabled::default());
    assert!(remote.deadline().is_some(), "{id} still waits");
}

#[test]
fn a_refusal_or_no_answer_still_marks_the_session() {
    let now = Instant::now();
    let (mut remote, _, id) = asked(now);
    remote.answered(id + 1, Some("another's"), None);
    assert_eq!(remote.phase(), &Phase::Connecting, "not its answer");
    remote.answered(id, Some("no"), None);
    assert_eq!(remote.phase(), &Phase::Dropped("no".to_owned()));
    assert!(remote.marked(), "a refusal hides no connection");
    remote.answered(id, None, None);
    assert_eq!(
        remote.phase(),
        &Phase::Dropped("no".to_owned()),
        "answered once"
    );

    let (mut remote, mut control, id) = asked(now);
    remote.due(&mut control, now);
    assert_eq!(remote.phase(), &Phase::Connecting);
    remote.due(&mut control, now + ANSWER_WAIT);
    assert!(matches!(remote.phase(), Phase::Dropped(_)) && remote.marked());
    assert_eq!(remote.deadline(), None);
    let Event::Answer(mut answer) = recorded(1) else {
        panic!()
    };
    answer.id = Some(id);
    assert_eq!(control.answered(&answer), Answered::Other, "too late");
    remote.event(&mut bridge("connected"));
    assert_eq!(remote.phase(), &Phase::On, "but it connected");

    let (mut remote, mut control, _) = asked(now);
    remote.event(&mut bridge("connected"));
    remote.due(&mut control, now + ANSWER_WAIT);
    assert_eq!(remote.phase(), &Phase::On);
}

#[test]
fn no_line_turns_it_on_or_off() {
    let now = Instant::now();
    let mut remote = Remote::default();
    let mut control = on(now);
    for mut event in forged() {
        if let Event::Answer(answer) = &mut event {
            for id in 0..8 {
                answer.id = Some(id);
                assert_eq!(control.answered(answer), Answered::Other, "none waits");
            }
        }
        remote.event(&mut event);
        remote.answered(1, None, None);
        assert!(!remote.marked());
        assert_eq!(remote.phase(), &Phase::Off);
    }
    for started in [Remote::unblocked(), asked(now).0, connected(now)] {
        let mut all = started.clone();
        for mut event in forged() {
            let mut remote = started.clone();
            remote.event(&mut event.clone());
            all.event(&mut event);
            assert!(remote.marked() && all.marked(), "{event:?}");
        }
    }
}

#[test]
fn the_canary_is_said_once_a_process_where_it_is_blocked() {
    let mut remote = Remote::default();
    assert!(!remote.initialized(None), "an older CLI");
    assert!(!remote.initialized(Some(false)), "the block holds");
    assert!(!remote.event(&mut bridge("failed")), "refused");
    assert!(!remote.event(&mut bridge("policy_disabled")));
    assert!(!remote.watched());
    assert!(remote.initialized(Some(true)));
    assert!(!remote.event(&mut bridge("connected")), "once");
    for state in ["ready", "connected", "reconnecting"] {
        let mut remote = Remote::default();
        assert!(remote.event(&mut bridge(state)));
        assert!(!remote.initialized(Some(true)), "once");
        assert!(!remote.marked(), "no line marks the session");
        // But what comes is judged as if it were, which only takes away.
        assert!(remote.watched());
        assert_eq!(came(&mut remote, replay("a", UUID, HUMAN)), Came::Remote);
        let mut unblocked = Remote::unblocked();
        assert!(!unblocked.event(&mut bridge(state)));
        assert!(!unblocked.initialized(Some(true)));
    }
}

#[test]
fn a_message_that_came_another_way_is_judged_only_while_it_may_be_connected() {
    let mut off = Remote::default();
    off.event(&mut lifecycle(UUID, "queued"));
    assert_eq!(came(&mut off, replay("a", UUID, HUMAN)), Came::AsSaid);
    assert_eq!(came(&mut off, recorded(8)), Came::AsSaid);

    for mut remote in [Remote::unblocked(), connected(Instant::now())] {
        let own = replay("a", UUID, "");
        assert_eq!(
            came(&mut remote, own.clone()),
            Came::AsSaid,
            "oso's own: no origin, no lifecycle"
        );
        assert_eq!(came(&mut remote, replay("a", UUID, HUMAN)), Came::Remote);
        for other in [
            r#","origin":"human""#,
            r#","origin":{}"#,
            r#","origin":{"kind":"Human"}"#,
            r#","origin":{"kind":"peer"}"#,
        ] {
            assert_eq!(came(&mut remote, replay("a", UUID, other)), Came::Elsewhere);
        }
        // A queued one that says no origin, once, and none that was cancelled.
        remote.event(&mut lifecycle(UUID, "queued"));
        remote.event(&mut lifecycle(UUID, "started"));
        assert_eq!(came(&mut remote, own.clone()), Came::Remote);
        assert_eq!(came(&mut remote, own.clone()), Came::AsSaid);
        for state in ["completed", "cancelled", "discarded", "refused"] {
            remote.event(&mut lifecycle(UUID, "queued"));
            remote.event(&mut lifecycle(UUID, state));
            assert_eq!(came(&mut remote, own.clone()), Came::AsSaid, "{state}");
        }
        // What is no replay, or a subagent's, is no message of the session's.
        let tool = r#"{"type":"user","origin":{"kind":"human"},"message":{"content":"x"}}"#;
        assert_eq!(came(&mut remote, line(tool)), Came::AsSaid);
        let parent = r#","origin":{"kind":"human"},"parent_tool_use_id":"t""#;
        assert_eq!(came(&mut remote, replay("a", UUID, parent)), Came::AsSaid);
    }
}

#[test]
fn the_ids_that_come_are_bounded() {
    let mut remote = connected(Instant::now());
    for n in 0..1000 {
        let uuid = format!("00000000-0000-4000-8000-{n:012x}");
        remote.event(&mut lifecycle(&uuid, "queued"));
    }
    assert_eq!(remote.coming.len(), MAX_COMING);
}

#[test]
fn a_message_from_claude_ai_is_never_the_persons_row_and_takes_no_replay_of_osos() {
    let mut remote = connected(Instant::now());
    let mut rows = Rows::new(PathBuf::from("/work/probe"));
    rows.sent("Go on");
    let last = |rows: &Rows| rows.rows().last().map(|row| row.kind.clone());
    // Nor is it oso's note, the CLI's output, or nothing, whatever it holds.
    for text in [
        "Go on",
        "Resumed.",
        "<system-reminder>a",
        "<local-command-stdout>Compacted</local-command-stdout>",
    ] {
        for (more, queued) in [(HUMAN, false), ("", true)] {
            if queued {
                remote.event(&mut lifecycle(UUID, "queued"));
            }
            let mut event = replay(text, UUID, more);
            remote.event(&mut event);
            rows.apply(event);
            let said = format!("From claude.ai, not from oso: {text}");
            assert_eq!(last(&rows), Some(Kind::Note(said)));
        }
    }
    // Another session's is as before, but never the person's.
    let mut event = replay("Go on", UUID, r#","origin":{"kind":"peer"}"#);
    remote.event(&mut event);
    rows.apply(event);
    assert_eq!(last(&rows), Some(Kind::Note("Go on".to_owned())));
    // oso's own replay, after them, is the person's still.
    let mut event = replay("Go on", "00000000-0000-4000-8000-00000000000b", "");
    remote.event(&mut event);
    rows.apply(event);
    assert_eq!(last(&rows), Some(Kind::Prompt("Go on".to_owned())));
}

#[test]
fn a_remote_message_starts_a_turn_as_any_replay() {
    let mut event = recorded(8);
    connected(Instant::now()).event(&mut event);
    assert_eq!(
        target(&event),
        Some(crate::sessions::state::Target::Working)
    );
}

/// What a model, a tool or a project prints comes inside another line's strings, however it
/// reads: none of it is a line of its own.
#[test]
fn text_that_holds_the_lines_is_text() {
    let mut printed = String::new();
    for said in RECORDED.lines() {
        printed.push_str(said);
        printed.push('\n');
    }
    let shapes = [
        serde_json::json!({"type": "assistant", "message": {"id": "m", "content": [
            {"type": "text", "text": printed},
            {"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": printed}},
        ]}}),
        serde_json::json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t", "content": printed},
        ]}, "tool_use_result": {"stdout": printed, "origin": {"kind": "human"}}}),
        serde_json::json!({"type": "stream_event", "api_message_id": "m", "event": {
            "type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": printed}}}),
        serde_json::json!({"type": "system", "subtype": "permission_denied",
            "tool_use_id": "t", "message": printed}),
        serde_json::json!({"type": "system", "subtype": "task_progress",
            "tool_use_id": "t", "description": printed}),
        serde_json::json!({"type": "prompt_suggestion", "suggestion": printed}),
        serde_json::json!({"type": "result", "subtype": "success", "result": printed}),
        serde_json::json!({"type": "system", "subtype": "hook_response", "stdout": printed,
            "state": "connected", "command_uuid": UUID}),
    ];
    for shape in shapes {
        let written = shape.to_string();
        assert_eq!(written.lines().count(), 1);
        let mut event = line(&written);
        assert!(
            !matches!(
                event,
                Event::Connection(_) | Event::Lifecycle(_) | Event::Answer(_)
            ),
            "{event:?}"
        );
        let mut remote = connected(Instant::now());
        let before = remote.clone();
        remote.event(&mut event);
        assert_eq!(remote, before);
        assert!(!matches!(event, Event::User(user) if user.came != Came::AsSaid));
    }
}
