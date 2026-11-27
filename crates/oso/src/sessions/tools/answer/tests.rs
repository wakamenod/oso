//! `session_answer`'s rules (ADR 0186): who may answer which request, and what is said.
//!
//! Must not depend on: the app; plain values stand in for what it finds.

use super::{
    Answering, MAX_REASON, ONLY_ORCHESTRATOR, PERSON_ANSWERING, Shown, Verdict, denial, denied_row,
    judge, plain_reason, said,
};
use crate::sessions::native::asks::{Answerer, RequestId};
use crate::sessions::native::stand_in::PersonOnly;

fn r(n: &str) -> RequestId {
    RequestId::parse(n).unwrap()
}

/// The orchestrator allows the Edit its worker shows now, as `r3`, which the rules let it.
fn allowed() -> Answering<'static> {
    Answering {
        orchestrator: true,
        of_its_space: true,
        own: false,
        native: true,
        request: Some(r("r3")),
        tool: "Edit",
        verdict: Verdict::Allow,
        reason: None,
        shown: Some(Shown {
            id: r("r3"),
            tool: "Edit",
            card: false,
            verdict: Ok(()),
        }),
        answered: None,
        person_answering: false,
    }
}

fn refused(call: &Answering<'_>, says: &str) {
    let why = judge(call).expect_err("refused");
    assert!(why.contains(says), "{why:?} says no {says:?}");
}

#[test]
fn the_orchestrator_answers_its_workers_request_shown_now() {
    assert_eq!(judge(&allowed()), Ok(()));
    let deny = Answering {
        verdict: Verdict::Deny,
        reason: Some("run the tests first"),
        ..allowed()
    };
    assert_eq!(judge(&deny), Ok(()));
}

#[test]
fn a_worker_or_a_former_orchestrator_is_refused() {
    let worker = Answering {
        orchestrator: false,
        ..allowed()
    };
    assert_eq!(judge(&worker), Err(ONLY_ORCHESTRATOR.to_owned()));
    // Whatever else holds: a worker of the same space, denying.
    let denying = Answering {
        verdict: Verdict::Deny,
        ..worker
    };
    assert_eq!(judge(&denying), Err(ONLY_ORCHESTRATOR.to_owned()));
}

#[test]
fn another_spaces_orchestrator_is_refused() {
    let other = Answering {
        of_its_space: false,
        ..allowed()
    };
    refused(&other, "not of your space");
}

#[test]
fn its_own_request_is_refused() {
    let own = Answering {
        own: true,
        ..allowed()
    };
    refused(&own, "your own");
    let denying = Answering {
        verdict: Verdict::Deny,
        ..own
    };
    refused(&denying, "your own");
}

#[test]
fn a_terminal_sessions_request_is_refused() {
    let terminal = Answering {
        native: false,
        shown: None,
        ..allowed()
    };
    refused(&terminal, "native");
}

#[test]
fn a_stale_or_wrong_id_is_refused() {
    // Not one of oso's ids at all.
    let unreadable = Answering {
        request: None,
        ..allowed()
    };
    refused(&unreadable, "request.id");
    // An id the session does not show now, answered by no one: gone, or never shown.
    let stale = Answering {
        request: Some(r("r2")),
        ..allowed()
    };
    refused(&stale, "no longer waits");
    // Nothing shown at all.
    let none = Answering {
        shown: None,
        ..allowed()
    };
    refused(&none, "no longer waits");
}

#[test]
fn a_tool_that_is_not_the_requests_is_refused() {
    let mismatch = Answering {
        tool: "Bash",
        ..allowed()
    };
    refused(&mismatch, "is of \"Edit\"");
    // Case counts: a tool's name is the CLI's.
    let case = Answering {
        tool: "edit",
        ..allowed()
    };
    refused(&case, "is of \"Edit\"");
}

#[test]
fn a_request_the_person_answered_first_is_refused_saying_so() {
    let first = Answering {
        request: Some(r("r2")),
        answered: Some(Answerer::Person),
        ..allowed()
    };
    refused(&first, "the person answered it first");
    // Its session shows nothing now.
    let alone = Answering {
        shown: None,
        ..first
    };
    refused(&alone, "the person answered it first");
}

