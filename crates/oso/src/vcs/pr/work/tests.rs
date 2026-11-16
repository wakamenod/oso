//! A repository's `gh` thread: which reads are answered, the repository a remote names, and
//! its runs against a stand-in `gh` that writes what it was asked.
//!
//! Must not depend on: the network or a real `gh`; a script answers in its place.

use super::{PrRequest, latest, owner_and_name};
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
fn a_repository_s_owner_and_name() {
    let repo = GhRepo::from_url("https://github.com/o/r.git", None).unwrap();
    assert_eq!(owner_and_name(&repo), ("o", "r"));
}

#[cfg(unix)]
mod with_gh {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use oso_platform::process::Executable;

    use super::super::{Found, Repo, head, repo_of, view};
    use crate::git::{Gh, Git};
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
            }
        }

        fn asked(&self) -> String {
            fs::read_to_string(self.answers.join("asked")).unwrap_or_default()
        }
    }

    fn repo(found: &Found) -> Repo {
        repo_of(&found.git).unwrap()
    }

    const HEAD: &str = r#"{"number":12,"title":"Add x","state":"OPEN","isDraft":false,
    "reviewDecision":"","url":"https://github.com/o/r/pull/12","headRefOid":"abc",
    "statusCheckRollup":[],"author":{"login":"ann"},"baseRefName":"main",
    "headRefName":"main","body":"","comments":[]}"#;

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
            asked.contains("-F owner=o -F name=r -F number=12"),
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
        let refused = repo_of(&found.git).err().unwrap_or_default();
        assert!(refused.contains("not on github.com"), "{refused}");
        assert_eq!(bench.asked(), "");
    }
}
