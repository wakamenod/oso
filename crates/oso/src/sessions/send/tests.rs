//! What a send pastes and when it is refused.
//!
//! Must not depend on: `app` or `dispatch`; these tests drive the pure functions,
//! with no window and no terminal.

use std::path::{Path, PathBuf};

use oso_core::editor::{SendExcerpt, SendPart};

use super::{MAX_BYTES, Standing, TooLarge, compose, fence, place, refusal};
use crate::sessions::state::{Reason, State};

const DIRECTORY: &str = "/work/repo";

fn excerpt(path: Option<&str>, first: usize, last: usize, text: Option<&str>) -> SendExcerpt {
    SendExcerpt {
        path: path.map(PathBuf::from),
        buffer: "notes.md".to_owned(),
        first,
        last,
        text: text.map(str::to_owned),
    }
}

fn sent(parts: &[SendPart]) -> String {
    compose(parts, Path::new(DIRECTORY)).unwrap()
}

#[test]
fn a_fence_is_longer_than_any_run_of_backticks() {
    assert_eq!(fence("plain"), "```");
    assert_eq!(fence("a `b` c"), "```");
    assert_eq!(fence("``"), "```");
    assert_eq!(fence("```"), "````");
    assert_eq!(fence("x ```` y ``"), "`````");
    assert_eq!(fence("`````"), "``````");
}

#[test]
fn a_region_is_its_place_and_its_text_in_a_fence() {
    let part = SendPart::Excerpt(excerpt(
        Some("/work/repo/src/main.rs"),
        10,
        12,
        Some("fn main() {\n    run();\n}"),
    ));
    assert_eq!(
        sent(&[part]),
        "src/main.rs:10-12\n```\nfn main() {\n    run();\n}\n```"
    );
}

#[test]
fn a_fence_grows_past_the_backticks_the_text_holds() {
    let part = SendPart::Excerpt(excerpt(
        Some("/work/repo/README.md"),
        3,
        3,
        Some("```rust\nx\n```\n"),
    ));
    assert_eq!(sent(&[part]), "README.md:3\n````\n```rust\nx\n```\n````");
}

#[test]
fn the_place_alone_is_a_path_and_lines() {
    let part = SendPart::Excerpt(excerpt(Some("/work/repo/src/app.rs"), 10, 20, None));
    assert_eq!(sent(&[part]), "src/app.rs:10-20");
}

#[test]
fn a_path_outside_the_directory_stays_absolute() {
    let outside = excerpt(Some("/elsewhere/lib.rs"), 1, 2, None);
    assert_eq!(
        place(&outside, Path::new(DIRECTORY)),
        "/elsewhere/lib.rs:1-2"
    );
    // A sibling whose name starts as the directory's is outside it.
    let sibling = excerpt(Some("/work/repo-2/lib.rs"), 4, 4, None);
    assert_eq!(
        place(&sibling, Path::new(DIRECTORY)),
        "/work/repo-2/lib.rs:4"
    );
}

#[test]
fn a_buffer_with_no_file_is_named_as_a_buffer() {
    let part = SendPart::Excerpt(excerpt(None, 2, 5, Some("text\n")));
    assert_eq!(sent(&[part]), "buffer notes.md, lines 2-5\n```\ntext\n```");
}

#[test]
fn control_bidi_and_invisible_characters_in_a_path_are_escaped() {
    let path = "/work/repo/a\nb\u{202e}c\u{200b}d\te.rs";
    let shown = place(&excerpt(Some(path), 1, 1, None), Path::new(DIRECTORY));
    assert_eq!(shown, "a\\u{a}b\\u{202e}c\\u{200b}d\\u{9}e.rs:1");
    let mut named = excerpt(None, 1, 1, None);
    named.buffer = "*x\u{1b}[31m*".to_owned();
    assert_eq!(
        place(&named, Path::new(DIRECTORY)),
        "buffer *x\\u{1b}[31m*, lines 1"
    );
}

#[test]
fn parts_are_joined_by_a_blank_line_and_a_note_goes_as_written() {
    let parts = [
        SendPart::Excerpt(excerpt(Some("/work/repo/a.rs"), 1, 1, Some("one\n"))),
        SendPart::Excerpt(excerpt(Some("/work/repo/b.rs"), 7, 8, None)),
        SendPart::Note("Why does this fail?".to_owned()),
    ];
    assert_eq!(
        sent(&parts),
        "a.rs:1\n```\none\n```\n\nb.rs:7-8\n\nWhy does this fail?"
    );
}

#[test]
fn a_send_is_at_most_256_kib() {
    let fits = SendPart::Note("x".repeat(MAX_BYTES));
    assert_eq!(sent(&[fits]).len(), MAX_BYTES);
    let over = SendPart::Note("x".repeat(MAX_BYTES + 1));
    assert_eq!(
        compose(&[over], Path::new(DIRECTORY)),
        Err(TooLarge {
            bytes: MAX_BYTES + 1
        })
    );
    // The fence and the place count too.
    let text = "y".repeat(MAX_BYTES - 4);
    let region = SendPart::Excerpt(excerpt(Some("/work/repo/a"), 1, 1, Some(&text)));
    assert!(compose(&[region], Path::new(DIRECTORY)).is_err());
}

fn standing(state: Option<State>) -> Standing {
    Standing {
        running: true,
        state,
        echo_off: false,
        held: false,
    }
}

#[test]
fn a_session_that_asks_is_refused_with_what_it_asks() {
    let cases = [
        (
            Reason::Permission,
            "api is asking for permission; answer it first",
        ),
        (
            Reason::Question,
            "api is asking a question; answer it first",
        ),
        (
            Reason::Plan,
            "api is asking to approve a plan; answer it first",
        ),
        (
            Reason::Elicitation,
            "api has an MCP server asking for input; answer it first",
        ),
        (Reason::Dialog, "api shows a dialog; answer it first"),
    ];
    for (reason, said) in cases {
        let refused = refusal("api", standing(Some(State::NeedsInput(reason))));
        assert_eq!(refused.as_deref(), Some(said));
    }
}

#[test]
fn a_session_that_works_or_waits_at_its_prompt_takes_a_paste() {
    for state in [
        State::Working,
        State::ReadyForReview,
        State::Done,
        State::Idle,
    ] {
        assert_eq!(refusal("api", standing(Some(state))), None, "{state:?}");
    }
}

#[test]
fn a_session_not_running_starting_or_at_a_password_is_refused() {
    let stopped = Standing {
        running: false,
        ..standing(Some(State::Idle))
    };
    assert_eq!(
        refusal("api", stopped).as_deref(),
        Some("api is not running: session-resume first")
    );
    for state in [State::Stopped, State::Failed] {
        assert!(refusal("api", standing(Some(state))).is_some());
    }
    assert_eq!(
        refusal("api", standing(None)).as_deref(),
        Some("api is starting; send again in a moment")
    );
    let password = Standing {
        echo_off: true,
        ..standing(Some(State::Idle))
    };
    assert_eq!(
        refusal("api", password).as_deref(),
        Some("api waits for a password; nothing is pasted there")
    );
}

#[test]
fn a_full_screen_view_is_refused_but_a_dialog_behind_it_says_what_it_asks() {
    let view = Standing {
        held: true,
        ..standing(Some(State::Working))
    };
    assert_eq!(
        refusal("api", view).as_deref(),
        Some("api shows a full-screen view; close it first")
    );
    let behind = Standing {
        held: true,
        ..standing(Some(State::NeedsInput(Reason::Permission)))
    };
    assert_eq!(
        refusal("api", behind).as_deref(),
        Some("api is asking for permission; answer it first")
    );
}
