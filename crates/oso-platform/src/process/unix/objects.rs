//! The file actions and attributes `posix_spawn` takes, as values that destroy themselves
//! (ADR 0137), for `process::unix`.
//!
//! Must not depend on: `control` or `text`, or what the child runs.

use std::ffi::CStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;

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

/// `Ok` if `error`, as the spawn functions return it, is none.
pub(super) fn done(error: libc::c_int) -> io::Result<()> {
    if error == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(error))
    }
}

/// A spawn's file actions, destroyed when dropped. Boxed, so that they never move once set up.
pub(super) struct Actions(Box<MaybeUninit<libc::posix_spawn_file_actions_t>>);

impl Actions {
    pub(super) fn new() -> io::Result<Self> {
        let mut actions = Box::new(MaybeUninit::uninit());
        // SAFETY: `init` sets up the object it is given, which the box holds.
        done(unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) })?;
        Ok(Self(actions))
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        self.0.as_mut_ptr()
    }

    /// The child's `to` becomes a copy of `from`, without close-on-exec.
    pub(super) fn dup2(&mut self, from: RawFd, to: RawFd) -> io::Result<()> {
        // SAFETY: the actions were set up by `new`; this takes two numbers.
        done(unsafe { libc::posix_spawn_file_actions_adddup2(self.as_mut_ptr(), from, to) })
    }

    /// The child's `to` becomes `path` opened with `flags`.
    pub(super) fn open(&mut self, to: RawFd, path: &CStr, flags: libc::c_int) -> io::Result<()> {
        // SAFETY: the actions were set up by `new`; `path` is NUL-terminated, and the call
        // copies it.
        done(unsafe {
            libc::posix_spawn_file_actions_addopen(self.as_mut_ptr(), to, path.as_ptr(), flags, 0)
        })
    }

    /// The child keeps this process's `fd`, as `exec` would: not if it is close-on-exec, or not
    /// open.
    pub(super) fn inherit(&mut self, fd: RawFd) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            // The inherit action clears close-on-exec in the child, so one that is set is
            // left out, and closed by `POSIX_SPAWN_CLOEXEC_DEFAULT`.
            // SAFETY: `F_GETFD` reads the flags of a descriptor number and touches no memory.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 || flags & libc::FD_CLOEXEC != 0 {
                return Ok(());
            }
            // SAFETY: the actions were set up by `new`; this takes a number.
            done(unsafe { posix_spawn_file_actions_addinherit_np(self.as_mut_ptr(), fd) })
        }
        // Linux closes only from 3 up (`close_from`), and `exec` closes what is close-on-exec.
        #[cfg(target_os = "linux")]
        {
            let _ = fd;
            Ok(())
        }
    }

    /// The child runs in `dir`.
    pub(super) fn chdir(&mut self, dir: &CStr) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        // SAFETY: the actions were set up by `new`; `dir` is NUL-terminated, and the call
        // copies it.
        let error =
            unsafe { posix_spawn_file_actions_addchdir_np(self.as_mut_ptr(), dir.as_ptr()) };
        #[cfg(target_os = "linux")]
        // SAFETY: as above.
        let error =
            unsafe { libc::posix_spawn_file_actions_addchdir_np(self.as_mut_ptr(), dir.as_ptr()) };
        done(error)
    }

    /// The child closes every descriptor from `first` up.
    #[cfg(target_os = "linux")]
    pub(super) fn close_from(&mut self, first: RawFd) -> io::Result<()> {
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
pub(super) struct Attributes(Box<MaybeUninit<libc::posix_spawnattr_t>>);

impl Attributes {
    pub(super) fn new() -> io::Result<Self> {
        let mut attributes = Box::new(MaybeUninit::uninit());
        // SAFETY: `init` sets up the object it is given, which the box holds.
        done(unsafe { libc::posix_spawnattr_init(attributes.as_mut_ptr()) })?;
        Ok(Self(attributes))
    }

    pub(super) fn as_ptr(&self) -> *const libc::posix_spawnattr_t {
        self.0.as_ptr()
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut libc::posix_spawnattr_t {
        self.0.as_mut_ptr()
    }

    pub(super) fn flags(&mut self, flags: libc::c_int) -> io::Result<()> {
        let flags = libc::c_short::try_from(flags)
            .map_err(|_| io::Error::other("the spawn's flags do not fit"))?;
        // SAFETY: the attributes were set up by `new`; this takes a number.
        done(unsafe { libc::posix_spawnattr_setflags(self.as_mut_ptr(), flags) })
    }

    /// With `POSIX_SPAWN_SETSIGMASK`: the child blocks no signal, whatever the spawning thread
    /// blocks.
    pub(super) fn no_signal_blocked(&mut self) -> io::Result<()> {
        let none = signal_set(&[])?;
        // SAFETY: the attributes were set up by `new`; `none` is a set `signal_set` filled,
        // which the call copies.
        done(unsafe { libc::posix_spawnattr_setsigmask(self.as_mut_ptr(), &raw const none) })
    }

    /// With `POSIX_SPAWN_SETSIGDEF`: `signals` are at their default action in the child.
    pub(super) fn default_signals(&mut self, signals: &[libc::c_int]) -> io::Result<()> {
        let set = signal_set(signals)?;
        // SAFETY: the attributes were set up by `new`; `set` is a set `signal_set` filled,
        // which the call copies.
        done(unsafe { libc::posix_spawnattr_setsigdefault(self.as_mut_ptr(), &raw const set) })
    }

    /// With `POSIX_SPAWN_SETPGROUP`: the child's process group, its own if 0.
    pub(super) fn process_group(&mut self, group: libc::pid_t) -> io::Result<()> {
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
