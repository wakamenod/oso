//! Pull requests' texts: the head line and its stale mark, a pull request's rows and what each
//! names, escaped and cut text from GitHub, and a list.
//!
//! Must not depend on: `gh`; the values are made here.

use proptest::prelude::*;

use super::super::{Bucket, Check, Comment, Counts, Listed, PrHead, PrView, Review, State, Thread};
use super::{PrRow, head_parts, list_text, view_text};

fn head() -> PrHead {
    PrHead {
        number: 12,
        title: "Add x".into(),
        state: State::Open,
        review: Some(Review::Approved),
        counts: Counts {
            pass: 3,
            fail: 1,
            pending: 0,
        },
        url: "https://github.com/o/r/pull/12".into(),
        head: "abcdef0123".into(),
    }
}

fn comment(body: &str) -> Comment {
    Comment {
        author: "bob".into(),
        association: "MEMBER".into(),
        body: body.into(),
        hunk: "@@ -1 +1 @@\n-old\n+new".into(),
    }
}

fn view() -> PrView {
    PrView {
        head: head(),
        author: "ann".into(),
        base: "main".into(),
        branch: "feature".into(),
        body: "Why\nHow".into(),
        comments: vec![comment("Thanks")],
        checks: vec![Check {
            name: "build".into(),
            bucket: Bucket::Fail,
            workflow: "CI".into(),
            link: "https://x/1".into(),
            took: Some(65),
        }],
        threads: vec![Thread {
            path: "src/a.rs".into(),
            line: Some(7),
            resolved: false,
            comments: vec![comment("Why this?")],
            more: 2,
        }],
        more_threads: 0,
    }
}

fn joined(parts: &[(String, Option<oso_core::syntax::Highlight>)]) -> String {
    parts.iter().map(|(text, _)| text.as_str()).collect()
}

#[test]
fn the_head_line_says_the_state_reviews_checks_and_a_stale_head() {
    let line = joined(&head_parts(&head(), Some("abcdef0")));
    assert_eq!(line, "#12 Add x  open, approved  ✓ 3 ✗ 1 ● 0");
    let stale = joined(&head_parts(&head(), Some("1234567")));
    assert!(stale.ends_with("  stale"), "{stale}");
}

#[test]
fn a_pull_request_s_lines_name_its_checks_and_threads() {
    let laid = view_text(&view());
    let text = laid.text.text().to_string();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(laid.built.rows.len(), lines.len());
    assert_eq!(lines[0], "#12 Add x");
    assert!(lines[laid.built.checks_line].starts_with("Checks (1)"));
    let check = lines
        .iter()
        .position(|line| line.contains("✗ build"))
        .unwrap();
    assert_eq!(
        laid.built.row(check),
        &PrRow::Check {
            link: "https://x/1".into()
        }
    );
    let thread = lines
        .iter()
        .position(|line| line.contains("src/a.rs:7"))
        .unwrap();
    let place = PrRow::Thread {
        path: "src/a.rs".into(),
        line: Some(7),
    };
    assert_eq!(laid.built.row(thread), &place);
    assert!(lines[thread].ends_with("unresolved"));
    // The thread's diff, comment and count of more name its place too.
    assert_eq!(laid.built.row(thread + 1), &place);
    assert!(text.contains("    │ +new") && text.contains("and 2 more comments"));
    assert_eq!(
        laid.built.pull,
        Some((12, "https://github.com/o/r/pull/12".into()))
    );
}

#[test]
fn github_s_words_cannot_draw_rows_of_their_own() {
    let mut view = view();
    view.head.title = "a\u{202e}b\rc".into();
    view.comments = vec![comment(&"line\n".repeat(100))];
    view.comments[0].author = "x\ny".into();
    let laid = view_text(&view);
    let text = laid.text.text().to_string();
    assert!(text.starts_with("#12 a\\u{202e}b\\u{d}c\n"));
    assert!(text.contains("@x\\u{a}y"));
    assert!(text.contains("and 40 more lines"));
    assert_eq!(laid.built.rows.len(), text.lines().count());
}

#[test]
fn a_list_names_each_pull_request() {
    let items = [Listed {
        number: 3,
        title: "t".into(),
        branch: "b".into(),
        author: "e".into(),
        draft: true,
        review: None,
        counts: Counts::default(),
        url: "https://github.com/o/r/pull/3".into(),
    }];
    let laid = list_text("github.com/o/r", &items);
    let text = laid.text.text().to_string();
    assert!(text.starts_with("Open pull requests of github.com/o/r (1)\n#3     draft  t"));
    assert_eq!(
        laid.built.row(1),
        &PrRow::Pull {
            number: 3,
            url: "https://github.com/o/r/pull/3".into()
        }
    );
}

proptest! {
    #[test]
    fn any_words_keep_one_row_per_line(title in "\\PC{0,40}", body in any::<String>()) {
        let mut view = view();
        view.head.title = title;
        view.body = body.clone();
        view.comments = vec![comment(&body)];
        let laid = view_text(&view);
        let text = laid.text.text().to_string();
        prop_assert_eq!(laid.built.rows.len(), text.lines().count());
    }
}
