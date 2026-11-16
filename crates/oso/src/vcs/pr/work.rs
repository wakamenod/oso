//! A repository's thread for `gh`, `oso-vcs-gh` (ADR 0127, step 5a): it reads the pull request
//! of the branch checked out for the status's head line, one pull request with its checks and
//! review threads, and the open ones, lays out their texts, and opens a pull request's address.
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
//! - An address is opened only if it is the pull request's own on that host (Q5), with the
//!   system's opener, which is waited for at most [`OPENER_LIMIT`].

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::write::{PrLaid, list_text, view_text};
use super::{
    CHECK_FIELDS, HEAD_FIELDS, LIST_FIELDS, PrHead, THREADS_QUERY, VIEW_FIELDS, is_pull_address,
    read_address, read_checks, read_head, read_list, read_threads, read_view,
};
use crate::git::{Gh, GhAnswer, GhRepo, Git, MAX_SHORT, TrustedRoot, trim_line};
use crate::vcs::read::{args, find, hooks_dir, set};

/// How long one run of `gh` may take.
pub(crate) const GH_LIMIT: Duration = Duration::from_secs(30);
/// How long the system's opener is waited for.
const OPENER_LIMIT: Duration = Duration::from_secs(10);
/// The most bytes of one of `gh`'s answers read: a pull request with its conversation, or its
/// review threads.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// What `gh pr checks` exits with while some checks are still going.
const CHECKS_PENDING: i32 = 8;
/// How many open pull requests a list asks for.
const LIST_LIMIT: &str = "100";

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
    /// Open `url`, the address of pull request `number`.
    Open { url: String, number: u64 },
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
    /// The list's text, and the repository it is of.
    List {
        generation: u64,
        laid: Result<(String, Box<PrLaid>), String>,
    },
    /// The address of the branch's pull request and its number.
    Address(Result<(String, u64), String>),
    /// What opening an address came to, for the echo area.
    Opened(String),
}

impl std::fmt::Debug for PrReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::Head { .. } => "Head",
            Self::View { .. } => "View",
            Self::List { .. } => "List",
            Self::Address(_) => "Address",
            Self::Opened(_) => "Opened",
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
    /// The empty private directory `gh` runs in.
    cwd: PathBuf,
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
            if let Err(why) = root.check() {
                let report = refused(&request, format!("Not run: {why}"));
                if report.is_some_and(|report| !send(report)) {
                    return;
                }
                continue;
            }
            let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                answer(&mut found, root.path(), state, request)
            }));
            let report = report.unwrap_or_else(|_| {
                log::warn!("a run of gh panicked");
                found = None;
                Some(PrReport::Opened("gh's thread failed: ask again".into()))
            });
            if report.is_some_and(|report| !send(report)) {
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
        PrRequest::Address | PrRequest::Open { .. } => 3,
    };
    let mut kept: Vec<PrRequest> = Vec::with_capacity(batch.len());
    for request in batch {
        let read = kind(&request);
        if read < 3 {
            kept.retain(|old| kind(old) != read);
        }
        kept.push(request);
    }
    kept
}

/// The report for `request` when it is refused, with `why`.
fn refused(request: &PrRequest, why: String) -> Option<PrReport> {
    Some(match request {
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
    })
}

fn answer(
    found: &mut Option<Found>,
    root: &Path,
    state: Option<&Path>,
    request: PrRequest,
) -> Option<PrReport> {
    if found.is_none() {
        match find_all(root, state) {
            Ok(all) => *found = Some(all),
            Err(why) => return refused(&request, why),
        }
    }
    let found = found.as_ref()?;
    let repo = match repo_of(&found.git) {
        Ok(repo) => repo,
        Err(why) => return refused(&request, why),
    };
    Some(match request {
        PrRequest::Head { generation } => PrReport::Head {
            generation,
            head: head(found, &repo),
        },
        PrRequest::View { generation, number } => PrReport::View {
            generation,
            laid: view(found, &repo, number).map(Box::new),
        },
        PrRequest::List { generation } => PrReport::List {
            generation,
            laid: list(found, &repo).map(|laid| (repo.0.as_str().to_owned(), Box::new(laid))),
        },
        PrRequest::Address => PrReport::Address(address(found, &repo)),
        PrRequest::Open { url, number } => PrReport::Opened(open(&repo.0, &url, number)),
    })
}

fn find_all(root: &Path, state: Option<&Path>) -> Result<Found, String> {
    let cwd = hooks_dir(state)?;
    let git = find(root, &cwd)?;
    let gh = Gh::find(root)
        .ok_or("gh is not installed, or not on PATH: oso reads pull requests through gh")?;
    Ok(Found { git, gh, cwd })
}

/// The repository on GitHub of the remote the branch checked out pushes to, else `origin`'s,
/// and the branch, if one is checked out.
struct Repo(GhRepo, Option<String>);

