//! Pseudo-terminals on macOS and Linux, over `libc`.
//!
//! Must not depend on: `control` or `text`.
//!
//! The master comes from `posix_openpt` with `O_CLOEXEC` rather than `openpty`,
//! which opens both ends without it: a spawn on another thread before they were
//! marked would hand them to a child that does not own the terminal (ADR 0120,
//! as built).

use std::ffi::{CStr, OsStr};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{Ready, Size};

#[derive(Debug)]
pub(super) struct Master(pub(super) File);

#[derive(Debug)]
pub(super) struct Slave(pub(super) OwnedFd);

#[derive(Debug, Clone)]
pub(super) struct Waker(pub(super) Arc<Wake>);

#[derive(Debug)]
pub(super) struct Wakeup {
    pub(super) reader: File,
    /// Kept, so that the pipe never ends while a thread waits on it.
    wake: Arc<Wake>,
}

/// The pipe's write end, and whether a wake is in it not yet seen. At most one
/// byte is in the pipe at a time.
#[derive(Debug)]
pub(super) struct Wake {
    writer: File,
    pub(super) pending: AtomicBool,
}

/// `TIOCSCTTY`, as `ioctl` takes it: macOS gives it as a narrower type.
#[cfg(target_os = "macos")]
const SET_CONTROLLING_TERMINAL: libc::c_ulong = libc::TIOCSCTTY as libc::c_ulong;
#[cfg(not(target_os = "macos"))]
const SET_CONTROLLING_TERMINAL: libc::Ioctl = libc::TIOCSCTTY;

/// `TIOCSIG`, as `ioctl` takes it. Only macOS sends `SIGHUP` with it: Linux's
/// accepts only `SIGINT`, `SIGQUIT` and `SIGTSTP`.
#[cfg(target_os = "macos")]
const SEND_SIGNAL: libc::c_ulong = libc::TIOCSIG as libc::c_ulong;

// libnotify is part of libSystem, which every macOS process links. No crate binds it.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// Whether `token` names one of this process's registrations. Its first call sets
    /// libnotify up.
    fn notify_is_valid_token(token: libc::c_int) -> bool;
}

/// Signals a child gets back at their default action, whatever oso was started
/// with: `nohup` and a shell's background jobs ignore some, and a terminal's
/// shell must still end on a hang-up and stop on Ctrl-Z.
const DEFAULT_SIGNALS: [libc::c_int; 10] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
    libc::SIGCHLD,
    libc::SIGALRM,
    libc::SIGPIPE,
];

/// The most descriptors the child marks close-on-exec one by one, where the
/// system cannot mark them all at once.
const MAX_MARKED: libc::c_int = 1 << 20;

/// XNU's `EREDRIVEOPEN`, "open again": an open of `/dev/ptmx` that races others can return it
/// as its error number, which no other system gives.
const OPEN_AGAIN: libc::c_int = -6;

/// How many times [`open_master`] opens `/dev/ptmx` while it says to open again.
const OPEN_TRIES: usize = 8;

/// The longest name of a terminal's slave: macOS's `TIOCPTYGNAME` fills 128
/// bytes.
const NAME_LEN: usize = 128;

pub(super) fn open(size: Size) -> io::Result<(Master, Slave)> {
    let master = open_master()?;
    // SAFETY: both take a descriptor, which `master` keeps open, and nothing else.
    let unlocked = unsafe {
        libc::grantpt(master.as_raw_fd()) == 0 && libc::unlockpt(master.as_raw_fd()) == 0
    };
    if !unlocked {
        return Err(io::Error::last_os_error());
    }
    let name = slave_name(&master)?;
    // `OpenOptions` opens with `O_CLOEXEC`. `O_NOCTTY` keeps the slave from becoming
    // oso's own controlling terminal.
    let slave = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(OsStr::from_bytes(name.to_bytes()))?;
    let master = Master(File::from(master));
    set_non_blocking(&master.0)?;
    master.resize(size)?;
    Ok((master, Slave(slave.into())))
}

