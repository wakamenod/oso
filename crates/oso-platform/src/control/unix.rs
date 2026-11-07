//! The control socket on Unix: a Unix domain socket in a `0700` directory.
//!
//! Must not depend on: `text` or the protocol spoken over the socket.

// The only place allowed to open local sockets (clippy.toml).
#![expect(
    clippy::disallowed_types,
    reason = "the dev control socket is the one user of Unix domain sockets"
)]

use std::ffi::OsString;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{is_plain_name, owner_pid, socket_pid};
use crate::file_meta::seal;
use crate::privacy::{
    euid, itself_problem, list_problem, parent_is_safe, whole_parent_list_problem,
};

/// The private directory that holds control sockets and the files they
/// write. Every way to get one checks the directory first.
#[derive(Debug, Clone)]
pub struct ControlDir {
    path: PathBuf,
}

impl ControlDir {
    /// The default directory, created if missing.
    pub fn open() -> io::Result<Self> {
        Self::at(default_path(
            std::env::var_os("XDG_RUNTIME_DIR"),
            std::env::temp_dir(),
            euid(),
        )?)
    }

    /// The default directory, which must already exist. For clients, which
    /// have nothing to connect to otherwise.
    pub fn open_existing() -> io::Result<Self> {
        let path = default_path(
            std::env::var_os("XDG_RUNTIME_DIR"),
            std::env::temp_dir(),
            euid(),
        )?;
        Self::check_parent(&path)?;
        Self::checked(path)
    }

