//! Starting a child on macOS and Linux with `posix_spawn`, by hand (ADR 0137): this process
//! never forks, and the child gets only the descriptors it is given.
//!
//! Must not depend on: `control` or `text`, or what the child runs; [`super`] has checked the
//! request and built its environment.
//!
//! - No fork: neither the memory map of a large process is copied nor are after-fork handlers
//!   run, so a spawn costs about the same on any thread and the child cannot stop in one.
//! - Only the descriptors the file actions name reach the child: macOS closes every other one
//!   as it starts the child (`POSIX_SPAWN_CLOEXEC_DEFAULT`), and Linux closes every one past
//!   them (`posix_spawn_file_actions_addclosefrom_np`, glibc 2.34 and up; musl has none and is
//!   not supported). So a descriptor this process made without close-on-exec, or marks only a
//!   moment later, reaches no child.
//! - A child in a terminal leads a session of its own, with the slave as its standard streams
//!   and its controlling terminal. Linux starts it with `POSIX_SPAWN_SETSID`, and opens the
//!   slave by name in the child, without `O_NOCTTY`, which a session leader with no terminal
//!   takes as its own. macOS gives a session a controlling terminal only on the child's own
//!   `TIOCSCTTY`, which `posix_spawn` cannot ask for: there this child alone is started by a
//!   fork, through the standard library, with a hook that sets it up and marks every other
//!   descriptor close-on-exec (`pty::Slave::run_in`).
//! - The child starts with no signal blocked, and `SIGPIPE` at its default action; a child in a
//!   terminal gets every signal a shell's job control needs at its default.
//! - A failed start comes back from `posix_spawn` itself, and leaves no child.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use super::{Child, Sink, Source, Spawn};
use crate::privacy::inheritance::INHERITED_FD;

#[cfg(test)]
mod tests;

#[cfg(all(target_os = "linux", not(target_env = "gnu")))]
compile_error!("oso on Linux needs glibc 2.34 or later to start processes (ADR 0137)");

// In libSystem since macOS 10.15, which every macOS process links; `libc` binds neither.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// Keeps the open descriptor `fd` in the child despite `POSIX_SPAWN_CLOEXEC_DEFAULT`.
    fn posix_spawn_file_actions_addinherit_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        fd: libc::c_int,
    ) -> libc::c_int;
    /// Changes the child's directory to `path`, which it copies.
    fn posix_spawn_file_actions_addchdir_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        path: *const libc::c_char,
    ) -> libc::c_int;
}

