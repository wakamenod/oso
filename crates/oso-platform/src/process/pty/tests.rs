//! Tests of terminals on Unix, with children of `/bin/sh`.
//!
//! Must not depend on: `control` or `text`.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::ExitStatusExt;
use std::process::Child;
use std::time::{Duration, Instant};

use super::{Master, Ready, Size, Slave, open, wake_pair};
use crate::process::{Executable, Sink, Source, Spawn, account_shell, spawn};

const SIZE: Size = Size {
    rows: 24,
    columns: 80,
    width: 0,
    height: 0,
};

/// How long a test waits for a child before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

fn request<'a>(
    sh: &'a Executable,
    args: &'a [OsString],
    set: &'a [(&'a str, &'a OsStr)],
    terminal: Option<&'a Slave>,
) -> Spawn<'a> {
    Spawn {
        executable: sh,
        args,
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Inherit,
        stderr: Sink::Inherit,
        own_process_group: false,
        set,
        terminal,
    }
}

fn sh() -> Executable {
    Executable::absolute("/bin/sh").unwrap()
}

/// `script` run by `/bin/sh` in a new terminal, with `set` added.
fn start_with(script: &str, set: &[(&str, &OsStr)]) -> (Master, Child) {
    let (master, slave) = open(SIZE).unwrap();
    let sh = sh();
    let args = ["-c".into(), script.into()];
    let child = spawn(&request(&sh, &args, set, Some(&slave))).unwrap();
    drop(slave);
    (master, child)
}

fn start(script: &str) -> (Master, Child) {
    start_with(script, &[])
}

/// What the child writes until `needle` appears, or the terminal ends if
/// `needle` is empty.
fn read_until(master: &Master, needle: &str) -> String {
    let (_waker, wakeup) = wake_pair().unwrap();
    let deadline = Instant::now() + PATIENCE;
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&seen).into_owned();
        if !needle.is_empty() && text.contains(needle) {
            return text;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "waited for {needle:?}, saw {text:?}");
        if !master.wait(&wakeup, false, Some(left)).unwrap().readable {
            continue;
        }
        match master.read(&mut buf) {
            Ok(0) if needle.is_empty() => return text,
            Ok(0) => panic!("the terminal ended before {needle:?}: {text:?}"),
            Ok(n) => seen.extend_from_slice(&buf[..n]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{error}"),
        }
    }
}

/// Ends a child that waits, as closing its terminal does.
fn hang_up(master: &Master, mut child: Child) {
    master.hang_up().unwrap();
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("the hang-up did not end the child: {:?}", child.wait());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(libc::SIGHUP), "{status:?}");
}

#[test]
fn a_child_runs_in_the_terminal_at_its_size() {
    let (master, mut child) = start(
        "stty size; test -t 0 && test -t 1 && test -t 2 && echo streams; echo ctty >/dev/tty",
    );
    let text = read_until(&master, "");
    assert!(text.contains("24 80"), "{text:?}");
    assert!(text.contains("streams"), "{text:?}");
    assert!(
        text.contains("ctty"),
        "the slave is its controlling terminal: {text:?}"
    );
    assert!(child.wait().unwrap().success());
}

#[test]
fn the_child_leads_its_session_and_a_hang_up_ends_it() {
    let (master, child) = start("echo ready; exec sleep 60");
    read_until(&master, "ready");
    assert_eq!(master.foreground_group().unwrap(), Some(child.id()));
    hang_up(&master, child);
}

#[test]
fn a_child_does_not_keep_the_spawning_threads_signal_mask() {
    let (master, child) = std::thread::spawn(|| {
        let mut blocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: `sigemptyset` fills `blocked`, `sigaddset` adds to it, and
        // `pthread_sigmask` reads it; it changes the mask of this short-lived thread
        // only.
        unsafe {
            libc::sigemptyset(blocked.as_mut_ptr());
            libc::sigaddset(blocked.as_mut_ptr(), libc::SIGHUP);
            libc::sigaddset(blocked.as_mut_ptr(), libc::SIGINT);
            libc::pthread_sigmask(libc::SIG_BLOCK, blocked.as_ptr(), std::ptr::null_mut());
        }
        start("echo ready; exec sleep 60")
    })
    .join()
    .unwrap();
    read_until(&master, "ready");
    hang_up(&master, child);
}

#[test]
fn a_resize_reaches_the_child() {
    let (master, child) = start("trap 'stty size' WINCH; echo ready; while :; do sleep 0.1; done");
    read_until(&master, "ready");
    master
        .resize(Size {
            rows: 30,
            columns: 100,
            width: 800,
            height: 600,
        })
        .unwrap();
    read_until(&master, "30 100");
    hang_up(&master, child);
}

