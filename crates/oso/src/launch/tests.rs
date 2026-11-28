//! The tests of an app launch's login environment (ADR 0187): what is read from the shell's
//! output, how it is merged, which shell runs, and running one with fixed arguments.
//!
//! Must not depend on: `app`; these run `/bin/sh`, never a login shell, and set no variable.

use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use super::{COMMAND, FALLBACK_SHELL, run, shell};
use super::{FALLBACK_LANG, Failure, MARKER, MAX_OUTPUT, START, merge, parse};

/// What a shell prints: `before`, the start line, then `entries`, each ended by NUL.
fn output(before: &[u8], entries: &[&str]) -> Vec<u8> {
    let mut out = before.to_vec();
    out.extend_from_slice(START);
    for entry in entries {
        out.extend_from_slice(entry.as_bytes());
        out.push(0);
    }
    out
}

fn pairs(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    list.iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

fn names(list: &[(OsString, OsString)]) -> Vec<&str> {
    list.iter()
        .map(|(name, _)| name.to_str().unwrap())
        .collect()
}

fn value<'a>(list: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsStr> {
    list.iter()
        .find(|(n, _)| n == name)
        .map(|(_, value)| value.as_os_str())
}

#[test]
fn what_the_profile_printed_before_the_start_is_ignored() {
    let out = output(
        b"Welcome!\n\0A=junk\0",
        &["HOME=/home/a", "PATH=/a/bin:/usr/bin"],
    );
    let taken = parse(&out).unwrap();
    assert_eq!(
        taken,
        pairs(&[("HOME", "/home/a"), ("PATH", "/a/bin:/usr/bin")])
    );
}

#[test]
fn output_with_no_start_is_refused() {
    assert!(matches!(
        parse(b"HOME=/home/a\0PATH=/usr/bin\0"),
        Err(Failure::NoMarker)
    ));
    assert!(matches!(parse(b""), Err(Failure::NoMarker)));
}

#[test]
fn names_that_are_not_portable_are_dropped() {
    let out = output(
        b"",
        &[
            "1A=x",
            "A-B=x",
            "BASH_FUNC_f%%=() {  :\n}",
            "=x",
            "no equals sign",
            "_OK=1",
            "Mixed_9=2",
            "A=b=c",
        ],
    );
    assert_eq!(
        parse(&out).unwrap(),
        pairs(&[("_OK", "1"), ("Mixed_9", "2"), ("A", "b=c")])
    );
}

#[test]
fn the_loaders_names_the_shells_own_and_the_marker_are_dropped() {
    let marker = format!("{MARKER}=1");
    let out = output(
        b"",
        &[
            "DYLD_INSERT_LIBRARIES=/tmp/x.dylib",
            "DYLD_LIBRARY_PATH=/tmp",
            "LD_PRELOAD=/tmp/x.so",
            "PWD=/",
            "OLDPWD=/tmp",
            "SHLVL=1",
            "_=/usr/bin/env",
            &marker,
            "GOPATH=/home/a/go",
        ],
    );
    assert_eq!(parse(&out).unwrap(), pairs(&[("GOPATH", "/home/a/go")]));
}

#[test]
fn output_over_the_limit_is_refused() {
    let long = format!("A={}", "x".repeat(MAX_OUTPUT));
    assert!(matches!(
        parse(&output(b"", &[&long])),
        Err(Failure::TooLong)
    ));
    // What fits is read.
    let fits = format!("A={}", "x".repeat(MAX_OUTPUT - START.len() - 3));
    assert_eq!(parse(&output(b"", &[&fits])).unwrap().len(), 1);
}

#[test]
fn the_first_of_a_name_is_kept() {
    let out = output(b"", &["A=first", "B=1", "A=second"]);
    assert_eq!(parse(&out).unwrap(), pairs(&[("A", "first"), ("B", "1")]));
}

#[test]
fn an_empty_path_is_read_and_left_to_merge() {
    let out = output(b"", &["PATH="]);
    assert_eq!(parse(&out).unwrap(), pairs(&[("PATH", "")]));
}

#[test]
fn the_shells_path_replaces_launchds() {
    let current = pairs(&[("PATH", "/usr/bin:/bin"), ("HOME", "/home/a")]);
    let merged = merge(&current, pairs(&[("PATH", "/opt/homebrew/bin:/usr/bin")]));
    assert_eq!(
        value(&merged, "PATH"),
        Some(OsStr::new("/opt/homebrew/bin:/usr/bin"))
    );
}

#[test]
fn an_empty_path_from_the_shell_keeps_launchds() {
    let current = pairs(&[("PATH", "/usr/bin:/bin")]);
    let merged = merge(&current, pairs(&[("PATH", "")]));
    assert_eq!(value(&merged, "PATH"), Some(OsStr::new("/usr/bin:/bin")));
}

#[test]
fn launchds_other_names_are_kept_and_new_ones_added() {
    let current = pairs(&[
        ("HOME", "/home/a"),
        ("USER", "a"),
        ("TMPDIR", "/var/folders/x/T/"),
        ("SSH_AUTH_SOCK", "/private/tmp/launchd/Listeners"),
    ]);
    let taken = pairs(&[
        ("HOME", "/elsewhere"),
        ("USER", "b"),
        ("TMPDIR", "/tmp"),
        ("SSH_AUTH_SOCK", "/tmp/agent"),
        ("GOPATH", "/home/a/go"),
    ]);
    let merged = merge(&current, taken);
    assert_eq!(value(&merged, "HOME"), Some(OsStr::new("/home/a")));
    assert_eq!(value(&merged, "USER"), Some(OsStr::new("a")));
    assert_eq!(
        value(&merged, "TMPDIR"),
        Some(OsStr::new("/var/folders/x/T/"))
    );
    assert_eq!(
        value(&merged, "SSH_AUTH_SOCK"),
        Some(OsStr::new("/private/tmp/launchd/Listeners"))
    );
    assert_eq!(value(&merged, "GOPATH"), Some(OsStr::new("/home/a/go")));
    assert_eq!(names(&merged).iter().filter(|n| **n == "HOME").count(), 1);
}

