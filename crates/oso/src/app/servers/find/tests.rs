//! Tests of finding a server's program: where it is looked for and in what order, and each
//! check of a program inside the project, on files in a scratch directory.
//!
//! Must not depend on: the editor or a server; these look at files only.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;

fn plain(command: &[&str], also: AlsoIn) -> Where {
    Where {
        in_project: Vec::new(),
        command: command.iter().map(|word| (*word).to_owned()).collect(),
        also_in: also,
    }
}

#[test]
fn a_missing_program_says_where_it_was_looked_for() {
    let root = env::temp_dir();
    let mut passed = Vec::new();
    let error = program(
        &plain(&["oso-no-such-server"], AlsoIn::Nowhere),
        &root,
        &mut passed,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "oso-no-such-server is not installed (looked on PATH)"
    );
    let error = program(
        &plain(&["oso-no-such-server"], AlsoIn::CargoBin),
        &root,
        &mut passed,
    )
    .unwrap_err();
    assert!(error.starts_with("oso-no-such-server is not installed (looked on PATH and in "));
    let error = program(
        &plain(&["oso-no-such-server"], AlsoIn::GoBin),
        &root,
        &mut passed,
    )
    .unwrap_err();
    assert!(
        error.contains("go") || env::var_os("GOBIN").is_some() || env::var_os("GOPATH").is_some(),
        "{error}"
    );
}

