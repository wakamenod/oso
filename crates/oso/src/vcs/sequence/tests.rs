//! Tests of what is stopped: git's files read in git's order, each that does not parse taken
//! as missing, the head's keys, the questions, escaped, when a step is refused, and the
//! commands, with a property test over any bytes.
//!
//! Must not depend on: git or the file system.

use std::collections::HashMap;

use proptest::prelude::*;

use super::{Doing, Step, Stop, Want, args, done, failed, judge, refusal, title};
use crate::vcs::history::Commit;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn judged(files: &[(&str, &str)], marks: &[&str]) -> Option<Stop> {
    let files: HashMap<String, Vec<u8>> = (files.iter())
        .map(|(name, text)| ((*name).to_owned(), text.as_bytes().to_vec()))
        .collect();
    judge(&|name| files.get(name).cloned(), &|name| {
        marks.contains(&name) || files.contains_key(name)
    })
}

#[test]
fn nothing_there_is_nothing_stopped() {
    assert_eq!(judged(&[], &[]), None);
}

#[test]
fn a_rebase_names_its_branch_onto_progress_and_where_it_stopped() {
    let head = format!("{C}\n");
    let stop = judged(
        &[
            ("rebase-merge/head-name", "refs/heads/main\n"),
            ("rebase-merge/onto", &format!("{A}\n")),
            ("rebase-merge/orig-head", &format!("{B}\n")),
            ("rebase-merge/msgnum", "2\n"),
            ("rebase-merge/end", "5\n"),
            ("REBASE_HEAD", &head),
        ],
        &["rebase-merge"],
    )
    .unwrap();
    assert_eq!(stop.doing, Doing::Rebase);
    assert_eq!(stop.branch.as_deref(), Some(&b"main"[..]));
    assert_eq!(stop.onto.as_deref(), Some(A));
    assert_eq!(stop.orig.as_deref(), Some(B));
    assert_eq!(stop.at.as_deref(), Some(C));
    assert_eq!(stop.progress, Some((2, 5)));
}

#[test]
fn a_detached_rebase_has_no_branch_and_stopped_sha_stands_in() {
    let stopped = format!("{C}\n");
    let stop = judged(
        &[
            ("rebase-merge/head-name", "detached HEAD\n"),
            ("rebase-merge/stopped-sha", &stopped),
        ],
        &["rebase-merge"],
    )
    .unwrap();
    assert_eq!(stop.branch, None);
    assert_eq!(stop.at.as_deref(), Some(C));
    // A short id could name a branch instead: it is as missing.
    let short = judged(
        &[("rebase-merge/stopped-sha", "cccc123\n")],
        &["rebase-merge"],
    );
    assert_eq!(short.unwrap().at, None);
}

#[test]
fn rebase_apply_is_a_rebase_unless_git_am_applies() {
    let files = [("rebase-apply/next", "1"), ("rebase-apply/last", "3")];
    let rebase = judged(&files, &["rebase-apply"]).unwrap();
    assert_eq!(rebase.doing, Doing::Rebase);
    assert_eq!(rebase.progress, Some((1, 3)));
    let am = judged(&files, &["rebase-apply", "rebase-apply/applying"]).unwrap();
    assert_eq!(am.doing, Doing::Am);
}

#[test]
fn git_s_order_puts_a_rebase_before_a_merge_and_a_merge_before_a_pick() {
    let merge = format!("{A}\n");
    let pick = format!("{B}\n");
    let both = [("MERGE_HEAD", merge.as_str()), ("CHERRY_PICK_HEAD", &pick)];
    assert_eq!(judged(&both, &[]).unwrap().doing, Doing::Merge);
    assert_eq!(
        judged(&both, &["rebase-merge"]).unwrap().doing,
        Doing::Rebase
    );
    let picked = judged(&[("CHERRY_PICK_HEAD", &pick)], &[]).unwrap();
    assert_eq!(
        (picked.doing, picked.at.as_deref()),
        (Doing::CherryPick, Some(B))
    );
    let reverted = judged(&[("REVERT_HEAD", &pick)], &[]).unwrap();
    assert_eq!(reverted.doing, Doing::Revert);
}