/// Signals a child in a terminal gets back at their default action, whatever oso was started
/// with: `nohup` and a shell's background jobs ignore some, and a terminal's shell must still
/// end on a hang-up and stop on Ctrl-Z.
const TERMINAL_SIGNALS: [libc::c_int; 10] = [
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

/// The first descriptor number no file action writes to: what a source below it is first
/// copied above, so that no file action overwrites a later one's source.
const FIRST_FREE: RawFd = INHERITED_FD + 1;

/// A process this module started.
#[derive(Debug)]
pub(super) struct Process {
    pid: libc::pid_t,
    /// How it ended, once waited for: from then on its ID may be another process's.
    status: Option<ExitStatus>,
}

/// Starts the child `request` asks for, with exactly the variables of `environment`, the last
/// of a name winning.
pub(super) fn start(
    request: &Spawn<'_>,
    environment: &[(OsString, OsString)],
) -> io::Result<Child> {
    #[cfg(target_os = "macos")]
    if let Some(slave) = request.terminal {
        return forked_in_terminal(request, slave, environment);
    }
    let program = c_string(request.executable.path().as_os_str())?;
    let args = std::iter::once(Ok(program.clone()))
        .chain(request.args.iter().map(|arg| c_string(arg)))
        .collect::<io::Result<Vec<_>>>()?;
    let variables = environment
        .iter()
        .map(|(name, value)| (name.as_os_str(), value.as_os_str()))
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(name, value)| {
            let mut pair = name.as_bytes().to_vec();
            pair.push(b'=');
            pair.extend_from_slice(value.as_bytes());
            c_string(OsStr::from_bytes(&pair))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let cwd = request
        .cwd
        .map(|cwd| c_string(cwd.as_os_str()))
        .transpose()?;

    let (mut actions, theirs, ours) = file_actions(request, cwd.as_deref())?;
    let attributes = attributes(request)?;
    let pid = spawn(&program, &args, &variables, &mut actions, &attributes)?;
    drop(theirs);
    Ok(Child {
        stdin: ours.stdin.map(Into::into),
        stdout: ours.stdout.map(Into::into),
        stderr: ours.stderr.map(Into::into),
        process: Process { pid, status: None },
    })
}

/// The file actions that give the child its descriptors and directory, with what the child
/// takes, closed here once it has started, and the ends of its pipes this process keeps.
fn file_actions(
    request: &Spawn<'_>,
    cwd: Option<&CStr>,
) -> io::Result<(Actions, Vec<OwnedFd>, Ours)> {
    let mut actions = Actions::new()?;
    let mut theirs = Vec::new();
    let mut ours = Ours::default();
    if let Some(terminal) = request.terminal {
        #[cfg(not(target_os = "macos"))]
        in_terminal(&mut actions, terminal)?;
        #[cfg(target_os = "macos")]
        let _ = terminal;
    } else {
        for (target, stream) in [
            (libc::STDIN_FILENO, Stream::from(request.stdin)),
            (libc::STDOUT_FILENO, Stream::from(request.stdout)),
            (libc::STDERR_FILENO, Stream::from(request.stderr)),
        ] {
            stream.place(target, &mut actions, &mut theirs, &mut ours)?;
        }
    }
    let first_closed = if let Some(passed) = request.pass_fd {
        let fd = above_targets(passed.fd, &mut theirs)?;
        actions.dup2(fd, INHERITED_FD)?;
        INHERITED_FD + 1
    } else {
        INHERITED_FD
    };
    if let Some(cwd) = cwd {
        actions.chdir(cwd)?;
    }
    #[cfg(not(target_os = "macos"))]
    actions.close_from(first_closed)?;
    #[cfg(target_os = "macos")]
    let _ = first_closed;
    Ok((actions, theirs, ours))
}

/// The attributes that set the child's signals, session and process group, and on macOS
/// close every descriptor the file actions do not name.
fn attributes(request: &Spawn<'_>) -> io::Result<Attributes> {
    let mut attributes = Attributes::new()?;
    let mut flags = libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF;
    #[cfg(target_os = "macos")]
    {
        flags |= libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
    }
    if request.terminal.is_some() {
        #[cfg(not(target_os = "macos"))]
        {
            flags |= libc::POSIX_SPAWN_SETSID;
        }
        attributes.default_signals(&TERMINAL_SIGNALS)?;
    } else {
        attributes.default_signals(&[libc::SIGPIPE])?;
    }
    if request.own_process_group {
        flags |= libc::POSIX_SPAWN_SETPGROUP;
        attributes.process_group(0)?;
    }
    attributes.no_signal_blocked()?;
    attributes.flags(flags)?;
    Ok(attributes)
}

/// `posix_spawn`, with arguments and variables as the system takes them.
fn spawn(
    program: &CStr,
    args: &[CString],
    variables: &[CString],
    actions: &mut Actions,
    attributes: &Attributes,
) -> io::Result<libc::pid_t> {
    let null_ended = |strings: &[CString]| {
        strings
            .iter()
            .map(|string| string.as_ptr().cast_mut())
            .chain(std::iter::once(std::ptr::null_mut()))
            .collect::<Vec<_>>()
    };
    let (args, variables) = (null_ended(args), null_ended(variables));
    let mut pid: libc::pid_t = 0;
    #[expect(
        clippy::disallowed_methods,
        reason = "this module is the one place allowed to spawn (ADR 0010, ADR 0137)"
    )]
    // SAFETY: `program` and every string `args` and `variables` point at are NUL-terminated
    // and outlive the call, and both lists end with a null pointer. `actions` and `attributes`
    // were initialized by their constructors and are not destroyed until dropped. `pid` is a
    // place for one `pid_t`. `posix_spawn` neither keeps nor writes through any of them but
    // `pid`.
    let error = unsafe {
        libc::posix_spawn(
            &raw mut pid,
            program.as_ptr(),
            actions.as_mut_ptr(),
            attributes.as_ptr(),
            args.as_ptr(),
            variables.as_ptr(),
        )
    };
    done(error).map(|()| pid)
}