#[test]
fn the_marker_goes() {
    let current = pairs(&[(MARKER, "1"), ("HOME", "/home/a"), ("LANG", "C")]);
    let merged = merge(&current, Vec::new());
    assert_eq!(names(&merged), ["HOME", "LANG"]);
}

#[test]
fn lang_falls_back_only_when_neither_has_it() {
    let merged = merge(&pairs(&[("HOME", "/h")]), Vec::new());
    assert_eq!(value(&merged, "LANG"), Some(OsStr::new(FALLBACK_LANG)));
    let merged = merge(&pairs(&[("HOME", "/h")]), pairs(&[("LANG", "ja_JP.UTF-8")]));
    assert_eq!(value(&merged, "LANG"), Some(OsStr::new("ja_JP.UTF-8")));
    let merged = merge(&pairs(&[("LANG", "C")]), pairs(&[("LANG", "ja_JP.UTF-8")]));
    assert_eq!(value(&merged, "LANG"), Some(OsStr::new("C")));
}

#[cfg(unix)]
#[test]
fn the_accounts_shell_runs_when_it_is_an_executable() {
    assert_eq!(
        shell(Some(PathBuf::from("/bin/sh"))),
        Some(PathBuf::from("/bin/sh"))
    );
}

#[cfg(unix)]
#[test]
fn bash_runs_when_the_account_has_no_shell_that_runs() {
    use std::os::unix::fs::PermissionsExt;

    let bash = Some(PathBuf::from(FALLBACK_SHELL));
    assert_eq!(shell(None), bash);
    assert_eq!(shell(Some(PathBuf::from("/no/such/shell"))), bash);
    let scratch = Scratch::new("shell");
    let plain = scratch.path().join("plain");
    std::fs::write(&plain, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(shell(Some(plain)), bash);
    assert_eq!(shell(Some(scratch.path().to_path_buf())), bash);
}

/// A directory of a test's own under the temporary directory, removed when dropped. Not the
/// saver's, which is in a higher layer.
#[cfg(unix)]
struct Scratch(PathBuf);

#[cfg(unix)]
impl Scratch {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "oso-launch-{test}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn sh(script: &str, timeout: Duration) -> Result<Vec<u8>, Failure> {
    run(
        std::path::Path::new("/bin/sh"),
        &[OsStr::new("-c"), OsStr::new(script)],
        timeout,
    )
}

#[cfg(unix)]
#[test]
fn what_a_shell_prints_is_read() {
    let out = sh(
        "printf 'junk'; printf '\\000oso-env\\000A=1\\000B=two\\000'",
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(parse(&out).unwrap(), pairs(&[("A", "1"), ("B", "two")]));
}

#[cfg(unix)]
#[test]
fn the_fixed_command_prints_the_environment() {
    let out = sh(COMMAND, Duration::from_secs(10)).unwrap();
    let taken = parse(&out).unwrap();
    // The process module gives the shell its allowlist and `PATH`.
    assert!(value(&taken, "PATH").is_some(), "{:?}", names(&taken));
    assert!(value(&taken, "HOME").is_some(), "{:?}", names(&taken));
}

#[cfg(unix)]
#[test]
fn a_nonzero_exit_with_good_output_is_read() {
    let out = sh(
        "printf '\\000oso-env\\000A=1\\000'; exit 3",
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(parse(&out).unwrap(), pairs(&[("A", "1")]));
}

#[cfg(unix)]
#[test]
fn a_shell_that_hangs_is_ended_with_its_group_within_the_bound() {
    let scratch = Scratch::new("hang");
    let pid_file = scratch.path().join("pid");
    // `wait` keeps the shell from exec'ing `sleep`, which then runs in its group.
    let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());
    let bound = Duration::from_millis(300);
    let started = Instant::now();
    let result = sh(&script, bound);
    let took = started.elapsed();
    assert!(
        matches!(result, Err(Failure::Timeout(t)) if t == bound),
        "{result:?}"
    );
    assert!(took < bound + Duration::from_secs(2), "{took:?}");
    // The `sleep` the shell started is gone too: read, never signalled, by its id.
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while oso_platform::process::start_time(pid).is_ok_and(|t| t.is_some()) {
        assert!(Instant::now() < deadline, "sleep {pid} still runs");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn output_over_the_limit_is_cut_and_then_refused() {
    let script = format!(
        "printf '\\000oso-env\\000'; head -c {} /dev/zero | tr '\\0' x",
        2 * MAX_OUTPUT
    );
    let out = sh(&script, Duration::from_secs(10)).unwrap();
    assert_eq!(out.len(), MAX_OUTPUT + 1);
    assert!(matches!(parse(&out), Err(Failure::TooLong)));
}

#[cfg(unix)]
#[test]
fn a_shell_that_cannot_start_says_so() {
    let result = sh_at("/no/such/shell");
    assert!(matches!(result, Err(Failure::Spawn(_))), "{result:?}");
}

#[cfg(unix)]
fn sh_at(path: &str) -> Result<Vec<u8>, Failure> {
    run(
        std::path::Path::new(path),
        &[OsStr::new("-c"), OsStr::new("true")],
        Duration::from_secs(1),
    )
}

#[test]
fn nothing_happens_without_the_marker() {
    if std::env::var_os(MARKER).is_none() {
        assert!(super::take_login_environment_if_app().is_none());
    }
}
