//! A repository's worktrees listed for `*Worktrees*` (ADR 0121, `worktree-list`): every row
//! looked at without `gh` first, then, for the person's listing, the repository's open pull
//! requests (ADR 0127, step 5b-2), then what `gh` says of the rows whose merge it alone can tell.
//! A newer listing of the person's stops an older one between rows; an agent's runs until it is
//! told to stop, and asks `gh` for no open pull request.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; the reports go back as the other
//! jobs' do.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::checks::{self, Depth};
use super::super::facts::RepoFacts;
use super::super::git::Repo;
use super::super::merged;
use super::{Jobs, Report, Snapshot, Trust, remove};
use crate::git::Gh;

impl Jobs {
    /// Lists the linked worktrees of the repository, a row each as it is looked at, then what
    /// `gh` says of the rows that need it. A newer listing stops this one.
    pub fn list(&self, token: u64, trust: Trust, snapshot: Snapshot) {
        let number = self.listing.fetch_add(1, Ordering::SeqCst) + 1;
        let newest = Arc::clone(&self.listing);
        self.list_while(token, (trust, true), snapshot, move || {
            newest.load(Ordering::SeqCst) == number
        });
    }

    /// Lists as [`Self::list`] does, for an agent's `worktree_list`, until `stop` is set: no
    /// listing of the person's stops it, and it stops none of theirs.
    pub fn list_for_agent(
        &self,
        token: u64,
        trust: Trust,
        snapshot: Snapshot,
        stop: Arc<AtomicBool>,
    ) {
        // An agent's listing asks `gh` for no open pull request (ADR 0127, step 5b-2).
        self.list_while(token, (trust, false), snapshot, move || {
            !stop.load(Ordering::SeqCst)
        });
    }

    /// A listing that goes on while `live` says so, and says when it ended.
    fn list_while(
        &self,
        token: u64,
        (trust, pulls): (Trust, bool),
        snapshot: Snapshot,
        live: impl Fn() -> bool + Send + 'static,
    ) {
        let gh_host = self.gh_host.clone();
        let removals = self.removals.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                // One stopped while it waited for a thread runs nothing.
                if live() {
                    let looking = Looking {
                        snapshot: &snapshot,
                        gh_host: gh_host.as_deref(),
                        removals: &removals,
                    };
                    list(repo, &looking, &live, (token, pulls), send);
                }
                send(Report::ListEnded { token });
            },
            move |why| Report::Listed {
                token,
                result: Err(why),
            },
        );
    }
}

/// What a listing looks with besides the repository.
struct Looking<'a> {
    snapshot: &'a Snapshot,
    gh_host: Option<&'a str>,
    /// The removals that run, whose rows say so.
    removals: &'a remove::Removals,
}

/// A listing's work: every row looked at without `gh`, then `gh` asked for the rows that need
/// it, while `live` says no newer listing was asked for.
fn list(
    repo: &Repo,
    looking: &Looking<'_>,
    live: &dyn Fn() -> bool,
    (token, pulls): (u64, bool),
    send: &dyn Fn(Report),
) {
    let Looking {
        snapshot,
        gh_host,
        removals,
    } = *looking;
    let listed = match repo.worktrees() {
        Ok(listed) => listed,
        Err(why) => {
            return send(Report::Listed {
                token,
                result: Err(why),
            });
        }
    };
    // The first is the main worktree.
    let linked: Vec<_> = listed.into_iter().skip(1).collect();
    send(Report::Listed {
        token,
        result: Ok(linked.len()),
    });
    let gh = Gh::find(repo.main());
    let facts = RepoFacts::gather(
        repo,
        gh_host,
        &snapshot.hosted,
        crate::sessions::registry::directory().as_deref(),
    );
    let mut asks = Vec::new();
    for listed in linked {
        if !live() {
            return;
        }
        let held = snapshot.held(&listed.path, &facts.hosted_idle);
        let mut looked = checks::look(repo, gh.as_ref(), &facts, listed, &held, Depth::Local);
        looked.removing = removals.running_at(&looked.listed.path);
        if let (Some(tip), Some(branch)) = (&looked.gh_pending, &looked.listed.branch) {
            let clean = looked.clean().then_some(looked.looked_at);
            asks.push((
                looked.listed.path.clone(),
                branch.clone(),
                tip.clone(),
                clean,
            ));
        }
        send(Report::Row {
            token,
            looked: Box::new(looked),
        });
    }
    let Some(gh) = gh else {
        return;
    };
    if pulls
        && live()
        && let Some((_, github)) = &facts.github
        && let Ok(pulls) = super::super::git::open_pulls(&gh, github, repo.hooks())
    {
        send(Report::Pulls { token, pulls });
    }
    for (path, branch, tip, clean) in asks {
        if !live() {
            return;
        }
        let merged = merged::by_pull_request(repo, &gh, &facts, &branch, &tip);
        send(Report::Merged {
            token,
            path,
            merged,
            clean,
        });
    }
}