#[test]
fn a_sequencer_between_picks_says_which_by_its_todo() {
    let todo = |text: &'static str| judged(&[("sequencer/todo", text)], &["sequencer"]);
    assert_eq!(
        todo("# comment\n\nrevert abc subject\n").unwrap().doing,
        Doing::Revert
    );
    assert_eq!(todo("pick abc subject\n").unwrap().doing, Doing::CherryPick);
    assert_eq!(todo("exec make\n"), None);
}

#[test]
fn a_bisect_is_shown_and_takes_no_step() {
    let stop = judged(&[("BISECT_LOG", "")], &[]).unwrap();
    assert_eq!(stop.doing, Doing::Bisect);
    for step in [Step::Continue, Step::Skip, Step::Abort] {
        assert!(Doing::Bisect.refuses(step).is_some());
    }
    assert!(Doing::Merge.refuses(Step::Skip).is_some());
    assert_eq!(Doing::Merge.refuses(Step::Abort), None);
    assert_eq!(Doing::Bisect.keys(), None);
    assert_eq!(
        Doing::Merge.keys().as_deref(),
        Some("C-c C-c continues, C-c C-k aborts")
    );
    assert_eq!(
        Doing::Rebase.keys().as_deref(),
        Some("C-c C-c continues, C-c C-s skips, C-c C-k aborts")
    );
}

/// A rebase of `main` stopped at `at`, from `orig`, 2 of 5.
fn rebase(at: Option<&str>, orig: &str) -> Stop {
    Stop {
        doing: Doing::Rebase,
        branch: Some(b"main".to_vec()),
        onto: Some(A.into()),
        at: at.map(str::to_owned),
        progress: Some((2, 5)),
        orig: Some(orig.into()),
        commits: Vec::new(),
    }
}

#[test]
fn a_skip_or_an_abort_runs_only_over_the_stop_it_was_asked_over() {
    let asked = rebase(None, B);
    for step in [Step::Skip, Step::Abort] {
        let want = Want::new(step, &asked);
        assert_eq!(refusal(Some(&asked), &want), None);
        // With no commit to tell them apart, the branch, the start or the step still do.
        let mut other = rebase(None, C);
        assert!(refusal(Some(&other), &want).unwrap().contains("moved on"));
        other = rebase(None, B);
        other.progress = Some((3, 5));
        assert!(refusal(Some(&other), &want).unwrap().contains("moved on"));
        other = rebase(None, B);
        other.branch = Some(b"feature".to_vec());
        assert!(refusal(Some(&other), &want).is_some());
    }
    // A continue goes on with whatever rebase is stopped; nothing or a merge refuses it.
    let want = Want::new(Step::Continue, &asked);
    assert_eq!(refusal(Some(&rebase(Some(C), C)), &want), None);
    assert!(
        refusal(None, &want)
            .unwrap()
            .starts_with("No rebase is stopped")
    );
    let merge = Stop {
        doing: Doing::Merge,
        ..rebase(None, B)
    };
    assert!(
        refusal(Some(&merge), &want)
            .unwrap()
            .contains("merge is stopped")
    );
    let skip = Want::new(Step::Skip, &merge);
    assert!(
        refusal(Some(&merge), &skip)
            .unwrap()
            .starts_with("A merge has no skip")
    );
}

#[test]
fn the_questions_escape_what_git_gave() {
    let mut stop = rebase(Some(C), B);
    stop.branch = Some("ma\u{202e}in".as_bytes().to_vec());
    stop.commits = vec![Commit {
        id: C.into(),
        short: "ccccccc".into(),
        subject: "a\u{2066}b\nc".as_bytes().to_vec(),
    }];
    for question in [stop.abort_question(), stop.skip_question()] {
        assert!(
            !question.contains(['\u{202e}', '\u{2066}', '\n']),
            "{question:?}"
        );
    }
}

#[test]
fn ids_that_are_not_hex_are_as_missing() {
    let stop = judged(
        &[
            ("MERGE_HEAD", "--upload-pack=evil\n"),
            ("rebase-merge/onto", "zz"),
        ],
        &[],
    )
    .unwrap();
    assert_eq!((stop.doing, stop.at), (Doing::Merge, None));
}

