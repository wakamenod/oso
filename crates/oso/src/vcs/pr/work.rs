//! A repository's thread for `gh`, `oso-vcs-gh` (ADR 0127, step 5a): it reads the pull request
//! of the branch checked out for the status's head line, one pull request with its checks and
//! review threads, and the open ones, lays out their texts, and opens a pull request's address;
//! from step 5b-1 the same for issues ([`issue`]).
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; the caller hands requests in,
//! takes reports out, and supplies the wake-up.
//!
//! - It starts only from a [`TrustedRoot`], looks at the root before each request, and finds
//!   git as the status's thread does; git only says which branch is checked out, which remote
//!   it pushes to and that remote's address.
//! - `gh` is [`Gh`]'s: run in the empty private directory, never in the repository, about the
//!   repository on `github.com` or the host `GH_HOST` names that the remote's address names
//!   (Q2), one request at a time, each run killed past [`GH_LIMIT`]. A remote elsewhere is said,
//!   and `gh` is not run.
//! - Only the latest read of each kind is answered: a read asked while one waits replaces it.
//! - Before each run of `gh` the root is looked at again, so a project no longer trusted runs
//!   nothing more, even within a request.
//! - The branch's pull request is the one `gh` names for that branch: one of another head
//!   branch, as a branch named as a number would get, is none.
//! - An address is opened only if it is the pull request's own on that host (Q5), with the
//!   system's opener, whose end is waited for at most [`OPENER_LIMIT`] and then on a thread of
//!   its own, so that it is reaped.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::write::{PrLaid, list_text, view_text};
use super::{
    CHECK_FIELDS, HEAD_FIELDS, LIST_FIELDS, PrHead, THREADS_QUERY, VIEW_FIELDS, is_pull_address,
    read_address, read_checks, read_head, read_list, read_threads, read_view,
};
use crate::git::{GH_LIMIT, Gh, GhAnswer, GhRepo, Git, PullTarget, TrustedRoot};
use crate::vcs::read::{find, hooks_dir};

/// What a request that panicked is answered.
const PANICKED: &str = "gh's thread failed: ask again";

/// How long the system's opener is waited for.
const OPENER_LIMIT: Duration = Duration::from_secs(2);
/// The most bytes of one of `gh`'s answers read: a pull request with its conversation, or its
/// review threads.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// What `gh pr checks` exits with while some checks are still going.
const CHECKS_PENDING: i32 = 8;
/// How many open pull requests a list asks for.
const LIST_LIMIT: &str = "100";

/// What an address is of: a pull request or an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GhItem {
    Pull,
    Issue,
}

/// What the thread is asked to do.
#[derive(Debug)]
pub(crate) enum PrRequest {
    /// The branch's pull request, for the status's head line.
    Head { generation: u64 },
    /// The pull request numbered `number`, or the branch's, with its checks and threads.
    View {
        generation: u64,
        number: Option<u64>,
    },
    /// The repository's open pull requests.
    List { generation: u64 },
    /// The address of the branch's pull request, for the echo area to show before it opens.
    Address,
    /// Open `url`, the address of `item` `number`.
    Open {
        url: String,
        number: u64,
        item: GhItem,
    },
    /// What a pull request of the branch starts from, into `base` or the remote's default
    /// (step 5a-2).
    Draft {
        generation: u64,
        base: Option<String>,
    },
    /// The branch's pull request, open, to merge (step 5a-2).
    MergeAsk { generation: u64 },
    /// The repository's open issues (step 5b-1).
    IssueList { generation: u64 },
    /// Issue `number`, with its comments.
    IssueView { generation: u64, number: u64 },
    /// What a worktree session from issue `number` starts from.
    IssueBrief { generation: u64, number: u64 },
}

/// What the thread found.
pub(crate) enum PrReport {
    /// The branch's pull request, or none, or why it is not known.
    Head {
        generation: u64,
        head: Result<Option<PrHead>, String>,
    },
    /// A pull request's text.
    View {
        generation: u64,
        laid: Result<Box<PrLaid>, String>,
    },
    /// The list's text.
    List {
        generation: u64,
        laid: Result<Box<PrLaid>, String>,
    },
    /// The address of the branch's pull request and its number.
    Address(Result<(String, u64), String>),
    /// What opening an address came to, for the echo area.
    Opened(String),
    /// What a pull request's message starts from.
    Drafted {
        generation: u64,
        draft: Result<Drafted, String>,
    },
    /// The pull request to merge.
    MergeAsked {
        generation: u64,
        merge: Result<MergeTarget, String>,
    },
    /// What a worktree session from an issue starts from (step 5b-1).
    IssueBriefed {
        generation: u64,
        brief: Result<IssueBrief, String>,
    },
}