/// A new master, close-on-exec. On macOS an open of `/dev/ptmx` that races others may fail
/// with XNU's own "open again" code: it opens again a few times, and then says that no
/// terminal could be opened.
fn open_master() -> io::Result<OwnedFd> {
    for _ in 0..OPEN_TRIES {
        // SAFETY: `posix_openpt` takes only flags, and returns a new descriptor or -1.
        let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
        if fd >= 0 {
            // SAFETY: `fd` was just opened, is valid, and nothing else owns it.
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(OPEN_AGAIN) {
            return Err(error);
        }
    }
    Err(io::Error::other("no terminal could be opened"))
}

/// The path of the slave that belongs to `master`.
fn slave_name(master: &OwnedFd) -> io::Result<std::ffi::CString> {
    let mut buf = [0u8; NAME_LEN];
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `TIOCPTYGNAME` writes at most 128 bytes, NUL-terminated, into its
        // argument, and `buf` holds 128 writable bytes.
        let done = unsafe {
            libc::ioctl(
                master.as_raw_fd(),
                libc::TIOCPTYGNAME.into(),
                buf.as_mut_ptr(),
            )
        };
        if done != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: `ptsname_r` writes at most `buf.len()` bytes, NUL-terminated, into
        // `buf`, which holds that many writable bytes.
        let error =
            unsafe { libc::ptsname_r(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
    }
    CStr::from_bytes_until_nul(&buf)
        .map(CStr::to_owned)
        .map_err(|_| io::Error::other("the terminal's name has no end"))
}

/// Sets `O_NONBLOCK` on `file`.
fn set_non_blocking(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    // SAFETY: `F_GETFL` and `F_SETFL` read and set the flags of `fd`, which `file`
    // keeps open, and touch no memory.
    let done = unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
    };
    if done {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

impl Master {
    pub(super) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        match (&self.0).read(buf) {
            // Linux says EIO once no process holds the slave; macOS says 0.
            Err(error) if error.raw_os_error() == Some(libc::EIO) => Ok(0),
            other => other,
        }
    }

    pub(super) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        (&self.0).write(buf)
    }

    pub(super) fn resize(&self, size: Size) -> io::Result<()> {
        let winsize = libc::winsize {
            ws_row: size.rows,
            ws_col: size.columns,
            ws_xpixel: size.width,
            ws_ypixel: size.height,
        };
        // SAFETY: `TIOCSWINSZ` reads one `winsize` from its argument, which points to
        // one that lives across the call.
        let done = unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCSWINSZ, &raw const winsize) };
        if done == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn echoes(&self) -> io::Result<bool> {
        let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `tcgetattr` fills the `termios` it is given, which `termios` has
        // room for.
        if unsafe { libc::tcgetattr(self.0.as_raw_fd(), termios.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `tcgetattr` succeeded, so it filled `termios`.
        let termios = unsafe { termios.assume_init() };
        Ok(termios.c_lflag & libc::ECHO != 0)
    }

    pub(super) fn foreground_group(&self) -> io::Result<Option<u32>> {
        // SAFETY: `tcgetpgrp` takes a descriptor, which `self` keeps open, and nothing
        // else.
        let group = unsafe { libc::tcgetpgrp(self.0.as_raw_fd()) };
        if group < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(u32::try_from(group).ok().filter(|&group| group > 0))
    }

    pub(super) fn hang_up(&self) -> io::Result<()> {
        // SAFETY: on macOS, `TIOCSIG` takes the signal's number by value and signals
        // the terminal's foreground process group, which the kernel picks, so no
        // group number can go stale between reading and signalling it.
        #[cfg(target_os = "macos")]
        if unsafe { libc::ioctl(self.0.as_raw_fd(), SEND_SIGNAL, libc::SIGHUP) } == 0 {
            return Ok(());
        }
        // Where the master cannot signal, as on Linux: by the group's number.
        let Some(group) = self.foreground_group()? else {
            return Ok(());
        };
        // SAFETY: `getpgrp` takes nothing and cannot fail.
        let own = unsafe { libc::getpgrp() };
        let Some(group) = group_to_signal(group, own) else {
            return Ok(());
        };
        // SAFETY: `kill` takes and returns plain numbers, and `group` is neither oso's
        // own nor one `kill` reads as more than one group.
        if unsafe { libc::kill(-group, libc::SIGHUP) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn wait(
        &self,
        wakeup: &Wakeup,
        writable: bool,
        timeout: Option<Duration>,
    ) -> io::Result<Ready> {
        let mut fds = [
            libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN | if writable { libc::POLLOUT } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: wakeup.reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout = timeout.map_or(-1, |timeout| {
            // Rounded up, so a short wait does not spin.
            let millis = timeout.as_nanos().div_ceil(1_000_000);
            libc::c_int::try_from(millis).unwrap_or(libc::c_int::MAX)
        });
        // SAFETY: `fds` holds `fds.len()` initialized `pollfd`s, which `poll` reads
        // and whose `revents` it writes, and lives across the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            return match error.kind() {
                io::ErrorKind::Interrupted => Ok(Ready::default()),
                _ => Err(error),
            };
        }
        let [master, wake] = fds.map(|fd| fd.revents);
        if (master | wake) & libc::POLLNVAL != 0 {
            return Err(io::Error::other("a descriptor waited for is not open"));
        }
        let woken = wake & libc::POLLIN != 0;
        if woken {
            wakeup.drain()?;
        }
        Ok(Ready {
            readable: master & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0,
            writable: master & libc::POLLOUT != 0,
            woken,
        })
    }
}

pub(super) fn wake_pair() -> io::Result<(Waker, Wakeup)> {
    let (reader, writer) = io::pipe()?;
    let (reader, writer) = (
        File::from(OwnedFd::from(reader)),
        File::from(OwnedFd::from(writer)),
    );
    set_non_blocking(&reader)?;
    set_non_blocking(&writer)?;
    let wake = Arc::new(Wake {
        writer,
        pending: AtomicBool::new(false),
    });
    Ok((Waker(Arc::clone(&wake)), Wakeup { reader, wake }))
}

impl Waker {
    pub(super) fn wake(&self) -> io::Result<()> {
        if self.0.pending.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        match (&self.0.writer).write(&[1]) {
            Ok(_) => Ok(()),
            // A full pipe will wake the thread anyway.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(error) => {
                self.0.pending.store(false, Ordering::Release);
                Err(error)
            }
        }
    }
}

impl Wakeup {
    /// Reads the wake not yet seen. The pipe is emptied before the wake is marked
    /// seen, so a wake that comes meanwhile either finds it still pending, and its
    /// work is read after this wait, or writes again for the next wait.
    fn drain(&self) -> io::Result<()> {
        let mut buf = [0u8; 16];
        loop {
            match (&self.reader).read(&mut buf) {
                Ok(n) if n == buf.len() => {}
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    // So that later wakes write again rather than stop for good.
                    self.wake.pending.store(false, Ordering::Release);
                    return Err(error);
                }
            }
        }
        self.wake.pending.store(false, Ordering::Release);
        Ok(())
    }
}

/// The group `hang_up` signals by number, given the terminal's foreground group
/// and oso's own: never 0 or 1, which `kill` reads as oso's group and every
/// process, and never oso's.
pub(super) fn group_to_signal(group: u32, own: libc::pid_t) -> Option<libc::pid_t> {
    libc::pid_t::try_from(group)
        .ok()
        .filter(|&group| group > 1 && group != own)
}

pub(super) fn run_in(slave: &Slave, command: &mut Command) -> io::Result<()> {
    // A fork while another thread sets libnotify up for the first time kills the child in
    // libSystem's after-fork handler, before `exec`, and the terminal ends with no output.
    // Asking libnotify anything first waits for that set-up and leaves it done.
    #[cfg(target_os = "macos")]
    // SAFETY: `notify_is_valid_token` takes a plain number and reads only libnotify's own
    // state, which it sets up on its first call, behind its own lock.
    unsafe {
        notify_is_valid_token(0);
    }
    // Each is a new close-on-exec copy, which the spawn puts at 0, 1 and 2.
    let stream = || slave.0.try_clone().map(Stdio::from);
    command.stdin(stream()?).stdout(stream()?).stderr(stream()?);
    let last = open_limit();
    #[expect(
        clippy::disallowed_methods,
        reason = "the terminal's child-side hook, in the process module (ADR 0120)"
    )]
    // SAFETY: the hook runs in the child between `fork` and `exec`, where only
    // async-signal-safe functions may be called. It calls only `setsid`, `ioctl`,
    // `signal`, `sigemptyset`, `sigprocmask`, `syscall` (`close_range`) and `fcntl`,
    // which are, on a `sigset_t` on its own stack that `sigemptyset` fills before
    // `sigprocmask` reads it. It allocates nothing
    // (an OS error is a number), and takes no lock. By then the standard library has
    // made the slave the child's descriptors 0, 1 and 2, as it sets up the standard
    // streams before it runs hooks. Marking the rest close-on-exec, rather than
    // closing them, keeps the standard library's own pipe that reports a failed
    // `exec`, which is close-on-exec already.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 || libc::ioctl(0, SET_CONTROLLING_TERMINAL, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            for signal in DEFAULT_SIGNALS {
                libc::signal(signal, libc::SIG_DFL);
            }
            // The child starts with the spawning thread's signal mask, which may
            // block what the shell must receive.
            let mut none = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(none.as_mut_ptr());
            libc::sigprocmask(libc::SIG_SETMASK, none.as_ptr(), std::ptr::null_mut());
            // A descriptor another thread made without close-on-exec, and was about
            // to mark, must not reach a shell that outlives it.
            #[cfg(target_os = "linux")]
            let marked = libc::syscall(
                libc::SYS_close_range,
                3,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) == 0;
            #[cfg(not(target_os = "linux"))]
            let marked = false;
            if !marked {
                for fd in 3..last {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }
    Ok(())
}

/// One past the highest descriptor this process may open, at most
/// [`MAX_MARKED`].
fn open_limit() -> libc::c_int {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` fills the `rlimit` it is given, which lives across the
    // call.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } != 0 {
        return MAX_MARKED;
    }
    libc::c_int::try_from(limit.rlim_cur)
        .map_or(MAX_MARKED, |limit| limit.min(MAX_MARKED))
        .min(max_files_per_process())
}

/// Where the system sets no limit of its own beside the soft one.
#[cfg(not(target_os = "macos"))]
fn max_files_per_process() -> libc::c_int {
    MAX_MARKED
}

/// The most descriptors macOS lets one process open, whatever its soft limit
/// says, or [`MAX_MARKED`] if it cannot tell.
#[cfg(target_os = "macos")]
fn max_files_per_process() -> libc::c_int {
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: `sysctlbyname` reads the NUL-terminated name, writes at most `size`
    // bytes into `value`, which has room for them, sets `size` to what it wrote, and
    // is given no new value.
    let done = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&raw mut value).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if done && size == std::mem::size_of::<libc::c_int>() && value > 0 {
        value
    } else {
        MAX_MARKED
    }
}
