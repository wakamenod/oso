//! Tests of starting a child by hand on Unix (ADR 0137): each child gets only the descriptors
//! it is given, and [`Child`] waits for and ends it as the standard library's
//! does.
//!
//! Must not depend on: `control` or `text`.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};

use crate::process::pty::{self, Size};
use crate::process::{Child, Executable, PassedFd, Sink, Source, Spawn, spawn};

const SIZE: Size = Size {
    rows: 24,
    columns: 80,
    width: 0,
    height: 0,
};

fn sh() -> Executable {
    Executable::absolute("/bin/sh").unwrap()
}

fn request<'a>(sh: &'a Executable, args: &'a [OsString]) -> Spawn<'a> {
    Spawn {
        executable: sh,
        args,
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: false,
        set: &[],
        terminal: None,
        pass_fd: None,
        args_logged: true,
    }
}

/// What a child writes to its standard output, and how it ended.
fn output_of(mut child: Child) -> (String, std::process::ExitStatus) {
    let mut text = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    let status = child.wait().unwrap();
    (text, status)
}

/// What a terminal's master reads until the child has written `needle`.
fn read_until(master: &pty::Master, needle: &str) -> String {
    let (_waker, wakeup) = pty::wake_pair().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&seen).into_owned();
        if text.contains(needle) {
            return text;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "waited for {needle:?}, saw {text:?}");
        if !master.wait(&wakeup, false, Some(left)).unwrap().readable {
            continue;
        }
        match master.read(&mut buf) {
            Ok(0) => panic!("the terminal ended before {needle:?}: {text:?}"),
            Ok(n) => seen.extend_from_slice(&buf[..n]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{error}"),
        }
    }
}

/// A copy of a descriptor of `/dev/null` at 200 or above, without close-on-exec, as a library
/// might leave one, or as macOS makes a socket or pipe for a moment.
fn leaky() -> OwnedFd {
    let file = std::fs::File::open("/dev/null").unwrap();
    // SAFETY: `F_DUPFD` copies a descriptor `file` keeps open to a free number from 200 up,
    // without close-on-exec, and touches no memory.
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 200) };
    assert!(fd >= 200, "{}", io::Error::last_os_error());
    // SAFETY: `fd` was just made by the call above, and nothing else owns it.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

/// A script that lists which of the descriptors 0 to 63, and `extra`, it holds, to `out`
/// (`>&3`, or nothing for its standard output). `test` runs in the shell, which opens nothing
/// for it; the shell's own copy for a redirection lives only while what is redirected runs.
fn list_open(extra: i32, out: &str) -> String {
    format!(
        "for fd in $(seq 0 63) {extra}; do test -e /dev/fd/$fd && printf '%s ' $fd {out}; done; \
         echo {out}"
    )
}

#[test]
fn a_child_gets_only_its_standard_streams() {
    let leak = leaky();
    let sh = sh();
    let args = ["-c".into(), list_open(leak.as_raw_fd(), "").into()];
    for stdin in [Source::Closed, Source::Piped] {
        let child = spawn(&Spawn {
            stdin,
            stdout: Sink::Piped,
            stderr: Sink::Piped,
            ..request(&sh, &args)
        })
        .unwrap();
        let (text, status) = output_of(child);
        assert!(status.success());
        assert_eq!(text, "0 1 2 \n", "{stdin:?}");
    }
}

#[test]
fn a_passed_descriptor_is_the_only_one_past_the_standard_streams() {
    let leak = leaky();
    let (mut reader, writer) = io::pipe().unwrap();
    let sh = sh();
    let script = list_open(leak.as_raw_fd(), ">&3");
    let args = ["-c".into(), script.into()];
    let mut child = spawn(&Spawn {
        pass_fd: Some(PassedFd::new(writer.as_fd())),
        args_logged: true,
        ..request(&sh, &args)
    })
    .unwrap();
    drop(writer);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(text, "0 1 2 3 \n");
}

#[test]
fn a_terminal_child_has_the_slave_as_its_controlling_terminal_and_nothing_else() {
    let leak = leaky();
    let (master, slave) = pty::open(SIZE).unwrap();
    let sh = sh();
    let script = format!(
        "{}; exec 9</dev/tty && echo controlling; echo done",
        list_open(leak.as_raw_fd(), "")
    );
    let args = ["-c".into(), script.into()];
    let mut child = spawn(&Spawn {
        stdout: Sink::Inherit,
        stderr: Sink::Inherit,
        terminal: Some(&slave),
        ..request(&sh, &args)
    })
    .unwrap();
    drop(slave);
    let text = read_until(&master, "done");
    assert!(text.starts_with("0 1 2 \r\n"), "{text:?}");
    assert!(text.contains("controlling"), "{text:?}");
    assert!(child.wait().unwrap().success());
}

