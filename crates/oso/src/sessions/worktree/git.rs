//! `git` and `gh` for worktree sessions (ADR 0121): a repository's main checkout, its linked
//! worktrees, and the pull requests `gh` knows of, each run bounded as ADR 0122's runner bounds
//! it.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; this runs on a worktree
//! job's thread and waits for git.
//!
//! - Every git run is [`review::git`](crate::review)'s: oso's own arguments and environment,
//!   no hook and no file system monitor of the repository's (the author's choice, ADR 0121
//!   step 4), `GIT_TERMINAL_PROMPT=0`, output bounded, killed after 10 s. Nothing here fetches.
//! - A linked worktree is run in only once its claim and the main repository's record of it
//!   are checked, and then with `GIT_DIR` and `GIT_WORK_TREE` given, so git looks for no
//!   repository of its own.
//! - `gh` runs in an empty private directory, naming the repository with `--repo` as its
//!   remote's address gives it, so it never runs git in the repository. It is asked only about
//!   `github.com`, or the host `GH_HOST` names. It gets by name the token for that host alone,
//!   its configuration and the proxy variables, and the same 10 s.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::Value;

use super::porcelain::{self, Listed};
use crate::project::linked_worktree;
use crate::review::git::{self as rgit, Git, GitError, MAX_SHORT};

/// The most bytes of a listing read: worktrees, branches, refs, paths.
pub const MAX_LISTING: usize = 4 * 1024 * 1024;
/// The most worktrees a repository's listing keeps.
pub const MAX_WORKTREES: usize = 256;
/// The most names a listing of branches or refs keeps.
pub const MAX_NAMES: usize = 10_000;
/// The most bytes of `gh`'s answer read.
pub const MAX_GH: usize = 256 * 1024;
/// How many merged pull requests of a branch `gh` is asked for.
const GH_LIMIT: &str = "30";

/// The GitHub host `gh` knows without `GH_HOST`.
pub const GITHUB: &str = "github.com";

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

/// A repository's main checkout, run in by git.
#[derive(Clone, Debug)]
pub struct Repo {
    git: Git,
    main: PathBuf,
    /// The empty private directory git takes for hooks, where `gh` runs.
    hooks: PathBuf,
}

/// Text for the echo area from a failed git run.
pub fn said(error: &GitError) -> String {
    porcelain::printable(&error.to_string(), 400)
}

impl Repo {
    /// git in the main checkout at `main`, a trusted project's root, taking `hooks`, an empty
    /// private directory, for its hooks.
    ///
    /// # Errors
    ///
    /// If git cannot be found, or kept from looking above `main`.
    pub fn open(main: &Path, hooks: &Path) -> Result<Repo, String> {
        let git = Git::new(main, hooks).map_err(|e| said(&e))?;
        Ok(Repo {
            git,
            main: main.to_owned(),
            hooks: hooks.to_owned(),
        })
    }

    /// The main checkout's root.
    pub fn main(&self) -> &Path {
        &self.main
    }

    /// git in the main checkout.
    pub fn git(&self) -> &Git {
        &self.git
    }

    /// The empty private directory.
    pub fn hooks(&self) -> &Path {
        &self.hooks
    }

    /// Runs git in the main checkout with `args`, keeping at most `limit` bytes.
    ///
    /// # Errors
    ///
    /// As [`Git::run`].
    pub fn run(&self, args: &[&str], limit: usize) -> Result<Vec<u8>, GitError> {
        run(&self.git, args, limit)
    }

    /// The repository's worktrees, the main one first, as git lists them.
    ///
    /// # Errors
    ///
    /// If git cannot list them.
    pub fn worktrees(&self) -> Result<Vec<Listed>, String> {
        let out = self
            .run(&["worktree", "list", "--porcelain", "-z"], MAX_LISTING)
            .map_err(|e| said(&e))?;
        Ok(porcelain::worktrees(&out, MAX_WORKTREES))
    }