#[test]
fn a_second_answer_is_refused() {
    let again = Answering {
        request: Some(r("r2")),
        answered: Some(Answerer::StandIn),
        shown: None,
        ..allowed()
    };
    refused(&again, "answered already");
    let deny_again = Answering {
        verdict: Verdict::Deny,
        ..again
    };
    refused(&deny_again, "answered already");
}

#[test]
fn a_request_the_person_is_answering_is_refused() {
    let answering = Answering {
        person_answering: true,
        ..allowed()
    };
    assert_eq!(judge(&answering), Err(PERSON_ANSWERING.to_owned()));
    let denying = Answering {
        verdict: Verdict::Deny,
        ..answering
    };
    assert_eq!(judge(&denying), Err(PERSON_ANSWERING.to_owned()));
}

#[test]
fn a_card_is_neither_allowed_nor_denied() {
    let card = Answering {
        tool: "ExitPlanMode",
        shown: Some(Shown {
            id: r("r3"),
            tool: "ExitPlanMode",
            card: true,
            verdict: Err(PersonOnly::Card),
        }),
        ..allowed()
    };
    refused(&card, "questions and plans are the person's");
    let deny = Answering {
        verdict: Verdict::Deny,
        ..card
    };
    refused(&deny, "questions and plans are the person's");
}

#[test]
fn what_stays_the_persons_is_denied_but_never_allowed() {
    for why in [
        PersonOnly::Off,
        PersonOnly::Tool,
        PersonOnly::NotShown,
        PersonOnly::Secret,
        PersonOnly::Outside,
        PersonOnly::Denied,
    ] {
        let kept = Answering {
            shown: Some(Shown {
                verdict: Err(why),
                ..allowed().shown.unwrap()
            }),
            ..allowed()
        };
        assert_eq!(judge(&kept), Err(why.says().to_owned()), "{why:?}");
        let deny = Answering {
            verdict: Verdict::Deny,
            ..kept
        };
        assert_eq!(judge(&deny), Ok(()), "{why:?}");
    }
}

#[test]
fn config_off_refuses_an_allow() {
    let off = Answering {
        shown: Some(Shown {
            verdict: Err(PersonOnly::Off),
            ..allowed().shown.unwrap()
        }),
        ..allowed()
    };
    refused(&off, "orchestrator-answers is empty");
}

#[test]
fn a_reason_is_a_denys_alone_and_plain() {
    let with_allow = Answering {
        reason: Some("fine"),
        ..allowed()
    };
    refused(&with_allow, "a deny's alone");
    for bad in [
        "a\nb",
        "a\u{202e}b",
        "a\u{200b}b",
        "a\u{7}b",
        "",
        &"x".repeat(MAX_REASON + 1),
    ] {
        assert!(!plain_reason(bad), "{bad:?}");
        let deny = Answering {
            verdict: Verdict::Deny,
            reason: Some(bad),
            ..allowed()
        };
        refused(&deny, "plain characters");
    }
    for good in ["run the tests first", "日本語で", &"é".repeat(MAX_REASON)] {
        assert!(plain_reason(good), "{good:?}");
    }
}

#[test]
fn the_worker_is_told_who_denied_and_why() {
    let plain = denial("fix-login", None);
    assert!(plain.contains("fix-login"), "{plain}");
    assert!(plain.contains("orchestrator"), "{plain}");
    let why = denial("fix-login", Some("run the tests first"));
    assert!(why.contains("fix-login"), "{why}");
    assert!(why.ends_with("run the tests first"), "{why}");
    assert_eq!(denied_row("fix-login"), "by fix-login");
    assert_eq!(
        said("fix-login", Verdict::Allow, "Bash", "api"),
        "fix-login allowed Bash in api"
    );
    assert_eq!(
        said("fix-login", Verdict::Deny, "Edit", "api"),
        "fix-login denied Edit in api"
    );
}

#[test]
fn an_answer_is_allow_or_deny() {
    assert_eq!(Verdict::parse("allow"), Some(Verdict::Allow));
    assert_eq!(Verdict::parse("deny"), Some(Verdict::Deny));
    for other in ["Allow", "yes", "", "allow "] {
        assert_eq!(Verdict::parse(other), None, "{other:?}");
    }
}