#[test]
fn what_is_written_reaches_the_child() {
    let (master, mut child) = start("read line; echo \"got $line\"");
    let mut input: &[u8] = b"hello\n";
    while !input.is_empty() {
        match master.write(input) {
            Ok(n) => input = &input[n..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{error}"),
        }
    }
    read_until(&master, "got hello");
    assert!(child.wait().unwrap().success());
}

#[test]
fn echo_off_is_seen() {
    let (master, child) = start("echo ready; read line; stty -echo; echo off; exec sleep 60");
    read_until(&master, "ready");
    assert!(master.echoes().unwrap());
    master.write(b"\n").unwrap();
    read_until(&master, "off");
    assert!(!master.echoes().unwrap());
    hang_up(&master, child);
}

#[test]
fn an_idle_terminal_would_block() {
    let (master, _slave) = open(SIZE).unwrap();
    let error = master.read(&mut [0; 16]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn no_other_child_inherits_either_end() {
    let (master, slave) = open(SIZE).unwrap();
    let fds = [master.0.0.as_raw_fd(), slave.0.0.as_raw_fd()];
    let script = format!(
        "for fd in {} {}; do test -e /dev/fd/$fd && echo leaked $fd; done; true",
        fds[0], fds[1]
    );
    let sh = sh();
    let args = ["-c".into(), script.into()];
    let mut child = spawn(&Spawn {
        stdout: Sink::Piped,
        ..request(&sh, &args, &[], None)
    })
    .unwrap();
    let mut output = String::new();
    io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut output).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(output, "", "{fds:?}");
}

#[test]
fn variables_set_by_value_reach_the_child_over_the_allowlist() {
    let set = [
        ("TERM", OsStr::new("xterm-256color")),
        ("OSO_TEST_VALUE", OsStr::new("a b")),
    ];
    let (master, mut child) = start_with("echo \"[$TERM] [$OSO_TEST_VALUE]\"", &set);
    let text = read_until(&master, "]\r\n");
    assert!(text.contains("[xterm-256color] [a b]"), "{text:?}");
    assert!(child.wait().unwrap().success());
}

#[test]
fn some_names_may_not_be_set() {
    let sh = sh();
    for name in [
        "",
        "A=B",
        "A\0B",
        "PATH",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
    ] {
        let set = [(name, OsStr::new("x"))];
        let error = spawn(&request(&sh, &[], &set, None)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{name:?}");
    }
}

#[test]
fn a_child_in_a_terminal_takes_no_other_streams_or_group() {
    let (_master, slave) = open(SIZE).unwrap();
    let sh = sh();
    let base = request(&sh, &[], &[], Some(&slave));
    for request in [
        Spawn {
            stdout: Sink::Piped,
            ..base
        },
        Spawn {
            stderr: Sink::Null,
            ..base
        },
        Spawn {
            own_process_group: true,
            ..base
        },
        Spawn {
            stdin: Source::Piped,
            ..base
        },
    ] {
        let error = spawn(&request).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}

#[test]
fn a_waker_wakes_the_wait_and_never_blocks() {
    let (master, _slave) = open(SIZE).unwrap();
    let (waker, wakeup) = wake_pair().unwrap();
    let short = Some(Duration::from_millis(10));
    assert_eq!(
        master.wait(&wakeup, false, short).unwrap(),
        Ready::default()
    );
    let other = waker.clone();
    std::thread::spawn(move || other.wake().unwrap())
        .join()
        .unwrap();
    let ready = master.wait(&wakeup, false, Some(PATIENCE)).unwrap();
    assert!(ready.woken && !ready.readable, "{ready:?}");
    // Wakes not yet seen are one byte in the pipe.
    for _ in 0..1000 {
        waker.wake().unwrap();
    }
    let mut buf = [0u8; 16];
    assert_eq!(io::Read::read(&mut &wakeup.0.reader, &mut buf).unwrap(), 1);
    waker
        .0
        .0
        .pending
        .store(false, std::sync::atomic::Ordering::Release);
    waker.wake().unwrap();
    assert!(master.wait(&wakeup, false, short).unwrap().woken);
    assert!(
        !master.wait(&wakeup, false, short).unwrap().woken,
        "drained"
    );
    assert!(master.wait(&wakeup, true, short).unwrap().writable);
}

#[test]
fn a_dropped_waker_does_not_end_the_wait() {
    let (master, _slave) = open(SIZE).unwrap();
    let (waker, wakeup) = wake_pair().unwrap();
    drop(waker);
    let short = Some(Duration::from_millis(10));
    assert_eq!(
        master.wait(&wakeup, false, short).unwrap(),
        Ready::default()
    );
}

#[test]
fn a_descriptor_left_open_on_exec_reaches_no_terminal_child() {
    let file = std::fs::File::open("/dev/null").unwrap();
    // SAFETY: `dup` takes a descriptor, which `file` keeps open, and returns a new
    // one without close-on-exec, or -1.
    let fd = unsafe { libc::dup(file.as_raw_fd()) };
    assert!(fd > 2);
    // SAFETY: `fd` was just made, and nothing else owns it.
    let leaky = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let script = format!("test -e /dev/fd/{fd} && echo leaked; echo done");
    // A child outside a terminal shows that the check can see it.
    let sh = sh();
    let args = ["-c".into(), script.clone().into()];
    let mut child = spawn(&Spawn {
        stdout: Sink::Piped,
        ..request(&sh, &args, &[], None)
    })
    .unwrap();
    let mut output = String::new();
    io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut output).unwrap();
    child.wait().unwrap();
    assert_eq!(output, "leaked\ndone\n");
    let (master, mut child) = start(&script);
    let text = read_until(&master, "done");
    assert!(!text.contains("leaked"), "{text:?}");
    child.wait().unwrap();
    drop(leaky);
}

#[test]
fn hang_up_never_signals_everything_or_oso() {
    use super::unix::group_to_signal;
    assert_eq!(group_to_signal(0, 42), None);
    assert_eq!(group_to_signal(1, 42), None);
    assert_eq!(group_to_signal(42, 42), None);
    assert_eq!(group_to_signal(u32::MAX, 42), None);
    assert_eq!(group_to_signal(43, 42), Some(43));
}

#[test]
fn names_passed_by_name_are_refused_too() {
    let sh = sh();
    for name in ["LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "PATH"] {
        let error = spawn(&Spawn {
            inherit: &[name],
            stdin: Source::Closed,
            ..request(&sh, &[], &[], None)
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{name:?}");
    }
}

#[test]
fn the_account_shell_is_an_absolute_path_if_any() {
    if let Some(shell) = account_shell() {
        assert!(shell.is_absolute(), "{}", shell.display());
    }
}