#[test]
fn a_count_past_its_end_is_dropped() {
    let stop = judged(
        &[("rebase-merge/msgnum", "9"), ("rebase-merge/end", "3")],
        &["rebase-merge"],
    )
    .unwrap();
    assert_eq!(stop.progress, None);
}

#[test]
fn the_questions_name_what_goes() {
    let mut stop = judged(
        &[
            ("rebase-merge/head-name", "refs/heads/main"),
            ("rebase-merge/orig-head", B),
            ("rebase-merge/msgnum", "2"),
            ("rebase-merge/end", "5"),
            ("REBASE_HEAD", C),
        ],
        &["rebase-merge"],
    )
    .unwrap();
    stop.commits = vec![
        Commit {
            id: B.into(),
            short: "bbbbbbb".into(),
            subject: b"Before".to_vec(),
        },
        Commit {
            id: C.into(),
            short: "ccccccc".into(),
            subject: b"Stopped here".to_vec(),
        },
    ];
    let abort = stop.abort_question();
    assert!(abort.contains("rebase of main"), "{abort}");
    assert!(abort.contains("2 of 5"), "{abort}");
    assert!(
        abort.contains("main goes back to bbbbbbb Before"),
        "{abort}"
    );
    let skip = stop.skip_question();
    assert!(skip.starts_with("Skip ccccccc Stopped here?"), "{skip}");
    let merge = judged(&[("MERGE_HEAD", A)], &[]).unwrap();
    assert_eq!(merge.at.as_deref(), Some(A));
    assert!(
        merge
            .abort_question()
            .contains(&format!("merge of {}", &A[..7]))
    );
}

#[test]
fn each_step_is_git_s_own_flag() {
    assert_eq!(
        args(Doing::CherryPick, Step::Continue),
        ["cherry-pick", "--continue"]
    );
    assert_eq!(args(Doing::Am, Step::Skip), ["am", "--skip"]);
    assert_eq!(title(Doing::Rebase, Step::Abort), "git rebase --abort");
    assert_eq!(done(Doing::Am, Step::Abort), "Aborted the git am");
}

#[test]
fn a_failure_that_left_a_conflict_says_what_comes_next() {
    let said = failed("git pull --rebase", "CONFLICT (content): x\n", true);
    assert!(said.contains("stopped on a conflict"), "{said}");
    assert!(said.contains("C-c C-c continues"), "{said}");
    let said = failed("git rebase --continue", "  git rebase --skip\n", true);
    assert!(said.contains("C-c C-s skips"), "{said}");
    // A hook's own words do not count.
    let said = failed("git rebase --continue", "lint: CONFLICT here\n", true);
    assert!(!said.contains("stopped on a conflict"), "{said}");
    let said = failed("git pull --rebase", "fatal: no such remote\n", false);
    assert!(said.contains("no such remote"), "{said}");
}

proptest! {
    #[test]
    fn any_bytes_never_panic_and_ids_stay_hex(
        texts in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..80), 8),
        marks in prop::collection::vec(any::<bool>(), 5),
    ) {
        let names = [
            "rebase-merge/head-name", "rebase-merge/onto", "rebase-merge/msgnum",
            "rebase-merge/end", "REBASE_HEAD", "MERGE_HEAD", "CHERRY_PICK_HEAD",
            "sequencer/todo",
        ];
        let dirs = ["rebase-merge", "rebase-apply", "rebase-apply/applying", "sequencer",
            "BISECT_LOG"];
        let files: HashMap<&str, Vec<u8>> = names.iter().copied().zip(texts).collect();
        let on: Vec<&str> = dirs.iter().zip(&marks).filter(|(_, on)| **on).map(|(d, _)| *d)
            .collect();
        let stop = judge(&|name| files.get(name).cloned(), &|name| on.contains(&name));
        if let Some(stop) = stop {
            for id in stop.ids() {
                prop_assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
                prop_assert!(matches!(id.len(), 40 | 64));
            }
            let _ = (stop.abort_question(), stop.skip_question());
        }
    }
}
