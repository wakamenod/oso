//! What a push, a pull or a fetch needs that runs no git (ADR 0127, step 2): where the branch
//! pushes and pulls as `for-each-ref` says, the arguments of a push, a `REMOTE/BRANCH` told
//! apart, a branch name checked, the progress git writes, and why one failed for want of a
//! password or a key.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these are plain functions of
//! git's output, which is untrusted: nothing here panics or grows past its input.
//!
//! - A push names its refs in full, `refs/heads/BRANCH:refs/heads/THERE`, after `--` and the
//!   remote, so that no name is read as an option or a pattern. A remote typed or picked must
//!   be one `git remote` lists; one from git's configuration is taken as git takes it, and
//!   shown with any user and password of an address left out.
//! - A name from git or a prompt holding a control or bidirectional character is refused, so
//!   that a question or the echo area shows it as it is.
//! - A forced push is `--force-with-lease=THERE:ID`, the remote's tip as oso last saw it, or
//!   `THERE:` when oso saw none, so that git refuses it if the remote moved since: never a
//!   plain `--force`.

use oso_core::editor::PushSwitches;

/// The most bytes of the end of a run's output looked through for its progress or its failure.
pub(crate) const MAX_LOOKED: usize = 16 * 1024;
/// The most characters of progress the echo area shows.
const MAX_PROGRESS: usize = 60;

/// What the log and the echo area call a pull.
pub(crate) fn pull_title(how: oso_core::editor::PullHow) -> &'static str {
    match how {
        oso_core::editor::PullHow::FfOnly => "git pull --ff-only",
        oso_core::editor::PullHow::Rebase => "git pull --rebase",
        oso_core::editor::PullHow::Merge => "git pull --no-rebase",
    }
}

/// What the log and the echo area call a fetch from `from`.
pub(crate) fn fetch_title(from: oso_core::editor::FetchFrom) -> &'static str {
    match from {
        oso_core::editor::FetchFrom::Upstream => "git fetch",
        oso_core::editor::FetchFrom::All => "git fetch --all",
    }
}

/// The `for-each-ref` format of [`Tracking::parse`]: the push target's remote, its ref there and
/// the local ref that tracks it, then the upstream's, NUL-separated.
pub(crate) const TRACKING_FORMAT: &str = "--format=%(push:remotename)%00%(push:remoteref)%00\
     %(push)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream)";

/// A remote's branch the branch checked out pushes to or pulls from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct End {
    pub(crate) remote: String,
    /// Its ref on the remote, `refs/heads/…`, if git or the local ref that tracks it says.
    /// git leaves `%(push:remoteref)` empty under `push.default=simple`, so it is then read
    /// from the tracking ref, `refs/remotes/REMOTE/…`; with neither, the caller takes the
    /// branch's own name, which `simple` and `current` push to.
    pub(crate) there: Option<String>,
    /// The local ref that tracks it, `refs/remotes/…`, if any.
    pub(crate) track: Option<String>,
}

/// Where the branch checked out pushes and pulls, as git's configuration says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tracking {
    /// `@{push}`, which `push.default` and the push remotes decide.
    pub(crate) push: Option<End>,
    pub(crate) upstream: Option<End>,
}

impl Tracking {
    /// Reads what `for-each-ref` printed with [`TRACKING_FORMAT`] for one branch. A part that is
    /// not UTF-8, empty or does not name a branch leaves its end unset.
    pub(crate) fn parse(out: &[u8]) -> Tracking {
        let out = out.strip_suffix(b"\n").unwrap_or(out);
        let parts: Vec<&[u8]> = out.split(|&byte| byte == 0).collect();
        let end = |at: usize| -> Option<End> {
            let text = |part: Option<&&[u8]>| {
                (part.map(|part| std::str::from_utf8(part)))
                    .and_then(Result::ok)
                    .filter(|text| !text.is_empty() && plain(text))
                    .map(str::to_owned)
            };
            let remote = text(parts.get(at))?;
            let track = text(parts.get(at + 2)).filter(|track| track.starts_with("refs/"));
            let tracked = (track.as_deref())
                .and_then(|track| {
                    track
                        .strip_prefix("refs/remotes/")?
                        .strip_prefix(remote.as_str())?
                        .strip_prefix('/')
                })
                .filter(|name| check_branch(name).is_ok())
                .map(|name| format!("refs/heads/{name}"));
            let there = text(parts.get(at + 1))
                .filter(|there| {
                    there
                        .strip_prefix("refs/heads/")
                        .is_some_and(|name| check_branch(name).is_ok())
                })
                .or(tracked);
            Some(End {
                remote,
                there,
                track,
            })
        };
        if parts.len() != 6 {
            return Tracking::default();
        }
        Tracking {
            push: end(0),
            upstream: end(3),
        }
    }
}

