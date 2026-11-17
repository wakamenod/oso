//! The git status's built-in menus (ADR 0127): the commit, push, pull and fetch menus (steps
//! 1b and 2), the stash and branch menus (step 3b), the log menu (step 4) and the pull request
//! menu (step 5a), each reached from a key of the status, the log menu from a graph's too.
//!
//! Must not depend on: `dispatch`, `app` or winit; menus are data.

use super::{BuiltInMenu, exit, open, switch};

/// Committing (ADR 0127, step 1b), from a git status's `c`, as magit's commit transient: its
/// switches, then each kind, with fixup and squash onto the commit at point (step 2).
pub(super) const COMMIT: BuiltInMenu = BuiltInMenu {
    name: "commit",
    title: "Commit",
    groups: &[
        (
            "Arguments",
            &[
                switch("- n", crate::commands::vcs::commit::NO_VERIFY, "no verify"),
                switch("- s", crate::commands::vcs::commit::SIGNOFF, "sign off"),
                switch(
                    "- e",
                    crate::commands::vcs::commit::ALLOW_EMPTY,
                    "allow empty",
                ),
            ],
        ),
        ("Create", &[exit("c", "git-commit", "commit")]),
        (
            "Edit HEAD",
            &[
                exit("a", "git-commit-amend", "amend"),
                exit("e", "git-commit-extend", "extend"),
                exit("w", "git-commit-reword", "reword"),
            ],
        ),
        (
            "Edit the commit at point",
            &[
                exit("f", "git-commit-fixup", "fixup"),
                exit("s", "git-commit-squash", "squash"),
            ],
        ),
    ],
    describe: None,
};

/// Pushing (ADR 0127, step 2), from a git status's `P`, as magit's push transient.
pub(super) const PUSH: BuiltInMenu = BuiltInMenu {
    name: "push",
    title: "Push",
    groups: &[
        (
            "Arguments",
            &[
                switch(
                    "- f",
                    crate::commands::vcs::remote::FORCE_WITH_LEASE,
                    "force with lease",
                ),
                switch(
                    "- n",
                    crate::commands::vcs::remote::PUSH_NO_VERIFY,
                    "no verify",
                ),
            ],
        ),
        (
            "Push the branch to",
            &[
                exit("p", "git-push", "its push target"),
                exit("u", "git-push-upstream", "its upstream"),
                exit("e", "git-push-elsewhere", "elsewhere"),
            ],
        ),
    ],
    describe: None,
};

/// Pulling (ADR 0127, step 2), from a git status's `F`: fast-forward only, unless `-r` rebases
/// or `-m` merges (step 3a).
pub(super) const PULL: BuiltInMenu = BuiltInMenu {
    name: "pull",
    title: "Pull",
    groups: &[
        (
            "Arguments",
            &[
                switch("- r", crate::commands::vcs::remote::PULL_REBASE, "rebase"),
                switch("- m", crate::commands::vcs::remote::PULL_MERGE, "merge"),
            ],
        ),
        (
            "Pull into the branch from",
            &[exit("u", "git-pull", "its upstream")],
        ),
    ],
    describe: None,
};

/// Fetching (ADR 0127, step 2), from a git status's `f`, pruning.
pub(super) const FETCH: BuiltInMenu = BuiltInMenu {
    name: "fetch",
    title: "Fetch",
    groups: &[(
        "Fetch, pruning, from",
        &[
            exit("u", "git-fetch", "the upstream's remote"),
            exit("a", "git-fetch-all", "every remote"),
        ],
    )],
    describe: None,
};

/// Stashing (ADR 0127, step 3b), from a git status's `z`: the stash at point, or one asked
/// for.
pub(super) const STASH: BuiltInMenu = BuiltInMenu {
    name: "stash",
    title: "Stash",
    groups: &[
        (
            "Arguments",
            &[switch(
                "- u",
                crate::commands::vcs::stash::STASH_UNTRACKED,
                "include untracked",
            )],
        ),
        (
            "Stash",
            &[
                exit("z", "git-stash", "the changes"),
                exit("a", "git-stash-apply", "apply"),
                exit("p", "git-stash-pop", "pop"),
                exit("k", "git-stash-drop", "drop"),
            ],
        ),
    ],
    describe: None,
};

/// Branches (ADR 0127, step 3b), from a git status's `b`: each reads its branch in a prompt.
pub(super) const BRANCH: BuiltInMenu = BuiltInMenu {
    name: "branch",
    title: "Branch",
    groups: &[
        (
            "Check out",
            &[
                exit("b", "git-branch-checkout", "a branch"),
                exit("c", "git-branch-create-checkout", "a new branch"),
            ],
        ),
        (
            "Change",
            &[
                exit("n", "git-branch-create", "make one"),
                exit("m", "git-branch-rename", "rename"),
                exit("k", "git-branch-delete", "delete"),
            ],
        ),
    ],
    describe: None,
};

/// Pull requests (ADR 0127, step 5a), from a git status's `N`, through `gh`: reading them, and
/// from step 5a-2 making one, with its switches, and the merge menu.
pub(super) const PULL_REQUEST: BuiltInMenu = BuiltInMenu {
    name: "pull-request",
    title: "Pull request",
    groups: &[
        (
            "Arguments",
            &[
                switch("- d", crate::commands::vcs::pr::create::DRAFT, "draft"),
                switch(
                    "- b",
                    crate::commands::vcs::pr::create::BASE,
                    "into a base read",
                ),
            ],
        ),
        (
            "The branch's pull request",
            &[
                exit("v", "git-pr-view", "view"),
                exit("k", "git-pr-checks", "checks"),
                exit("o", "git-pr-browse", "open in the browser"),
                exit("c", "git-pr-create", "create"),
                open("m", "pull-request-merge", "merge"),
            ],
        ),
        (
            "The repository's",
            &[exit("l", "git-pr-list", "the open ones")],
        ),
    ],
    describe: None,
};

/// Merging the branch's pull request (ADR 0127, step 5a-2), from the pull request menu's `m`:
/// each head asks first, naming the pull request, its base, the method and its head.
pub(super) const PULL_REQUEST_MERGE: BuiltInMenu = BuiltInMenu {
    name: "pull-request-merge",
    title: "Merge the pull request",
    groups: &[
        (
            "Arguments",
            &[switch(
                "- d",
                crate::commands::vcs::pr::create::DELETE_BRANCH,
                "delete the branch on the remote",
            )],
        ),
        (
            "Merge",
            &[
                exit("s", "git-pr-merge-squash", "squashed"),
                exit("m", "git-pr-merge-commit", "with a merge commit"),
                exit("r", "git-pr-merge-rebase", "rebased"),
            ],
        ),
    ],
    describe: None,
};

/// The log (ADR 0127, step 4, the build plan's Q3), from a git status's or a graph's `l`, as
/// magit's: each head opens the graph.
pub(super) const LOG: BuiltInMenu = BuiltInMenu {
    name: "log",
    title: "Log",
    groups: &[(
        "Show the graph of",
        &[
            exit("l", "git-graph", "the branch checked out"),
            exit("a", "git-graph-all", "every ref"),
            exit("o", "git-graph-other", "another ref"),
        ],
    )],
    describe: None,
};