    /// git in the linked worktree at `path`, if it is one of this repository's whose claim
    /// holds: its `.git` names this repository, which records it back.
    pub fn in_worktree(&self, path: &Path) -> Option<Git> {
        let entry = linked_worktree(&self.main, path)?;
        Some(self.git.for_worktree(&entry, path))
    }

    /// The local branches' short names.
    ///
    /// # Errors
    ///
    /// If git cannot list them.
    pub fn branches(&self) -> Result<Vec<String>, String> {
        let args = [
            "for-each-ref",
            "--format=%(refname:short)%00",
            "refs/heads/",
        ];
        let out = self.run(&args, MAX_LISTING).map_err(|e| said(&e))?;
        Ok(porcelain::names(&out, MAX_NAMES))
    }

    /// The remotes' names, `origin` first.
    pub fn remotes(&self) -> Vec<String> {
        let Ok(out) = self.run(&["remote"], MAX_SHORT * 4) else {
            return Vec::new();
        };
        let mut remotes: Vec<String> = String::from_utf8_lossy(&out)
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        remotes.sort_by_key(|name| name != "origin");
        remotes
    }

    /// The address of `remote`, as its configuration gives it.
    pub fn remote_url(&self, remote: &str) -> Option<String> {
        let out = self
            .run(&["remote", "get-url", "--", remote], MAX_SHORT)
            .ok()?;
        let url = String::from_utf8(out).ok()?;
        Some(url.trim().to_owned())
    }
}

/// Runs `git` with `args`, keeping at most `limit` bytes of its output.
///
/// # Errors
///
/// As [`Git::run`].
pub fn run(git: &Git, args: &[&str], limit: usize) -> Result<Vec<u8>, GitError> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git.run(&args, &[], limit)
}

/// Runs `git` with `args` and gives its exit status's code, as `merge-base --is-ancestor`
/// answers with it.
///
/// # Errors
///
/// If git cannot run or ran too long.
pub fn status(git: &Git, args: &[&str]) -> Result<Option<i32>, GitError> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    git.output(&args, &[], MAX_SHORT)
        .map(|out| out.status.code())
}

/// A repository on a GitHub host, as `gh --repo` takes it: `HOST/OWNER/REPO`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhRepo(String);

impl GhRepo {
    /// The repository a remote's address names, in the https, ssh or scp form, if each part
    /// is a plain name and its host is `github.com` or `gh_host`, the host `GH_HOST` names.
    pub fn from_url(url: &str, gh_host: Option<&str>) -> Option<GhRepo> {
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
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether it is on `github.com`, not the host `GH_HOST` names.
    pub fn on_github(&self) -> bool {
        self.0
            .split('/')
            .next()
            .is_some_and(|host| host.eq_ignore_ascii_case(GITHUB))
    }
}

/// The variables `gh` takes from oso's environment to ask about `repo`: a host's tokens go only
/// to that host.
pub fn gh_inherit(repo: &GhRepo) -> Vec<&'static str> {
    let tokens: &[&str] = if repo.on_github() {
        &GH_GITHUB
    } else {
        &GH_ENTERPRISE
    };
    GH_INHERIT.iter().chain(tokens).copied().collect()
}

/// A merged pull request of a branch, as `gh` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedPr {
    pub number: u64,
    /// Its head commit when it was merged.
    pub head: String,
    /// The branch it was merged into.
    pub base: String,
}

/// `gh`, found once.
#[derive(Clone, Debug)]
pub struct Gh {
    executable: Executable,
}

impl Gh {
    /// `gh` from `PATH`, leaving out `project` and what is under it.
    pub fn find(project: &Path) -> Option<Gh> {
        Executable::from_path("gh", Some(project))
            .ok()
            .map(|executable| Gh { executable })
    }

    /// The `gh` at `executable`, for tests.
    #[cfg(all(test, unix))]
    pub fn at(executable: Executable) -> Gh {
        Gh { executable }
    }

