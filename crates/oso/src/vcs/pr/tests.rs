//! `gh`'s answers read by shape: a branch's pull request and its checks' counts, one with its
//! checks, threads and conversation, a list, the only address `o` opens, and any bytes read
//! without a panic.
//!
//! Must not depend on: `gh` or the network; the answers are written here.

use proptest::prelude::*;

use super::{
    Bucket, Counts, Review, State, is_plain_link, is_plain_path, is_pull_address, read_checks,
    read_head, read_list, read_threads, read_view, seconds,
};

const HEAD: &str = r#"{"number":12,"title":"Add x","state":"OPEN","isDraft":false,
"reviewDecision":"APPROVED","url":"https://github.com/o/r/pull/12","headRefOid":"abc",
"statusCheckRollup":[
 {"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},
 {"__typename":"CheckRun","status":"COMPLETED","conclusion":"SKIPPED"},
 {"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"},
 {"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""},
 {"__typename":"StatusContext","state":"PENDING"},
 {"__typename":"StatusContext","state":"ERROR"}]}"#;

#[test]
fn a_branch_s_pull_request_reads_with_its_checks_counted() {
    let head = read_head(HEAD.as_bytes()).unwrap();
    assert_eq!(head.number, 12);
    assert_eq!(head.state, State::Open);
    assert_eq!(head.review, Some(Review::Approved));
    assert_eq!(
        head.counts,
        Counts {
            pass: 2,
            fail: 2,
            pending: 2
        }
    );
    assert_eq!(head.head, "abc");
    let draft = HEAD.replace(r#""isDraft":false"#, r#""isDraft":true"#);
    assert_eq!(read_head(draft.as_bytes()).unwrap().state, State::Draft);
    let merged = HEAD.replace("OPEN", "MERGED");
    assert_eq!(read_head(merged.as_bytes()).unwrap().state, State::Merged);
    assert_eq!(read_head(b"{}"), None);
    assert_eq!(
        read_head(br#"{"number":1,"title":"t","state":"ODD"}"#),
        None
    );
}

#[test]
fn a_pull_request_reads_with_its_conversation_checks_and_threads() {
    let view = HEAD.replace(
        r#""headRefOid":"abc","#,
        r#""headRefOid":"abc","author":{"login":"ann"},"baseRefName":"main",
        "headRefName":"feature","body":"Why\nand how","comments":[
        {"author":{"login":"bob"},"authorAssociation":"MEMBER","body":"Looks good"},
        {"author":null,"authorAssociation":"NONE","body":"From a gone account"},
        {"body":5}],"#,
    );
    let view = read_view(view.as_bytes()).unwrap();
    assert_eq!((view.author.as_str(), view.base.as_str()), ("ann", "main"));
    assert_eq!(view.branch, "feature");
    assert_eq!(
        view.comments.len(),
        2,
        "an entry of another shape is left out"
    );
    assert_eq!(view.comments[1].author, "ghost");
    let checks = read_checks(
        br#"[{"name":"build","bucket":"fail","workflow":"CI","link":"https://x/1",
        "startedAt":"2026-11-16T12:00:00Z","completedAt":"2026-11-16T12:03:05Z"},
        {"name":"lint","bucket":"pending","workflow":"CI","startedAt":"2026-11-16T12:00:00Z",
        "completedAt":"0001-01-01T00:00:00Z"},
        {"name":"odd","bucket":"strange"}]"#,
    )
    .unwrap();
    assert_eq!(checks.len(), 2);
    assert_eq!(
        (checks[0].bucket, checks[0].took),
        (Bucket::Fail, Some(185))
    );
    assert_eq!((checks[1].bucket, checks[1].took), (Bucket::Pending, None));
    let threads = br#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"totalCount":3,
    "nodes":[
     {"isResolved":true,"path":"a.rs","line":3,"comments":{"totalCount":1,"nodes":[
       {"author":{"login":"c"},"authorAssociation":"OWNER","body":"done","diffHunk":"@@ x"}]}},
     {"isResolved":false,"path":"b.rs","line":null,"originalLine":9,"comments":{"totalCount":4,
      "nodes":[{"author":{"login":"d"},"authorAssociation":"NONE","body":"why?","diffHunk":"@@"}]}}
    ]}}}}}"#;
    let (threads, more) = read_threads(threads).unwrap();
    assert_eq!(more, 1);
    assert_eq!(
        (threads[0].path.as_str(), threads[0].resolved),
        ("b.rs", false)
    );
    assert_eq!((threads[0].line, threads[0].more), (Some(9), 3));
    assert_eq!(threads[1].comments[0].hunk, "@@ x");
    assert_eq!(read_threads(b"{\"data\":null}"), None);
}