    /// The directory at `path`, created with mode `0700` if missing. Its
    /// parent must exist, and must not let other users replace the
    /// directory: not writable by group or others, or sticky like `/tmp`.
    pub fn at(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        Self::check_parent(&path)?;
        match DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(with_path(e, "cannot create", &path)),
        }
        Self::checked(path)
    }

    /// Refuses a parent where other users could rename the directory away
    /// and put their own in its place after it is checked.
    fn check_parent(path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        // All of this goes by the one place a link leads to.
        let real = fs::canonicalize(parent).map_err(|e| with_path(e, "cannot inspect", parent))?;
        let meta = fs::metadata(&real).map_err(|e| with_path(e, "cannot inspect", parent))?;
        if !parent_is_safe(meta.mode()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to use {} for the control socket: other users can write to it",
                    parent.display()
                ),
            ));
        }
        match list_problem(&real, whole_parent_list_problem)
            .map_err(|e| with_path(e, "cannot inspect", parent))?
        {
            Some(problem) => Err(refused(parent, problem.message())),
            None => Ok(()),
        }
    }

    /// Checks the directory without following a symlink at `path` itself.
    fn checked(path: PathBuf) -> io::Result<Self> {
        let meta =
            fs::symlink_metadata(&path).map_err(|e| with_path(e, "cannot inspect", &path))?;
        match itself_problem(&path, &meta).map_err(|e| with_path(e, "cannot inspect", &path))? {
            Some(problem) => Err(refused(&path, problem.message())),
            None => Ok(Self { path }),
        }
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Listens on `<pid>.sock` for this process. Files left by an earlier
    /// process with the same ID are removed first; other stale files are
    /// left to [`Self::remove_stale`], which may block.
    pub fn bind(&self) -> io::Result<ControlListener> {
        let pid = std::process::id();
        drop(CleanupGuard {
            dir: self.path.clone(),
            pid,
        });
        let socket = self.path.join(format!("{pid}.sock"));
        let listener =
            UnixListener::bind(&socket).map_err(|e| with_path(e, "cannot bind", &socket))?;
        // The directory already keeps other users out; this is a second wall.
        fs::set_permissions(&socket, Permissions::from_mode(0o600))?;
        Ok(ControlListener {
            listener,
            cleanup: CleanupGuard {
                dir: self.path.clone(),
                pid,
            },
        })
    }

    /// Connects to the instance with process ID `pid`, or to the only
    /// running instance if `pid` is `None`. Returns the instance's process
    /// ID and the stream.
    pub fn connect(&self, pid: Option<u32>) -> io::Result<(u32, ControlStream)> {
        if let Some(pid) = pid {
            let socket = self.path.join(format!("{pid}.sock"));
            let stream = UnixStream::connect(&socket)
                .map_err(|e| with_path(e, "cannot connect to", &socket))?;
            return Ok((pid, ControlStream::verified(stream)?));
        }
        let mut live = Vec::new();
        for pid in self.socket_pids()? {
            if let Ok(stream) = UnixStream::connect(self.path.join(format!("{pid}.sock"))) {
                live.push((pid, stream));
            }
        }
        match live.len() {
            1 => {
                let (pid, stream) = live.remove(0);
                Ok((pid, ControlStream::verified(stream)?))
            }
            0 => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no running oso dev instance found in {}",
                    self.path.display()
                ),
            )),
            _ => {
                let pids: Vec<String> = live.iter().map(|(pid, _)| pid.to_string()).collect();
                Err(io::Error::other(format!(
                    "several oso dev instances are running (pids {}); choose one with --pid",
                    pids.join(", ")
                )))
            }
        }
    }

    /// Writes `bytes` to a new file `<pid>-<name>` with mode `0600`, for
    /// this process. Fails if the file exists, without following symlinks.
    /// `name` must be a plain file name.
    pub fn write_private_file(&self, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        if !is_plain_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a plain file name: {name:?}"),
            ));
        }
        let path = self.path.join(format!("{}-{name}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| with_path(e, "cannot create", &path))?;
        if let Err(e) = seal(&file) {
            let _ = fs::remove_file(&path);
            return Err(with_path(e, "cannot use", &path));
        }
        // No fsync: the file is a short-lived dev artifact read right away.
        file.write_all(bytes)?;
        Ok(path)
    }

    /// The process IDs of the sockets in the directory, in ascending order.
    fn socket_pids(&self) -> io::Result<Vec<u32>> {
        let mut pids: Vec<u32> = self
            .file_names()?
            .iter()
            .filter_map(|name| socket_pid(name))
            .collect();
        pids.sort_unstable();
        Ok(pids)
    }

    /// The names of the directory's entries that are valid UTF-8.
    fn file_names(&self) -> io::Result<Vec<String>> {
        Ok(fs::read_dir(&self.path)?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect())
    }

    /// Removes the files of other instances that are gone: their sockets
    /// refuse connections, or they have no socket at all. This process's
    /// files are kept. Errors are ignored; a file that cannot be removed does
    /// no harm.
    ///
    /// Probing a socket may block (on Linux, while a stopped instance's
    /// backlog is full), so call this away from the UI thread. On macOS a
    /// full backlog refuses connections instead, so such a live instance
    /// would be treated as stale; this is rare enough to accept for a dev
    /// tool.
    pub fn remove_stale(&self) {
        let own_pid = std::process::id();
        let Ok(names) = self.file_names() else {
            return;
        };
        let live: Vec<u32> = names
            .iter()
            .filter_map(|name| socket_pid(name))
            .filter(|&pid| {
                pid == own_pid || UnixStream::connect(self.path.join(format!("{pid}.sock"))).is_ok()
            })
            .collect();
        for name in &names {
            if let Some(pid) = owner_pid(name)
                && pid != own_pid
                && !live.contains(&pid)
            {
                let _ = fs::remove_file(self.path.join(name));
            }
        }
    }
}

/// `$XDG_RUNTIME_DIR/oso` if that is an absolute path, otherwise
/// `oso-<euid>` in `temp_dir`, which must be absolute too. The user ID keeps
/// users apart in a shared `/tmp`.
fn default_path(
    xdg_runtime_dir: Option<OsString>,
    temp_dir: PathBuf,
    euid: u32,
) -> io::Result<PathBuf> {
    match xdg_runtime_dir.map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Ok(dir.join("oso")),
        _ if temp_dir.is_absolute() => Ok(temp_dir.join(format!("oso-{euid}"))),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "the temporary directory {} is not an absolute path",
                temp_dir.display()
            ),
        )),
    }
}

fn refused(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "refusing to use {} for the control socket: it {why}",
            path.display()
        ),
    )
}

fn with_path(e: io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("{what} {}: {e}", path.display()))
}

/// A listening control socket. Accepts only peers of the same user.
#[derive(Debug)]
pub struct ControlListener {
    listener: UnixListener,
    cleanup: CleanupGuard,
}

impl ControlListener {
    /// The socket's path.
    pub fn socket_path(&self) -> PathBuf {
        self.cleanup.dir.join(format!("{}.sock", self.cleanup.pid))
    }

