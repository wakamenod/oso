//! Tests of reading a file with ADR 0108's checks: a file the configuration names, with a
//! limit of its own (ADR 0116). `config.toml`'s own reads are tested through its thread, in
//! `watch`'s tests.
//!
//! Must not depend on: `app` or `dispatch`.

#[cfg(unix)]
use std::fs;

use super::read_checked;
use crate::saver::tests::Scratch;

/// A scratch directory of this user's alone, mode 0700, whatever the umask.
fn scratch(test: &str) -> Scratch {
    let scratch = Scratch::new(test);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    scratch
}

/// A file in `scratch` with `content`, mode 0644, whatever the umask.
fn file(scratch: &Scratch, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = scratch.file(name, content);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    path
}

/// A file the configuration names, migemo's dictionary, is read with the file's own checks
/// and a limit of its own (ADR 0116), and what refuses it says why without quoting it.
#[test]
fn a_file_the_configuration_names_has_the_files_checks_and_its_own_limit() {
    let scratch = scratch("config-read-named");
    let fits = file(&scratch, "fits", "かんじ\t漢字\n".as_bytes());
    assert_eq!(
        read_checked(&fits, 64),
        Ok(("かんじ\t漢字\n".to_owned(), false))
    );
    let large = file(&scratch, "large", &vec![b'x'; 2 * 1024 * 1024 + 1]);
    let binary = file(&scratch, "binary", b"\xa4\xab\xa4\xf3\n");
    let missing = scratch.path().join("missing");
    for (path, max, why) in [
        (&large, 2 * 1024 * 1024, "it is larger than 2 MiB"),
        (&fits, 8, "it is larger than 8 bytes"),
        (&binary, 64, "it is not UTF-8 text"),
        (&missing, 64, "there is no such file"),
    ] {
        assert_eq!(read_checked(path, max), Err(why.to_owned()), "{path:?}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fits, fs::Permissions::from_mode(0o664)).unwrap();
        assert!(read_checked(&fits, 64).is_ok_and(|(_, group)| group));
        fs::set_permissions(&fits, fs::Permissions::from_mode(0o646)).unwrap();
        let refused = read_checked(&fits, 64);
        assert_eq!(refused, Err("other users can change it".to_owned()));
    }
}

/// A pipe, or a link to one, is refused at once: nobody writes to it, so a read that waited
/// on it would never end.
#[cfg(unix)]
#[test]
fn a_pipe_or_a_link_to_one_is_refused_without_waiting() {
    use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
    let scratch = scratch("config-read-pipe");
    let pipe = scratch.path().join("pipe");
    let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
    let status = spawn(&Spawn {
        executable: &mkfifo,
        args: &[pipe.clone().into_os_string()],
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        set: &[],
        terminal: None,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run mkfifo");
    assert!(status.success());
    let link = scratch.path().join("link");
    std::os::unix::fs::symlink(&pipe, &link).unwrap();
    for path in [&pipe, &link] {
        let refused = read_checked(path, 64);
        assert_eq!(
            refused,
            Err("it is not a regular file".to_owned()),
            "{path:?}"
        );
    }
}
