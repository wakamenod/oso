//! `gh`, as oso runs it (ADR 0121 for worktree sessions, ADR 0127 for pull requests): the
//! repository on a GitHub host a remote names, and each run bounded as git's runner bounds it.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread, nor on what uses it; this runs on
//! the threads that wait for `gh`.
//!
//! - `gh` runs in an empty private directory, naming the repository with `--repo` as its
//!   remote's address gives it, so it never runs git in the repository. It is asked only about
//!   `github.com`, or the host `GH_HOST` names.
//! - It gets by name the token for that host alone, its configuration and the proxy variables,
//!   is told to ask nothing and print no colour, and is killed past the time its caller gives.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::long::Long;
use super::{GitError, collect, printable, read_at_most};

/// The most bytes of `gh`'s answer read.
pub(crate) const MAX_GH: usize = 256 * 1024;
/// The GitHub host `gh` knows without `GH_HOST`.
const GITHUB: &str = "github.com";

/// Variables `gh` takes from oso's environment by name for any host: its configuration, and the
/// proxies and certificates it talks through.
const GH_INHERIT: [&str; 11] = [
    "GH_CONFIG_DIR",
    "XDG_CONFIG_HOME",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "ALL_PROXY",
];

/// The token variables `gh` takes for `github.com`.
const GH_GITHUB: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

/// The variables `gh` takes for the host `GH_HOST` names: the host and its tokens.
const GH_ENTERPRISE: [&str; 3] = ["GH_HOST", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];

/// What `gh` is told so that it asks nothing and prints nothing but its answer.
const GH_SET: [(&str, &str); 5] = [
    ("GH_PROMPT_DISABLED", "1"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1"),
    ("GH_SPINNER_DISABLED", "1"),
    ("NO_COLOR", "1"),
];

/// A repository on a GitHub host, as `gh --repo` takes it: `HOST/OWNER/REPO`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GhRepo(String);

impl GhRepo {
    /// The repository a remote's address names, in the https, ssh or scp form, if each part
    /// is a plain name and its host is `github.com` or `gh_host`, the host `GH_HOST` names.
    pub(crate) fn from_url(url: &str, gh_host: Option<&str>) -> Option<GhRepo> {
        let rest = if let Some(rest) = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("ssh://"))
        {
            // `[user@]host[:port]/owner/repo`
            let (host, path) = rest.split_once('/')?;
            let host = host.rsplit('@').next()?;
            let host = host.split(':').next()?;
            format!("{host}/{path}")
        } else {
            // `user@host:owner/repo`
            let (user_host, path) = url.split_once(':')?;
            let host = user_host.rsplit('@').next()?;
            format!("{host}/{path}")
        };
        let rest = rest.trim_end_matches('/');
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let parts: Vec<&str> = rest.split('/').collect();
        let plain = |part: &&str| {
            !part.is_empty()
                && !part.starts_with('-')
                && part != &"."
                && part != &".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        };
        if parts.len() != 3 || !parts.iter().all(plain) {
            return None;
        }
        let host = parts[0];
        let known = host.eq_ignore_ascii_case(GITHUB)
            || gh_host.is_some_and(|gh_host| host.eq_ignore_ascii_case(gh_host.trim()));
        known.then(|| GhRepo(parts.join("/")))
    }

    /// As `gh --repo` takes it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Its host, owner and name.
    pub(crate) fn parts(&self) -> (&str, &str, &str) {
        let mut parts = self.0.splitn(3, '/');
        let mut next = || parts.next().unwrap_or_default();
        (next(), next(), next())
    }

    /// The address of its issue `number`, as GitHub writes it (step 5b-1).
    pub(crate) fn issue_address(&self, number: u64) -> String {
        format!("https://{}/issues/{number}", self.0)
    }

    /// The address of its pull request `number`, as GitHub writes it.
    pub(crate) fn pull_address(&self, number: u64) -> String {
        format!("https://{}/pull/{number}", self.0)
    }

    /// Whether it is on `github.com`, not the host `GH_HOST` names.
    pub(crate) fn on_github(&self) -> bool {
        self.parts().0.eq_ignore_ascii_case(GITHUB)
    }
}

/// The variables `gh` takes from oso's environment to ask about `repo`: a host's tokens go only
/// to that host.
fn gh_inherit(repo: &GhRepo) -> Vec<&'static str> {
    let tokens: &[&str] = if repo.on_github() {
        &GH_GITHUB
    } else {
        &GH_ENTERPRISE
    };
    GH_INHERIT.iter().chain(tokens).copied().collect()
}