/// A push to run, as the run thread planned it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Push {
    /// The branch checked out, without `refs/heads/`.
    pub(crate) branch: String,
    /// The commit it was at as the push was planned, which a forced push's question was about.
    pub(crate) head: String,
    pub(crate) remote: String,
    /// The ref it goes to on the remote, `refs/heads/…`.
    pub(crate) there: String,
    /// `--set-upstream`.
    pub(crate) set_upstream: bool,
    pub(crate) switches: PushSwitches,
    /// For a forced push, the remote's tip as oso last saw it, empty if it saw none.
    pub(crate) lease: Option<String>,
    /// For a forced push, the commits the remote holds that `HEAD` does not, which it drops:
    /// each `ID SUBJECT`, one line, at most [`MAX_DROPPED`], and how many in all; `None` if git
    /// could not say.
    pub(crate) dropped: Option<(Vec<String>, usize)>,
}

/// The most commits a forced push's question names.
pub(crate) const MAX_DROPPED: usize = 5;

impl Push {
    /// git's arguments, after oso's own.
    pub(crate) fn args(&self) -> Vec<String> {
        let mut words: Vec<String> = ["push", "-v", "--progress"].map(str::to_owned).to_vec();
        if self.switches.no_verify {
            words.push("--no-verify".into());
        }
        if self.set_upstream {
            words.push("--set-upstream".into());
        }
        if self.switches.force_with_lease {
            let lease = self.lease.as_deref().unwrap_or_default();
            words.push(format!("--force-with-lease={}:{lease}", self.there));
        }
        words.push("--".into());
        words.push(self.remote.clone());
        words.push(format!("refs/heads/{}:{}", self.branch, self.there));
        words
    }

    /// Where it goes, as the echo area names it: `REMOTE/BRANCH`.
    pub(crate) fn destination(&self) -> String {
        let there = self
            .there
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.there);
        format!("{}/{there}", shown_remote(&self.remote))
    }

    /// What the log and the echo area call it.
    pub(crate) fn title(&self) -> String {
        let force = if self.switches.force_with_lease {
            " --force-with-lease"
        } else {
            ""
        };
        format!("git push{force} {}", self.destination())
    }

    /// The question before a forced push.
    pub(crate) fn question(&self) -> String {
        let at = self.destination();
        let Some((named, count)) = &self.dropped else {
            return format!(
                "Force-push {} to {at}? oso could not tell what it drops there",
                self.branch
            );
        };
        match count {
            0 => format!("Force-push {} to {at}? It drops nothing there", self.branch),
            n if named.is_empty() => {
                format!(
                    "Force-push {} to {at}, dropping {n} commits there?",
                    self.branch
                )
            }
            n => {
                let commits = if *n == 1 { "commit" } else { "commits" };
                let more = n.saturating_sub(named.len());
                let more = if more > 0 {
                    format!(", and {more} more")
                } else {
                    String::new()
                };
                format!(
                    "Force-push {} to {at}, dropping {n} {commits} there: {}{more}?",
                    self.branch,
                    named.join("; ")
                )
            }
        }
    }
}

/// Whether `text` holds no control and no bidirectional character, so that it shows as it is.
pub(crate) fn plain(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_control() || super::commit::is_bidi_format(c))
}

/// `remote` as the echo area shows it: an address's user and password, if it has any, left out.
pub(crate) fn shown_remote(remote: &str) -> String {
    let Some((scheme, rest)) = remote.split_once("://") else {
        return remote.to_owned();
    };
    let host_end = rest.find('/').unwrap_or(rest.len());
    match rest[..host_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => remote.to_owned(),
    }
}

/// The remote and the branch `answer`, `REMOTE/BRANCH`, names, by the longest of `remotes` it
/// starts with, as a remote's name may hold `/`; or why it names none.
pub(crate) fn split_target<'a>(
    answer: &'a str,
    remotes: &[String],
) -> Result<(&'a str, &'a str), String> {
    let found = (remotes.iter())
        .filter_map(|remote| {
            let rest = answer.strip_prefix(remote.as_str())?.strip_prefix('/')?;
            Some((&answer[..remote.len()], rest))
        })
        .max_by_key(|(remote, _)| remote.len());
    let (remote, branch) = found.ok_or_else(|| {
        format!(
            "Not REMOTE/BRANCH of a remote here: {}",
            oso_core::editor::row_text(answer)
        )
    })?;
    check_branch(branch)?;
    Ok((remote, branch))
}

