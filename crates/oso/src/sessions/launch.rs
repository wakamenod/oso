//! How a hosted session's `claude` starts (ADR 0121, Q22): as the person's login shell would
//! run it, so that their `PATH`, version managers and proxies apply.
//!
//! Must not depend on: `app` or the UI thread; [`launch`] may look on `PATH`, which stats
//! files, so it runs on the session's start thread.
//!
//! - For `sh`, `bash`, `zsh`, `dash` and `ksh`, by the shell's file name, the program is
//!   `SHELL -l -c SCRIPT CLAUDE ARGS…`. The arguments are positional parameters, never text
//!   inside the script, so nothing in a name or a brief is parsed by the shell.
//! - [`SCRIPT`] runs `[sessions] claude` as given. Else it looks for `claude` on the `PATH` the
//!   login shell set, as oso's own look does: only in absolute entries, and in none inside the
//!   session's directory.
//! - For any other shell (`fish`, `nu`), `claude` runs directly, with the environment the
//!   terminal gives: `[sessions] claude`, else `claude` on oso's own `PATH`, else the person is
//!   told to set `[sessions] claude`.

use std::ffi::OsString;
use std::path::Path;

use oso_platform::process::Executable;

/// The shells that run [`SCRIPT`] as POSIX has it, by file name.
const POSIX_SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "ksh"];

