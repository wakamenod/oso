//! Pushes, pulls and fetches on the repository's run thread (ADR 0127, step 2): where a push
//! goes, read from git's own configuration as the person asked, and the runs themselves.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on the repository's
//! run thread and waits for git.
//!
//! - `p` follows `@{push}` and `u` the upstream, as `for-each-ref` names them; with none, the
//!   UI is given the remotes to pick from, and nothing is written to the configuration but
//!   what `--set-upstream` sets for `u` (Q4).
//! - A remote named in a prompt must be one `git remote` lists; a branch must pass
//!   [`remote::check_branch`].
//! - A forced push carries the tip of the local ref that tracks where it goes, read here, as
//!   its lease, and the commits that tip holds and `HEAD` does not, for the question.
//! - Each run gets the repository's own hooks (`pre-push`, `post-merge`, the reference
//!   transaction's), the proxies and no controlling terminal ([`Git::for_remote`]).
//! - A pull fast-forwards only, unless the menu's `-r` rebases or `-m` merges (step 3a); one
//!   that stops on a conflict says so, and the status shows it stopped.

use std::ffi::OsStr;

use oso_core::editor::{FetchFrom, PullHow, PushSwitches, row_text};

use super::sequence::stopped_now;
use super::stream::stream;
use super::{RunEnd, RunReport, Watch};
use crate::git::{Git, MAX_SHORT, trim_line};
use crate::vcs::remote::{
    self, MAX_DROPPED, Push, Tracking, check_branch, fetch_title, pull_title, split_target,
};
use crate::vcs::sequence;

/// The most remotes, or remote branches, a prompt is given: the most a completion opens with on
/// the UI thread before its list is built on the worker (`complete::WORKER_PAST`).
const MAX_LISTED: usize = 5000;
/// The most bytes of a list of remotes or remote branches read.
const MAX_LIST: usize = 1024 * 1024;
/// The most characters of a dropped commit's subject a question names.
const MAX_SUBJECT: usize = 60;

/// Where a push was asked to go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    PushRemote,
    Upstream,
    /// The remote branches to pick from, for `e`.
    Elsewhere,
    /// The branch's own name on `remote`, picked in a prompt; set as the upstream if
    /// `set_upstream`.
    Remote {
        remote: String,
        set_upstream: bool,
    },
    /// `REMOTE/BRANCH`, typed or picked in a prompt.
    Branch(String),
}

/// What planning a push found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Planned {
    Ready(Push),
    /// No target is set: the remotes to pick from, and whether the one picked becomes the
    /// upstream.
    Remote {
        remotes: Vec<String>,
        set_upstream: bool,
    },
    /// The remote branches to pick from, each `REMOTE/BRANCH`, the branch's own name on each
    /// remote first.
    Branch {
        targets: Vec<String>,
    },
}

/// Plans the push `want` asks for with `switches`, or says why there is none.
pub(super) fn plan(git: &Git, want: Want, switches: PushSwitches) -> Result<Planned, String> {
    let branch = current_branch(git)?;
    let refs = format!("refs/heads/{branch}");
    let tracking = tracking(git, &branch);
    let own = |remote: &str| {
        (
            refs.clone(),
            Some(format!("refs/remotes/{remote}/{branch}")),
        )
    };
    let (remote, (there, track), set_upstream) = match want {
        Want::PushRemote | Want::Upstream => {
            let set_upstream = want == Want::Upstream;
            let end = if set_upstream {
                tracking.upstream
            } else {
                tracking.push
            };
            let Some(end) = end else {
                let remotes = remotes(git)?;
                if remotes.is_empty() {
                    return Err("The repository has no remote: git remote add makes one".into());
                }
                return Ok(Planned::Remote {
                    remotes,
                    set_upstream,
                });
            };
            let there = end.there.unwrap_or_else(|| refs.clone());
            (end.remote, (there, end.track), false)
        }
        Want::Elsewhere => {
            return Ok(Planned::Branch {
                targets: targets(git, &branch)?,
            });
        }
        Want::Remote {
            remote,
            set_upstream,
        } => {
            if !remotes(git)?.contains(&remote) {
                return Err(format!("No remote here is named {}", row_text(&remote)));
            }
            let own = own(&remote);
            (remote, own, set_upstream)
        }
        Want::Branch(answer) => {
            let remotes = remotes(git)?;
            let (remote, name) = split_target(answer.trim(), &remotes)?;
            let track = Some(format!("refs/remotes/{remote}/{name}"));
            (
                remote.to_owned(),
                (format!("refs/heads/{name}"), track),
                false,
            )
        }
    };
    let head = tip_of(git, &refs).ok_or_else(|| format!("{branch} has no commit to push"))?;
    let mut push = Push {
        head,
        branch,
        remote,
        there,
        set_upstream,
        switches,
        lease: None,
        dropped: None,
    };
    if switches.force_with_lease {
        let tip = track.as_deref().and_then(|track| tip_of(git, track));
        // With no tip seen, the lease says the remote holds none: nothing there is dropped.
        push.dropped = match &tip {
            Some(tip) => dropped(git, tip),
            None => Some((Vec::new(), 0)),
        };
        push.lease = Some(tip.unwrap_or_default());
    }
    Ok(Planned::Ready(push))
}