/// `gh`, found once.
#[derive(Clone, Debug)]
pub(crate) struct Gh {
    executable: Executable,
    /// The project it is asked about, left out of the `PATH` of `gh` and what it starts.
    project: Option<PathBuf>,
}

impl Gh {
    /// `gh` from `PATH`, leaving out `project` and what is under it, there and in its children's.
    pub(crate) fn find(project: &Path) -> Option<Gh> {
        let executable = dev_gh().or_else(|| Executable::from_path("gh", Some(project)).ok())?;
        Some(Gh {
            executable,
            project: Some(project.to_owned()),
        })
    }

    /// The `gh` at `executable`, for tests.
    #[cfg(all(test, unix))]
    pub(crate) fn at(executable: Executable) -> Gh {
        Gh {
            executable,
            project: None,
        }
    }

    /// Runs `gh` with `args` about `repo`, in `cwd`, an empty private directory, and gives its
    /// answer, at most `limit` bytes of it; a run past `within` is killed.
    ///
    /// # Errors
    ///
    /// Why `gh` gave no answer: it could not run, ran past `within`, or said more than `limit`.
    /// A `gh` that ran and failed is an answer, with its exit code and words.
    pub(crate) fn run(
        &self,
        repo: &GhRepo,
        args: &[&str],
        cwd: &Path,
        limit: usize,
        within: Duration,
    ) -> Result<GhAnswer, String> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let set: Vec<(&str, &OsStr)> = GH_SET
            .iter()
            .map(|(name, value)| (*name, OsStr::new(*value)))
            .collect();
        let child = spawn(&Spawn {
            executable: &self.executable,
            args: &args,
            cwd: Some(cwd),
            project: Some(self.project.as_deref().unwrap_or(cwd)),
            inherit: &gh_inherit(repo),
            stdin: Source::Closed,
            stdout: Sink::Piped,
            stderr: Sink::Piped,
            own_process_group: false,
            own_session: false,
            set: &set,
            terminal: None,
            pass_fd: None,
        })
        .map_err(|e| format!("cannot run gh: {e}"))?;
        let read = move |stdout| read_at_most(stdout, limit);
        let out = collect(child, None, read, within).map_err(|e| gh_said(&e, within))?;
        if out.cut {
            return Err("gh said more than oso reads".to_owned());
        }
        Ok(GhAnswer {
            code: out.status.code(),
            stdout: out.stdout,
            stderr: printable(out.stderr.trim(), 300),
        })
    }
}

impl Gh {
    /// Starts `gh` with `args` about `repo` in `cwd`, an empty private directory, `input` on its
    /// standard input if any, in a process group of its own with no time limit: a run the
    /// person stops, as git's long runs are (ADR 0127, step 5a).
    ///
    /// # Errors
    ///
    /// If `gh` cannot be started.
    pub(crate) fn start_long(
        &self,
        repo: &GhRepo,
        args: &[&str],
        cwd: &Path,
        input: Option<Vec<u8>>,
    ) -> Result<Long, String> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let set: Vec<(&str, &OsStr)> = GH_SET
            .iter()
            .map(|(name, value)| (*name, OsStr::new(*value)))
            .collect();
        let stdin = if input.is_some() {
            Source::Piped
        } else {
            Source::Closed
        };
        let child = spawn(&Spawn {
            executable: &self.executable,
            args: &args,
            cwd: Some(cwd),
            project: Some(self.project.as_deref().unwrap_or(cwd)),
            inherit: &gh_inherit(repo),
            stdin,
            stdout: Sink::Piped,
            stderr: Sink::Piped,
            own_process_group: true,
            own_session: false,
            set: &set,
            terminal: None,
            pass_fd: None,
        })
        .map_err(|e| format!("cannot run gh: {e}"))?;
        Long::adopt(child, input).map_err(|e| format!("cannot run gh: {e}"))
    }
}

