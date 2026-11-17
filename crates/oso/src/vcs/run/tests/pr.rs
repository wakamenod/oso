//! A repository's run thread over a pull request's runs: one asked for a repository the
//! branch's remote no longer names is refused before `gh` is looked for.
//!
//! Must not depend on: `app`, `dispatch`, winit or `gh`.

use oso_core::editor::MergeMethod;

use super::super::{RunEnd, RunRequest};
use super::{repo, runner, until_end};
use crate::vcs::pr::create::PrRun;

#[test]
fn a_run_for_a_repository_the_remote_no_longer_names_is_refused() {
    let repo = repo("pr-other-repo");
    repo.git(&["remote", "add", "origin", "https://github.com/o/now.git"]);
    let (runner, got) = runner(&repo);
    let runs = [
        PrRun::Create {
            repo: "github.com/o/then".into(),
            base: "main".into(),
            head: "main".into(),
            title: "t".into(),
            body: String::new(),
            draft: false,
        },
        PrRun::Merge {
            repo: "github.com/o/then".into(),
            number: 1,
            method: MergeMethod::Squash,
            base: "main".into(),
            head_commit: "a".repeat(40),
            delete_branch: false,
        },
    ];
    for run in runs {
        assert!(runner.ask(RunRequest::Pr { run }));
        let (end, output, began) = until_end(&got);
        assert!(
            matches!(&end, RunEnd::Refused(why) if why.contains("another repository")),
            "{end:?}"
        );
        assert!(!began, "{output}");
    }
}
