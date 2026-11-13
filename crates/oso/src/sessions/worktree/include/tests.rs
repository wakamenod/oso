//! Tests of reading `.worktreeinclude` and copying what it takes into a new worktree, on scratch
//! directories, with git's answer written by hand.
//!
//! Must not depend on: `app` or `dispatch`.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::*;

/// A scratch directory with no links in its path, holding `from` (the main checkout), `to` (the
/// new worktree) and `outside`, removed at the end of the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-include-test-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        for dir in ["from", "to", "outside"] {
            fs::create_dir_all(path.join(dir)).unwrap();
        }
        Self(path)
    }

    fn from(&self) -> PathBuf {
        self.0.join("from")
    }

    fn to(&self) -> PathBuf {
        self.0.join("to")
    }

    fn outside(&self) -> PathBuf {
        self.0.join("outside")
    }

    /// Writes `content` at `relative` in the main checkout, making its directories.
    fn file(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.from().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    /// `.worktreeinclude` holding `text`, read back.
    fn patterns(&self, text: &str) -> Patterns {
        fs::write(self.from().join(NAME), text).unwrap();
        read(&self.from()).unwrap().unwrap()
    }

    /// Copies what `text`'s patterns take among `ignored`, within `limits`.
    fn copy(&self, text: &str, ignored: &[&str], limits: Limits) -> Copied {
        let patterns = self.patterns(text);
        let ignored: Vec<PathBuf> = ignored.iter().map(PathBuf::from).collect();
        copy_within(&patterns, &ignored, &self.from(), &self.to(), limits).unwrap()
    }

    /// The new worktree's files, relative to it, sorted.
    fn copied(&self) -> Vec<String> {
        let mut found = Vec::new();
        list(&self.to(), &self.to(), &mut found);
        found.sort();
        found
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn list(root: &Path, dir: &Path, found: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            list(root, &entry.path(), found);
        } else {
            let relative = entry.path().strip_prefix(root).unwrap().to_owned();
            found.push(relative.to_string_lossy().into_owned());
        }
    }
}

const NO_LIMITS: Limits = Limits {
    files: MAX_FILES,
    bytes: MAX_BYTES,
    visits: MAX_VISITS,
};

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode()
}

/// The permission bits the umask leaves of `0o777`, read from a directory made the usual way.
fn umask_leaves(scratch: &Scratch) -> u32 {
    let probe = scratch.0.join("probe");
    fs::create_dir(&probe).unwrap();
    let leaves = mode(&probe) & 0o777;
    fs::remove_dir(probe).unwrap();
    leaves
}