/// Refuses, before git sees it, a branch name that could be taken as anything else on a command
/// line or that git's `check-ref-format --branch` refuses for its shape: empty, starting with `-`
/// or `/`, ending with `/` or `.`, a part starting with `.` or ending with `.lock`, holding `..`,
/// `@{`, `//`, white space, a control or bidirectional character or one of `~^:?*[\`, or `@`
/// alone. It is a
/// stricter filter than git's own, which still checks what it is given.
pub(crate) fn check_branch(name: &str) -> Result<(), String> {
    let bad = name.is_empty()
        || name == "@"
        || name.starts_with(['-', '/'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
        || name
            .split('/')
            .any(|part| part.starts_with('.') || part.ends_with(".lock"))
        || !plain(name)
        || name
            .chars()
            .any(|c| c.is_whitespace() || "~^:?*[\\".contains(c));
    if bad {
        return Err(format!(
            "Not a branch name git takes: {}",
            oso_core::editor::row_text(name)
        ));
    }
    Ok(())
}

/// The progress the end of `output`, what git wrote, shows last, as `Writing objects 42%`: the
/// last line, or part a carriage return began, that holds a percentage after a phase's name.
/// `None` if none does. Looks at the last [`MAX_LOOKED`] bytes only.
pub(crate) fn progress(output: &[u8]) -> Option<String> {
    let from = output.len().saturating_sub(MAX_LOOKED);
    let text = String::from_utf8_lossy(&output[from..]);
    let part = text
        .rsplit(['\r', '\n'])
        // The server's own lines (`remote: …`) are its words, not git's: they are not echoed.
        .find(|part| {
            part.contains('%') && part.contains(':') && !part.trim_start().starts_with("remote:")
        })?;
    let (phase, rest) = part.rsplit_once(':')?;
    let percent = rest.trim_start().split('%').next()?.trim();
    if percent.is_empty() || percent.len() > 3 || !percent.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let phase: String = phase
        .chars()
        .filter(|c| !c.is_control() && !super::commit::is_bidi_format(*c))
        .take(MAX_PROGRESS)
        .collect();
    let phase = phase.trim();
    if phase.is_empty() {
        return None;
    }
    Some(format!("{phase} {percent}%"))
}

/// Why a remote's run failed for want of something no one can type there, from what git, `ssh`
/// or a credential helper wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wanted {
    /// A password, a passphrase or a key.
    Credentials,
    /// `ssh` could not check the server's key.
    HostKey,
}

/// What a failed run wrote, the end of `output`, says it wanted, if anything.
pub(crate) fn wanted(output: &str) -> Option<Wanted> {
    let from = output.floor_char_boundary(output.len().saturating_sub(MAX_LOOKED));
    let text = output[from..].to_lowercase();
    const CREDENTIALS: [&str; 9] = [
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "permission denied (publickey",
        "authentication failed",
        "can't open /dev/tty",
        "ssh_askpass",
        "read_passphrase",
        "invalid username or password",
    ];
    if text.contains("host key verification failed") {
        return Some(Wanted::HostKey);
    }
    CREDENTIALS
        .iter()
        .any(|needle| text.contains(needle))
        .then_some(Wanted::Credentials)
}

/// What the echo area says of a remote's run, `title`, that failed, from what it wrote.
pub(crate) fn failed(title: &str, output: &str) -> String {
    match wanted(output) {
        Some(Wanted::Credentials) => "git asked for a password; set up a credential helper or \
                                      ssh-agent, or push from a terminal"
            .to_owned(),
        Some(Wanted::HostKey) => "ssh could not check the server's key: connect once from a \
                                  terminal, which asks to trust it"
            .to_owned(),
        None => {
            let from = output.floor_char_boundary(output.len().saturating_sub(MAX_LOOKED));
            // A server's own error (`fatal: remote error: …`) is its words: not echoed.
            let line = output[from..].lines().rev().find(|line| {
                let line = line.trim_start();
                (line.starts_with("fatal: ")
                    || line.starts_with("error: ")
                    || line.starts_with("! ["))
                    && !line.contains("remote error:")
            });
            match line {
                Some(line) => format!("{title} failed: {}; $ shows the output", line.trim()),
                None => format!("{title} failed: $ shows the output"),
            }
        }
    }
}

#[cfg(test)]
mod tests;