impl std::fmt::Debug for PrReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::Head { .. } => "Head",
            Self::View { .. } => "View",
            Self::List { .. } => "List",
            Self::Address(_) => "Address",
            Self::Opened(_) => "Opened",
            Self::Drafted { .. } => "Drafted",
            Self::MergeAsked { .. } => "MergeAsked",
            Self::IssueBriefed { .. } => "IssueBriefed",
        };
        f.debug_tuple(kind).finish_non_exhaustive()
    }
}

/// A repository's thread for `gh`. Dropping it ends the thread once its run is done.
pub(crate) struct PrWorker {
    requests: Sender<PrRequest>,
}

impl PrWorker {
    /// Starts the thread for the project at `root`, with the empty private directory under
    /// `state`, handing each report to `send`, which says `false` once the event loop has gone.
    pub(crate) fn start(
        root: TrustedRoot,
        state: Option<PathBuf>,
        send: impl FnMut(PrReport) -> bool + Send + 'static,
    ) -> std::io::Result<PrWorker> {
        let (requests, queue) = mpsc::channel();
        thread::Builder::new()
            .name("oso-vcs-gh".into())
            .spawn(move || serve(&queue, send, &root, state.as_deref()))?;
        Ok(PrWorker { requests })
    }

    /// Asks the thread for `request`; `false` if it has ended.
    pub(crate) fn ask(&self, request: PrRequest) -> bool {
        self.requests.send(request).is_ok()
    }
}

/// What the thread finds once and keeps.
struct Found {
    git: Git,
    gh: Gh,
    /// The empty private directory `gh` runs in, its own.
    cwd: PathBuf,
    root: TrustedRoot,
}

fn serve(
    queue: &Receiver<PrRequest>,
    mut send: impl FnMut(PrReport) -> bool,
    root: &TrustedRoot,
    state: Option<&Path>,
) {
    let mut found: Option<Found> = None;
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        batch.extend(queue.try_iter());
        for request in latest(batch) {
            let report = if let Err(why) = root.check() {
                refused(&request, format!("Not run: {why}"))
            } else {
                // A request that panics is answered as one refused, of its own kind.
                let stand_in = refused(&request, PANICKED.into());
                let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    answer(&mut found, root, state, request)
                }));
                answered.unwrap_or_else(|_| {
                    log::warn!("a run of gh panicked");
                    found = None;
                    stand_in
                })
            };
            if !send(report) {
                return;
            }
        }
    }
}

/// The requests of `batch` to answer, in order: of each read, only the last.
fn latest(batch: Vec<PrRequest>) -> Vec<PrRequest> {
    let kind = |request: &PrRequest| match request {
        PrRequest::Head { .. } => 0,
        PrRequest::View { .. } => 1,
        PrRequest::List { .. } => 2,
        PrRequest::Draft { .. } => 4,
        PrRequest::MergeAsk { .. } => 5,
        PrRequest::IssueList { .. } => 6,
        PrRequest::IssueView { .. } => 7,
        PrRequest::IssueBrief { .. } => 8,
        PrRequest::Address | PrRequest::Open { .. } => 3,
    };
    let mut kept: Vec<PrRequest> = Vec::with_capacity(batch.len());
    for request in batch {
        let read = kind(&request);
        if read != 3 {
            kept.retain(|old| kind(old) != read);
        }
        kept.push(request);
    }
    kept
}

/// The report for `request` when it is refused, with `why`.
fn refused(request: &PrRequest, why: String) -> PrReport {
    match request {
        PrRequest::Head { generation } => PrReport::Head {
            generation: *generation,
            head: Err(why),
        },
        PrRequest::View { generation, .. } => PrReport::View {
            generation: *generation,
            laid: Err(why),
        },
        PrRequest::List { generation } => PrReport::List {
            generation: *generation,
            laid: Err(why),
        },
        PrRequest::Address => PrReport::Address(Err(why)),
        PrRequest::Open { .. } => PrReport::Opened(why),
        PrRequest::Draft { generation, .. } => PrReport::Drafted {
            generation: *generation,
            draft: Err(why),
        },
        PrRequest::MergeAsk { generation } => PrReport::MergeAsked {
            generation: *generation,
            merge: Err(why),
        },
        PrRequest::IssueList { generation } => PrReport::List {
            generation: *generation,
            laid: Err(why),
        },
        PrRequest::IssueView { generation, .. } => PrReport::View {
            generation: *generation,
            laid: Err(why),
        },
        PrRequest::IssueBrief { generation, .. } => PrReport::IssueBriefed {
            generation: *generation,
            brief: Err(why),
        },
    }
}