#[test]
fn a_list_reads_its_pull_requests() {
    let list = read_list(
        br#"[{"number":3,"title":"t","headRefName":"b","author":{"login":"e"},"isDraft":true,
        "reviewDecision":"","statusCheckRollup":[],"url":"https://github.com/o/r/pull/3"},
        {"number":"x"}]"#,
    )
    .unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].draft && list[0].review.is_none());
    assert_eq!(read_list(b"{}"), None);
}

#[test]
fn only_the_pull_request_s_own_address_opens() {
    let repo = &crate::git::GhRepo::from_url("https://github.com/o/r", None).unwrap();
    assert!(is_pull_address("https://github.com/o/r/pull/12", repo, 12));
    assert!(is_pull_address("https://GitHub.com/o/r/pull/12", repo, 12));
    for url in [
        "http://github.com/o/r/pull/12",
        "https://github.com/o/r/pull/13",
        "https://github.com/o/other/pull/12",
        "https://evil.example/github.com/o/r/pull/12",
        "https://github.com/o/r/pull/12?x",
        "file:///etc/passwd",
        "",
    ] {
        assert!(!is_pull_address(url, repo, 12), "{url}");
    }
}

#[test]
fn a_thread_s_file_is_visited_only_under_the_root() {
    assert!(is_plain_path("src/a.rs"));
    assert!(is_plain_path("a b/ü.rs"));
    for path in [
        "",
        "/etc/passwd",
        "../x",
        "a/../../x",
        "./a",
        "a\\..\\b",
        "C:x",
        "a:b",
    ] {
        assert!(!is_plain_path(path), "{path:?}");
    }
}

#[test]
fn only_a_plain_https_link_is_copied() {
    assert!(is_plain_link(
        "https://github.com/o/r/actions/runs/1/job/2?pr=3&x=y#step:4"
    ));
    for link in [
        "http://ci.example/1",
        "https://ci.example/1\ncurl x|sh",
        "https://ci.example/1 x",
        "https://ci.example/$(x)",
        "https://ci.example/a;b",
        "https://ci.example/a,b",
        "https://ci.example/'a'",
        "https://",
        "",
    ] {
        assert!(!is_plain_link(link), "{link:?}");
    }
}

#[test]
fn times_read_as_github_writes_them() {
    assert_eq!(seconds("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(seconds("2026-11-16T12:00:00Z"), Some(1_794_830_400));
    assert_eq!(seconds("2000-02-29T00:00:01Z"), Some(951_782_401));
    for odd in [
        "2026-11-16 12:00:00Z",
        "2026-13-01T00:00:00Z",
        "2026-11-16T12:00:00+09:00",
    ] {
        assert_eq!(seconds(odd), None, "{odd}");
    }
}

proptest! {
    #[test]
    fn no_reader_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = read_head(&bytes);
        let _ = read_view(&bytes);
        let _ = read_checks(&bytes);
        let _ = read_threads(&bytes);
        let _ = read_list(&bytes);
    }

    #[test]
    fn no_time_panics(at in "\\PC{0,24}") {
        let _ = seconds(&at);
    }
}
