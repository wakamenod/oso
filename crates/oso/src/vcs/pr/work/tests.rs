//! A repository's `gh` thread: which reads are answered, the repository a remote names, and
//! its runs against a stand-in `gh` that writes what it was asked.
//!
//! Must not depend on: the network or a real `gh`; a script answers in its place.

use super::{PrRequest, latest};
use crate::git::GhRepo;

#[test]
fn of_each_read_only_the_last_asked_is_answered() {
    let batch = vec![
        PrRequest::Head { generation: 1 },
        PrRequest::View {
            generation: 2,
            number: None,
        },
        PrRequest::Head { generation: 3 },
        PrRequest::Address,
        PrRequest::View {
            generation: 4,
            number: Some(9),
        },
        PrRequest::Address,
    ];
    let kept: Vec<String> = latest(batch).iter().map(|r| format!("{r:?}")).collect();
    assert_eq!(kept.len(), 4, "{kept:?}");
    assert!(kept[1].contains("Address"), "{kept:?}");
    assert!(
        kept[0].contains("Head") && kept[0].contains('3'),
        "{kept:?}"
    );
    assert!(
        kept[2].contains("View") && kept[2].contains('9'),
        "{kept:?}"
    );
    assert!(kept[3].contains("Address"), "{kept:?}");
}

#[test]
fn a_repository_s_parts() {
    let repo = GhRepo::from_url("https://github.com/o/r.git", None).unwrap();
    assert_eq!(repo.parts(), ("github.com", "o", "r"));
    assert_eq!(repo.pull_address(3), "https://github.com/o/r/pull/3");
}

#[cfg(unix)]
mod with_gh {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use oso_platform::process::Executable;

    use super::super::{Found, Target, head, target_of, view};
    use crate::git::{Gh, Git, TrustedRoot};
    use crate::review::tests::Repo as Scratch;

    /// A repository on `main` whose `origin` is on github.com, and a `gh` that logs each run's
    /// arguments and answers from files named for the command it was given.
    struct Bench {
        repo: Scratch,
        hooks: PathBuf,
        answers: PathBuf,
    }

    impl Bench {
        fn new(name: &str) -> Self {
            let repo = Scratch::new(name);
            let hooks = repo.dir.join("hooks");
            let answers = repo.dir.join("answers");
            for path in [&hooks, &answers] {
                fs::create_dir(path).unwrap();
            }
            repo.git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
            Self {
                repo,
                hooks,
                answers,
            }
        }

        /// `gh pr view`, `gh pr checks`, `gh api` and `gh pr list` write `NAME.json` and exit
        /// with `NAME.code`, saying `NAME.err`.
        fn answer(&self, name: &str, json: &str, code: u8, err: &str) {
            fs::write(self.answers.join(format!("{name}.json")), json).unwrap();
            fs::write(self.answers.join(format!("{name}.code")), code.to_string()).unwrap();
            fs::write(self.answers.join(format!("{name}.err")), err).unwrap();
        }

        fn found(&self) -> Found {
            let script = self.answers.join("gh");
            let a = self.answers.display();
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{a}/asked'\nname=\"$1-$2\"\n\
                     cat \"{a}/$name.json\"\ncat \"{a}/$name.err\" >&2\n\
                     exit $(cat \"{a}/$name.code\")\n"
                ),
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            Found {
                git: Git::new(&self.repo.root, &self.hooks).unwrap(),
                gh: Gh::at(Executable::absolute(script).unwrap()),
                cwd: self.hooks.clone(),
                root: TrustedRoot::for_tests(self.repo.root.clone()),
            }
        }