#[test]
fn go_s_bin_is_gobin_then_gopath_then_home() {
    // Absolute paths on this system: a drive starts one on Windows.
    let at = |path: &str| {
        if cfg!(windows) {
            PathBuf::from(format!("C:{path}"))
        } else {
            PathBuf::from(path)
        }
    };
    let home = Some(at("/home/p"));
    assert_eq!(
        go_bin_of(
            Some(at("/opt/gobin").into()),
            Some(env::join_paths([at("/g1"), at("/g2")]).unwrap()),
            home.clone()
        ),
        Some(at("/opt/gobin"))
    );
    let paths: OsString = env::join_paths([at("/g1"), at("/g2")]).unwrap();
    assert_eq!(
        go_bin_of(None, Some(paths), home.clone()),
        Some(at("/g1").join("bin"))
    );
    assert_eq!(
        go_bin_of(
            Some("relative".into()),
            Some("also/relative".into()),
            home.clone()
        ),
        Some(at("/home/p").join("go").join("bin")),
        "relative entries are not taken"
    );
    assert_eq!(go_bin_of(None, None, None), None);
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
mod unix {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    /// A fresh project for one test, with `node_modules/.bin`.
    fn project(name: &str) -> PathBuf {
        let dir = fs::canonicalize(env::temp_dir())
            .unwrap()
            .join(format!("oso-find-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("node_modules/.bin")).unwrap();
        dir
    }

    /// A program file at `path`, executable, with `mode`.
    fn program_at(path: &Path, mode: u32) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn typescript() -> Where {
        Where {
            in_project: vec![
                InProject {
                    program: "typescript-language-server",
                    args: &["--stdio"],
                    needs: Some("node_modules/typescript/lib/tsserver.js"),
                },
                InProject {
                    program: "tsc",
                    args: &["--lsp", "--stdio"],
                    needs: None,
                },
            ],
            command: vec!["oso-no-such-tsc".into(), "--lsp".into(), "--stdio".into()],
            also_in: AlsoIn::Nowhere,
        }
    }

    #[test]
    fn the_projects_server_comes_first_when_its_typescript_is_there() {
        let root = project("first");
        let server = root.join("node_modules/typescript-language-server/lib/cli.mjs");
        program_at(&server, 0o755);
        symlink(
            &server,
            root.join("node_modules/.bin/typescript-language-server"),
        )
        .unwrap();
        program_at(&root.join("node_modules/typescript/lib/tsserver.js"), 0o644);
        program_at(&root.join("node_modules/.bin/tsc"), 0o755);
        let mut passed = Vec::new();
        let (found, args) = program(&typescript(), &root, &mut passed).unwrap();
        assert_eq!(found.path(), server, "the link is followed");
        assert_eq!(args, ["--stdio"]);
        assert!(passed.is_empty());

        // Without tsserver.js, the project's tsc.
        fs::remove_file(root.join("node_modules/typescript/lib/tsserver.js")).unwrap();
        let (found, args) = program(&typescript(), &root, &mut passed).unwrap();
        assert_eq!(found.path(), root.join("node_modules/.bin/tsc"));
        assert_eq!(args, ["--lsp", "--stdio"]);
        assert!(
            passed[0].contains("tsserver.js: it is not there"),
            "{passed:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_program_that_leads_out_or_that_others_can_change_is_passed_over() {
        let root = project("checks");
        let outside = root
            .parent()
            .unwrap()
            .join(format!("oso-find-outside-{}", std::process::id()));
        program_at(&outside, 0o755);
        symlink(&outside, root.join("node_modules/.bin/tsc")).unwrap();
        let mut passed = Vec::new();
        let error = program(&typescript(), &root, &mut passed).unwrap_err();
        assert!(
            passed
                .iter()
                .any(|why| why.contains("leads outside the project")),
            "{passed:?}"
        );
        assert!(
            error.contains("oso-no-such-tsc is not installed"),
            "{error}"
        );

        fs::remove_file(root.join("node_modules/.bin/tsc")).unwrap();
        program_at(&root.join("node_modules/.bin/tsc"), 0o775);
        let mut passed = Vec::new();
        assert!(program(&typescript(), &root, &mut passed).is_err());
        assert!(
            passed
                .iter()
                .any(|why| why.contains("others can change it")),
            "{passed:?}"
        );

        fs::remove_file(root.join("node_modules/.bin/tsc")).unwrap();
        fs::create_dir(root.join("node_modules/.bin/tsc")).unwrap();
        let mut passed = Vec::new();
        assert!(program(&typescript(), &root, &mut passed).is_err());
        assert!(
            passed.iter().any(|why| why.contains("not a regular file")),
            "{passed:?}"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn a_directory_others_can_change_on_the_way_is_refused_too() {
        let root = project("way");
        program_at(&root.join("node_modules/.bin/tsc"), 0o755);
        fs::set_permissions(
            root.join("node_modules/.bin"),
            fs::Permissions::from_mode(0o775),
        )
        .unwrap();
        let mut passed = Vec::new();
        assert!(program(&typescript(), &root, &mut passed).is_err());
        assert!(
            passed.iter().any(|why| why.contains("or the way to it")),
            "{passed:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_file_the_projects_server_needs_must_be_the_projects_too() {
        let root = project("needs");
        let server = root.join("node_modules/typescript-language-server/lib/cli.mjs");
        program_at(&server, 0o755);
        symlink(
            &server,
            root.join("node_modules/.bin/typescript-language-server"),
        )
        .unwrap();
        // node_modules/typescript leads to another place than the project.
        let elsewhere = root
            .parent()
            .unwrap()
            .join(format!("oso-find-typescript-{}", std::process::id()));
        program_at(&elsewhere.join("lib/tsserver.js"), 0o644);
        symlink(&elsewhere, root.join("node_modules/typescript")).unwrap();
        let mut passed = Vec::new();
        assert!(program(&typescript(), &root, &mut passed).is_err());
        assert!(
            passed
                .iter()
                .any(|why| why.contains("tsserver.js: it leads outside the project")),
            "{passed:?}"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(elsewhere).unwrap();
    }

    #[test]
    fn an_absolute_path_or_a_name_on_path_is_found() {
        let root = env::temp_dir();
        let mut passed = Vec::new();
        let (found, args) = program(
            &plain(&["/bin/sh", "-x"], AlsoIn::Nowhere),
            &root,
            &mut passed,
        )
        .unwrap();
        assert_eq!(found.path(), Path::new("/bin/sh"));
        assert_eq!(args, ["-x"]);
        assert!(program(&plain(&["sh"], AlsoIn::Nowhere), &root, &mut passed).is_ok());
        assert!(
            program(
                &plain(&["/no/such/server"], AlsoIn::Nowhere),
                &root,
                &mut passed
            )
            .is_err()
        );
    }
}