    /// Waits for the next connection. A peer running as another user is
    /// disconnected and reported as [`io::ErrorKind::PermissionDenied`].
    pub fn accept(&self) -> io::Result<ControlStream> {
        let (stream, _) = self.listener.accept()?;
        ControlStream::verified(stream)
    }

    /// A guard that removes the socket and the files this process wrote
    /// when dropped. The listener itself may live on a thread that never
    /// returns, so the owner of the guard decides when to clean up.
    pub fn cleanup_guard(&self) -> CleanupGuard {
        self.cleanup.clone()
    }
}

/// Removes this process's socket and files from the control directory when
/// dropped.
#[derive(Debug, Clone)]
pub struct CleanupGuard {
    dir: PathBuf,
    pid: u32,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && owner_pid(name) == Some(self.pid)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// A connection whose peer runs as the same user.
#[derive(Debug)]
pub struct ControlStream(UnixStream);

impl ControlStream {
    fn verified(stream: UnixStream) -> io::Result<Self> {
        check_peer(peer_owner(&stream)?, euid())?;
        Ok(Self(stream))
    }

    /// Another handle to the same connection.
    pub fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    /// Limits how long a read may block. `None` blocks indefinitely.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }

    /// Limits how long a write may block. `None` blocks indefinitely.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_write_timeout(timeout)
    }

    /// A connected pair, for tests.
    #[doc(hidden)]
    pub fn pair() -> io::Result<(Self, Self)> {
        let (a, b) = UnixStream::pair()?;
        Ok((Self::verified(a)?, Self::verified(b)?))
    }
}

impl Read for ControlStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for ControlStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Accepts a peer only if it runs as `own`, the effective user ID of this
/// process. A root peer is refused unless this process runs as root.
fn check_peer(peer: u32, own: u32) -> io::Result<()> {
    if peer == own {
        Ok(())
    } else {
        // The user IDs are left out of the message, which is logged.
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refused a control socket peer running as another user",
        ))
    }
}

/// The user that owns the process at the other end of `stream`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_owner(stream: &UnixStream) -> io::Result<u32> {
    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let size = std::mem::size_of::<libc::ucred>();
    let mut len = libc::socklen_t::try_from(size).expect("ucred fits in socklen_t");
    // SAFETY: `fd` belongs to `stream`, which is borrowed for the call.
    // `cred` and `len` are initialized locals that outlive the call, and
    // `len` holds the size of `cred`, so the kernel writes within it.
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    if usize::try_from(len).ok() != Some(size) {
        return Err(io::Error::other("SO_PEERCRED returned a short ucred"));
    }
    Ok(cred.uid)
}

/// The user that owns the process at the other end of `stream`.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn peer_owner(stream: &UnixStream) -> io::Result<u32> {
    let fd = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `fd` belongs to `stream`, which is borrowed for the call, and
    // `uid` and `gid` are initialized locals that outlive the call.
    let ret = unsafe { libc::getpeereid(fd, &raw mut uid, &raw mut gid) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

/// Other Unix systems: no peer check is implemented, so every peer is
/// refused.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn peer_owner(_stream: &UnixStream) -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the control socket cannot check peers on this system",
    ))
}

#[cfg(test)]
mod tests {
    use super::{ControlDir, ControlStream, check_peer, default_path, euid, peer_owner};
    use std::ffi::OsString;
    use std::fs::{self, Permissions};
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh scratch directory for one test, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            // Keep it short: socket paths are limited to about 100 bytes.
            let path = std::env::temp_dir().join(format!("osoc-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn creates_the_directory_with_mode_0700() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        assert_eq!(mode(dir.path()), 0o700);
    }

    #[test]
    fn refuses_a_directory_with_mode_0755() {
        let scratch = Scratch::new();
        let path = scratch.join("c");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
        let err = ControlDir::at(&path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("mode is not 0700"), "{err}");
        // It is refused, not repaired.
        assert_eq!(mode(&path), 0o755);
    }

    #[test]
    fn refuses_a_symlink_to_a_private_directory() {
        let scratch = Scratch::new();
        let target = scratch.join("real");
        ControlDir::at(&target).unwrap();
        let link = scratch.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = ControlDir::at(&link).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("symlink"), "{err}");
    }

