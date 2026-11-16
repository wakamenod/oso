//! Finding a language server's program (ADR 0138, Which servers and Windows), on the server's
//! thread before each start: an absolute path as it is, or a name on `PATH` without the
//! project, then, for an entry that says so, in Cargo's `bin` directory, which is found from
//! oso's own environment and home, never from the project. On Windows only an `.exe` starts.
//!
//! Must not depend on: the UI thread or the editor; this looks at the file system.

use std::env;
use std::path::{Path, PathBuf};

use oso_platform::process::Executable;

/// The program `program` names, or why it is not there, for a person.
pub(super) fn program(
    program: &str,
    in_cargo_bin: bool,
    root: &Path,
) -> Result<Executable, String> {
    let found = if Path::new(program).is_absolute() {
        Executable::absolute(program).map_err(|error| error.to_string())
    } else {
        Executable::from_path(program, Some(root)).or_else(|_| {
            let cargo = cargo_bin().filter(|_| in_cargo_bin);
            match cargo {
                Some(dir) => Executable::absolute(dir.join(exe(program))).map_err(|_| {
                    format!(
                        "{program} is not installed (looked on PATH and in {})",
                        dir.display()
                    )
                }),
                None => Err(format!("{program} is not installed (looked on PATH)")),
            }
        })
    }?;
    only_exe(&found)?;
    Ok(found)
}

/// Cargo's `bin` directory: `$CARGO_HOME/bin`, else `~/.cargo/bin`, from oso's own
/// environment.
fn cargo_bin() -> Option<PathBuf> {
    if let Some(home) = env::var_os("CARGO_HOME").map(PathBuf::from)
        && home.is_absolute()
    {
        return Some(home.join("bin"));
    }
    oso_platform::privacy::home().map(|home| home.join(".cargo").join("bin"))
}

/// `name` as the file it is on this system.
fn exe(name: &str) -> String {
    if cfg!(windows) && !name.to_ascii_lowercase().ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// On Windows, refuses a program that is not an `.exe`: a `.bat` or `.cmd` runs through
/// `cmd.exe`, whose quoting has let arguments run commands (CVE-2024-24576).
fn only_exe(found: &Executable) -> Result<(), String> {
    if cfg!(windows) && !is_exe(found.path()) {
        return Err(format!(
            "{} is not an .exe: oso starts no .bat or .cmd as a server",
            found.path().display()
        ));
    }
    Ok(())
}

/// Whether `path` names an `.exe`, ignoring case.
fn is_exe(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_says_where_it_was_looked_for() {
        let root = env::temp_dir();
        let error = program("oso-no-such-server", false, &root).unwrap_err();
        assert_eq!(
            error,
            "oso-no-such-server is not installed (looked on PATH)"
        );
        let error = program("oso-no-such-server", true, &root).unwrap_err();
        assert!(error.starts_with("oso-no-such-server is not installed (looked on PATH and in "));
        assert!(
            error.contains(".cargo") || env::var_os("CARGO_HOME").is_some(),
            "{error}"
        );
    }

    #[test]
    fn only_an_exe_is_one() {
        for exe in ["C:/ra/rust-analyzer.exe", "C:/ra/RA.EXE", "ra.Exe"] {
            assert!(is_exe(Path::new(exe)), "{exe}");
        }
        for other in [
            "C:/n/tsserver.cmd",
            "C:/n/a.bat",
            "C:/n/a.BAT",
            "C:/n/a",
            "a.exe.cmd",
        ] {
            assert!(!is_exe(Path::new(other)), "{other}");
        }
        assert_eq!(exe("rust-analyzer.EXE"), "rust-analyzer.EXE");
    }

    #[cfg(unix)]
    #[test]
    fn an_absolute_path_or_a_name_on_path_is_found() {
        let root = env::temp_dir();
        assert_eq!(
            program("/bin/sh", false, &root).unwrap().path(),
            Path::new("/bin/sh")
        );
        assert!(program("sh", false, &root).is_ok());
        assert!(program("/no/such/server", false, &root).is_err());
    }
}