#[test]
fn a_child_runs_in_its_directory_with_its_variables() {
    let sh = sh();
    let args = [
        "-c".into(),
        "pwd; echo \"$OSO_TEST_VALUE\"; echo \"$OSO_TEST_VALUE_2\"".into(),
    ];
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let set = [
        ("OSO_TEST_VALUE", std::ffi::OsStr::new("first")),
        ("OSO_TEST_VALUE", std::ffi::OsStr::new("a b")),
        ("OSO_TEST_VALUE_2", std::ffi::OsStr::new("")),
    ];
    let child = spawn(&Spawn {
        cwd: Some(&dir),
        stdout: Sink::Piped,
        set: &set,
        ..request(&sh, &args)
    })
    .unwrap();
    let (text, status) = output_of(child);
    assert!(status.success());
    assert_eq!(
        text,
        format!("{}\na b\n\n", dir.display()),
        "the last value wins"
    );
}

#[test]
fn a_nul_in_an_argument_is_refused() {
    let sh = sh();
    let args = ["-c".into(), "true\0false".into()];
    let error = spawn(&request(&sh, &args)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_program_gone_since_it_was_found_fails_to_start() {
    let dir = std::env::temp_dir().join(format!("oso-spawn-gone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("program");
    std::fs::write(&path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let program = Executable::absolute(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let error = spawn(&request(&program, &[])).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_child_is_waited_for_and_killed_as_the_standard_librarys_is() {
    let sh = sh();
    let args = ["-c".into(), "exec sleep 60".into()];
    let mut child = spawn(&request(&sh, &args)).unwrap();
    assert!(child.id() > 0);
    assert_eq!(child.try_wait().unwrap(), None);
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    assert_eq!(
        child.try_wait().unwrap(),
        Some(status),
        "the status is kept"
    );
    assert_eq!(child.wait().unwrap(), status);
    child.kill().unwrap();
}

#[test]
fn waiting_closes_the_standard_input_first() {
    let cat = Executable::absolute("/bin/cat").unwrap();
    let mut child = spawn(&Spawn {
        stdin: Source::Piped,
        ..request(&cat, &[])
    })
    .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn the_output_of_both_streams_is_read_at_once() {
    // More than a pipe holds on each, so a reader of one at a time would stall the child.
    let sh = sh();
    let script = "i=0; while [ $i -lt 2000 ]; do \
                  echo 0123456789012345678901234567890123456789012345678901234567890123456789 >&2; \
                  echo out; i=$((i+1)); done";
    let args = ["-c".into(), script.into()];
    let child = spawn(&Spawn {
        stdout: Sink::Piped,
        stderr: Sink::Piped,
        ..request(&sh, &args)
    })
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 2000 * 4);
    assert_eq!(output.stderr.len(), 2000 * 71);
}

#[test]
fn a_program_that_asks_for_no_terminal_still_has_the_slave_as_its_controlling_one() {
    // A shell may take a terminal as its own, which `sleep` never does: only the start makes
    // the slave its controlling terminal (on macOS a child's own `TIOCSCTTY`, ADR 0137). The
    // spawn returns once the child has run `exec`, so this needs no waiting.
    let sleep = Executable::from_path("sleep", None).unwrap();
    let args = ["60".into()];
    for _ in 0..20 {
        let (master, slave) = pty::open(SIZE).unwrap();
        let mut child = spawn(&Spawn {
            stdout: Sink::Inherit,
            stderr: Sink::Inherit,
            terminal: Some(&slave),
            ..request(&sleep, &args)
        })
        .unwrap();
        drop(slave);
        assert_eq!(master.foreground_group().unwrap(), Some(child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[test]
fn a_low_descriptor_passed_is_not_overwritten_by_a_stream_set_up_before_it() {
    // This process's standard error, at 2, passed while the child's standard error is a pipe:
    // were it placed after the pipe's `dup2` to 2, the child's 3 would be the pipe too.
    let sh = sh();
    let script = "if [ /dev/fd/3 -ef /dev/fd/2 ]; then echo same; else echo apart; fi";
    let args = ["-c".into(), script.into()];
    let stderr = io::stderr();
    let child = spawn(&Spawn {
        stdout: Sink::Piped,
        stderr: Sink::Piped,
        pass_fd: Some(PassedFd::new(stderr.as_fd())),
        args_logged: true,
        ..request(&sh, &args)
    })
    .unwrap();
    let (text, status) = output_of(child);
    assert!(status.success());
    assert_eq!(text, "apart\n");
}

#[test]
fn a_child_waited_for_elsewhere_is_never_signalled() {
    let sh = sh();
    let args = ["-c".into(), "exit 0".into()];
    let mut child = spawn(&request(&sh, &args)).unwrap();
    let pid = libc::pid_t::try_from(child.id()).unwrap();
    let mut status = 0;
    // SAFETY: `waitpid` writes one `c_int` into `status`, which lives across the call.
    assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
    let error = child.wait().unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    child.kill().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_terminal_child_that_ended_before_the_check_keeps_its_status() {
    // Linux takes the terminal from a session as its leader exits: a short child may have
    // ended by the time the spawn asks the terminal for its session, which then has none.
    let sh = sh();
    let args = ["-c".into(), "read line; exit 3".into()];
    let (master, slave) = pty::open(SIZE).unwrap();
    let mut child = spawn(&Spawn {
        stdout: Sink::Inherit,
        stderr: Sink::Inherit,
        terminal: Some(&slave),
        ..request(&sh, &args)
    })
    .unwrap();
    master.write(b"\n").unwrap();
    let pid = libc::pid_t::try_from(child.id()).unwrap();
    let stat = format!("/proc/{pid}/stat");
    let deadline = Instant::now() + Duration::from_secs(10);
    // A zombie, not yet waited for: `Z` follows the program's name in parentheses.
    while !std::fs::read_to_string(&stat)
        .unwrap()
        .rsplit_once(')')
        .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
    {
        assert!(Instant::now() < deadline, "the child did not end");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        slave.session().unwrap_err().raw_os_error(),
        Some(libc::ENOTTY)
    );
    super::controls(&slave, pid).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(3));
}

#[cfg(target_os = "linux")]
#[test]
fn a_terminal_that_is_another_sessions_fails_the_start() {
    let sh = sh();
    let args = ["-c".into(), "read line".into()];
    let (master, slave) = pty::open(SIZE).unwrap();
    let request = Spawn {
        stdout: Sink::Inherit,
        stderr: Sink::Inherit,
        terminal: Some(&slave),
        ..request(&sh, &args)
    };
    let mut first = spawn(&request).unwrap();
    let error = spawn(&request).unwrap_err();
    let holder = format!("its session is {}", first.id());
    assert!(error.to_string().contains(&holder), "{error}");
    master.write(b"\n").unwrap();
    assert!(first.wait().unwrap().success());
}

/// Waits up to 10 s for `done`.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn a_look_at_an_ended_child_leaves_it_to_be_waited_for() {
    let sh = sh();
    let args = ["-c".into(), "exit 7".into()];
    let mut child = spawn(&request(&sh, &args)).unwrap();
    assert!(eventually(|| child.exited().unwrap()));
    assert!(child.exited().unwrap(), "a second look says the same");
    assert_eq!(child.wait().unwrap().code(), Some(7), "it was not reaped");
    assert!(child.exited().unwrap());
}

#[test]
fn a_running_child_has_not_exited_and_its_group_is_reached_until_it_is_reaped() {
    let sh = sh();
    let args = ["-c".into(), "sleep 60 & echo $!; wait".into()];
    let mut start = request(&sh, &args);
    start.stdout = Sink::Piped;
    start.own_process_group = true;
    let mut child = spawn(&start).unwrap();
    let mut line = Vec::new();
    let mut stdout = child.stdout.take().unwrap();
    let mut byte = [0];
    while stdout.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
        line.push(byte[0]);
    }
    let grandchild: libc::pid_t = String::from_utf8(line).unwrap().trim().parse().unwrap();
    assert!(!child.exited().unwrap(), "it runs");
    // SAFETY: `kill` with these numbers ends the child alone; the grandchild stays.
    unsafe { libc::kill(child.id().cast_signed(), libc::SIGKILL) };
    assert!(eventually(|| child.exited().unwrap()), "the child ended");
    // Not reaped by the look: what it left in its group can still be reached.
    assert!(alive(grandchild), "the grandchild still runs");
    child
        .signal_group(crate::process::Signal::Terminate)
        .unwrap();
    assert!(eventually(|| !alive(grandchild)), "the grandchild ended");
    child.wait().unwrap();
}

/// Whether a process `pid` is there to be signalled.
fn alive(pid: libc::pid_t) -> bool {
    // SAFETY: `kill` with signal 0 sends nothing; it takes and returns plain numbers.
    unsafe { libc::kill(pid, 0) == 0 }
}