/// The script a POSIX login shell runs: its first operand as the program, the rest as its
/// arguments, each as given. An operand that is not an absolute path is looked for on `PATH`,
/// in its absolute entries outside the directory the session runs in, each compared by the
/// directory it is with links resolved; if none has it, the script says so and ends with 127.
pub const SCRIPT: &str = r#"case $0 in /*) exec "$0" "$@";; esac
here=$(pwd -P)
p=$PATH:
while [ -n "$p" ]; do
  d=${p%%:*}; p=${p#*:}
  case $d in /*) ;; *) continue ;; esac
  r=$(cd "$d" 2>/dev/null && pwd -P) || continue
  case $r in "$here"|"$here"/*) continue ;; esac
  if [ -f "$r/$0" ] && [ -x "$r/$0" ]; then exec "$r/$0" "$@"; fi
done
printf 'oso: no %s on an absolute PATH entry outside this directory; set [sessions] claude\n' "$0" >&2
exit 127"#;

/// The program's name when the login shell finds it on its own `PATH`.
const CLAUDE: &str = "claude";

/// What starts a session: a program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: Executable,
    pub args: Vec<OsString>,
}

/// Why a session cannot start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The shell is not one that runs [`SCRIPT`], and `claude` is neither configured nor on
    /// oso's `PATH`.
    NotFound,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str(
                "claude is not on oso's PATH; set [sessions] claude to its absolute path",
            ),
        }
    }
}

/// Whether `shell` runs [`SCRIPT`] as POSIX has it, by its file name.
pub fn is_posix(shell: &Path) -> bool {
    shell
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| POSIX_SHELLS.contains(&name))
}

/// What starts `claude` with `args` in a terminal whose shell is `shell`. `claude` is
/// `[sessions] claude`, if set; `project` is left out of the `PATH` searched.
pub fn launch(
    shell: &Executable,
    claude: Option<&Executable>,
    args: Vec<OsString>,
    project: Option<&Path>,
) -> Result<Launch, LaunchError> {
    if is_posix(shell.path()) {
        let program = claude.map_or_else(|| OsString::from(CLAUDE), |c| c.path().into());
        let mut wrapped: Vec<OsString> = vec!["-l".into(), "-c".into(), SCRIPT.into(), program];
        wrapped.extend(args);
        return Ok(Launch {
            program: shell.clone(),
            args: wrapped,
        });
    }
    let program = match claude {
        Some(claude) => claude.clone(),
        None => Executable::from_path(CLAUDE, project).map_err(|_| LaunchError::NotFound)?,
    };
    Ok(Launch { program, args })
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use oso_platform::process::{self, Executable, Sink, Source, Spawn};

    use super::{SCRIPT, is_posix, launch};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "oso-launch-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fake `claude` that writes each argument it got, NUL after each, to `$OUT`.
    fn fake_claude(dir: &Path) -> Executable {
        let path = dir.join("claude");
        std::fs::write(
            &path,
            "#!/bin/sh\nfor a; do printf '%s\\0' \"$a\"; done > \"$OUT\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Executable::absolute(path).unwrap()
    }

    /// Arguments a shell must not parse: substitutions, separators, quotes, a line break, an
    /// option's shape and bytes that are not UTF-8.
    fn hostile() -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "--name=$(touch pwned)",
            "`touch pwned`",
            "; touch pwned",
            "a\nb",
            "it's \"quoted\"",
            "-l",
            "",
            "$0 $@ $HOME",
        ]
        .map(OsString::from)
        .to_vec();
        args.push(OsString::from_vec(b"\xff\xfe".to_vec()));
        args
    }

    fn run(launch: &super::Launch, scratch: &Scratch) -> Vec<OsString> {
        assert!(status(launch, scratch), "{launch:?}");
        read_out(scratch)
    }

    /// Whether the launch ran and ended with 0.
    fn status(launch: &super::Launch, scratch: &Scratch) -> bool {
        let out = scratch.0.join("out");
        let home = scratch.0.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let set = [
            ("OUT", out.as_os_str()),
            // The login shell reads this home's profile, which is empty.
            ("HOME", home.as_os_str()),
        ];
        let mut child = process::spawn(&Spawn {
            executable: &launch.program,
            args: &launch.args,
            cwd: Some(&scratch.0),
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            set: &set,
            terminal: None,
        })
        .unwrap();
        child.wait().unwrap().success()
    }

    /// What the fake wrote of its arguments.
    fn read_out(scratch: &Scratch) -> Vec<OsString> {
        std::fs::read(scratch.0.join("out"))
            .unwrap()
            .split(|&b| b == 0)
            // Each argument ends with a NUL, so the last piece is empty.
            .map(|arg| OsStr::from_bytes(arg).to_owned())
            .collect::<Vec<_>>()
            .split_last()
            .map(|(_, args)| args.to_vec())
            .unwrap_or_default()
    }

    #[test]
    fn posix_shells_by_name() {
        for shell in [
            "/bin/sh",
            "/bin/bash",
            "/usr/bin/zsh",
            "/bin/dash",
            "/bin/ksh",
        ] {
            assert!(is_posix(Path::new(shell)), "{shell}");
        }
        for shell in ["/usr/bin/fish", "/opt/nu", "/bin/zsh5", "/bin/tcsh"] {
            assert!(!is_posix(Path::new(shell)), "{shell}");
        }
    }

    #[test]
    fn every_installed_posix_shell_passes_the_arguments_intact() {
        let mut ran = 0;
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh", "/bin/dash", "/bin/ksh"] {
            let Ok(shell) = Executable::absolute(shell) else {
                continue;
            };
            let scratch = Scratch::new(shell.path().file_name().unwrap().to_str().unwrap());
            let claude = fake_claude(&scratch.0);
            let launch = launch(&shell, Some(&claude), hostile(), None).unwrap();
            assert_eq!(launch.program, shell);
            assert_eq!(launch.args[..3], ["-l", "-c", SCRIPT].map(OsString::from));
            assert_eq!(
                run(&launch, &scratch),
                hostile(),
                "{}",
                shell.path().display()
            );
            assert!(!scratch.0.join("pwned").exists());
            ran += 1;
        }
        assert!(ran > 0, "no POSIX shell found");
    }

    /// A login profile, for each shell, that sets `PATH` to `path`.
    fn profile(home: &Path, path: &str) {
        std::fs::create_dir_all(home).unwrap();
        for file in [".profile", ".bash_profile", ".zprofile"] {
            std::fs::write(home.join(file), format!("PATH='{path}'\nexport PATH\n")).unwrap();
        }
    }

    #[test]
    fn a_bare_claude_is_found_only_on_absolute_path_entries_outside_the_session() {
        let mut ran = 0;
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh", "/bin/dash", "/bin/ksh"] {
            let Ok(shell) = Executable::absolute(shell) else {
                continue;
            };
            let name = shell.path().file_name().unwrap().to_str().unwrap();
            let session = Scratch::new(&format!("bare-{name}"));
            let elsewhere = Scratch::new(&format!("bare-elsewhere-{name}"));
            // The session's own `claude`, which must never run.
            let bin = session.0.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(
                bin.join("claude"),
                "#!/bin/sh\n/usr/bin/touch \"$OUT.pwned\"\n",
            )
            .unwrap();
            std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            let _ = fake_claude(&elsewhere.0);
            let launch = launch(&shell, None, hostile(), None).unwrap();
            let home = session.0.join("home");
            let path = format!("bin:{}:{}", bin.display(), elsewhere.0.display());
            profile(&home, &path);
            assert_eq!(run(&launch, &session), hostile(), "{name}");
            // With no other, nothing runs.
            profile(&home, &format!("bin:{}", bin.display()));
            assert!(!status(&launch, &session), "{name}");
            assert!(!session.0.join("out.pwned").exists(), "{name}");
            ran += 1;
        }
        assert!(ran > 0, "no POSIX shell found");
    }

    #[test]
    fn another_shell_runs_claude_directly() {
        let scratch = Scratch::new("fish");
        let claude = fake_claude(&scratch.0);
        let fish = scratch.0.join("fish");
        std::fs::write(&fish, "#!/bin/sh\nexit 9\n").unwrap();
        std::fs::set_permissions(&fish, std::fs::Permissions::from_mode(0o755)).unwrap();
        let fish = Executable::absolute(fish).unwrap();
        let launch = launch(&fish, Some(&claude), hostile(), None).unwrap();
        assert_eq!(launch.program, claude);
        assert_eq!(run(&launch, &scratch), hostile());
    }

    #[test]
    fn without_a_posix_shell_claude_must_be_found() {
        let scratch = Scratch::new("nu");
        let nu = scratch.0.join("nu");
        std::fs::write(&nu, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&nu, std::fs::Permissions::from_mode(0o755)).unwrap();
        let nu = Executable::absolute(nu).unwrap();
        // Looked for on oso's PATH, leaving out this directory as the project: found or not,
        // never a `claude` of the project's.
        let project = scratch.0.clone();
        let _ = fake_claude(&scratch.0);
        match launch(&nu, None, Vec::new(), Some(&project)) {
            Ok(launch) => assert!(!launch.program.path().starts_with(&project)),
            Err(e) => assert!(e.to_string().contains("[sessions] claude")),
        }
    }
}
