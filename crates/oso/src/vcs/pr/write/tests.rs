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
        branch: "feature".into(),
        cross: false,
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
            description: String::new(),
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
    assert_eq!(laid.built.row(check), PrRow::Check(0));
    assert_eq!(laid.built.links, ["https://x/1"]);
    let thread = lines
        .iter()
        .position(|line| line.contains("src/a.rs:7"))
        .unwrap();
    let place = PrRow::Thread(0);
    assert_eq!(laid.built.row(thread), place);
    assert_eq!(laid.built.threads, [("src/a.rs".to_owned(), Some(7))]);
    assert!(lines[thread].ends_with("unresolved"));
    // The thread's diff, comment and count of more name its place too.
    assert_eq!(laid.built.row(thread + 1), place);
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
        cross: false,
    }];
    let laid = list_text("github.com/o/r", &items);
    let text = laid.text.text().to_string();
    assert!(text.starts_with("Open pull requests of github.com/o/r (1)\n#3     draft  t"));
    assert_eq!(laid.built.row(1), PrRow::Pull(0));
    assert_eq!(
        laid.built.pulls,
        [(3, "https://github.com/o/r/pull/3".to_owned())]
    );
}

#[test]
fn a_thread_s_path_is_kept_once_and_a_text_is_cut_in_rows() {
    let mut view = view();
    let path = "d/".repeat(2000);
    let thread = Thread {
        path,
        line: Some(1),
        resolved: false,
        comments: vec![comment(&"x\n".repeat(61)); 50],
        more: 0,
    };
    view.threads = vec![thread; 100];
    let laid = view_text(&view);
    let built = &laid.built;
    // One copy of each path, whatever the rows that name it.
    assert_eq!(built.threads.len(), 100);
    assert!(built.rows.len() <= super::MAX_ROWS, "{}", built.rows.len());
    let text = laid.text.text().to_string();
    assert!(text.ends_with("The rest is cut: the address shows it all\n"));
    assert_eq!(built.rows.len(), text.lines().count());
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

#[test]
fn an_issue_s_lines_and_a_list_s_name_what_they_show() {
    use super::super::issue::{Issue, IssueListed};
    use super::{issue_list_text, issue_text};
    let listed = [IssueListed {
        number: 45,
        title: "t\u{202e}".into(),
        labels: vec!["bug".into()],
        author: "ann".into(),
        updated: "2026-11-16T12:00:00Z".into(),
        url: "https://github.com/o/r/issues/45".into(),
    }];
    let laid = issue_list_text("github.com/o/r", &listed);
    let text = laid.text.text().to_string();
    assert!(
        text.contains("#45    t\\u{202e}  [bug]  @ann  2026-11-16"),
        "{text}"
    );
    assert_eq!(laid.built.row(1), PrRow::Pull(0));
    assert_eq!(
        laid.built.pulls,
        [(45, "https://github.com/o/r/issues/45".to_owned())]
    );
    let issue = Issue {
        number: 45,
        title: "t".into(),
        body: "b".into(),
        state: "OPEN".into(),
        author: "ann".into(),
        labels: Vec::new(),
        url: "https://github.com/o/r/issues/45".into(),
        comments: vec![comment("Seen")],
    };
    let laid = issue_text(&issue);
    let text = laid.text.text().to_string();
    assert!(text.starts_with("#45 t\nState:    open"), "{text}");
    assert_eq!(laid.built.rows.len(), text.lines().count());
    assert_eq!(
        laid.built.pull,
        Some((45, "https://github.com/o/r/issues/45".into()))
    );
}

#[test]
fn a_worktree_s_pull_request_is_one_escaped_line() {
    let item = Listed {
        number: 12,
        title: "t".into(),
        branch: "b\u{202e}".into(),
        author: "e".into(),
        draft: false,
        review: Some(Review::Approved),
        counts: Counts {
            pass: 3,
            fail: 0,
            pending: 1,
        },
        url: String::new(),
        cross: false,
    };
    assert_eq!(
        super::pull_summary(&item),
        "PR #12 open, approved ✓ 3 ✗ 0 ● 1"
    );
    let draft = Listed {
        draft: true,
        review: None,
        counts: Counts::default(),
        ..item
    };
    assert_eq!(super::pull_summary(&draft), "PR #12 draft");
}
