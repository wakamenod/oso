//! Which file the channel may open (ADR 0085): only a regular file that is there, not too
//! large and with one name. Looked at on the connection's thread, as the UI thread never
//! waits on the file system.
//!
//! Must not depend on: editor state; it only reads a file's metadata.

/// The largest file the channel opens, in bytes: an agent reads what it opens, and memory
/// spent on a huge one is the person's.
pub const MAX_OPEN_BYTES: u64 = 256 << 20;

/// Why the file at `path` may not be opened for the channel, if it may not: there is none,
/// it is no regular file, it is too large, or it has more than one name, so that its other
/// names, which the secret patterns judge, are not seen (ADR 0085).
pub(super) fn may_open(path: &std::path::Path) -> Result<(), &'static str> {
    let meta = std::fs::metadata(path).map_err(|_| "no file is at that path")?;
    if !meta.is_file() {
        return Err("that is not a regular file");
    }
    if meta.len() > MAX_OPEN_BYTES {
        return Err("the file is larger than the agent may open; ask the person");
    }
    #[cfg(unix)]
    if std::os::unix::fs::MetadataExt::nlink(&meta) > 1 {
        return Err("the file has more than one name; ask the person");
    }
    Ok(())
}
