//! The private directory on systems without Unix modes, which today means Windows.
//! Nothing here can check who else may read it: privacy comes from the per-user place
//! the caller chose, whose access list the new directory inherits.
//!
//! Must not depend on: `control`, `process` or `text`.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Windows has no limit a process can set: nothing to do, and nothing to report.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on Unix, where it can fail"
)]
pub(super) fn disable_core_dumps() -> io::Result<()> {
    Ok(())
}

/// Whether a real directory is at `path`.
pub(super) fn existing_private(path: &Path) -> io::Result<bool> {
    check_private(path)?;
    Ok(path.exists())
}

/// There is no count of names to go by.
pub(super) fn has_one_name(_: &fs::Metadata) -> bool {
    true
}

/// There is no owner to compare by.
pub(super) fn is_this_users(_: &fs::Metadata) -> bool {
    true
}

/// `%LOCALAPPDATA%`, if it is an absolute path.
pub(super) fn state_home_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    var("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// `%APPDATA%\oso`, if `%APPDATA%` is an absolute path (ADR 0108).
pub(super) fn config_home_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    var("APPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|base| base.join("oso"))
}

/// There are no modes to go by: only the file is looked at, through links.
pub(super) fn who_can_change(path: &Path) -> io::Result<super::WhoCanChange> {
    Ok(super::WhoCanChange {
        file: super::Writers::Nobody,
        way: super::Writers::Nobody,
        metadata: fs::metadata(path)?,
    })
}

/// Makes sure the directory at `path` is a real directory, creating it if missing.
pub(super) fn open_private(path: &Path) -> io::Result<()> {
    check_private(path)?;
    fs::create_dir_all(path)?;
    check_private(path)
}

/// Whether what is at `path`, if anything, is a real directory.
pub(super) fn check_private(path: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refused: it is not a plain directory: {}", path.display()),
        ));
    }
    Ok(())
}