/// Makes a pipe at `path` with `mkfifo`, as the crate's other tests do.
fn fifo(path: &Path) {
    use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
    let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
    let status = spawn(&Spawn {
        executable: &mkfifo,
        args: &[path.as_os_str().to_owned()],
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
}

#[test]
fn a_file_taken_and_ignored_is_copied_with_its_content_and_mode() {
    let scratch = Scratch::new("copied");
    let env = scratch.file(".env", "SECRET=1");
    fs::set_permissions(&env, fs::Permissions::from_mode(0o600)).unwrap();
    scratch.file("settings.local.json", "{}");
    let copied = scratch.copy(
        ".env\n# a comment\n\n*.local.json\n",
        &[".env", "settings.local.json"],
        NO_LIMITS,
    );
    assert_eq!(
        copied,
        Copied {
            files: 2,
            bytes: 10,
            stopped: None
        }
    );
    assert_eq!(
        fs::read_to_string(scratch.to().join(".env")).unwrap(),
        "SECRET=1"
    );
    assert_eq!(mode(&scratch.to().join(".env")) & 0o777, 0o600);
}

#[test]
fn a_file_is_copied_only_if_both_taken_and_listed() {
    let scratch = Scratch::new("both");
    scratch.file("target/out.o", "object");
    scratch.file(".env.local", "not ignored, or tracked");
    scratch.file(".env", "wanted");
    let copied = scratch.copy(".env*\n", &["target/out.o", ".env"], NO_LIMITS);
    assert_eq!(copied.files, 1);
    assert_eq!(scratch.copied(), [".env"]);
}

#[test]
fn a_listed_directory_is_copied_whole_when_taken_and_descended_when_not() {
    let scratch = Scratch::new("directories");
    scratch.file("config/a", "a");
    scratch.file("config/deeper/b", "b");
    scratch.file("node_modules/a/.env", "inner");
    scratch.file("node_modules/a/index.js", "code");
    let copied = scratch.copy(
        "config/\n**/.env\n",
        &["config/", "node_modules/"],
        NO_LIMITS,
    );
    assert_eq!(copied.files, 3);
    assert_eq!(
        scratch.copied(),
        ["config/a", "config/deeper/b", "node_modules/a/.env"]
    );
}

#[test]
fn a_nested_repository_is_skipped() {
    let scratch = Scratch::new("nested");
    scratch.file("vendor/lib/.git/HEAD", "ref");
    scratch.file("vendor/lib/.env", "theirs");
    scratch.file("other/.git", "gitdir: elsewhere");
    scratch.file("other/.env", "theirs too");
    scratch.file("vendor/.env", "ours");
    let copied = scratch.copy("**/.env\nother/\n", &["vendor/", "other/"], NO_LIMITS);
    assert_eq!(copied.files, 1);
    assert_eq!(scratch.copied(), ["vendor/.env"]);
}

#[test]
fn no_link_in_the_main_checkout_is_followed() {
    let scratch = Scratch::new("source-links");
    fs::write(scratch.outside().join(".env"), "outside").unwrap();
    fs::write(scratch.outside().join("secret"), "outside").unwrap();
    symlink(
        scratch.outside().join("secret"),
        scratch.from().join(".env"),
    )
    .unwrap();
    symlink(scratch.outside(), scratch.from().join("node_modules")).unwrap();
    symlink(scratch.outside(), scratch.from().join("linked")).unwrap();
    let copied = scratch.copy(
        "**/.env\nlinked/\n",
        &[".env", "node_modules/", "node_modules/.env", "linked/"],
        NO_LIMITS,
    );
    assert_eq!(copied, Copied::default());
    assert!(scratch.copied().is_empty());
}

#[test]
fn no_link_in_the_new_worktree_is_written_through() {
    let scratch = Scratch::new("target-links");
    scratch.file("config/secret", "kept here");
    scratch.file(".env", "kept here too");
    symlink(scratch.outside(), scratch.to().join("config")).unwrap();
    symlink(
        scratch.outside().join("dangling"),
        scratch.to().join(".env"),
    )
    .unwrap();
    let copied = scratch.copy("config/\n.env\n", &["config/secret", ".env"], NO_LIMITS);
    assert_eq!(copied.files, 0);
    assert_eq!(fs::read_dir(scratch.outside()).unwrap().count(), 0);
}

#[test]
fn a_file_already_in_the_new_worktree_is_left_as_it_is() {
    let scratch = Scratch::new("existing");
    scratch.file(".env", "new");
    fs::write(scratch.to().join(".env"), "old").unwrap();
    let copied = scratch.copy(".env\n", &[".env"], NO_LIMITS);
    assert_eq!(copied, Copied::default());
    assert_eq!(
        fs::read_to_string(scratch.to().join(".env")).unwrap(),
        "old"
    );
}

#[test]
fn only_regular_files_are_copied() {
    let scratch = Scratch::new("pipe");
    fifo(&scratch.from().join("pipe"));
    fs::create_dir(scratch.from().join("cache")).unwrap();
    fifo(&scratch.from().join("cache/pipe"));
    let copied = scratch.copy("pipe\n**/pipe\n", &["pipe", "cache/"], NO_LIMITS);
    assert_eq!(copied, Copied::default());
    assert!(scratch.copied().is_empty());
}

#[test]
fn set_user_id_set_group_id_and_sticky_are_dropped() {
    let scratch = Scratch::new("setuid");
    let leaves = umask_leaves(&scratch);
    let tool = scratch.file("bin/tool", "#!/bin/sh\n");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o7755)).unwrap();
    let copied = scratch.copy("bin/\n", &["bin/"], NO_LIMITS);
    assert_eq!(copied.files, 1);
    assert_eq!(
        mode(&scratch.to().join("bin/tool")) & 0o7777,
        0o755 & leaves
    );
}

#[test]
fn a_path_that_leaves_the_checkout_is_skipped() {
    let scratch = Scratch::new("leaves");
    fs::write(scratch.0.join("secret"), "beside the checkout").unwrap();
    fs::write(scratch.outside().join("secret"), "outside").unwrap();
    let outside = scratch.outside().join("secret");
    let copied = scratch.copy(
        "*\n",
        &[
            "../secret",
            "../outside/",
            outside.to_str().unwrap(),
            ".git/config",
        ],
        NO_LIMITS,
    );
    assert_eq!(copied, Copied::default());
    assert!(scratch.copied().is_empty());
}

#[test]
fn the_copy_stops_at_the_files_cap() {
    let scratch = Scratch::new("files-cap");
    for name in ["a", "b", "c"] {
        scratch.file(&format!("keys/{name}"), name);
    }
    let limits = Limits {
        files: 2,
        ..NO_LIMITS
    };
    let copied = scratch.copy("keys/\n", &["keys/"], limits);
    assert_eq!(
        copied,
        Copied {
            files: 2,
            bytes: 2,
            stopped: Some(Stop::Files)
        }
    );
    assert_eq!(scratch.copied().len(), 2);
}

