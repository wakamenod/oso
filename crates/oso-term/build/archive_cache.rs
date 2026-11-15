//! A cache of the built libghostty-vt archive in the target directory. Cargo runs a build
//! script once for each set of features in a package's dependency tree, so a build with
//! `oso-platform`'s `dev` and one without would each run Zig; the second takes the archive the
//! first built. The cache sits in the
//! profile directory that holds `OUT_DIR`, under one directory per key, the sha256 of what the
//! archive is built from. The archive's sha256 is recorded beside it when it is put there and
//! checked on a copy before it is used; anything else rebuilds.
//!
//! Must not depend on: a path outside that profile directory; it never follows a symbolic
//! link, and an error writing the cache only costs the next build its reuse.

use std::fs;
use std::path::{Path, PathBuf};

use super::sha256;

const ARCHIVE: &str = "libghostty-vt.a";
const SUM: &str = "libghostty-vt.a.sha256";

pub(crate) struct Cache {
    dir: PathBuf,
}

impl Cache {
    /// The cache for `key` in the profile directory of `out`, cargo's
    /// `<profile>/build/oso-term-<hash>/out`; `None` if `out` is not laid out so, or the
    /// profile directory or the cache's are not plain directories.
    pub(crate) fn new(out: &Path, key: &str) -> Option<Cache> {
        let mut up = out.ancestors().skip(2);
        let build = up.next()?;
        let profile = up.next()?;
        if build.file_name()? != "build" || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let root = profile.join("ghostty-vt-cache");
        let dir = root.join(key);
        let plain = plain_dir(profile) && [&root, &dir].iter().all(|d| absent_or_dir(d));
        plain.then_some(Cache { dir })
    }

    /// Copies the cached archive to `into` if it is there and its sha256 is the one recorded;
    /// whether it did.
    pub(crate) fn get(&self, into: &Path) -> bool {
        let (archive, sum) = (self.dir.join(ARCHIVE), self.dir.join(SUM));
        if !plain_dir(&self.dir) || !plain_file(&archive) || !plain_file(&sum) {
            return false;
        }
        let Ok(recorded) = fs::read_to_string(&sum) else {
            return false;
        };
        // The copy is what is checked and linked, so the cache cannot change it in between.
        if fs::copy(&archive, into).is_err() {
            return false;
        }
        if sha256(into) == recorded.trim() {
            return true;
        }
        let _ = fs::remove_file(into);
        false
    }

    /// Puts `archive` in the cache with its sha256, each file written beside its place and
    /// renamed into it.
    pub(crate) fn put(&self, archive: &Path) {
        if fs::create_dir_all(&self.dir).is_err() || !plain_dir(&self.dir) {
            return;
        }
        let pid = std::process::id();
        let sum = sha256(archive);
        let staged = self.dir.join(format!(".{ARCHIVE}.{pid}"));
        let staged_sum = self.dir.join(format!(".{SUM}.{pid}"));
        let put = fs::copy(archive, &staged).is_ok()
            && fs::rename(&staged, self.dir.join(ARCHIVE)).is_ok()
            && fs::write(&staged_sum, &sum).is_ok()
            && fs::rename(&staged_sum, self.dir.join(SUM)).is_ok();
        if !put {
            let _ = fs::remove_file(staged);
            let _ = fs::remove_file(staged_sum);
        }
    }
}

fn plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

fn plain_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

fn absent_or_dir(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(m) => m.file_type().is_dir(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}