fn answer(
    found: &mut Option<Found>,
    root: &TrustedRoot,
    state: Option<&Path>,
    request: PrRequest,
) -> PrReport {
    if found.is_none() {
        match find_all(root, state) {
            Ok(all) => *found = Some(all),
            Err(why) => return refused(&request, why),
        }
    }
    let Some(found) = found.as_ref() else {
        return refused(&request, PANICKED.into());
    };
    let target = match PullTarget::of(&found.git) {
        Ok(target) => target,
        Err(why) => return refused(&request, why),
    };
    match request {
        PrRequest::Head { generation } => PrReport::Head {
            generation,
            head: head(found, &target),
        },
        PrRequest::View { generation, number } => PrReport::View {
            generation,
            laid: view(found, &target, number).map(Box::new),
        },
        PrRequest::List { generation } => PrReport::List {
            generation,
            laid: list(found, &target).map(Box::new),
        },
        PrRequest::Address => PrReport::Address(address(found, &target)),
        PrRequest::Open { url, number, item } => {
            PrReport::Opened(open(&target.github, (&url, number, item), root.path()))
        }
        PrRequest::IssueList { generation } => PrReport::List {
            generation,
            laid: issue::list(found, &target).map(Box::new),
        },
        PrRequest::IssueView { generation, number } => PrReport::View {
            generation,
            laid: issue::view(found, &target, number).map(Box::new),
        },
        PrRequest::IssueBrief { generation, number } => PrReport::IssueBriefed {
            generation,
            brief: issue::brief(found, &target, number),
        },
        PrRequest::Draft { generation, base } => PrReport::Drafted {
            generation,
            draft: create::draft(found, &target, base.as_deref()),
        },
        PrRequest::MergeAsk { generation } => PrReport::MergeAsked {
            generation,
            merge: create::merge_target(found, &target),
        },
    }
}

fn find_all(root: &TrustedRoot, state: Option<&Path>) -> Result<Found, String> {
    let hooks = hooks_dir(state)?;
    let git = find(root.path(), &hooks)?;
    let gh = Gh::find(root.path())
        .ok_or("gh is not installed, or not on PATH: oso reads pull requests through gh")?;
    let state = state.ok_or("oso has no state directory, so it runs no gh")?;
    let cwd = crate::git::empty_hooks(state, "gh")?;
    Ok(Found {
        git,
        gh,
        cwd,
        root: root.clone(),
    })
}

/// The repository on GitHub the branch checked out's remote names now, as `git` says, for the
/// run thread to check a pull request's run against (step 5a-2).
pub(crate) fn branch_repo(git: &Git) -> Result<GhRepo, String> {
    PullTarget::of(git).map(|target| target.github)
}

/// Runs `gh` with `words` about `target`'s repository, once the root is still trusted.
fn run(found: &Found, target: &PullTarget, words: &[&str]) -> Result<GhAnswer, String> {
    found
        .root
        .check()
        .map_err(|why| format!("Not run: {why}"))?;
    (found.gh).run(&target.github, words, &found.cwd, MAX_ANSWER, GH_LIMIT)
}

/// `gh pr view` of `number`, or of the branch checked out.
fn pr_view(
    found: &Found,
    target: &PullTarget,
    number: Option<u64>,
    fields: &str,
) -> Result<GhAnswer, String> {
    let which = match (number, &target.branch) {
        (Some(number), _) => number.to_string(),
        (None, Some(branch)) => branch.clone(),
        (None, None) => return Err("No branch is checked out, so no pull request is its".into()),
    };
    let repo = target.github.as_str();
    run(
        found,
        target,
        &["pr", "view", &which, "--repo", repo, "--json", fields],
    )
}

/// Whether `gh` said the branch has no pull request.
fn none_found(answer: &GhAnswer) -> bool {
    answer.code != Some(0) && answer.stderr.contains("no pull requests found")
}

/// Why the branch has none.
fn no_pull_request(target: &PullTarget) -> String {
    let branch = target.branch.as_deref().unwrap_or("HEAD");
    format!("{branch} has no pull request: N c makes one")
}

fn head(found: &Found, target: &PullTarget) -> Result<Option<PrHead>, String> {
    let answer = pr_view(found, target, None, HEAD_FIELDS)?;
    if none_found(&answer) {
        return Ok(None);
    }
    let head = read_head(&answer.into_stdout()?).ok_or_else(unexpected)?;
    Ok((Some(&head.branch) == target.branch.as_ref()).then_some(head))
}