    /// The merged pull requests whose head is `branch` in `repo`, run in `cwd`, an empty
    /// private directory.
    ///
    /// # Errors
    ///
    /// Why `gh` gave no answer: it could not run, failed, ran past 10 s, or said something
    /// that is not the JSON asked for.
    pub fn merged(&self, repo: &GhRepo, branch: &str, cwd: &Path) -> Result<Vec<MergedPr>, String> {
        let args: Vec<OsString> = [
            "pr",
            "list",
            "--repo",
            repo.as_str(),
            "--head",
            branch,
            "--state",
            "merged",
            "--json",
            "number,headRefOid,baseRefName",
            "--limit",
            GH_LIMIT,
        ]
        .map(OsString::from)
        .to_vec();
        let set: Vec<(&str, &OsStr)> = GH_SET
            .iter()
            .map(|(name, value)| (*name, OsStr::new(*value)))
            .collect();
        let child = spawn(&Spawn {
            executable: &self.executable,
            args: &args,
            cwd: Some(cwd),
            project: Some(cwd),
            inherit: &gh_inherit(repo),
            stdin: Source::Closed,
            stdout: Sink::Piped,
            stderr: Sink::Piped,
            own_process_group: false,
            set: &set,
            terminal: None,
        })
        .map_err(|e| format!("cannot run gh: {e}"))?;
        let out = rgit::collect(child, None, |stdout| rgit::read_at_most(stdout, MAX_GH))
            .map_err(|e| gh_said(&e))?;
        if out.cut {
            return Err("gh said more than oso reads".to_owned());
        }
        if !out.status.success() {
            let why = porcelain::printable(out.stderr.trim(), 300);
            return Err(format!("gh failed: {why}"));
        }
        parse_merged(&out.stdout).ok_or_else(|| "gh gave an unexpected answer".to_owned())
    }
}

/// Text for the echo area from a failed `gh` run, in `gh`'s words.
fn gh_said(error: &GitError) -> String {
    let said = match error {
        GitError::Spawn(e) => format!("cannot run gh: {e}"),
        GitError::Io(e) => format!("cannot talk to gh: {e}"),
        GitError::TimedOut => format!("gh took more than {} s", rgit::RUN_LIMIT.as_secs()),
        GitError::TooLong => "gh said more than oso reads".to_owned(),
        GitError::Failed { stderr, .. } => format!("gh failed: {}", stderr.trim()),
        GitError::NoCeiling | GitError::Unexpected(_) | GitError::BadRevision => {
            "gh gave an unexpected answer".to_owned()
        }
    };
    porcelain::printable(&said, 400)
}

/// The pull requests in `gh pr list --json number,headRefOid,baseRefName`'s answer; `None` if it
/// is not that shape. Entries missing a field are left out.
pub fn parse_merged(json: &[u8]) -> Option<Vec<MergedPr>> {
    let Value::Array(items) = serde_json::from_slice::<Value>(json).ok()? else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|item| {
                Some(MergedPr {
                    number: item.get("number")?.as_u64()?,
                    head: item.get("headRefOid")?.as_str()?.to_owned(),
                    base: item.get("baseRefName")?.as_str()?.to_owned(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{GhRepo, MergedPr, gh_inherit, parse_merged};

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

    #[test]
    fn gh_answers_are_read_by_shape() {
        let json = br#"[{"number":12,"headRefOid":"abc","baseRefName":"main"},{"number":"x"}]"#;
        assert_eq!(
            parse_merged(json),
            Some(vec![MergedPr {
                number: 12,
                head: "abc".into(),
                base: "main".into()
            }])
        );
        assert_eq!(parse_merged(b"[]"), Some(Vec::new()));
        assert_eq!(parse_merged(b"{}"), None);
        assert_eq!(parse_merged(b"not json"), None);
    }
}