        fn asked(&self) -> String {
            fs::read_to_string(self.answers.join("asked")).unwrap_or_default()
        }
    }

    fn repo(found: &Found) -> Target {
        target_of(&found.git).unwrap()
    }

    const HEAD: &str = r#"{"number":12,"title":"Add x","state":"OPEN","isDraft":false,
    "reviewDecision":"","url":"https://github.com/o/r/pull/12","headRefOid":"abc",
    "statusCheckRollup":[],"author":{"login":"ann"},"baseRefName":"main",
    "headRefName":"main","body":"","comments":[]}"#;

    #[test]
    fn a_branch_named_as_a_number_gets_no_other_branch_s_pull_request() {
        let bench = Bench::new("pr-number-branch");
        bench.repo.git(&["checkout", "-q", "-b", "42"]);
        // `gh pr view 42` answers pull request #42, whose head is `main`.
        bench.answer("pr-view", HEAD, 0, "");
        let found = bench.found();
        assert_eq!(head(&found, &repo(&found)), Ok(None));
        let refused = view(&found, &repo(&found), None).err().unwrap_or_default();
        assert!(refused.contains("42 has no pull request"), "{refused}");
    }

    #[test]
    fn the_branch_s_pull_request_is_asked_of_its_remote_s_repository_outside_it() {
        let bench = Bench::new("pr-head");
        bench.answer("pr-view", HEAD, 0, "");
        let found = bench.found();
        let head = head(&found, &repo(&found)).unwrap().unwrap();
        assert_eq!(head.number, 12);
        let asked = bench.asked();
        assert!(
            asked.starts_with("pr view main --repo github.com/o/r --json "),
            "{asked}"
        );
    }

    #[test]
    fn a_branch_with_no_pull_request_has_none() {
        let bench = Bench::new("pr-none");
        bench.answer(
            "pr-view",
            "",
            1,
            "no pull requests found for branch \"main\"",
        );
        let found = bench.found();
        assert_eq!(head(&found, &repo(&found)), Ok(None));
        let refused = view(&found, &repo(&found), None).err().unwrap_or_default();
        assert!(refused.contains("main has no pull request"), "{refused}");
    }

    #[test]
    fn a_pull_request_reads_its_checks_pending_and_its_threads_by_variables() {
        let bench = Bench::new("pr-view");
        bench.answer("pr-view", HEAD, 0, "");
        let checks = r#"[{"name":"build","bucket":"pending","workflow":"CI","link":"https://x"}]"#;
        bench.answer("pr-checks", checks, 8, "");
        let threads = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"totalCount":0,"nodes":[]}}}}}"#;
        bench.answer("api-graphql", threads, 0, "");
        let found = bench.found();
        let laid = view(&found, &repo(&found), Some(12)).unwrap();
        let text = laid.text.text().to_string();
        assert!(text.contains("● build"), "{text}");
        let asked = bench.asked();
        assert!(
            asked.contains("pr view 12 --repo github.com/o/r"),
            "{asked}"
        );
        assert!(
            asked.contains("api graphql --hostname github.com -f query="),
            "{asked}"
        );
        assert!(
            asked.contains("-f owner=o -f name=r -F number=12"),
            "{asked}"
        );
    }

    #[test]
    fn a_remote_off_github_asks_gh_nothing() {
        let bench = Bench::new("pr-elsewhere");
        bench.repo.git(&[
            "remote",
            "set-url",
            "origin",
            "https://evil.example/o/r.git",
        ]);
        let found = bench.found();
        let refused = target_of(&found.git).err().unwrap_or_default();
        assert!(refused.contains("not on github.com"), "{refused}");
        assert_eq!(bench.asked(), "");
    }

    /// A branch `feature` with one commit past `main`, which `origin/main` names, its upstream.
    fn feature_on_main(bench: &Bench) {
        bench
            .repo
            .git(&["commit", "-q", "--allow-empty", "-m", "first"]);
        bench
            .repo
            .git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        bench
            .repo
            .git(&["checkout", "-q", "-b", "feature", "--track", "origin/main"]);
        bench
            .repo
            .git(&["commit", "-q", "--allow-empty", "-m", "Add feature"]);
        bench.answer(
            "repo-view",
            r#"{"defaultBranchRef":{"name":"main"}}"#,
            0,
            "",
        );
    }

    #[test]
    fn a_branch_tracking_main_is_pushed_to_its_own_name_not_to_main() {
        let bench = Bench::new("pr-draft-own-name");
        feature_on_main(&bench);
        let found = bench.found();
        let drafted = super::super::create::draft(&found, &repo(&found), None).unwrap();
        assert_eq!(
            (drafted.base.as_str(), drafted.branch.as_str()),
            ("main", "feature")
        );
        // origin has no feature: it is pushed there, its upstream (origin/main) kept.
        assert!(drafted.push_first && !drafted.set_upstream);
        assert!(
            drafted.text.starts_with("Add feature\n"),
            "{}",
            drafted.text
        );
        bench
            .repo
            .git(&["update-ref", "refs/remotes/origin/feature", "HEAD"]);
        let drafted = super::super::create::draft(&found, &repo(&found), None).unwrap();
        assert!(!drafted.push_first, "the remote's feature has it");
    }

    #[test]
    fn a_branch_is_not_its_own_base() {
        let bench = Bench::new("pr-draft-own-base");
        feature_on_main(&bench);
        let found = bench.found();
        let refused = super::super::create::draft(&found, &repo(&found), Some("feature"));
        assert!(refused.unwrap_err().contains("its own base"));
        let refused = super::super::create::draft(&found, &repo(&found), Some("-x"));
        assert!(refused.unwrap_err().contains("Not a branch name"));
    }

    #[cfg(unix)]
    #[test]
    fn a_template_in_a_linked_directory_is_not_read_and_one_beside_is() {
        let bench = Bench::new("pr-draft-template");
        feature_on_main(&bench);
        let outside = bench.repo.dir.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(
            outside.join("PULL_REQUEST_TEMPLATE.md"),
            "## From outside\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, bench.repo.root.join(".github")).unwrap();
        let found = bench.found();
        let drafted = super::super::create::draft(&found, &repo(&found), None).unwrap();
        assert!(!drafted.text.contains("From outside"), "{}", drafted.text);
        fs::create_dir(bench.repo.root.join("docs")).unwrap();
        fs::write(
            bench.repo.root.join("docs/pull_request_template.md"),
            "## Checklist\n",
        )
        .unwrap();
        let drafted = super::super::create::draft(&found, &repo(&found), None).unwrap();
        assert!(
            drafted.text.ends_with("\n## Checklist\n"),
            "{}",
            drafted.text
        );
    }

    #[test]
    fn a_merge_is_of_the_branch_s_open_pull_request() {
        let bench = Bench::new("pr-merge-target");
        feature_on_main(&bench);
        let id = "b".repeat(40);
        let merge = format!(
            r#"{{"number":7,"state":"OPEN","baseRefName":"main","headRefOid":"{id}","headRefName":"feature"}}"#
        );
        bench.answer("pr-view", &merge, 0, "");
        let found = bench.found();
        let target = super::super::create::merge_target(&found, &repo(&found)).unwrap();
        assert_eq!(
            (target.number, target.head_commit.as_str()),
            (7, id.as_str())
        );
        bench.answer("pr-view", &merge.replace("OPEN", "MERGED"), 0, "");
        let refused = super::super::create::merge_target(&found, &repo(&found)).unwrap_err();
        assert!(refused.contains("not open"), "{refused}");
        bench.answer("pr-view", &merge.replace("\"feature\"", "\"other\""), 0, "");
        let refused = super::super::create::merge_target(&found, &repo(&found)).unwrap_err();
        assert!(refused.contains("has no pull request"), "{refused}");
    }
}