/// Runs `push` with the names `pass` passes on to its hooks. A forced one is refused if the
/// branch moved since it was planned, as its question named what it would drop then.
pub(super) fn push(
    git: &Git,
    push: &Push,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    let now = tip_of(git, &format!("refs/heads/{}", push.branch));
    if push.switches.force_with_lease && now.as_deref() != Some(push.head.as_str()) {
        return RunEnd::Refused(format!(
            "{} moved since the question: P -f asks again",
            push.branch
        ));
    }
    run(
        &git.for_remote(pass),
        &push.args(),
        &push.title(),
        watch,
        send,
        None,
    )
}

/// Pulls the upstream as `how` says, with the names `pass` passes on to its hooks.
pub(super) fn pull(
    git: &Git,
    how: PullHow,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    let branch = match current_branch(git) {
        Ok(branch) => branch,
        Err(why) => return RunEnd::Refused(why),
    };
    if tracking(git, &branch).upstream.is_none() {
        return RunEnd::Refused(format!("{branch} has no upstream: P u sets one"));
    }
    // Each says how in whole, so that neither `pull.rebase` nor `pull.ff` changes it:
    // `--no-rebase` keeps a fast-forward from rebasing, an explicit `--rebase` is never
    // interactive, and `--ff` merges where `pull.ff=only` would refuse.
    let words: &[&str] = match how {
        PullHow::FfOnly => &["pull", "--ff-only", "--no-rebase", "--progress"],
        PullHow::Rebase => &["pull", "--rebase", "--progress"],
        PullHow::Merge => &["pull", "--no-rebase", "--ff", "--no-edit", "--progress"],
    };
    let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    // A rebase or a merge may stop: the read after it is a read's, with the plain git.
    let reads = (how != PullHow::FfOnly).then_some(git);
    run(
        &git.for_remote(pass),
        &words,
        pull_title(how),
        watch,
        send,
        reads,
    )
}

/// Fetches from `from`, pruning, with the names `pass` passes on to its hooks.
pub(super) fn fetch(
    git: &Git,
    from: FetchFrom,
    pass: &[String],
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
) -> RunEnd {
    let mut words: Vec<String> = ["fetch", "--prune", "--progress"]
        .map(str::to_owned)
        .to_vec();
    match from {
        FetchFrom::All => words.push("--all".into()),
        FetchFrom::Upstream => {
            let branch = match current_branch(git) {
                Ok(branch) => branch,
                Err(why) => return RunEnd::Refused(why),
            };
            let Some(upstream) = tracking(git, &branch).upstream else {
                return RunEnd::Refused(format!(
                    "{branch} has no upstream: f a fetches every remote"
                ));
            };
            words.extend(["--".to_owned(), upstream.remote]);
        }
    }
    run(
        &git.for_remote(pass),
        &words,
        fetch_title(from),
        watch,
        send,
        None,
    )
}

/// Runs a remote's `words`, called `title`, to its end; one that may leave a rebase or a merge
/// stopped is given `reads`, the plain git, to tell after a failure, which then names it.
fn run(
    git: &Git,
    words: &[String],
    title: &str,
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
    reads: Option<&Git>,
) -> RunEnd {
    let streamed = match stream(git, words, None, watch, send, true) {
        Ok(streamed) => streamed,
        Err(why) => return RunEnd::Refused(why),
    };
    match streamed.status {
        _ if streamed.stopped => RunEnd::Stopped,
        Ok(status) if status.success() => RunEnd::Done,
        Ok(_) => RunEnd::Failed {
            said: match reads {
                Some(reads) => {
                    sequence::failed(title, &streamed.tail, stopped_now(reads).is_some())
                }
                None => remote::failed(title, &streamed.tail),
            },
            hooks: Vec::new(),
        },
        Err(why) => RunEnd::Failed {
            said: format!("{title} failed: {why}"),
            hooks: Vec::new(),
        },
    }
}