    #[test]
    fn refuses_a_file() {
        let scratch = Scratch::new();
        let path = scratch.join("c");
        fs::write(&path, b"").unwrap();
        let err = ControlDir::at(&path).unwrap_err();
        assert!(err.to_string().contains("not a directory"), "{err}");
    }

    #[test]
    fn refuses_a_directory_owned_by_another_user() {
        // Only root can create one, so this runs where `/` is owned by
        // another user, which is everywhere except as root.
        if euid() == 0 {
            return;
        }
        let err = ControlDir::at("/").unwrap_err();
        assert!(err.to_string().contains("owned by another user"), "{err}");
    }

    #[test]
    fn open_existing_does_not_create() {
        // The default directory may exist on a developer machine, so check
        // the check itself: a missing directory is an error.
        let scratch = Scratch::new();
        let err = ControlDir::checked(scratch.join("missing")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn default_path_prefers_an_absolute_xdg_runtime_dir() {
        let tmp = PathBuf::from("/tmp");
        assert_eq!(
            default_path(Some(OsString::from("/run/user/501")), tmp.clone(), 501).unwrap(),
            PathBuf::from("/run/user/501/oso")
        );
        assert_eq!(
            default_path(Some(OsString::from("relative")), tmp.clone(), 501).unwrap(),
            PathBuf::from("/tmp/oso-501")
        );
        assert_eq!(
            default_path(None, tmp, 501).unwrap(),
            PathBuf::from("/tmp/oso-501")
        );
    }

    #[test]
    fn default_path_refuses_a_relative_temp_dir() {
        let err = default_path(None, PathBuf::from("tmp"), 501).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_list_on_the_directory_or_its_parent_counts_as_it_does_for_any_private_directory() {
        use crate::file_meta::test_list::{clear, set_for_everyone};

        let scratch = Scratch::new();
        let parent = scratch.join("parent");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, Permissions::from_mode(0o755)).unwrap();
        let path = parent.join("c");

        set_for_everyone(&parent, "allow", "write");
        assert!(ControlDir::at(&path).is_err());
        assert!(!path.exists());
        set_for_everyone(&parent, "allow,directory_inherit", "read");
        assert!(ControlDir::at(&path).is_err());
        assert!(!path.exists());
        set_for_everyone(&parent, "deny", "delete");
        ControlDir::at(&path).unwrap();
        clear(&parent);

        set_for_everyone(&path, "allow", "read");
        let err = ControlDir::at(&path).unwrap_err();
        assert!(err.to_string().contains("access list"), "{err}");
        set_for_everyone(&path, "deny", "readsecurity");
        let err = ControlDir::at(&path).unwrap_err();
        clear(&path);
        assert!(err.to_string().contains("cannot inspect"), "{err}");
        set_for_everyone(&parent, "deny", "readsecurity");
        let err = ControlDir::at(&path).unwrap_err();
        clear(&parent);
        assert!(err.to_string().contains("cannot inspect"), "{err}");

        set_for_everyone(&path, "deny", "delete");
        ControlDir::at(&path).unwrap();
        clear(&path);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_list_taken_after_the_check_stops_a_private_file_and_leaves_nothing() {
        use crate::file_meta::test_list::{clear, set_for_everyone};

        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        set_for_everyone(dir.path(), "allow,file_inherit", "read");
        let err = dir.write_private_file("shot.png", b"made up").unwrap_err();
        assert!(err.to_string().contains("access list"), "{err}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

        // A deny taken the same way is only removed.
        set_for_everyone(dir.path(), "deny,file_inherit,only_inherit", "delete");
        let path = dir.write_private_file("shot.png", b"made up").unwrap();
        fs::remove_file(path).unwrap();
        clear(dir.path());
    }

    #[test]
    fn refuses_a_parent_other_users_can_write() {
        let scratch = Scratch::new();
        let parent = scratch.join("shared");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, Permissions::from_mode(0o777)).unwrap();
        let err = ControlDir::at(parent.join("c")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("other users can write"), "{err}");
        assert!(!parent.join("c").exists(), "nothing is created");
        // A sticky one, like /tmp, is fine.
        fs::set_permissions(&parent, Permissions::from_mode(0o1777)).unwrap();
        ControlDir::at(parent.join("c")).unwrap();
    }

    #[test]
    fn only_peers_of_the_same_user_are_accepted() {
        assert!(check_peer(501, 501).is_ok());
        assert!(check_peer(0, 0).is_ok());
        for (peer, own) in [(502, 501), (0, 501), (501, 0)] {
            let err = check_peer(peer, own).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::PermissionDenied, "{peer} {own}");
        }
    }

    #[test]
    fn bind_replaces_a_symlink_planted_at_its_socket_path() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let target = scratch.join("target");
        fs::write(&target, b"keep").unwrap();
        let own = dir.path().join(format!("{}.sock", std::process::id()));
        std::os::unix::fs::symlink(&target, &own).unwrap();
        let listener = dir.bind().unwrap();
        assert!(fs::symlink_metadata(&own).unwrap().file_type().is_socket());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        drop(listener);
    }

    #[test]
    fn binds_a_private_socket_and_accepts_the_same_user() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let listener = dir.bind().unwrap();
        let socket = listener.socket_path();
        assert_eq!(mode(&socket), 0o600);

        let (pid, mut client) = dir.connect(None).unwrap();
        assert_eq!(pid, std::process::id());
        let server = listener.accept().unwrap();
        client.write_all(b"ping\n").unwrap();
        let mut line = String::new();
        BufReader::new(server).read_line(&mut line).unwrap();
        assert_eq!(line, "ping\n");
    }

