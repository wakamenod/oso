//! What a terminal starts (ADR 0120, Q18): which shell, with which arguments and environment.
//! The order of shells is `[terminal] shell`, `$SHELL` if absolute, the account's shell, then
//! `/bin/sh`.
//!
//! Must not depend on: `app` or `config`; the caller hands the settings in. Nothing here reads
//! the disk or the user database: [`resolve`] takes the lookups as functions, which the start
//! thread gives it, never the UI thread.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// The shell when nothing else names one.
pub(crate) const FALLBACK: &str = "/bin/sh";

/// A terminal's settings, from `[terminal]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// `shell`, an absolute path.
    pub shell: Option<PathBuf>,
    /// `login`: start the shell with `-l`.
    pub login: bool,
    /// `env`: names passed on from oso's environment.
    pub env: Vec<String>,
    /// `scrollback`, in lines.
    pub scrollback: usize,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            shell: None,
            login: true,
            env: Vec::new(),
            scrollback: oso_term::limits::SCROLLBACK_DEFAULT,
        }
    }
}

/// The shell found, and what was passed over on the way, for `*Messages*`.
#[derive(Debug)]
pub(crate) struct Resolved<E> {
    pub executable: E,
    pub path: PathBuf,
    pub passed_over: Vec<String>,
}

/// The shells to try, in order: `configured`, `$SHELL` if absolute, `account`'s, `/bin/sh`.
/// Each is tried once.
pub(crate) fn candidates(
    configured: Option<&Path>,
    env_shell: Option<&OsStr>,
    account: impl FnOnce() -> Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |path: PathBuf| {
        if path.is_absolute() && !out.contains(&path) {
            out.push(path);
        }
    };
    if let Some(path) = configured {
        add(path.to_owned());
    }
    if let Some(shell) = env_shell {
        add(PathBuf::from(shell));
    }
    if let Some(path) = account() {
        add(path);
    }
    add(PathBuf::from(FALLBACK));
    out
}

/// The first of `candidates` that `check` takes as an executable, and why each before it was
/// passed over.
///
/// # Errors
///
/// When none is an executable, the last one's error.
pub(crate) fn resolve<E>(
    candidates: Vec<PathBuf>,
    mut check: impl FnMut(&Path) -> io::Result<E>,
) -> io::Result<Resolved<E>> {
    let mut passed_over = Vec::new();
    let mut last = io::Error::from(io::ErrorKind::NotFound);
    for path in candidates {
        match check(&path) {
            Ok(executable) => {
                return Ok(Resolved {
                    executable,
                    path,
                    passed_over,
                });
            }
            Err(error) => {
                passed_over.push(format!("{}: {error}", path.display()));
                last = error;
            }
        }
    }
    Err(last)
}

/// The shell's arguments: `-l` for a login shell, else none, as an interactive shell reads its
/// startup files on its own.
pub(crate) fn args(login: bool) -> Vec<OsString> {
    if login {
        vec![OsString::from("-l")]
    } else {
        Vec::new()
    }
}

/// The variables a terminal sets by value over the process module's allowlist.
pub(crate) fn set(shell: &Path) -> Vec<(String, OsString)> {
    vec![
        ("SHELL".to_owned(), shell.as_os_str().to_owned()),
        ("TERM".to_owned(), OsString::from("xterm-256color")),
        ("COLORTERM".to_owned(), OsString::from("truecolor")),
        ("TERM_PROGRAM".to_owned(), OsString::from("oso")),
        (
            "TERM_PROGRAM_VERSION".to_owned(),
            OsString::from(env!("CARGO_PKG_VERSION")),
        ),
    ]
}

/// The names a terminal passes on from oso's environment besides the allowlist:
/// `SSH_AUTH_SOCK`, so that `ssh` finds its agent, then `[terminal] env`'s.
pub(crate) fn inherit(env: &[String]) -> Vec<String> {
    let mut names = vec!["SSH_AUTH_SOCK".to_owned()];
    for name in env {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{args, candidates, inherit, resolve, set};

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    // The paths are Unix's: on Windows, which has no terminals yet, none is absolute.
    #[cfg(unix)]
    #[test]
    fn the_order_is_the_setting_shell_the_account_then_sh() {
        let order = candidates(
            Some(Path::new("/opt/fish")),
            Some(OsStr::new("/bin/zsh")),
            || Some(PathBuf::from("/bin/bash")),
        );
        assert_eq!(
            order,
            paths(&["/opt/fish", "/bin/zsh", "/bin/bash", "/bin/sh"])
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_relative_shell_is_skipped_and_each_is_tried_once() {
        let order = candidates(None, Some(OsStr::new("zsh")), || {
            Some(PathBuf::from("/bin/sh"))
        });
        assert_eq!(order, paths(&["/bin/sh"]));
    }

    #[test]
    fn the_account_is_asked_once() {
        // The lookup runs once, on the start thread, never twice.
        let mut asked = 0;
        let _ = candidates(None, None, || {
            asked += 1;
            None
        });
        assert_eq!(asked, 1);
    }

    #[test]
    fn the_first_executable_wins_and_the_others_are_named() {
        let found = resolve(paths(&["/missing", "/bin/sh"]), |path| {
            if path == Path::new("/missing") {
                Err(io::Error::new(io::ErrorKind::NotFound, "not an executable"))
            } else {
                Ok(path.to_owned())
            }
        })
        .unwrap();
        assert_eq!(found.path, Path::new("/bin/sh"));
        assert_eq!(found.passed_over, ["/missing: not an executable"]);
        assert!(
            resolve(paths(&["/missing"]), |_| Err::<(), _>(io::Error::other(
                "no"
            )))
            .is_err()
        );
    }

    #[test]
    fn a_login_shell_takes_l() {
        assert_eq!(args(true), ["-l"]);
        assert!(args(false).is_empty());
    }

    #[test]
    fn the_environment_names_the_shell_and_the_terminal() {
        let vars = set(Path::new("/bin/zsh"));
        let get = |name: &str| {
            vars.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(get("SHELL").as_deref(), Some("/bin/zsh"));
        assert_eq!(get("TERM").as_deref(), Some("xterm-256color"));
        assert_eq!(get("COLORTERM").as_deref(), Some("truecolor"));
        assert_eq!(get("TERM_PROGRAM").as_deref(), Some("oso"));
        assert!(get("TERM_PROGRAM_VERSION").is_some());
        assert!(get("OSO_TERMINAL_TOKEN").is_none());
        assert_eq!(
            inherit(&["LANG".to_owned(), "SSH_AUTH_SOCK".to_owned()]),
            ["SSH_AUTH_SOCK", "LANG"]
        );
    }
}