/// The branch checked out, or why there is none to push or pull.
fn current_branch(git: &Git) -> Result<String, String> {
    // Not `--short`, which a tag of the same name would make `heads/NAME`.
    let words = ["symbolic-ref", "--quiet", "HEAD"].map(OsStr::new);
    let detached = || "HEAD is detached: check out a branch first".to_owned();
    let out = git.run(&words, &[], MAX_SHORT).map_err(|_| detached())?;
    let full = std::str::from_utf8(trim_line(&out))
        .map_err(|_| "The branch's name is not UTF-8: use a shell".to_owned())?;
    let branch = full
        .strip_prefix("refs/heads/")
        .ok_or_else(detached)?
        .to_owned();
    check_branch(&branch)?;
    Ok(branch)
}

/// Where `branch` pushes and pulls; nothing if git cannot say.
fn tracking(git: &Git, branch: &str) -> Tracking {
    let refs = format!("refs/heads/{branch}");
    let words = ["for-each-ref", remote::TRACKING_FORMAT, &refs];
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    git.run(&words, &[], MAX_SHORT)
        .map(|out| Tracking::parse(&out))
        .unwrap_or_default()
}

/// The remotes `git remote` lists, at most [`MAX_LISTED`], each a name with no control
/// character.
fn remotes(git: &Git) -> Result<Vec<String>, String> {
    let out = git
        .run(&[OsStr::new("remote")], &[], MAX_LIST)
        .map_err(|e| e.to_string())?;
    Ok(listed(&out, |_| true))
}

/// What `e` picks from: `REMOTE/BRANCH` for `branch` on each remote, then each remote branch
/// the repository tracks, but `HEAD`s.
fn targets(git: &Git, branch: &str) -> Result<Vec<String>, String> {
    let remotes = remotes(git)?;
    if remotes.is_empty() {
        return Err("The repository has no remote: git remote add makes one".into());
    }
    let words = [
        "for-each-ref",
        "--format=%(refname:lstrip=2)",
        "refs/remotes",
    ]
    .map(OsStr::new);
    let out = git.run(&words, &[], MAX_LIST).unwrap_or_default();
    let mut targets: Vec<String> = (remotes.iter())
        .map(|remote| format!("{remote}/{branch}"))
        .collect();
    let own = targets.len();
    for tracked in listed(&out, |name| !name.ends_with("/HEAD")) {
        // `for-each-ref` lists each name once: only the branch's own names may come again.
        if !targets[..own].contains(&tracked) && targets.len() < MAX_LISTED {
            targets.push(tracked);
        }
    }
    Ok(targets)
}

/// The lines of `out` that are UTF-8 with no control character, that `keep` keeps, at most
/// [`MAX_LISTED`].
fn listed(out: &[u8], keep: impl Fn(&str) -> bool) -> Vec<String> {
    (out.split(|&byte| byte == b'\n'))
        .filter_map(|line| std::str::from_utf8(line).ok())
        .filter(|line| !line.is_empty() && remote::plain(line) && keep(line))
        .take(MAX_LISTED)
        .map(str::to_owned)
        .collect()
}

/// The commit the local ref `track` is at, as hex, if it is one.
fn tip_of(git: &Git, track: &str) -> Option<String> {
    let at = format!("{track}^{{commit}}");
    let words = ["rev-parse", "--verify", "--quiet", &at];
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let out = git.run(&words, &[], MAX_SHORT).ok()?;
    let id = std::str::from_utf8(trim_line(&out)).ok()?;
    (!id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| id.to_owned())
}

/// The commits `tip` holds and `HEAD` does not: the first [`MAX_DROPPED`], each `ID SUBJECT` on
/// one line, and how many in all; `None` if git cannot count them, which the question says
/// rather than that it drops nothing.
fn dropped(git: &Git, tip: &str) -> Option<(Vec<String>, usize)> {
    let range = format!("HEAD..{tip}");
    let count = ["rev-list", "--count", &range, "--"];
    let count: Vec<&OsStr> = count.iter().map(OsStr::new).collect();
    let all: usize = (git.run(&count, &[], MAX_SHORT).ok())
        .and_then(|out| std::str::from_utf8(trim_line(&out)).ok()?.parse().ok())?;
    let most = format!("--max-count={MAX_DROPPED}");
    let words = [
        "log",
        "--no-show-signature",
        "--no-color",
        "--format=%h %s",
        &most,
        &range,
        "--",
    ];
    let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let out = git.run(&words, &[], MAX_LIST).unwrap_or_default();
    let named = (String::from_utf8_lossy(&out).lines())
        .filter(|line| !line.is_empty())
        .map(|line| row_text(&line.chars().take(MAX_SUBJECT).collect::<String>()))
        .take(MAX_DROPPED)
        .collect();
    Some((named, all))
}