    #[test]
    fn peer_uid_is_our_own() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_owner(&a).unwrap(), euid());
        assert!(ControlStream::pair().is_ok());
    }

    #[test]
    fn connect_reports_no_instance() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let err = dir.connect(None).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn connect_asks_for_a_pid_when_several_run() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let _ours = dir.bind().unwrap();
        let _other = UnixListener::bind(dir.path().join("1.sock")).unwrap();
        let err = dir.connect(None).unwrap_err();
        assert!(err.to_string().contains("--pid"), "{err}");
        assert_eq!(dir.connect(Some(1)).unwrap().0, 1);
    }

    #[test]
    fn bind_removes_stale_sockets_and_their_files() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        // A socket whose listener is gone refuses connections.
        drop(UnixListener::bind(dir.path().join("7.sock")).unwrap());
        fs::write(dir.path().join("7-screenshot-1.png"), b"").unwrap();
        // Files of an instance with no socket at all.
        fs::write(dir.path().join("8-screenshot-1.png"), b"").unwrap();
        // A live instance and a file that belongs to no instance stay.
        let live = UnixListener::bind(dir.path().join("9.sock")).unwrap();
        fs::write(dir.path().join("9-screenshot-1.png"), b"").unwrap();
        fs::write(dir.path().join("notes.txt"), b"").unwrap();

        let _listener = dir.bind().unwrap();
        dir.remove_stale();
        let mut names = dir.file_names().unwrap();
        names.sort();
        let own = format!("{}.sock", std::process::id());
        let mut expected = vec![
            own,
            "9-screenshot-1.png".to_owned(),
            "9.sock".to_owned(),
            "notes.txt".to_owned(),
        ];
        expected.sort();
        assert_eq!(names, expected);
        drop(live);
    }

    #[test]
    fn cleanup_removes_only_our_files() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let listener = dir.bind().unwrap();
        let shot = dir.write_private_file("screenshot-1.png", b"png").unwrap();
        fs::write(dir.path().join("9-screenshot-1.png"), b"").unwrap();
        drop(listener.cleanup_guard());
        assert!(!shot.exists());
        assert!(!listener.socket_path().exists());
        assert!(dir.path().join("9-screenshot-1.png").exists());
    }

    #[test]
    fn private_files_are_0600_and_never_overwritten() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let path = dir.write_private_file("a.png", b"one").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read(&path).unwrap(), b"one");
        let err = dir.write_private_file("a.png", b"two").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"one");
    }

    #[test]
    fn private_files_do_not_follow_symlinks() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let target = scratch.join("target");
        let name = format!("{}-b.png", std::process::id());
        std::os::unix::fs::symlink(&target, dir.path().join(name)).unwrap();
        assert!(dir.write_private_file("b.png", b"x").is_err());
        assert!(!target.exists());
    }

    #[test]
    fn private_files_need_plain_names() {
        let scratch = Scratch::new();
        let dir = ControlDir::at(scratch.join("c")).unwrap();
        let err = dir.write_private_file("../x.png", b"x").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }
}
