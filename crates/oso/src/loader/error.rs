//! Why a file could not be loaded, as the UI and the agent channel are told.
//!
//! Must not depend on: `app` or the loading thread's state.

use std::io;

use super::gib;

/// Why a file could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    /// There is no such file. Emacs visits it as a new, empty file.
    NotFound,
    /// A directory, a device or a pipe: only regular files are opened.
    NotAFile,
    /// The file holds this many bytes, more than the load may read. Nothing was read.
    TooLarge(u64),
    /// Reading failed.
    Io(io::Error),
    /// The loading thread panicked.
    Panicked,
    /// The file opened has more than one name, which a load for the agent channel refuses:
    /// its other names, which the secret patterns judge, are not seen (ADR 0085).
    #[cfg_attr(
        not(unix),
        expect(dead_code, reason = "only Unix counts a file's names")
    )]
    SeveralNames,
    /// The file opened is larger than a load for the agent channel takes (ADR 0085).
    TooLargeForTheAgent,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such file"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge(bytes) => write!(f, "{}, too large to open without asking", gib(*bytes)),
            Self::Io(e) => write!(f, "{e}"),
            Self::Panicked => f.write_str("loading failed"),
            Self::SeveralNames => f.write_str("the file has more than one name"),
            Self::TooLargeForTheAgent => f.write_str("the file is larger than the agent may open"),
        }
    }
}