/// Makes `slave` the child's standard streams, and, as `POSIX_SPAWN_SETSID` runs before the
/// file actions, its controlling terminal: the open has no `O_NOCTTY`.
#[cfg(not(target_os = "macos"))]
fn in_terminal(actions: &mut Actions, slave: &super::pty::Slave) -> io::Result<()> {
    actions.open(libc::STDIN_FILENO, slave.name(), libc::O_RDWR)?;
    for target in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        actions.dup2(libc::STDIN_FILENO, target)?;
    }
    Ok(())
}

/// Starts a child in a terminal on macOS by a fork, through the standard library, whose hook
/// makes `slave` its controlling terminal (ADR 0137).
#[cfg(target_os = "macos")]
fn forked_in_terminal(
    request: &Spawn<'_>,
    slave: &super::pty::Slave,
    environment: &[(OsString, OsString)],
) -> io::Result<Child> {
    #[expect(
        clippy::disallowed_methods,
        reason = "this module is the one place allowed to spawn (ADR 0010, ADR 0137)"
    )]
    let mut command = std::process::Command::new(request.executable.path());
    command
        .args(request.args)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)));
    if let Some(cwd) = request.cwd {
        command.current_dir(cwd);
    }
    slave.run_in(&mut command, &TERMINAL_SIGNALS)?;
    // The standard library's `Child` holds no pipe here, and dropping it neither waits nor
    // kills: the process is waited for by its ID from now on.
    let pid = command.spawn()?.id();
    Ok(Child {
        stdin: None,
        stdout: None,
        stderr: None,
        process: Process {
            pid: libc::pid_t::try_from(pid)
                .map_err(|_| io::Error::other("a process ID out of range"))?,
            status: None,
        },
    })
}

/// Where one of the child's standard streams comes from or goes.
#[derive(Clone, Copy)]
enum Stream {
    Inherit,
    Null,
    Piped,
}

impl From<Source> for Stream {
    fn from(source: Source) -> Self {
        match source {
            Source::Closed => Self::Null,
            Source::Piped => Self::Piped,
        }
    }
}

impl From<Sink> for Stream {
    fn from(sink: Sink) -> Self {
        match sink {
            Sink::Inherit => Self::Inherit,
            Sink::Piped => Self::Piped,
            Sink::Null => Self::Null,
        }
    }
}

/// The ends of the pipes this process keeps, by the stream of the child's they serve.
#[derive(Default)]
struct Ours {
    stdin: Option<OwnedFd>,
    stdout: Option<OwnedFd>,
    stderr: Option<OwnedFd>,
}

impl Stream {
    /// Adds what makes this the child's descriptor `target`.
    fn place(
        self,
        target: RawFd,
        actions: &mut Actions,
        theirs: &mut Vec<OwnedFd>,
        ours: &mut Ours,
    ) -> io::Result<()> {
        match self {
            Self::Inherit => actions.inherit(target),
            Self::Null => {
                let flags = if target == libc::STDIN_FILENO {
                    libc::O_RDONLY
                } else {
                    libc::O_WRONLY
                };
                actions.open(target, c"/dev/null", flags)
            }
            Self::Piped => {
                // Both ends are close-on-exec; the child's is placed by `dup2`.
                let (reader, writer) = io::pipe()?;
                let (child, kept) = if target == libc::STDIN_FILENO {
                    (OwnedFd::from(reader), OwnedFd::from(writer))
                } else {
                    (OwnedFd::from(writer), OwnedFd::from(reader))
                };
                let fd = above_targets(child.as_fd(), theirs)?;
                actions.dup2(fd, target)?;
                theirs.push(child);
                let slot = match target {
                    libc::STDIN_FILENO => &mut ours.stdin,
                    libc::STDOUT_FILENO => &mut ours.stdout,
                    _ => &mut ours.stderr,
                };
                *slot = Some(kept);
                Ok(())
            }
        }
    }
}