fn repo_of(git: &Git) -> Result<Repo, String> {
    let read = |words: &[&str]| -> Option<String> {
        let out = git.run(&args(words), &set(), MAX_SHORT).ok()?;
        let line = std::str::from_utf8(trim_line(&out)).ok()?.to_owned();
        (!line.is_empty()).then_some(line)
    };
    let branch = read(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .filter(|branch| !branch.starts_with('-'));
    let remote = branch
        .as_deref()
        .and_then(|branch| {
            let head = format!("refs/heads/{branch}");
            read(&["for-each-ref", "--format=%(upstream:remotename)", &head])
        })
        .unwrap_or_else(|| "origin".to_owned());
    let url = read(&["remote", "get-url", "--", &remote])
        .ok_or_else(|| format!("No remote {remote}: a pull request is a GitHub remote's"))?;
    let gh_host = std::env::var("GH_HOST").ok();
    let repo = GhRepo::from_url(&url, gh_host.as_deref()).ok_or_else(|| {
        format!("{remote} is not on github.com or the host GH_HOST names: gh is not asked")
    })?;
    Ok(Repo(repo, branch))
}

/// `gh pr view` of `number`, or of the branch checked out.
fn pr_view(
    found: &Found,
    repo: &Repo,
    number: Option<u64>,
    fields: &str,
) -> Result<GhAnswer, String> {
    let which = match (number, &repo.1) {
        (Some(number), _) => number.to_string(),
        (None, Some(branch)) => branch.clone(),
        (None, None) => return Err("No branch is checked out, so no pull request is its".into()),
    };
    let words = [
        "pr",
        "view",
        &which,
        "--repo",
        repo.0.as_str(),
        "--json",
        fields,
    ];
    found
        .gh
        .run(&repo.0, &words, &found.cwd, (MAX_ANSWER, GH_LIMIT))
}

/// Whether `gh` said the branch has no pull request.
fn none_found(answer: &GhAnswer) -> bool {
    answer.code != Some(0) && answer.stderr.contains("no pull requests found")
}

fn head(found: &Found, repo: &Repo) -> Result<Option<PrHead>, String> {
    let answer = pr_view(found, repo, None, HEAD_FIELDS)?;
    if none_found(&answer) {
        return Ok(None);
    }
    let out = answer.ok()?;
    read_head(&out).map(Some).ok_or_else(unexpected)
}

fn view(found: &Found, repo: &Repo, number: Option<u64>) -> Result<PrLaid, String> {
    let answer = pr_view(found, repo, number, VIEW_FIELDS)?;
    if none_found(&answer) {
        let branch = repo.1.as_deref().unwrap_or("HEAD");
        return Err(format!("{branch} has no pull request: N c makes one"));
    }
    let mut view = read_view(&answer.ok()?).ok_or_else(unexpected)?;
    let n = view.head.number.to_string();
    let checks = [
        "pr",
        "checks",
        &n,
        "--repo",
        repo.0.as_str(),
        "--json",
        CHECK_FIELDS,
    ];
    let checks = found
        .gh
        .run(&repo.0, &checks, &found.cwd, (MAX_ANSWER, GH_LIMIT))?;
    view.checks = match checks.code {
        Some(0 | CHECKS_PENDING) => read_checks(&checks.stdout).ok_or_else(unexpected)?,
        _ if checks.stderr.contains("no checks reported") => Vec::new(),
        _ => return Err(format!("gh failed: {}", checks.stderr)),
    };
    let (owner, name) = owner_and_name(&repo.0);
    let host = repo.0.as_str().split('/').next().unwrap_or_default();
    let threads = [
        "api",
        "graphql",
        "--hostname",
        host,
        "-f",
        &format!("query={THREADS_QUERY}"),
        "-F",
        &format!("owner={owner}"),
        "-F",
        &format!("name={name}"),
        "-F",
        &format!("number={n}"),
    ];
    let threads = found
        .gh
        .run(&repo.0, &threads, &found.cwd, (MAX_ANSWER, GH_LIMIT))?;
    (view.threads, view.more_threads) = read_threads(&threads.ok()?).ok_or_else(unexpected)?;
    Ok(view_text(&view))
}

fn list(found: &Found, repo: &Repo) -> Result<PrLaid, String> {
    let words = [
        "pr",
        "list",
        "--repo",
        repo.0.as_str(),
        "--state",
        "open",
        "--limit",
        LIST_LIMIT,
        "--json",
        LIST_FIELDS,
    ];
    let out = found
        .gh
        .run(&repo.0, &words, &found.cwd, (MAX_ANSWER, GH_LIMIT))?
        .ok()?;
    let items = read_list(&out).ok_or_else(unexpected)?;
    Ok(list_text(repo.0.as_str(), &items))
}

fn address(found: &Found, repo: &Repo) -> Result<(String, u64), String> {
    let answer = pr_view(found, repo, None, "number,url")?;
    if none_found(&answer) {
        let branch = repo.1.as_deref().unwrap_or("HEAD");
        return Err(format!("{branch} has no pull request: N c makes one"));
    }
    let (url, number) = read_address(&answer.ok()?).ok_or_else(unexpected)?;
    if !is_pull_address(&url, repo.0.as_str(), number) {
        return Err(format!(
            "gh gave an address that is not #{number}'s on {}",
            repo.0.as_str()
        ));
    }
    Ok((url, number))
}

/// Opens `url` with the system's opener, if it is pull request `number`'s own address on
/// `repo`'s host; what that came to.
fn open(repo: &GhRepo, url: &str, number: u64) -> String {
    if !is_pull_address(url, repo.as_str(), number) {
        return format!(
            "Not opened: not the address of #{number} on {}",
            repo.as_str()
        );
    }
    let mut child = match oso_platform::process::open_link(url) {
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
    format!("Opening {url}")
}

/// The owner and name of `repo`, `HOST/OWNER/NAME`.
fn owner_and_name(repo: &GhRepo) -> (&str, &str) {
    let mut parts = repo.as_str().splitn(3, '/').skip(1);
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    )
}

fn unexpected() -> String {
    "gh gave an unexpected answer".to_owned()
}

#[cfg(test)]
mod tests;