#[test]
fn the_copy_stops_at_the_bytes_cap_with_no_file_left_partly_written() {
    let scratch = Scratch::new("bytes-cap");
    scratch.file("a", "0123456789");
    scratch.file("b", "0123456789");
    let limits = Limits {
        bytes: 15,
        ..NO_LIMITS
    };
    let copied = scratch.copy("a\nb\n", &["a", "b"], limits);
    assert_eq!(
        copied,
        Copied {
            files: 1,
            bytes: 10,
            stopped: Some(Stop::Bytes)
        }
    );
    assert_eq!(scratch.copied(), ["a"]);
}

#[test]
fn the_copy_stops_at_the_visits_cap() {
    let scratch = Scratch::new("visits-cap");
    for name in ["a", "b", "c", "d", "e"] {
        scratch.file(&format!("node_modules/{name}"), name);
    }
    scratch.file(".env", "listed first");
    let limits = Limits {
        visits: 3,
        ..NO_LIMITS
    };
    // The directory descended goes last, so the listed file is copied all the same.
    let copied = scratch.copy("**/.env\n", &["node_modules/", ".env"], limits);
    assert_eq!(
        copied,
        Copied {
            files: 1,
            bytes: 12,
            stopped: Some(Stop::Visits)
        }
    );
}

#[test]
fn reading_the_patterns_wants_one_regular_file_within_its_caps() {
    let scratch = Scratch::new("read");
    let root = scratch.from();
    assert!(read(&root).unwrap().is_none(), "nothing there");
    fs::write(scratch.outside().join("patterns"), ".env\n").unwrap();
    symlink(scratch.outside().join("patterns"), root.join(NAME)).unwrap();
    assert!(read(&root).unwrap().is_none(), "a link");
    fs::remove_file(root.join(NAME)).unwrap();
    fs::create_dir(root.join(NAME)).unwrap();
    assert!(read(&root).unwrap().is_none(), "a directory");
    fs::remove_dir(root.join(NAME)).unwrap();
    fs::write(root.join(NAME), "#".repeat(64 * 1024 + 1)).unwrap();
    assert!(matches!(read(&root), Err(IncludeError::TooLarge)));
    let many: String = (0..=MAX_PATTERNS)
        .map(|n| format!("p{n}\n# note\n\n"))
        .collect();
    fs::write(root.join(NAME), many).unwrap();
    assert!(matches!(read(&root), Err(IncludeError::TooMany)));
    fs::write(root.join(NAME), b"\xff\xfe.secret\n.env\n").unwrap();
    let patterns = read(&root).unwrap().unwrap();
    assert!(patterns.take(Path::new(".env"), false));
    assert!(!patterns.take(Path::new(".secret"), false));
    fs::write(root.join(NAME), "a{\n").unwrap();
    let error = read(&root).map(|_| ()).unwrap_err();
    assert!(matches!(error, IncludeError::Pattern(_)));
    assert!(error.to_string().starts_with(NAME));
}

#[test]
fn a_file_with_another_name_is_not_copied() {
    let scratch = Scratch::new("hard-link");
    let outside = scratch.outside().join("secret");
    fs::write(&outside, "kept elsewhere").unwrap();
    fs::hard_link(&outside, scratch.from().join(".env")).unwrap();
    scratch.file(".env.local", "local");
    let copied = scratch.copy(".env*\n", &[".env", ".env.local"], NO_LIMITS);
    assert_eq!(copied.files, 1);
    assert_eq!(scratch.copied(), [".env.local"]);
}

#[test]
fn each_directory_descended_gets_its_share_of_the_visits() {
    let scratch = Scratch::new("visits-share");
    for n in 0..20 {
        scratch.file(&format!("big/{n:02}"), "x");
    }
    scratch.file("small/.env", "small");
    let limits = Limits {
        visits: 10,
        ..NO_LIMITS
    };
    // `big/` comes first, but takes only its half of the visits.
    let copied = scratch.copy("**/.env\n", &["big/", "small/"], limits);
    assert_eq!(scratch.copied(), ["small/.env"]);
    assert_eq!(copied.files, 1);
    assert_eq!(copied.stopped, Some(Stop::Visits), "big/ was cut short");
}

#[test]
fn a_patterns_error_is_shown_with_its_controls_escaped() {
    let error = IncludeError::Pattern("bad \u{1b}[2J\u{202e}glob".to_owned());
    let shown = error.to_string();
    assert!(
        !shown.contains('\u{1b}') && !shown.contains('\u{202e}'),
        "{shown}"
    );
    assert!(shown.contains("\\u{1b}"), "{shown}");
}