/// `fd`'s number if no file action writes to it, or else that of a close-on-exec copy above
/// them, which `theirs` keeps until the child has started.
fn above_targets(fd: BorrowedFd<'_>, theirs: &mut Vec<OwnedFd>) -> io::Result<RawFd> {
    if fd.as_raw_fd() >= FIRST_FREE {
        return Ok(fd.as_raw_fd());
    }
    // SAFETY: `F_DUPFD_CLOEXEC` copies a descriptor `fd` keeps open to a free number from
    // `FIRST_FREE` up, and touches no memory.
    let copy = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, FIRST_FREE) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `copy` was just made by the call above, and nothing else owns it.
    theirs.push(unsafe { OwnedFd::from_raw_fd(copy) });
    Ok(copy)
}

/// `string` as the system takes it, refused if it holds NUL.
fn c_string(string: &OsStr) -> io::Result<CString> {
    CString::new(string.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a program, argument, variable or directory holds NUL",
        )
    })
}

/// `Ok` if `error`, as the spawn functions return it, is none.
fn done(error: libc::c_int) -> io::Result<()> {
    if error == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(error))
    }
}

/// A spawn's file actions, destroyed when dropped. Boxed, so that they never move once set up.
struct Actions(Box<MaybeUninit<libc::posix_spawn_file_actions_t>>);

impl Actions {
    fn new() -> io::Result<Self> {
        let mut actions = Box::new(MaybeUninit::uninit());
        // SAFETY: `init` sets up the object it is given, which the box holds.
        done(unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) })?;
        Ok(Self(actions))
    }

    fn as_mut_ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        self.0.as_mut_ptr()
    }

    /// The child's `to` becomes a copy of `from`, without close-on-exec.
    fn dup2(&mut self, from: RawFd, to: RawFd) -> io::Result<()> {
        // SAFETY: the actions were set up by `new`; this takes two numbers.
        done(unsafe { libc::posix_spawn_file_actions_adddup2(self.as_mut_ptr(), from, to) })
    }

    /// The child's `to` becomes `path` opened with `flags`.
    fn open(&mut self, to: RawFd, path: &CStr, flags: libc::c_int) -> io::Result<()> {
        // SAFETY: the actions were set up by `new`; `path` is NUL-terminated, and the call
        // copies it.
        done(unsafe {
            libc::posix_spawn_file_actions_addopen(self.as_mut_ptr(), to, path.as_ptr(), flags, 0)
        })
    }

    /// The child keeps this process's `fd`, as it is.
    fn inherit(&mut self, fd: RawFd) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: the actions were set up by `new`; this takes a number.
            done(unsafe { posix_spawn_file_actions_addinherit_np(self.as_mut_ptr(), fd) })
        }
        // Linux closes only what `close_from` names, from 3 up.
        #[cfg(not(target_os = "macos"))]
        {
            let _ = fd;
            Ok(())
        }
    }

    /// The child runs in `dir`.
    fn chdir(&mut self, dir: &CStr) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        // SAFETY: the actions were set up by `new`; `dir` is NUL-terminated, and the call
        // copies it.
        let error =
            unsafe { posix_spawn_file_actions_addchdir_np(self.as_mut_ptr(), dir.as_ptr()) };
        #[cfg(not(target_os = "macos"))]
        // SAFETY: as above.
        let error =
            unsafe { libc::posix_spawn_file_actions_addchdir_np(self.as_mut_ptr(), dir.as_ptr()) };
        done(error)
    }

    /// The child closes every descriptor from `first` up.
    #[cfg(not(target_os = "macos"))]
    fn close_from(&mut self, first: RawFd) -> io::Result<()> {
        // SAFETY: the actions were set up by `new`; this takes a number.
        done(unsafe { libc::posix_spawn_file_actions_addclosefrom_np(self.as_mut_ptr(), first) })
    }
}

impl Drop for Actions {
    fn drop(&mut self) {
        // SAFETY: the actions were set up by `new`, and are destroyed only here.
        unsafe {
            libc::posix_spawn_file_actions_destroy(self.as_mut_ptr());
        }
    }
}