/// A dev build's end-to-end flow's stand-in `gh`, at the absolute path `OSO_DEV_GH` names, as
/// `PATH` is the platform's own and cannot be given to an instance.
#[cfg(feature = "dev")]
fn dev_gh() -> Option<Executable> {
    Executable::absolute(std::env::var_os("OSO_DEV_GH")?).ok()
}

#[cfg(not(feature = "dev"))]
fn dev_gh() -> Option<Executable> {
    None
}

/// What `gh` answered: its exit code, what it wrote, and the start of its words on standard
/// error, escaped.
#[derive(Debug)]
pub(crate) struct GhAnswer {
    pub(crate) code: Option<i32>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

impl GhAnswer {
    /// What it wrote, if it succeeded; else its words.
    ///
    /// # Errors
    ///
    /// `gh failed: …` with its words, if it exited other than 0.
    pub(crate) fn into_stdout(self) -> Result<Vec<u8>, String> {
        if self.code == Some(0) {
            Ok(self.stdout)
        } else {
            Err(format!("gh failed: {}", self.stderr))
        }
    }
}

/// Text for the echo area from a failed `gh` run, in `gh`'s words.
fn gh_said(error: &GitError, within: Duration) -> String {
    let said = match error {
        GitError::Spawn(e) => format!("cannot run gh: {e}"),
        GitError::Io(e) => format!("cannot talk to gh: {e}"),
        GitError::TimedOut => format!("gh took more than {} s", within.as_secs()),
        GitError::TooLong => "gh said more than oso reads".to_owned(),
        GitError::Failed { stderr, .. } => format!("gh failed: {}", stderr.trim()),
        GitError::Stopped => "gh was stopped".to_owned(),
        GitError::NoCeiling | GitError::Unexpected(_) | GitError::BadRevision => {
            "gh gave an unexpected answer".to_owned()
        }
    };
    printable(&said, 400)
}

#[cfg(test)]
mod tests {
    use super::{GhRepo, gh_inherit};

    #[test]
    fn remote_addresses_name_a_repository() {
        for (url, repo) in [
            ("https://github.com/o/r.git", Some("github.com/o/r")),
            ("https://github.com/o/r", Some("github.com/o/r")),
            ("https://user@GitHub.com:443/o/r/", Some("GitHub.com/o/r")),
            ("git@github.com:o/r.git", Some("github.com/o/r")),
            ("/srv/git/r.git", None),
            ("file:///srv/r", None),
            ("https://github.com/o/r/extra", None),
            ("git@github.com:-o/r", None),
            ("git@github.com:o/r;rm", None),
            ("https://github.com/../r", None),
        ] {
            assert_eq!(
                GhRepo::from_url(url, None).as_ref().map(GhRepo::as_str),
                repo,
                "{url}"
            );
        }
    }

    #[test]
    fn gh_is_asked_only_about_github_or_gh_host() {
        let other = "ssh://git@evil.example/team/app.git";
        assert_eq!(GhRepo::from_url(other, None), None, "no GH_HOST");
        assert_eq!(GhRepo::from_url(other, Some("ghe.example.com")), None);
        let enterprise = GhRepo::from_url(other, Some("evil.example")).unwrap();
        assert_eq!(enterprise.as_str(), "evil.example/team/app");
        assert!(!enterprise.on_github());
        let github = GhRepo::from_url("https://github.com/o/r", Some("ghe.example.com")).unwrap();
        assert!(github.on_github());
    }

    #[test]
    fn a_hosts_tokens_go_only_to_that_host() {
        let github = gh_inherit(&GhRepo::from_url("https://github.com/o/r", None).unwrap());
        assert!(github.contains(&"GH_TOKEN") && github.contains(&"GITHUB_TOKEN"));
        assert!(!github.iter().any(|name| name.contains("ENTERPRISE")));
        assert!(!github.contains(&"GH_HOST"));
        let other = GhRepo::from_url("https://ghe.example.com/o/r", Some("ghe.example.com"));
        let enterprise = gh_inherit(&other.unwrap());
        assert!(enterprise.contains(&"GH_ENTERPRISE_TOKEN") && enterprise.contains(&"GH_HOST"));
        assert!(!enterprise.contains(&"GH_TOKEN") && !enterprise.contains(&"GITHUB_TOKEN"));
        assert!(enterprise.contains(&"HTTPS_PROXY"));
    }
}