fn view(found: &Found, target: &PullTarget, number: Option<u64>) -> Result<PrLaid, String> {
    let answer = pr_view(found, target, number, VIEW_FIELDS)?;
    if none_found(&answer) {
        return Err(no_pull_request(target));
    }
    let mut view = read_view(&answer.into_stdout()?).ok_or_else(unexpected)?;
    if number.is_none() && Some(&view.branch) != target.branch.as_ref() {
        return Err(no_pull_request(target));
    }
    let n = view.head.number.to_string();
    let repo = target.github.as_str();
    let checks = ["pr", "checks", &n, "--repo", repo, "--json", CHECK_FIELDS];
    let checks = run(found, target, &checks)?;
    view.checks = match checks.code {
        Some(0 | CHECKS_PENDING) => read_checks(&checks.stdout).ok_or_else(unexpected)?,
        _ if checks.stderr.contains("no checks reported") => Vec::new(),
        _ => return Err(format!("gh failed: {}", checks.stderr)),
    };
    let (host, owner, name) = target.github.parts();
    let query = format!("query={THREADS_QUERY}");
    let (owner, name, number) = (
        format!("owner={owner}"),
        format!("name={name}"),
        format!("number={n}"),
    );
    // `-f` takes the owner and name as strings, `-F` the number as one.
    let threads = [
        "api",
        "graphql",
        "--hostname",
        host,
        "-f",
        &query,
        "-f",
        &owner,
        "-f",
        &name,
        "-F",
        &number,
    ];
    let threads = run(found, target, &threads)?.into_stdout()?;
    (view.threads, view.more_threads) = read_threads(&threads).ok_or_else(unexpected)?;
    Ok(view_text(&view))
}

fn list(found: &Found, target: &PullTarget) -> Result<PrLaid, String> {
    let repo = target.github.as_str();
    let words = [
        "pr",
        "list",
        "--repo",
        repo,
        "--state",
        "open",
        "--limit",
        LIST_LIMIT,
        "--json",
        LIST_FIELDS,
    ];
    let out = run(found, target, &words)?.into_stdout()?;
    let items = read_list(&out).ok_or_else(unexpected)?;
    Ok(list_text(repo, &items))
}

fn address(found: &Found, target: &PullTarget) -> Result<(String, u64), String> {
    let answer = pr_view(found, target, None, "number,url,headRefName")?;
    if none_found(&answer) {
        return Err(no_pull_request(target));
    }
    let (url, number, branch) = read_address(&answer.into_stdout()?).ok_or_else(unexpected)?;
    if Some(&branch) != target.branch.as_ref() {
        return Err(no_pull_request(target));
    }
    if !is_pull_address(&url, &target.github, number) {
        let repo = target.github.as_str();
        return Err(format!(
            "gh gave an address that is not #{number}'s on {repo}"
        ));
    }
    Ok((url, number))
}

/// Opens `url` with the system's opener, if it is pull request `number`'s own address on
/// `repo`'s host, or issue `number`'s if `issue`, leaving `project` out of the opener's `PATH`;
/// what that came to.
fn open(repo: &GhRepo, (url, number, item): (&str, u64, GhItem), project: &Path) -> String {
    let own = match item {
        GhItem::Issue => super::issue::is_issue_address(url, repo, number),
        GhItem::Pull => is_pull_address(url, repo, number),
    };
    if !own {
        return format!(
            "Not opened: not the address of #{number} on {}",
            repo.as_str()
        );
    }
    let mut child = match oso_platform::process::open_link(url, Some(project)) {
        Ok(child) => child,
        Err(e) => return format!("Cannot open {url}: {e}"),
    };
    let deadline = Instant::now() + OPENER_LIMIT;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return format!("Opened {url}"),
            Ok(Some(status)) => return format!("The system's opener failed ({status}) for {url}"),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return format!("Cannot open {url}: {e}"),
        }
    }
    // An opener that waits for the browser is reaped on a thread of its own.
    let reaped = thread::Builder::new()
        .name("oso-vcs-opener".into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(e) = reaped {
        log::warn!("cannot wait for the system's opener: {e}");
    }
    format!("Opening {url}")
}

fn unexpected() -> String {
    "gh gave an unexpected answer".to_owned()
}

mod create;
mod issue;

pub(crate) use create::{Drafted, MergeTarget};
pub(crate) use issue::IssueBrief;

#[cfg(test)]
mod tests;