/// A spawn's attributes, destroyed when dropped. Boxed, so that they never move once set up.
struct Attributes(Box<MaybeUninit<libc::posix_spawnattr_t>>);

impl Attributes {
    fn new() -> io::Result<Self> {
        let mut attributes = Box::new(MaybeUninit::uninit());
        // SAFETY: `init` sets up the object it is given, which the box holds.
        done(unsafe { libc::posix_spawnattr_init(attributes.as_mut_ptr()) })?;
        Ok(Self(attributes))
    }

    fn as_ptr(&self) -> *const libc::posix_spawnattr_t {
        self.0.as_ptr()
    }

    fn as_mut_ptr(&mut self) -> *mut libc::posix_spawnattr_t {
        self.0.as_mut_ptr()
    }

    fn flags(&mut self, flags: libc::c_int) -> io::Result<()> {
        let flags = libc::c_short::try_from(flags)
            .map_err(|_| io::Error::other("the spawn's flags do not fit"))?;
        // SAFETY: the attributes were set up by `new`; this takes a number.
        done(unsafe { libc::posix_spawnattr_setflags(self.as_mut_ptr(), flags) })
    }

    /// With `POSIX_SPAWN_SETSIGMASK`: the child blocks no signal, whatever the spawning thread
    /// blocks.
    fn no_signal_blocked(&mut self) -> io::Result<()> {
        let none = signal_set(&[])?;
        // SAFETY: the attributes were set up by `new`; `none` is a set `signal_set` filled,
        // which the call copies.
        done(unsafe { libc::posix_spawnattr_setsigmask(self.as_mut_ptr(), &raw const none) })
    }

    /// With `POSIX_SPAWN_SETSIGDEF`: `signals` are at their default action in the child.
    fn default_signals(&mut self, signals: &[libc::c_int]) -> io::Result<()> {
        let set = signal_set(signals)?;
        // SAFETY: the attributes were set up by `new`; `set` is a set `signal_set` filled,
        // which the call copies.
        done(unsafe { libc::posix_spawnattr_setsigdefault(self.as_mut_ptr(), &raw const set) })
    }

    /// With `POSIX_SPAWN_SETPGROUP`: the child's process group, its own if 0.
    fn process_group(&mut self, group: libc::pid_t) -> io::Result<()> {
        // SAFETY: the attributes were set up by `new`; this takes a number.
        done(unsafe { libc::posix_spawnattr_setpgroup(self.as_mut_ptr(), group) })
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the attributes were set up by `new`, and are destroyed only here.
        unsafe {
            libc::posix_spawnattr_destroy(self.as_mut_ptr());
        }
    }
}

/// The set of `signals`.
fn signal_set(signals: &[libc::c_int]) -> io::Result<libc::sigset_t> {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` fills the set it is given, which `set` has room for, before
    // `sigaddset` reads and changes it.
    let filled = unsafe {
        libc::sigemptyset(set.as_mut_ptr()) == 0
            && signals
                .iter()
                .all(|&signal| libc::sigaddset(set.as_mut_ptr(), signal) == 0)
    };
    if !filled {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `sigemptyset` succeeded, so it filled the set.
    Ok(unsafe { set.assume_init() })
}

impl Process {
    pub(super) fn id(&self) -> u32 {
        self.pid.cast_unsigned()
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.reap(0)? {
                return Ok(status);
            }
        }
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.reap(libc::WNOHANG)
    }

    pub(super) fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: `kill` takes and returns plain numbers, and `pid` is this child's, which has
        // not been waited for, so no other process can have it.
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// How the child ended, waiting for it unless `flags` say not to; `None` if it runs.
    fn reap(&mut self, flags: libc::c_int) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let mut status: libc::c_int = 0;
        loop {
            // SAFETY: `waitpid` writes one `c_int` into `status`, which lives across the call.
            let reaped = unsafe { libc::waitpid(self.pid, &raw mut status, flags) };
            if reaped == self.pid {
                let status = ExitStatus::from_raw(status);
                self.status = Some(status);
                return Ok(Some(status));
            }
            if reaped == 0 {
                return Ok(None);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}
